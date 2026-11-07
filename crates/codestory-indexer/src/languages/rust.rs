//! Rust extraction rules.
//!
//! Rust's graph extraction lives here: the split rule assets (`rust.graph.scm`
//! plus the tags query that no other language pairs with a graph file), the
//! compiled-rule cache, the parser constructor, and the projection facts that
//! used to be spelled `"rust"` in four separate `match` statements — the `::`
//! qualified-name delimiter, the C-style route-comment claim, the dedicated
//! semantic resolver, and the `rust` family bucket. Every language-keyed
//! dispatch in the crate reaches them through [`super::EXTRACTIONS`] rather
//! than by naming the language.
//!
//! Rust method-call placeholders carry a syntax marker even when the file-local
//! receiver inference cannot prove an owner. That marker keeps an untyped
//! `worker.execute()` call unresolved instead of letting the later name-only
//! resolver attach it to an unrelated globally unique `execute` method.
//!
//! Several Rust surfaces are deliberately *not* here yet, and all of them are
//! shared seams rather than registry rows:
//!
//! * `lib.rs::apply_rust_receiver_call_hints` and its helper cluster. Rust's
//!   receiver-call inference does not run through `language_receiver_call_specs`
//!   — it rewrites already-built nodes behind a `language_name == "rust"` check
//!   in `index_file` — so the registry's `receiver_call_specs` hook cannot
//!   express it. Routing it would mean a new `LanguageExtraction` field, which
//!   is one change for all sixteen languages, not part of Rust's rollback unit.
//! * `collect_rust_macro_call_edges`, `collect_rust_generic_type_argument_edges`,
//!   `classify_rust_visibility`, `reconcile_rust_impl_anchors`,
//!   `normalize_rust_impl_expr_surface`, `is_rust_local_symbol_import_path`,
//!   `collect_rust_web_route` and `rust_function_name`: same reason. Each hangs
//!   off a per-feature dispatch with its own non-uniform signature.
//! * `LanguageRuleset::Rust`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! The original language-module split preserved extraction behavior;
//! `tests/language_extraction_snapshot.rs` pins both Rust fixture projections.
//! Displayed callable and struct/enum declarations retain attached outer
//! attributes and complete source extents after precise occurrences, symbol
//! identity and execution ownership have been resolved.

use std::collections::HashMap;
use std::sync::OnceLock;

use codestory_contracts::graph::{Node, NodeKind};
use tree_sitter::{Node as TsNode, Tree};

use super::LanguageExtraction;
use crate::{
    CompiledLanguageRules, GraphNodeSpan, LanguageRuleset, ts_node_graph_span, walk_tree_nodes,
};

const GRAPH_QUERY: &str = include_str!("../../rules/rust.graph.scm");
const TAGS_QUERY: &str = include_str!("../../rules/rust.tags.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// Callsite marker written onto edges produced from Rust method-call syntax.
pub(crate) const MEMBER_CALLSITE_MARKER: &str = "syntax:rust-method-call";

/// The single registry row for Rust.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["rust"],
    language_name: "rust",
    extensions: &["rs"],
    ruleset: LanguageRuleset::Rust,
    parser_language: rust_language,
    graph_query: GRAPH_QUERY,
    tags_query: Some(TAGS_QUERY),
    compiled_rules: &RULES,
    // Rust's receiver-call inference is not a `language_receiver_call_specs`
    // collector; see the module docs.
    member_edge_specs: None,
    receiver_call_specs: None,
    type_usage_specs: None,
    callsite_marker_families: &[("rust_method", MEMBER_CALLSITE_MARKER)],
    // Rust's rule file already emits METHOD for `impl` members, so the
    // FUNCTION-to-METHOD promotion must stay off; turning it on would reclassify
    // free functions declared inside a type-like owner.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: "::",
    route_comments_are_c_style: true,
    // `semantic::RustSemanticResolver` is private to that module, so the
    // registry records the choice and the residual match constructs it.
    uses_generic_semantic_resolver: false,
    semantic_family: "rust",
};

fn rust_language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

pub(crate) const DECLARATION_REVISION: &str = extraction_revision!("rust-declaration-source-v6");

pub(crate) fn callable_declaration_span(node: TsNode<'_>) -> GraphNodeSpan {
    if !matches!(node.kind(), "function_item" | "function_signature_item") {
        return ts_node_graph_span(node);
    }
    outer_attribute_envelope(node)
}

fn outer_attribute_envelope(node: TsNode<'_>) -> GraphNodeSpan {
    let mut span = ts_node_graph_span(node);
    let mut previous = node.prev_named_sibling();
    while let Some(sibling) = previous {
        match sibling.kind() {
            "attribute_item" => {
                let attribute = ts_node_graph_span(sibling);
                span.start_line = attribute.start_line;
                span.start_col = attribute.start_col;
            }
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        previous = sibling.prev_named_sibling();
    }
    span
}

/// Expand displayed source only after precise occurrences, ownership and IDs are resolved.
pub(crate) fn apply_declaration_envelopes(tree: &Tree, nodes: &mut [Node]) {
    if tree.root_node().has_error() {
        return;
    }
    let mut envelopes = HashMap::<(NodeKind, GraphNodeSpan), GraphNodeSpan>::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        let (span, kinds): (_, &[NodeKind]) = match node.kind() {
            "function_item" | "function_signature_item" => (
                ts_node_graph_span(node),
                &[NodeKind::FUNCTION, NodeKind::METHOD],
            ),
            "struct_item" | "enum_item" => {
                // Type graph rows still carry the precise name-token anchor;
                // their occurrences and identities were captured before this stage.
                let Some(name) = node.child_by_field_name("name") else {
                    return;
                };
                let kinds: &[NodeKind] = if node.kind() == "struct_item" {
                    &[NodeKind::STRUCT]
                } else {
                    &[NodeKind::ENUM]
                };
                (ts_node_graph_span(name), kinds)
            }
            _ => return,
        };
        let envelope = outer_attribute_envelope(node);
        if envelope != span {
            for &kind in kinds {
                envelopes.insert((kind, span), envelope);
            }
        }
    });
    for node in nodes {
        let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) =
            (node.start_line, node.start_col, node.end_line, node.end_col)
        else {
            continue;
        };
        if let Some(envelope) = envelopes.get(&(
            node.kind,
            GraphNodeSpan {
                start_line,
                start_col,
                end_line,
                end_col,
            },
        )) {
            node.start_line = Some(envelope.start_line);
            node.start_col = Some(envelope.start_col);
            node.end_line = Some(envelope.end_line);
            node.end_col = Some(envelope.end_col);
        }
    }
}

#[cfg(test)]
mod type_source_extent_tests {
    use std::path::Path;

    use codestory_contracts::graph::{EdgeKind, OccurrenceKind};

    use super::*;
    use crate::{IndexResult, generate_id, get_language_for_ext, index_file};

    const SOURCE: &str = "pub struct Record<T> {\n    pub value: T,\n}\npub struct Tuple<T>(\n    pub T,\n);\npub struct Unit;\npub enum Choice<T> {\n    Record { value: T },\n    Tuple(T),\n    Unit,\n}\nmod nested {\n    pub struct Record {\n        pub flag: bool,\n    }\n    pub fn helper() {}\n    pub fn invoke(value: Record) { helper(); }\n}\npub fn invoke(value: Record<u8>) { nested::helper(); }\n";

    fn extract(source: &str) -> IndexResult {
        index_file(
            Path::new("type_extents.rs"),
            source,
            &get_language_for_ext("rs").unwrap(),
            None,
            None,
        )
        .unwrap()
    }

    fn named<'a>(result: &'a IndexResult, kind: NodeKind, name: &str) -> &'a Node {
        let matches = result
            .nodes
            .iter()
            .filter(|node| node.kind == kind && node.qualified_name.as_deref() == Some(name))
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "expected one {kind:?} {name}");
        matches[0]
    }

    fn span(node: &Node) -> (u32, u32, u32, u32) {
        (
            node.start_line.unwrap(),
            node.start_col.unwrap(),
            node.end_line.unwrap(),
            node.end_col.unwrap(),
        )
    }

    #[test]
    fn rust_type_display_extents_include_fields_variants_and_terminators() {
        let result = extract(SOURCE);
        for (kind, name, expected) in [
            (NodeKind::STRUCT, "Record", (1, 1, 3, 2)),
            (NodeKind::STRUCT, "Tuple", (4, 1, 6, 3)),
            (NodeKind::STRUCT, "Unit", (7, 1, 7, 17)),
            (NodeKind::ENUM, "Choice", (8, 1, 12, 2)),
            (NodeKind::STRUCT, "nested::Record", (14, 5, 16, 6)),
        ] {
            assert_eq!(span(named(&result, kind, name)), expected, "{name}");
        }
        let attributed = extract(
            "// unrelated comment\n#[repr(C)]\n// between attributes\n#[derive(Clone)]\npub struct Attributed {\n    pub value: u8,\n}\n#[cfg(feature = \"optional\")]\npub enum Mode {\n    Active,\n}\n",
        );
        assert_eq!(
            span(named(&attributed, NodeKind::STRUCT, "Attributed")),
            (2, 1, 7, 2)
        );
        assert_eq!(
            span(named(&attributed, NodeKind::ENUM, "Mode")),
            (8, 1, 11, 2)
        );
    }

    #[test]
    fn rust_type_display_extents_preserve_precise_names_ids_and_ownership() {
        let result = extract(SOURCE);
        let shifted = extract(&format!("// unrelated header\n{SOURCE}"));
        for (kind, name, anchor) in [
            (NodeKind::STRUCT, "Record", (1, 12, 1, 18)),
            (NodeKind::STRUCT, "Tuple", (4, 12, 4, 17)),
            (NodeKind::STRUCT, "Unit", (7, 12, 7, 16)),
            (NodeKind::ENUM, "Choice", (8, 10, 8, 16)),
            (NodeKind::STRUCT, "nested::Record", (14, 16, 14, 22)),
        ] {
            let node = named(&result, kind, name);
            let canonical_id = format!("type_extents.rs:{name}");
            assert_eq!(node.canonical_id.as_deref(), Some(canonical_id.as_str()));
            assert_eq!(node.id.0, generate_id(&canonical_id));
            assert_eq!(node.id, named(&shifted, kind, name).id);
            let occurrences = result
                .occurrences
                .iter()
                .filter(|occurrence| {
                    occurrence.element_id == node.id.0
                        && occurrence.kind == OccurrenceKind::DECLARATION
                })
                .collect::<Vec<_>>();
            assert_eq!(occurrences.len(), 1, "{name}");
            let location = &occurrences[0].location;
            assert_eq!(
                (
                    location.start_line,
                    location.start_col,
                    location.end_line,
                    location.end_col
                ),
                anchor,
                "{name} declaration must remain a name token"
            );
        }
        assert_ne!(
            named(&result, NodeKind::STRUCT, "Record").id,
            named(&result, NodeKind::STRUCT, "nested::Record").id
        );
        let owner = named(&result, NodeKind::FUNCTION, "nested::invoke");
        let helper = named(&result, NodeKind::FUNCTION, "nested::helper");
        let targets = |indexed: &IndexResult| {
            indexed
                .edges
                .iter()
                .filter(|edge| edge.kind == EdgeKind::CALL && edge.effective_source() == owner.id)
                .map(|edge| edge.effective_target())
                .collect::<Vec<_>>()
        };
        let baseline_targets = targets(&result);
        assert_eq!(baseline_targets.len(), 1, "nested callable owns one call");
        if baseline_targets[0] != helper.id {
            // index_file may retain an unresolved reference; display extents
            // must preserve that target rather than require new resolution.
            let reference = result
                .nodes
                .iter()
                .find(|node| node.id == baseline_targets[0])
                .unwrap();
            assert_eq!(reference.kind, NodeKind::UNKNOWN);
            assert!(crate::node_matches_name(reference, "helper"));
        }
        assert_eq!(
            targets(&shifted),
            baseline_targets,
            "the nested callable must retain its exact call target after a header shift"
        );
    }

    #[test]
    fn rust_type_display_extents_do_not_promote_enum_variants() {
        let result = extract(SOURCE);
        for (name, expected) in [
            ("Choice::Record", (9, 5, 9, 11)),
            ("Choice::Tuple", (10, 5, 10, 10)),
            ("Choice::Unit", (11, 5, 11, 9)),
        ] {
            let variant = named(&result, NodeKind::ENUM_CONSTANT, name);
            assert_eq!(span(variant), expected);
            assert!(result.edges.iter().any(|edge| {
                edge.kind == EdgeKind::MEMBER
                    && edge.source == named(&result, NodeKind::ENUM, "Choice").id
                    && edge.target == variant.id
            }));
        }
    }

    #[test]
    fn rust_type_display_extents_fail_closed_on_malformed_syntax() {
        let source = "pub struct Intact {\n    pub value: u8,\n}\nfn broken( {\n";
        let result = extract(source);
        assert_eq!(
            span(named(&result, NodeKind::STRUCT, "Intact")),
            (1, 12, 1, 18)
        );
    }
}
