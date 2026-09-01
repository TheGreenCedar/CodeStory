//! Trace adapters that merge packet batch retrieval results into agent answers.

#![allow(clippy::items_after_test_module)]

use super::packet_candidate::{PacketSearchHit, merge_packet_candidate_graph};
use super::packet_scoring::{
    normalize_identifier, packet_citation_key, packet_citation_rank, sort_by_cached_rank_desc,
};
use super::trace::field;
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
) -> usize {
    merge_packet_search_hits(
        answer,
        hits,
        include_evidence,
        rank_terms,
        stage_carry_limit,
        None,
    )
}

fn merge_packet_search_hits(
    answer: &mut AgentAnswerDto,
    hits: &[PacketSearchHit],
    include_evidence: bool,
    rank_terms: &[String],
    stage_carry_limit: usize,
    exact_query: Option<&str>,
) -> usize {
    let mut citation_indices = answer
        .citations
        .iter()
        .enumerate()
        .map(|(index, citation)| (packet_citation_key(citation), index))
        .collect::<HashMap<_, _>>();
    let mut candidates = hits
        .iter()
        .map(|hit| (hit.citation(include_evidence), hit))
        .collect::<Vec<_>>();
    sort_by_cached_rank_desc(&mut candidates, |(citation, _)| {
        packet_citation_rank(citation, rank_terms, true)
    });
    let selected = select_packet_candidate_indices(&candidates, stage_carry_limit, exact_query);
    for candidate_index in &selected {
        let (citation, hit) = &candidates[*candidate_index];
        if include_evidence {
            merge_packet_candidate_graph(answer, hit);
        }
        let key = packet_citation_key(citation);
        if let Some(existing_index) = citation_indices.get(&key).copied() {
            merge_packet_citation_provenance(&mut answer.citations[existing_index], citation, &[]);
        } else {
            let citation_index = answer.citations.len();
            citation_indices.insert(key, citation_index);
            answer.citations.push(citation.clone());
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
            (query.purpose == PACKET_OWNER_MEMBER_QUERY_PURPOSE).then_some(query.query.as_str()),
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
    use crate::agent::packet_candidate::{PacketGraphDirection, PacketGraphEdgeProvenance};
    use codestory_contracts::api::{
        AgentAnswerDto, AgentRetrievalTraceDto, EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto,
        GraphResponse, NodeId, NodeKind, PacketEvidenceResolutionDto, PacketEvidenceTierDto,
        PacketPlanQueryDto, RetrievalScoreBreakdownDto, SearchHit, SearchHitOrigin,
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
        );

        assert_eq!(answer.citations.len(), 1);
        assert_eq!(answer.citations[0].display_name, "BaseRequest.finalize");
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
}
