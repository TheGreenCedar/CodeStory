//! Bash extraction rules.
//!
//! Bash's graph extraction lives here: the tree-sitter grammar, the rule file,
//! the compiled-rule cache, and the projection facts (no method promotion, a
//! `.` qualified-name delimiter, hash comments rather than C-style ones, the
//! shared name-only semantic resolver, and its own family bucket). Every
//! language-keyed dispatch in the crate reaches them through
//! [`super::EXTRACTIONS`] rather than by spelling `"bash"`.
//!
//! Bash has no manual receiver-call engine and no member-call syntax, so
//! `receiver_call_specs` is `None` and `callsite_marker_families` is empty:
//! shell has no receiver-qualified call form for the resolver to aim at, and
//! the rule file emits no `call_syntax` attribute.
//!
//! Two Bash surfaces are deliberately *not* here, and both are shared seams
//! rather than Bash content:
//!
//! * `lib.rs::collect_bash_source_import_specs` and its `"bash"` arm in
//!   `collect_runtime_import_specs`. The per-language runtime-import
//!   collectors have no [`super::LanguageExtraction`] field, so relocating one
//!   would leave the `"bash"` spelling in the dispatch and only move code;
//!   routing them through the registry is one change for all sixteen
//!   languages, exactly as `collect_ktor_route` was left out of Kotlin's
//!   rollback unit.
//! * `LanguageRuleset::Bash`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The parser config below is the one that used
//! to sit in `language_configs.rs::bash`, and
//! `tests/language_extraction_snapshot.rs` pins the rendered projection of
//! both Bash fixtures so the move stays output-equal.

use std::sync::OnceLock;

use super::LanguageExtraction;
use crate::{CompiledLanguageRules, LanguageRuleset};

const GRAPH_QUERY: &str = include_str!("../../rules/bash.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for Bash.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["bash"],
    language_name: "bash",
    extensions: &["sh", "bash"],
    ruleset: LanguageRuleset::Bash,
    parser_language: bash_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    member_edge_specs: None,
    receiver_call_specs: None,
    type_usage_specs: None,
    callsite_marker_families: &[],
    // Shell has no type-like owners, so a `function_definition` never projects
    // as METHOD; Bash was absent from the promotion roster in `lib.rs`.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    // Shell comments start with `#`; Bash was absent from the C-style roster
    // in `route_language_uses_c_style_comments`.
    route_comments_are_c_style: false,
    uses_generic_semantic_resolver: true,
    semantic_family: "bash",
};

fn bash_language() -> tree_sitter::Language {
    tree_sitter_bash::LANGUAGE.into()
}
