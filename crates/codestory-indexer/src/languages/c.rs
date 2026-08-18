//! C extraction rules.
//!
//! C's graph extraction lives here: the tree-sitter grammar, the rule file, and
//! the compiled-rule cache, plus the projection facts that used to be spelled
//! `"c"` in a handful of `match` arms scattered through `lib.rs` — the `::`
//! qualified-name delimiter, the `native` semantic family, and the fact that C
//! has neither a manual receiver-call engine nor a member-call syntax marker.
//! Every language-keyed dispatch in the crate reaches those through
//! [`super::EXTRACTIONS`] rather than by spelling `"c"`.
//!
//! Four C surfaces are deliberately *not* here, and each is a shared seam
//! rather than C content:
//!
//! * `lib.rs::infer_header_language_config`, which picks between C and C++ for
//!   a bare `.h` from compilation-database evidence instead of from the
//!   extension registry. It now builds its C branch out of [`EXTRACTION`], but
//!   the decision itself belongs to the C/C++ header seam, not to C.
//! * `lib.rs::collect_declaration_span_overrides`, whose `"c"` arm has no
//!   [`super::LanguageExtraction`] field to land in. Giving it one would add a
//!   field for all sixteen languages at once, which is a different rollback
//!   unit.
//! * `lib.rs::append_manual_c_enum_member_edges` and
//!   `infer_cpp_access_from_tree`, which answer for `"c"` *and* `"cpp"` from
//!   one body. They move when C++ moves, not before.
//! * `LanguageRuleset::C`, which stays in `lib.rs` because the enum is the
//!   compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. The parser config below is the one that used
//! to sit in `language_configs.rs`, and `tests/language_extraction_snapshot.rs`
//! pins the rendered projection of both C fixtures so the move stays
//! output-equal.

use std::sync::OnceLock;

use super::LanguageExtraction;
use crate::{CompiledLanguageRules, LanguageRuleset};

const GRAPH_QUERY: &str = include_str!("../../rules/c.scm");

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for C.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["c"],
    language_name: "c",
    // `h` is here because `get_language_for_ext("h")` resolved to the C parser
    // before the move; the C/C++ header inference in `lib.rs` overrides that
    // choice on the path-based route, but the extension-only route must keep
    // answering `c`.
    extensions: &["c", "h"],
    ruleset: LanguageRuleset::C,
    parser_language: c_language,
    graph_query: GRAPH_QUERY,
    tags_query: None,
    compiled_rules: &RULES,
    // C has no manual receiver-call engine: `language_receiver_call_specs`
    // never had a `"c"` arm, so member calls come from the rule file alone.
    member_edge_specs: None,
    receiver_call_specs: None,
    type_usage_specs: None,
    // ...and therefore no member-call syntax marker either. `rules/c.scm`
    // emits no `call_syntax`, so there is nothing for the marker match to key.
    callsite_marker_families: &[],
    // C struct members are fields, including function pointers; nothing in C
    // projects a FUNCTION under a type-like owner, so the promotion that Kotlin
    // and Swift need was never enabled for C.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: "::",
    // The framework-route scanner's C-style-comment roster deliberately does
    // not list C — it lists the languages that carry HTTP route declarations.
    // Adding C here would change what the route scanner claims, not tidy it.
    route_comments_are_c_style: false,
    // `semantic::CSemanticResolver` is private to that module, so the registry
    // records the choice and `dedicated_semantic_resolver` still builds it.
    uses_generic_semantic_resolver: false,
    semantic_family: "native",
};

fn c_language() -> tree_sitter::Language {
    tree_sitter_c::LANGUAGE.into()
}
