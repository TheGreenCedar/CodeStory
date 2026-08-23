use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{EdgeKind, NodeKind};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

fn index_rust(source: &str) -> anyhow::Result<Storage> {
    let project = tempdir()?;
    let path = project.path().join("fixture.rs");
    fs::write(&path, source)?;
    let mut storage = Storage::new_in_memory()?;
    WorkspaceIndexer::new(project.path().to_path_buf()).run_incremental(
        &mut storage,
        &codestory_workspace::RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: vec![path],
            files_to_remove: vec![],
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    Ok(storage)
}

#[test]
fn call_placeholder_before_declaration_does_not_consume_its_ordinal() -> anyhow::Result<()> {
    let storage = index_rust(
        "fn caller() { target(); }\n\
         fn target() {}\n",
    )?;
    let declarations = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == "target")
        .filter(|node| node.start_line == Some(2))
        .collect::<Vec<_>>();
    assert_eq!(
        declarations.len(),
        1,
        "one target declaration: {declarations:?}"
    );
    assert!(
        declarations[0]
            .canonical_id
            .as_deref()
            .is_some_and(|identity| identity.ends_with("fixture.rs:target#0")),
        "call placeholders must not shift declaration ordinals: {:?}",
        declarations[0].canonical_id
    );
    Ok(())
}

#[test]
fn same_line_duplicate_definitions_stay_distinct_and_calls_stay_ambiguous() -> anyhow::Result<()> {
    let storage = index_rust(
        "fn caller() { duplicate(); }\n\
         fn duplicate() {} fn duplicate() {}\n",
    )?;
    let nodes = storage.get_nodes()?;
    let mut declarations = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == "duplicate")
        .filter(|node| node.start_line == Some(2))
        .collect::<Vec<_>>();
    declarations.sort_by_key(|node| node.start_col);
    assert_eq!(
        declarations.len(),
        2,
        "duplicate definitions must remain distinct: {declarations:?}"
    );
    assert_eq!(
        declarations
            .iter()
            .map(|node| {
                node.canonical_id
                    .as_deref()
                    .and_then(|identity| identity.rsplit("fixture.rs:").next())
            })
            .collect::<Vec<_>>(),
        [Some("duplicate#0"), Some("duplicate#1")]
    );

    let caller = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == "caller")
        .expect("caller declaration");
    let call = storage
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL && edge.effective_source() == caller.id)
        .expect("duplicate call edge");
    assert_eq!(
        call.resolved_target, None,
        "ambiguous duplicate target must fail closed"
    );
    Ok(())
}
