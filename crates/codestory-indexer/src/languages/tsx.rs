//! TSX extraction rules.
//!
//! TSX is the row the [`super::LanguageExtraction::dispatch_names`] field
//! exists for. `.tsx` is not a language in the public registry — the contracts
//! registry routes it to `typescript` alongside `.ts`, `.mts` and `.cts` — but
//! the indexer has always kept it as a *separate parser configuration*, because
//! it needs the JSX-aware grammar (`LANGUAGE_TSX`) and its own rule file. So
//! this row answers to the indexer-only dispatch name `tsx` while reporting
//! `typescript` as its public `language_name`, and `extraction_for_ext` keys on
//! the extension so `tsx` and `ts` stay two configs for one public language.
//!
//! Three TSX-adjacent surfaces are deliberately *not* here, and all three are
//! shared with a language whose own S3 package has not landed:
//!
//! * `lib.rs::collect_typescript_receiver_call_edges`. TSX and TypeScript run
//!   the same receiver-call engine; the body belongs to TypeScript (#1681) and
//!   this row only points at it. Copying it here would fork one engine into
//!   two.
//! * `TS_MEMBER_CALLSITE_MARKER` and the `ts_member` `call_syntax` arm.
//!   `rules/tsx.graph.scm` and `rules/typescript.graph.scm` emit the *same*
//!   `call_syntax = "ts_member"` and share one marker constant, so the marker
//!   is TypeScript's to move, not TSX's. Claiming it here would make the
//!   registry's `call_syntax` uniqueness check fail the day #1681 lands, and
//!   would park a TypeScript constant inside the TSX module. Both marker
//!   fields are therefore `None` and the residual `ts_member` arm in `lib.rs`
//!   keeps answering — identically — for `.tsx` and `.ts` alike.
//! * `lib.rs::collect_tsx_jsx_usage_edges` and the `reconcile_tsx_usage_targets`
//!   / `prune_tsx_duplicate_reference_nodes` pair. Their name says `tsx` but
//!   their gate is `is_jsx_like_file`, which is true for `.jsx` as well, and
//!   `.jsx` is JavaScript (#1680). They are a JSX-dialect seam across two
//!   languages, not TSX content.
//!
//! `LanguageRuleset::Tsx` also stays in `lib.rs`, for the same reason Kotlin's
//! does: the enum is the compiled-rule cache key for every language at once.
//!
//! This is a move, not a rewrite. `tests/language_extraction_snapshot.rs` pins
//! the rendered projection of both TSX fixtures so the move stays output-equal.

use super::LanguageExtraction;
use crate::{CompiledLanguageRules, LanguageRuleset, TYPESCRIPT_TAGS_QUERY};
use std::sync::OnceLock;

const GRAPH_QUERY: &str = include_str!("../../rules/tsx.graph.scm");

/// TSX reuses TypeScript's tags query verbatim; it always has.
const TAGS_QUERY: &str = TYPESCRIPT_TAGS_QUERY;

static RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

/// The single registry row for TSX.
pub(crate) const EXTRACTION: LanguageExtraction = LanguageExtraction {
    dispatch_names: &["tsx"],
    language_name: "typescript",
    extensions: &["tsx"],
    ruleset: LanguageRuleset::Tsx,
    parser_language: tsx_language,
    graph_query: GRAPH_QUERY,
    tags_query: Some(TAGS_QUERY),
    compiled_rules: &RULES,
    // Shared with TypeScript; see the module doc.
    receiver_call_specs: Some(crate::collect_typescript_receiver_call_edges),
    // `ts_member` is TypeScript's marker, not TSX's; see the module doc.
    member_callsite_marker: None,
    graph_call_syntax: None,
    // A `method_definition` is already a METHOD in the TSX grammar, so the
    // FUNCTION-to-METHOD promotion never applied to TSX and must not start.
    promotes_type_member_functions_to_methods: false,
    qualified_name_delimiter: ".",
    // Deliberately `false`, and deliberately different from the CLI's comment
    // roster. The framework-route scanner reaches languages by their *public*
    // registry name, so a `.tsx` file is scanned as `typescript` and the
    // dispatch name `tsx` has never been in that roster. Flipping this to
    // `true` would not fix anything — it would add a route-claim surface that
    // no `.tsx` file has today.
    route_comments_are_c_style: false,
    // TypeScript has a dedicated resolver, and `.tsx` files reach it as
    // `typescript` because `detect_resolver_language` returns the public
    // registry name.
    uses_generic_semantic_resolver: false,
    semantic_family: "webscript",
};

fn tsx_language() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TSX.into()
}
