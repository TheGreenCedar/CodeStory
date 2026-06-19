use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{
    AccessKind, EdgeKind, NodeId, NodeKind, OccurrenceKind, ResolutionCertainty,
};
use codestory_indexer::resolution::{RESOLUTION_SUPPORT_SNAPSHOT_VERSION, ResolutionPass};
use codestory_indexer::{IncrementalIndexingStats, WorkspaceIndexer};
use codestory_store::Store as Storage;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn run_incremental_indexing(
    root: &Path,
    storage: &mut Storage,
    files_to_index: Vec<PathBuf>,
) -> anyhow::Result<IncrementalIndexingStats> {
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index,
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };
    indexer.run_incremental(storage, &refresh_info, &event_bus, None)
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
fn test_failed_file_attempt_is_recorded_as_incomplete_with_attached_error() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join("broken.ts");

    let mut storage = Storage::new_in_memory()?;
    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    let files = storage.get_files()?;
    assert_eq!(
        files.len(),
        1,
        "failed attempts should still record file inventory"
    );
    let file = &files[0];
    assert_eq!(file.path, file_path);
    assert!(file.indexed);
    assert!(!file.complete);

    let errors = storage.get_errors(None)?;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].file_id, Some(NodeId(file.id)));

    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    let errors = storage.get_errors(None)?;
    assert_eq!(
        errors.len(),
        1,
        "reindexing the same failed file should replace, not duplicate, its error"
    );
    assert_eq!(errors[0].file_id, Some(NodeId(file.id)));

    Ok(())
}

#[test]
fn test_svelte_tauri_invoke_surfaces_registered_rust_command_boundary() -> anyhow::Result<()> {
    let storage = index_project(&[
        (
            "src/App.svelte",
            r#"
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  export async function refresh() {
    await invoke("get_snapshot");
  }
</script>
"#,
        ),
        (
            "src-tauri/src/lib.rs",
            r#"
#[tauri::command]
fn get_snapshot() -> String {
    String::new()
}

pub fn build() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_snapshot]);
}
"#,
        ),
    ])?;

    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;
    let command = nodes
        .iter()
        .find(|node| node.canonical_id.as_deref() == Some("tauri:command:get_snapshot"))
        .ok_or_else(|| anyhow::anyhow!("missing tauri command node"))?;
    let function = nodes
        .iter()
        .find(|node| node.serialized_name == "get_snapshot" && node.kind == NodeKind::FUNCTION)
        .ok_or_else(|| anyhow::anyhow!("missing registered Rust command function"))?;

    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.target == command.id
                && edge.certainty == Some(ResolutionCertainty::Uncertain)
        }),
        "expected Svelte invoke() to create uncertain command evidence"
    );
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.source == command.id
                && edge.target == function.id
                && matches!(
                    edge.certainty,
                    Some(ResolutionCertainty::Probable | ResolutionCertainty::Certain)
                )
        }),
        "expected registered Tauri command symbol to link to Rust function"
    );

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
fn test_incremental_indexing_second_run_reuses_unchanged_extraction_cache_and_resolution_support()
-> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join("main.rs");
    fs::write(&file_path, "fn helper() {}\nfn run() { helper(); }\n")?;

    let mut storage = Storage::new_in_memory()?;
    let first_stats = run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;
    assert_eq!(first_stats.artifact_cache_hits, 0);
    assert_eq!(first_stats.artifact_cache_misses, 1);
    assert!(!first_stats.resolution_support_snapshot_hit);
    assert!(storage.has_ready_resolution_support_snapshot(RESOLUTION_SUPPORT_SNAPSHOT_VERSION)?);

    let second_stats = run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;
    assert_eq!(second_stats.artifact_cache_hits, 1);
    assert_eq!(second_stats.artifact_cache_misses, 0);

    let resolution_stats = ResolutionPass::new().run(&mut storage)?;
    assert!(resolution_stats.telemetry.support_snapshot_hit);

    let nodes = storage.get_nodes()?;
    assert!(nodes.iter().any(|node| node.serialized_name == "run"));

    Ok(())
}

#[test]
fn test_index_artifact_cache_copies_across_compatible_roots() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let source_root = dir.path().join("source");
    let target_root = dir.path().join("target");
    fs::create_dir_all(source_root.join("src"))?;
    fs::create_dir_all(target_root.join("src"))?;
    let source_file = source_root.join("src/main.rs");
    let target_file = target_root.join("src/main.rs");
    let source = "fn helper() {}\nfn run() { helper(); }\n";
    fs::write(&source_file, source)?;
    fs::write(&target_file, source)?;

    let source_db = dir.path().join("source.db");
    let target_db = dir.path().join("target.db");
    let mut source_storage = Storage::open(&source_db)?;
    let first_stats =
        run_incremental_indexing(&source_root, &mut source_storage, vec![source_file])?;
    assert_eq!(first_stats.artifact_cache_writes, 1);
    drop(source_storage);

    let mut target_storage = Storage::open(&target_db)?;
    assert_eq!(
        target_storage.copy_index_artifact_cache_from(&source_db)?,
        1
    );
    let target_stats =
        run_incremental_indexing(&target_root, &mut target_storage, vec![target_file.clone()])?;
    assert_eq!(target_stats.artifact_cache_hits, 1);
    assert_eq!(target_stats.artifact_cache_misses, 0);

    let source_root_text = source_root.to_string_lossy();
    for node in target_storage.get_nodes()? {
        assert!(!node.serialized_name.contains(source_root_text.as_ref()));
        assert!(
            !node
                .canonical_id
                .as_deref()
                .unwrap_or_default()
                .contains(source_root_text.as_ref())
        );
    }
    assert_eq!(
        target_storage
            .get_file_by_path(&target_file)?
            .map(|file| file.path),
        Some(target_file)
    );

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
fn test_incremental_go_structural_change_removes_stale_projection() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join("mux.go");

    fs::write(
        &file_path,
        r#"
package mux

type Router struct {}

func (r *Router) StrictSlash(value bool) *Router { return r }
func (r *Router) Handle(path string) {}
"#,
    )?;

    let mut storage = Storage::new_in_memory()?;
    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    let before_nodes = storage.get_nodes()?;
    assert!(
        before_nodes
            .iter()
            .any(|node| node.serialized_name == "Router.StrictSlash"),
        "expected initial Go parser-backed method projection"
    );
    let file_id = before_nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE && node.serialized_name.ends_with("mux.go"))
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing Go file node"))?;
    assert!(
        storage
            .get_callable_projection_states_for_file(file_id.0)?
            .iter()
            .any(|state| state.symbol_key.contains("StrictSlash")),
        "Go parser-backed indexing should persist callable projection state"
    );

    fs::write(
        &file_path,
        r#"
package mux

type Router struct {}

func (r *Router) Handle(path string) {}
"#,
    )?;

    run_incremental_indexing(root, &mut storage, vec![file_path.clone()])?;

    let after_nodes = storage.get_nodes()?;
    assert!(
        !after_nodes
            .iter()
            .any(|node| node.serialized_name.ends_with(".StrictSlash")),
        "stale Go method should be removed after structural refresh"
    );
    let states_after = storage.get_callable_projection_states_for_file(file_id.0)?;
    assert!(
        states_after
            .iter()
            .all(|state| !state.symbol_key.contains("StrictSlash")),
        "stale Go projection state should be removed"
    );

    Ok(())
}

#[test]
fn test_incremental_indexing_deletes_removed_files_before_resolution() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let removed = root.join("lib.rs");
    let caller = root.join("main.rs");
    fs::write(&removed, "pub fn helper() {}\n")?;
    fs::write(
        &caller,
        "mod lib;\nuse crate::lib::helper;\n\nfn run() {\n    helper();\n}\n",
    )?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let initial = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![removed.clone(), caller.clone()],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };
    indexer.run_incremental(&mut storage, &initial, &event_bus, None)?;

    let helper_id = storage
        .get_nodes()?
        .into_iter()
        .find(|node| node.serialized_name.ends_with("helper") && node.kind == NodeKind::FUNCTION)
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing helper() node after initial index"))?;
    let run_id = storage
        .get_nodes()?
        .into_iter()
        .find(|node| node.serialized_name == "run" && node.kind == NodeKind::FUNCTION)
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("missing run() node after initial index"))?;
    assert!(
        storage
            .get_edges()?
            .iter()
            .any(|edge| edge.kind == EdgeKind::CALL
                && edge.source == run_id
                && edge.resolved_target == Some(helper_id)),
        "expected initial run() call to resolve to helper()"
    );

    let removed_file_id = storage
        .get_file_by_path(&removed)?
        .ok_or_else(|| anyhow::anyhow!("missing removed.py file record"))?
        .id;
    fs::remove_file(&removed)?;

    let refresh = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![caller.clone()],
        files_to_remove: vec![removed_file_id],
        existing_file_ids: std::collections::HashMap::new(),
    };
    let stats = indexer.run_incremental(&mut storage, &refresh, &event_bus, None)?;

    let nodes = storage.get_nodes()?;
    assert!(
        !nodes
            .iter()
            .any(|node| node.serialized_name.ends_with("helper") && node.kind == NodeKind::FUNCTION),
        "deleted helper() symbol should be removed after cleanup"
    );
    let call_edges = storage
        .get_edges()?
        .into_iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == run_id)
        .collect::<Vec<_>>();
    assert!(
        !call_edges.is_empty(),
        "expected run() to keep its call edge after refresh"
    );
    assert!(
        call_edges
            .iter()
            .all(|edge| edge.resolved_target != Some(helper_id) && edge.resolved_target.is_none()),
        "calls should not resolve against symbols from the deleted file: {call_edges:?}"
    );
    assert_eq!(
        stats.resolved_calls, 0,
        "incremental resolution should not count deleted-file targets as valid resolutions"
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
