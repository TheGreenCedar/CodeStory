//! How the file-structural fence classifies one refresh (SRC-C2).
//!
//! These read `classify_projection_update` directly, so they can pin the two
//! things an end-to-end probe cannot reach: the one unowned row class that
//! genuinely still needs `FullReplace`, and the wire format a store written by
//! the previous release compares against.

use crate::*;

/// A file whose declarations are almost all unowned: a module node and its
/// import edge, a class header and its definition occurrence, and one
/// method. Nothing but the method has a projection row, so everything else
/// is fenced by the file-structural row.
///
/// `import_edge_file` is the `file_node_id` the import edge is recorded
/// against — `Some(file)` for the ordinary case, anything else for the one
/// class `Store::delete_unowned_projection_for_file` cannot reach.
fn projection_for_unowned_declarations(
    shift: u32,
    import_edge_file: Option<NodeId>,
    keep_module_node: bool,
) -> Vec<CallableProjectionState> {
    let file_id = NodeId(1);
    let module_id = NodeId(20);
    let class_id = NodeId(30);
    let callable_id = NodeId(2);
    let mut nodes = vec![
        Node {
            id: file_id,
            kind: NodeKind::FILE,
            serialized_name: "Widget.java".to_string(),
            qualified_name: Some("Widget.java".to_string()),
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(40),
            end_col: Some(1),
            ..Default::default()
        },
        Node {
            id: class_id,
            kind: NodeKind::CLASS,
            serialized_name: "Widget".to_string(),
            qualified_name: Some("Widget".to_string()),
            file_node_id: Some(file_id),
            start_line: Some(3 + shift),
            start_col: Some(1),
            end_line: Some(12 + shift),
            end_col: Some(1),
            ..Default::default()
        },
        Node {
            id: callable_id,
            kind: NodeKind::METHOD,
            serialized_name: "Widget.total".to_string(),
            qualified_name: Some("Widget.total".to_string()),
            file_node_id: Some(file_id),
            start_line: Some(6 + shift),
            start_col: Some(5),
            end_line: Some(8 + shift),
            end_col: Some(5),
            ..Default::default()
        },
    ];
    if keep_module_node {
        nodes.push(Node {
            id: module_id,
            kind: NodeKind::MODULE,
            serialized_name: "java.util.List".to_string(),
            qualified_name: Some("java.util.List".to_string()),
            file_node_id: Some(file_id),
            start_line: Some(1 + shift),
            start_col: Some(1),
            end_line: Some(1 + shift),
            end_col: Some(23),
            ..Default::default()
        });
    }
    let edges = vec![Edge {
        id: EdgeId(7),
        source: file_id,
        target: module_id,
        kind: EdgeKind::IMPORT,
        file_node_id: import_edge_file,
        line: Some(1 + shift),
        ..Default::default()
    }];
    let occurrences = vec![
        Occurrence {
            element_id: module_id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_id,
                start_line: 1 + shift,
                start_col: 1,
                end_line: 1 + shift,
                end_col: 23,
            },
        },
        Occurrence {
            element_id: class_id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_id,
                start_line: 3 + shift,
                start_col: 1,
                end_line: 3 + shift,
                end_col: 21,
            },
        },
    ];
    build_callable_projection_states(&nodes, &edges, &occurrences)
}

#[test]
fn unowned_declarations_that_only_moved_are_repositioned() {
    let before = projection_for_unowned_declarations(0, Some(NodeId(1)), true);
    assert_eq!(
        classify_projection_update(
            &before,
            &projection_for_unowned_declarations(0, Some(NodeId(1)), true)
        ),
        ProjectionUpdateMode::NoChanges,
        "an unchanged file must not be reprojected"
    );

    let shifted = projection_for_unowned_declarations(1, Some(NodeId(1)), true);
    assert_eq!(
        classify_projection_update(&before, &shifted),
        ProjectionUpdateMode::RepositionUnowned {
            changed_callers: vec![NodeId(2)]
        },
        "an import, a class header and their rows moving one line must be \
         repaired in place, and the callable that moved with them must still \
         be repaired caller-scoped"
    );
}

#[test]
fn a_removed_unowned_declaration_still_forces_a_full_replacement() {
    // The reposition repair never deletes a node row, so it is only sound
    // while the unowned population is unchanged.
    let before = projection_for_unowned_declarations(0, Some(NodeId(1)), true);
    let without_module = projection_for_unowned_declarations(1, Some(NodeId(1)), false);
    assert_eq!(
        classify_projection_update(&before, &without_module),
        ProjectionUpdateMode::FullReplace,
        "a declaration that disappeared must not be repaired in place"
    );
}

#[test]
fn an_unowned_edge_outside_the_file_scope_still_forces_a_full_replacement() {
    // `delete_unowned_projection_for_file` deletes edge rows by
    // `file_node_id`, so an unowned edge recorded against another file is
    // never reached by it; non-call edge ids do not include the line, so
    // the re-insert is ignored and the stale line survives. This is the one
    // unowned class that still has to take the whole-file replacement, and
    // it is the only reason the identity hash keeps any line at all.
    let before = projection_for_unowned_declarations(0, Some(NodeId(999)), true);
    assert_eq!(
        classify_projection_update(
            &before,
            &projection_for_unowned_declarations(0, Some(NodeId(999)), true)
        ),
        ProjectionUpdateMode::NoChanges,
        "an unchanged file must not be reprojected"
    );
    assert_eq!(
        classify_projection_update(
            &before,
            &projection_for_unowned_declarations(1, Some(NodeId(999)), true)
        ),
        ProjectionUpdateMode::FullReplace,
        "an unowned edge the file-scoped cleanup cannot reach must keep \
         forcing a full replacement when it moves"
    );

    // Control: the identical shift is repairable once the same edge is
    // recorded against the file the cleanup deletes by.
    let file_scoped = projection_for_unowned_declarations(0, Some(NodeId(1)), true);
    assert_eq!(
        classify_projection_update(
            &file_scoped,
            &projection_for_unowned_declarations(1, Some(NodeId(1)), true)
        ),
        ProjectionUpdateMode::RepositionUnowned {
            changed_callers: vec![NodeId(2)]
        },
    );
}

/// The change detector for
/// `projection_for_unowned_declarations(0, Some(NodeId(1)), true)`, as produced
/// by the release that wrote the databases this one has to read.
///
/// This constant is a compatibility pin, not a golden output. Every store
/// in the field holds this number for its own files; changing any part
/// string the change detector hashes would make every one of them compare
/// unequal on the next refresh, and `FullReplace` deletes annotations.
/// Moving it is a deliberate, declared re-projection wave.
const FROZEN_FILE_STRUCTURAL_BODY_HASH: i64 = -3_900_083_451_554_756_332;

fn file_structural_row(states: &[CallableProjectionState]) -> &CallableProjectionState {
    states
        .iter()
        .find(|state| state.symbol_key == FILE_STRUCTURAL_SYMBOL_KEY)
        .expect("file structural row")
}

#[test]
fn a_store_from_the_previous_release_upgrades_without_a_reprojection_wave() {
    let before = projection_for_unowned_declarations(0, Some(NodeId(1)), true);
    assert_eq!(
        file_structural_row(&before).body_hash,
        FROZEN_FILE_STRUCTURAL_BODY_HASH,
        "the change detector's wire format is frozen: a different value here \
         re-replaces every file in every existing database"
    );

    // What such a database actually holds: the identity column carries the
    // one constant the previous release wrote there for every file.
    let legacy = before
        .iter()
        .cloned()
        .map(|mut state| {
            if state.symbol_key == FILE_STRUCTURAL_SYMBOL_KEY {
                state.signature_hash = callable_signature_hash(FILE_STRUCTURAL_SYMBOL_KEY);
            }
            state
        })
        .collect::<Vec<_>>();
    assert_ne!(
        file_structural_row(&legacy).signature_hash,
        file_structural_row(&before).signature_hash,
        "the fixture must actually differ from a real identity"
    );
    assert_eq!(
        classify_projection_update(&legacy, &before),
        ProjectionUpdateMode::NoChanges,
        "an untouched file in an upgraded store must not be reprojected"
    );
    let shifted = projection_for_unowned_declarations(1, Some(NodeId(1)), true);
    assert_eq!(
        classify_projection_update(&legacy, &shifted),
        ProjectionUpdateMode::FullReplace,
        "a legacy row carries no identity, so its fence moving is still churn"
    );
}
