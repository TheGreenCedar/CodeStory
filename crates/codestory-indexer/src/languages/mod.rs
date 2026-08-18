//! Per-language extraction rules, one module per language, behind a registry.
//!
//! ARCH-012: the indexer used to spread each language across a dozen
//! hand-maintained name rosters — parser configs, compiled-rule caches,
//! receiver-call dispatch, name-projection flags, comment style, resolver
//! selection, family buckets — so adding or auditing a language meant finding
//! every match arm that spelled its name. A migrated language instead owns one
//! module here and appears exactly once, as one [`LanguageExtraction`] row in
//! [`EXTRACTIONS`]; every consumer iterates the registry.
//!
//! The migration is one language per rollback unit (Kotlin first, then #1677
//! through #1691). Until a language has moved, its rows stay in the residual
//! `match` at each call site, and each consumer therefore reads
//! "registry first, residual match second". That ordering is behaviour-neutral
//! because the registry and the residual match are disjoint by construction —
//! `registry_rows_do_not_shadow_unmigrated_languages` proves it — and the last
//! package to land deletes the residual arms entirely.

pub(crate) mod bash;
pub(crate) mod c;
pub(crate) mod cpp;
pub(crate) mod csharp;
pub(crate) mod dart;
pub(crate) mod go;
pub(crate) mod java;
pub(crate) mod javascript;
pub(crate) mod kotlin;
pub(crate) mod php;
pub(crate) mod ruby;
pub(crate) mod swift;
pub(crate) mod typescript;

use std::sync::OnceLock;

use tree_sitter::{Language, Tree};

use crate::{
    CompiledLanguageRules, LanguageRuleset, ManualMemberEdgeSpec, ManualReceiverCallSpec,
    ManualTypeUsageSpec,
};

/// Everything the indexer knows about one migrated language.
///
/// Each field replaces one hand-kept roster entry. A new language fills the
/// struct once instead of editing the dispatches this struct feeds.
pub(crate) struct LanguageExtraction {
    /// Every language name this row answers to in the indexer's language-keyed
    /// dispatches. Normally one name; TSX exists as its own dispatch name while
    /// still reporting `typescript` as its public `language_name`.
    pub(crate) dispatch_names: &'static [&'static str],
    /// Public registry name from `codestory_contracts::language_support`.
    pub(crate) language_name: &'static str,
    /// Extensions this row serves. Must be a subset of the contracts registry's
    /// extensions for `language_name`; the drift test below enforces it.
    pub(crate) extensions: &'static [&'static str],
    /// Discriminant used by the compiled-rule cache.
    pub(crate) ruleset: LanguageRuleset,
    /// Tree-sitter grammar constructor.
    pub(crate) parser_language: fn() -> Language,
    /// Graph DSL rule file contents.
    pub(crate) graph_query: &'static str,
    /// Optional tags query contents.
    pub(crate) tags_query: Option<&'static str>,
    /// Process-wide cache for the compiled graph/tags rules.
    pub(crate) compiled_rules: &'static OnceLock<Result<CompiledLanguageRules, String>>,
    /// Manual MEMBER-edge collector, when the language has one. Added by the
    /// first migrated language that owns one (Go); Kotlin's rules produce
    /// MEMBER edges from the rule file alone and its row is `None`, which is
    /// exactly what the residual `_ => Vec::new()` arm gave it.
    pub(crate) member_edge_specs: Option<fn(&Tree, &str) -> Vec<ManualMemberEdgeSpec>>,
    /// Manual receiver-call collector, when the language has one.
    pub(crate) receiver_call_specs: Option<fn(&Tree, &str) -> Vec<ManualReceiverCallSpec>>,
    /// Manual type-usage collector, when the language has one (C# today).
    /// A collector may emit a spec only for a type it resolved against its
    /// own binding tables — the engine stamps the resulting TYPE_USAGE edge
    /// `certainty = Certain` at emit on the strength of that gate (P2a).
    pub(crate) type_usage_specs: Option<fn(&Tree, &str) -> Vec<ManualTypeUsageSpec>>,
    /// `(call_syntax, marker)` families for callsite markers: every
    /// `call_syntax` value this row's rule file emits, paired with the marker
    /// written onto edges that carry it. Most languages own at most one family
    /// (their member-call syntax); PHP owns two (`php_member` member calls and
    /// `php_new` construction). Empty when the rule file emits no
    /// `call_syntax`, which is exactly what the old `None`/`None` pair meant.
    pub(crate) callsite_marker_families: &'static [(&'static str, &'static str)],
    /// Member functions of a type-like owner project as METHOD, not FUNCTION.
    pub(crate) promotes_type_member_functions_to_methods: bool,
    /// Separator between an owner and its member in qualified names.
    pub(crate) qualified_name_delimiter: &'static str,
    /// Framework-route text scanning strips `//` and `/* */` comments.
    pub(crate) route_comments_are_c_style: bool,
    /// Whether the language resolves through the shared name-only resolver.
    /// `false` means `semantic::dedicated_semantic_resolver` still constructs
    /// it, because those resolver types are private to that module.
    pub(crate) uses_generic_semantic_resolver: bool,
    /// Bucket used to keep semantic candidates inside one language family.
    pub(crate) semantic_family: &'static str,
}

/// Every language whose extraction rules have moved into this module tree.
pub(crate) const EXTRACTIONS: &[LanguageExtraction] = &[
    kotlin::EXTRACTION,
    java::EXTRACTION,
    cpp::EXTRACTION,
    c::EXTRACTION,
    javascript::EXTRACTION,
    typescript::EXTRACTION,
    tsx::EXTRACTION,
    python::EXTRACTION,
    rust::EXTRACTION,
    go::EXTRACTION,
    ruby::EXTRACTION,
    php::EXTRACTION,
    csharp::EXTRACTION,
    swift::EXTRACTION,
    dart::EXTRACTION,
    bash::EXTRACTION,
];

/// Look a row up by any of its dispatch names.
pub(crate) fn extraction_for_language(language_name: &str) -> Option<&'static LanguageExtraction> {
    EXTRACTIONS
        .iter()
        .find(|extraction| extraction.dispatch_names.contains(&language_name))
}

/// Look a row up by file extension. The extension is expected to be already
/// normalized (no leading dot, lowercase).
pub(crate) fn extraction_for_ext(ext: &str) -> Option<&'static LanguageExtraction> {
    EXTRACTIONS
        .iter()
        .find(|extraction| extraction.extensions.contains(&ext))
}

/// Look a row up by its compiled-rule discriminant.
pub(crate) fn extraction_for_ruleset(
    ruleset: LanguageRuleset,
) -> Option<&'static LanguageExtraction> {
    EXTRACTIONS
        .iter()
        .find(|extraction| extraction.ruleset == ruleset)
}

/// Callsite marker for a `call_syntax` value emitted by a migrated rule file.
pub(crate) fn member_callsite_marker_for_call_syntax(call_syntax: &str) -> Option<&'static str> {
    EXTRACTIONS.iter().find_map(|extraction| {
        extraction
            .callsite_marker_families
            .iter()
            .find(|(family_call_syntax, _)| *family_call_syntax == call_syntax)
            .map(|(_, marker)| *marker)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::language_support::{
        LanguageSupportMode, language_support_profile_for_ext,
        language_support_profile_for_language_name,
    };
    use std::collections::HashSet;

    /// Every registry row must agree with the public language registry.
    ///
    /// A row that claimed an extension the contracts registry routes elsewhere
    /// would silently steal files from another language's parser.
    #[test]
    fn registry_rows_agree_with_the_public_language_registry() {
        for extraction in EXTRACTIONS {
            assert!(
                !extraction.extensions.is_empty(),
                "{} must claim at least one extension",
                extraction.language_name
            );
            for extension in extraction.extensions {
                let profile = language_support_profile_for_ext(extension)
                    .unwrap_or_else(|| panic!("`{extension}` has no public language profile"));
                assert_eq!(
                    profile.language_name, extraction.language_name,
                    "`{extension}` is routed to {} by the public registry",
                    profile.language_name
                );
                assert_eq!(
                    profile.support_mode,
                    LanguageSupportMode::ParserBackedGraph,
                    "`{extension}` is not a parser-backed claim",
                );
            }
            assert!(
                !extraction.dispatch_names.is_empty(),
                "{} must answer to at least one dispatch name",
                extraction.language_name
            );
            for dispatch_name in extraction.dispatch_names {
                // A dispatch name is either this row's own registry name, or a
                // finer-grained indexer-only name that the public registry does
                // not know (TSX). Anything else means the row would answer for
                // some *other* language's dispatches.
                assert!(
                    *dispatch_name == extraction.language_name
                        || language_support_profile_for_language_name(dispatch_name).is_none(),
                    "{} claims dispatch name `{dispatch_name}`, which the public registry \
                    assigns to another language",
                    extraction.language_name
                );
            }
            for (call_syntax, marker) in extraction.callsite_marker_families {
                assert!(
                    !call_syntax.trim().is_empty() && !marker.trim().is_empty(),
                    "{} must pair a non-empty call_syntax with a non-empty marker",
                    extraction.language_name
                );
            }
        }
    }

    /// Registry rows and the residual `match` arms must stay disjoint.
    ///
    /// Every consumer reads "registry first, residual match second". If a
    /// language were listed in both, the registry answer would silently shadow
    /// the residual arm and a behavioural difference between them would become
    /// invisible. Uniqueness inside the registry is the enforceable half of
    /// that; the residual half is enforced by each consumer's own test.
    #[test]
    fn registry_rows_do_not_shadow_unmigrated_languages() {
        let mut dispatch_names = HashSet::new();
        let mut extensions = HashSet::new();
        let mut rulesets = Vec::new();
        let mut call_syntaxes = HashSet::new();
        for extraction in EXTRACTIONS {
            for name in extraction.dispatch_names {
                assert!(
                    dispatch_names.insert(*name),
                    "dispatch name `{name}` is claimed twice"
                );
            }
            for extension in extraction.extensions {
                assert!(
                    extensions.insert(*extension),
                    "extension `{extension}` is claimed twice"
                );
            }
            assert!(
                !rulesets.contains(&extraction.ruleset),
                "ruleset {:?} is claimed twice",
                extraction.ruleset
            );
            rulesets.push(extraction.ruleset);
            for (call_syntax, _) in extraction.callsite_marker_families {
                assert!(
                    call_syntaxes.insert(*call_syntax),
                    "call_syntax `{call_syntax}` is claimed twice"
                );
            }
        }
    }

    /// The Kotlin row must keep the exact projection facts it had while it was
    /// spread across `lib.rs`. These four values used to live in four separate
    /// match statements; a wrong value here is a silent projection change that
    /// no threshold test would catch.
    #[test]
    fn kotlin_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let kotlin = extraction_for_language("kotlin").expect("kotlin row");
        assert!(kotlin.promotes_type_member_functions_to_methods);
        assert_eq!(kotlin.qualified_name_delimiter, ".");
        assert!(kotlin.route_comments_are_c_style);
        assert_eq!(kotlin.semantic_family, "kotlin");
        assert!(kotlin.uses_generic_semantic_resolver);
        assert_eq!(
            kotlin.callsite_marker_families,
            &[("kotlin_member", "syntax:kotlin-member-call")]
        );
        assert_eq!(
            member_callsite_marker_for_call_syntax("kotlin_member"),
            Some("syntax:kotlin-member-call")
        );
        assert!(kotlin.receiver_call_specs.is_some());
        assert!(kotlin.tags_query.is_none());
        assert_eq!(
            extraction_for_ext("kt").map(|row| row.language_name),
            Some("kotlin")
        );
        assert_eq!(
            extraction_for_ext("kts").map(|row| row.language_name),
            Some("kotlin")
        );
    }

    /// The C row must keep the exact projection facts it had while it was
    /// spread across `lib.rs`, `language_configs.rs` and `semantic/mod.rs`.
    ///
    /// C's rosters were mostly *absences* — no receiver-call engine, no
    /// member-call marker, no promotion of type members to methods, and
    /// deliberately no entry in the route scanner's C-style-comment list. An
    /// absence is exactly what a "fill in the obvious value" mistake turns into
    /// a silent behaviour change, and `promotes_type_member_functions_to_methods`
    /// is invisible to the C snapshots (no C fixture projects a FUNCTION under a
    /// type-like owner), so it is pinned here instead.
    #[test]
    fn c_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let c = extraction_for_language("c").expect("c row");
        assert!(!c.promotes_type_member_functions_to_methods);
        assert_eq!(c.qualified_name_delimiter, "::");
        assert!(!c.route_comments_are_c_style);
        assert_eq!(c.semantic_family, "native");
        assert!(!c.uses_generic_semantic_resolver);
        assert!(c.callsite_marker_families.is_empty());
        assert!(c.receiver_call_specs.is_none());
        assert!(c.tags_query.is_none());
        assert_eq!(
            extraction_for_ext("c").map(|row| row.language_name),
            Some("c")
        );
        // `h` resolved to the C parser through `get_language_for_ext` before
        // the move; the C/C++ header inference is a separate, path-based seam.
        assert_eq!(
            extraction_for_ext("h").map(|row| row.language_name),
            Some("c")
        );
    }

    /// The TSX row must keep the exact facts the god file gave the dispatch
    /// name `tsx`, including the three that are `false`/`None` on purpose.
    ///
    /// Every value below was read off a `match` arm — or off the *absence* of
    /// one — before the move. The absences are the dangerous half: nothing
    /// else in the suite notices if a row silently starts promoting member
    /// functions to methods, claims a call syntax that belongs to TypeScript,
    /// or joins the framework-route comment roster.
    #[test]
    fn tsx_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let tsx = extraction_for_language("tsx").expect("tsx row");

        // `.tsx` is `typescript` to the outside world and `tsx` inside the
        // indexer; that split is the reason `dispatch_names` exists.
        assert_eq!(tsx.language_name, "typescript");
        assert_eq!(tsx.dispatch_names, &["tsx"]);
        assert_eq!(
            extraction_for_ext("tsx").map(|row| row.language_name),
            Some("typescript")
        );

        // TSX and TypeScript are two rows sharing one public language name.
        // They must stay distinct: `.ts` and `.tsx` have different grammars and
        // different rule files, so a single row answering both extensions would
        // parse one of them with the wrong grammar.
        let typescript = extraction_for_language("typescript").expect("typescript row");
        assert_eq!(typescript.dispatch_names, &["typescript"]);
        assert_eq!(
            extraction_for_ext("ts").map(|row| row.ruleset),
            Some(typescript.ruleset)
        );
        assert_ne!(tsx.ruleset, typescript.ruleset);
        assert_ne!(tsx.graph_query, typescript.graph_query);

        // `promotes_type_member_functions_to_methods` was `matches!(name,
        // "swift" | "dart")` and `qualified_name_delimiter` was `"::"` only for
        // `rust`/`cpp`/`c`; neither ever matched `tsx`.
        assert!(!tsx.promotes_type_member_functions_to_methods);
        assert_eq!(tsx.qualified_name_delimiter, ".");
        // The framework-route comment roster reaches languages by their public
        // registry name, so `tsx` was never in it. This deliberately disagrees
        // with the CLI highlighter, which does render `tsx` comments; they are
        // separate facts with separate owners.
        assert!(!tsx.route_comments_are_c_style);

        // `rules/tsx.graph.scm` emits TypeScript's `ts_member` call syntax and
        // shares its marker constant rather than declaring its own. The TSX row
        // therefore leaves its marker-family table empty on purpose, and the
        // lookup is answered by the TypeScript row — claiming the syntax here
        // as well would give one `call_syntax` two owners.
        assert!(tsx.callsite_marker_families.is_empty());
        assert_eq!(
            member_callsite_marker_for_call_syntax("ts_member"),
            Some(typescript::MEMBER_CALLSITE_MARKER)
        );

        // TypeScript's receiver-call engine is shared, not copied.
        assert!(tsx.receiver_call_specs.is_some());
        assert!(tsx.tags_query.is_some());
        assert!(tsx.graph_query.contains("ts_member"));
        assert!(!tsx.uses_generic_semantic_resolver);
        assert_eq!(tsx.semantic_family, "webscript");
    }

    /// The Python row must keep the exact projection facts it had while it was
    /// spread across `lib.rs`. Python's values differ from Kotlin's on two of
    /// them, which is precisely why they have to be pinned: `promotes_type_...`
    /// was false because the rule file already projects a class body's
    /// `function_definition` as a METHOD, and `route_comments_are_c_style` was
    /// false because Python comments are `#`.
    #[test]
    fn python_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let python = extraction_for_language("python").expect("python row");
        assert!(!python.promotes_type_member_functions_to_methods);
        assert_eq!(python.qualified_name_delimiter, ".");
        assert!(!python.route_comments_are_c_style);
        assert_eq!(python.semantic_family, "python");
        assert!(!python.uses_generic_semantic_resolver);
        assert_eq!(
            python.callsite_marker_families,
            &[("python_attribute", "syntax:python-attribute-call")]
        );
        assert_eq!(
            member_callsite_marker_for_call_syntax("python_attribute"),
            Some("syntax:python-attribute-call")
        );
        assert!(python.receiver_call_specs.is_some());
        assert!(python.tags_query.is_none());
        assert_eq!(
            extraction_for_ext("py").map(|row| row.language_name),
            Some("python")
        );
        assert_eq!(
            extraction_for_ext("pyi").map(|row| row.language_name),
            Some("python")
        );
    }

    /// The Rust row must keep the exact projection facts it had while it was
    /// spread across `lib.rs`, `language_configs.rs` and `semantic/mod.rs`.
    ///
    /// `qualified_name_delimiter` is the sharpest of these: `::` used to be one
    /// arm of a three-language match and no other test in the tree reads it, so
    /// a `.` here would rename every Rust member without turning a single
    /// threshold suite red.
    #[test]
    fn rust_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let rust = extraction_for_language("rust").expect("rust row");
        assert_eq!(rust.qualified_name_delimiter, "::");
        assert!(!rust.promotes_type_member_functions_to_methods);
        assert!(rust.route_comments_are_c_style);
        assert_eq!(rust.semantic_family, "rust");
        // Rust resolves through `semantic::RustSemanticResolver`, not the shared
        // name-only resolver; the generic flag must stay off.
        assert!(!rust.uses_generic_semantic_resolver);
        assert_eq!(
            rust.callsite_marker_families,
            &[("rust_method", "syntax:rust-method-call")]
        );
        assert_eq!(
            member_callsite_marker_for_call_syntax("rust_method"),
            Some("syntax:rust-method-call")
        );
        // Rust's receiver-call inference is not a `language_receiver_call_specs`
        // collector; see `languages::rust`'s module docs.
        assert!(rust.receiver_call_specs.is_none());
        // Rust is the only graph rule file that pairs with a tags query.
        assert!(rust.tags_query.is_some());
        assert_eq!(
            extraction_for_ext("rs").map(|row| row.language_name),
            Some("rust")
        );
    }

    /// The Go row must keep the exact projection facts it had while it was
    /// spread across `lib.rs`. Go differs from Kotlin on three of them —
    /// it does *not* promote type-member functions to methods, it does *not*
    /// use the generic semantic resolver, and it owns a manual MEMBER-edge
    /// collector — and each wrong value is a silent projection change that no
    /// threshold test would catch.
    #[test]
    fn go_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let go = extraction_for_language("go").expect("go row");
        assert!(!go.promotes_type_member_functions_to_methods);
        assert_eq!(go.qualified_name_delimiter, ".");
        assert!(go.route_comments_are_c_style);
        assert_eq!(go.semantic_family, "go");
        assert!(!go.uses_generic_semantic_resolver);
        assert_eq!(
            go.callsite_marker_families,
            &[("go_selector", "syntax:go-selector-call")]
        );
        assert_eq!(
            member_callsite_marker_for_call_syntax("go_selector"),
            Some("syntax:go-selector-call")
        );
        assert!(go.receiver_call_specs.is_some());
        assert!(go.member_edge_specs.is_some());
        assert!(go.tags_query.is_none());
        assert_eq!(
            extraction_for_ext("go").map(|row| row.language_name),
            Some("go")
        );
    }

    /// The Dart row must keep the exact projection facts it had while it was
    /// spread across `lib.rs`. Same reasoning as the Kotlin case above: these
    /// values used to live in separate match statements, and a wrong value
    /// here is a silent projection change that no threshold test would catch.
    #[test]
    fn dart_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let dart = extraction_for_language("dart").expect("dart row");
        assert!(dart.promotes_type_member_functions_to_methods);
        assert_eq!(dart.qualified_name_delimiter, ".");
        assert!(dart.route_comments_are_c_style);
        assert_eq!(dart.semantic_family, "dart");
        assert!(dart.uses_generic_semantic_resolver);
        assert_eq!(
            dart.callsite_marker_families,
            &[("dart_member", "syntax:dart-member-call")]
        );
        assert_eq!(
            member_callsite_marker_for_call_syntax("dart_member"),
            Some("syntax:dart-member-call")
        );
        assert!(dart.receiver_call_specs.is_some());
        assert!(dart.tags_query.is_none());
        assert_eq!(
            extraction_for_ext("dart").map(|row| row.language_name),
            Some("dart")
        );
    }

    /// The Bash row must keep the exact projection facts it had while it was
    /// spread across `lib.rs`, `language_configs.rs` and `semantic/mod.rs`.
    ///
    /// This row is load-bearing in a way Kotlin's was not. Mutation testing on
    /// the two Bash fixtures shows `language_extraction_snapshot` catches the
    /// routing and rule-file fields (`extensions`, `graph_query`,
    /// `language_name`) but is *blind* to six others, because shell has no
    /// type-like owners, no owner-qualified names, no framework routes and no
    /// receiver-qualified call form for the fixtures to exercise. Flipping
    /// `promotes_type_member_functions_to_methods`, `qualified_name_delimiter`,
    /// `route_comments_are_c_style`, `semantic_family`,
    /// `uses_generic_semantic_resolver`, or wiring a foreign engine into
    /// `receiver_call_specs` each leaves both goldens byte-identical. The
    /// assertions below are the pre-move roster restated, so a registry row
    /// that contradicted the god file fails here instead of shipping.
    #[test]
    fn bash_row_keeps_the_projection_facts_it_had_in_the_god_file() {
        let bash = extraction_for_language("bash").expect("bash row");
        // `lib.rs::promotes_type_member_functions_to_methods` listed only
        // `swift` and `dart`; Bash fell through to `false`.
        assert!(!bash.promotes_type_member_functions_to_methods);
        // `lib.rs::qualified_name_delimiter` gave `::` to rust/cpp/c only.
        assert_eq!(bash.qualified_name_delimiter, ".");
        // Bash was absent from `route_language_uses_c_style_comments`.
        assert!(!bash.route_comments_are_c_style);
        assert_eq!(bash.semantic_family, "bash");
        assert!(bash.uses_generic_semantic_resolver);
        // Shell has no member-call syntax and no manual receiver-call engine.
        assert!(bash.receiver_call_specs.is_none());
        assert!(bash.callsite_marker_families.is_empty());
        assert!(bash.tags_query.is_none());
        assert_eq!(
            extraction_for_ext("sh").map(|row| row.language_name),
            Some("bash")
        );
        assert_eq!(
            extraction_for_ext("bash").map(|row| row.language_name),
            Some("bash")
        );
    }

    /// The PHP row is the reason the registry's marker fields became a
    /// list-valued family table: member calls and `new` construction are two
    /// distinct callsite syntaxes, and the old single-valued pair could only
    /// ever route one of them. Both families must resolve, and the migration
    /// must have preserved the member-call family exactly.
    #[test]
    fn php_row_carries_both_member_and_construction_marker_families() {
        let php = extraction_for_language("php").expect("php row");
        assert_eq!(
            php.callsite_marker_families,
            &[
                ("php_member", "syntax:php-member-call"),
                ("php_new", "syntax:php-new"),
            ]
        );
        assert_eq!(
            member_callsite_marker_for_call_syntax("php_member"),
            Some(php::MEMBER_CALLSITE_MARKER)
        );
        assert_eq!(
            member_callsite_marker_for_call_syntax("php_new"),
            Some(php::NEW_CALLSITE_MARKER)
        );
        assert!(php.graph_query.contains("php_new"));
    }
}
pub(crate) mod python;
pub(crate) mod rust;
pub(crate) mod tsx;
