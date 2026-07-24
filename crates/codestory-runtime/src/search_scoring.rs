use super::{
    AgentHybridWeightsDto, ApiError, AppController, ExpandedSymbolMatches, HashMap, HashSet,
    NodeId, NodeKind, RetrievalStateDto, SearchHit, SearchPlanSubqueryDto, SearchRequest, Storage,
    aggregate_symbol_matches, architecture_query_intents, decorate_search_hit_evidence,
    extract_symbol_search_terms, node_display_name, preferred_occurrence,
    retrieval_file_role_from_path, route_endpoint_adjusted_search_score, symbol_name_match_rank,
};
#[cfg(test)]
use super::{
    EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP, HybridSearchConfig, HybridSearchHit, RetrievalModeDto,
    RetrievalScoreBreakdownDto, SearchEngine, apply_hybrid_limits,
    compare_search_hits_with_project_root, exact_symbol_query_terms, is_non_primary_source_hit,
    looks_like_standalone_symbol_query, mixed_natural_language_query, normalized_hybrid_weights,
    query_mentions_non_primary_source,
};
#[cfg(test)]
use crate::search_publication::{
    retrieval_state_from_engine, retrieval_state_from_engine_with_storage_contract,
    retrieval_state_from_parts, retrieval_state_from_storage,
};
#[cfg(test)]
use crate::search_state::reload_llm_docs_from_storage;
#[cfg(test)]
use crate::search_terms::search_plan_terms;
use crate::search_terms::split_camel_identifier;
#[cfg(test)]
use crate::semantic_projection::LLM_DOC_RELOAD_BATCH_SIZE;

#[derive(Debug, Clone)]
pub(crate) struct HybridSearchScoredHit {
    pub hit: SearchHit,
    pub lexical_score: f32,
    pub semantic_score: f32,
    pub graph_score: f32,
    pub total_score: f32,
}

#[cfg(test)]
pub(super) fn exact_symbol_merged_lexical_hybrid_hits(
    engine: &SearchEngine,
    query: &str,
    graph_boosts: &HashMap<codestory_contracts::graph::NodeId, f32>,
) -> Vec<HybridSearchHit> {
    crate::search::lexical::exact_symbol_merged_lexical_hybrid_hits_for_symbols(
        engine.symbols(),
        query,
        graph_boosts,
    )
}

#[cfg(test)]
pub(super) struct HybridHitsContext<'a> {
    pub(super) req: &'a SearchRequest,
    pub(super) graph_boosts: &'a HashMap<codestory_contracts::graph::NodeId, f32>,
    pub(super) requested_max_results: usize,
    pub(super) request_weights: Option<AgentHybridWeightsDto>,
    pub(super) prefer_primary_sources: bool,
    pub(super) storage_retrieval: &'a RetrievalStateDto,
    pub(super) use_exact_symbol_lexical_fast_path: bool,
}

#[cfg(test)]
pub(super) fn hybrid_hits_for_retrieval_state(
    engine: &mut SearchEngine,
    context: HybridHitsContext<'_>,
    retrieval: &mut RetrievalStateDto,
) -> Vec<HybridSearchHit> {
    let uses_hybrid = !semantic_disabled_by_request_weights(context.request_weights.as_ref())
        && !context.use_exact_symbol_lexical_fast_path
        && retrieval.mode == RetrievalModeDto::Hybrid;
    let mut hits = if !uses_hybrid {
        exact_symbol_merged_lexical_hybrid_hits(engine, &context.req.query, context.graph_boosts)
    } else {
        let config = hybrid_search_config_for_request(
            context.req,
            context.requested_max_results,
            context.request_weights.clone(),
            context.prefer_primary_sources,
        );
        match engine.search_hybrid_with_scores(&context.req.query, context.graph_boosts, config) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(
                    "Hybrid retrieval failed for query {:?}; falling back to symbolic ranking: {}",
                    context.req.query,
                    error
                );
                *retrieval = retrieval_state_from_parts(
                    engine.semantic_doc_count(),
                    engine.embedding_model_id().map(str::to_string),
                    engine.embedding_runtime_configured(),
                    Some(format!(
                        "Semantic query fallback engaged after runtime error: {error}"
                    )),
                    context.storage_retrieval.current_embedding.clone(),
                    context.storage_retrieval.stored_embedding.clone(),
                    true,
                );
                exact_symbol_merged_lexical_hybrid_hits(
                    engine,
                    &context.req.query,
                    context.graph_boosts,
                )
            }
        }
    };
    if uses_hybrid
        && context.request_weights.is_none()
        && !mixed_natural_language_query(&context.req.query)
    {
        let additional = exact_symbol_merged_lexical_hybrid_hits(
            engine,
            &context.req.query,
            context.graph_boosts,
        );
        merge_hybrid_hits_by_node_id(&mut hits, additional);
    }
    hits
}

#[cfg(test)]
pub(super) fn exact_symbol_lexical_fast_path(
    req: &SearchRequest,
    request_weights: Option<&AgentHybridWeightsDto>,
) -> bool {
    request_weights.is_none()
        && req.hybrid_weights.is_none()
        && req
            .hybrid_limits
            .as_ref()
            .and_then(|limits| limits.semantic)
            .is_none()
        && !exact_symbol_query_terms(&req.query).is_empty()
        && has_fast_path_symbol_signal(&req.query)
}

#[cfg(test)]
pub(super) fn semantic_disabled_by_request_weights(
    request_weights: Option<&AgentHybridWeightsDto>,
) -> bool {
    request_weights
        .and_then(|weights| weights.semantic)
        .is_some_and(|semantic| semantic <= f32::EPSILON)
}

#[cfg(test)]
pub(super) fn has_fast_path_symbol_signal(query: &str) -> bool {
    let trimmed = query.trim();
    looks_like_standalone_symbol_query(trimmed)
        && (trimmed.contains('_')
            || trimmed.contains("::")
            || trimmed.contains('.')
            || trimmed.contains('$')
            || trimmed
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
            || trimmed.chars().skip(1).any(|ch| ch.is_ascii_uppercase()))
}

#[cfg(test)]
pub(super) fn merge_hybrid_hits_by_node_id(
    hits: &mut Vec<HybridSearchHit>,
    additional: Vec<HybridSearchHit>,
) {
    let mut existing = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| (hit.node_id, index))
        .collect::<HashMap<_, _>>();

    for hit in additional {
        if let Some(index) = existing.get(&hit.node_id).copied() {
            let current = &mut hits[index];
            current.lexical_score = current.lexical_score.max(hit.lexical_score);
            current.semantic_score = current.semantic_score.max(hit.semantic_score);
            current.graph_score = current.graph_score.max(hit.graph_score);
            current.total_score = current.total_score.max(hit.total_score);
            continue;
        }

        existing.insert(hit.node_id, hits.len());
        hits.push(hit);
    }
}

#[cfg(test)]
pub(super) fn hybrid_search_config_for_request(
    req: &SearchRequest,
    requested_max_results: usize,
    request_weights: Option<AgentHybridWeightsDto>,
    prefer_primary_sources: bool,
) -> HybridSearchConfig {
    let mut config = HybridSearchConfig {
        max_results: requested_max_results,
        ..HybridSearchConfig::default()
    };
    let has_request_weights = request_weights.is_some();
    let (lexical_weight, semantic_weight, graph_weight) =
        normalized_hybrid_weights(request_weights, &config);
    config.lexical_weight = lexical_weight;
    config.semantic_weight = semantic_weight;
    config.graph_weight = graph_weight;
    let has_exact_symbol_terms = !exact_symbol_query_terms(&req.query).is_empty();
    let mixed_nl = mixed_natural_language_query(&req.query);
    if !has_request_weights && has_exact_symbol_terms && !mixed_nl {
        config.lexical_weight = 0.85;
        config.semantic_weight = 0.15;
        config.graph_weight = 0.0;
        config.max_results = requested_max_results
            .saturating_mul(5)
            .clamp(requested_max_results, EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP);
        config.lexical_limit = config.lexical_limit.max(80);
        config.semantic_limit = config.semantic_limit.max(20);
    }
    apply_hybrid_limits(req.hybrid_limits.clone(), &mut config);
    if prefer_primary_sources && !has_exact_symbol_terms {
        config.max_results = requested_max_results.saturating_mul(5).min(80);
    }
    config
}

#[cfg(test)]
pub(super) fn should_pretruncate_primary_source_window(
    query: &str,
    prefer_primary_sources: bool,
    candidate_count: usize,
    requested_max_results: usize,
) -> bool {
    prefer_primary_sources
        && exact_symbol_query_terms(query).is_empty()
        && candidate_count > requested_max_results
}

#[cfg(test)]
pub(super) fn primary_source_retention_threshold(requested_max_results: usize) -> usize {
    requested_max_results.clamp(1, 3)
}

pub(super) fn merge_search_hits_by_node_id(hits: &mut Vec<SearchHit>, additional: Vec<SearchHit>) {
    let mut existing = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| (hit.node_id.clone(), index))
        .collect::<HashMap<_, _>>();

    for hit in additional {
        if let Some(index) = existing.get(&hit.node_id).copied() {
            if hit.score > hits[index].score {
                hits[index] = hit;
            }
            continue;
        }

        existing.insert(hit.node_id.clone(), hits.len());
        hits.push(hit);
    }
}

pub(super) fn search_plan_subquery_candidate_limit(
    subquery: &SearchPlanSubqueryDto,
    limit: usize,
) -> usize {
    if subquery.role == "original_question"
        && architecture_query_intents(&subquery.query).is_empty()
    {
        limit
    } else {
        limit.saturating_mul(5).clamp(limit, 50)
    }
}

#[cfg(test)]
pub(super) fn truncate_repo_text_hits_for_query(
    query: &str,
    hits: &mut Vec<SearchHit>,
    limit: usize,
) {
    if limit == 0 {
        hits.clear();
        return;
    }
    if hits.len() <= limit {
        return;
    }
    if architecture_query_intents(query).is_empty() {
        hits.truncate(limit);
        return;
    }
    diversify_architecture_repo_text_hits(query, hits, limit);
}

#[cfg(test)]
pub(super) fn diversify_architecture_repo_text_hits(
    query: &str,
    hits: &mut Vec<SearchHit>,
    limit: usize,
) {
    let query_terms = search_plan_terms(query)
        .extracted
        .into_iter()
        .map(|term| term.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut selected = hits
        .iter()
        .take(limit)
        .cloned()
        .enumerate()
        .collect::<Vec<_>>();

    for (candidate_rank, candidate) in hits.iter().enumerate().skip(limit) {
        let candidate_score = architecture_repo_text_surface_score(&query_terms, candidate);
        if candidate_score == 0 {
            continue;
        }
        let Some(candidate_key) = architecture_repo_text_surface_key(candidate) else {
            continue;
        };
        if selected.iter().any(|(_, hit)| {
            architecture_repo_text_surface_key(hit).as_ref() == Some(&candidate_key)
        }) {
            continue;
        }
        let Some(replace_index) =
            architecture_repo_text_replacement_index(&query_terms, &selected, candidate_score)
        else {
            continue;
        };
        selected[replace_index] = (candidate_rank, candidate.clone());
    }

    selected.sort_by_key(|(rank, _)| *rank);
    *hits = selected.into_iter().map(|(_, hit)| hit).collect();
}

#[cfg(test)]
pub(super) fn architecture_repo_text_replacement_index(
    query_terms: &HashSet<String>,
    selected: &[(usize, SearchHit)],
    candidate_score: u32,
) -> Option<usize> {
    let mut bucket_counts = HashMap::<String, usize>::new();
    for (_, hit) in selected {
        if let Some(bucket) = architecture_repo_text_bucket_key(hit) {
            *bucket_counts.entry(bucket).or_default() += 1;
        }
    }

    selected
        .iter()
        .enumerate()
        .filter_map(|(index, (rank, hit))| {
            let bucket = architecture_repo_text_bucket_key(hit)?;
            if bucket_counts.get(&bucket).copied().unwrap_or_default() <= 1 {
                return None;
            }
            let score = architecture_repo_text_surface_score(query_terms, hit);
            let strong_distinct_surface = candidate_score >= 3 && score <= candidate_score + 1;
            (candidate_score >= score || strong_distinct_surface).then_some((index, score, *rank))
        })
        .min_by_key(|(_, score, rank)| (*score, std::cmp::Reverse(*rank)))
        .map(|(index, _, _)| index)
}

#[cfg(test)]
pub(super) fn architecture_repo_text_surface_score(
    query_terms: &HashSet<String>,
    hit: &SearchHit,
) -> u32 {
    let Some(path) = hit.file_path.as_deref() else {
        return 0;
    };
    if retrieval_file_role_from_path(path).is_non_primary() {
        return 0;
    }
    architecture_repo_text_surface_terms(path)
        .into_iter()
        .filter(|term| query_terms.contains(*term))
        .count()
        .min(u32::MAX as usize) as u32
}

#[cfg(test)]
pub(super) fn architecture_repo_text_surface_terms(path: &str) -> Vec<&'static str> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let path_terms = architecture_coverage_terms(path, "");
    let mut terms = Vec::new();
    if normalized.contains("sourcegroup")
        || normalized.contains("source_group")
        || normalized.contains("source-group")
        || architecture_has_all_terms(&path_terms, &["source", "group"])
    {
        terms.extend(["source", "group", "source-group", "configuration"]);
    }
    if architecture_has_all_terms(&path_terms, &["source", "group"])
        && architecture_has_any_term(&path_terms, &["cxx", "cdb", "compile", "database"])
    {
        terms.extend(["source", "group", "source-group", "cxx", "cdb", "indexing"]);
    }
    if normalized.contains("lib_cxx") || normalized.contains("/cxx/") {
        terms.extend(["cxx", "indexing", "indexer"]);
    }
    if normalized.contains("lib_java") || normalized.contains("/java/") {
        terms.extend(["java", "indexing", "indexer"]);
    }
    if normalized.contains("/data/indexer/") || normalized.contains("indexercommand") {
        terms.extend(["data", "indexer", "indexing", "command", "work"]);
    }
    if normalized.contains("/data/storage/")
        || architecture_has_all_terms(&path_terms, &["storage", "access"])
        || architecture_has_all_terms(&path_terms, &["persistent", "storage"])
    {
        terms.extend(["data", "storage", "access", "persistence"]);
    }
    if normalized.contains("/project/") {
        terms.extend(["project", "source", "group", "configuration"]);
    }
    if normalized.contains("codestory-cli") {
        terms.extend(["cli", "command", "entrypoint"]);
    }
    if normalized.contains("codestory-workspace") {
        terms.extend(["workspace", "file", "discovery", "source"]);
    }
    if normalized.contains("codestory-indexer") {
        terms.extend(["indexer", "indexing", "symbol", "extraction", "extract"]);
    }
    if normalized.contains("codestory-store") {
        terms.extend(["store", "storage", "persistence", "persist"]);
    }
    if normalized.contains("snapshot") {
        terms.extend(["snapshot", "refresh"]);
    }
    if normalized.contains("storage_impl") || normalized.contains("/storage_impl/") {
        terms.extend(["storage", "persistence", "projection", "sqlite"]);
    }
    if normalized.contains("/collections/") {
        terms.extend(["payload", "collection", "collections"]);
        if architecture_has_any_term(&path_terms, &["post", "posts"]) {
            terms.extend(["posts", "post", "writing"]);
        }
        if architecture_has_any_term(&path_terms, &["comment", "comments"]) {
            terms.extend(["comments", "comment"]);
        }
        if architecture_has_all_terms(&path_terms, &["social", "entries"]) {
            terms.extend(["social", "elsewhere", "feed"]);
        }
    }
    if normalized.ends_with("/lib/payload.ts") {
        terms.extend(["payload", "client"]);
    }
    if normalized.contains("/lib/content-data/") {
        terms.extend(["content"]);
        if normalized.ends_with("/post-content.ts") {
            terms.extend(["posts", "post", "writing"]);
        }
        if normalized.ends_with("/comment-content.ts") {
            terms.extend(["comments", "comment"]);
        }
        if normalized.ends_with("/social-entry-content.ts") {
            terms.extend(["social", "elsewhere", "feed"]);
        }
    }
    if normalized.ends_with("/app/feed.xml/route.ts") {
        terms.extend(["feed", "rss"]);
    }
    if normalized.ends_with("/posts/[slug]/comments/route.ts") {
        terms.extend(["posts", "comments", "comment", "submission"]);
    }
    if normalized.contains("codestory-runtime") {
        if normalized.ends_with("/lib.rs") {
            terms.extend(["runtime", "orchestration", "indexing"]);
        }
        if normalized.contains("/services.rs") {
            terms.extend(["runtime", "service", "orchestration", "indexing"]);
        }
        if normalized.contains("/search") || normalized.contains("search_runtime") {
            terms.extend(["search"]);
        }
        if normalized.contains("symbol_query") {
            terms.extend(["symbol", "search"]);
        }
        if normalized.contains("/agent/") || normalized.contains("orchestrator") {
            terms.extend(["orchestration"]);
        }
        if normalized.contains("graph") {
            terms.extend(["graph"]);
        }
    }
    terms
}

#[cfg(test)]
pub(super) fn architecture_repo_text_surface_key(hit: &SearchHit) -> Option<String> {
    architecture_repo_text_path_key(hit, true)
}

#[cfg(test)]
pub(super) fn architecture_repo_text_bucket_key(hit: &SearchHit) -> Option<String> {
    architecture_repo_text_path_key(hit, false)
}

#[cfg(test)]
pub(super) fn architecture_repo_text_path_key(
    hit: &SearchHit,
    include_surface: bool,
) -> Option<String> {
    let path = normalize_repo_text_path(hit.file_path.as_deref()?);
    let parts = path.split('/').collect::<Vec<_>>();
    if let Some(crate_index) = parts.iter().position(|part| *part == "crates") {
        let crate_name = parts.get(crate_index + 1)?;
        if !include_surface {
            return Some(format!("crates/{crate_name}"));
        }
        let surface = parts
            .iter()
            .skip(crate_index + 2)
            .position(|part| *part == "src")
            .and_then(|src_offset| parts.get(crate_index + 3 + src_offset))
            .map(|part| part.trim_end_matches(".rs"))
            .unwrap_or("root");
        return Some(format!("crates/{crate_name}/{surface}"));
    }
    let src_index = parts.iter().position(|part| *part == "src")?;
    let root = parts.get(src_index + 1)?;
    let domain = parts.get(src_index + 2).copied().unwrap_or("root");
    if !include_surface {
        return Some(format!("src/{root}/{domain}"));
    }
    let stem = parts
        .last()
        .map(|part| part.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(part))
        .unwrap_or("root");
    Some(format!("src/{root}/{domain}/{stem}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArchitectureCoverage {
    pub(super) key: String,
    pub(super) score: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArchitectureCoverageLane {
    Indexed,
    RepoText,
}

#[derive(Debug, Clone)]
pub(super) struct ArchitectureCoverageCandidate {
    lane: ArchitectureCoverageLane,
    source_rank: usize,
    hit: SearchHit,
    coverage: ArchitectureCoverage,
}

pub(super) fn apply_architecture_cross_source_coverage(
    query: &str,
    indexed_symbol_hits: &mut Vec<SearchHit>,
    repo_text_hits: &mut Vec<SearchHit>,
    indexed_candidates: &[SearchHit],
    repo_text_candidates: &[SearchHit],
    limit: usize,
) {
    if limit == 0 || architecture_query_intents(query).is_empty() {
        return;
    }
    indexed_symbol_hits.truncate(limit);
    repo_text_hits.truncate(limit);

    let mut selected_ids = indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .map(|hit| hit.node_id.clone())
        .collect::<HashSet<_>>();
    let mut selected_paths = indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .filter_map(|hit| hit.file_path.as_deref())
        .map(normalize_repo_text_path)
        .collect::<HashSet<_>>();
    let mut selected_keys =
        architecture_selected_coverage_keys(indexed_symbol_hits, repo_text_hits);

    let mut candidates = indexed_candidates
        .iter()
        .enumerate()
        .skip(limit)
        .filter_map(|(rank, hit)| {
            architecture_coverage_candidate(
                ArchitectureCoverageLane::Indexed,
                rank,
                hit,
                &selected_ids,
                &selected_paths,
                &selected_keys,
            )
        })
        .chain(
            repo_text_candidates
                .iter()
                .enumerate()
                .skip(limit)
                .filter_map(|(rank, hit)| {
                    architecture_coverage_candidate(
                        ArchitectureCoverageLane::RepoText,
                        rank,
                        hit,
                        &selected_ids,
                        &selected_paths,
                        &selected_keys,
                    )
                }),
        )
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .coverage
            .score
            .cmp(&left.coverage.score)
            .then_with(|| left.source_rank.cmp(&right.source_rank))
            .then_with(|| left.coverage.key.cmp(&right.coverage.key))
    });

    let mut replacement_count = 0usize;
    let replacement_limit = limit.min(8);
    for candidate in candidates {
        if selected_keys.contains(&candidate.coverage.key)
            || selected_ids.contains(&candidate.hit.node_id)
            || candidate
                .hit
                .file_path
                .as_deref()
                .map(normalize_repo_text_path)
                .is_some_and(|path| selected_paths.contains(&path))
        {
            continue;
        }
        let Some(replace_index) = architecture_coverage_replacement_index(
            indexed_symbol_hits,
            repo_text_hits,
            candidate.lane,
            candidate.coverage.score,
        ) else {
            continue;
        };

        match candidate.lane {
            ArchitectureCoverageLane::Indexed => {
                selected_ids.remove(&indexed_symbol_hits[replace_index].node_id);
                if let Some(path) = indexed_symbol_hits[replace_index].file_path.as_deref() {
                    selected_paths.remove(&normalize_repo_text_path(path));
                }
                indexed_symbol_hits[replace_index] = candidate.hit.clone();
            }
            ArchitectureCoverageLane::RepoText => {
                selected_ids.remove(&repo_text_hits[replace_index].node_id);
                if let Some(path) = repo_text_hits[replace_index].file_path.as_deref() {
                    selected_paths.remove(&normalize_repo_text_path(path));
                }
                repo_text_hits[replace_index] = candidate.hit.clone();
            }
        }
        selected_ids.insert(candidate.hit.node_id.clone());
        if let Some(path) = candidate.hit.file_path.as_deref() {
            selected_paths.insert(normalize_repo_text_path(path));
        }
        selected_keys.insert(candidate.coverage.key);
        replacement_count += 1;
        if replacement_count >= replacement_limit {
            break;
        }
    }
}

pub(super) fn architecture_coverage_candidate(
    lane: ArchitectureCoverageLane,
    source_rank: usize,
    hit: &SearchHit,
    selected_ids: &HashSet<NodeId>,
    selected_paths: &HashSet<String>,
    selected_keys: &HashSet<String>,
) -> Option<ArchitectureCoverageCandidate> {
    if selected_ids.contains(&hit.node_id) {
        return None;
    }
    if hit
        .file_path
        .as_deref()
        .map(normalize_repo_text_path)
        .is_some_and(|path| selected_paths.contains(&path))
    {
        return None;
    }
    let coverage = architecture_coverage_for_hit(hit)?;
    (!selected_keys.contains(&coverage.key)).then(|| ArchitectureCoverageCandidate {
        lane,
        source_rank,
        hit: hit.clone(),
        coverage,
    })
}

pub(super) fn architecture_selected_coverage_keys(
    indexed_symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
) -> HashSet<String> {
    indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .filter_map(architecture_coverage_for_hit)
        .map(|coverage| coverage.key)
        .collect()
}

pub(super) fn architecture_coverage_replacement_index(
    indexed_symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
    lane: ArchitectureCoverageLane,
    candidate_score: u32,
) -> Option<usize> {
    let key_counts = architecture_coverage_key_counts(indexed_symbol_hits, repo_text_hits);
    let hits = match lane {
        ArchitectureCoverageLane::Indexed => indexed_symbol_hits,
        ArchitectureCoverageLane::RepoText => repo_text_hits,
    };
    hits.iter()
        .enumerate()
        .filter_map(|(index, hit)| {
            let coverage = architecture_coverage_for_hit(hit);
            let score = coverage
                .as_ref()
                .map(|coverage| coverage.score)
                .unwrap_or(0);
            let protected = coverage.as_ref().is_some_and(|coverage| {
                coverage.score >= 8 && key_counts.get(&coverage.key).copied().unwrap_or(0) <= 1
            });
            if protected || score > candidate_score {
                return None;
            }
            Some((index, score))
        })
        .min_by_key(|(_, score)| *score)
        .map(|(index, _)| index)
}

pub(super) fn architecture_coverage_key_counts(
    indexed_symbol_hits: &[SearchHit],
    repo_text_hits: &[SearchHit],
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for coverage in indexed_symbol_hits
        .iter()
        .chain(repo_text_hits.iter())
        .filter_map(architecture_coverage_for_hit)
    {
        *counts.entry(coverage.key).or_default() += 1;
    }
    counts
}

pub(super) fn architecture_coverage_for_hit(hit: &SearchHit) -> Option<ArchitectureCoverage> {
    let path = hit.file_path.as_deref()?;
    if retrieval_file_role_from_path(path).is_non_primary() {
        return None;
    }
    let normalized = normalize_repo_text_path(path);
    let terms = architecture_coverage_terms(path, &hit.display_name);
    let source_kind = architecture_source_kind(&normalized);
    let coverage = if normalized.contains("/cli/src/main.rs") {
        ArchitectureCoverage {
            key: format!("cli:top_level_entrypoint:{source_kind}"),
            score: 8,
        }
    } else if normalized.contains("/exec/src/main.rs") {
        ArchitectureCoverage {
            key: format!("exec:binary_entrypoint:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/cli.rs") {
        ArchitectureCoverage {
            key: format!("exec:cli_options:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/exec/src/lib.rs") {
        ArchitectureCoverage {
            key: format!("exec:runtime:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/")
        && architecture_has_all_terms(&terms, &["exec", "events"])
        && architecture_path_stem(&normalized).contains("events")
    {
        ArchitectureCoverage {
            key: format!("exec:events:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/")
        && architecture_has_all_terms(&terms, &["event", "processor", "jsonl", "output"])
    {
        ArchitectureCoverage {
            key: format!("exec:jsonl_event_processor:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/exec/src/")
        && architecture_has_all_terms(&terms, &["event", "processor"])
    {
        ArchitectureCoverage {
            key: format!("exec:event_processor:{source_kind}"),
            score: 8,
        }
    } else if architecture_has_all_terms(&terms, &["source", "group"])
        && architecture_has_any_term(&terms, &["config", "cdb", "compile", "database", "cxx"])
    {
        ArchitectureCoverage {
            key: format!("source_group:configuration:{source_kind}"),
            score: 10,
        }
    } else if architecture_has_all_terms(&terms, &["indexer", "command", "cxx"]) {
        ArchitectureCoverage {
            key: format!("indexing:cxx_command:{source_kind}"),
            score: 10,
        }
    } else if architecture_has_all_terms(&terms, &["indexer", "java"]) {
        ArchitectureCoverage {
            key: format!("indexing:java:{source_kind}"),
            score: if source_kind == "impl" { 10 } else { 8 },
        }
    } else if architecture_has_all_terms(&terms, &["storage", "access", "proxy"]) {
        ArchitectureCoverage {
            key: format!("storage:access_proxy:{source_kind}"),
            score: if source_kind == "impl" { 10 } else { 8 },
        }
    } else if architecture_has_all_terms(&terms, &["persistent", "storage"]) {
        ArchitectureCoverage {
            key: format!("storage:persistent:{source_kind}"),
            score: 8,
        }
    } else if architecture_has_all_terms(&terms, &["storage", "access"]) {
        ArchitectureCoverage {
            key: format!("storage:access:{source_kind}"),
            score: 9,
        }
    } else if normalized.ends_with("/project.cpp")
        || architecture_has_all_terms(&terms, &["project", "build", "index"])
    {
        ArchitectureCoverage {
            key: format!("project:build_index:{source_kind}"),
            score: 8,
        }
    } else if architecture_has_all_terms(&terms, &["indexer", "command"]) {
        ArchitectureCoverage {
            key: format!(
                "indexing:{}:{source_kind}",
                architecture_path_stem(&normalized)
            ),
            score: 4,
        }
    } else if normalized.contains("sourcegroup") && normalized.contains("/project/") {
        ArchitectureCoverage {
            key: format!(
                "source_group:{}:{source_kind}",
                architecture_path_stem(&normalized)
            ),
            score: 4,
        }
    } else if architecture_has_all_terms(&terms, &["payload", "config"])
        && normalized.ends_with(".ts")
    {
        ArchitectureCoverage {
            key: format!("payload:config:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/collections/")
        && architecture_has_any_term(&terms, &["post", "posts"])
    {
        ArchitectureCoverage {
            key: format!("payload:posts_collection:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/collections/")
        && architecture_has_any_term(&terms, &["comment", "comments"])
    {
        ArchitectureCoverage {
            key: format!("payload:comments_collection:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/collections/")
        && architecture_has_all_terms(&terms, &["social", "entries"])
    {
        ArchitectureCoverage {
            key: format!("payload:social_entries_collection:{source_kind}"),
            score: 9,
        }
    } else if normalized.contains("/posts/")
        && normalized.contains("/comments/")
        && architecture_has_all_terms(&terms, &["comments", "route"])
    {
        ArchitectureCoverage {
            key: format!("comments:submission_route:{source_kind}"),
            score: 10,
        }
    } else if architecture_has_all_terms(&terms, &["feed", "route"]) {
        ArchitectureCoverage {
            key: format!("feed:rss_route:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/lib/")
        && architecture_has_all_terms(&terms, &["payload"])
        && architecture_has_any_term(&terms, &["client", "lib"])
    {
        ArchitectureCoverage {
            key: format!("payload:client:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/content-data/")
        && architecture_has_all_terms(&terms, &["post", "content"])
    {
        ArchitectureCoverage {
            key: format!("content:post_data:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/content-data/")
        && architecture_has_all_terms(&terms, &["comment", "content"])
    {
        ArchitectureCoverage {
            key: format!("content:comment_data:{source_kind}"),
            score: 10,
        }
    } else if normalized.contains("/content-data/")
        && architecture_has_all_terms(&terms, &["social", "content"])
    {
        ArchitectureCoverage {
            key: format!("content:social_data:{source_kind}"),
            score: 9,
        }
    } else {
        return None;
    };
    Some(coverage)
}

pub(super) fn architecture_coverage_terms(path: &str, display_name: &str) -> HashSet<String> {
    let mut terms = HashSet::new();
    for raw in [path, display_name] {
        for fragment in raw.split(|ch: char| !ch.is_ascii_alphanumeric()) {
            if fragment.is_empty() {
                continue;
            }
            terms.insert(fragment.to_ascii_lowercase());
            for camel_part in split_camel_identifier(fragment) {
                terms.insert(camel_part);
            }
        }
    }
    terms
}

pub(super) fn architecture_has_all_terms(terms: &HashSet<String>, required: &[&str]) -> bool {
    required.iter().all(|term| terms.contains(*term))
}

pub(super) fn architecture_has_any_term(terms: &HashSet<String>, required: &[&str]) -> bool {
    required.iter().any(|term| terms.contains(*term))
}

pub(super) fn architecture_source_kind(path: &str) -> &'static str {
    if path.ends_with(".h") || path.ends_with(".hpp") || path.ends_with(".hh") {
        "decl"
    } else if path.ends_with(".cpp")
        || path.ends_with(".cxx")
        || path.ends_with(".cc")
        || path.ends_with(".c")
        || path.ends_with(".rs")
        || path.ends_with(".ts")
        || path.ends_with(".tsx")
        || path.ends_with(".js")
        || path.ends_with(".jsx")
    {
        "impl"
    } else {
        "file"
    }
}

pub(super) fn architecture_path_stem(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.').map(|(stem, _)| stem))
        .unwrap_or(path)
}

pub(super) fn normalize_repo_text_path(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

pub(super) fn dedupe_inexact_search_hits_by_display_key(query: &str, hits: &mut Vec<SearchHit>) {
    let mut seen = HashSet::<(String, NodeKind, Option<String>)>::new();
    hits.retain(|hit| {
        let rank = symbol_name_match_rank(query, &hit.display_name);
        let is_exact_match =
            rank.exact_display != 0 || rank.exact_terminal != 0 || rank.exact_leading != 0;
        if is_exact_match {
            return true;
        }

        seen.insert((hit.display_name.clone(), hit.kind, hit.file_path.clone()))
    });
}

pub(super) fn did_you_mean_suggestions(scored_hits: &[HybridSearchScoredHit]) -> Vec<SearchHit> {
    const MIN_SEMANTIC_SCORE: f32 = 0.18;
    const MAX_SUGGESTIONS: usize = 5;

    if scored_hits.is_empty()
        || scored_hits
            .iter()
            .any(|hit| hit.lexical_score > 0.01 || hit.graph_score > 0.25)
    {
        return Vec::new();
    }

    scored_hits
        .iter()
        .filter(|hit| hit.semantic_score >= MIN_SEMANTIC_SCORE)
        .take(MAX_SUGGESTIONS)
        .map(|hit| hit.hit.clone())
        .collect()
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(super) struct HybridSearchInstrumentation {
    pub(super) symbol_table_size: usize,
    pub(super) exact_symbol_merge_queries: usize,
    pub(super) hybrid_max_results: usize,
    pub(super) hybrid_lexical_limit: usize,
    pub(super) hybrid_semantic_limit: usize,
    pub(super) mixed_natural_language: bool,
}

impl AppController {
    pub(crate) fn build_search_hit(
        storage: &Storage,
        node_names: &HashMap<codestory_contracts::graph::NodeId, String>,
        id: codestory_contracts::graph::NodeId,
        score: f32,
    ) -> Result<Option<SearchHit>, ApiError> {
        let node = match storage.get_node(id) {
            Ok(Some(node)) if node.kind != codestory_contracts::graph::NodeKind::UNKNOWN => node,
            _ => return Ok(None),
        };

        let display_name = node_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| node_display_name(&node));

        let mut file_path = Self::file_path_for_node(storage, &node).ok().flatten();
        let mut line = node.start_line;
        if let Ok(occs) = storage.get_occurrences_for_node(id)
            && let Some(occ) = preferred_occurrence(&occs)
        {
            if file_path.is_none()
                && let Ok(Some(file_node)) = storage.get_node(occ.location.file_node_id)
            {
                file_path = Some(file_node.serialized_name);
            }
            if line.is_none() {
                line = Some(occ.location.start_line);
            }
        }

        let openapi_endpoint = node
            .canonical_id
            .as_deref()
            .is_some_and(|value| value.starts_with("openapi:endpoint:"));
        let structural_unit = storage.get_structural_text_unit(id).map_err(|error| {
            ApiError::internal(format!(
                "Failed to load structural provenance for node {}: {error}",
                id.0
            ))
        })?;

        let mut hit = SearchHit {
            node_id: NodeId::from(id),
            display_name,
            kind: NodeKind::from(node.kind),
            file_path,
            line,
            score: route_endpoint_adjusted_search_score(score, node.canonical_id.as_deref()),
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: Some(if structural_unit.is_some() {
                codestory_contracts::api::PacketEvidenceTierDto::StructuralText
            } else if openapi_endpoint {
                codestory_contracts::api::PacketEvidenceTierDto::ExactSource
            } else {
                codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph
            }),
            evidence_producer: Some(if let Some(unit) = structural_unit.as_ref() {
                unit.producer.clone()
            } else {
                if openapi_endpoint {
                    "openapi_endpoint_schema"
                } else {
                    "route_endpoint"
                }
                .to_string()
            }),
            resolution_status: Some(if structural_unit.is_some() || openapi_endpoint {
                codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly
            } else {
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved
            }),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(structural_unit.is_none() && !openapi_endpoint),
            source_excerpt: None,
            verification_targets: Vec::new(),
            score_breakdown: None,
        };
        decorate_search_hit_evidence(&mut hit);
        Ok(Some(hit))
    }

    #[cfg(test)]
    #[allow(dead_code)]
    #[cfg(test)]
    pub(super) fn is_repo_explanation_search_query(query: &str) -> bool {
        let lower = query.to_ascii_lowercase();
        let subject =
            lower.contains("repo") || lower.contains("repository") || lower.contains("codebase");
        let intent = lower.contains("fit together")
            || lower.contains("how does")
            || lower.contains("explain")
            || lower.contains("overview")
            || lower.contains("architecture");
        subject && intent
    }

    pub(super) fn expanded_symbol_hits(
        &self,
        storage: &Storage,
        query: &str,
    ) -> Result<Vec<SearchHit>, ApiError> {
        let Some((expanded_matches, node_names)) = self.expanded_symbol_matches(query)? else {
            return Ok(Vec::new());
        };
        Ok(expanded_matches
            .into_iter()
            .map(|(id, score)| Self::build_search_hit(storage, &node_names, id, score))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect())
    }

    fn expanded_symbol_matches(&self, query: &str) -> Result<ExpandedSymbolMatches, ApiError> {
        let mut s = self.state.lock();
        let engine = s.search_engine.as_mut().ok_or_else(|| {
            ApiError::invalid_argument("Search engine not initialized. Open a project first.")
        })?;
        let direct_matches = engine.search_symbol_with_scores(query);
        let terms = extract_symbol_search_terms(query);
        if terms.is_empty() {
            return Ok(None);
        }

        let mut expanded = Vec::<(codestory_contracts::graph::NodeId, f32)>::new();
        for term in terms {
            expanded.extend(engine.search_symbol_with_scores(&term));
            if let Ok(ids) = engine.search_full_text(&term) {
                expanded.extend(ids.into_iter().enumerate().map(|(rank, id)| {
                    let text_score = 40.0_f32 - (rank as f32 * 1.5);
                    (id, text_score)
                }));
            }
        }

        Ok(Some((
            aggregate_symbol_matches(direct_matches, expanded),
            s.node_names.clone(),
        )))
    }

    fn search_hybrid_results(
        &self,
        mut req: SearchRequest,
        _focus_node_id: Option<NodeId>,
        max_results: usize,
        _request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<(Vec<SearchHit>, RetrievalStateDto), ApiError> {
        req.limit_per_source = max_results.clamp(1, 50) as u32;
        req.expand_search_plan = false;
        let results = self.search_results(req)?;
        Ok((results.hits, results.retrieval))
    }

    /// Run hybrid search through the same sidecar-primary contract as `search_results`.
    ///
    /// `max_results` limits returned hits; it is not a retrieval budget and does not prove packet
    /// sufficiency.
    pub fn search_hybrid(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: Option<u32>,
        hybrid_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<Vec<SearchHit>, ApiError> {
        let (hits, _) = self.search_hybrid_results(
            req,
            focus_node_id,
            max_results.unwrap_or(20).clamp(1, 50) as usize,
            hybrid_weights,
        )?;
        Ok(hits)
    }

    pub(crate) fn search_hybrid_scored(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: usize,
        request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<Vec<HybridSearchScoredHit>, ApiError> {
        let (hits, _) =
            self.search_hybrid_results(req, focus_node_id, max_results, request_weights)?;
        Ok(hits
            .into_iter()
            .map(|hit| HybridSearchScoredHit {
                lexical_score: hit.score,
                semantic_score: hit
                    .score_breakdown
                    .as_ref()
                    .map(|scores| scores.semantic)
                    .unwrap_or(0.0),
                graph_score: hit
                    .score_breakdown
                    .as_ref()
                    .map(|scores| scores.graph)
                    .unwrap_or(0.0),
                total_score: hit
                    .score_breakdown
                    .as_ref()
                    .map(|scores| scores.total)
                    .unwrap_or(hit.score),
                hit,
            })
            .collect())
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn search_hybrid_scored_inner(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: usize,
        request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<(Vec<HybridSearchScoredHit>, RetrievalStateDto), ApiError> {
        self.ensure_search_state()?;
        let storage = self.open_storage_read_only()?;
        let semantic_disabled = semantic_disabled_by_request_weights(request_weights.as_ref());
        let storage_retrieval = if semantic_disabled {
            None
        } else {
            Some(retrieval_state_from_storage(&storage)?)
        };
        let mut graph_boosts = HashMap::<codestory_contracts::graph::NodeId, f32>::new();
        let requested_max_results = max_results.clamp(1, 50);
        let prefer_primary_sources = !query_mentions_non_primary_source(&req.query);
        let use_exact_symbol_lexical_fast_path =
            exact_symbol_lexical_fast_path(&req, request_weights.as_ref());
        let hybrid_config = hybrid_search_config_for_request(
            &req,
            requested_max_results,
            request_weights.clone(),
            prefer_primary_sources,
        );
        let exact_symbol_merge_queries =
            crate::search::lexical::exact_symbol_merged_lexical_queries(&req.query).len();

        let focus_core_id = match focus_node_id {
            Some(value) => Some(value.to_core()?),
            None => None,
        };
        if let Some(center) = focus_core_id {
            graph_boosts.insert(center, 1.0);
            if let Ok(edges) = storage.get_edges_for_node_id(center) {
                for edge in edges.into_iter().take(240) {
                    let (source, target) = edge.effective_endpoints();
                    if source != center {
                        graph_boosts.entry(source).or_insert(0.55);
                    }
                    if target != center {
                        graph_boosts.entry(target).or_insert(0.55);
                    }
                }
            }
        }

        let (hybrid, node_names, retrieval) = {
            let mut s = self.state.lock();
            let engine = s.search_engine.as_mut().ok_or_else(|| {
                ApiError::invalid_argument("Search engine not initialized. Open a project first.")
            })?;
            let mut retrieval = storage_retrieval
                .clone()
                .unwrap_or_else(|| retrieval_state_from_engine(engine));

            if !semantic_disabled
                && !use_exact_symbol_lexical_fast_path
                && retrieval.mode == RetrievalModeDto::Hybrid
                && engine.semantic_doc_count() == 0
            {
                if !engine.embedding_runtime_configured()
                    && let Err(error) =
                        engine.set_embedding_runtime_for_runtime(&self.runtime_config)
                {
                    tracing::warn!(
                        "Search embedding runtime unavailable during hybrid load: {error}"
                    );
                }
                if engine.embedding_runtime_configured() && engine.semantic_doc_count() == 0 {
                    reload_llm_docs_from_storage(&storage, engine, LLM_DOC_RELOAD_BATCH_SIZE)?;
                }
                if let Some(storage_retrieval) = storage_retrieval.as_ref() {
                    retrieval = retrieval_state_from_engine_with_storage_contract(
                        engine,
                        storage_retrieval,
                    );
                } else {
                    retrieval = retrieval_state_from_engine(engine);
                }
            } else if !semantic_disabled
                && (engine.semantic_doc_count() > 0 || engine.embedding_runtime_configured())
                && let Some(storage_retrieval) = storage_retrieval.as_ref()
            {
                retrieval =
                    retrieval_state_from_engine_with_storage_contract(engine, storage_retrieval);
            }

            let context_storage_retrieval = storage_retrieval
                .clone()
                .unwrap_or_else(|| retrieval.clone());
            let symbol_table_size = engine.symbols().len();
            let hits = hybrid_hits_for_retrieval_state(
                engine,
                HybridHitsContext {
                    req: &req,
                    graph_boosts: &graph_boosts,
                    requested_max_results,
                    request_weights,
                    prefer_primary_sources,
                    storage_retrieval: &context_storage_retrieval,
                    use_exact_symbol_lexical_fast_path,
                },
                &mut retrieval,
            );
            s.last_hybrid_instrumentation = Some(HybridSearchInstrumentation {
                symbol_table_size,
                exact_symbol_merge_queries,
                hybrid_max_results: hybrid_config.max_results,
                hybrid_lexical_limit: hybrid_config.lexical_limit,
                hybrid_semantic_limit: hybrid_config.semantic_limit,
                mixed_natural_language: mixed_natural_language_query(&req.query),
            });
            tracing::info!(
                symbol_table_size,
                exact_symbol_merge_queries,
                hybrid_max_results = hybrid_config.max_results,
                hybrid_lexical_limit = hybrid_config.lexical_limit,
                hybrid_semantic_limit = hybrid_config.semantic_limit,
                mixed_nl = mixed_natural_language_query(&req.query),
                "hybrid_search_instrumentation"
            );

            (hits, s.node_names.clone(), retrieval)
        };

        let mut out = Vec::with_capacity(hybrid.len());
        for scored in hybrid {
            if let Some(mut hit) =
                Self::build_search_hit(&storage, &node_names, scored.node_id, scored.total_score)?
            {
                hit.score_breakdown = Some(RetrievalScoreBreakdownDto {
                    lexical: scored.lexical_score,
                    semantic: scored.semantic_score,
                    graph: scored.graph_score,
                    total: scored.total_score,
                    tier_cap: None,
                    boosts: Vec::new(),
                    dampening: Vec::new(),
                    final_rank_reason: None,
                    provenance: Vec::new(),
                });
                out.push(HybridSearchScoredHit {
                    hit,
                    lexical_score: scored.lexical_score,
                    semantic_score: scored.semantic_score,
                    graph_score: scored.graph_score,
                    total_score: scored.total_score,
                });
            }
        }
        if should_pretruncate_primary_source_window(
            &req.query,
            prefer_primary_sources,
            out.len(),
            requested_max_results,
        ) {
            let top_window_has_non_primary = out
                .iter()
                .take(requested_max_results)
                .any(|scored| is_non_primary_source_hit(&scored.hit));
            if top_window_has_non_primary {
                let primary_count = out
                    .iter()
                    .filter(|scored| !is_non_primary_source_hit(&scored.hit))
                    .count();
                if primary_count >= primary_source_retention_threshold(requested_max_results) {
                    out.retain(|scored| !is_non_primary_source_hit(&scored.hit));
                }
            } else {
                out.truncate(requested_max_results);
            }
        }
        let project_root = self.require_project_root().ok();
        out.sort_by(|left, right| {
            compare_search_hits_with_project_root(
                project_root.as_deref(),
                &req.query,
                &left.hit,
                &right.hit,
            )
        });
        out.truncate(requested_max_results);

        Ok((out, retrieval))
    }
}
