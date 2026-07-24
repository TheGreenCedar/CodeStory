use super::{
    AgentHybridWeightsDto, ApiError, AppController, EdgeKind, GraphNodeId, GraphResponse, HashMap,
    HashSet, NodeId, NodeKind, RetrievalFileRole, RetrievalStateDto, SearchHit, SearchHitOrigin,
    SearchHybridLimitsDto, SearchMatchQualityDto, SearchPlanAnchorGroupDto,
    SearchPlanBridgeConfidenceDto, SearchPlanBridgeDto, SearchPlanBridgeEvidenceKindDto,
    SearchPlanBridgeStatusDto, SearchPlanCandidateWindowDto, SearchPlanChannelDto, SearchPlanDto,
    SearchPlanNextActionDto, SearchPlanPromotionStatusDto, SearchPlanRejectedHitDto,
    SearchPlanSubqueryDto, SearchPlanTermsDto, SearchQueryAssessmentDto, SearchRepoTextMode,
    SearchRequest, SearchResultsDto, Storage, TrailConfigDto, agent, architecture_query_intents,
    clamp_usize_to_u32, compare_search_hits_with_project_root, leading_symbol_segment,
    looks_like_repo_text_query, normalize_path_key, normalize_symbol_query,
    retrieval_file_role_from_path, retrieval_state_from_storage_for_runtime,
    should_expand_symbol_query, terminal_symbol_segment,
};
use crate::search_intent::{
    SearchIntentFilter, SearchIntentQuery, annotate_search_hit_match_quality,
    apply_search_intent_filters, parse_search_intent_query, search_hit_match_quality,
    search_query_assessment,
};
use crate::search_scoring::{
    ArchitectureCoverage, apply_architecture_cross_source_coverage, architecture_coverage_for_hit,
    dedupe_inexact_search_hits_by_display_key, did_you_mean_suggestions,
    merge_search_hits_by_node_id, search_plan_subquery_candidate_limit,
};
use crate::search_terms::{
    SEARCH_PLAN_BASE_SOURCE_TRUTH_CHECKS, SEARCH_PLAN_EXPLICIT_ANCHOR_MARKER,
    SEARCH_PLAN_MAX_SEED_ANCHORS, SEARCH_PLAN_OPTIONAL_SUBQUERY_LIMIT,
    SEARCH_PLAN_REPO_TEXT_SOURCE_TRUTH_CHECK, SEARCH_PLAN_ROLE_SPECS,
    SEARCH_PLAN_SEED_ANCHOR_MARKER, SEARCH_PLAN_SYMBOL_TERMS, search_plan_terms,
};

fn is_low_confidence_search_plan_bridge(bridge: &SearchPlanBridgeDto) -> bool {
    bridge.confidence == SearchPlanBridgeConfidenceDto::Low
}

#[derive(Debug, Clone, Default)]
pub(super) struct SearchPlanExecutedEvidence {
    indexed_symbol_hits: Vec<SearchHit>,
    repo_text_hits: Vec<SearchHit>,
    suggestions: Vec<SearchHit>,
    candidate_windows: Vec<SearchPlanCandidateWindowDto>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SearchPlanActivePathEvidence {
    pub(super) caller_count: u32,
}

#[derive(Debug, Clone)]
pub(super) struct SearchPlanBuild {
    plan: SearchPlanDto,
    indexed_symbol_hits: Vec<SearchHit>,
}

pub(super) fn search_plan_eligible(
    query: &str,
    exact_symbol_hit_count: u32,
    intents: &[String],
) -> bool {
    let broad_query = looks_like_repo_text_query(query) || query.split_whitespace().count() >= 4;
    let has_seed_anchors = query.contains(SEARCH_PLAN_SEED_ANCHOR_MARKER);
    let broad_explanation_prompt =
        search_plan_broad_explanation_prompt_with_architecture_terms(query);
    !intents.is_empty()
        && broad_query
        && (exact_symbol_hit_count == 0 || has_seed_anchors || broad_explanation_prompt)
}

pub(super) fn search_plan_broad_explanation_prompt_with_architecture_terms(query: &str) -> bool {
    let lower = query.to_ascii_lowercase();
    let asks_for_flow = lower.contains("explain how")
        || lower.contains("trace how")
        || lower.starts_with("how ")
        || lower.contains(" how ");
    if !asks_for_flow {
        return false;
    }
    let tokens = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<HashSet<_>>();
    [
        "cli",
        "command",
        "runtime",
        "workspace",
        "indexer",
        "indexing",
        "store",
        "storage",
        "persistence",
        "snapshot",
        "search",
        "trail",
        "snippet",
        "configuration",
        "source",
        "activation",
        "host",
        "execution",
    ]
    .iter()
    .filter(|term| tokens.contains(**term))
    .count()
        >= 3
}

pub(super) fn search_plan_subqueries(
    query: &str,
    terms: &SearchPlanTermsDto,
    intents: &[String],
) -> Vec<SearchPlanSubqueryDto> {
    if intents.is_empty() {
        return Vec::new();
    }
    let mut subqueries = Vec::new();
    let mut seen = HashSet::new();

    push_search_plan_subquery(
        &mut subqueries,
        &mut seen,
        query.trim().to_string(),
        "original_question",
        vec![
            SearchPlanChannelDto::Semantic,
            SearchPlanChannelDto::Lexical,
        ],
    );
    push_search_plan_seed_anchor_subqueries(&mut subqueries, &mut seen, query);
    push_search_plan_explicit_anchor_subqueries(&mut subqueries, &mut seen, query);
    push_search_plan_symbol_term_subquery(&mut subqueries, &mut seen, terms);
    push_search_plan_role_subqueries(&mut subqueries, &mut seen, terms);
    push_search_plan_named_anchor_subqueries(&mut subqueries, &mut seen, terms);
    push_search_plan_fallback_subquery(&mut subqueries, &mut seen, terms);
    subqueries
}

pub(super) fn push_search_plan_seed_anchor_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: &str,
) {
    for anchor in search_plan_seed_anchor_terms(query)
        .into_iter()
        .take(SEARCH_PLAN_MAX_SEED_ANCHORS)
    {
        push_required_search_plan_subquery(
            subqueries,
            seen,
            anchor,
            "named_anchor",
            vec![
                SearchPlanChannelDto::TypedSymbol,
                SearchPlanChannelDto::Lexical,
            ],
        );
    }
}

pub(super) fn search_plan_seed_anchor_terms(query: &str) -> Vec<String> {
    query
        .split(SEARCH_PLAN_SEED_ANCHOR_MARKER)
        .skip(1)
        .map(|rest| rest.lines().next().unwrap_or(rest))
        .flat_map(|anchors| anchors.split(','))
        .map(str::trim)
        .filter(|anchor| !anchor.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn push_search_plan_explicit_anchor_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: &str,
) {
    for anchor in search_plan_explicit_anchor_terms(query) {
        push_required_search_plan_subquery(
            subqueries,
            seen,
            anchor,
            "named_anchor",
            vec![
                SearchPlanChannelDto::TypedSymbol,
                SearchPlanChannelDto::Lexical,
            ],
        );
    }
}

pub(super) fn search_plan_explicit_anchor_terms(query: &str) -> Vec<String> {
    let Some(rest) = query.split(SEARCH_PLAN_EXPLICIT_ANCHOR_MARKER).nth(1) else {
        return Vec::new();
    };
    let anchor_text = rest.split('.').next().unwrap_or(rest).trim();
    anchor_text
        .split(',')
        .map(|part| part.trim().trim_start_matches("and ").trim())
        .filter(|anchor| !anchor.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn push_search_plan_symbol_term_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    let symbol_terms = sorted_search_plan_symbol_terms(terms);
    if symbol_terms.is_empty() {
        return;
    }
    push_search_plan_subquery(
        subqueries,
        seen,
        symbol_terms
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        "typed_anchor_terms",
        vec![
            SearchPlanChannelDto::TypedSymbol,
            SearchPlanChannelDto::Lexical,
        ],
    );
}

pub(super) fn push_search_plan_named_anchor_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    let symbol_terms = sorted_search_plan_symbol_terms(terms);
    for term in symbol_terms
        .iter()
        .filter(|term| search_plan_named_anchor_term(term))
        .take(5)
    {
        push_search_plan_subquery(
            subqueries,
            seen,
            term.clone(),
            "named_anchor",
            vec![
                SearchPlanChannelDto::TypedSymbol,
                SearchPlanChannelDto::Lexical,
            ],
        );
    }
}

pub(super) fn sorted_search_plan_symbol_terms(terms: &SearchPlanTermsDto) -> Vec<String> {
    let mut symbol_terms = terms
        .extracted
        .iter()
        .filter(|term| search_plan_symbol_term(term))
        .cloned()
        .collect::<Vec<_>>();
    symbol_terms.sort_by(|left, right| {
        search_plan_symbol_subquery_term_score(right)
            .cmp(&search_plan_symbol_subquery_term_score(left))
            .then_with(|| left.cmp(right))
    });
    symbol_terms
}

pub(super) fn search_plan_symbol_term(term: &str) -> bool {
    term.chars().any(|ch| ch.is_ascii_uppercase())
        || SEARCH_PLAN_SYMBOL_TERMS
            .iter()
            .any(|symbol_term| term.eq_ignore_ascii_case(symbol_term))
}

pub(super) fn search_plan_symbol_subquery_term_score(term: &str) -> u32 {
    let uppercase_count = term.chars().filter(|ch| ch.is_ascii_uppercase()).count() as u32;
    let lowercase_count = term.chars().filter(|ch| ch.is_ascii_lowercase()).count() as u32;
    let mut score = term.len().min(40) as u32;
    if uppercase_count >= 2 && lowercase_count > 0 {
        score += 120;
    } else if uppercase_count > 0 && lowercase_count > 0 {
        score += 40;
    }
    if term.contains('_') || term.contains('-') {
        score += 35;
    }
    if SEARCH_PLAN_SYMBOL_TERMS
        .iter()
        .any(|symbol_term| term.eq_ignore_ascii_case(symbol_term))
    {
        score += 20;
    }
    score
}

pub(super) fn search_plan_named_anchor_term(term: &str) -> bool {
    let uppercase_count = term.chars().filter(|ch| ch.is_ascii_uppercase()).count();
    let lowercase_count = term.chars().filter(|ch| ch.is_ascii_lowercase()).count();
    uppercase_count >= 1 && lowercase_count > 0 && term.len() >= 4
}

pub(super) fn push_search_plan_role_subqueries(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    for (role, needles) in SEARCH_PLAN_ROLE_SPECS {
        let role_terms = search_plan_matching_terms(terms, needles);
        if role_terms.len() >= 2 {
            push_search_plan_subquery(
                subqueries,
                seen,
                role_terms.join(" "),
                role,
                vec![
                    SearchPlanChannelDto::TypedSymbol,
                    SearchPlanChannelDto::Lexical,
                    SearchPlanChannelDto::RepoText,
                ],
            );
        }
    }
}

pub(super) fn search_plan_matching_terms(
    terms: &SearchPlanTermsDto,
    needles: &[&str],
) -> Vec<String> {
    terms
        .extracted
        .iter()
        .filter(|term| {
            needles
                .iter()
                .any(|needle| term.eq_ignore_ascii_case(needle))
        })
        .cloned()
        .collect()
}

pub(super) fn push_search_plan_fallback_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    terms: &SearchPlanTermsDto,
) {
    if subqueries.len() >= 3 {
        return;
    }
    let fallback = terms
        .extracted
        .iter()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    push_search_plan_subquery(
        subqueries,
        seen,
        fallback,
        "repo_text_terms",
        vec![
            SearchPlanChannelDto::RepoText,
            SearchPlanChannelDto::Lexical,
        ],
    );
}

pub(super) fn push_search_plan_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: String,
    role: &str,
    channels: Vec<SearchPlanChannelDto>,
) {
    let key = query.to_ascii_lowercase();
    if !query.trim().is_empty()
        && seen.insert(key)
        && subqueries.len() < SEARCH_PLAN_OPTIONAL_SUBQUERY_LIMIT
    {
        subqueries.push(SearchPlanSubqueryDto {
            query,
            role: role.to_string(),
            channels,
        });
    }
}

pub(super) fn push_required_search_plan_subquery(
    subqueries: &mut Vec<SearchPlanSubqueryDto>,
    seen: &mut HashSet<String>,
    query: String,
    role: &str,
    channels: Vec<SearchPlanChannelDto>,
) {
    let key = query.to_ascii_lowercase();
    if !query.trim().is_empty() && seen.insert(key) {
        subqueries.push(SearchPlanSubqueryDto {
            query,
            role: role.to_string(),
            channels,
        });
    }
}

pub(super) fn search_plan_subqueries_for_repo_text_mode(
    subqueries: Vec<SearchPlanSubqueryDto>,
    allow_repo_text: bool,
) -> Vec<SearchPlanSubqueryDto> {
    if allow_repo_text {
        return subqueries;
    }

    subqueries
        .into_iter()
        .filter_map(|mut subquery| {
            subquery
                .channels
                .retain(|channel| *channel != SearchPlanChannelDto::RepoText);
            (!subquery.channels.is_empty()).then_some(subquery)
        })
        .collect()
}

pub(super) fn search_plan_candidate_window(
    channel: SearchPlanChannelDto,
    subquery: &SearchPlanSubqueryDto,
    limit_per_source: u32,
    returned_count: usize,
    truncated: bool,
    reason: &str,
) -> SearchPlanCandidateWindowDto {
    SearchPlanCandidateWindowDto {
        channel,
        subquery: subquery.query.clone(),
        limit: limit_per_source,
        returned_count: clamp_usize_to_u32(returned_count),
        truncated,
        score_reasons: vec![reason.to_string()],
    }
}

pub(super) fn search_plan_symbol_windows(
    subquery: &SearchPlanSubqueryDto,
    hits: &[SearchHit],
    suggestions: &[SearchHit],
    limit_per_source: u32,
    semantic_ready: bool,
) -> Vec<SearchPlanCandidateWindowDto> {
    let mut windows = Vec::new();
    if search_plan_uses_channel(subquery, SearchPlanChannelDto::TypedSymbol) {
        windows.push(search_plan_candidate_window(
            SearchPlanChannelDto::TypedSymbol,
            subquery,
            limit_per_source,
            hits.iter().filter(|hit| hit.resolvable).count(),
            hits.len() >= limit_per_source as usize,
            "executed typed-symbol retrieval for this planned subquery",
        ));
    }
    if search_plan_uses_channel(subquery, SearchPlanChannelDto::Lexical) {
        windows.push(search_plan_candidate_window(
            SearchPlanChannelDto::Lexical,
            subquery,
            limit_per_source,
            search_plan_lexical_hit_count(hits),
            hits.len() >= limit_per_source as usize,
            "executed lexical/name/path retrieval for this planned subquery",
        ));
    }
    if search_plan_uses_channel(subquery, SearchPlanChannelDto::Semantic) {
        windows.push(search_plan_candidate_window(
            SearchPlanChannelDto::Semantic,
            subquery,
            limit_per_source,
            search_plan_semantic_hit_count(hits, suggestions),
            suggestions.len() >= limit_per_source as usize,
            if semantic_ready {
                "executed semantic retrieval for this planned subquery"
            } else {
                "semantic retrieval unavailable; this subquery relied on lexical indexed evidence"
            },
        ));
    }
    windows
}

pub(super) fn search_plan_uses_channel(
    subquery: &SearchPlanSubqueryDto,
    channel: SearchPlanChannelDto,
) -> bool {
    subquery.channels.contains(&channel)
}

pub(super) fn search_plan_lexical_hit_count(hits: &[SearchHit]) -> usize {
    hits.iter()
        .filter(|hit| {
            hit.score_breakdown
                .as_ref()
                .is_none_or(|breakdown| breakdown.lexical > 0.0)
        })
        .count()
}

pub(super) fn search_plan_semantic_hit_count(
    hits: &[SearchHit],
    suggestions: &[SearchHit],
) -> usize {
    hits.iter()
        .chain(suggestions.iter())
        .filter(|hit| {
            hit.score_breakdown
                .as_ref()
                .is_some_and(|breakdown| breakdown.semantic > 0.0)
        })
        .count()
}

pub(super) fn same_search_file(left: &SearchHit, right: &SearchHit) -> bool {
    let Some(left_path) = left.file_path.as_deref() else {
        return false;
    };
    let Some(right_path) = right.file_path.as_deref() else {
        return false;
    };
    normalize_path_key(left_path) == normalize_path_key(right_path)
}

pub(super) fn hit_matches_identifier(hit: &SearchHit, identifier: &str) -> bool {
    hit_exactly_matches_identifier(hit, identifier) || {
        let normalized_identifier = normalize_symbol_query(identifier);
        !normalized_identifier.is_empty()
            && leading_symbol_segment(&hit.display_name) == normalized_identifier
    }
}

pub(super) fn hit_exactly_matches_identifier(hit: &SearchHit, identifier: &str) -> bool {
    let normalized_identifier = normalize_symbol_query(identifier);
    if normalized_identifier.is_empty() {
        return false;
    }
    normalize_symbol_query(&hit.display_name) == normalized_identifier
        || terminal_symbol_segment(&hit.display_name) == normalized_identifier
}

pub(super) fn repo_text_line_identifiers(hit: &SearchHit) -> Vec<String> {
    let Some(path) = hit.file_path.as_deref() else {
        return Vec::new();
    };
    let Some(line) = hit.line else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let start = line.saturating_sub(3) as usize;
    let window = contents
        .lines()
        .skip(start)
        .take(5)
        .collect::<Vec<_>>()
        .join("\n");
    let mut identifiers = Vec::new();
    let mut seen = HashSet::new();
    for token in window.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        if token.len() < 3 {
            continue;
        }
        let looks_symbolic = token.chars().any(|ch| ch.is_ascii_uppercase())
            || token.contains('_')
            || matches!(
                token,
                "auth" | "feed" | "posts" | "storage" | "indexer" | "service" | "trail" | "snippet"
            );
        if looks_symbolic && seen.insert(token.to_ascii_lowercase()) {
            identifiers.push(token.to_string());
        }
    }
    identifiers
}

pub(super) fn repo_text_identifiers_at(identifiers: &[Vec<String>], index: usize) -> &[String] {
    identifiers
        .get(index)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(super) fn repo_text_mentions_hit(identifiers: &[String], hit: &SearchHit) -> bool {
    identifiers
        .iter()
        .any(|identifier| hit_matches_identifier(hit, identifier))
}

pub(super) fn search_plan_group_confidence(query: &str, hit: &SearchHit) -> String {
    let exact = search_hit_match_quality(query, hit);
    if matches!(
        exact,
        SearchMatchQualityDto::Exact | SearchMatchQualityDto::NormalizedExact
    ) {
        "high".to_string()
    } else {
        "medium".to_string()
    }
}

pub(super) fn search_plan_typed_anchor_group(
    query: &str,
    hit: &SearchHit,
    repo_text_hits: &[SearchHit],
    repo_text_identifiers: &[Vec<String>],
    used_repo_text: &mut HashSet<usize>,
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> SearchPlanAnchorGroupDto {
    let mut supporting_hits = vec![hit.clone()];
    let mut reasons = vec!["typed indexed symbol selected as an anchor candidate".to_string()];
    let active_path = search_plan_active_path_for_hit(hit, active_path_evidence);
    reasons.extend(search_plan_active_path_reasons(hit, active_path));

    for (repo_index, repo_hit) in repo_text_hits.iter().enumerate() {
        if !same_search_file(hit, repo_hit) {
            continue;
        }
        let identifiers = repo_text_identifiers_at(repo_text_identifiers, repo_index);
        if identifiers.is_empty() {
            supporting_hits.push(repo_hit.clone());
            used_repo_text.insert(repo_index);
            reasons.push("repo-text hit appears in the same file as the anchor".to_string());
        } else if repo_text_mentions_hit(identifiers, hit) {
            supporting_hits.push(repo_hit.clone());
            used_repo_text.insert(repo_index);
            reasons.push("repo-text hit names this anchor in the same file".to_string());
        }
    }

    SearchPlanAnchorGroupDto {
        anchor: hit.display_name.clone(),
        chosen_symbol: Some(hit.clone()),
        supporting_hits,
        promotion_status: SearchPlanPromotionStatusDto::TypedAnchor,
        promotion_method: Some("indexed_symbol".to_string()),
        caller_count: active_path.caller_count,
        definition_only: search_plan_definition_only(hit, active_path),
        no_visible_callers: search_plan_no_visible_callers(hit, active_path),
        confidence: search_plan_group_confidence(query, hit),
        reasons,
    }
}

pub(super) fn search_plan_promoted_anchor_group(
    symbol: SearchHit,
    repo_hit: &SearchHit,
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> SearchPlanAnchorGroupDto {
    let active_path = search_plan_active_path_for_hit(&symbol, active_path_evidence);
    let mut reasons =
        vec!["repo-text lead was promoted to an indexed symbol in the same file".to_string()];
    reasons.extend(search_plan_active_path_reasons(&symbol, active_path));
    let definition_only = search_plan_definition_only(&symbol, active_path);
    let no_visible_callers = search_plan_no_visible_callers(&symbol, active_path);

    SearchPlanAnchorGroupDto {
        anchor: symbol.display_name.clone(),
        chosen_symbol: Some(symbol.clone()),
        supporting_hits: vec![symbol, repo_hit.clone()],
        promotion_status: SearchPlanPromotionStatusDto::Promoted,
        promotion_method: Some("same_file_exact_identifier".to_string()),
        caller_count: active_path.caller_count,
        definition_only,
        no_visible_callers,
        confidence: "medium".to_string(),
        reasons,
    }
}

pub(super) fn search_plan_unbound_repo_text_group(
    repo_hit: &SearchHit,
    identifiers: &[String],
) -> SearchPlanAnchorGroupDto {
    SearchPlanAnchorGroupDto {
        anchor: repo_hit.display_name.clone(),
        chosen_symbol: None,
        supporting_hits: vec![repo_hit.clone()],
        promotion_status: if identifiers.is_empty() {
            SearchPlanPromotionStatusDto::NeedsSourceRead
        } else {
            SearchPlanPromotionStatusDto::Ambiguous
        },
        promotion_method: None,
        caller_count: 0,
        definition_only: false,
        no_visible_callers: false,
        confidence: "low".to_string(),
        reasons: vec![
            "repo-text lead could not be bound to one indexed symbol before source reads"
                .to_string(),
        ],
    }
}

pub(super) fn search_plan_active_path_for_hit(
    hit: &SearchHit,
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> SearchPlanActivePathEvidence {
    active_path_evidence
        .get(&hit.node_id)
        .copied()
        .unwrap_or_default()
}

pub(super) fn search_plan_active_path_reasons(
    hit: &SearchHit,
    active_path: SearchPlanActivePathEvidence,
) -> Vec<String> {
    if !search_plan_callable_hit(hit) {
        return Vec::new();
    }
    if active_path.caller_count > 0 {
        vec![format!(
            "call graph shows {} visible production caller(s); rank as active-path evidence",
            active_path.caller_count
        )]
    } else {
        vec![
            "call graph shows no visible production callers; treat as definition-only evidence unless a framework/runtime entry path is source-verified"
                .to_string(),
        ]
    }
}

pub(super) fn search_plan_definition_only(
    hit: &SearchHit,
    active_path: SearchPlanActivePathEvidence,
) -> bool {
    search_plan_callable_hit(hit) && active_path.caller_count == 0
}

pub(super) fn search_plan_no_visible_callers(
    hit: &SearchHit,
    active_path: SearchPlanActivePathEvidence,
) -> bool {
    search_plan_callable_hit(hit) && active_path.caller_count == 0
}

pub(super) fn search_plan_callable_hit(hit: &SearchHit) -> bool {
    matches!(
        hit.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    )
}

pub(super) fn search_plan_runtime_call_is_speculative(
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
    confidence: Option<f32>,
) -> bool {
    matches!(
        certainty.or_else(|| {
            codestory_contracts::graph::ResolutionCertainty::from_confidence(confidence)
        }),
        Some(
            codestory_contracts::graph::ResolutionCertainty::Uncertain
                | codestory_contracts::graph::ResolutionCertainty::Probable
        )
    ) || confidence.is_some_and(|confidence| {
        confidence < codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN
    })
}

pub(super) fn search_plan_caller_is_test_or_bench(
    storage: &Storage,
    caller_id: GraphNodeId,
) -> bool {
    let Ok(Some(caller)) = storage.get_node(caller_id) else {
        return false;
    };
    let Ok(Some(path)) = AppController::file_path_for_node(storage, &caller) else {
        return false;
    };
    search_plan_path_is_test_or_bench(&path)
}

pub(super) fn search_plan_path_is_test_or_bench(path: &str) -> bool {
    matches!(
        retrieval_file_role_from_path(path),
        RetrievalFileRole::Test | RetrievalFileRole::Benchmark
    )
}

pub(super) fn search_plan_anchor_groups(
    query: &str,
    terms: &SearchPlanTermsDto,
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    suggestions: &[SearchHit],
    active_path_evidence: &HashMap<NodeId, SearchPlanActivePathEvidence>,
) -> Vec<SearchPlanAnchorGroupDto> {
    let mut groups = Vec::new();
    let mut grouped_ids = HashSet::new();
    let mut used_repo_text = HashSet::new();
    let mut all_symbol_hits = indexed_hits
        .iter()
        .chain(suggestions.iter())
        .cloned()
        .collect::<Vec<_>>();
    all_symbol_hits
        .sort_by(|left, right| compare_search_hits_with_project_root(None, query, left, right));
    let repo_text_identifiers = repo_text_hits
        .iter()
        .map(repo_text_line_identifiers)
        .collect::<Vec<_>>();

    let mut grouped_anchor_names = HashSet::new();
    for hit in all_symbol_hits.iter().filter(|hit| hit.resolvable).take(16) {
        if !grouped_ids.insert(hit.node_id.clone()) {
            continue;
        }
        let anchor_name = hit.display_name.to_ascii_lowercase();
        if !grouped_anchor_names.insert(anchor_name) {
            continue;
        }
        groups.push(search_plan_typed_anchor_group(
            query,
            hit,
            repo_text_hits,
            &repo_text_identifiers,
            &mut used_repo_text,
            active_path_evidence,
        ));
    }

    for (repo_index, repo_hit) in repo_text_hits.iter().enumerate() {
        if used_repo_text.contains(&repo_index) {
            continue;
        }
        let identifiers = repo_text_identifiers_at(&repo_text_identifiers, repo_index);
        let promoted = identifiers.iter().find_map(|identifier| {
            all_symbol_hits
                .iter()
                .find(|hit| {
                    same_search_file(hit, repo_hit)
                        && hit_exactly_matches_identifier(hit, identifier)
                })
                .cloned()
        });
        if let Some(symbol) = promoted {
            if !grouped_ids.insert(symbol.node_id.clone()) {
                continue;
            }
            groups.push(search_plan_promoted_anchor_group(
                symbol,
                repo_hit,
                active_path_evidence,
            ));
        } else {
            groups.push(search_plan_unbound_repo_text_group(repo_hit, identifiers));
        }
    }

    let term_set = terms
        .extracted
        .iter()
        .map(|term| term.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    groups.sort_by(|left, right| {
        search_plan_group_score(right, &term_set)
            .cmp(&search_plan_group_score(left, &term_set))
            .then_with(|| left.anchor.cmp(&right.anchor))
    });
    groups.truncate(8);
    groups
}

pub(super) fn search_plan_group_score(
    group: &SearchPlanAnchorGroupDto,
    terms: &HashSet<String>,
) -> u32 {
    let mut score = 0;
    if group.chosen_symbol.is_some() {
        score += 100;
    }
    if group.caller_count > 0 {
        score += 90 + group.caller_count.min(5) * 6;
    }
    score += match group.promotion_status {
        SearchPlanPromotionStatusDto::TypedAnchor => 60,
        SearchPlanPromotionStatusDto::Promoted => 45,
        SearchPlanPromotionStatusDto::Ambiguous => 10,
        SearchPlanPromotionStatusDto::NeedsSourceRead => 0,
    };
    if group
        .supporting_hits
        .iter()
        .any(|hit| hit.origin == SearchHitOrigin::TextMatch)
    {
        score += 12;
    }
    if group
        .promotion_method
        .as_deref()
        .is_some_and(|method| method == "same_file_exact_identifier")
        || group
            .reasons
            .iter()
            .any(|reason| reason.contains("names this anchor"))
    {
        score += 35;
    }
    let text = group
        .chosen_symbol
        .as_ref()
        .map(|hit| {
            format!(
                "{} {}",
                hit.display_name,
                hit.file_path.as_deref().unwrap_or_default()
            )
        })
        .unwrap_or_else(|| group.anchor.clone())
        .to_ascii_lowercase();
    score
        + terms
            .iter()
            .filter(|term| text.contains(term.as_str()))
            .count() as u32
            * 8
}

pub(super) fn search_plan_rejected_hits(
    anchor_groups: &[SearchPlanAnchorGroupDto],
    suggestions: &[SearchHit],
    indexed_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
) -> Vec<SearchPlanRejectedHitDto> {
    let chosen = anchor_groups
        .iter()
        .filter_map(|group| group.chosen_symbol.as_ref().map(|hit| hit.node_id.clone()))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut rejected = suggestions
        .iter()
        .chain(indexed_hits.iter())
        .chain(repo_text_hits.iter())
        .filter(|hit| !chosen.contains(&hit.node_id) && seen.insert(hit.node_id.clone()))
        .map(|hit| {
            let coverage = architecture_coverage_for_hit(hit);
            let coverage_score = coverage
                .as_ref()
                .map(|coverage| coverage.score)
                .unwrap_or(0);
            (hit, coverage, coverage_score)
        })
        .collect::<Vec<_>>();

    rejected.sort_by(
        |(left, left_coverage, left_score), (right, right_coverage, right_score)| {
            right_score
                .cmp(left_score)
                .then_with(|| right_coverage.is_some().cmp(&left_coverage.is_some()))
                .then_with(|| {
                    (right.origin == SearchHitOrigin::TextMatch)
                        .cmp(&(left.origin == SearchHitOrigin::TextMatch))
                })
        },
    );

    rejected
        .into_iter()
        .take(8)
        .map(|(hit, coverage, _)| SearchPlanRejectedHitDto {
            display_name: hit.display_name.clone(),
            reason: search_plan_rejected_hit_reason(hit, coverage.as_ref()),
            origin: hit.origin,
            file_path: hit.file_path.clone(),
            line: hit.line,
        })
        .collect()
}

pub(super) fn search_plan_rejected_hit_reason(
    hit: &SearchHit,
    coverage: Option<&ArchitectureCoverage>,
) -> String {
    let source = match hit.origin {
        SearchHitOrigin::IndexedSymbol => "indexed_symbol",
        SearchHitOrigin::TextMatch => "repo_text",
    };
    if let Some(coverage) = coverage {
        format!(
            "not selected after anchor grouping and final coverage ranking; source={source}; coverage_key={}; coverage_score={}",
            coverage.key, coverage.score
        )
    } else {
        format!("not selected after anchor grouping and evidence ranking; source={source}")
    }
}

pub(super) fn search_plan_bridge_request(from: &NodeId, to: &NodeId) -> TrailConfigDto {
    TrailConfigDto {
        root_id: from.clone(),
        mode: codestory_contracts::api::TrailMode::ToTargetSymbol,
        target_id: Some(to.clone()),
        depth: 0,
        direction: codestory_contracts::api::TrailDirection::Outgoing,
        caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
        edge_filter: Vec::new(),
        show_utility_calls: false,
        hide_speculative: true,
        story: false,
        node_filter: Vec::new(),
        max_nodes: 80,
        layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
    }
}

pub(super) fn graph_response_has_bridge(
    graph: &codestory_contracts::api::GraphResponse,
    from: &NodeId,
    to: &NodeId,
) -> bool {
    if from == to {
        return true;
    }
    graph.nodes.iter().any(|node| node.id == *from)
        && graph.nodes.iter().any(|node| node.id == *to)
        && !graph.edges.is_empty()
}

pub(super) fn graph_bridge_evidence_kind(graph: &GraphResponse) -> SearchPlanBridgeEvidenceKindDto {
    if graph.edges.iter().any(|edge| {
        edge.callsite_identity
            .as_deref()
            .is_some_and(|identity| identity.starts_with("payload:"))
    }) || graph
        .nodes
        .iter()
        .any(|node| node.label.contains("payload collection "))
    {
        return SearchPlanBridgeEvidenceKindDto::DataCollectionUsage;
    }
    if graph.nodes.iter().any(|node| {
        node.label.contains(" route; confidence=")
            || node
                .qualified_name
                .as_deref()
                .is_some_and(|name| name.starts_with("framework::"))
    }) {
        return SearchPlanBridgeEvidenceKindDto::FrameworkRoute;
    }
    if graph.edges.iter().any(|edge| edge.kind == EdgeKind::CALL)
        && graph.nodes.iter().any(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && node.file_path.as_deref().is_some_and(|path| {
                    let path = path.to_ascii_lowercase();
                    path.ends_with(".tsx") || path.ends_with(".jsx")
                })
                && node
                    .label
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
        })
    {
        return SearchPlanBridgeEvidenceKindDto::ComponentUsage;
    }
    SearchPlanBridgeEvidenceKindDto::GraphPath
}

pub(super) fn shared_file_bridge(from: &SearchHit, to: &SearchHit) -> bool {
    same_search_file(from, to)
}

pub(super) fn search_plan_next_actions(
    groups: &[SearchPlanAnchorGroupDto],
) -> Vec<SearchPlanNextActionDto> {
    groups
        .iter()
        .filter_map(|group| group.chosen_symbol.as_ref())
        .take(4)
        .flat_map(|hit| {
            [
                SearchPlanNextActionDto {
                    action: "symbol".to_string(),
                    node_id: hit.node_id.clone(),
                    options: Vec::new(),
                },
                SearchPlanNextActionDto {
                    action: "trail".to_string(),
                    node_id: hit.node_id.clone(),
                    options: vec!["story".to_string(), "hide_speculative".to_string()],
                },
                SearchPlanNextActionDto {
                    action: "snippet".to_string(),
                    node_id: hit.node_id.clone(),
                    options: vec!["function_body".to_string(), "context=40".to_string()],
                },
            ]
        })
        .collect()
}

pub(super) fn search_plan_source_truth_checks(groups: &[SearchPlanAnchorGroupDto]) -> Vec<String> {
    let mut checks = SEARCH_PLAN_BASE_SOURCE_TRUTH_CHECKS
        .iter()
        .map(|check| (*check).to_string())
        .collect::<Vec<_>>();
    if search_plan_has_unbound_repo_text_group(groups) {
        checks.push(SEARCH_PLAN_REPO_TEXT_SOURCE_TRUTH_CHECK.to_string());
    }
    checks
}

pub(super) fn search_plan_has_unbound_repo_text_group(groups: &[SearchPlanAnchorGroupDto]) -> bool {
    groups.iter().any(|group| {
        matches!(
            group.promotion_status,
            SearchPlanPromotionStatusDto::NeedsSourceRead | SearchPlanPromotionStatusDto::Ambiguous
        )
    })
}

impl AppController {
    #[allow(clippy::too_many_arguments)]
    fn execute_search_plan_subqueries(
        &self,
        storage: &Storage,
        subqueries: &[SearchPlanSubqueryDto],
        filters: &[SearchIntentFilter],
        indexed_hit_ids: &HashSet<NodeId>,
        limit_per_source: usize,
        hybrid_weights: Option<AgentHybridWeightsDto>,
        hybrid_limits: Option<SearchHybridLimitsDto>,
        semantic_ready: bool,
    ) -> Result<SearchPlanExecutedEvidence, ApiError> {
        let mut evidence = SearchPlanExecutedEvidence::default();
        let project_root = self.require_project_root().ok();
        let mut seen_repo_text_ids = indexed_hit_ids.clone();

        for subquery in subqueries {
            let uses_indexed = subquery.channels.iter().any(|channel| {
                matches!(
                    channel,
                    SearchPlanChannelDto::TypedSymbol
                        | SearchPlanChannelDto::Lexical
                        | SearchPlanChannelDto::Semantic
                )
            });
            if uses_indexed {
                let subquery_limit =
                    search_plan_subquery_candidate_limit(subquery, limit_per_source);
                let mut scored_hits = self.search_hybrid_scored(
                    SearchRequest {
                        query: subquery.query.clone(),
                        repo_text: SearchRepoTextMode::Off,
                        limit_per_source: subquery_limit as u32,
                        expand_search_plan: false,
                        hybrid_weights: hybrid_weights.clone(),
                        hybrid_limits: hybrid_limits.clone(),
                    },
                    None,
                    subquery_limit,
                    hybrid_weights.clone(),
                )?;
                let mut suggestions = did_you_mean_suggestions(&scored_hits);
                let mut hits = scored_hits
                    .drain(..)
                    .map(|scored| scored.hit)
                    .collect::<Vec<_>>();
                if should_expand_symbol_query(&subquery.query, hits.len()) {
                    let expanded_hits = self.expanded_symbol_hits(storage, &subquery.query)?;
                    merge_search_hits_by_node_id(&mut hits, expanded_hits);
                }
                apply_search_intent_filters(&mut hits, filters);
                apply_search_intent_filters(&mut suggestions, filters);
                hits.sort_by(|left, right| {
                    compare_search_hits_with_project_root(
                        project_root.as_deref(),
                        &subquery.query,
                        left,
                        right,
                    )
                });
                suggestions.sort_by(|left, right| {
                    compare_search_hits_with_project_root(
                        project_root.as_deref(),
                        &subquery.query,
                        left,
                        right,
                    )
                });
                dedupe_inexact_search_hits_by_display_key(&subquery.query, &mut hits);
                hits.truncate(subquery_limit);
                suggestions.truncate(subquery_limit);
                annotate_search_hit_match_quality(&subquery.query, &mut hits);
                annotate_search_hit_match_quality(&subquery.query, &mut suggestions);
                evidence
                    .candidate_windows
                    .extend(search_plan_symbol_windows(
                        subquery,
                        &hits,
                        &suggestions,
                        subquery_limit as u32,
                        semantic_ready,
                    ));
                merge_search_hits_by_node_id(&mut evidence.indexed_symbol_hits, hits);
                merge_search_hits_by_node_id(&mut evidence.suggestions, suggestions);
            }

            if subquery.channels.contains(&SearchPlanChannelDto::RepoText) {
                let repo_text_limit =
                    search_plan_subquery_candidate_limit(subquery, limit_per_source);
                let scan = Self::collect_repo_text_hits(
                    storage,
                    project_root.as_deref(),
                    &subquery.query,
                    repo_text_limit,
                    &seen_repo_text_ids,
                )?;
                let mut hits = scan.hits;
                apply_search_intent_filters(&mut hits, filters);
                annotate_search_hit_match_quality(&subquery.query, &mut hits);
                for hit in &hits {
                    seen_repo_text_ids.insert(hit.node_id.clone());
                }
                evidence
                    .candidate_windows
                    .push(SearchPlanCandidateWindowDto {
                        channel: SearchPlanChannelDto::RepoText,
                        subquery: subquery.query.clone(),
                        limit: repo_text_limit as u32,
                        returned_count: clamp_usize_to_u32(hits.len()),
                        truncated: hits.len() >= repo_text_limit,
                        score_reasons: vec![
                            "executed repo-text retrieval for this planned subquery; hits require promotion or source reads"
                                .to_string(),
                        ],
                    });
                merge_search_hits_by_node_id(&mut evidence.repo_text_hits, hits);
            }
        }

        Ok(evidence)
    }

    fn search_plan_active_path_evidence<'a, I>(
        &self,
        storage: &Storage,
        hits: I,
    ) -> HashMap<NodeId, SearchPlanActivePathEvidence>
    where
        I: IntoIterator<Item = &'a SearchHit>,
    {
        let mut evidence = HashMap::new();
        for hit in hits {
            if evidence.contains_key(&hit.node_id) {
                continue;
            }
            if let Some(active_path) = self.search_plan_active_path_evidence_for_hit(storage, hit) {
                evidence.insert(hit.node_id.clone(), active_path);
            }
        }
        evidence
    }

    fn search_plan_active_path_evidence_for_hit(
        &self,
        storage: &Storage,
        hit: &SearchHit,
    ) -> Option<SearchPlanActivePathEvidence> {
        if !search_plan_callable_hit(hit) {
            return None;
        }
        let node_id = hit.node_id.to_core().ok()?;
        let edges = storage.get_edges_for_node_id(node_id).ok()?;
        let mut callers = HashSet::new();
        for edge in edges {
            if edge.kind != codestory_contracts::graph::EdgeKind::CALL {
                continue;
            }
            if search_plan_runtime_call_is_speculative(edge.certainty, edge.confidence) {
                continue;
            }
            let (source, target) = edge.effective_endpoints();
            if target != node_id || source == node_id {
                continue;
            }
            if search_plan_caller_is_test_or_bench(storage, source) {
                continue;
            }
            callers.insert(source);
        }

        Some(SearchPlanActivePathEvidence {
            caller_count: callers.len().min(u32::MAX as usize) as u32,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_search_plan(
        &self,
        storage: &Storage,
        original_query: &str,
        effective_query: &str,
        query_assessment: &SearchQueryAssessmentDto,
        indexed_symbol_hits: &[SearchHit],
        repo_text_hits: &[SearchHit],
        suggestions: &[SearchHit],
        retrieval: &RetrievalStateDto,
        limit_per_source: u32,
        filters: &[SearchIntentFilter],
        indexed_hit_ids: &HashSet<NodeId>,
        allow_repo_text: bool,
        hybrid_weights: Option<AgentHybridWeightsDto>,
        hybrid_limits: Option<SearchHybridLimitsDto>,
    ) -> Result<Option<SearchPlanBuild>, ApiError> {
        let intents = architecture_query_intents(effective_query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        let eligible = search_plan_eligible(
            effective_query,
            query_assessment.exact_symbol_hit_count,
            &intents,
        );
        if !eligible {
            return Ok(None);
        }
        let terms = search_plan_terms(effective_query);
        let subqueries = search_plan_subqueries_for_repo_text_mode(
            search_plan_subqueries(effective_query, &terms, &intents),
            allow_repo_text,
        );
        let mut executed = self.execute_search_plan_subqueries(
            storage,
            &subqueries,
            filters,
            indexed_hit_ids,
            limit_per_source as usize,
            hybrid_weights,
            hybrid_limits,
            retrieval.semantic_ready,
        )?;
        let mut plan_indexed_hits = indexed_symbol_hits.to_vec();
        merge_search_hits_by_node_id(&mut plan_indexed_hits, executed.indexed_symbol_hits.clone());
        let mut plan_repo_text_hits = repo_text_hits.to_vec();
        merge_search_hits_by_node_id(&mut plan_repo_text_hits, executed.repo_text_hits.clone());
        let mut plan_suggestions = suggestions.to_vec();
        merge_search_hits_by_node_id(&mut plan_suggestions, executed.suggestions.clone());
        let active_path_evidence = self.search_plan_active_path_evidence(
            storage,
            plan_indexed_hits.iter().chain(plan_suggestions.iter()),
        );
        let anchor_groups = search_plan_anchor_groups(
            effective_query,
            &terms,
            &plan_indexed_hits,
            &plan_repo_text_hits,
            &plan_suggestions,
            &active_path_evidence,
        );
        let mut bridges = self.search_plan_bridges(&anchor_groups);
        let original_bridge_count = bridges.len();
        bridges.retain(|bridge| !is_low_confidence_search_plan_bridge(bridge));
        let suppressed_low_confidence_bridges = original_bridge_count.saturating_sub(bridges.len());
        if !bridges.is_empty() {
            executed
                .candidate_windows
                .push(SearchPlanCandidateWindowDto {
                    channel: SearchPlanChannelDto::Bridge,
                    subquery: "selected_anchor_bridges".to_string(),
                    limit: anchor_groups.len().saturating_sub(1).min(u32::MAX as usize) as u32,
                    returned_count: clamp_usize_to_u32(bridges.len()),
                    truncated: false,
                    score_reasons: vec![
                        "bridge evidence expands only after anchor grouping".to_string(),
                    ],
                });
        }
        let mut source_truth_checks = search_plan_source_truth_checks(&anchor_groups);
        if suppressed_low_confidence_bridges > 0 {
            source_truth_checks.push(format!(
                "Suppressed {suppressed_low_confidence_bridges} low-confidence bridge candidate(s); treat missing bridge rows as a source-truth prompt, not proof of isolation."
            ));
        }
        let next_actions = search_plan_next_actions(&anchor_groups);
        let plan = SearchPlanDto {
            original_query: original_query.to_string(),
            eligible,
            intents,
            terms,
            subqueries,
            candidate_windows: executed.candidate_windows,
            anchor_groups: anchor_groups.clone(),
            bridges,
            rejected_hits: search_plan_rejected_hits(
                &anchor_groups,
                &plan_suggestions,
                &plan_indexed_hits,
                &plan_repo_text_hits,
            ),
            next_actions,
            source_truth_checks,
        };
        Ok(Some(SearchPlanBuild {
            plan,
            indexed_symbol_hits: executed.indexed_symbol_hits,
        }))
    }

    fn search_plan_bridges(&self, groups: &[SearchPlanAnchorGroupDto]) -> Vec<SearchPlanBridgeDto> {
        let selected = groups
            .iter()
            .filter_map(|group| group.chosen_symbol.as_ref().map(|hit| (group, hit)))
            .take(5)
            .collect::<Vec<_>>();
        selected
            .windows(2)
            .map(|pair| {
                let (from_group, from_hit) = pair[0];
                let (to_group, to_hit) = pair[1];
                let mut notes = Vec::new();
                if from_hit.node_id == to_hit.node_id {
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Supported,
                        confidence: SearchPlanBridgeConfidenceDto::High,
                        evidence_kind: SearchPlanBridgeEvidenceKindDto::SameAnchor,
                        direction: Some("self".to_string()),
                        node_count: 1,
                        edge_count: 0,
                        truncated: false,
                        notes,
                    };
                }
                let forward = self.graph_trail(search_plan_bridge_request(
                    &from_hit.node_id,
                    &to_hit.node_id,
                ));
                if let Ok(graph) = forward
                    && graph_response_has_bridge(&graph, &from_hit.node_id, &to_hit.node_id)
                {
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Supported,
                        confidence: if graph.truncated {
                            SearchPlanBridgeConfidenceDto::Medium
                        } else {
                            SearchPlanBridgeConfidenceDto::High
                        },
                        evidence_kind: graph_bridge_evidence_kind(&graph),
                        direction: Some("forward".to_string()),
                        node_count: clamp_usize_to_u32(graph.nodes.len()),
                        edge_count: clamp_usize_to_u32(graph.edges.len()),
                        truncated: graph.truncated,
                        notes,
                    };
                }
                let reverse = self.graph_trail(search_plan_bridge_request(
                    &to_hit.node_id,
                    &from_hit.node_id,
                ));
                if let Ok(graph) = reverse
                    && graph_response_has_bridge(&graph, &to_hit.node_id, &from_hit.node_id)
                {
                    notes.push(
                        "reverse graph path found; direction is partial evidence for the original flow"
                            .to_string(),
                    );
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Partial,
                        confidence: if graph.truncated {
                            SearchPlanBridgeConfidenceDto::Low
                        } else {
                            SearchPlanBridgeConfidenceDto::Medium
                        },
                        evidence_kind: graph_bridge_evidence_kind(&graph),
                        direction: Some("reverse".to_string()),
                        node_count: clamp_usize_to_u32(graph.nodes.len()),
                        edge_count: clamp_usize_to_u32(graph.edges.len()),
                        truncated: graph.truncated,
                        notes,
                    };
                }
                if shared_file_bridge(from_hit, to_hit) {
                    notes.push(
                        "anchors share a source file; this is low-confidence bridge evidence only"
                            .to_string(),
                    );
                    return SearchPlanBridgeDto {
                        from_anchor: from_group.anchor.clone(),
                        to_anchor: to_group.anchor.clone(),
                        status: SearchPlanBridgeStatusDto::Partial,
                        confidence: SearchPlanBridgeConfidenceDto::Low,
                        evidence_kind: SearchPlanBridgeEvidenceKindDto::SharedFile,
                        direction: None,
                        node_count: 2,
                        edge_count: 0,
                        truncated: false,
                        notes,
                    };
                }
                notes.push(
                    "no graph path or shared-file bridge found inside the bounded evidence budget"
                        .to_string(),
                );
                SearchPlanBridgeDto {
                    from_anchor: from_group.anchor.clone(),
                    to_anchor: to_group.anchor.clone(),
                    status: SearchPlanBridgeStatusDto::Unsupported,
                    confidence: SearchPlanBridgeConfidenceDto::Low,
                    evidence_kind: SearchPlanBridgeEvidenceKindDto::IsolatedAnchors,
                    direction: None,
                    node_count: 2,
                    edge_count: 0,
                    truncated: false,
                    notes,
                }
            })
            .collect()
    }

    /// Run sidecar-primary search and return only the hit list.
    ///
    /// Use [`AppController::search_results`] when the caller needs retrieval mode, diagnostics,
    /// or search-plan metadata.
    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        Ok(self.search_results(req)?.hits)
    }

    /// Run sidecar-primary search with retrieval state metadata.
    ///
    /// The returned retrieval state distinguishes full sidecar evidence from degraded or
    /// diagnostic-only paths. Callers should not collapse those states into a generic success.
    pub fn search_results(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        agent::retrieval_primary::with_stable_retrieval_publication(self, "search output", || {
            self.search_results_once(req.clone())
        })
    }

    fn search_results_once(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        self.ensure_consistent_read_state("Search")?;
        let original_query = req.query.clone();
        let intent_query = parse_search_intent_query(&original_query);
        let limit_per_source = req.limit_per_source.clamp(1, 50) as usize;
        let repo_text_mode = req.repo_text;
        self.search_results_sidecar_primary(
            original_query,
            intent_query,
            limit_per_source,
            repo_text_mode,
            req.expand_search_plan,
            req.hybrid_weights.clone(),
            req.hybrid_limits.clone(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn search_results_sidecar_primary(
        &self,
        original_query: String,
        intent_query: SearchIntentQuery,
        limit_per_source: usize,
        repo_text_mode: SearchRepoTextMode,
        expand_search_plan: bool,
        hybrid_weights: Option<AgentHybridWeightsDto>,
        hybrid_limits: Option<SearchHybridLimitsDto>,
    ) -> Result<SearchResultsDto, ApiError> {
        if !agent::retrieval_primary::sidecar_retrieval_primary_enabled(self) {
            let reason = agent::retrieval_primary::sidecar_retrieval_unavailable_reason(self)
                .unwrap_or_else(|| {
                    "full retrieval is mandatory; legacy search is disabled".to_string()
                });
            return Err(
                agent::retrieval_primary::sidecar_retrieval_unavailable_error(self, reason),
            );
        }

        let query = intent_query.effective_query.clone();
        let (query_result, resolution) = agent::retrieval_primary::run_and_resolve_sidecar_query(
            self,
            &query,
            limit_per_source,
            None,
        )?;
        let mut indexed_symbol_hits = resolution.resolved_hits.clone();
        if let Some(reason) = agent::retrieval_primary::sidecar_primary_result_rejection_reason(
            &query_result,
            &indexed_symbol_hits,
        ) {
            let diagnostic = agent::retrieval_primary::sidecar_rejection_diagnostic(
                self,
                &query_result,
                &indexed_symbol_hits,
                5,
            );
            return Err(
                agent::retrieval_primary::sidecar_retrieval_unavailable_error(
                    self,
                    format!("sidecar search rejected query: {reason}; {diagnostic}"),
                ),
            );
        }
        let initial_sidecar_hits = indexed_symbol_hits.clone();

        apply_search_intent_filters(&mut indexed_symbol_hits, &intent_query.filters);
        let project_root = self.require_project_root().ok();
        indexed_symbol_hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(project_root.as_deref(), &query, left, right)
        });
        dedupe_inexact_search_hits_by_display_key(&query, &mut indexed_symbol_hits);
        indexed_symbol_hits.truncate(limit_per_source);
        annotate_search_hit_match_quality(&query, &mut indexed_symbol_hits);

        let storage = self.open_storage_read_only()?;
        let retrieval = retrieval_state_from_storage_for_runtime(&storage, &self.runtime_config)?;
        let freshness = self.index_freshness().ok();
        let mut repo_text_hits = Vec::new();
        let mut suggestions = Vec::new();
        let query_assessment = search_query_assessment(
            &query,
            &indexed_symbol_hits,
            &repo_text_hits,
            repo_text_mode,
            false,
            None,
        );
        let indexed_hit_ids = indexed_symbol_hits
            .iter()
            .map(|hit| hit.node_id.clone())
            .collect::<HashSet<_>>();
        let mut search_plan_anchor_rank = HashMap::<NodeId, usize>::new();
        let search_plan = if expand_search_plan {
            match self.build_search_plan(
                &storage,
                &original_query,
                &query,
                &query_assessment,
                &indexed_symbol_hits,
                &repo_text_hits,
                &suggestions,
                &retrieval,
                limit_per_source as u32,
                &intent_query.filters,
                &indexed_hit_ids,
                false,
                hybrid_weights,
                hybrid_limits,
            )? {
                Some(plan_build) => {
                    for (rank, group) in plan_build.plan.anchor_groups.iter().enumerate() {
                        if let Some(symbol) = &group.chosen_symbol {
                            search_plan_anchor_rank
                                .entry(symbol.node_id.clone())
                                .or_insert(rank);
                            merge_search_hits_by_node_id(
                                &mut indexed_symbol_hits,
                                vec![symbol.clone()],
                            );
                        }
                    }
                    merge_search_hits_by_node_id(
                        &mut indexed_symbol_hits,
                        plan_build.indexed_symbol_hits,
                    );
                    Some(plan_build.plan)
                }
                None => None,
            }
        } else {
            None
        };
        indexed_symbol_hits.sort_by(|left, right| {
            let anchor_order = match (
                search_plan_anchor_rank.get(&left.node_id),
                search_plan_anchor_rank.get(&right.node_id),
            ) {
                (Some(left_rank), Some(right_rank)) => left_rank.cmp(right_rank),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            anchor_order.then_with(|| {
                compare_search_hits_with_project_root(project_root.as_deref(), &query, left, right)
            })
        });
        dedupe_inexact_search_hits_by_display_key(&query, &mut indexed_symbol_hits);
        let indexed_symbol_candidates = indexed_symbol_hits.clone();
        apply_architecture_cross_source_coverage(
            &query,
            &mut indexed_symbol_hits,
            &mut repo_text_hits,
            &indexed_symbol_candidates,
            &[],
            limit_per_source,
        );
        indexed_symbol_hits.truncate(limit_per_source);
        annotate_search_hit_match_quality(&query, &mut indexed_symbol_hits);
        crate::search_evidence::attach_pinned_search_evidence(
            &storage,
            project_root.as_deref(),
            &mut indexed_symbol_hits,
        );
        crate::search_evidence::attach_pinned_search_evidence(
            &storage,
            project_root.as_deref(),
            &mut suggestions,
        );
        let hits = indexed_symbol_hits.clone();
        let retrieval_shadow = Some(
            agent::retrieval_primary::shadow_from_query_result_with_candidate_admission_diagnostics(
                self,
                query_result.clone(),
                &resolution,
                &initial_sidecar_hits,
                &hits,
            ),
        );

        Ok(SearchResultsDto {
            query: original_query,
            retrieval_publication: None,
            retrieval,
            retrieval_shadow,
            freshness,
            limit_per_source: limit_per_source as u32,
            repo_text_mode,
            repo_text_enabled: false,
            query_assessment: Some(query_assessment),
            search_plan,
            repo_text_stats: None,
            suggestions,
            indexed_symbol_hits,
            repo_text_hits,
            hits,
        })
    }
}
