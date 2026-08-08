use crate::trail::is_speculative_trail_edge;
use codestory_contracts::api::{AgentAnswerDto, EdgeKind, GraphArtifactDto, GraphResponse};

pub fn packet_execution_graphs(answer: &AgentAnswerDto) -> Vec<&GraphResponse> {
    answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .filter(|graph| {
            graph.edges.iter().any(|edge| {
                edge.kind == EdgeKind::CALL
                    && edge.source != edge.target
                    && !is_speculative_trail_edge(edge)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, EdgeId,
        GraphEdgeDto, NodeId,
    };

    fn edge(
        id: &str,
        kind: EdgeKind,
        certainty: Option<&str>,
        confidence: Option<f32>,
    ) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId("source".to_string()),
            target: NodeId("target".to_string()),
            kind,
            confidence,
            certainty: certainty.map(str::to_string),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    fn graph(id: &str, edges: Vec<GraphEdgeDto>) -> GraphArtifactDto {
        GraphArtifactDto::Uml {
            id: id.to_string(),
            title: id.to_string(),
            graph: GraphResponse {
                center_id: NodeId("source".to_string()),
                nodes: Vec::new(),
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }
    }

    fn answer(graphs: Vec<GraphArtifactDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "answer".to_string(),
            prompt: "question".to_string(),
            summary: "summary".to_string(),
            freshness: None,
            source_coverage: Vec::new(),
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs,
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "answer".to_string(),
                retrieval_publication: None,
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
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

    #[test]
    fn execution_graphs_require_a_non_speculative_non_self_call_edge() {
        let answer = answer(vec![
            graph(
                "qualified",
                vec![edge("certain", EdgeKind::CALL, Some("certain"), Some(0.85))],
            ),
            graph(
                "probable",
                vec![edge(
                    "probable",
                    EdgeKind::CALL,
                    Some("probable"),
                    Some(0.70),
                )],
            ),
            graph(
                "low-confidence",
                vec![edge("low", EdgeKind::CALL, Some("certain"), Some(0.54))],
            ),
            graph(
                "macro",
                vec![edge(
                    "macro",
                    EdgeKind::MACRO_USAGE,
                    Some("certain"),
                    Some(0.85),
                )],
            ),
        ]);

        let execution_graphs = packet_execution_graphs(&answer);

        assert_eq!(execution_graphs.len(), 1);
        assert_eq!(
            execution_graphs[0].edges[0].id,
            EdgeId("certain".to_string())
        );
    }
}
