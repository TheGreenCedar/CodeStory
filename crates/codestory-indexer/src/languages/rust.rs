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
//! Callable declaration envelopes now also retain attached outer attributes
//! after symbol identity and execution ownership have been resolved.

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

pub(crate) const CALLABLE_DECLARATION_REVISION: &str = "rust-callable-declaration-v4";

pub(crate) fn callable_declaration_span(node: TsNode<'_>) -> GraphNodeSpan {
    let mut span = ts_node_graph_span(node);
    if !matches!(node.kind(), "function_item" | "function_signature_item") {
        return span;
    }
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

/// Expand displayed source only after lexical ownership and stable identities are resolved.
pub(crate) fn apply_callable_declaration_envelopes(tree: &Tree, nodes: &mut [Node]) {
    if tree.root_node().has_error() {
        return;
    }
    let mut envelopes = HashMap::<GraphNodeSpan, GraphNodeSpan>::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if !matches!(node.kind(), "function_item" | "function_signature_item") {
            return;
        }
        let span = ts_node_graph_span(node);
        let envelope = callable_declaration_span(node);
        if envelope != span {
            envelopes.insert(span, envelope);
        }
    });
    for node in nodes {
        if !matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
            continue;
        }
        let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) =
            (node.start_line, node.start_col, node.end_line, node.end_col)
        else {
            continue;
        };
        if let Some(envelope) = envelopes.get(&GraphNodeSpan {
            start_line,
            start_col,
            end_line,
            end_col,
        }) {
            node.start_line = Some(envelope.start_line);
            node.start_col = Some(envelope.start_col);
        }
    }
}
