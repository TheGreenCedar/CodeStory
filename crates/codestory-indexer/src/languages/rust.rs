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
//! Rust carries no `member_callsite_marker`/`graph_call_syntax` pair: its rule
//! file emits no `call_syntax` attribute, so the callsite-marker match has no
//! Rust arm to move. Both fields are therefore `None`, which the registry's
//! pairing invariant requires.
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
//! This is a move, not a rewrite. `tests/language_extraction_snapshot.rs` pins
//! the rendered projection of both Rust fixtures so the move stays
//! output-equal.

use std::sync::OnceLock;

use super::LanguageExtraction;
use crate::{CompiledLanguageRules, LanguageRuleset};

const GRAPH_QUERY: &str = include_str!("../../rules/rust.graph.scm");
const TAGS_QUERY: &str = include_str!("../../rules/rust.tags.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

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
    member_callsite_marker: None,
    graph_call_syntax: None,
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
