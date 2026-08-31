//! Packet batch retrieval orchestration for missing material proof.
#![allow(clippy::items_after_test_module)]

use super::packet_candidate::PacketSearchHit;
#[cfg(test)]
use super::packet_candidate::merge_packet_candidate_graph;
use super::packet_required_probes::packet_sufficiency_required_probe_queries_from_terms;
use super::packet_scoring::{
    normalize_identifier, packet_stage_citation_carry_limit, packet_subquery_hit_limit,
};
#[cfg(test)]
use super::packet_scoring::{packet_citation_key, packet_citation_rank, sort_by_cached_rank_desc};
use super::packet_terms::packet_probe_terms;
use super::packet_trace::merge_packet_fused_subquery_batch;
#[cfg(test)]
use super::packet_trace::{
    append_packet_query_timing_fields, packet_query_diagnostic, packet_query_duration_ms,
};
#[cfg(test)]
use super::trace::field;
use crate::{AppController, clamp_u128_to_u32};
use codestory_agent::packet_obligations::{
    PacketProofEvidenceExtras, preview_packet_obligation_plan_before_budget,
};
use codestory_agent::packet_plan::packet_owner_member_probe_queries;
pub(crate) use codestory_agent::packet_scoring::packet_file_stem_matches_query;
use codestory_agent::planning::{
    PACKET_ADJACENT_VARIANT_QUERY_PURPOSE, PACKET_CONCRETE_FILE_QUERY_PURPOSE,
    PACKET_FLOW_ROLE_QUERY_PURPOSE, PACKET_GENERIC_TERM_QUERY_PURPOSE,
    PACKET_OWNER_MEMBER_QUERY_PURPOSE, packet_plan_query_is_exact_symbol_identity,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, ApiError,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto,
    PacketSidecarQueryDiagnosticDto, PacketTaskClassDto, RetrievalAnnotationDto,
};
#[cfg(test)]
use codestory_contracts::api::{
    AgentRetrievalStepDto, NodeKind, SearchHit, SearchHitOrigin, SearchMatchQualityDto,
};
use std::collections::HashSet;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::Instant;

const DEFAULT_SLA_TARGET_MS: u32 = 18_000;
const PACKET_OWNER_MEMBER_QUERY_LIMIT: usize = 4;
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

    #[cfg(test)]
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
    question: &str,
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
        // Planned subqueries never ran, so their evidence is genuinely absent.
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::gap(
                "packet_subqueries skipped budget=tiny",
            ));
        return Ok(());
    }

    let adaptive_queries = packet_adaptive_material_queries(question, plan, answer, limit);
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
        rank_terms,
        stage_carry_limit,
        &Vec::new(),
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

#[cfg(test)]
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
        PacketBudgetModeDto::Compact
        | PacketBudgetModeDto::Standard
        | PacketBudgetModeDto::Deep => 16,
    }
}

fn packet_adaptive_material_queries(
    question: &str,
    plan: &PacketPlanDto,
    answer: &AgentAnswerDto,
    limit: usize,
) -> Vec<PacketPlanQueryDto> {
    // Pre-cap preview proving; stage 4 threads the runtime's real evidence
    // extras here alongside the other proving sites.
    let preview = preview_packet_obligation_plan_before_budget(
        question,
        plan.task_class,
        &plan.obligations,
        answer,
        &PacketProofEvidenceExtras::default(),
    );
    let mut queries = Vec::new();
    let mut seen = HashSet::<String>::new();

    let mut push = |query: &str, purpose: String| {
        let query = query.trim();
        let key = normalize_identifier(query);
        if query.is_empty()
            || packet_query_completed(answer, query)
            || (!key.is_empty() && !seen.insert(key))
            || queries.len() >= limit
        {
            return false;
        }
        queries.push(PacketPlanQueryDto {
            query: query.to_string(),
            purpose,
        });
        true
    };

    let missing_material = preview
        .claim_obligations
        .iter()
        .filter(|obligation| {
            obligation.material
                && obligation.proof_status
                    != codestory_contracts::api::PacketObligationProofStatusDto::Proven
        })
        .collect::<Vec<_>>();
    let mut obligation_added_query = vec![false; missing_material.len()];

    let structural_schema_flow = missing_material
        .iter()
        .any(|obligation| obligation.id.starts_with("sql_"));
    let owner_member_queries = if !missing_material.is_empty() && !structural_schema_flow {
        packet_owner_member_probe_queries(
            question,
            &answer.citations,
            limit.min(PACKET_OWNER_MEMBER_QUERY_LIMIT),
        )
    } else {
        Vec::new()
    };

    // Reserve the bounded owner/member slice, then spread the rest across open claims before
    // considering any claim's fallback paths.
    let first_material_query_limit = limit.saturating_sub(owner_member_queries.len());
    for (index, obligation) in missing_material
        .iter()
        .enumerate()
        .take(first_material_query_limit)
    {
        if let Some(query) = obligation.open_next_candidates.first() {
            obligation_added_query[index] |=
                push(query, format!("material obligation {}", obligation.id));
        }
    }

    for query in owner_member_queries {
        let _ = push(&query, PACKET_OWNER_MEMBER_QUERY_PURPOSE.to_string());
    }

    for (index, obligation) in missing_material.iter().enumerate() {
        for query in obligation.open_next_candidates.iter().skip(1) {
            obligation_added_query[index] |=
                push(query, format!("material obligation {}", obligation.id));
        }
    }

    for obligation in plan
        .obligations
        .query_obligations
        .iter()
        .filter(|obligation| obligation.material)
    {
        let _ = push(
            &obligation.query,
            format!("material query obligation {}", obligation.id),
        );
    }

    for (index, obligation) in missing_material.iter().enumerate() {
        for query in &obligation.carrier_paths {
            obligation_added_query[index] |=
                push(query, format!("material obligation {}", obligation.id));
        }
    }

    let missing_material_without_query = obligation_added_query.iter().any(|added| !added);
    if missing_material_without_query {
        for query in packet_anchor_probe_queries(plan) {
            let _ = push(&query, "unresolved material behavior anchor".to_string());
        }
    }

    queries
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

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
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
        // Anchor probes never dispatched, so their evidence is genuinely absent.
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::gap(format!(
                "packet_anchor_probes skipped reason={reason}"
            )));
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
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_anchor_probes reduced query_limit={query_limit} usage_pct={}",
                packet_latency.budget_usage_percent(consumed_ms)
            )));
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
                    .map(|hit| (hit.citation(include_evidence), hit))
                    .collect::<Vec<_>>();
                sort_by_cached_rank_desc(&mut citations, |(citation, _)| {
                    packet_citation_rank(citation, rank_terms, true)
                });
                for (citation, hit) in citations.into_iter().take(stage_carry_limit) {
                    if include_evidence {
                        merge_packet_candidate_graph(answer, hit);
                    }
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
                // Echoes prompt-derived probe text: telemetry about the run, not a gap.
                answer
                    .retrieval_trace
                    .annotations
                    .push(RetrievalAnnotationDto::observation(format!(
                        "packet_anchor_probe query=`{}` hits={} added={}{}",
                        query.replace('`', "'"),
                        hits.len(),
                        added,
                        timing_note
                    )));
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
                answer
                    .retrieval_trace
                    .annotations
                    .push(RetrievalAnnotationDto::gap(format!(
                        "packet_anchor_probe_failed query=`{}` error={}",
                        query.replace('`', "'"),
                        message
                    )));
            }
            return Err(error);
        }
    }
    packet_latency.apply_to_trace(answer);
    Ok(())
}

#[cfg(test)]
pub(crate) fn packet_anchor_probe_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact => 12,
        PacketBudgetModeDto::Standard => 40,
        PacketBudgetModeDto::Deep => 40,
    }
}

#[cfg(test)]
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

#[cfg(test)]
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
            !packet_anchor_probe_is_instruction_noise(query)
                && (query.purpose.contains("symbol probe")
                    || packet_task_seed_anchor_probe(&query.query)
                    || query.purpose.contains("concrete symbol")
                    || is_packet_code_like_term(&query.query))
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
    if packet_plan_query_is_exact_symbol_identity(query) {
        0
    } else if matches!(
        query.purpose.as_str(),
        PACKET_FLOW_ROLE_QUERY_PURPOSE | PACKET_CONCRETE_FILE_QUERY_PURPOSE
    ) || (packet_anchor_probe_has_strong_code_shape(&query.query)
        && !matches!(
            query.purpose.as_str(),
            PACKET_ADJACENT_VARIANT_QUERY_PURPOSE | PACKET_GENERIC_TERM_QUERY_PURPOSE
        ))
    {
        1
    } else if query.purpose.contains("concrete symbol") {
        2
    } else if packet_task_seed_anchor_probe(&query.query) {
        3
    } else if matches!(
        query.purpose.as_str(),
        PACKET_ADJACENT_VARIANT_QUERY_PURPOSE | PACKET_GENERIC_TERM_QUERY_PURPOSE
    ) {
        5
    } else {
        4
    }
}

fn packet_anchor_probe_is_instruction_noise(query: &PacketPlanQueryDto) -> bool {
    if packet_plan_query_is_exact_symbol_identity(query)
        || packet_anchor_probe_has_strong_code_shape(&query.query)
    {
        return false;
    }
    matches!(
        normalize_identifier(&query.query).as_str(),
        "answer"
            | "cite"
            | "cites"
            | "explain"
            | "file"
            | "files"
            | "name"
            | "names"
            | "source"
            | "sources"
            | "supporting"
            | "symbol"
            | "symbols"
            | "trace"
    )
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

#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto,
        PacketPlanDto, PacketPlanQueryDto, PacketTaskClassDto, RetrievalAnnotationKindDto,
    };

    /// EV-6c (#1775) helpers. Every gap producer in this module writes onto the answer's
    /// retrieval trace, so the tests below drive the real production entry points
    /// (`run_packet_planned_subqueries`, `run_packet_anchor_probes`) and read back the kind the
    /// producer stamped. Nothing here hand-builds an annotation.
    fn empty_answer() -> AgentAnswerDto {
        AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "ev6c".to_string(),
            prompt: "ev6c packet".to_string(),
            summary: String::new(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "ev6c".to_string(),
                retrieval_publication: None,
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 0,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                semantic_stage_timeout_zero_hits: 0,
                semantic_abstained_count: 0,
                annotations: Vec::new(),
                packet_claim_profile_telemetry: None,
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    fn anchor_citation(display_name: &str) -> codestory_contracts::api::AgentCitationDto {
        codestory_contracts::api::AgentCitationDto {
            node_id: codestory_contracts::api::NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/site.rb".to_string()),
            line: Some(1),
            score: 1.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            source_excerpt: None,
        }
    }

    fn ev6c_limits() -> PacketBudgetLimitsDto {
        PacketBudgetLimitsDto {
            max_anchors: 8,
            max_files: 8,
            max_snippets: 8,
            max_trail_edges: 8,
            max_output_bytes: 64_000,
        }
    }

    fn ev6c_plan() -> PacketPlanDto {
        let question = "Trace how StringUtils normalizes request routes";
        let task_class = PacketTaskClassDto::RouteTracing;
        let queries = vec![
            PacketPlanQueryDto {
                query: question.to_string(),
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
        ];
        PacketPlanDto {
            task_class,
            inferred_task_class: false,
            obligations: codestory_agent::packet_obligations::build_packet_obligation_plan(
                question, task_class, &queries,
            ),
            queries,
            probe_resolutions: Vec::new(),
            trace: Vec::new(),
        }
    }

    /// Every annotation the run produced, as `(kind, text)`, in emission order.
    fn classified(answer: &AgentAnswerDto) -> Vec<(RetrievalAnnotationKindDto, String)> {
        answer
            .retrieval_trace
            .annotations
            .iter()
            .map(|annotation| (annotation.kind, annotation.text.clone()))
            .collect()
    }

    fn kind_of(answer: &AgentAnswerDto, prefix: &str) -> RetrievalAnnotationKindDto {
        let matches = answer
            .retrieval_trace
            .annotations
            .iter()
            .filter(|annotation| annotation.text.starts_with(prefix))
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one annotation starting with `{prefix}`, got {:?}",
            classified(answer)
        );
        matches[0].kind
    }

    #[test]
    fn packet_subqueries_skipped_by_budget_is_published_as_an_evidence_gap() {
        // EV-6c (#1775). A tiny budget means the planned subqueries never ran, so the evidence
        // they would have produced is genuinely absent. Reclassifying this producer as an
        // observation would leave `agent_confidence` at high for a packet that skipped its
        // supplemental retrieval outright.
        let mut answer = empty_answer();
        run_packet_planned_subqueries(
            &AppController::new(),
            "Trace how StringUtils normalizes request routes",
            &ev6c_plan(),
            PacketBudgetModeDto::Tiny,
            &ev6c_limits(),
            false,
            PacketLatencyBudget::new(Some(120_000)),
            &[],
            &mut answer,
        )
        .expect("skipping subqueries on a tiny budget is not an error");

        assert_eq!(
            classified(&answer),
            vec![(
                RetrievalAnnotationKindDto::Gap,
                "packet_subqueries skipped budget=tiny".to_string()
            )],
            "the skipped-subquery producer must publish an evidence gap"
        );
    }

    #[test]
    fn material_queries_dropped_for_latency_are_published_as_an_evidence_gap() {
        // The material queries were planned but never dispatched, so the packet must report
        // missing evidence and the missed SLA instead of retaining clean confidence.
        let mut answer = empty_answer();
        let packet_latency = PacketLatencyBudget {
            started_at: Instant::now() - std::time::Duration::from_secs(2),
            target_ms: 1_000,
        };
        run_packet_planned_subqueries(
            &AppController::new(),
            "Trace how StringUtils normalizes request routes",
            &ev6c_plan(),
            PacketBudgetModeDto::Compact,
            &ev6c_limits(),
            false,
            packet_latency,
            &[],
            &mut answer,
        )
        .expect("dropping material queries for latency is not an execution error");

        assert_eq!(
            kind_of(
                &answer,
                "packet_material_queries skipped reason=latency_budget_exhausted count="
            ),
            RetrievalAnnotationKindDto::Gap,
            "an SLA-driven material-query drop must publish an evidence gap"
        );
        assert!(answer.retrieval_trace.sla_missed);
    }

    #[test]
    fn failed_fused_subquery_batch_is_published_as_an_evidence_gap() {
        // EV-6c (#1775). The fused batch failing means none of the planned subqueries returned
        // evidence. The sibling `packet_subqueries fused_batch=` note on the same path is
        // routine telemetry, so this also pins that the two are not classified alike.
        let mut answer = empty_answer();
        let error = run_packet_planned_subqueries(
            &AppController::new(),
            "Trace how StringUtils normalizes request routes",
            &ev6c_plan(),
            PacketBudgetModeDto::Compact,
            &ev6c_limits(),
            false,
            PacketLatencyBudget::new(Some(120_000)),
            &[],
            &mut answer,
        )
        .expect_err("an unopened controller cannot serve a fused packet batch");
        assert!(
            !error.message.is_empty(),
            "fail-closed batch error must carry a reason"
        );

        assert_eq!(
            kind_of(&answer, "packet_fused_subquery_batch_failed error="),
            RetrievalAnnotationKindDto::Gap,
            "a failed subquery batch is missing evidence, not telemetry: {:?}",
            classified(&answer)
        );
        assert_eq!(
            kind_of(&answer, "packet_material_queries fused_batch="),
            RetrievalAnnotationKindDto::Observation,
            "batch sizing is routine telemetry: {:?}",
            classified(&answer)
        );
    }

    #[test]
    fn anchor_probes_skipped_by_budget_are_published_as_an_evidence_gap() {
        // EV-6c (#1775). Anchor probes never dispatched: the anchors they would have found are
        // absent from the packet, so the reason string must ride a `Gap`.
        let mut answer = empty_answer();
        run_packet_anchor_expansion(
            &AppController::new(),
            &ev6c_plan(),
            PacketBudgetModeDto::Tiny,
            &ev6c_limits(),
            false,
            PacketLatencyBudget::new(Some(120_000)),
            &[],
            &mut answer,
        )
        .expect("skipping anchor probes on a tiny budget is not an error");

        assert_eq!(
            classified(&answer),
            vec![(
                RetrievalAnnotationKindDto::Gap,
                "packet_anchor_probes skipped reason=budget=tiny".to_string()
            )],
            "the skipped-anchor-probe producer must publish an evidence gap"
        );
    }

    #[test]
    fn anchor_probes_dropped_for_latency_are_published_as_an_evidence_gap() {
        // EV-6c (#1775). The other reason this producer fires: the latency budget was already
        // spent. This is the reclassification that would hurt most — an answer that silently
        // dropped its anchor expansion to hit an SLA must not also report clean confidence.
        let mut answer = empty_answer();
        answer.retrieval_trace.total_latency_ms = 5_000;
        run_packet_anchor_expansion(
            &AppController::new(),
            &ev6c_plan(),
            PacketBudgetModeDto::Compact,
            &ev6c_limits(),
            false,
            PacketLatencyBudget::new(Some(1_000)),
            &[],
            &mut answer,
        )
        .expect("dropping anchor probes for latency is not an error");

        assert_eq!(
            classified(&answer),
            vec![(
                RetrievalAnnotationKindDto::Gap,
                "packet_anchor_probes skipped reason=latency_budget_exhausted".to_string()
            )],
            "an SLA-driven anchor-probe drop must publish an evidence gap"
        );
        assert!(
            answer.retrieval_trace.sla_missed,
            "the latency-driven drop must also record the missed SLA"
        );
    }

    #[test]
    fn failed_anchor_probes_are_published_as_evidence_gaps_per_query() {
        // EV-6c (#1775). One gap per unanswered probe query, so the packet cannot claim the
        // anchors it asked for.
        let plan = ev6c_plan();
        let expected_queries = packet_anchor_probe_queries(&plan);
        assert!(
            !expected_queries.is_empty(),
            "fixture plan must yield anchor probe queries"
        );

        let mut answer = empty_answer();
        run_packet_anchor_expansion(
            &AppController::new(),
            &plan,
            PacketBudgetModeDto::Compact,
            &ev6c_limits(),
            false,
            PacketLatencyBudget::new(Some(120_000)),
            &[],
            &mut answer,
        )
        .expect_err("an unopened controller cannot serve anchor probes");

        let failures = answer
            .retrieval_trace
            .annotations
            .iter()
            .filter(|annotation| {
                annotation
                    .text
                    .starts_with("packet_anchor_probe_failed query=")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            failures.len(),
            expected_queries.len(),
            "every unanswered probe query must be reported: {:?}",
            classified(&answer)
        );
        for failure in failures {
            assert_eq!(
                failure.kind,
                RetrievalAnnotationKindDto::Gap,
                "a probe query that returned no evidence is a gap: {}",
                failure.text
            );
        }
    }

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
    fn adaptive_queries_follow_missing_material_obligations_and_skip_completed_work() {
        let question = "Explain the indexing runtime, persistence, and snapshot flow.";
        let task_class = PacketTaskClassDto::ArchitectureExplanation;
        let original = PacketPlanQueryDto {
            query: question.to_string(),
            purpose: "original task phrasing".to_string(),
        };
        let plan = PacketPlanDto {
            task_class,
            inferred_task_class: false,
            queries: vec![original.clone()],
            probe_resolutions: Vec::new(),
            obligations: codestory_agent::packet_obligations::build_packet_obligation_plan(
                question,
                task_class,
                &[original],
            ),
            trace: Vec::new(),
        };
        let mut answer = empty_answer();

        let queries = packet_adaptive_material_queries(question, &plan, &answer, 16)
            .into_iter()
            .map(|query| query.query)
            .collect::<Vec<_>>();
        assert_eq!(
            queries.first().map(String::as_str),
            Some("indexing runtime")
        );
        for expected in [
            "indexing entrypoint",
            "file discovery",
            "symbol extraction",
            "storage persistence",
        ] {
            assert!(queries.iter().any(|query| query == expected), "{queries:?}");
        }

        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(PacketSidecarQueryDiagnosticDto {
                query: "indexing entrypoint".to_string(),
                completion: codestory_contracts::api::PacketQueryCompletionDto::Completed,
                retrieval_mode: "full".to_string(),
                sidecar_query_ms: Some(1),
                candidate_resolution_ms: Some(0),
                total_elapsed_ms: Some(1),
                sidecar_stage_count: 1,
                sidecar_stage_total_ms: Some(1),
                batch_query_wall_ms: Some(1),
                candidate_count: 1,
                resolved_hit_count: 1,
                unresolved_candidate_count: 0,
                blocking_unresolved_candidate_count: 0,
                semantic_stage_timeout_zero_hits: false,
                semantic_abstained: false,
                diagnostic: None,
            });
        let queries = packet_adaptive_material_queries(question, &plan, &answer, 16)
            .into_iter()
            .map(|query| query.query)
            .collect::<Vec<_>>();
        assert!(!queries.contains(&"indexing entrypoint".to_string()));
        assert_eq!(
            queries.first().map(String::as_str),
            Some("indexing runtime")
        );
    }

    #[test]
    fn adaptive_queries_use_retrieved_owners_for_missing_lifecycle_members() {
        let question = "Trace how Jekyll's build command creates a site and runs the read, generate, render, and write phases. Cite the source files and name the supporting symbols.";
        let task_class = PacketTaskClassDto::RouteTracing;
        let original = PacketPlanQueryDto {
            query: question.to_string(),
            purpose: "original task phrasing".to_string(),
        };
        let plan = PacketPlanDto {
            task_class,
            inferred_task_class: false,
            queries: vec![original.clone()],
            probe_resolutions: Vec::new(),
            obligations: codestory_agent::packet_obligations::build_packet_obligation_plan(
                question,
                task_class,
                &[original],
            ),
            trace: Vec::new(),
        };
        let mut answer = empty_answer();
        answer.citations.push(anchor_citation("Jekyll::Site.posts"));

        let queries = packet_adaptive_material_queries(question, &plan, &answer, 16)
            .into_iter()
            .map(|query| query.query)
            .collect::<Vec<_>>();

        for expected in ["Site.read", "Site.generate", "Site.render", "Site.write"] {
            assert!(
                queries.iter().any(|query| query == expected),
                "missing {expected} from {queries:?}"
            );
        }
        assert!(queries.len() <= 16);
    }

    #[test]
    fn adaptive_queries_reserve_batch_space_for_explicit_owner_members() {
        let question = "Explain how package:http exposes top-level helpers, BaseClient convenience methods, BaseRequest finalization, and IOClient send behavior.";
        let task_class = PacketTaskClassDto::DataFlow;
        let original = PacketPlanQueryDto {
            query: question.to_string(),
            purpose: "original task phrasing".to_string(),
        };
        let plan = PacketPlanDto {
            task_class,
            inferred_task_class: false,
            queries: vec![original.clone()],
            probe_resolutions: Vec::new(),
            obligations: codestory_agent::packet_obligations::build_packet_obligation_plan(
                question,
                task_class,
                &[original],
            ),
            trace: Vec::new(),
        };

        let queries = packet_adaptive_material_queries(question, &plan, &empty_answer(), 16);

        for expected in ["BaseRequest.finalize", "IOClient.send"] {
            assert!(
                queries.iter().any(|query| query.query == expected),
                "missing {expected} from {queries:?}"
            );
        }
        let owner_probe_indexes = queries
            .iter()
            .enumerate()
            .filter(|(_, query)| query.purpose == PACKET_OWNER_MEMBER_QUERY_PURPOSE)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(owner_probe_indexes.len(), PACKET_OWNER_MEMBER_QUERY_LIMIT);
        let last_owner_probe = *owner_probe_indexes.last().expect("owner probes");
        assert!(
            queries.iter().skip(last_owner_probe + 1).any(|query| {
                query.purpose.starts_with("material obligation ")
                    || query.purpose.starts_with("material query obligation ")
            }),
            "owner probes starved the remaining material queries: {queries:?}"
        );
        assert!(queries.len() <= 16);
    }

    #[test]
    fn adaptive_sql_queries_reserve_the_named_schema_entities() {
        let question = "Explain schema relationships between artists, albums, tracks, invoices, and invoice lines across the SQL scripts.";
        let task_class = PacketTaskClassDto::DataFlow;
        let original = PacketPlanQueryDto {
            query: question.to_string(),
            purpose: "original task phrasing".to_string(),
        };
        let plan = PacketPlanDto {
            task_class,
            inferred_task_class: false,
            queries: vec![original.clone()],
            probe_resolutions: Vec::new(),
            obligations: codestory_agent::packet_obligations::build_packet_obligation_plan(
                question,
                task_class,
                &[original],
            ),
            trace: Vec::new(),
        };

        let queries = packet_adaptive_material_queries(question, &plan, &empty_answer(), 16)
            .into_iter()
            .map(|query| query.query)
            .collect::<Vec<_>>();

        for expected in [
            "public.artist",
            "public.album",
            "public.track",
            "public.invoice",
            "public.invoiceline",
        ] {
            assert!(
                queries.iter().any(|query| query == expected),
                "missing {expected} from {queries:?}"
            );
        }
        assert!(!queries.iter().any(|query| query.starts_with("Chinook.")));
        assert!(queries.len() <= 16);
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
    fn packet_anchor_probe_queries_keep_distinct_late_phases_ahead_of_synthetic_variants() {
        let plan = PacketPlanDto {
            task_class: PacketTaskClassDto::RouteTracing,
            inferred_task_class: false,
            queries: vec![
                PacketPlanQueryDto {
                    query: "Trace how Jekyll builds a site through reading, rendering, and writing"
                        .to_string(),
                    purpose: "original task phrasing for sidecar-primary source-backed retrieval"
                        .to_string(),
                },
                PacketPlanQueryDto {
                    query: "Trace".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "Jekyll".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "build".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "reading".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "rendering".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "writing".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
                PacketPlanQueryDto {
                    query: "build_site".to_string(),
                    purpose: PACKET_ADJACENT_VARIANT_QUERY_PURPOSE.to_string(),
                },
                PacketPlanQueryDto {
                    query: "reading_rendering".to_string(),
                    purpose: PACKET_ADJACENT_VARIANT_QUERY_PURPOSE.to_string(),
                },
                PacketPlanQueryDto {
                    query: "cite".to_string(),
                    purpose: "concrete symbol, file, route, or code term".to_string(),
                },
            ],
            probe_resolutions: Vec::new(),
            obligations: Default::default(),
            trace: Vec::new(),
        };

        let queries = packet_anchor_probe_queries(&plan);

        assert_eq!(
            &queries[..5],
            &[
                "Jekyll".to_string(),
                "build".to_string(),
                "reading".to_string(),
                "rendering".to_string(),
                "writing".to_string(),
            ]
        );
        assert!(!queries.contains(&"Trace".to_string()));
        assert!(!queries.contains(&"cite".to_string()));
        assert!(
            queries.iter().position(|query| query == "writing")
                < queries.iter().position(|query| query == "build_site")
        );
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
                purpose: PACKET_FLOW_ROLE_QUERY_PURPOSE.to_string(),
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
