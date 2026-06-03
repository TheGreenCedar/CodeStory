//! Citation construction from search hits for agent retrieval.

use codestory_contracts::api::{AgentCitationDto, EdgeId, GraphResponse, NodeId, SearchHit};

pub(crate) fn to_citation_from_hit(
    hit: &SearchHit,
    subgraph_id: Option<&str>,
    primary_graph: Option<&GraphResponse>,
    include_evidence: bool,
) -> AgentCitationDto {
    AgentCitationDto {
        node_id: hit.node_id.clone(),
        display_name: hit.display_name.clone(),
        kind: hit.kind,
        file_path: hit.file_path.clone(),
        line: hit.line,
        score: hit.score,
        origin: hit.origin,
        resolvable: hit.resolvable,
        subgraph_id: subgraph_id.map(ToOwned::to_owned),
        evidence_edge_ids: if include_evidence && hit.resolvable {
            evidence_edge_ids_for_node(primary_graph, &hit.node_id)
        } else {
            Vec::new()
        },
        retrieval_score_breakdown: include_evidence
            .then(|| hit.score_breakdown.clone())
            .flatten(),
    }
}

pub(crate) fn evidence_edge_ids_for_node(
    primary_graph: Option<&GraphResponse>,
    node_id: &NodeId,
) -> Vec<EdgeId> {
    let Some(graph) = primary_graph else {
        return Vec::new();
    };

    let mut edge_ids = graph
        .edges
        .iter()
        .filter(|edge| edge.source == *node_id || edge.target == *node_id)
        .map(|edge| edge.id.clone())
        .collect::<Vec<_>>();
    edge_ids.sort_by(|left, right| left.0.cmp(&right.0));
    edge_ids.truncate(12);
    edge_ids
}
