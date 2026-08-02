//! Trace adapters that merge packet batch retrieval results into agent answers.

#![allow(clippy::items_after_test_module)]

use super::citation::to_citation_from_hit;
use super::packet_scoring::{packet_citation_key, packet_citation_rank, sort_by_cached_rank_desc};
use super::trace::field;
use codestory_contracts::api::{
    AgentAnswerDto, AgentResponseBlockDto, AgentResponseSectionDto, AgentRetrievalStepDto,
    AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, AgentRetrievalSummaryFieldDto,
    PacketPlanQueryDto, PacketSidecarQueryDiagnosticDto, SearchHit,
};
use std::collections::HashSet;

fn sanitize_section_id(value: &str) -> String {
    let mut id = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    id.trim_matches('-').chars().take(48).collect()
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_packet_fused_subquery_batch(
    answer: &mut AgentAnswerDto,
    pending: &[(usize, &PacketPlanQueryDto)],
    results: &[(String, Vec<SearchHit>)],
    duration_ms: u32,
    diagnostics: &[PacketSidecarQueryDiagnosticDto],
    include_evidence: bool,
    rank_terms: &[String],
    stage_carry_limit: usize,
) {
    let mut citation_keys = answer
        .citations
        .iter()
        .map(packet_citation_key)
        .collect::<HashSet<_>>();

    for (diagnostic_index, ((plan_index, query), (result_query, hits))) in
        pending.iter().zip(results.iter()).enumerate()
    {
        debug_assert_eq!(query.query, *result_query);
        let diagnostic = packet_query_diagnostic(diagnostics, diagnostic_index, result_query);
        let step_duration = packet_query_duration_ms(diagnostic)
            .unwrap_or(duration_ms / pending.len().max(1) as u32);
        let mut added = 0usize;
        let mut citations = hits
            .iter()
            .map(|hit| to_citation_from_hit(hit, None, None, include_evidence))
            .collect::<Vec<_>>();
        sort_by_cached_rank_desc(&mut citations, |citation| {
            packet_citation_rank(citation, rank_terms, true)
        });
        for citation in citations.into_iter().take(stage_carry_limit) {
            if citation_keys.insert(packet_citation_key(&citation)) {
                answer.citations.push(citation);
                added = added.saturating_add(1);
            }
        }
        let mut output = vec![
            field("hits", hits.len().to_string()),
            field("citations_added", added.to_string()),
            field("mode", "packet_fused_batch".to_string()),
        ];
        append_packet_query_timing_fields(&mut output, diagnostic);
        answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
            kind: AgentRetrievalStepKindDto::Search,
            status: AgentRetrievalStepStatusDto::Ok,
            duration_ms: step_duration,
            input: vec![field("query", query.query.clone())],
            output,
            message: Some(format!("packet subquery `{}`", query.purpose)),
        });
        let timing_note = packet_query_timing_annotation(diagnostic);
        answer.retrieval_trace.annotations.push(format!(
            "packet_fused_subquery index={} query=`{}` purpose=`{}` hits={} citations_added={}{}",
            plan_index,
            query.query.replace('`', "'"),
            query.purpose.replace('`', "'"),
            hits.len(),
            added,
            timing_note
        ));
        answer.sections.push(AgentResponseSectionDto {
            id: format!("packet-subquery-{}", sanitize_section_id(&query.query)),
            title: format!("Planned query: {}", query.query),
            blocks: vec![AgentResponseBlockDto::Markdown {
                markdown: format!(
                    "Purpose: {}\n\nFused packet retrieval found {} candidate hits. Use packet citations for exact files and symbols.",
                    query.purpose,
                    hits.len()
                ),
            }],
        });
    }
}

pub(crate) fn packet_query_diagnostic<'a>(
    diagnostics: &'a [PacketSidecarQueryDiagnosticDto],
    index: usize,
    query: &str,
) -> Option<&'a PacketSidecarQueryDiagnosticDto> {
    diagnostics
        .get(index)
        .filter(|diagnostic| diagnostic.query == query)
        .or_else(|| {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.query == query)
        })
}

pub(crate) fn packet_query_duration_ms(
    diagnostic: Option<&PacketSidecarQueryDiagnosticDto>,
) -> Option<u32> {
    diagnostic.and_then(|diagnostic| diagnostic.total_elapsed_ms.or(diagnostic.sidecar_query_ms))
}

pub(crate) fn append_packet_query_timing_fields(
    output: &mut Vec<AgentRetrievalSummaryFieldDto>,
    diagnostic: Option<&PacketSidecarQueryDiagnosticDto>,
) {
    let Some(diagnostic) = diagnostic else {
        return;
    };
    if let Some(value) = diagnostic.sidecar_query_ms {
        output.push(field("sidecar_query_ms", value.to_string()));
    }
    if let Some(value) = diagnostic.candidate_resolution_ms {
        output.push(field("candidate_resolution_ms", value.to_string()));
    }
    if let Some(value) = diagnostic.total_elapsed_ms {
        output.push(field("sidecar_total_ms", value.to_string()));
    }
    output.push(field(
        "sidecar_stage_count",
        diagnostic.sidecar_stage_count.to_string(),
    ));
    if let Some(value) = diagnostic.sidecar_stage_total_ms {
        output.push(field("sidecar_stage_total_ms", value.to_string()));
    }
    if let Some(value) = diagnostic.batch_query_wall_ms {
        output.push(field("batch_query_wall_ms", value.to_string()));
    }
}

fn packet_query_timing_annotation(diagnostic: Option<&PacketSidecarQueryDiagnosticDto>) -> String {
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

#[cfg(test)]
mod golden_tests {
    use super::*;
    use crate::agent::citation::to_citation_from_hit;
    use codestory_contracts::api::{
        AgentAnswerDto, AgentRetrievalTraceDto, NodeId, NodeKind, PacketPlanQueryDto, SearchHit,
        SearchHitOrigin,
    };

    #[test]
    fn merge_fused_batch_golden_trace_shape() {
        let query = PacketPlanQueryDto {
            query: "exec_events".to_string(),
            purpose: "symbol probe".to_string(),
        };
        let pending = vec![(1usize, &query)];
        let hit = SearchHit {
            node_id: NodeId("node-1".to_string()),
            display_name: "ThreadEvent".to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some("crates/exec/src/exec_events.rs".to_string()),
            line: Some(10),
            score: 0.8,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            match_quality: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
            verification_targets: Vec::new(),
            score_breakdown: None,
        };
        let results = vec![("exec_events".to_string(), vec![hit])];
        let diagnostics = vec![PacketSidecarQueryDiagnosticDto {
            query: "exec_events".to_string(),
            completion: codestory_contracts::api::PacketQueryCompletionDto::Completed,
            retrieval_mode: "full".to_string(),
            sidecar_query_ms: Some(9),
            candidate_resolution_ms: Some(3),
            total_elapsed_ms: Some(12),
            sidecar_stage_count: 0,
            sidecar_stage_total_ms: None,
            batch_query_wall_ms: Some(11),
            candidate_count: 1,
            resolved_hit_count: 1,
            unresolved_candidate_count: 0,
            blocking_unresolved_candidate_count: 0,
            semantic_stage_timeout_zero_hits: false,
            semantic_abstained: false,
            diagnostic: None,
        }];
        let rank_terms = vec!["exec".to_string(), "events".to_string()];
        let mut answer = AgentAnswerDto {
            answer_id: "golden".to_string(),
            prompt: "trace exec flow".to_string(),
            summary: "summary".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "hybrid-v1".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "r".to_string(),
                retrieval_publication: None,
                resolved_profile: codestory_contracts::api::AgentRetrievalPresetDto::Architecture,
                policy_mode: codestory_contracts::api::AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 0,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                semantic_stage_timeout_zero_hits: 0,
                semantic_abstained_count: 0,
                annotations: Vec::new(),
                packet_claim_profile_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        };

        merge_packet_fused_subquery_batch(
            &mut answer,
            &pending,
            &results,
            12,
            &diagnostics,
            false,
            &rank_terms,
            6,
        );

        assert_eq!(answer.citations.len(), 1);
        assert_eq!(answer.retrieval_trace.steps.len(), 1);
        assert_eq!(
            answer.retrieval_trace.steps[0]
                .output
                .iter()
                .find(|field| field.key == "mode")
                .map(|field| field.value.as_str()),
            Some("packet_fused_batch")
        );
        assert_eq!(answer.retrieval_trace.steps[0].duration_ms, 12);
        assert_eq!(
            answer.retrieval_trace.steps[0]
                .output
                .iter()
                .find(|field| field.key == "sidecar_query_ms")
                .map(|field| field.value.as_str()),
            Some("9")
        );
        assert_eq!(
            answer.retrieval_trace.steps[0]
                .output
                .iter()
                .find(|field| field.key == "batch_query_wall_ms")
                .map(|field| field.value.as_str()),
            Some("11")
        );
        let citation = to_citation_from_hit(&results[0].1[0], None, None, false);
        assert_eq!(answer.citations[0].display_name, citation.display_name);
    }
}
