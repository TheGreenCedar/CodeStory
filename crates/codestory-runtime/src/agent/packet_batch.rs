//! Packet batch retrieval orchestration: anchor expansion and planned subqueries.
#![allow(clippy::items_after_test_module)]

use super::citation::to_citation_from_hit;
use super::packet_scoring::{
    normalize_identifier, packet_adjacent_query_stop_term, packet_citation_key,
    packet_citation_rank, packet_query_stop_term, packet_stage_citation_carry_limit,
    packet_subquery_hit_limit,
};
use super::packet_trace::{
    merge_packet_lexical_subquery_batch, merge_packet_semantic_subquery_batch,
};
use super::planning::packet_subquery_hybrid_weights;
use super::trace::field;
use crate::{AppController, clamp_u128_to_u32, query_has_symbol_or_literal_signal};
use codestory_contracts::api::{
    AgentAnswerDto, AgentHybridWeightsDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, ApiError, NodeKind, PacketBudgetLimitsDto, PacketBudgetModeDto,
    PacketPlanDto, PacketPlanQueryDto, SearchHit, SearchHitOrigin, SearchMatchQualityDto,
    SemanticFallbackRecordDto,
};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::Instant;

const DEFAULT_SLA_TARGET_MS: u32 = 18_000;

/// Hybrid weights for lexical-only subqueries that returned no indexed hits.
const PACKET_LEXICAL_MISS_HYBRID_RETRY_WEIGHTS: AgentHybridWeightsDto = AgentHybridWeightsDto {
    lexical: Some(0.35),
    semantic: Some(0.55),
    graph: Some(0.10),
};
#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketLatencyBudget {
    pub(crate) started_at: Instant,
    pub(crate) target_ms: u128,
}

impl PacketLatencyBudget {
    pub(crate) fn new(requested_ms: Option<u32>) -> Self {
        Self {
            started_at: Instant::now(),
            target_ms: requested_ms
                .unwrap_or(DEFAULT_SLA_TARGET_MS)
                .clamp(1_000, 120_000) as u128,
        }
    }

    fn elapsed_ms(&self) -> u128 {
        self.started_at.elapsed().as_millis()
    }

    pub(crate) fn exhausted(&self) -> bool {
        self.elapsed_ms() >= self.target_ms
    }

    pub(crate) fn budget_usage_percent(&self, consumed_trace_ms: u32) -> u128 {
        (consumed_trace_ms as u128)
            .saturating_mul(100)
            .checked_div(self.target_ms.max(1))
            .unwrap_or(100)
    }

    pub(crate) fn apply_to_trace(self, answer: &mut AgentAnswerDto) {
        answer.retrieval_trace.sla_target_ms = Some(clamp_u128_to_u32(self.target_ms));
        if (answer.retrieval_trace.total_latency_ms as u128) > self.target_ms || self.exhausted() {
            answer.retrieval_trace.sla_missed = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_packet_planned_subqueries(
    controller: &AppController,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    include_evidence: bool,
    packet_latency: PacketLatencyBudget,
    rank_terms: &[String],
    answer: &mut AgentAnswerDto,
) -> Result<(), ApiError> {
    let limit = packet_subquery_limit(budget);
    if limit == 0 {
        answer
            .retrieval_trace
            .annotations
            .push("packet_subqueries skipped budget=tiny".to_string());
        return Ok(());
    }

    let pending: Vec<(usize, &PacketPlanQueryDto)> = plan
        .queries
        .iter()
        .enumerate()
        .skip(1)
        .take(limit)
        .collect();
    if pending.is_empty() {
        return Ok(());
    }

    let per_query_limit = packet_subquery_hit_limit(limits);
    let stage_carry_limit = packet_stage_citation_carry_limit(limits);
    let mut lexical_pending = Vec::new();
    let mut semantic_pending = Vec::new();
    for entry in &pending {
        if packet_subquery_is_lexical_only(budget, entry.1) {
            lexical_pending.push(*entry);
        } else {
            semantic_pending.push(*entry);
        }
    }

    let warm_queries = pending
        .iter()
        .map(|(_, query)| query.query.clone())
        .collect::<Vec<_>>();
    if let Err(error) = controller.warm_packet_subquery_embeddings(&warm_queries) {
        answer.retrieval_trace.annotations.push(format!(
            "packet_subquery_embedding_warmup_failed error={:?}",
            error
        ));
    }

    answer.retrieval_trace.annotations.push(format!(
        "packet_subqueries lexical_batch={} semantic={} total={}",
        lexical_pending.len(),
        semantic_pending.len(),
        pending.len()
    ));

    if !lexical_pending.is_empty() {
        let batch = lexical_pending
            .iter()
            .map(|(_, query)| (query.query.clone(), per_query_limit))
            .collect::<Vec<_>>();
        let started_at = Instant::now();
        match controller.search_lexical_hybrid_batch(&batch) {
            Ok(results) => {
                let duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
                answer.retrieval_trace.total_latency_ms = answer
                    .retrieval_trace
                    .total_latency_ms
                    .saturating_add(duration_ms);
                merge_packet_lexical_subquery_batch(
                    answer,
                    &lexical_pending,
                    &results,
                    duration_ms,
                    include_evidence,
                    rank_terms,
                    stage_carry_limit,
                );

                let hybrid_retry_pending: Vec<(usize, &PacketPlanQueryDto)> = lexical_pending
                    .iter()
                    .zip(results.iter())
                    .filter(|((_, query), (_, hits))| {
                        packet_lexical_subquery_needs_hybrid_retry(query, hits.len())
                    })
                    .map(|(entry, _)| *entry)
                    .collect();
                if !hybrid_retry_pending.is_empty() && !packet_latency.exhausted() {
                    answer.retrieval_trace.annotations.push(format!(
                        "packet_lexical_subquery_hybrid_retry count={}",
                        hybrid_retry_pending.len()
                    ));
                    let retry_batch = hybrid_retry_pending
                        .iter()
                        .map(|(_, query)| {
                            (
                                query.query.clone(),
                                per_query_limit,
                                Some(PACKET_LEXICAL_MISS_HYBRID_RETRY_WEIGHTS),
                            )
                        })
                        .collect::<Vec<_>>();
                    let retry_started = Instant::now();
                    match controller.search_semantic_hybrid_batch(&retry_batch) {
                        Ok(outcome) => {
                            let retry_duration_ms =
                                clamp_u128_to_u32(retry_started.elapsed().as_millis());
                            answer.retrieval_trace.total_latency_ms = answer
                                .retrieval_trace
                                .total_latency_ms
                                .saturating_add(retry_duration_ms);
                            record_semantic_fallbacks(answer, &outcome.fallbacks);
                            merge_packet_semantic_subquery_batch(
                                answer,
                                &hybrid_retry_pending,
                                &outcome.results,
                                retry_duration_ms,
                                include_evidence,
                                rank_terms,
                                budget,
                                stage_carry_limit,
                            );
                        }
                        Err(error) => {
                            answer.retrieval_trace.annotations.push(format!(
                                "packet_lexical_subquery_hybrid_retry_failed error={:?}",
                                error
                            ));
                            return Err(error);
                        }
                    }
                }
            }
            Err(error) => {
                answer.retrieval_trace.annotations.push(format!(
                    "packet_lexical_subquery_batch_failed error={:?}",
                    error
                ));
                return Err(error);
            }
        }
    }

    if !semantic_pending.is_empty() {
        if packet_latency.exhausted() {
            answer.retrieval_trace.annotations.push(
                "packet_semantic_subqueries skipped reason=latency_budget_exhausted".to_string(),
            );
        } else {
            let batch = semantic_pending
                .iter()
                .map(|(_, query)| {
                    (
                        query.query.clone(),
                        per_query_limit,
                        packet_subquery_hybrid_weights(budget, query),
                    )
                })
                .collect::<Vec<_>>();
            let started_at = Instant::now();
            match controller.search_semantic_hybrid_batch(&batch) {
                Ok(outcome) => {
                    let duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
                    answer.retrieval_trace.total_latency_ms = answer
                        .retrieval_trace
                        .total_latency_ms
                        .saturating_add(duration_ms);
                    record_semantic_fallbacks(answer, &outcome.fallbacks);
                    merge_packet_semantic_subquery_batch(
                        answer,
                        &semantic_pending,
                        &outcome.results,
                        duration_ms,
                        include_evidence,
                        rank_terms,
                        budget,
                        stage_carry_limit,
                    );
                }
                Err(error) => {
                    for (plan_index, query) in &semantic_pending {
                        answer.retrieval_trace.annotations.push(format!(
                            "packet_semantic_subquery_batch_failed index={} query=`{}` error={:?}",
                            plan_index,
                            query.query.replace('`', "'"),
                            error
                        ));
                    }
                    return Err(error);
                }
            }
        }
    }
    packet_latency.apply_to_trace(answer);
    Ok(())
}

pub(crate) fn record_semantic_fallbacks(
    answer: &mut AgentAnswerDto,
    fallbacks: &[SemanticFallbackRecordDto],
) {
    for fallback in fallbacks {
        answer
            .retrieval_trace
            .semantic_fallbacks
            .push(fallback.clone());
        answer.retrieval_trace.annotations.push(format!(
            "semantic_fallback query=`{}` reason={}",
            fallback.query.replace('`', "'"),
            fallback.reason
        ));
    }
    answer.retrieval_trace.semantic_fallback_count =
        answer.retrieval_trace.semantic_fallbacks.len() as u32;
    if !fallbacks.is_empty() {
        answer.retrieval_trace.annotations.push(format!(
            "semantic_fallback_summary count={} degraded_runtime=true",
            fallbacks.len()
        ));
    }
}

fn packet_subquery_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact => 3,
        PacketBudgetModeDto::Standard => 4,
        PacketBudgetModeDto::Deep => 6,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_packet_anchor_expansion(
    controller: &AppController,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    include_evidence: bool,
    packet_latency: PacketLatencyBudget,
    rank_terms: &[String],
    answer: &mut AgentAnswerDto,
) -> Result<(), ApiError> {
    let consumed_ms = answer.retrieval_trace.total_latency_ms;
    let query_limit = packet_anchor_probe_limit_for_budget(budget, packet_latency, consumed_ms);
    if query_limit == 0 {
        let reason = if packet_anchor_probe_limit(budget) == 0 {
            "budget=tiny"
        } else if packet_latency.exhausted() || consumed_ms as u128 >= packet_latency.target_ms {
            "latency_budget_exhausted"
        } else {
            "reduced_probe_budget"
        };
        answer
            .retrieval_trace
            .annotations
            .push(format!("packet_anchor_probes skipped reason={reason}"));
        if reason == "latency_budget_exhausted" {
            answer.retrieval_trace.sla_missed = true;
        }
        return Ok(());
    }

    let mut citation_keys = answer
        .citations
        .iter()
        .map(packet_citation_key)
        .collect::<HashSet<_>>();
    let per_query_limit = packet_subquery_hit_limit(limits).min(packet_anchor_per_query_limit(
        limits,
        packet_latency,
        consumed_ms,
    ));
    let stage_carry_limit = packet_stage_citation_carry_limit(limits);

    let queries = packet_anchor_probe_queries(plan)
        .into_iter()
        .take(query_limit)
        .collect::<Vec<_>>();
    if queries.is_empty() {
        return Ok(());
    }
    if query_limit < packet_anchor_probe_limit(budget) {
        answer.retrieval_trace.annotations.push(format!(
            "packet_anchor_probes reduced query_limit={query_limit} usage_pct={}",
            packet_latency.budget_usage_percent(consumed_ms)
        ));
    }

    let started_at = Instant::now();
    let result = controller.search_symbolic_packet_anchor_batch(&queries, per_query_limit);
    let duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
    answer.retrieval_trace.total_latency_ms = answer
        .retrieval_trace
        .total_latency_ms
        .saturating_add(duration_ms);
    match result {
        Ok(results) => {
            let per_step_duration = duration_ms / results.len().max(1) as u32;
            for (query, hits) in results {
                let mut added = 0usize;
                let mut citations = hits
                    .iter()
                    .filter(|hit| packet_anchor_hit_is_relevant(&query, hit))
                    .map(|hit| to_citation_from_hit(hit, None, None, include_evidence))
                    .collect::<Vec<_>>();
                citations.sort_by(|left, right| {
                    packet_citation_rank(right, rank_terms, true)
                        .partial_cmp(&packet_citation_rank(left, rank_terms, true))
                        .unwrap_or(Ordering::Equal)
                });
                for citation in citations.into_iter().take(stage_carry_limit) {
                    if citation_keys.insert(packet_citation_key(&citation)) {
                        answer.citations.push(citation);
                        added = added.saturating_add(1);
                    }
                }
                answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Ok,
                    duration_ms: per_step_duration,
                    input: vec![field("query", query.clone())],
                    output: vec![
                        field("hits", hits.len().to_string()),
                        field("accepted_hits", added.to_string()),
                        field("stage_carry_limit", stage_carry_limit.to_string()),
                        field("mode", "symbolic_packet_anchor_probe"),
                    ],
                    message: Some("Packet symbol probe expanded broad task wording.".to_string()),
                });
                answer.retrieval_trace.annotations.push(format!(
                    "packet_anchor_probe query=`{}` hits={} added={}",
                    query.replace('`', "'"),
                    hits.len(),
                    added
                ));
            }
        }
        Err(error) => {
            let message = error.message.clone();
            for query in queries {
                answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Error,
                    duration_ms: 0,
                    input: vec![field("query", query.clone())],
                    output: Vec::new(),
                    message: Some(message.clone()),
                });
                answer.retrieval_trace.annotations.push(format!(
                    "packet_anchor_probe_failed query=`{}` error={}",
                    query.replace('`', "'"),
                    message
                ));
            }
            return Err(error);
        }
    }
    packet_latency.apply_to_trace(answer);
    Ok(())
}

pub(crate) fn packet_anchor_probe_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact => 28,
        PacketBudgetModeDto::Standard => 40,
        PacketBudgetModeDto::Deep => 40,
    }
}

pub(crate) fn packet_anchor_probe_limit_for_budget(
    budget: PacketBudgetModeDto,
    packet_latency: PacketLatencyBudget,
    consumed_trace_ms: u32,
) -> usize {
    let base = packet_anchor_probe_limit(budget);
    if base == 0 {
        return 0;
    }
    if packet_latency.exhausted() || consumed_trace_ms as u128 >= packet_latency.target_ms {
        return 0;
    }
    let usage_pct = packet_latency.budget_usage_percent(consumed_trace_ms);
    if usage_pct >= 75 {
        (base / 4).max(1)
    } else if usage_pct >= 50 {
        (base / 2).max(1)
    } else {
        base
    }
}

fn packet_anchor_per_query_limit(
    limits: &PacketBudgetLimitsDto,
    packet_latency: PacketLatencyBudget,
    consumed_trace_ms: u32,
) -> usize {
    let base = limits.max_anchors.clamp(5, 10) as usize;
    let usage_pct = packet_latency.budget_usage_percent(consumed_trace_ms);
    if usage_pct >= 75 {
        base.min(5)
    } else if usage_pct >= 50 {
        base.min(7)
    } else {
        base
    }
}

pub(crate) fn packet_anchor_probe_queries(plan: &PacketPlanDto) -> Vec<String> {
    let mut ranked = plan
        .queries
        .iter()
        .skip(1)
        .enumerate()
        .filter(|query| {
            let query = query.1;
            query.purpose.contains("symbol probe")
                || query.purpose.contains("concrete symbol")
                || is_packet_code_like_term(&query.query)
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(index, query)| (packet_anchor_probe_priority(query), *index));
    ranked
        .into_iter()
        .map(|(_, query)| query.query.clone())
        .collect()
}

fn packet_anchor_probe_priority(query: &PacketPlanQueryDto) -> u8 {
    if query.purpose.contains("symbol probe") {
        0
    } else if packet_anchor_probe_has_strong_code_shape(&query.query) {
        1
    } else {
        2
    }
}

fn packet_anchor_probe_has_strong_code_shape(query: &str) -> bool {
    let trimmed = query.trim();
    trimmed.contains("::")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('.')
        || trimmed.contains('_')
        || trimmed.contains('-')
        || (trimmed.chars().any(|ch| ch.is_ascii_lowercase())
            && trimmed.chars().skip(1).any(|ch| ch.is_ascii_uppercase()))
}

pub(crate) fn packet_anchor_hit_is_relevant(query: &str, hit: &SearchHit) -> bool {
    if hit.origin != SearchHitOrigin::IndexedSymbol || !hit.resolvable {
        return false;
    }
    if hit.kind == NodeKind::FILE
        && !is_packet_path_like_query(query)
        && !packet_file_stem_matches_query(query, hit.file_path.as_deref())
    {
        return false;
    }
    matches!(
        hit.match_quality,
        Some(
            SearchMatchQualityDto::Exact
                | SearchMatchQualityDto::NormalizedExact
                | SearchMatchQualityDto::Prefix
        )
    ) || hit
        .score_breakdown
        .as_ref()
        .is_some_and(|breakdown| breakdown.lexical >= 0.25 || breakdown.graph >= 0.25)
}

fn is_packet_path_like_query(query: &str) -> bool {
    query.contains('/') || query.contains('\\') || query.contains('.')
}

pub(crate) fn packet_file_stem_matches_query(query: &str, path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return false;
    }
    let normalized_path = path.replace('\\', "/");
    let file_name = normalized_path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim();
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    normalize_identifier(stem) == normalized_query
}

fn packet_subquery_is_lexical_only(
    budget: PacketBudgetModeDto,
    query: &PacketPlanQueryDto,
) -> bool {
    packet_subquery_hybrid_weights(budget, query)
        .and_then(|weights| weights.semantic)
        .is_some_and(|semantic| semantic <= f32::EPSILON)
}

fn packet_lexical_subquery_needs_hybrid_retry(
    query: &PacketPlanQueryDto,
    hit_count: usize,
) -> bool {
    if hit_count > 0 {
        return false;
    }
    let trimmed = query.query.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if packet_query_stop_term(&lowered) || packet_adjacent_query_stop_term(&lowered) {
        return false;
    }
    if trimmed.len() <= 3 {
        return false;
    }
    if query_has_symbol_or_literal_signal(trimmed) {
        return true;
    }
    if is_packet_code_like_term(trimmed) {
        return true;
    }
    if query.purpose.contains("symbol") || query.purpose.contains("flow anchor") {
        return trimmed.len() >= 5
            && !packet_query_stop_term(&lowered)
            && !packet_adjacent_query_stop_term(&lowered);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{PacketPlanDto, PacketPlanQueryDto, PacketTaskClassDto};

    #[test]
    fn packet_lexical_subquery_hybrid_retry_for_empty_symbol_probe() {
        let query = PacketPlanQueryDto {
            query: "dispatchRequest".to_string(),
            purpose: "concrete symbol, file, route, or code term".to_string(),
        };
        assert!(packet_lexical_subquery_needs_hybrid_retry(&query, 0));
        assert!(!packet_lexical_subquery_needs_hybrid_retry(&query, 1));
    }

    #[test]
    fn packet_lexical_subquery_hybrid_retry_for_short_concrete_term() {
        let query = PacketPlanQueryDto {
            query: "HTTP".to_string(),
            purpose: "concrete symbol, file, route, or code term".to_string(),
        };
        assert!(packet_lexical_subquery_needs_hybrid_retry(&query, 0));
    }

    #[test]
    fn packet_lexical_subquery_skips_hybrid_retry_for_generic_concrete_terms() {
        for query in [
            PacketPlanQueryDto {
                query: "Explain".to_string(),
                purpose: "concrete symbol, file, route, or code term".to_string(),
            },
            PacketPlanQueryDto {
                query: "CLI".to_string(),
                purpose: "concrete symbol, file, route, or code term".to_string(),
            },
        ] {
            assert!(
                !packet_lexical_subquery_needs_hybrid_retry(&query, 0),
                "generic term `{}` should not trigger hybrid retry",
                query.query
            );
        }
    }

    #[test]
    fn packet_anchor_probe_queries_prioritize_symbol_probes_under_reduced_windows() {
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: false,
            queries: vec![
                PacketPlanQueryDto {
                    query: "Explain request JSONL flow".to_string(),
                    purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                        .to_string(),
                },
                PacketPlanQueryDto {
                    query: "CLI".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "JSONL".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "EventProcessorWithJsonOutput".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
                PacketPlanQueryDto {
                    query: "ThreadStartParams".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
                PacketPlanQueryDto {
                    query: "exec_events.rs".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
                PacketPlanQueryDto {
                    query: "workspace/app/src/lib.rs".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
            ],
            trace: Vec::new(),
        };

        let queries = packet_anchor_probe_queries(&plan);

        assert_eq!(
            &queries[..4],
            &[
                "EventProcessorWithJsonOutput".to_string(),
                "ThreadStartParams".to_string(),
                "exec_events.rs".to_string(),
                "workspace/app/src/lib.rs".to_string(),
            ]
        );
    }
}

fn is_packet_code_like_term(token: &str) -> bool {
    if token.len() < 3 {
        return false;
    }
    token.contains("::")
        || token.contains('/')
        || token.contains('\\')
        || token.contains('.')
        || token.contains('_')
        || token.contains('-')
        || token.chars().skip(1).any(|ch| ch.is_ascii_uppercase())
}
