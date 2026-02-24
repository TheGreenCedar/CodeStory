use codestory_core::{EdgeKind, NodeId, NodeKind, OccurrenceKind};
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
        .find(|n| n.serialized_name == "MyClass")
        .expect("MyClass node not found");
    assert_eq!(my_class.kind, NodeKind::CLASS);

    // Find myMethod
    let my_method = nodes
        .iter()
        .find(|n| n.serialized_name.ends_with("myMethod") && n.kind == NodeKind::FUNCTION)
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
