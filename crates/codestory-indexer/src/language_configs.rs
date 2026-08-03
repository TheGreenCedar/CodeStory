use super::{
    BASH_GRAPH_QUERY, GO_GRAPH_QUERY, JAVA_GRAPH_QUERY, JAVASCRIPT_GRAPH_QUERY, LanguageConfig,
    LanguageRuleset, PHP_GRAPH_QUERY, PYTHON_GRAPH_QUERY, RUBY_GRAPH_QUERY, RUST_GRAPH_QUERY,
    RUST_TAGS_QUERY, SWIFT_GRAPH_QUERY, TSX_GRAPH_QUERY, TSX_TAGS_QUERY, TYPESCRIPT_GRAPH_QUERY,
    TYPESCRIPT_TAGS_QUERY, languages, make_language_config,
};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext, normalize_extension,
};

pub(super) fn get_language_for_ext(ext: &str) -> Option<LanguageConfig> {
    let ext = normalize_extension(ext);
    let profile = language_support_profile_for_ext(&ext)?;
    if profile.support_mode != LanguageSupportMode::ParserBackedGraph {
        return None;
    }

    // Registry first: a migrated language owns its parser config. The arms
    // below are the languages whose S3 package has not landed yet.
    if let Some(extraction) = languages::extraction_for_ext(&ext) {
        return Some(make_language_config(
            (extraction.parser_language)(),
            extraction.language_name,
            extraction.graph_query,
            extraction.tags_query,
            extraction.ruleset,
        ));
    }

    match (profile.language_name, ext.as_str()) {
        // `ts`/`mts`/`cts` are answered by the registry above; `tsx` keeps its
        // own grammar and rule file until #1682 gives it a registry row.
        ("bash", _) => Some(bash()),
        _ => None,
    }
}

fn bash() -> LanguageConfig {
    make_language_config(
        tree_sitter_bash::LANGUAGE.into(),
        "bash",
        BASH_GRAPH_QUERY,
        None,
        LanguageRuleset::Bash,
    )
}
