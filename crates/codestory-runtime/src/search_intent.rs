use super::{
    LanguageSupportProfile, NodeKind, Path, SearchHit, SearchMatchQualityDto,
    SearchQueryAssessmentDto, SearchRepoTextMode, architecture_query_intents,
    exact_symbol_query_terms, language_support_profile_for_ext,
    language_support_profile_for_language_name, leading_symbol_segment, normalize_symbol_query,
    query_has_symbol_or_literal_signal, symbol_name_match_rank, symbol_query,
    terminal_symbol_segment,
};

#[derive(Debug, Clone)]
pub(super) struct SearchIntentQuery {
    pub(super) effective_query: String,
    pub(super) filters: Vec<SearchIntentFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SearchIntentFilter {
    Kind(String),
    Path(String),
    Name(String),
    Language(String),
}

pub(super) fn parse_search_intent_query(query: &str) -> SearchIntentQuery {
    let mut free_terms = Vec::new();
    let mut name_terms = Vec::new();
    let mut fallback_terms = Vec::new();
    let mut filters = Vec::new();

    for token in query.split_whitespace() {
        let Some((field, raw_value)) = token.split_once(':') else {
            free_terms.push(token.to_string());
            continue;
        };
        let value = strip_query_value_quotes(raw_value);
        if value.is_empty() {
            free_terms.push(token.to_string());
            continue;
        }
        match field.to_ascii_lowercase().as_str() {
            "kind" => {
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Kind(value));
            }
            "path" | "file" => {
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Path(value));
            }
            "name" | "symbol" => {
                name_terms.push(value.clone());
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Name(value));
            }
            "lang" | "language" => {
                fallback_terms.push(value.clone());
                filters.push(SearchIntentFilter::Language(value));
            }
            _ => free_terms.push(token.to_string()),
        }
    }

    let effective_query = if !free_terms.is_empty() {
        free_terms.join(" ")
    } else if !name_terms.is_empty() {
        name_terms.join(" ")
    } else if !fallback_terms.is_empty() {
        fallback_terms.join(" ")
    } else {
        query.trim().to_string()
    };

    SearchIntentQuery {
        effective_query,
        filters,
    }
}

pub(super) fn strip_query_value_quotes(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if matches!(first, b'"' | b'\'' | b'`') && first == last {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

pub(super) fn apply_search_intent_filters(
    hits: &mut Vec<SearchHit>,
    filters: &[SearchIntentFilter],
) {
    if filters.is_empty() {
        return;
    }
    hits.retain(|hit| {
        filters
            .iter()
            .all(|filter| search_hit_matches_intent_filter(hit, filter))
    });
}

pub(super) fn search_hit_matches_intent_filter(
    hit: &SearchHit,
    filter: &SearchIntentFilter,
) -> bool {
    match filter {
        SearchIntentFilter::Kind(kind) => search_hit_kind_matches(hit.kind, kind),
        SearchIntentFilter::Path(fragment) => hit.file_path.as_deref().is_some_and(|path| {
            path.to_ascii_lowercase()
                .contains(&fragment.to_ascii_lowercase())
        }),
        SearchIntentFilter::Name(name) => search_hit_name_matches(&hit.display_name, name),
        SearchIntentFilter::Language(language) => hit
            .file_path
            .as_deref()
            .is_some_and(|path| language_filter_matches_path(language, path)),
    }
}

pub(super) fn search_hit_kind_matches(kind: NodeKind, requested: &str) -> bool {
    let normalized = normalize_filter_token(requested);
    let actual = normalize_filter_token(&format!("{kind:?}"));
    if normalized == actual {
        return true;
    }
    match normalized.as_str() {
        "fn" | "func" | "function" => kind == NodeKind::FUNCTION,
        "method" => kind == NodeKind::METHOD,
        "callable" => matches!(
            kind,
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
        ),
        "type" => matches!(
            kind,
            NodeKind::STRUCT
                | NodeKind::CLASS
                | NodeKind::INTERFACE
                | NodeKind::UNION
                | NodeKind::ENUM
                | NodeKind::TYPEDEF
        ),
        "var" | "variable" => matches!(kind, NodeKind::VARIABLE | NodeKind::GLOBAL_VARIABLE),
        "global" | "globalvar" | "globalvariable" => kind == NodeKind::GLOBAL_VARIABLE,
        "const" | "constant" => kind == NodeKind::CONSTANT,
        "field" | "member" => kind == NodeKind::FIELD,
        "file" => kind == NodeKind::FILE,
        _ => false,
    }
}

pub(super) fn search_hit_name_matches(display_name: &str, requested: &str) -> bool {
    let requested = requested.trim();
    if requested.is_empty() {
        return true;
    }
    let display_lower = display_name.to_ascii_lowercase();
    let requested_lower = requested.to_ascii_lowercase();
    display_lower.contains(&requested_lower)
        || normalize_symbol_query(display_name).contains(&normalize_symbol_query(requested))
}

pub(super) fn language_filter_matches_path(requested: &str, path: &str) -> bool {
    let requested_lower = requested
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if requested_lower.is_empty() {
        return false;
    }
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension.is_empty() {
        return false;
    }

    if let Some(language_name) = language_family_alias(&requested_lower) {
        return language_profile_matches_extension_name(language_name, &extension);
    }

    if let Some(profile) = language_support_profile_for_language_name(&requested_lower) {
        return language_profile_matches_extension(profile, &extension);
    }

    if language_support_profile_for_ext(&requested_lower).is_some() {
        return requested_lower == extension;
    }

    let requested_normalized = normalize_filter_token(&requested_lower);
    match requested_normalized.as_str() {
        "markdown" => matches!(extension.as_str(), "md" | "mdx"),
        _ => requested_normalized == normalize_filter_token(&extension),
    }
}

pub(super) fn indexed_file_matches_language_filter(
    stored_language: &str,
    path: &Path,
    requested: &str,
) -> bool {
    stored_language.eq_ignore_ascii_case(requested)
        || language_filter_matches_path(requested, &path.to_string_lossy())
}

pub(super) fn language_family_alias(requested: &str) -> Option<&'static str> {
    match requested {
        "ts" => Some("typescript"),
        "js" => Some("javascript"),
        "kt" => Some("kotlin"),
        "c++" | "cplusplus" => Some("cpp"),
        "c#" | "cs" => Some("csharp"),
        _ => None,
    }
}

pub(super) fn language_profile_matches_extension(
    profile: &LanguageSupportProfile,
    extension: &str,
) -> bool {
    profile.extensions.contains(&extension)
}

pub(super) fn language_profile_matches_extension_name(
    language_name: &str,
    extension: &str,
) -> bool {
    language_support_profile_for_language_name(language_name)
        .is_some_and(|profile| language_profile_matches_extension(profile, extension))
}

pub(super) fn normalize_filter_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

pub(super) fn exact_symbol_hit_count(query: &str, hits: &[SearchHit]) -> u32 {
    hits.iter()
        .filter(|hit| {
            let rank = symbol_name_match_rank(query, &hit.display_name);
            rank.exact_display > 0 || rank.exact_terminal > 0 || rank.exact_leading > 0
        })
        .count()
        .min(u32::MAX as usize) as u32
}

pub(super) fn search_hit_match_quality(query: &str, hit: &SearchHit) -> SearchMatchQualityDto {
    if hit.is_text_match() {
        return SearchMatchQualityDto::RepoText;
    }
    let query = query.trim();
    if query.is_empty() {
        return SearchMatchQualityDto::SemanticSuggestion;
    }
    let query_normalized = normalize_symbol_query(query);
    let display_normalized = normalize_symbol_query(&hit.display_name);
    let terminal = terminal_symbol_segment(&hit.display_name);
    let leading = leading_symbol_segment(&hit.display_name);
    let exact_terms = exact_symbol_query_terms(query);
    if hit.display_name == query || exact_terms.contains(&hit.display_name) {
        return SearchMatchQualityDto::Exact;
    }
    if exact_terms.iter().any(|term| {
        let normalized = normalize_symbol_query(term);
        display_normalized == normalized || terminal == normalized || leading == normalized
    }) {
        return SearchMatchQualityDto::NormalizedExact;
    }
    if display_normalized == query_normalized
        || terminal == query_normalized
        || leading == query_normalized
    {
        return SearchMatchQualityDto::NormalizedExact;
    }
    if display_normalized.starts_with(&query_normalized)
        || terminal.starts_with(&query_normalized)
        || leading.starts_with(&query_normalized)
    {
        return SearchMatchQualityDto::Prefix;
    }
    if hit
        .score_breakdown
        .as_ref()
        .is_some_and(|breakdown| breakdown.semantic > 0.0 && breakdown.lexical <= f32::EPSILON)
    {
        return SearchMatchQualityDto::SemanticSuggestion;
    }
    SearchMatchQualityDto::Fuzzy
}

pub(super) fn annotate_search_hit_match_quality(query: &str, hits: &mut [SearchHit]) {
    for hit in hits {
        hit.match_quality = Some(search_hit_match_quality(query, hit));
    }
}

pub(super) fn text_contains_query_term(text: &str, term: &str) -> bool {
    if text.contains(term) {
        return true;
    }
    let Some(singular) = term.strip_suffix('s') else {
        return false;
    };
    singular.len() >= 3 && text.contains(singular)
}

pub(super) fn weak_search_top_hit(query: &str, hits: &[SearchHit]) -> bool {
    hits.first().is_none_or(|hit| {
        hit.score < 0.5
            || matches!(
                search_hit_match_quality(query, hit),
                SearchMatchQualityDto::Fuzzy | SearchMatchQualityDto::SemanticSuggestion
            )
    })
}

#[cfg(test)]
pub(super) fn repo_text_auto_fallback_reason(
    query: &str,
    indexed_hits: &[SearchHit],
) -> Option<String> {
    if query.trim().is_empty() {
        return None;
    }
    if indexed_hits.is_empty() {
        return Some("auto fallback: no indexed symbol hits matched the query".to_string());
    }
    if exact_symbol_hit_count(query, indexed_hits) == 0 {
        if query_has_symbol_or_literal_signal(query) {
            return Some(
                "auto fallback: query looks like a concrete anchor but no exact indexed symbol matched"
                    .to_string(),
            );
        }
        if weak_search_top_hit(query, indexed_hits) {
            return Some(
                "auto fallback: top indexed symbol hit was weak and non-exact".to_string(),
            );
        }
    }
    None
}

pub(super) fn search_query_assessment(
    query: &str,
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    repo_text_mode: SearchRepoTextMode,
    repo_text_enabled: bool,
    repo_text_fallback_reason: Option<String>,
) -> SearchQueryAssessmentDto {
    let exact_symbol_hit_count = exact_symbol_hit_count(query, indexed_hits);
    let weak_top_hit = exact_symbol_hit_count == 0 && weak_search_top_hit(query, indexed_hits);
    let stale_or_missing_anchor =
        exact_symbol_hit_count == 0 && query_has_symbol_or_literal_signal(query);
    let architecture_intents = architecture_query_intents(query);

    SearchQueryAssessmentDto {
        exact_symbol_hit_count,
        weak_top_hit,
        stale_or_missing_anchor,
        repo_text_fallback_reason,
        recommended_next_action: Some(search_query_recommended_next_action(
            exact_symbol_hit_count,
            &architecture_intents,
            indexed_hits,
            repo_text_hits,
            repo_text_mode,
            repo_text_enabled,
        )),
    }
}

pub(super) fn search_query_recommended_next_action(
    exact_symbol_hit_count: u32,
    architecture_intents: &[symbol_query::ArchitectureQueryIntent],
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    repo_text_mode: SearchRepoTextMode,
    repo_text_enabled: bool,
) -> String {
    if exact_symbol_hit_count > 0 && !architecture_intents.is_empty() {
        return format!(
            "Architecture intent detected ({}); open the strongest production entrypoint/orchestrator with symbol, trail, and function-body snippet before answering.",
            architecture_intent_labels(architecture_intents)
        );
    }
    if exact_symbol_hit_count > 0 {
        return "Open the exact indexed hit with symbol, trail, and snippet before answering."
            .to_string();
    }
    if !architecture_intents.is_empty() && !indexed_hits.is_empty() {
        return format!(
            "Architecture intent detected ({}) with no exact anchor; run drill with concrete anchors from ground/search, then inspect symbol, trail, and function-body snippets before answering. Treat broad search hits as leads only.",
            architecture_intent_labels(architecture_intents)
        );
    }
    if !repo_text_hits.is_empty() {
        return "Use repo-text hits to choose a concrete identifier, then rerun symbol/trail/snippet."
            .to_string();
    }
    if repo_text_mode == SearchRepoTextMode::Off || !repo_text_enabled {
        return "Run retrieval index to restore full sidecar mode, then rerun search --why with a shorter concrete symbol.".to_string();
    }
    "Try a shorter symbol, file name, or literal from ground output.".to_string()
}

pub(super) fn architecture_intent_labels(
    intents: &[symbol_query::ArchitectureQueryIntent],
) -> String {
    intents
        .iter()
        .map(|intent| intent.label())
        .collect::<Vec<_>>()
        .join(", ")
}
