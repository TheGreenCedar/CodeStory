use super::{
    AgentHybridWeightsDto, ApiError, AppController, ExpandedSymbolMatches, HashMap, HashSet,
    NodeId, NodeKind, RetrievalStateDto, SearchHit, SearchPlanSubqueryDto, SearchRequest, Storage,
    aggregate_symbol_matches, extract_symbol_search_terms, node_display_name, preferred_occurrence,
    route_endpoint_adjusted_search_score, symbol_name_match_rank,
};
#[cfg(test)]
use super::{
    EXACT_SYMBOL_HYBRID_MAX_RESULTS_CAP, HybridSearchConfig, HybridSearchHit, RetrievalModeDto,
    SearchEngine, apply_hybrid_limits, compare_search_hits_with_project_root,
    exact_symbol_query_terms, is_non_primary_source_hit, looks_like_standalone_symbol_query,
    mixed_natural_language_query, normalized_hybrid_weights, query_mentions_non_primary_source,
};
#[cfg(test)]
use codestory_contracts::api::RetrievalScoreBreakdownDto;

use crate::agent::packet_evidence::decorate_lexical_search_hit_evidence;
#[cfg(test)]
use crate::agent::packet_evidence::decorate_search_hit_evidence;
use crate::controller_symbols::node_names_for_ids;
#[cfg(test)]
use crate::search_publication::{
    retrieval_state_from_engine, retrieval_state_from_engine_with_storage_contract,
    retrieval_state_from_parts, retrieval_state_from_storage,
};
#[cfg(test)]
use crate::search_state::reload_llm_docs_from_storage;
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

impl HybridSearchScoredHit {
    pub(crate) fn from_search_hit(hit: SearchHit) -> Self {
        let breakdown = hit.score_breakdown.as_ref();
        Self {
            lexical_score: breakdown.map(|scores| scores.lexical).unwrap_or(0.0),
            semantic_score: breakdown.map(|scores| scores.semantic).unwrap_or(0.0),
            graph_score: breakdown.map(|scores| scores.graph).unwrap_or(0.0),
            total_score: breakdown.map(|scores| scores.total).unwrap_or(hit.score),
            hit,
        }
    }
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
    _subquery: &SearchPlanSubqueryDto,
    limit: usize,
) -> usize {
    // The plan's own existence is the breadth gate. Conditioning escalation on
    // the query text turns the condition itself into steering surface, which is
    // how the deleted architecture-intent check earned the holdout prompts a
    // head start.
    limit.saturating_mul(5).clamp(limit, 50)
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

        let hit = SearchHit {
            node_id: NodeId::from(id),
            display_name,
            kind: NodeKind::from(node.kind),
            file_path,
            line,
            score: route_endpoint_adjusted_search_score(score, node.canonical_id.as_deref()),
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            target: None,
            match_quality: None,
            resolvable: true,
            evidence_tier: structural_unit
                .as_ref()
                .map(|_| codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
                .or_else(|| {
                    openapi_endpoint
                        .then_some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource)
                }),
            evidence_producer: structural_unit
                .as_ref()
                .map(|unit| unit.producer.clone())
                .or_else(|| openapi_endpoint.then(|| "openapi_endpoint_schema".to_string())),
            resolution_status: (structural_unit.is_some() || openapi_endpoint)
                .then_some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly),
            loss_reason: None,
            eligible_for_sufficiency: (structural_unit.is_some() || openapi_endpoint)
                .then_some(false),
            source_excerpt: None,
            verification_targets: Vec::new(),
            score_breakdown: None,
        };
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
            .map(|mut hit| {
                decorate_lexical_search_hit_evidence(&mut hit);
                hit
            })
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

        let matches = aggregate_symbol_matches(direct_matches, expanded);
        let node_names = node_names_for_ids(&s.node_names, matches.iter().map(|(id, _)| *id));
        Ok(Some((matches, node_names)))
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
            .map(HybridSearchScoredHit::from_search_hit)
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
            Some(retrieval_state_from_storage(
                &storage,
                &self.require_project_root()?,
            )?)
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

            let node_names = node_names_for_ids(&s.node_names, hits.iter().map(|hit| hit.node_id));
            (hits, node_names, retrieval)
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
                decorate_search_hit_evidence(&mut hit);
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
                None,
            )
        });
        out.truncate(requested_max_results);

        Ok((out, retrieval))
    }
}
