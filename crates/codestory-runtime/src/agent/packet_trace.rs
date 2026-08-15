//! Trace adapters that merge packet batch retrieval results into agent answers.

#![allow(clippy::items_after_test_module)]

use super::packet_batch::{
    PACKET_MATERIAL_OBLIGATION_QUERY_PURPOSE_PREFIX,
    PACKET_MATERIAL_QUERY_OBLIGATION_QUERY_PURPOSE_PREFIX,
};
use super::packet_candidate::{PacketSearchHit, merge_packet_candidate_graph_for_requirements};
use super::packet_capping::PACKET_MATERIAL_QUERY_CARRIER_ROLE;
use super::packet_scoring::{
    normalize_identifier, packet_citation_key, packet_citation_rank, sort_by_cached_rank_desc,
};
use super::trace::field;
use codestory_agent::packet_flow_requirements::FlowRequirement;
use codestory_agent::packet_terms::prompt_search_terms;
use codestory_agent::planning::PACKET_OWNER_MEMBER_QUERY_PURPOSE;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentResponseBlockDto, AgentResponseSectionDto,
    AgentRetrievalStepDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto,
    AgentRetrievalSummaryFieldDto, PacketPlanQueryDto, PacketSidecarQueryDiagnosticDto,
    RetrievalAnnotationDto,
};
use std::collections::{HashMap, HashSet};

pub(crate) fn merge_packet_initial_search_hits(
    answer: &mut AgentAnswerDto,
    hits: &[PacketSearchHit],
    include_evidence: bool,
    rank_terms: &[String],
    stage_carry_limit: usize,
    flow_requirements: &[FlowRequirement],
) -> usize {
    merge_packet_search_hits(
        answer,
        hits,
        include_evidence,
        rank_terms,
        stage_carry_limit,
        flow_requirements,
        None,
        None,
    )
}

fn merge_packet_search_hits(
    answer: &mut AgentAnswerDto,
    hits: &[PacketSearchHit],
    include_evidence: bool,
    rank_terms: &[String],
    stage_carry_limit: usize,
    flow_requirements: &[FlowRequirement],
    exact_query: Option<&str>,
    first_selected_coverage_role: Option<&str>,
) -> usize {
    let mut citation_indices = answer
        .citations
        .iter()
        .enumerate()
        .map(|(index, citation)| (packet_citation_key(citation), index))
        .collect::<HashMap<_, _>>();
    let mut candidates = hits
        .iter()
        .map(|hit| {
            (
                hit.citation_for_requirements(include_evidence, flow_requirements),
                hit,
            )
        })
        .collect::<Vec<_>>();
    sort_by_cached_rank_desc(&mut candidates, |(citation, _)| {
        packet_citation_rank(citation, rank_terms, true)
    });
    let selected = select_packet_candidate_indices(
        &candidates,
        flow_requirements,
        stage_carry_limit,
        exact_query,
    );
    for (selected_index, candidate_index) in selected.iter().enumerate() {
        let (citation, hit) = &candidates[*candidate_index];
        if include_evidence {
            merge_packet_candidate_graph_for_requirements(answer, hit, flow_requirements);
        }
        let key = packet_citation_key(citation);
        if let Some(existing_index) = citation_indices.get(&key).copied() {
            let proof_edge_ids = if include_evidence {
                hit.proof_edge_ids_for_requirements(citation, flow_requirements)
            } else {
                Vec::new()
            };
            merge_packet_citation_provenance(
                &mut answer.citations[existing_index],
                citation,
                &proof_edge_ids,
            );
            if selected_index == 0
                && answer.citations[existing_index].coverage_role.is_none()
                && let Some(role) = first_selected_coverage_role
            {
                answer.citations[existing_index].coverage_role = Some(role.to_string());
            }
        } else {
            let citation_index = answer.citations.len();
            citation_indices.insert(key, citation_index);
            let mut citation = citation.clone();
            if selected_index == 0
                && citation.coverage_role.is_none()
                && let Some(role) = first_selected_coverage_role
            {
                citation.coverage_role = Some(role.to_string());
            }
            answer.citations.push(citation);
        }
    }
    selected.len()
}

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
    for (diagnostic_index, ((plan_index, query), (result_query, hits))) in
        pending.iter().zip(results.iter()).enumerate()
    {
        debug_assert_eq!(query.query, *result_query);
        let diagnostic = packet_query_diagnostic(diagnostics, diagnostic_index, result_query);
        let step_duration = packet_query_duration_ms(diagnostic)
            .unwrap_or(duration_ms / pending.len().max(1) as u32);
        let before = answer.citations.len();
        let query_rank_terms = prompt_search_terms(&query.query);
        let effective_rank_terms = if query_rank_terms.is_empty() {
            rank_terms
        } else {
            &query_rank_terms
        };
        merge_packet_search_hits(
            answer,
            hits,
            include_evidence,
            effective_rank_terms,
            stage_carry_limit,
            flow_requirements,
            (query.purpose == PACKET_OWNER_MEMBER_QUERY_PURPOSE).then_some(query.query.as_str()),
            (query
                .purpose
                .starts_with(PACKET_MATERIAL_OBLIGATION_QUERY_PURPOSE_PREFIX)
                || query
                    .purpose
                    .starts_with(PACKET_MATERIAL_QUERY_OBLIGATION_QUERY_PURPOSE_PREFIX))
            .then_some(PACKET_MATERIAL_QUERY_CARRIER_ROLE),
        );
        let added = answer.citations.len().saturating_sub(before);
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
    exact_query: Option<&str>,
) -> Vec<usize> {
    if limit == 0 {
        return Vec::new();
    }
    let mut selected = Vec::new();
    let mut selected_set = HashSet::new();
    if let Some(exact_query) = exact_query {
        let exact_query = normalize_identifier(exact_query);
        if let Some((index, _)) = candidates.iter().enumerate().find(|(_, (citation, _))| {
            citation.origin == codestory_contracts::api::SearchHitOrigin::IndexedSymbol
                && citation.resolvable
                && normalize_identifier(&citation.display_name) == exact_query
        }) {
            selected.push(index);
            selected_set.insert(index);
            if selected.len() >= limit {
                return selected;
            }
        }
    }
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
                        || hit.has_proof_call_provenance_for_requirement(citation, requirement))
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

fn merge_packet_citation_provenance(
    existing: &mut AgentCitationDto,
    candidate: &AgentCitationDto,
    proof_edge_ids: &[codestory_contracts::api::EdgeId],
) {
    let mut merged_edge_ids = Vec::new();
    for edge_id in proof_edge_ids
        .iter()
        .chain(existing.evidence_edge_ids.iter())
        .chain(candidate.evidence_edge_ids.iter())
    {
        if !merged_edge_ids.contains(edge_id) {
            merged_edge_ids.push(edge_id.clone());
        }
    }
    merged_edge_ids.truncate(12);
    existing.evidence_edge_ids = merged_edge_ids;

    if packet_candidate_evidence_lane_is_stronger(existing, candidate) {
        existing.score = candidate.score;
        existing.retrieval_score_breakdown = candidate.retrieval_score_breakdown.clone();
        existing.evidence_tier = candidate.evidence_tier;
        existing.evidence_producer = candidate.evidence_producer.clone();
        existing.resolution_status = candidate.resolution_status;
        existing.eligible_for_sufficiency = candidate.eligible_for_sufficiency;
    }
}

fn packet_candidate_evidence_lane_is_stronger(
    existing: &AgentCitationDto,
    candidate: &AgentCitationDto,
) -> bool {
    let existing_eligible =
        codestory_agent::packet_evidence::citation_sufficiency_eligible(existing);
    let candidate_eligible =
        codestory_agent::packet_evidence::citation_sufficiency_eligible(candidate);
    if existing_eligible != candidate_eligible {
        return candidate_eligible;
    }

    let resolution_rank = |resolution| match resolution {
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::Resolved) => 4,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly) => 3,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::Unresolved) => 2,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::DiagnosticOnly) => 1,
        None => 0,
    };
    let existing_resolution = resolution_rank(existing.resolution_status);
    let candidate_resolution = resolution_rank(candidate.resolution_status);
    if existing_resolution != candidate_resolution {
        return candidate_resolution > existing_resolution;
    }

    let tier_rank = |tier| match tier {
        Some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource) => 9,
        Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph) => 8,
        Some(codestory_contracts::api::PacketEvidenceTierDto::LexicalSource) => 7,
        Some(codestory_contracts::api::PacketEvidenceTierDto::SymbolDoc) => 6,
        Some(codestory_contracts::api::PacketEvidenceTierDto::ComponentReport) => 5,
        Some(codestory_contracts::api::PacketEvidenceTierDto::DenseSemantic) => 4,
        Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText) => 3,
        Some(codestory_contracts::api::PacketEvidenceTierDto::SyntheticSourceScan) => 2,
        Some(codestory_contracts::api::PacketEvidenceTierDto::GeneratedSummary) => 1,
        None => 0,
    };
    let existing_tier = tier_rank(existing.evidence_tier);
    let candidate_tier = tier_rank(candidate.evidence_tier);
    if existing_tier != candidate_tier {
        return candidate_tier > existing_tier;
    }
    candidate.score > existing.score
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
    use crate::agent::packet_budget::{
        apply_packet_budget_with_extra_and_obligation_carriers, cap_packet_graph_edges_for_test,
        packet_budget_limits,
    };
    use crate::agent::packet_candidate::{PacketGraphDirection, PacketGraphEdgeProvenance};
    use codestory_agent::packet_flow_requirements::packet_flow_requirements_for_terms;
    use codestory_agent::packet_obligations::{
        build_packet_obligation_plan, capture_packet_obligation_edge_proofs_before_budget,
        finalize_packet_obligation_plan, install_retained_packet_obligation_edge_proofs,
        protected_packet_obligation_carrier_node_ids, protected_packet_obligation_edge_ids,
    };
    use codestory_agent::packet_terms::packet_probe_terms;
    use codestory_contracts::api::{
        AgentAnswerDto, AgentRetrievalTraceDto, EdgeId, EdgeKind, GraphArtifactDto, GraphEdgeDto,
        GraphNodeDto, GraphResponse, NodeId, NodeKind, PacketBudgetDto, PacketBudgetLimitsDto,
        PacketBudgetModeDto, PacketBudgetUsageDto, PacketEvidenceResolutionDto,
        PacketEvidenceTierDto, PacketObligationProofStatusDto, PacketPlanQueryDto,
        PacketTaskClassDto, RetrievalScoreBreakdownDto, SearchHit, SearchHitOrigin,
    };

    fn empty_answer(prompt: &str) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "packet-trace".into(),
            prompt: prompt.into(),
            summary: "summary".into(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "hybrid-v1".into(),
            graphs: Vec::new(),
            source_coverage: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "request".into(),
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
        }
    }

    fn call_boundary_hit(
        center_id: &str,
        display_name: &str,
        target_id: &str,
        target_label: &str,
        edge_id: &str,
        file_path: &str,
    ) -> PacketSearchHit {
        let receiver_owner = target_label
            .rsplit_once('.')
            .map(|(owner, _)| owner)
            .or_else(|| display_name.rsplit_once('.').map(|(owner, _)| owner))
            .unwrap_or(display_name);
        call_boundary_hit_with_receiver_owner(
            center_id,
            display_name,
            target_id,
            target_label,
            edge_id,
            file_path,
            receiver_owner,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn call_boundary_hit_with_receiver_owner(
        center_id: &str,
        display_name: &str,
        target_id: &str,
        target_label: &str,
        edge_id: &str,
        file_path: &str,
        receiver_owner: &str,
    ) -> PacketSearchHit {
        let center_id = NodeId(center_id.into());
        let target_id = NodeId(target_id.into());
        PacketSearchHit {
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: display_name.into(),
                kind: NodeKind::METHOD,
                file_path: Some(file_path.into()),
                line: Some(10),
                score: 0.6,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::LexicalSource),
                evidence_producer: Some("symbol_doc".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: Some(RetrievalScoreBreakdownDto {
                    lexical: 0.6,
                    semantic: 0.0,
                    graph: 0.0,
                    total: 0.6,
                    tier_cap: None,
                    boosts: Vec::new(),
                    dampening: Vec::new(),
                    final_rank_reason: Some("lexical source".into()),
                    provenance: vec!["symbol_doc".into()],
                }),
            },
            graph_provenance: vec![PacketGraphEdgeProvenance {
                edge_id: EdgeId(edge_id.into()),
                direction: PacketGraphDirection::Outgoing,
                hop: 1,
                producers: vec!["core_incident_call".into()],
                certainty: None,
            }],
            graph: Some(GraphResponse {
                center_id: center_id.clone(),
                nodes: vec![
                    GraphNodeDto {
                        id: center_id.clone(),
                        label: display_name.into(),
                        kind: NodeKind::METHOD,
                        depth: 0,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some(file_path.into()),
                        qualified_name: Some(display_name.into()),
                        member_access: None,
                    },
                    GraphNodeDto {
                        id: target_id.clone(),
                        label: target_label.into(),
                        kind: NodeKind::UNKNOWN,
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some(file_path.into()),
                        qualified_name: Some(target_label.into()),
                        member_access: None,
                    },
                ],
                edges: vec![GraphEdgeDto {
                    id: EdgeId(edge_id.into()),
                    source: center_id,
                    target: target_id,
                    kind: EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: Some(format!(
                        "{file_path}:10:1:20|syntax:js-member-call|receiver-owner:{receiver_owner}"
                    )),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        }
    }

    fn complete_packet_budget(answer: &AgentAnswerDto) -> PacketBudgetDto {
        let trail_edges = answer
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                codestory_contracts::api::GraphArtifactDto::Uml { graph, .. } => {
                    Some(graph.edges.len())
                }
                codestory_contracts::api::GraphArtifactDto::Mermaid { .. } => None,
            })
            .sum::<usize>();
        PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: PacketBudgetLimitsDto {
                max_anchors: 13,
                max_files: 13,
                max_snippets: 13,
                max_trail_edges: 20,
                max_output_bytes: 98_304,
            },
            used: PacketBudgetUsageDto {
                anchors: u32::try_from(answer.citations.len()).unwrap_or(u32::MAX),
                files: 3,
                snippets: 0,
                trail_edges: u32::try_from(trail_edges).unwrap_or(u32::MAX),
                output_bytes: 1_024,
            },
            truncated: false,
            omitted_sections: Vec::new(),
            next_deeper_command: None,
        }
    }

    fn mark_dense_only(hit: &mut PacketSearchHit) {
        hit.hit.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        hit.hit.evidence_producer = Some("dense_anchor".into());
        hit.hit.eligible_for_sufficiency = Some(false);
        hit.hit.score_breakdown = Some(RetrievalScoreBreakdownDto {
            lexical: 0.0,
            semantic: hit.hit.score,
            graph: 0.0,
            total: hit.hit.score,
            tier_cap: Some(0.4),
            boosts: Vec::new(),
            dampening: vec!["dense_only".into()],
            final_rank_reason: Some("dense anchor".into()),
            provenance: vec!["dense_anchor".into()],
        });
    }

    fn dense_distractor(id: &str) -> PacketSearchHit {
        PacketSearchHit::without_graph(SearchHit {
            node_id: NodeId(id.into()),
            display_name: format!("metrics_hook_{id}"),
            kind: NodeKind::FUNCTION,
            file_path: Some("src/telemetry.js".into()),
            line: Some(1),
            score: 0.99,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            match_quality: None,
            evidence_tier: Some(PacketEvidenceTierDto::DenseSemantic),
            evidence_producer: Some("dense_anchor".into()),
            resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(false),
            source_excerpt: None,
            verification_targets: Vec::new(),
            score_breakdown: None,
        })
    }

    #[test]
    fn fused_subquery_ranking_preserves_the_subquery_intent() {
        let mut relevant = dense_distractor("relevant");
        relevant.hit.display_name = "readQueryFromClient".into();
        relevant.hit.file_path = Some("src/networking.c".into());
        relevant.hit.score = 0.1;
        let mut whole_task_distractor = dense_distractor("whole-task");
        whole_task_distractor.hit.display_name = "commandExecutionRouter".into();
        whole_task_distractor.hit.file_path = Some("src/commands.c".into());
        whole_task_distractor.hit.score = 0.1;
        let query = PacketPlanQueryDto {
            query: "reads client input".into(),
            purpose: "material obligation command_network_input".into(),
        };
        let pending = vec![(0, &query)];
        let results = vec![(query.query.clone(), vec![whole_task_distractor, relevant])];
        let mut answer = empty_answer("Trace client input through command execution.");

        merge_packet_fused_subquery_batch(
            &mut answer,
            &pending,
            &results,
            1,
            &[],
            false,
            &["command".into(), "execution".into()],
            1,
            &[],
        );

        assert_eq!(answer.citations.len(), 1);
        assert_eq!(answer.citations[0].display_name, "readQueryFromClient");
    }

    #[test]
    fn material_query_carrier_survives_the_real_citation_cap() {
        let prompt = "Trace request intake through dispatch.";
        let mut answer = empty_answer(prompt);
        for index in 0..13 {
            let mut distractor = dense_distractor(&format!("noise-{index}"));
            distractor.hit.file_path = Some(format!("src/noise_{index}.rs"));
            distractor.hit.score = 0.99 - index as f32 / 100.0;
            answer.citations.push(distractor.citation(false));
        }

        let mut carrier = dense_distractor("request-intake");
        carrier.hit.display_name = "decodeRequest".into();
        carrier.hit.file_path = Some("src/request_intake.rs".into());
        carrier.hit.score = 0.01;
        let query = PacketPlanQueryDto {
            query: "request intake".into(),
            purpose: format!("{PACKET_MATERIAL_OBLIGATION_QUERY_PURPOSE_PREFIX}request_intake"),
        };
        let pending = vec![(0, &query)];
        let results = vec![(query.query.clone(), vec![carrier])];

        merge_packet_fused_subquery_batch(
            &mut answer,
            &pending,
            &results,
            1,
            &[],
            false,
            &[],
            1,
            &[],
        );

        assert_eq!(answer.citations.len(), 14);
        assert!(answer.citations.iter().any(|citation| {
            citation.file_path.as_deref() == Some("src/request_intake.rs")
                && citation.coverage_role.as_deref() == Some(PACKET_MATERIAL_QUERY_CARRIER_ROLE)
        }));

        let temp = tempfile::tempdir().expect("packet budget root");
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let budget = apply_packet_budget_with_extra_and_obligation_carriers(
            temp.path(),
            prompt,
            PacketTaskClassDto::RouteTracing,
            PacketBudgetModeDto::Compact,
            limits,
            &mut answer,
            &[],
            &[],
            &[],
        );

        assert!(budget.truncated, "compact citation cap must run");
        assert!(
            answer
                .citations
                .iter()
                .any(|citation| { citation.file_path.as_deref() == Some("src/request_intake.rs") })
        );
    }

    fn requests_session_request_hit() -> PacketSearchHit {
        let session_request = NodeId("5296498989960597280".into());
        let prepare_request = NodeId("9192115447235681128".into());
        let mut hit = PacketSearchHit {
            hit: SearchHit {
                node_id: session_request.clone(),
                display_name: "Session.request".into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/requests/sessions.py".into()),
                line: Some(557),
                score: 0.22,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::DenseSemantic),
                evidence_producer: Some("dense_anchor".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(false),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance: vec![
                PacketGraphEdgeProvenance {
                    edge_id: EdgeId("-6363172310279055617".into()),
                    direction: PacketGraphDirection::Outgoing,
                    hop: 1,
                    producers: vec!["core_incident_call".into()],
                    certainty: Some("certain".into()),
                },
                PacketGraphEdgeProvenance {
                    edge_id: EdgeId("2489411124501892282".into()),
                    direction: PacketGraphDirection::Outgoing,
                    hop: 1,
                    producers: vec!["core_incident_call".into()],
                    certainty: Some("certain".into()),
                },
            ],
            graph: Some(GraphResponse {
                center_id: session_request.clone(),
                nodes: vec![
                    GraphNodeDto {
                        id: session_request.clone(),
                        label: "Session.request".into(),
                        kind: NodeKind::METHOD,
                        depth: 0,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/requests/sessions.py".into()),
                        qualified_name: Some("Session.request".into()),
                        member_access: None,
                    },
                    GraphNodeDto {
                        id: prepare_request.clone(),
                        label: "Session.prepare_request".into(),
                        kind: NodeKind::METHOD,
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/requests/sessions.py".into()),
                        qualified_name: Some("Session.prepare_request".into()),
                        member_access: None,
                    },
                ],
                edges: vec![
                    GraphEdgeDto {
                        id: EdgeId("-6363172310279055617".into()),
                        source: session_request.clone(),
                        target: session_request.clone(),
                        kind: EdgeKind::CALL,
                        confidence: Some(0.95),
                        certainty: Some("certain".into()),
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                    GraphEdgeDto {
                        id: EdgeId("2489411124501892282".into()),
                        source: session_request,
                        target: prepare_request,
                        kind: EdgeKind::CALL,
                        confidence: Some(1.0),
                        certainty: Some("certain".into()),
                        callsite_identity: None,
                        candidate_targets: Vec::new(),
                    },
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        };
        mark_dense_only(&mut hit);
        hit
    }

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

    #[test]
    fn owner_member_probe_carries_its_exact_hit_before_higher_ranked_distractors() {
        let query = PacketPlanQueryDto {
            query: "BaseRequest.finalize".to_string(),
            purpose: PACKET_OWNER_MEMBER_QUERY_PURPOSE.to_string(),
        };
        let pending = vec![(0usize, &query)];
        let mut exact = dense_distractor("exact-owner-member");
        exact.hit.display_name = "BaseRequest.finalize".to_string();
        exact.hit.file_path = Some("pkgs/http/lib/src/base_request.dart".to_string());
        exact.hit.score = 0.01;
        exact.hit.eligible_for_sufficiency = Some(true);
        exact.hit.evidence_tier = Some(PacketEvidenceTierDto::ResolvedGraph);
        let results = vec![(
            query.query.clone(),
            vec![dense_distractor("higher-ranked"), exact],
        )];
        let mut answer = empty_answer("Explain BaseRequest finalization.");

        merge_packet_fused_subquery_batch(
            &mut answer,
            &pending,
            &results,
            1,
            &[],
            false,
            &["unrelated".to_string()],
            1,
            &[],
        );

        assert_eq!(answer.citations.len(), 1);
        assert_eq!(answer.citations[0].display_name, "BaseRequest.finalize");
    }

    #[test]
    fn initial_session_request_hit_keeps_lawful_receipt_and_duplicate_merge_is_idempotent() {
        let prompt = "Trace how a top-level request call becomes a prepared request and sends it through a session adapter.";
        let terms = packet_probe_terms(prompt);
        let task_class = PacketTaskClassDto::ArchitectureExplanation;
        let requirements = packet_flow_requirements_for_terms(&terms, task_class);
        let entrypoint = requirements
            .iter()
            .find(|requirement| requirement.id == "request_entrypoint")
            .expect("client request entrypoint requirement");
        let hit = requests_session_request_hit();
        let plain_initial_citation = hit.citation_for_requirements(false, &requirements);
        assert!(plain_initial_citation.evidence_edge_ids.is_empty());

        let mut evidence_disabled = empty_answer(prompt);
        evidence_disabled
            .citations
            .push(plain_initial_citation.clone());
        merge_packet_initial_search_hits(
            &mut evidence_disabled,
            std::slice::from_ref(&hit),
            false,
            &terms,
            8,
            &requirements,
        );
        assert!(evidence_disabled.citations[0].evidence_edge_ids.is_empty());
        assert!(evidence_disabled.graphs.is_empty());

        let mut answer = empty_answer(prompt);
        answer.citations.push(plain_initial_citation);
        let mut primary_hits = (0..20)
            .map(|index| dense_distractor(&format!("primary-{index}")))
            .collect::<Vec<_>>();
        primary_hits.push(hit.clone());
        let selected = merge_packet_initial_search_hits(
            &mut answer,
            &primary_hits,
            true,
            &terms,
            8,
            &requirements,
        );
        assert_eq!(selected, 8);

        let session_citations = answer
            .citations
            .iter()
            .filter(|citation| citation.node_id == hit.hit.node_id)
            .collect::<Vec<_>>();
        assert_eq!(
            session_citations.len(),
            1,
            "initial citation enriches in place"
        );
        let session_citation = session_citations[0];
        assert_eq!(
            session_citation.evidence_edge_ids,
            [EdgeId("2489411124501892282".into())],
            "the lawful prepare receipt must beat the resolved false self-loop"
        );
        assert_eq!(
            session_citation.evidence_tier,
            Some(PacketEvidenceTierDto::ResolvedGraph)
        );
        assert_eq!(
            session_citation.evidence_producer.as_deref(),
            Some("core_incident_call")
        );
        assert_eq!(session_citation.eligible_for_sufficiency, Some(true));
        assert!(hit.has_proof_call_provenance_for_requirement(session_citation, entrypoint));
        assert!(answer.graphs.iter().any(|artifact| matches!(
            artifact,
            codestory_contracts::api::GraphArtifactDto::Uml { graph, .. }
                if graph.center_id == hit.hit.node_id
                    && graph.edges.iter().any(|edge| edge.id.0 == "2489411124501892282")
        )));

        let query = PacketPlanQueryDto {
            query: "request entrypoint".into(),
            purpose: "client entrypoint".into(),
        };
        let pending = vec![(0usize, &query)];
        let results = vec![(query.query.clone(), vec![hit])];
        let citation_count = answer.citations.len();
        let graph_count = answer.graphs.len();
        let subgraph_ids = answer.subgraph_ids.clone();
        for _ in 0..2 {
            merge_packet_fused_subquery_batch(
                &mut answer,
                &pending,
                &results,
                1,
                &[],
                true,
                &terms,
                8,
                &requirements,
            );
            assert_eq!(answer.citations.len(), citation_count);
            assert_eq!(answer.graphs.len(), graph_count);
            assert_eq!(answer.subgraph_ids, subgraph_ids);
            let citation = answer
                .citations
                .iter()
                .find(|citation| citation.node_id.0 == "5296498989960597280")
                .expect("session request citation");
            assert_eq!(
                citation.evidence_edge_ids,
                [EdgeId("2489411124501892282".into())]
            );
            assert_eq!(
                citation.evidence_tier,
                Some(PacketEvidenceTierDto::ResolvedGraph)
            );
        }

        assert!(cap_packet_graph_edges_for_test(
            &mut answer,
            1,
            &[EdgeId("2489411124501892282".into())],
        ));
        let GraphArtifactDto::Uml { id, graph, .. } = &answer.graphs[0] else {
            panic!("expected candidate selection view");
        };
        let immutable_selection_view_id = id.clone();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].id.0, "2489411124501892282");
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 1);

        merge_packet_fused_subquery_batch(
            &mut answer,
            &pending,
            &results,
            1,
            &[],
            true,
            &terms,
            8,
            &requirements,
        );
        let GraphArtifactDto::Uml { id, graph, .. } = &answer.graphs[0] else {
            panic!("expected replayed candidate selection view");
        };
        assert_eq!(id, &immutable_selection_view_id);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].id.0, "2489411124501892282");
        assert_eq!(graph.omitted_edge_count, 1);

        answer.citations.extend((0..20).map(|index| {
            let mut distractor = dense_distractor(&format!("final-cap-{index}"));
            distractor.hit.score = 20.0 - index as f32 / 100.0;
            distractor.citation(false)
        }));
        let mut obligation_plan = build_packet_obligation_plan(prompt, task_class, &[query]);
        let snapshot = capture_packet_obligation_edge_proofs_before_budget(
            prompt,
            task_class,
            &obligation_plan,
            &answer,
        );
        assert_eq!(
            protected_packet_obligation_carrier_node_ids(&snapshot),
            [NodeId("5296498989960597280".into())]
        );
        assert_eq!(
            protected_packet_obligation_edge_ids(&snapshot),
            [EdgeId("2489411124501892282".into())]
        );
        let temp = tempfile::tempdir().expect("packet budget root");
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let budget = apply_packet_budget_with_extra_and_obligation_carriers(
            temp.path(),
            prompt,
            task_class,
            PacketBudgetModeDto::Compact,
            limits.clone(),
            &mut answer,
            &[],
            protected_packet_obligation_carrier_node_ids(&snapshot),
            protected_packet_obligation_edge_ids(&snapshot),
        );
        assert!(
            budget.truncated,
            "compact citation cap must run in this fixture"
        );
        assert!(
            answer
                .citations
                .iter()
                .any(|citation| citation.node_id.0 == "5296498989960597280")
        );
        assert!(answer.graphs.iter().any(|artifact| matches!(
            artifact,
            GraphArtifactDto::Uml { graph, .. }
                if graph.edges.iter().any(|edge| edge.id.0 == "2489411124501892282")
        )));
        install_retained_packet_obligation_edge_proofs(
            &mut obligation_plan,
            &answer,
            &budget,
            &snapshot,
            limits.max_anchors as usize,
        );
        finalize_packet_obligation_plan(prompt, task_class, &mut obligation_plan, &answer, &budget);
        let obligation = obligation_plan
            .claim_obligations
            .iter()
            .find(|obligation| obligation.id == "request_entrypoint")
            .expect("request entrypoint obligation");
        assert_eq!(
            obligation.proof_status,
            PacketObligationProofStatusDto::Proven
        );
        assert!(obligation.carrier_edge_proofs.iter().any(|proof| {
            proof.edge_id == EdgeId("2489411124501892282".into())
                && proof.carrier_node_id == NodeId("5296498989960597280".into())
        }));
    }

    #[test]
    fn duplicate_citation_promotes_the_strongest_admissible_lane_atomically() {
        let hit = call_boundary_hit(
            "application-handle",
            "app.handle",
            "router-handle",
            "Router.handle",
            "handle-proof",
            "src/application.js",
        );
        let candidate = hit.citation(true);
        let mut existing = candidate.clone();
        existing.score = 0.99;
        existing.evidence_edge_ids = vec![EdgeId("dense-context".into())];
        existing.retrieval_score_breakdown = Some(RetrievalScoreBreakdownDto {
            lexical: 0.0,
            semantic: 0.99,
            graph: 0.0,
            total: 0.99,
            tier_cap: Some(0.4),
            boosts: Vec::new(),
            dampening: vec!["dense_only".into()],
            final_rank_reason: Some("dense anchor".into()),
            provenance: vec!["dense_anchor".into()],
        });
        existing.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        existing.evidence_producer = Some("dense_anchor".into());
        existing.resolution_status = Some(PacketEvidenceResolutionDto::Resolved);
        existing.eligible_for_sufficiency = Some(false);

        merge_packet_citation_provenance(
            &mut existing,
            &candidate,
            &[EdgeId("handle-proof".into())],
        );

        assert_eq!(existing.score, 0.6);
        assert_eq!(
            existing.evidence_edge_ids,
            [
                EdgeId("handle-proof".into()),
                EdgeId("dense-context".into())
            ]
        );
        assert_eq!(
            existing.evidence_tier,
            Some(PacketEvidenceTierDto::LexicalSource)
        );
        assert_eq!(existing.evidence_producer.as_deref(), Some("symbol_doc"));
        assert_eq!(
            existing.resolution_status,
            Some(PacketEvidenceResolutionDto::Resolved)
        );
        assert_eq!(existing.eligible_for_sufficiency, Some(true));
        let breakdown = existing
            .retrieval_score_breakdown
            .as_ref()
            .expect("lexical score breakdown");
        assert_eq!(breakdown.lexical, 0.6);
        assert_eq!(breakdown.semantic, 0.0);
        assert_eq!(breakdown.provenance, ["symbol_doc"]);
    }

    #[test]
    fn compact_server_flow_reserves_all_three_exact_call_boundaries() {
        let prompt = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        let terms = packet_probe_terms(prompt);
        let requirements =
            packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing);
        assert_eq!(requirements.len(), 3);

        let mut carriers = [
            call_boundary_hit_with_receiver_owner(
                "application-route",
                "app.route",
                "route",
                "route",
                "route-proof",
                "src/application.js",
                "app.router",
            ),
            call_boundary_hit_with_receiver_owner(
                "application-handle",
                "app.handle",
                "router-handle",
                "handle",
                "handle-proof",
                "src/application.js",
                "app.router",
            ),
            call_boundary_hit(
                "response-send",
                "res.send",
                "response-end",
                "end",
                "send-proof",
                "src/response.js",
            ),
        ];
        for carrier in &mut carriers {
            mark_dense_only(carrier);
        }
        let queries = [
            PacketPlanQueryDto {
                query: "application use".into(),
                purpose: "registration".into(),
            },
            PacketPlanQueryDto {
                query: "application handle".into(),
                purpose: "dispatch".into(),
            },
            PacketPlanQueryDto {
                query: "response send".into(),
                purpose: "terminal".into(),
            },
        ];
        let pending = queries
            .iter()
            .enumerate()
            .collect::<Vec<(usize, &PacketPlanQueryDto)>>();
        let results = queries
            .iter()
            .zip(carriers.iter())
            .enumerate()
            .map(|(query_index, (query, carrier))| {
                let mut hits = (0..14)
                    .map(|index| dense_distractor(&format!("{query_index}-{index}")))
                    .collect::<Vec<_>>();
                hits.push(carrier.clone());
                (query.query.clone(), hits)
            })
            .collect::<Vec<_>>();
        let mut answer = empty_answer(prompt);

        merge_packet_fused_subquery_batch(
            &mut answer,
            &pending,
            &results,
            3,
            &[],
            true,
            &terms,
            1,
            &requirements,
        );

        assert_eq!(answer.citations.len(), 3);
        assert!(
            answer
                .citations
                .iter()
                .all(|citation| !citation.display_name.contains("metrics_hook"))
        );
        for (display_name, edge_id) in [
            ("app.route", "route-proof"),
            ("app.handle", "handle-proof"),
            ("res.send", "send-proof"),
        ] {
            let citation = answer
                .citations
                .iter()
                .find(|citation| citation.display_name == display_name)
                .expect("reserved flow carrier");
            assert_eq!(citation.evidence_edge_ids[0], EdgeId(edge_id.into()));
            assert_eq!(
                citation.evidence_tier,
                Some(PacketEvidenceTierDto::ResolvedGraph)
            );
            assert_eq!(citation.eligible_for_sufficiency, Some(true));
            assert!(answer.graphs.iter().any(|artifact| matches!(
                artifact,
                codestory_contracts::api::GraphArtifactDto::Uml { graph, .. }
                    if graph.edges.iter().any(|edge| edge.id.0 == edge_id)
            )));
        }

        let mut obligation_plan =
            build_packet_obligation_plan(prompt, PacketTaskClassDto::RouteTracing, &queries);
        finalize_packet_obligation_plan(
            prompt,
            PacketTaskClassDto::RouteTracing,
            &mut obligation_plan,
            &answer,
            &complete_packet_budget(&answer),
        );
        for requirement_id in ["request_entrypoint", "request_dispatch", "request_terminal"] {
            let obligation = obligation_plan
                .claim_obligations
                .iter()
                .find(|obligation| obligation.id == requirement_id)
                .unwrap_or_else(|| panic!("missing {requirement_id} obligation"));
            assert_eq!(
                obligation.proof_status,
                PacketObligationProofStatusDto::Proven,
                "receiver-matched CALL must survive full obligation finalization: {obligation:?}"
            );
        }
    }

    #[test]
    fn owner_invalid_unresolved_call_context_stays_non_proven_after_full_merge() {
        let prompt = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        let terms = packet_probe_terms(prompt);
        let requirements =
            packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing);
        let mut bad_carriers = [
            call_boundary_hit_with_receiver_owner(
                "bad-application-use",
                "app.use",
                "metrics-use",
                "use",
                "metrics-use-context",
                "src/application.js",
                "Metrics",
            ),
            {
                let mut hit = call_boundary_hit_with_receiver_owner(
                    "bad-application-handle",
                    "app.handle",
                    "telemetry-handle",
                    "handle",
                    "telemetry-handle-context",
                    "src/application.js",
                    "Telemetry",
                );
                hit.graph.as_mut().expect("graph").edges[0].confidence = Some(1.0);
                hit
            },
            call_boundary_hit_with_receiver_owner(
                "bad-response-send",
                "res.send",
                "telemetry-end",
                "end",
                "telemetry-end-context",
                "src/response.js",
                "Telemetry",
            ),
            call_boundary_hit_with_receiver_owner(
                "bad-response-write",
                "res.send",
                "cache-write",
                "write",
                "cache-write-context",
                "src/response.js",
                "Cache",
            ),
        ];
        for carrier in &mut bad_carriers {
            mark_dense_only(carrier);
        }
        let queries = [
            PacketPlanQueryDto {
                query: "application use".into(),
                purpose: "registration".into(),
            },
            PacketPlanQueryDto {
                query: "application handle".into(),
                purpose: "dispatch".into(),
            },
            PacketPlanQueryDto {
                query: "response send".into(),
                purpose: "terminal".into(),
            },
            PacketPlanQueryDto {
                query: "response finalization".into(),
                purpose: "terminal fallback".into(),
            },
        ];
        let pending = queries
            .iter()
            .enumerate()
            .collect::<Vec<(usize, &PacketPlanQueryDto)>>();
        let results = queries
            .iter()
            .zip(bad_carriers)
            .map(|(query, carrier)| (query.query.clone(), vec![carrier]))
            .collect::<Vec<_>>();
        let mut answer = empty_answer(prompt);

        merge_packet_fused_subquery_batch(
            &mut answer,
            &pending,
            &results,
            3,
            &[],
            true,
            &terms,
            1,
            &requirements,
        );

        assert_eq!(answer.citations.len(), 4);
        assert!(
            answer
                .citations
                .iter()
                .all(|citation| citation.evidence_edge_ids.is_empty()),
            "owner-invalid unresolved CALLs must remain graph-only context"
        );
        for edge_id in [
            "metrics-use-context",
            "telemetry-handle-context",
            "telemetry-end-context",
            "cache-write-context",
        ] {
            assert!(answer.graphs.iter().any(|artifact| matches!(
                artifact,
                codestory_contracts::api::GraphArtifactDto::Uml { graph, .. }
                    if graph.edges.iter().any(|edge| edge.id.0 == edge_id)
            )));
        }

        let mut obligation_plan =
            build_packet_obligation_plan(prompt, PacketTaskClassDto::RouteTracing, &queries);
        finalize_packet_obligation_plan(
            prompt,
            PacketTaskClassDto::RouteTracing,
            &mut obligation_plan,
            &answer,
            &complete_packet_budget(&answer),
        );
        for requirement_id in ["request_entrypoint", "request_dispatch", "request_terminal"] {
            let obligation = obligation_plan
                .claim_obligations
                .iter()
                .find(|obligation| obligation.id == requirement_id)
                .unwrap_or_else(|| panic!("missing {requirement_id} obligation"));
            assert_ne!(
                obligation.proof_status,
                PacketObligationProofStatusDto::Proven,
                "metrics/telemetry receiver context must not become proof: {obligation:?}"
            );
        }
    }

    #[test]
    fn raw_ownerless_unknown_receipt_stays_non_proven_on_all_early_paths() {
        let prompt = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        let mut hit = call_boundary_hit_with_receiver_owner(
            "raw-application-handle",
            "app.handle",
            "raw-handle",
            "handle",
            "raw-ownerless-handle",
            "src/application.js",
            "app.router",
        );
        hit.graph.as_mut().expect("graph").edges[0].callsite_identity = None;

        let mut raw_answer = empty_answer(prompt);
        raw_answer.citations = vec![hit.citation_for_requirements(true, &[])];
        raw_answer
            .graphs
            .push(codestory_contracts::api::GraphArtifactDto::Uml {
                id: "raw-early-path-context".into(),
                title: "Raw early-path context".into(),
                graph: hit.graph.clone().expect("graph"),
            });
        assert_eq!(
            raw_answer.citations[0].evidence_edge_ids,
            [EdgeId("raw-ownerless-handle".into())],
            "the finalizer must reject raw presentation context without a runtime sanitizer"
        );

        for path in ["tiny", "empty_query_batch", "latency_exhausted"] {
            let mut answer = raw_answer.clone();
            let mut budget = complete_packet_budget(&answer);
            match path {
                "tiny" => {
                    budget.requested = PacketBudgetModeDto::Tiny;
                    budget.limits.max_anchors = 3;
                    budget.limits.max_files = 3;
                    budget.limits.max_snippets = 6;
                    budget.limits.max_trail_edges = 12;
                    budget.limits.max_output_bytes = 24 * 1_024;
                }
                "empty_query_batch" => assert!(answer.retrieval_trace.steps.is_empty()),
                "latency_exhausted" => {
                    answer.retrieval_trace.total_latency_ms = 1_000;
                    answer.retrieval_trace.sla_target_ms = Some(1);
                    answer.retrieval_trace.sla_missed = true;
                }
                _ => unreachable!(),
            }

            let mut plan =
                build_packet_obligation_plan(prompt, PacketTaskClassDto::RouteTracing, &[]);
            finalize_packet_obligation_plan(
                prompt,
                PacketTaskClassDto::RouteTracing,
                &mut plan,
                &answer,
                &budget,
            );
            let obligation = plan
                .claim_obligations
                .iter()
                .find(|obligation| obligation.id == "request_dispatch")
                .expect("dispatch obligation");
            assert_ne!(
                obligation.proof_status,
                PacketObligationProofStatusDto::Proven,
                "{path} raw ownerless UNKNOWN became proof: {obligation:?}"
            );
        }
    }

    #[test]
    fn dense_dispatch_negatives_never_promote_or_finalize_as_proven() {
        let prompt = "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.";
        let terms = packet_probe_terms(prompt);
        let requirements =
            packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing);
        let requirement = requirements
            .iter()
            .find(|requirement| requirement.id == "request_dispatch")
            .expect("dispatch requirement");

        let wrong_owner = call_boundary_hit_with_receiver_owner(
            "wrong-owner",
            "app.handle",
            "telemetry-handle",
            "handle",
            "wrong-owner-edge",
            "src/application.js",
            "telemetry",
        );
        let mut confidence_only_wrong_owner = wrong_owner.clone();
        confidence_only_wrong_owner
            .graph
            .as_mut()
            .expect("graph")
            .edges[0]
            .confidence = Some(1.0);

        let mut wrong_target = call_boundary_hit_with_receiver_owner(
            "wrong-target",
            "app.handle",
            "metrics-record",
            "Metrics.record",
            "wrong-target-edge",
            "src/application.js",
            "app.router",
        );
        {
            let graph = wrong_target.graph.as_mut().expect("graph");
            graph.nodes[1].kind = NodeKind::METHOD;
            graph.edges[0].certainty = Some("certain".into());
            graph.edges[0].confidence = Some(1.0);
            wrong_target.graph_provenance[0].certainty = Some("certain".into());
        }

        let mut speculative = call_boundary_hit_with_receiver_owner(
            "speculative",
            "app.handle",
            "router-handle",
            "handle",
            "speculative-edge",
            "src/application.js",
            "app.router",
        );
        {
            let graph = speculative.graph.as_mut().expect("graph");
            graph.edges[0].certainty = Some("probable".into());
            graph.edges[0].confidence = None;
            speculative.graph_provenance[0].certainty = Some("probable".into());
        }

        let mut incoming = call_boundary_hit_with_receiver_owner(
            "incoming",
            "app.handle",
            "router-handle",
            "handle",
            "incoming-edge",
            "src/application.js",
            "app.router",
        );
        {
            let graph = incoming.graph.as_mut().expect("graph");
            graph.nodes[1].kind = NodeKind::METHOD;
            let edge = &mut graph.edges[0];
            edge.certainty = Some("certain".into());
            edge.confidence = Some(1.0);
            std::mem::swap(&mut edge.source, &mut edge.target);
            incoming.graph_provenance[0].direction = PacketGraphDirection::Incoming;
            incoming.graph_provenance[0].certainty = Some("certain".into());
        }

        let mut no_callsite = call_boundary_hit_with_receiver_owner(
            "no-callsite",
            "app.handle",
            "router-handle",
            "handle",
            "no-callsite-edge",
            "src/application.js",
            "app.router",
        );
        no_callsite.graph.as_mut().expect("graph").edges[0].callsite_identity = None;

        let preexisting_context = [
            ("resolved_wrong_target", wrong_target.clone()),
            ("resolved_incoming", incoming.clone()),
        ];

        for (shape, mut hit) in [
            ("wrong_owner", wrong_owner),
            ("confidence_only_wrong_owner", confidence_only_wrong_owner),
            ("wrong_target", wrong_target),
            ("speculative", speculative),
            ("incoming", incoming),
            ("no_callsite", no_callsite),
        ] {
            mark_dense_only(&mut hit);
            let query = PacketPlanQueryDto {
                query: "application handle".into(),
                purpose: "dispatch".into(),
            };
            let results = vec![(query.query.clone(), vec![hit])];
            let mut answer = empty_answer(prompt);
            merge_packet_fused_subquery_batch(
                &mut answer,
                &[(0, &query)],
                &results,
                1,
                &[],
                true,
                &terms,
                1,
                &requirements,
            );
            let citation = answer
                .citations
                .iter()
                .find(|citation| citation.display_name == "app.handle")
                .expect("generic fill retains the carrier");
            assert_eq!(
                citation.evidence_tier,
                Some(PacketEvidenceTierDto::DenseSemantic),
                "{shape} must not promote"
            );
            assert_eq!(citation.eligible_for_sufficiency, Some(false));
            assert!(citation.evidence_edge_ids.is_empty());
            assert!(
                !results[0].1[0].has_proof_call_provenance_for_requirement(citation, requirement)
            );

            let mut plan = build_packet_obligation_plan(
                prompt,
                PacketTaskClassDto::RouteTracing,
                std::slice::from_ref(&query),
            );
            finalize_packet_obligation_plan(
                prompt,
                PacketTaskClassDto::RouteTracing,
                &mut plan,
                &answer,
                &complete_packet_budget(&answer),
            );
            let obligation = plan
                .claim_obligations
                .iter()
                .find(|obligation| obligation.id == "request_dispatch")
                .expect("dispatch obligation");
            assert_ne!(
                obligation.proof_status,
                PacketObligationProofStatusDto::Proven,
                "{shape} became proof: {obligation:?}"
            );
        }

        for (shape, mut hit) in preexisting_context {
            mark_dense_only(&mut hit);
            let mut answer = empty_answer(prompt);
            answer.citations = vec![hit.citation_for_requirements(true, &[])];
            answer
                .graphs
                .push(codestory_contracts::api::GraphArtifactDto::Uml {
                    id: format!("{shape}-context"),
                    title: "preexisting packet context".into(),
                    graph: hit.graph.clone().expect("graph"),
                });
            assert_eq!(answer.citations[0].evidence_edge_ids.len(), 1);

            merge_packet_fused_subquery_batch(
                &mut answer,
                &[],
                &[],
                0,
                &[],
                true,
                &terms,
                1,
                &requirements,
            );
            assert_eq!(
                answer.citations[0].evidence_edge_ids.len(),
                1,
                "{shape} stays presentation context; the owning finalizer must reject it"
            );

            let mut plan =
                build_packet_obligation_plan(prompt, PacketTaskClassDto::RouteTracing, &[]);
            finalize_packet_obligation_plan(
                prompt,
                PacketTaskClassDto::RouteTracing,
                &mut plan,
                &answer,
                &complete_packet_budget(&answer),
            );
            let obligation = plan
                .claim_obligations
                .iter()
                .find(|obligation| obligation.id == "request_dispatch")
                .expect("dispatch obligation");
            assert_ne!(
                obligation.proof_status,
                PacketObligationProofStatusDto::Proven,
                "preexisting {shape} became proof: {obligation:?}"
            );
        }
    }
}
