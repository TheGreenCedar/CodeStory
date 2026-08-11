//! Trace adapters that merge packet batch retrieval results into agent answers.

#![allow(clippy::items_after_test_module)]

use super::packet_candidate::{PacketSearchHit, merge_packet_candidate_graph};
use super::packet_scoring::{packet_citation_key, packet_citation_rank, sort_by_cached_rank_desc};
use super::trace::field;
use codestory_agent::packet_flow_requirements::FlowRequirement;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentResponseBlockDto, AgentResponseSectionDto,
    AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    AgentRetrievalSummaryFieldDto, PacketPlanQueryDto, PacketSidecarQueryDiagnosticDto,
    RetrievalAnnotationDto,
};
use std::collections::{HashMap, HashSet};

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
    results: &[(String, Vec<PacketSearchHit>)],
    duration_ms: u32,
    diagnostics: &[PacketSidecarQueryDiagnosticDto],
    include_evidence: bool,
    rank_terms: &[String],
    stage_carry_limit: usize,
    flow_requirements: &[FlowRequirement],
) {
    let mut citation_indices = answer
        .citations
        .iter()
        .enumerate()
        .map(|(index, citation)| (packet_citation_key(citation), index))
        .collect::<HashMap<_, _>>();

    for (diagnostic_index, ((plan_index, query), (result_query, hits))) in
        pending.iter().zip(results.iter()).enumerate()
    {
        debug_assert_eq!(query.query, *result_query);
        let diagnostic = packet_query_diagnostic(diagnostics, diagnostic_index, result_query);
        let step_duration = packet_query_duration_ms(diagnostic)
            .unwrap_or(duration_ms / pending.len().max(1) as u32);
        let mut added = 0usize;
        let mut candidates = hits
            .iter()
            .map(|hit| (hit.citation(include_evidence), hit))
            .collect::<Vec<_>>();
        sort_by_cached_rank_desc(&mut candidates, |(citation, _)| {
            packet_citation_rank(citation, rank_terms, true)
        });
        let selected =
            select_packet_candidate_indices(&candidates, flow_requirements, stage_carry_limit);
        for candidate_index in selected {
            let (citation, hit) = &candidates[candidate_index];
            if include_evidence {
                merge_packet_candidate_graph(answer, hit);
            }
            let key = packet_citation_key(citation);
            if let Some(existing_index) = citation_indices.get(&key).copied() {
                merge_packet_citation_provenance(&mut answer.citations[existing_index], citation);
            } else {
                let citation_index = answer.citations.len();
                citation_indices.insert(key, citation_index);
                answer.citations.push(citation.clone());
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
        // Echoes prompt-derived subquery text: per-query telemetry, not an evidence gap.
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::observation(format!(
                "packet_fused_subquery index={} query=`{}` purpose=`{}` hits={} citations_added={}{}",
                plan_index,
                query.query.replace('`', "'"),
                query.purpose.replace('`', "'"),
                hits.len(),
                added,
                timing_note
            )));
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

fn select_packet_candidate_indices(
    candidates: &[(AgentCitationDto, &PacketSearchHit)],
    flow_requirements: &[FlowRequirement],
    limit: usize,
) -> Vec<usize> {
    let mut selected = Vec::new();
    let mut selected_set = HashSet::new();
    for requirement in flow_requirements {
        let Some((index, _)) = candidates
            .iter()
            .enumerate()
            .find(|(index, (citation, hit))| {
                !selected_set.contains(index)
                    && requirement.evidence.citation_proves(citation)
                    && (requirement
                        .evidence
                        .citation_proves_without_call_boundary(citation)
                        || hit.has_certain_call_provenance())
            })
        else {
            continue;
        };
        selected.push(index);
        selected_set.insert(index);
        if selected.len() >= limit {
            return selected;
        }
    }
    for index in 0..candidates.len() {
        if selected.len() >= limit {
            break;
        }
        if selected_set.insert(index) {
            selected.push(index);
        }
    }
    selected
}

fn merge_packet_citation_provenance(existing: &mut AgentCitationDto, candidate: &AgentCitationDto) {
    existing
        .evidence_edge_ids
        .extend(candidate.evidence_edge_ids.iter().cloned());
    existing
        .evidence_edge_ids
        .sort_by(|left, right| left.0.cmp(&right.0));
    existing.evidence_edge_ids.dedup();
    existing.evidence_edge_ids.truncate(12);
    if existing.retrieval_score_breakdown.is_none() {
        existing.retrieval_score_breakdown = candidate.retrieval_score_breakdown.clone();
    }
    if existing.evidence_tier.is_none() {
        existing.evidence_tier = candidate.evidence_tier;
    }
    if existing.evidence_producer.is_none() {
        existing.evidence_producer = candidate.evidence_producer.clone();
    }
    if existing.resolution_status.is_none() {
        existing.resolution_status = candidate.resolution_status;
    }
    if existing.eligible_for_sufficiency.is_none() {
        existing.eligible_for_sufficiency = candidate.eligible_for_sufficiency;
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
    use crate::agent::packet_candidate::{PacketGraphDirection, PacketGraphEdgeProvenance};
    use codestory_agent::packet_flow_requirements::packet_flow_requirements_for_terms;
    use codestory_agent::packet_terms::packet_probe_terms;
    use codestory_contracts::api::{
        AgentAnswerDto, AgentRetrievalTraceDto, EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto,
        GraphResponse, NodeId, NodeKind, PacketPlanQueryDto, PacketTaskClassDto, SearchHit,
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
        let results = vec![(
            "exec_events".to_string(),
            vec![PacketSearchHit::without_graph(hit)],
        )];
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
            source_coverage: Vec::new(),
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
                source_freshness_telemetry: None,
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
            &[],
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
        let citation = results[0].1[0].citation(false);
        assert_eq!(answer.citations[0].display_name, citation.display_name);

        let carrier_id = NodeId("session-send".into());
        let carrier = PacketSearchHit {
            hit: SearchHit {
                node_id: carrier_id.clone(),
                display_name: "Session.send".into(),
                kind: NodeKind::METHOD,
                file_path: Some("requests/sessions.py".into()),
                line: Some(50),
                score: 0.01,
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
            },
            graph_provenance: vec![PacketGraphEdgeProvenance {
                edge_id: EdgeId("request-to-send".into()),
                direction: PacketGraphDirection::Outgoing,
                hop: 1,
                producers: vec!["scip_graph_projection".into()],
                certainty: Some("certain".into()),
            }],
            graph: Some(GraphResponse {
                center_id: carrier_id.clone(),
                nodes: vec![
                    GraphNodeDto {
                        id: NodeId("session-request".into()),
                        label: "Session.request".into(),
                        kind: NodeKind::METHOD,
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("requests/sessions.py".into()),
                        qualified_name: Some("Session.request".into()),
                        member_access: None,
                    },
                    GraphNodeDto {
                        id: carrier_id.clone(),
                        label: "Session.send".into(),
                        kind: NodeKind::METHOD,
                        depth: 0,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("requests/sessions.py".into()),
                        qualified_name: Some("Session.send".into()),
                        member_access: None,
                    },
                ],
                edges: vec![GraphEdgeDto {
                    id: EdgeId("request-to-send".into()),
                    source: NodeId("session-request".into()),
                    target: carrier_id,
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".into()),
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        };
        let mut proof_answer = answer.clone();
        proof_answer.citations = vec![carrier.citation(false)];
        proof_answer.graphs.clear();
        proof_answer.subgraph_ids.clear();
        let mut low_ranked_hits = (0..6)
            .map(|index| {
                let mut distractor = results[0].1[0].clone();
                distractor.hit.node_id = NodeId(format!("distractor-{index}"));
                distractor.hit.display_name = format!("dispatch_hook_{index}");
                distractor.hit.score = 1.0 - index as f32 / 100.0;
                distractor
            })
            .collect::<Vec<_>>();
        low_ranked_hits.push(carrier);
        let proof_results = vec![("request dispatch".to_string(), low_ranked_hits)];
        let proof_query = PacketPlanQueryDto {
            query: "request dispatch".into(),
            purpose: "ordered flow".into(),
        };
        let proof_pending = vec![(0usize, &proof_query)];
        let flow_terms = packet_probe_terms(
            "Explain how a top-level request call becomes a prepared request and sends it through a session adapter.",
        );
        let requirements =
            packet_flow_requirements_for_terms(&flow_terms, PacketTaskClassDto::DataFlow);
        merge_packet_fused_subquery_batch(
            &mut proof_answer,
            &proof_pending,
            &proof_results,
            1,
            &[],
            true,
            &flow_terms,
            6,
            &requirements,
        );
        let retained = proof_answer
            .citations
            .iter()
            .filter(|citation| citation.display_name == "Session.send")
            .collect::<Vec<_>>();
        assert_eq!(
            retained.len(),
            1,
            "duplicate citation must be enriched in place"
        );
        assert_eq!(
            retained[0].evidence_edge_ids,
            [EdgeId("request-to-send".into())]
        );
        assert!(proof_answer.graphs.iter().any(|artifact| matches!(
            artifact,
            codestory_contracts::api::GraphArtifactDto::Uml { graph, .. }
                if graph.edges.iter().any(|edge| edge.id.0 == "request-to-send")
        )));
    }
}
