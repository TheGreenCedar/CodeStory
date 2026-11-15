use super::*;
use crate::semantic_projection::process_semantic_symbol_nodes;

#[derive(serde::Deserialize)]
struct RetainedType {
    node: Node,
    path: String,
    first_line: usize,
    source: String,
    precise: SourceLocation,
    before: String,
    expanded: String,
    attached: String,
    children: Vec<String>,
    digest: Vec<String>,
}

fn declaration(node: &Node, location: SourceLocation) -> Occurrence {
    Occurrence {
        element_id: node.id.0,
        kind: OccurrenceKind::DECLARATION,
        location,
    }
}

fn bounds(node: &Node) -> (Option<u32>, Option<u32>, Option<u32>, Option<u32>) {
    (node.start_line, node.start_col, node.end_line, node.end_col)
}

// Exercise the shared fresh-source materialization path, including its Store
// lookup, persisted document and pending dense metadata. No embedding runs here.
fn materialize(
    node: &Node,
    path: &str,
    text: &str,
    context: &SemanticDocGraphContext,
    occurrences: &[Occurrence],
    alias_mode: SemanticDocAliasMode,
) -> (SymbolSearchDoc, Vec<PendingLlmSymbolDoc>) {
    let mut store = Store::new_in_memory().unwrap();
    let file_id = node.file_node_id.unwrap();
    store
        .insert_nodes_batch(&[
            Node {
                id: file_id,
                kind: NodeKind::FILE,
                serialized_name: path.into(),
                ..Default::default()
            },
            node.clone(),
        ])
        .unwrap();
    // Retain identical duplicates for the hostile persisted-row case instead
    // of letting the normal uniqueness index silently weaken that assertion.
    store
        .get_connection()
        .execute_batch("DROP INDEX idx_occurrence_unique")
        .unwrap();
    store.insert_occurrences_batch(occurrences).unwrap();
    assert_eq!(store.get_occurrences().unwrap().len(), occurrences.len());
    let files = HashMap::from([(path.to_string(), Some(text.to_string()))]);
    let mut stats = SemanticProjectionStats::default();
    let mut build_ns = 0;
    let mut pending = Vec::new();
    process_semantic_symbol_nodes(
        &mut store,
        &[node],
        context,
        &files,
        None,
        &HashMap::from([(node.id, AccessKind::Public)]),
        &HashMap::new(),
        1,
        alias_mode,
        128,
        &HashSet::new(),
        1,
        usize::MAX,
        "core:test-type-inputs",
        None,
        &mut stats,
        &mut build_ns,
        &mut pending,
        &mut Vec::new(),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(
        bounds(&store.get_node(node.id).unwrap().unwrap()),
        bounds(node)
    );
    let mut docs = store
        .get_symbol_search_docs_for_node_ids(&[node.id])
        .unwrap();
    assert_eq!(docs.len(), 1);
    let doc = docs.remove(0);
    assert_eq!(
        doc.start_line, node.start_line,
        "public metadata stays expanded"
    );
    assert!(semantic_doc_text_budget_cost(&doc.doc_text) <= 128);
    for dense in &pending {
        assert_eq!(
            (dense.start_line, dense.end_line),
            (node.start_line, node.end_line)
        );
        assert_eq!(dense.doc_text, doc.doc_text);
    }
    (doc, pending)
}

fn raw_doc(
    node: &Node,
    path: &str,
    text: &str,
    context: &SemanticDocGraphContext,
    mode: SemanticDocAliasMode,
) -> String {
    build_llm_symbol_doc_text_with_policy(
        context,
        node,
        &node_display_name(node),
        Some(path),
        &HashMap::from([(path.into(), Some(text.into()))]),
        mode,
        128,
    )
}

#[test]
fn retained_type_documents_restore_precise_inputs_without_narrowing_public_source() {
    // Independent source/document snapshots: before name-span extraction and
    // after display expansion. Expected documents are not produced by a selector.
    let cases: Vec<RetainedType> =
        serde_json::from_str(include_str!("fixtures/rust_type_semantic_inputs.json")).unwrap();
    for case in cases {
        let text = format!("{}{}", "\n".repeat(case.first_line - 1), case.source);
        let mut context = semantic_policy_context(&case.path, &case.node);
        context.child_labels.insert(case.node.id, case.children);
        context.edge_digests.insert(case.node.id, case.digest);
        let mode = SemanticDocAliasMode::CurrentAlias;
        let mut before = case.node.clone();
        before.start_line = Some(case.precise.start_line);
        before.start_col = Some(case.precise.start_col);
        before.end_line = Some(case.precise.end_line);
        before.end_col = Some(case.precise.end_col);
        assert_eq!(
            raw_doc(&before, &case.path, &text, &context, mode),
            case.before
        );
        assert_eq!(
            raw_doc(&case.node, &case.path, &text, &context, mode),
            case.expanded
        );
        let (actual, pending) = materialize(
            &case.node,
            &case.path,
            &text,
            &context,
            &[declaration(&case.node, case.precise)],
            mode,
        );
        assert_eq!(
            actual.attached_comment_text.as_deref(),
            Some(case.attached.as_str())
        );
        assert_eq!(
            pending.len(),
            1,
            "retained public type must have dense input"
        );
        assert_eq!(
            actual.doc_text, case.before,
            "{} compact input drift",
            case.node.serialized_name
        );
    }
}

fn short_type() -> (Node, SourceLocation, &'static str) {
    let node = Node {
        start_line: Some(2),
        start_col: Some(1),
        end_line: Some(5),
        end_col: Some(2),
        qualified_name: Some("T".into()),
        ..semantic_policy_node(10, NodeKind::STRUCT, "T", 1)
    };
    let anchor = SourceLocation {
        file_node_id: CoreNodeId(1),
        start_line: 3,
        start_col: 12,
        end_line: 3,
        end_col: 13,
    };
    (
        node,
        anchor,
        "/// boundary\n#[repr(C)]\npub struct T {\n    pub value: u8,\n}\n",
    )
}

#[test]
fn precise_type_view_also_restores_visible_identity_line() {
    let (node, anchor, text) = short_type();
    let context = semantic_policy_context("src/types.rs", &node);
    let valid = declaration(&node, anchor);
    let reference = Occurrence {
        kind: OccurrenceKind::REFERENCE,
        location: SourceLocation {
            start_line: 4,
            start_col: 16,
            end_line: 4,
            end_col: 18,
            ..valid.location.clone()
        },
        ..valid.clone()
    };
    let definition = Occurrence {
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            start_col: 1,
            end_col: 15,
            ..valid.location.clone()
        },
        ..valid.clone()
    };
    let (actual, _) = materialize(
        &node,
        "src/types.rs",
        text,
        &context,
        &[reference, valid, definition],
        SemanticDocAliasMode::NoAlias,
    );
    assert!(actual.doc_text.contains("line: 3\n"), "{}", actual.doc_text);
    assert!(!actual.doc_text.contains("line: 2\n"));
    assert!(actual.doc_text.contains("signature: pub struct T {"));
    assert_eq!(actual.attached_comment_text.as_deref(), Some("boundary"));
}

#[test]
fn unqualified_type_declaration_rows_keep_the_display_input() {
    let (node, anchor, text) = short_type();
    let context = semantic_policy_context("src/types.rs", &node);
    let valid = declaration(&node, anchor);
    let changed = |edit: fn(&mut Occurrence)| {
        let mut row = valid.clone();
        edit(&mut row);
        vec![row]
    };
    let cases = [
        ("missing", vec![]),
        ("identical duplicates", vec![valid.clone(), valid.clone()]),
        (
            "ambiguous",
            vec![
                valid.clone(),
                Occurrence {
                    location: SourceLocation {
                        start_col: 11,
                        end_col: 13,
                        ..valid.location.clone()
                    },
                    ..valid.clone()
                },
            ],
        ),
        (
            "foreign file",
            changed(|o| o.location.file_node_id = CoreNodeId(2)),
        ),
        ("wrong element", changed(|o| o.element_id = 11)),
        (
            "outside lines",
            changed(|o| {
                o.location.start_line = 6;
                o.location.end_line = 6;
            }),
        ),
        (
            "outside column",
            changed(|o| {
                o.location.start_line = 5;
                o.location.end_line = 5;
                o.location.start_col = 2;
                o.location.end_col = 3;
            }),
        ),
        (
            "zero line",
            changed(|o| {
                o.location.start_line = 0;
                o.location.end_line = 0;
            }),
        ),
        ("zero column", changed(|o| o.location.start_col = 0)),
        ("reversed lines", changed(|o| o.location.start_line = 4)),
        ("reversed columns", changed(|o| o.location.end_col = 11)),
        ("reference", changed(|o| o.kind = OccurrenceKind::REFERENCE)),
        (
            "definition",
            changed(|o| o.kind = OccurrenceKind::DEFINITION),
        ),
    ];
    let expected = raw_doc(
        &node,
        "src/types.rs",
        text,
        &context,
        SemanticDocAliasMode::NoAlias,
    );
    for (label, rows) in cases {
        let (actual, _) = materialize(
            &node,
            "src/types.rs",
            text,
            &context,
            &rows,
            SemanticDocAliasMode::NoAlias,
        );
        assert_eq!(actual.doc_text, expected, "{label}");
    }
}

#[test]
fn precise_type_anchors_do_not_narrow_callables_variants_or_other_languages() {
    for (kind, path, text, start, end, name_line) in [
        (
            NodeKind::FUNCTION,
            "src/lib.rs",
            "#[track_caller]\npub fn work() {\n    retain_body();\n}\n",
            1,
            4,
            2,
        ),
        (
            NodeKind::METHOD,
            "src/lib.rs",
            "#[track_caller]\npub fn work() {\n    retain_body();\n}\n",
            1,
            4,
            2,
        ),
        (
            NodeKind::ENUM_CONSTANT,
            "src/lib.rs",
            "enum E {\n    Work {\n        member: u8,\n    },\n}\n",
            2,
            4,
            2,
        ),
        (
            NodeKind::STRUCT,
            "src/types.hpp",
            "// declaration\nstruct Work {\n    unsigned member;\n};\n",
            2,
            4,
            2,
        ),
    ] {
        let node = Node {
            start_line: Some(start),
            start_col: Some(1),
            end_line: Some(end),
            end_col: Some(8),
            ..semantic_policy_node(10, kind, "Work", 1)
        };
        let context = semantic_policy_context(path, &node);
        let anchor = SourceLocation {
            file_node_id: CoreNodeId(1),
            start_line: name_line,
            start_col: 5,
            end_line: name_line,
            end_col: 9,
        };
        let expected = raw_doc(&node, path, text, &context, SemanticDocAliasMode::NoAlias);
        let (actual, _) = materialize(
            &node,
            path,
            text,
            &context,
            &[declaration(&node, anchor)],
            SemanticDocAliasMode::NoAlias,
        );
        assert_eq!(actual.doc_text, expected, "{kind:?} {path}");
        assert!(actual.doc_text.contains("body_summary:"));
    }
}
