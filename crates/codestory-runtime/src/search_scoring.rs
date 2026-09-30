use super::{
    AgentHybridWeightsDto, ApiError, AppController, ExpandedSymbolMatches, HashMap, HashSet,
    NodeId, NodeKind, RetrievalStateDto, SearchHit, SearchPlanSubqueryDto, SearchRequest, Storage,
    aggregate_symbol_matches, extract_symbol_search_terms, node_display_name, preferred_occurrence,
    route_endpoint_adjusted_search_score, symbol_name_match_rank,
};

use crate::agent::packet_evidence::decorate_lexical_search_hit_evidence;
use crate::controller_symbols::node_names_for_ids;

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

        let mut file_path = if node.kind == codestory_contracts::graph::NodeKind::FILE {
            // FILE nodes have no parent file_node_id. Resolve their location by
            // the same pinned file identity used by the core publication.
            storage
                .get_file_by_id(id.0)
                .map_err(|error| {
                    ApiError::internal(format!("Failed to load indexed file identity: {error}"))
                })?
                .map(|file| file.path.to_string_lossy().into_owned())
        } else {
            Self::file_path_for_node(storage, &node).ok().flatten()
        };
        let mut line = node.start_line;
        if let Ok(occs) = storage.get_occurrences_for_node(id)
            && let Some(occ) = preferred_occurrence(&occs)
        {
            if file_path.is_none()
                && node.kind != codestory_contracts::graph::NodeKind::FILE
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
        self.ensure_search_state()?;
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
}
