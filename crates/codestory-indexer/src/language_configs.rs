use super::{LanguageConfig, languages, make_language_config};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext, normalize_extension,
};

pub(super) fn get_language_for_ext(ext: &str) -> Option<LanguageConfig> {
    let ext = normalize_extension(ext);
    let profile = language_support_profile_for_ext(&ext)?;
    if profile.support_mode != LanguageSupportMode::ParserBackedGraph {
        return None;
    }

    // Every parser-backed language now owns its config through a registry row,
    // so this is the only lookup: an extension the registry does not claim has
    // no parser config at all.
    let extraction = languages::extraction_for_ext(&ext)?;
    Some(make_language_config(
        (extraction.parser_language)(),
        extraction.language_name,
        extraction.graph_query,
        extraction.tags_query,
        extraction.ruleset,
    ))
}
