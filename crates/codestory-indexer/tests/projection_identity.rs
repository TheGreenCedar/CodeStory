//! Callable projection identity across position-shifting edits (W4.6/CR-008).
//!
//! Every assertion here reads rows the production incremental path actually
//! wrote: node identities, occurrence spans, edge lines, and the bookmark table
//! that a whole-file replacement destroys.

use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{EdgeKind, NodeKind};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn reindex(root: &Path, storage: &mut Storage, path: &Path) -> anyhow::Result<()> {
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path.to_path_buf()],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    indexer.run_incremental(storage, &refresh_info, &event_bus, None)?;
    Ok(())
}

fn node_id_by_name(storage: &Storage, name: &str, kind: NodeKind) -> anyhow::Result<i64> {
    let matches = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.serialized_name == name && node.kind == kind)
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one `{name}` of kind {kind:?}, got {:?}",
        matches
            .iter()
            .map(|node| (node.id.0, node.start_line))
            .collect::<Vec<_>>()
    );
    Ok(matches[0].id.0)
}

fn occurrence_spans(storage: &Storage, file_id: i64) -> anyhow::Result<Vec<(i64, u32, u32)>> {
    let mut rows = storage
        .get_occurrences_for_file(codestory_contracts::graph::NodeId(file_id))?
        .into_iter()
        .map(|occ| {
            (
                occ.element_id,
                occ.location.start_line,
                occ.location.start_col,
            )
        })
        .collect::<Vec<_>>();
    rows.sort();
    Ok(rows)
}

fn file_node_id(storage: &Storage) -> anyhow::Result<i64> {
    Ok(storage
        .get_nodes()?
        .into_iter()
        .find(|node| node.kind == NodeKind::FILE)
        .expect("file node")
        .id
        .0)
}

const RUST_BEFORE: &str = r#"use std::fmt::Debug;

struct Thing {
    value: i32,
}

fn helper(a: i32) -> i32 {
    a + 1
}

fn caller() -> i32 {
    let t = Thing { value: 1 };
    helper(t.value)
}
"#;

const RUST_AFTER_SHIFT: &str = r#"use std::fmt::Debug;

struct Thing {
    value: i32,
}

// a comment inserted above the first function
fn helper(a: i32) -> i32 {
    a + 1
}

fn caller() -> i32 {
    let t = Thing { value: 1 };
    helper(t.value)
}
"#;

#[test]
fn position_shift_keeps_callable_identity_and_its_annotations() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let path = root.join("lib.rs");
    fs::write(&path, RUST_BEFORE)?;
    let mut storage = Storage::new_in_memory()?;
    reindex(root, &mut storage, &path)?;

    let helper_before = node_id_by_name(&storage, "helper", NodeKind::FUNCTION)?;
    let caller_before = node_id_by_name(&storage, "caller", NodeKind::FUNCTION)?;
    let category = storage.create_bookmark_category("review")?;
    let bookmark = storage.add_bookmark(
        category,
        codestory_contracts::graph::NodeId(helper_before),
        Some("look here"),
    )?;

    fs::write(&path, RUST_AFTER_SHIFT)?;
    reindex(root, &mut storage, &path)?;

    // Identity survives the shift, so anything anchored to it survives too.
    assert_eq!(
        node_id_by_name(&storage, "helper", NodeKind::FUNCTION)?,
        helper_before,
        "a callable that only moved must keep its identity"
    );
    assert_eq!(
        node_id_by_name(&storage, "caller", NodeKind::FUNCTION)?,
        caller_before
    );
    let bookmarks = storage.get_bookmarks(Some(category))?;
    assert_eq!(
        bookmarks.len(),
        1,
        "a position-shifting edit must not delete user annotations: {bookmarks:?}"
    );
    assert_eq!(bookmarks[0].id, bookmark);
    assert_eq!(bookmarks[0].node_id.0, helper_before);

    // The projection is repaired, not merely preserved: positions move with the
    // source and no stale duplicate survives.
    let file_id = file_node_id(&storage)?;
    let spans = occurrence_spans(&storage, file_id)?;
    assert!(
        spans.contains(&(helper_before, 8, 1)),
        "helper's definition occurrence must follow it to line 8: {spans:?}"
    );
    assert!(
        !spans
            .iter()
            .any(|(element, line, _)| *element == helper_before && *line == 7),
        "the pre-shift occurrence must not survive as a duplicate: {spans:?}"
    );
    let mut seen = spans.clone();
    seen.dedup();
    assert_eq!(
        seen.len(),
        spans.len(),
        "duplicate occurrence rows: {spans:?}"
    );

    let nodes = storage.get_nodes()?;
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.serialized_name == "helper")
            .count(),
        2,
        "the function and its one call placeholder, and no orphaned copies: {:?}",
        nodes
            .iter()
            .map(|node| (node.serialized_name.as_str(), node.id.0, node.start_line))
            .collect::<Vec<_>>()
    );
    let helper_node = nodes
        .iter()
        .find(|node| node.id.0 == helper_before)
        .expect("helper node");
    assert_eq!(
        helper_node.start_line,
        Some(8),
        "the surviving node row carries the new position"
    );
    Ok(())
}

#[test]
fn a_moved_call_edge_carries_its_new_line() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let path = root.join("lib.rs");
    fs::write(&path, RUST_BEFORE)?;
    let mut storage = Storage::new_in_memory()?;
    reindex(root, &mut storage, &path)?;
    fs::write(&path, RUST_AFTER_SHIFT)?;
    reindex(root, &mut storage, &path)?;

    let nodes = storage.get_nodes()?;
    let caller = nodes
        .iter()
        .find(|node| node.serialized_name == "caller" && node.kind == NodeKind::FUNCTION)
        .expect("caller node");
    let call_lines = storage
        .get_edges()?
        .into_iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.source == caller.id)
        .map(|edge| edge.line)
        .collect::<Vec<_>>();
    assert_eq!(
        call_lines,
        vec![Some(14)],
        "exactly one call edge, on the line the call now occupies"
    );
    Ok(())
}

#[test]
fn a_shifted_occurrence_no_callable_owns_forces_a_full_replacement() -> anyhow::Result<()> {
    // Occurrence rows carry no id, so an occurrence the caller-scoped cleanup
    // does not delete becomes a duplicate rather than an updated row. The
    // struct field's occurrence sits outside every callable, so nothing but the
    // file fence can see it move.
    let dir = tempdir()?;
    let root = dir.path();
    let path = root.join("lib.rs");
    fs::write(&path, RUST_BEFORE)?;
    let mut storage = Storage::new_in_memory()?;
    reindex(root, &mut storage, &path)?;
    let field = storage
        .get_nodes()?
        .into_iter()
        .find(|node| node.serialized_name == "Thing::value")
        .expect("field node");

    // Insert above everything, so the field itself moves.
    fs::write(&path, format!("// header\n{RUST_BEFORE}"))?;
    reindex(root, &mut storage, &path)?;

    let file_id = file_node_id(&storage)?;
    let field_spans = occurrence_spans(&storage, file_id)?
        .into_iter()
        .filter(|(element, _, _)| *element == field.id.0)
        .collect::<Vec<_>>();
    assert_eq!(
        field_spans,
        vec![(field.id.0, 5, 5)],
        "the field's occurrence must move once, not be duplicated"
    );
    Ok(())
}

#[test]
fn a_body_edit_still_replaces_the_symbols_it_removes() -> anyhow::Result<()> {
    // Fail-closed control: relaxing the position trigger must not relax
    // deletion. A callable that disappears is still fully replaced.
    let dir = tempdir()?;
    let root = dir.path();
    let path = root.join("lib.rs");
    fs::write(&path, RUST_BEFORE)?;
    let mut storage = Storage::new_in_memory()?;
    reindex(root, &mut storage, &path)?;
    assert!(
        storage
            .get_nodes()?
            .iter()
            .any(|node| node.serialized_name == "helper" && node.kind == NodeKind::FUNCTION)
    );

    fs::write(
        &path,
        "use std::fmt::Debug;\n\nstruct Thing {\n    value: i32,\n}\n\nfn caller() -> i32 {\n    let t = Thing { value: 1 };\n    t.value\n}\n",
    )?;
    reindex(root, &mut storage, &path)?;
    assert!(
        !storage
            .get_nodes()?
            .iter()
            .any(|node| node.serialized_name == "helper"),
        "a deleted callable must not survive an incremental refresh"
    );
    Ok(())
}
