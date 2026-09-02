//! Deterministic packet citation capping over admitted repository evidence.

use crate::agent::packet_scoring::{packet_citation_key, packet_display_path};
use codestory_contracts::api::{AgentAnswerDto, PacketBudgetLimitsDto};
use std::collections::HashSet;

/// Preserve exact typed selectors first, then sidecar retrieval order. Prefer
/// a first path witness before a second range from an already represented
/// path. No prompt text, evidence role, or answer shape participates.
pub(crate) fn cap_packet_citations_in_repository_order(
    answer: &mut AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
) -> bool {
    let original_len = answer.citations.len();
    let mut exact = Vec::new();
    let mut retrieval = Vec::new();
    for citation in std::mem::take(&mut answer.citations) {
        if citation
            .evidence_producer
            .as_deref()
            .is_some_and(|producer| producer.starts_with("packet_exact_"))
        {
            exact.push(citation);
        } else {
            retrieval.push(citation);
        }
    }

    let mut seen_identities = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut selected = Vec::new();
    let mut repeated_paths = Vec::new();
    for citation in exact.into_iter().chain(retrieval) {
        if !seen_identities.insert(packet_citation_key(&citation)) {
            continue;
        }
        let path = citation.file_path.as_deref().map(packet_display_path);
        let path_is_new = path.as_ref().is_none_or(|path| !seen_paths.contains(path));
        let exact_selector = citation
            .evidence_producer
            .as_deref()
            .is_some_and(|producer| producer.starts_with("packet_exact_"));
        if exact_selector || path_is_new {
            admit_citation(citation, path, limits, &mut selected, &mut seen_paths);
        } else {
            repeated_paths.push(citation);
        }
    }
    for citation in repeated_paths {
        let path = citation.file_path.as_deref().map(packet_display_path);
        admit_citation(citation, path, limits, &mut selected, &mut seen_paths);
    }
    answer.citations = selected;
    answer.citations.len() < original_len
}

fn admit_citation(
    citation: codestory_contracts::api::AgentCitationDto,
    path: Option<String>,
    limits: &PacketBudgetLimitsDto,
    selected: &mut Vec<codestory_contracts::api::AgentCitationDto>,
    seen_paths: &mut HashSet<String>,
) {
    if selected.len() >= limits.max_anchors as usize
        || path.as_ref().is_some_and(|path| {
            !seen_paths.contains(path) && seen_paths.len() >= limits.max_files as usize
        })
    {
        return;
    }
    if let Some(path) = path {
        seen_paths.insert(path);
    }
    selected.push(citation);
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentCitationDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
        AgentRetrievalTraceDto, NodeId, NodeKind, SearchHitOrigin,
    };

    fn answer(citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "answer".into(),
            prompt: "irrelevant wording".into(),
            summary: String::new(),
            freshness: None,
            sections: Vec::new(),
            citations,
            subgraph_ids: Vec::new(),
            retrieval_version: "test".into(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "request".into(),
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
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    fn citation(id: &str, path: &str, exact: bool) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(id.into()),
            display_name: id.into(),
            kind: NodeKind::FUNCTION,
            file_path: Some(path.into()),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: exact.then(|| "packet_exact_symbol_probe".into()),
            resolution_status: None,
            loss_reason: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
        }
    }

    #[test]
    fn exact_leads_and_distinct_paths_precede_repeats() {
        let mut answer = answer(vec![
            citation("repeat-a", "src/a.rs", false),
            citation("repeat-b", "src/a.rs", false),
            citation("other", "src/b.rs", false),
            citation("exact", "src/exact.rs", true),
        ]);
        cap_packet_citations_in_repository_order(
            &mut answer,
            &PacketBudgetLimitsDto {
                max_anchors: 3,
                max_files: 3,
                max_snippets: 3,
                max_trail_edges: 3,
                max_output_bytes: 16 * 1024,
            },
        );
        assert_eq!(
            answer
                .citations
                .iter()
                .map(|citation| citation.node_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["exact", "repeat-a", "other"]
        );
    }
}
