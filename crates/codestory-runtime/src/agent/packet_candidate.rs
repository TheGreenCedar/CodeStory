//! Runtime-only packet candidates that keep graph proof beside public search hits.

use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, EdgeId, EdgeKind, GraphArtifactDto, GraphResponse, SearchHit,
};
use std::collections::HashSet;
use std::ops::Deref;

const PACKET_SEARCH_PROVENANCE_GRAPH_ID: &str = "packet-search-provenance";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacketGraphDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketGraphEdgeProvenance {
    pub(crate) edge_id: EdgeId,
    pub(crate) direction: PacketGraphDirection,
    pub(crate) hop: u32,
    pub(crate) producers: Vec<String>,
    pub(crate) certainty: Option<String>,
}

/// A packet-only search result. Public search DTOs stay unchanged while exact graph proof remains
/// attached until the packet citation and graph artifact are assembled.
#[derive(Debug, Clone)]
pub(crate) struct PacketSearchHit {
    pub(crate) hit: SearchHit,
    pub(crate) graph_provenance: Vec<PacketGraphEdgeProvenance>,
    pub(crate) graph: Option<GraphResponse>,
}

impl PacketSearchHit {
    #[cfg(test)]
    pub(crate) fn without_graph(hit: SearchHit) -> Self {
        Self {
            hit,
            graph_provenance: Vec::new(),
            graph: None,
        }
    }

    pub(crate) fn citation(&self, include_evidence: bool) -> AgentCitationDto {
        let mut citation = codestory_agent::citation::to_citation_from_hit(
            &self.hit,
            None,
            None,
            include_evidence,
        );
        if include_evidence && self.hit.resolvable {
            citation.evidence_edge_ids.extend(
                self.graph_provenance
                    .iter()
                    .map(|provenance| provenance.edge_id.clone()),
            );
            citation
                .evidence_edge_ids
                .sort_by(|left, right| left.0.cmp(&right.0));
            citation.evidence_edge_ids.dedup();
            citation.evidence_edge_ids.truncate(12);
        }
        citation
    }

    pub(crate) fn has_certain_call_provenance(&self) -> bool {
        let Some(graph) = self.graph.as_ref() else {
            return false;
        };
        self.graph_provenance.iter().any(|provenance| {
            provenance.certainty.as_deref() == Some("certain")
                && graph
                    .edges
                    .iter()
                    .any(|edge| edge.id == provenance.edge_id && edge.kind == EdgeKind::CALL)
        })
    }
}

impl Deref for PacketSearchHit {
    type Target = SearchHit;

    fn deref(&self) -> &Self::Target {
        &self.hit
    }
}

/// Merge only the graph rows belonging to a retained packet candidate. Existing artifacts win by
/// edge ID, so a candidate cannot duplicate proof that the initial neighborhood already carries.
pub(crate) fn merge_packet_candidate_graph(answer: &mut AgentAnswerDto, hit: &PacketSearchHit) {
    let Some(candidate_graph) = hit.graph.as_ref() else {
        return;
    };
    let existing_edge_ids = answer
        .graphs
        .iter()
        .filter_map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => Some(graph.edges.iter()),
            GraphArtifactDto::Mermaid { .. } => None,
        })
        .flatten()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();
    let missing_edges = candidate_graph
        .edges
        .iter()
        .filter(|edge| !existing_edge_ids.contains(&edge.id))
        .cloned()
        .collect::<Vec<_>>();
    if missing_edges.is_empty() {
        return;
    }

    let mut needed_node_ids = missing_edges
        .iter()
        .flat_map(|edge| [edge.source.clone(), edge.target.clone()])
        .collect::<HashSet<_>>();
    needed_node_ids.insert(candidate_graph.center_id.clone());
    let missing_nodes = candidate_graph
        .nodes
        .iter()
        .filter(|node| needed_node_ids.contains(&node.id))
        .cloned()
        .collect::<Vec<_>>();

    if let Some(GraphArtifactDto::Uml { graph, .. }) = answer.graphs.iter_mut().find(|artifact| {
        matches!(
            artifact,
            GraphArtifactDto::Uml { id, .. } if id == PACKET_SEARCH_PROVENANCE_GRAPH_ID
        )
    }) {
        let mut node_ids = graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        graph.nodes.extend(
            missing_nodes
                .into_iter()
                .filter(|node| node_ids.insert(node.id.clone())),
        );
        graph.edges.extend(missing_edges);
        graph
            .edges
            .sort_by(|left, right| left.id.0.cmp(&right.id.0));
        graph.canonical_layout = None;
    } else {
        answer.graphs.push(GraphArtifactDto::Uml {
            id: PACKET_SEARCH_PROVENANCE_GRAPH_ID.to_string(),
            title: "Packet search graph provenance".to_string(),
            graph: GraphResponse {
                center_id: candidate_graph.center_id.clone(),
                nodes: missing_nodes,
                edges: missing_edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });
    }
    if !answer
        .subgraph_ids
        .iter()
        .any(|id| id == PACKET_SEARCH_PROVENANCE_GRAPH_ID)
    {
        answer
            .subgraph_ids
            .push(PACKET_SEARCH_PROVENANCE_GRAPH_ID.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentRetrievalTraceDto, GraphEdgeDto, GraphNodeDto, NodeId, NodeKind, SearchHitOrigin,
    };

    fn answer() -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "answer".into(),
            prompt: "prompt".into(),
            summary: "summary".into(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "sidecar".into(),
            graphs: Vec::new(),
            source_coverage: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "r".into(),
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

    fn packet_hit(edge_id: &str) -> PacketSearchHit {
        let node_id = NodeId("2".into());
        PacketSearchHit {
            hit: SearchHit {
                node_id: node_id.clone(),
                display_name: "Session.send".into(),
                kind: NodeKind::METHOD,
                file_path: Some("requests/sessions.py".into()),
                line: Some(1),
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
            },
            graph_provenance: vec![PacketGraphEdgeProvenance {
                edge_id: EdgeId(edge_id.into()),
                direction: PacketGraphDirection::Incoming,
                hop: 1,
                producers: vec!["scip_graph_projection".into()],
                certainty: Some("certain".into()),
            }],
            graph: Some(GraphResponse {
                center_id: node_id.clone(),
                nodes: [("1", "Session.request"), ("2", "Session.send")]
                    .into_iter()
                    .map(|(id, label)| GraphNodeDto {
                        id: NodeId(id.into()),
                        label: label.into(),
                        kind: NodeKind::METHOD,
                        depth: u32::from(id != "2"),
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: None,
                        qualified_name: None,
                        member_access: None,
                    })
                    .collect(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId(edge_id.into()),
                    source: NodeId("1".into()),
                    target: node_id,
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
        }
    }

    #[test]
    fn citation_and_graph_keep_exact_packet_candidate_provenance() {
        let hit = packet_hit("edge-1");
        let citation = hit.citation(true);
        assert_eq!(citation.evidence_edge_ids, [EdgeId("edge-1".into())]);
        assert!(hit.has_certain_call_provenance());

        let mut answer = answer();
        merge_packet_candidate_graph(&mut answer, &hit);
        merge_packet_candidate_graph(&mut answer, &hit);
        let GraphArtifactDto::Uml { graph, .. } = &answer.graphs[0] else {
            panic!("expected UML graph");
        };
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(answer.subgraph_ids, [PACKET_SEARCH_PROVENANCE_GRAPH_ID]);
    }
}
