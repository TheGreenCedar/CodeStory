//! Packet batch retrieval orchestration: anchor expansion and planned subqueries.
#![allow(clippy::items_after_test_module)]

use super::citation::to_citation_from_hit;
use super::packet_required_probes::packet_sufficiency_required_probe_queries_from_terms;
use super::packet_scoring::{
    normalize_identifier, packet_citation_key, packet_citation_rank,
    packet_stage_citation_carry_limit, packet_subquery_hit_limit,
};
use super::packet_terms::packet_probe_terms;
use super::packet_trace::{
    append_packet_query_timing_fields, merge_packet_fused_subquery_batch, packet_query_diagnostic,
    packet_query_duration_ms,
};
use super::trace::field;
use crate::{AppController, clamp_u128_to_u32, query_has_symbol_or_literal_signal};
use codestory_contracts::api::{
    AgentAnswerDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    ApiError, NodeKind, PacketBudgetLimitsDto, PacketBudgetModeDto, PacketPlanDto,
    PacketPlanQueryDto, PacketSidecarQueryDiagnosticDto, PacketTaskClassDto, SearchHit,
    SearchHitOrigin, SearchMatchQualityDto,
};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::Instant;

const DEFAULT_SLA_TARGET_MS: u32 = 18_000;
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

    pub(crate) fn remaining_ms(&self) -> u32 {
        clamp_u128_to_u32(self.target_ms.saturating_sub(self.elapsed_ms()).max(1_000))
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

    let pending = packet_planned_subqueries(plan, budget, limit);
    if pending.is_empty() {
        return Ok(());
    }

    let per_query_limit = packet_subquery_hit_limit(limits);
    let stage_carry_limit = packet_stage_citation_carry_limit(limits);
    let batch = pending
        .iter()
        .map(|(_, query)| (query.query.clone(), per_query_limit))
        .collect::<Vec<_>>();
    answer.retrieval_trace.annotations.push(format!(
        "packet_subqueries fused_batch={} total={}",
        batch.len(),
        pending.len()
    ));

    let started_at = Instant::now();
    let outcome =
        match controller.search_packet_fused_batch(&batch, Some(packet_latency.remaining_ms())) {
            Ok(outcome) => outcome,
            Err(error) => {
                answer.retrieval_trace.annotations.push(format!(
                    "packet_fused_subquery_batch_failed error={error:?}"
                ));
                return Err(error);
            }
        };
    let duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
    answer.retrieval_trace.total_latency_ms = answer
        .retrieval_trace
        .total_latency_ms
        .saturating_add(duration_ms);
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .extend(outcome.sidecar_diagnostics.clone());
    annotate_packet_batch_timing(
        answer,
        "packet_fused_subquery_batch",
        duration_ms,
        &outcome.sidecar_diagnostics,
    );

    let mut total_duration_ms = duration_ms;
    let mut results = outcome.results;
    let mut effective_diagnostics = outcome.sidecar_diagnostics;
    let retry_pending = packet_fused_retry_pending(&pending, &outcome.retryable_queries);
    if !retry_pending.is_empty() {
        if !packet_fused_retry_is_live() {
            return Err(ApiError::new(
                "cancelled",
                "packet fused retry was cancelled before dispatch",
            ));
        }
        if packet_latency.exhausted() {
            answer.retrieval_trace.annotations.push(format!(
                "packet_fused_blocking_cancel_retry skipped reason=latency_budget_exhausted count={}",
                retry_pending.len()
            ));
        } else {
            answer.retrieval_trace.annotations.push(format!(
                "packet_fused_blocking_cancel_retry count={}",
                retry_pending.len()
            ));
            let retry_batch = retry_pending
                .iter()
                .map(|(_, query)| (query.query.clone(), per_query_limit))
                .collect::<Vec<_>>();
            let retry_started_at = Instant::now();
            let retry_outcome = controller
                .search_packet_fused_batch(&retry_batch, Some(packet_latency.remaining_ms()))
                .map_err(|error| {
                    answer.retrieval_trace.annotations.push(format!(
                        "packet_fused_blocking_cancel_retry_failed error={error:?}"
                    ));
                    error
                })?;
            let retry_duration_ms = clamp_u128_to_u32(retry_started_at.elapsed().as_millis());
            total_duration_ms = total_duration_ms.saturating_add(retry_duration_ms);
            answer.retrieval_trace.total_latency_ms = answer
                .retrieval_trace
                .total_latency_ms
                .saturating_add(retry_duration_ms);
            answer
                .retrieval_trace
                .packet_sidecar_diagnostics
                .extend(retry_outcome.sidecar_diagnostics.clone());
            annotate_packet_batch_timing(
                answer,
                "packet_fused_blocking_cancel_retry_batch",
                retry_duration_ms,
                &retry_outcome.sidecar_diagnostics,
            );
            replace_packet_fused_results(&mut results, retry_outcome.results);
            replace_packet_fused_diagnostics(
                &mut effective_diagnostics,
                retry_outcome.sidecar_diagnostics,
            );
            if !retry_outcome.retryable_queries.is_empty() {
                answer.retrieval_trace.annotations.push(format!(
                    "packet_fused_blocking_cancel_retry exhausted count={}",
                    retry_outcome.retryable_queries.len()
                ));
            }
        }
    }

    merge_packet_fused_subquery_batch(
        answer,
        &pending,
        &results,
        total_duration_ms,
        &effective_diagnostics,
        include_evidence,
        rank_terms,
        stage_carry_limit,
    );
    packet_latency.apply_to_trace(answer);
    Ok(())
}

fn packet_fused_retry_pending<'a>(
    pending: &[(usize, &'a PacketPlanQueryDto)],
    retryable_queries: &[String],
) -> Vec<(usize, &'a PacketPlanQueryDto)> {
    let retryable = retryable_queries
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    pending
        .iter()
        .copied()
        .filter(|(_, query)| retryable.contains(query.query.as_str()))
        .collect()
}

fn packet_fused_retry_is_live() -> bool {
    !crate::services::active_public_operation_cancellation()
        .is_some_and(|cancelled| cancelled.load(AtomicOrdering::Acquire))
}

fn replace_packet_fused_results(
    results: &mut [(String, Vec<SearchHit>)],
    retry_results: Vec<(String, Vec<SearchHit>)>,
) {
    for (retry_query, retry_hits) in retry_results {
        if let Some((_, hits)) = results.iter_mut().find(|(query, _)| *query == retry_query) {
            *hits = retry_hits;
        }
    }
}

fn replace_packet_fused_diagnostics(
    diagnostics: &mut [PacketSidecarQueryDiagnosticDto],
    retry_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
) {
    for retry in retry_diagnostics {
        if let Some(diagnostic) = diagnostics
            .iter_mut()
            .find(|diagnostic| diagnostic.query == retry.query)
        {
            *diagnostic = retry;
        }
    }
}

fn annotate_packet_batch_timing(
    answer: &mut AgentAnswerDto,
    label: &str,
    duration_ms: u32,
    diagnostics: &[PacketSidecarQueryDiagnosticDto],
) {
    let attributed_ms = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.total_elapsed_ms.or(diagnostic.sidecar_query_ms))
        .fold(0_u32, u32::saturating_add);
    let overhead_ms = duration_ms.saturating_sub(attributed_ms);
    let batch_query_wall_ms = diagnostics
        .iter()
        .find_map(|diagnostic| diagnostic.batch_query_wall_ms);
    let batch_wall_note = batch_query_wall_ms
        .map(|ms| format!(" batch_query_wall_ms={ms}"))
        .unwrap_or_default();
    let mut annotation = format!(
        "{label} total_ms={} attributed_query_ms={} overhead_ms={} queries={}",
        duration_ms,
        attributed_ms,
        overhead_ms,
        diagnostics.len()
    );
    annotation.push_str(&batch_wall_note);
    answer.retrieval_trace.annotations.push(annotation);
}

fn packet_anchor_timing_annotation(diagnostic: Option<&PacketSidecarQueryDiagnosticDto>) -> String {
    let Some(diagnostic) = diagnostic else {
        return String::new();
    };
    match (
        diagnostic.sidecar_query_ms,
        diagnostic.candidate_resolution_ms,
        diagnostic.total_elapsed_ms,
        diagnostic.batch_query_wall_ms,
    ) {
        (Some(query_ms), Some(resolution_ms), Some(total_ms), Some(batch_ms)) => format!(
            " sidecar_query_ms={} candidate_resolution_ms={} total_elapsed_ms={} batch_query_wall_ms={}",
            query_ms, resolution_ms, total_ms, batch_ms
        ),
        (Some(query_ms), Some(resolution_ms), Some(total_ms), None) => format!(
            " sidecar_query_ms={} candidate_resolution_ms={} total_elapsed_ms={}",
            query_ms, resolution_ms, total_ms
        ),
        (_, _, Some(total_ms), Some(batch_ms)) => {
            format!(" total_elapsed_ms={total_ms} batch_query_wall_ms={batch_ms}")
        }
        (_, _, Some(total_ms), None) => format!(" total_elapsed_ms={total_ms}"),
        _ => String::new(),
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

fn packet_planned_subqueries(
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limit: usize,
) -> Vec<(usize, &PacketPlanQueryDto)> {
    plan.queries
        .iter()
        .enumerate()
        .skip(1)
        .take(limit)
        .filter(|(_, query)| packet_planned_subquery_should_run(budget, query))
        .collect()
}

fn packet_planned_subquery_should_run(
    budget: PacketBudgetModeDto,
    query: &PacketPlanQueryDto,
) -> bool {
    if !matches!(
        budget,
        PacketBudgetModeDto::Compact | PacketBudgetModeDto::Standard
    ) || !query
        .purpose
        .contains("concrete symbol, file, route, or code term")
    {
        return true;
    }
    let trimmed = query.query.trim();
    query_has_symbol_or_literal_signal(trimmed) || is_packet_code_like_term(trimmed)
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
    let batch = queries
        .iter()
        .map(|query| (query.clone(), per_query_limit))
        .collect::<Vec<_>>();
    let result = controller.search_packet_fused_batch(&batch, Some(packet_latency.remaining_ms()));
    let duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
    answer.retrieval_trace.total_latency_ms = answer
        .retrieval_trace
        .total_latency_ms
        .saturating_add(duration_ms);
    match result {
        Ok(outcome) => {
            answer
                .retrieval_trace
                .packet_sidecar_diagnostics
                .extend(outcome.sidecar_diagnostics.clone());
            let diagnostics = outcome.sidecar_diagnostics;
            annotate_packet_batch_timing(
                answer,
                "packet_anchor_probe_batch",
                duration_ms,
                &diagnostics,
            );
            let results = outcome.results;
            let per_step_duration = duration_ms / results.len().max(1) as u32;
            for (diagnostic_index, (query, hits)) in results.into_iter().enumerate() {
                let diagnostic = packet_query_diagnostic(&diagnostics, diagnostic_index, &query);
                let step_duration =
                    packet_query_duration_ms(diagnostic).unwrap_or(per_step_duration);
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
                let mut output = vec![
                    field("hits", hits.len().to_string()),
                    field("accepted_hits", added.to_string()),
                    field("stage_carry_limit", stage_carry_limit.to_string()),
                    field("mode", "symbolic_packet_anchor_probe"),
                ];
                append_packet_query_timing_fields(&mut output, diagnostic);
                answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Ok,
                    duration_ms: step_duration,
                    input: vec![field("query", query.clone())],
                    output,
                    message: Some("Packet symbol probe expanded broad task wording.".to_string()),
                });
                let timing_note = packet_anchor_timing_annotation(diagnostic);
                answer.retrieval_trace.annotations.push(format!(
                    "packet_anchor_probe query=`{}` hits={} added={}{}",
                    query.replace('`', "'"),
                    hits.len(),
                    added,
                    timing_note
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
        PacketBudgetModeDto::Compact => 12,
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
    } else if usage_pct >= 50 || (budget == PacketBudgetModeDto::Compact && usage_pct >= 25) {
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
    let required_probes = packet_anchor_required_probe_keys(plan);
    let mut ranked = plan
        .queries
        .iter()
        .skip(1)
        .enumerate()
        .filter(|query| {
            let query = query.1;
            query.purpose.contains("symbol probe")
                || packet_task_seed_anchor_probe(&query.query)
                || query.purpose.contains("concrete symbol")
                || is_packet_code_like_term(&query.query)
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(index, query)| {
        (
            !required_probes.contains(&normalize_identifier(&query.query)),
            packet_anchor_probe_priority(query),
            *index,
        )
    });
    let mut seen = HashSet::<String>::new();
    let mut queries = ranked
        .into_iter()
        .filter_map(|(_, query)| {
            if is_packet_path_like_query(&query.query) {
                return Some(query.query.clone());
            }
            let key = normalize_identifier(&query.query);
            if key.len() < 2 || seen.insert(key) {
                Some(query.query.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    reserve_architecture_main_anchor_probe(plan, &required_probes, &mut queries);
    queries
}

fn reserve_architecture_main_anchor_probe(
    plan: &PacketPlanDto,
    required_probes: &HashSet<String>,
    queries: &mut Vec<String>,
) {
    if plan.task_class != PacketTaskClassDto::ArchitectureExplanation
        || !required_probes.contains("searchentrypoint")
    {
        return;
    }
    queries.retain(|query| normalize_identifier(query) != "main");
    let insert_at = queries
        .iter()
        .take_while(|query| required_probes.contains(&normalize_identifier(query)))
        .count();
    queries.insert(insert_at, "main".to_string());
}

fn packet_anchor_required_probe_keys(plan: &PacketPlanDto) -> HashSet<String> {
    let Some(prompt) = plan.queries.first() else {
        return HashSet::new();
    };
    let terms = packet_probe_terms(&prompt.query);
    packet_sufficiency_required_probe_queries_from_terms(&terms, plan.task_class)
        .into_iter()
        .map(|query| normalize_identifier(&query))
        .filter(|query| !query.is_empty())
        .collect()
}

fn packet_anchor_probe_priority(query: &PacketPlanQueryDto) -> u8 {
    if query.purpose.contains("symbol probe") {
        0
    } else if packet_task_seed_anchor_probe(&query.query) {
        1
    } else if packet_anchor_probe_has_strong_code_shape(&query.query) {
        2
    } else {
        3
    }
}

fn packet_task_seed_anchor_probe(query: &str) -> bool {
    matches!(
        normalize_identifier(query).as_str(),
        "main" | "run" | "entrypoint"
    )
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
    let query_path = query.replace('\\', "/");
    let query_file_name = query_path.rsplit('/').next().unwrap_or(query).trim();
    let query_stem = query_file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(query_file_name);
    let normalized_query = normalize_identifier(query_stem);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_plan::build_packet_plan;
    use codestory_contracts::api::{PacketPlanDto, PacketPlanQueryDto, PacketTaskClassDto};

    #[test]
    fn packet_latency_budget_preserves_advertised_range_and_default() {
        assert_eq!(PacketLatencyBudget::new(None).target_ms, 18_000);
        assert_eq!(PacketLatencyBudget::new(Some(10)).target_ms, 1_000);
        assert_eq!(PacketLatencyBudget::new(Some(120_001)).target_ms, 120_000);
        assert!(PacketLatencyBudget::new(Some(1_000)).remaining_ms() >= 1_000);
    }

    #[test]
    fn packet_fused_retry_uses_only_reported_blocking_deadlines() {
        let first = PacketPlanQueryDto {
            query: "ordinary empty".to_string(),
            purpose: "supplemental".to_string(),
        };
        let second = PacketPlanQueryDto {
            query: "timed out".to_string(),
            purpose: "required flow anchor".to_string(),
        };
        let pending = vec![(1, &first), (2, &second)];

        assert!(packet_fused_retry_pending(&pending, &[]).is_empty());
        let retry = packet_fused_retry_pending(&pending, &["timed out".to_string()]);
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].0, 2);
        assert_eq!(retry[0].1.query, "timed out");
    }

    #[test]
    fn packet_planned_subqueries_skip_low_signal_lexical_concrete_terms() {
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::RouteTracing,
            inferred_task_class: false,
            queries: vec![
                PacketPlanQueryDto {
                    query: "Trace how Redis initializes command routing".to_string(),
                    purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                        .to_string(),
                },
                PacketPlanQueryDto {
                    query: "Trace".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "Redis".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "initializes".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
            ],
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        };

        let pending = packet_planned_subqueries(&plan, PacketBudgetModeDto::Compact, 3);

        assert!(pending.is_empty());
    }

    #[test]
    fn packet_planned_subqueries_keep_code_like_terms_and_deep_semantic_terms() {
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: false,
            queries: vec![
                PacketPlanQueryDto {
                    query: "Explain string predicate helpers".to_string(),
                    purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                        .to_string(),
                },
                PacketPlanQueryDto {
                    query: "StringUtils".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "CharSequenceUtils".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "Commons".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
            ],
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        };

        let compact = packet_planned_subqueries(&plan, PacketBudgetModeDto::Compact, 3)
            .into_iter()
            .map(|(_, query)| query.query.as_str())
            .collect::<Vec<_>>();
        let deep = packet_planned_subqueries(&plan, PacketBudgetModeDto::Deep, 3)
            .into_iter()
            .map(|(_, query)| query.query.as_str())
            .collect::<Vec<_>>();

        assert_eq!(compact, ["StringUtils", "CharSequenceUtils"]);
        assert_eq!(deep, ["StringUtils", "CharSequenceUtils", "Commons"]);
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
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
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

    #[test]
    fn packet_anchor_probe_queries_count_normalized_variants_once() {
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: false,
            queries: vec![
                PacketPlanQueryDto {
                    query: "Explain predicate helpers".to_string(),
                    purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                        .to_string(),
                },
                PacketPlanQueryDto {
                    query: "isBlank".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
                PacketPlanQueryDto {
                    query: "is_blank".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
                PacketPlanQueryDto {
                    query: "StringUtils.java isBlank".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
            ],
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        };

        let queries = packet_anchor_probe_queries(&plan);

        assert_eq!(
            queries,
            [
                "isBlank".to_string(),
                "StringUtils.java isBlank".to_string()
            ]
        );
    }

    #[test]
    fn packet_anchor_probe_queries_keep_path_like_normalized_matches() {
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: false,
            queries: vec![
                PacketPlanQueryDto {
                    query: "Explain library entrypoints".to_string(),
                    purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                        .to_string(),
                },
                PacketPlanQueryDto {
                    query: "src/lib.rs".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
                PacketPlanQueryDto {
                    query: "src_lib_rs".to_string(),
                    purpose: "symbol probe expanded from task wording".to_string(),
                },
            ],
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        };

        let queries = packet_anchor_probe_queries(&plan);

        assert_eq!(
            queries,
            ["src/lib.rs".to_string(), "src_lib_rs".to_string()]
        );
    }

    #[test]
    fn compact_packet_anchor_probe_limit_stays_bounded() {
        assert_eq!(packet_anchor_probe_limit(PacketBudgetModeDto::Compact), 12);
        assert_eq!(
            packet_anchor_probe_limit_for_budget(
                PacketBudgetModeDto::Compact,
                PacketLatencyBudget::new(None),
                0,
            ),
            12
        );
    }

    #[test]
    fn compact_packet_anchor_probe_limit_tapers_under_budget_pressure() {
        let latency = PacketLatencyBudget::new(Some(18_000));
        assert_eq!(
            packet_anchor_probe_limit_for_budget(PacketBudgetModeDto::Compact, latency, 4_500,),
            6
        );
        assert_eq!(
            packet_anchor_probe_limit_for_budget(PacketBudgetModeDto::Compact, latency, 9_000,),
            6
        );
        assert_eq!(
            packet_anchor_probe_limit_for_budget(PacketBudgetModeDto::Compact, latency, 13_500,),
            3
        );
    }

    #[test]
    fn packet_anchor_probe_queries_execute_entrypoint_seed_queries() {
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: false,
            queries: vec![
                PacketPlanQueryDto {
                    query: "Explain the runtime flow".to_string(),
                    purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                        .to_string(),
                },
                PacketPlanQueryDto {
                    query: "architecture entrypoint".to_string(),
                    purpose: "task-class retrieval seed".to_string(),
                },
                PacketPlanQueryDto {
                    query: "main".to_string(),
                    purpose: "task-class retrieval seed".to_string(),
                },
                PacketPlanQueryDto {
                    query: "run".to_string(),
                    purpose: "task-class retrieval seed".to_string(),
                },
                PacketPlanQueryDto {
                    query: "entrypoint".to_string(),
                    purpose: "task-class retrieval seed".to_string(),
                },
            ],
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        };

        let queries = packet_anchor_probe_queries(&plan);

        assert!(queries.contains(&"main".to_string()));
        assert!(queries.contains(&"run".to_string()));
        assert!(queries.contains(&"entrypoint".to_string()));
        assert!(!queries.contains(&"architecture entrypoint".to_string()));
    }

    #[test]
    fn compact_search_flow_reserves_main_anchor_query() {
        let plan = build_packet_plan(
            "Explain how a search command parses CLI flags, walks candidate inputs, and executes sequential or parallel searches through matcher, searcher, and printer components.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
            PacketBudgetModeDto::Compact,
        );

        assert_eq!(
            plan.queries.len(),
            32,
            "fixture should exercise the plan cap"
        );
        let selected = packet_anchor_probe_queries(&plan)
            .into_iter()
            .take(packet_anchor_probe_limit(PacketBudgetModeDto::Compact))
            .collect::<Vec<_>>();

        assert!(selected.iter().any(|query| query == "main"));
    }

    #[test]
    fn compact_flow_anchor_window_prioritizes_roles_over_generated_variants() {
        let mut queries = vec![PacketPlanQueryDto {
            query: "Explain the command execution flow".to_string(),
            purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                .to_string(),
        }];
        for query in [
            "execution entrypoint",
            "dispatch boundary",
            "result rendering",
        ] {
            queries.push(PacketPlanQueryDto {
                query: query.to_string(),
                purpose: "symbol probe expanded from task wording".to_string(),
            });
        }
        for index in 0..15 {
            queries.push(PacketPlanQueryDto {
                query: format!("GeneratedVariant{index}"),
                purpose: "symbol probe expanded from task wording".to_string(),
            });
        }
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: false,
            queries,
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        };

        let selected = packet_anchor_probe_queries(&plan)
            .into_iter()
            .take(packet_anchor_probe_limit(PacketBudgetModeDto::Compact))
            .collect::<Vec<_>>();

        assert_eq!(selected.len(), 12);
        assert!(selected.starts_with(&[
            "execution entrypoint".to_string(),
            "dispatch boundary".to_string(),
            "result rendering".to_string(),
        ]));
        assert!(!selected.contains(&"GeneratedVariant14".to_string()));
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
