use codestory_core::{AccessKind, EdgeKind, NodeId, NodeKind, OccurrenceKind};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_search::SearchEngine;
use codestory_storage::Storage;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn run_incremental_indexing(
    root: &Path,
    storage: &mut Storage,
    files_to_index: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_project::RefreshInfo {
        files_to_index,
        files_to_remove: vec![],
    };
    indexer.run_incremental(storage, &refresh_info, &event_bus, None)?;
    Ok(())
}

fn index_project(files: &[(&str, &str)]) -> anyhow::Result<Storage> {
    let dir = tempdir()?;
    let root = dir.path();
    let mut indexed_files = Vec::with_capacity(files.len());
    for (relative_path, contents) in files {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
        indexed_files.push(path);
    }

    let mut storage = Storage::new_in_memory()?;
    run_incremental_indexing(root, &mut storage, indexed_files)?;
    Ok(storage)
}

#[test]
fn test_integration_full_loop() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "main.cpp",
        r#"
class MyClass {
public:
    void myMethod() {}
};

int main() {
    MyClass obj;
    obj.myMethod();
    return 0;
}
"#,
    )])?;

    // 3. Verify Storage
    let nodes = storage.get_nodes()?;

    // Find MyClass
    let my_class = nodes
        .iter()
        .find(|n| n.serialized_name.ends_with("MyClass"))
        .expect("MyClass node not found");
    assert_eq!(my_class.kind, NodeKind::CLASS);

    // Find myMethod
    let my_method = nodes
        .iter()
        .find(|n| {
            n.serialized_name.ends_with("myMethod")
                && matches!(n.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        })
        .expect("myMethod node not found");
    // In our indexer, methods currently default to FUNCTION if not using specific METHOD kind in TS graph
    // But let's check what it actually is

    // 4. Verify Edges (CALL)
    let edges = storage.get_edges()?;
    let call_edge = edges
        .iter()
        .find(|e| e.kind == EdgeKind::CALL)
        .expect("CALL edge not found");
    assert!(
        call_edge.resolved_target == Some(my_method.id),
        "CALL edge not resolved to myMethod"
    );
    assert!(call_edge.line.is_some(), "CALL edge missing line metadata");

    // 5. Verify Occurrences
    let occs = storage.get_occurrences_for_element(my_method.id.0)?;
    assert!(!occs.is_empty(), "No occurrences for myMethod");

    // Check if we have at least one DEFINITION
    let has_def = occs.iter().any(|o| o.kind == OccurrenceKind::DEFINITION);

    assert!(has_def, "Method definition occurrence missing");

    // Metadata on nodes should be populated
    assert!(my_method.file_node_id.is_some());
    assert!(my_method.start_line.is_some());
    assert!(my_method.end_line.is_some());

    Ok(())
}

#[test]
fn test_incremental_indexing_modification() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();

    // 1. Create initial file
    let f1 = root.join("file1.rs");
    fs::write(&f1, "struct Foo {}")?;

    let mut storage = Storage::new_in_memory()?;
    // 2. Index initial version
    run_incremental_indexing(root, &mut storage, vec![f1.clone()])?;

    let nodes = storage.get_nodes()?;
    assert!(nodes.iter().any(|n| n.serialized_name == "Foo"));
    let initial_count = nodes.len();

    // 3. Modify file to add new symbol
    fs::write(&f1, "struct Foo {}\nstruct Bar {}")?;

    run_incremental_indexing(root, &mut storage, vec![f1.clone()])?;

    // 4. Verify both symbols exist
    let nodes = storage.get_nodes()?;
    assert!(nodes.iter().any(|n| n.serialized_name == "Foo"));
    assert!(nodes.iter().any(|n| n.serialized_name == "Bar"));
    assert!(nodes.len() > initial_count);

    Ok(())
}

#[test]
fn test_incremental_indexing_body_only_change_updates_changed_callable_projection()
-> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join("main.rs");

    fs::write(
        &file_path,
        r#"
fn helper() {}
fn keep() { helper(); }
fn changed() { helper(); }
"#,
    )?;

    let mut storage = Storage::new_in_memory()?;
    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    let before_nodes = storage.get_nodes()?;
    let file_id = before_nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE && node.serialized_name.ends_with("main.rs"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing file node"))?;
    let keep_id = before_nodes
        .iter()
        .find(|node| node.serialized_name == "keep" && node.kind == NodeKind::FUNCTION)
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing keep() node"))?;
    let changed_id = before_nodes
        .iter()
        .find(|node| node.serialized_name == "changed" && node.kind == NodeKind::FUNCTION)
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing changed() node"))?;

    let states_before = storage.get_callable_projection_states_for_file(file_id.0)?;
    let keep_before = states_before
        .iter()
        .find(|state| state.node_id == keep_id)
        .map(|state| state.body_hash)
        .ok_or_else(|| anyhow::anyhow!("missing keep() projection state"))?;
    let changed_before = states_before
        .iter()
        .find(|state| state.node_id == changed_id)
        .map(|state| state.body_hash)
        .ok_or_else(|| anyhow::anyhow!("missing changed() projection state"))?;

    fs::write(
        &file_path,
        r#"
fn helper() {}
fn keep() { helper(); }
fn changed() { helper(); helper(); }
"#,
    )?;

    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    let states_after = storage.get_callable_projection_states_for_file(file_id.0)?;
    let keep_after = states_after
        .iter()
        .find(|state| state.node_id == keep_id)
        .map(|state| state.body_hash)
        .ok_or_else(|| anyhow::anyhow!("missing keep() projection state after refresh"))?;
    let changed_after = states_after
        .iter()
        .find(|state| state.node_id == changed_id)
        .map(|state| state.body_hash)
        .ok_or_else(|| anyhow::anyhow!("missing changed() projection state after refresh"))?;

    assert_eq!(keep_after, keep_before);
    assert_ne!(changed_after, changed_before);

    let edges = storage.get_edges()?;
    let keep_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == keep_id)
        .count();
    let changed_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == changed_id)
        .count();
    assert_eq!(keep_calls, 1);
    assert_eq!(changed_calls, 2);

    Ok(())
}

#[test]
fn test_incremental_indexing_structural_change_removes_stale_projection() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join("main.rs");

    fs::write(
        &file_path,
        r#"
fn stale() {}
fn keep() { stale(); }
"#,
    )?;

    let mut storage = Storage::new_in_memory()?;
    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    fs::write(
        &file_path,
        r#"
fn fresh() {}
fn keep() { fresh(); }
"#,
    )?;

    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    let nodes = storage.get_nodes()?;
    assert!(!nodes.iter().any(|node| node.serialized_name == "stale"));
    assert!(nodes.iter().any(|node| node.serialized_name == "fresh"));

    let file_id = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE && node.serialized_name.ends_with("main.rs"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing file node after refresh"))?;
    let states = storage.get_callable_projection_states_for_file(file_id.0)?;
    assert!(
        states
            .iter()
            .all(|state| !state.symbol_key.contains("stale"))
    );
    assert!(
        states
            .iter()
            .any(|state| state.symbol_key.contains("fresh"))
    );

    Ok(())
}

#[test]
fn test_indexing_real_repo_rust_files_does_not_drop_projection_on_graph_error() -> anyhow::Result<()>
{
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cases = [
        (
            PathBuf::from("src/lib.rs"),
            vec![
                "build_callable_projection_states",
                "classify_projection_update",
                "generate_id",
                "apply_line_range_call_attribution",
            ],
        ),
        (
            PathBuf::from("src/semantic/mod.rs"),
            vec!["SemanticCandidateIndex", "to_candidates"],
        ),
        (
            PathBuf::from("tests/call_resolution_common_methods.rs"),
            vec!["test_run_incremental_clone_does_not_resolve_to_unrelated_field_clone"],
        ),
    ];

    let dir = tempdir()?;
    let root = dir.path();
    let mut files_to_index = Vec::new();
    for (relative_path, _) in &cases {
        let source_path = manifest_dir.join(relative_path);
        let target_path = root.join(relative_path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target_path, fs::read_to_string(&source_path)?)?;
        files_to_index.push(target_path);
    }

    let mut storage = Storage::new_in_memory()?;
    run_incremental_indexing(root, &mut storage, files_to_index)?;

    let errors = storage.get_errors(None)?;
    assert!(
        errors.is_empty(),
        "expected repo Rust files to index cleanly, got errors: {errors:?}"
    );

    let nodes = storage.get_nodes()?;
    for (relative_path, expected_symbols) in cases {
        let indexed_file = root.join(&relative_path);
        assert!(
            nodes.iter().any(|node| {
                node.kind == NodeKind::FILE
                    && node.serialized_name == indexed_file.to_string_lossy()
            }),
            "missing file node for {}",
            indexed_file.display()
        );
        for expected_symbol in expected_symbols {
            assert!(
                nodes
                    .iter()
                    .any(|node| node.serialized_name == expected_symbol),
                "missing indexed symbol `{expected_symbol}` from {}",
                relative_path.display()
            );
        }
    }

    Ok(())
}

#[test]
fn test_multi_file_cross_references() -> anyhow::Result<()> {
    let storage = index_project(&[
        ("types.rs", "pub struct MyType { pub value: i32 }"),
        (
            "main.rs",
            r#"
use types::MyType;

fn process(t: MyType) {
    println!("{}", t.value);
}
"#,
        ),
    ])?;

    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;

    // Verify cross-file references
    assert!(nodes.iter().any(|n| n.serialized_name == "MyType"));
    assert!(nodes.iter().any(|n| n.serialized_name == "process"));

    // Should have edges (TYPE_USAGE or similar)
    assert!(edges.iter().any(|e| e.kind == EdgeKind::IMPORT));

    Ok(())
}

#[test]
fn test_python_module_constants_are_classified_as_constants() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "constants.py",
        r#"
API_TOKEN = "secret"
retry_limit = 3
"#,
    )])?;

    let nodes = storage.get_nodes()?;
    assert!(
        nodes
            .iter()
            .any(|node| node.serialized_name == "API_TOKEN" && node.kind == NodeKind::CONSTANT),
        "expected top-level ALL_CAPS assignment to be classified as CONSTANT"
    );
    assert!(
        nodes.iter().any(|node| {
            node.serialized_name == "retry_limit" && node.kind == NodeKind::VARIABLE
        }),
        "expected mixed-case assignment to remain VARIABLE"
    );

    Ok(())
}

#[test]
fn test_typescript_type_alias_and_enum_are_indexed() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "App.tsx",
        r#"
type Props = { label: string };

enum Status {
    Ready = "ready",
}

function Badge(props: Props) {
    return <span>{props.label}</span>;
}

function App() {
    return <Badge label="hello" variant={Status.Ready} />;
}
"#,
    )])?;

    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;
    assert!(
        nodes
            .iter()
            .any(|node| node.serialized_name == "Props" && node.kind == NodeKind::TYPEDEF)
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.serialized_name == "Status" && node.kind == NodeKind::ENUM)
    );
    let app_id = nodes
        .iter()
        .find(|node| node.serialized_name == "App" && node.kind == NodeKind::FUNCTION)
        .map(|node| node.id)
        .expect("App function node not found");
    let badge_id = nodes
        .iter()
        .find(|node| node.serialized_name == "Badge")
        .map(|node| node.id)
        .expect("Badge usage node not found");
    let label_id = nodes
        .iter()
        .find(|node| node.serialized_name == "label" && node.kind == NodeKind::FIELD)
        .map(|node| node.id)
        .expect("label prop node not found");
    let variant_id = nodes
        .iter()
        .find(|node| node.serialized_name == "variant" && node.kind == NodeKind::FIELD)
        .map(|node| node.id)
        .expect("variant prop node not found");
    assert!(edges.iter().any(|edge| {
        edge.kind == EdgeKind::CALL && edge.source == app_id && edge.target == badge_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == EdgeKind::USAGE && edge.source == app_id && edge.target == label_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == EdgeKind::USAGE && edge.source == app_id && edge.target == variant_id
    }));

    Ok(())
}

#[test]
fn test_indexing_and_search_projection_cleanup() -> anyhow::Result<()> {
    let mut storage =
        index_project(&[("a.rs", "struct Alpha {}\n"), ("b.rs", "struct Beta {}\n")])?;

    let mut engine = SearchEngine::new(None)?;
    let nodes = storage.get_nodes()?;
    let non_file_nodes: Vec<_> = nodes
        .iter()
        .filter(|node| node.kind != NodeKind::FILE && node.file_node_id.is_some())
        .map(|node| (node.id, node.serialized_name.clone()))
        .collect();
    assert!(!non_file_nodes.is_empty());

    let alpha_node_id = non_file_nodes
        .iter()
        .find(|(_, name)| name == "Alpha")
        .map(|(id, _)| *id)
        .ok_or_else(|| anyhow::anyhow!("missing Alpha node"))?;

    let _beta_node_id = non_file_nodes
        .iter()
        .find(|(_, name)| name == "Beta")
        .map(|(id, _)| *id)
        .ok_or_else(|| anyhow::anyhow!("missing Beta node"))?;

    engine.index_nodes(
        non_file_nodes
            .iter()
            .map(|(id, name)| (*id, name.clone()))
            .collect(),
    )?;

    let alpha_search = engine.search_symbol("Alpha");
    assert!(alpha_search.contains(&alpha_node_id));
    assert!(!engine.search_symbol("Beta").is_empty());

    let file_a_nodes = storage.get_nodes()?;
    let file_node_id = file_a_nodes
        .into_iter()
        .find(|node| node.kind == NodeKind::FILE && node.serialized_name.ends_with("a.rs"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing file node for a.rs"))?;

    let deleted_nodes: Vec<NodeId> = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.file_node_id == Some(file_node_id))
        .map(|node| node.id)
        .collect();

    storage.delete_file_projection(file_node_id.0)?;
    engine.remove_nodes(&deleted_nodes)?;

    assert!(engine.search_symbol("Alpha").is_empty());
    let beta_search = engine.search_symbol("Beta");
    assert!(!beta_search.is_empty());
    assert!(beta_search.iter().all(|id| *id != alpha_node_id));

    Ok(())
}

#[test]
fn test_cpp_access_specifiers_are_captured_from_rules() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "main.cpp",
        r#"
class Widget {
public:
    void open();
protected:
    void guard();
private:
    void close();
};
"#,
    )])?;

    let nodes = storage.get_nodes()?;
    let open = nodes
        .iter()
        .find(|n| n.serialized_name.ends_with("open"))
        .ok_or_else(|| anyhow::anyhow!("open node missing"))?;
    let guard = nodes
        .iter()
        .find(|n| n.serialized_name.ends_with("guard"))
        .ok_or_else(|| anyhow::anyhow!("guard node missing"))?;
    let close = nodes
        .iter()
        .find(|n| n.serialized_name.ends_with("close"))
        .ok_or_else(|| anyhow::anyhow!("close node missing"))?;

    assert_eq!(
        storage.get_component_access(open.id)?,
        Some(AccessKind::Public)
    );
    assert_eq!(
        storage.get_component_access(guard.id)?,
        Some(AccessKind::Protected)
    );
    assert_eq!(
        storage.get_component_access(close.id)?,
        Some(AccessKind::Private)
    );

    Ok(())
}
