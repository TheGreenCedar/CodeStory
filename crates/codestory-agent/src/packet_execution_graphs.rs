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
