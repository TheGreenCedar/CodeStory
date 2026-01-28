use codestory_core::{EdgeKind, NodeKind, OccurrenceKind};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_integration_full_loop() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();

    // 1. Create a small project
    let f1 = root.join("main.cpp");
    fs::write(
        &f1,
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
    )?;

    // 2. Index the project
    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: vec![f1.clone()],
        files_to_remove: vec![],
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

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
        .find(|n| n.serialized_name == "myMethod")
        .expect("myMethod node not found");
    // In our indexer, methods currently default to FUNCTION if not using specific METHOD kind in TS graph
    // But let's check what it actually is

    // 4. Verify Edges (CALL)
    let edges = storage.get_edges()?;
    assert!(
        edges.iter().any(|e| e.kind == EdgeKind::CALL),
        "CALL edge not found"
    );

    // 5. Verify Occurrences
    let occs = storage.get_occurrences_for_element(my_method.id.0)?;
    assert!(!occs.is_empty(), "No occurrences for myMethod");

    // Check if we have at least one DEFINITION and one REFERENCE
    let has_def = occs.iter().any(|o| o.kind == OccurrenceKind::DEFINITION);
    let has_ref = occs.iter().any(|o| o.kind == OccurrenceKind::REFERENCE);

    assert!(has_def, "Method definition occurrence missing");
    assert!(has_ref, "Method call (reference) occurrence missing");

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
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    // 2. Index initial version
    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: vec![f1.clone()],
        files_to_remove: vec![],
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let nodes = storage.get_nodes()?;
    assert!(nodes.iter().any(|n| n.serialized_name == "Foo"));
    let initial_count = nodes.len();

    // 3. Modify file to add new symbol
    fs::write(&f1, "struct Foo {}\nstruct Bar {}")?;

    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: vec![f1.clone()],
        files_to_remove: vec![],
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    // 4. Verify both symbols exist
    let nodes = storage.get_nodes()?;
    assert!(nodes.iter().any(|n| n.serialized_name == "Foo"));
    assert!(nodes.iter().any(|n| n.serialized_name == "Bar"));
    assert!(nodes.len() > initial_count);

    Ok(())
}

#[test]
fn test_multi_file_cross_references() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();

    // Create files with cross-references
    let f1 = root.join("types.rs");
    let f2 = root.join("main.rs");

    fs::write(&f1, "pub struct MyType { pub value: i32 }")?;
    fs::write(
        &f2,
        r#"
use types::MyType;

fn process(t: MyType) {
    println!("{}", t.value);
}
"#,
    )?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: vec![f1.clone(), f2.clone()],
        files_to_remove: vec![],
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;

    // Verify cross-file references
    assert!(nodes.iter().any(|n| n.serialized_name == "MyType"));
    assert!(nodes.iter().any(|n| n.serialized_name == "process"));

    // Should have edges (TYPE_USAGE or similar)
    assert!(!edges.is_empty());

    Ok(())
}
