//! Packet batch retrieval orchestration for missing material proof.
#![allow(clippy::items_after_test_module)]

use super::packet_candidate::PacketSearchHit;
use super::packet_plan::packet_plan_query_is_typed_free_query;
use super::packet_scoring::{
    normalize_identifier, packet_stage_citation_carry_limit, packet_subquery_hit_limit,
};
use super::packet_trace::merge_packet_fused_subquery_batch;
use crate::{AppController, clamp_u128_to_u32};
use codestory_contracts::api::{
    AgentAnswerDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, ApiError,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto,
    PacketSidecarQueryDiagnosticDto, RetrievalAnnotationDto,
};
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
    answer: &mut AgentAnswerDto,
) -> Result<(), ApiError> {
    let limit = packet_subquery_limit(budget);
    if limit == 0 {
        // Planned subqueries never ran, so their evidence is genuinely absent.
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::gap(
                "packet_subqueries skipped budget=tiny",
            ));
        return Ok(());
    }

    let adaptive_queries = packet_free_queries(plan, answer, limit);
    let pending = adaptive_queries
        .iter()
        .enumerate()
        .map(|(index, query)| (plan.queries.len().saturating_add(index), query))
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }
    if packet_latency.exhausted() {
        answer.retrieval_trace.sla_missed = true;
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::gap(format!(
                "packet_material_queries skipped reason=latency_budget_exhausted count={}",
                pending.len()
            )));
        return Ok(());
    }

    let per_query_limit = packet_subquery_hit_limit(limits);
    let stage_carry_limit = packet_stage_citation_carry_limit(limits);
    let batch = pending
        .iter()
        .map(|(_, query)| (query.query.clone(), per_query_limit))
        .collect::<Vec<_>>();
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(format!(
            "packet_material_queries fused_batch={} total={}",
            batch.len(),
            pending.len()
        )));

    let started_at = Instant::now();
    let outcome =
        match controller.search_packet_fused_batch(&batch, Some(packet_latency.remaining_ms())) {
            Ok(outcome) => outcome,
            Err(error) => {
                answer
                    .retrieval_trace
                    .annotations
                    .push(RetrievalAnnotationDto::gap(format!(
                        "packet_fused_subquery_batch_failed error={error:?}"
                    )));
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
            // The retry never ran, so those queries contributed no evidence.
            answer
                .retrieval_trace
                .annotations
                .push(RetrievalAnnotationDto::gap(format!(
                    "packet_fused_blocking_cancel_retry skipped reason=latency_budget_exhausted count={}",
                    retry_pending.len()
                )));
        } else {
            answer
                .retrieval_trace
                .annotations
                .push(RetrievalAnnotationDto::observation(format!(
                    "packet_fused_blocking_cancel_retry count={}",
                    retry_pending.len()
                )));
            let retry_batch = retry_pending
                .iter()
                .map(|(_, query)| (query.query.clone(), per_query_limit))
                .collect::<Vec<_>>();
            let retry_started_at = Instant::now();
            let retry_outcome = controller
                .search_packet_fused_batch(&retry_batch, Some(packet_latency.remaining_ms()))
                .map_err(|error| {
                    answer
                        .retrieval_trace
                        .annotations
                        .push(RetrievalAnnotationDto::gap(format!(
                            "packet_fused_blocking_cancel_retry_failed error={error:?}"
                        )));
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
                // Retries were exhausted with queries still unresolved: their evidence is missing.
                answer
                    .retrieval_trace
                    .annotations
                    .push(RetrievalAnnotationDto::gap(format!(
                        "packet_fused_blocking_cancel_retry exhausted count={}",
                        retry_outcome.retryable_queries.len()
                    )));
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
    results: &mut [(String, Vec<PacketSearchHit>)],
    retry_results: Vec<(String, Vec<PacketSearchHit>)>,
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
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(annotation));
}

fn packet_subquery_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact
        | PacketBudgetModeDto::Standard
        | PacketBudgetModeDto::Deep => 16,
    }
}

fn packet_free_queries(
    plan: &PacketPlanDto,
    answer: &AgentAnswerDto,
    limit: usize,
) -> Vec<PacketPlanQueryDto> {
    let mut seen = HashSet::new();
    plan.queries
        .iter()
        .filter(|query| packet_plan_query_is_typed_free_query(query))
        .filter(|query| !packet_query_completed(answer, &query.query))
        .filter(|query| {
            let key = normalize_identifier(&query.query);
            key.is_empty() || seen.insert(key)
        })
        .take(limit)
        .cloned()
        .collect()
}

fn packet_query_completed(answer: &AgentAnswerDto, query: &str) -> bool {
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .iter()
        .rev()
        .find(|diagnostic| diagnostic.query == query)
        .is_some_and(|diagnostic| {
            diagnostic.completion == codestory_contracts::api::PacketQueryCompletionDto::Completed
        })
        || answer.retrieval_trace.steps.iter().rev().any(|step| {
            step.kind == AgentRetrievalStepKindDto::Search
                && step.status == AgentRetrievalStepStatusDto::Ok
                && step
                    .input
                    .iter()
                    .any(|field| field.key == "query" && field.value == query)
        })
}
