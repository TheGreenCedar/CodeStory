//! Runtime-only packet candidates that keep graph proof beside public search hits.

use codestory_agent::packet_flow_requirements::{
    FlowRequirement, flow_requirement_call_receipt_is_valid,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, EdgeId, EdgeKind, GraphArtifactDto, GraphResponse, SearchHit,
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ops::Deref;

const PACKET_CANDIDATE_SELECTION_VIEW_ID: &str = "packet-search-provenance";
const PACKET_CANDIDATE_SELECTION_VIEW_ID_PREFIX: &str = "packet-search-provenance-";
const PACKET_CANDIDATE_GRAPH_EDGE_LIMIT: usize = 20;
const PACKET_CITATION_EDGE_LIMIT: usize = 12;

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

    #[cfg(test)]
    pub(crate) fn citation(&self, include_evidence: bool) -> AgentCitationDto {
        self.citation_for_requirements(include_evidence, &[])
    }

    pub(crate) fn citation_for_requirements(
        &self,
        include_evidence: bool,
        flow_requirements: &[FlowRequirement],
    ) -> AgentCitationDto {
        let citation = codestory_agent::citation::to_citation_from_hit(
            &self.hit,
            None,
            None,
            include_evidence,
        );
        self.citation_for_requirements_from_base(citation, include_evidence, flow_requirements)
    }

    fn citation_for_requirements_from_base(
        &self,
        mut citation: AgentCitationDto,
        include_evidence: bool,
        flow_requirements: &[FlowRequirement],
    ) -> AgentCitationDto {
        if include_evidence && self.hit.resolvable {
            let proof_edge_ids = self.proof_edge_ids_for_requirements(&citation, flow_requirements);
            citation.evidence_edge_ids = self.selected_edge_ids_for_requirements(
                &citation,
                flow_requirements,
                PACKET_CITATION_EDGE_LIMIT,
            );
            if !proof_edge_ids.is_empty()
                && citation.evidence_tier
                    == Some(codestory_contracts::api::PacketEvidenceTierDto::DenseSemantic)
                && self.hit.resolvable
                && citation.file_path.is_some()
                && citation.line.is_some()
            {
                // The candidate is no longer dense-only: the exact carrier now owns a strict,
                // receiver-aware parser receipt. Publish that stronger lane atomically so a
                // duplicate dense anchor cannot keep the carrier ineligible.
                citation.evidence_tier =
                    Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph);
                citation.evidence_producer = Some("core_incident_call".to_string());
                citation.eligible_for_sufficiency = Some(true);
                if let Some(breakdown) = citation.retrieval_score_breakdown.as_mut() {
                    breakdown.graph = breakdown.graph.max(breakdown.total);
                    breakdown.tier_cap = None;
                    breakdown.dampening.retain(|reason| reason != "dense_only");
                    if !breakdown
                        .provenance
                        .iter()
                        .any(|producer| producer == "core_incident_call")
                    {
                        breakdown.provenance.push("core_incident_call".to_string());
                    }
                    breakdown.final_rank_reason =
                        Some("receiver-aware parser CALL receipt".to_string());
                }
            }
        }
        citation
    }

    pub(crate) fn has_proof_call_provenance_for_requirement(
        &self,
        citation: &AgentCitationDto,
        requirement: &FlowRequirement,
    ) -> bool {
        !self
            .proof_edge_ids_for_requirement(citation, requirement)
            .is_empty()
    }

    #[cfg(test)]
    pub(crate) fn has_proof_call_provenance(&self) -> bool {
        let Some(graph) = self.graph.as_ref() else {
            return false;
        };
        let provenance_ids = self
            .graph_provenance
            .iter()
            .map(|provenance| &provenance.edge_id)
            .collect::<HashSet<_>>();
        graph.edges.iter().any(|edge| {
            provenance_ids.contains(&edge.id)
                && edge.kind == EdgeKind::CALL
                && (edge.certainty.as_deref() == Some("certain")
                    || (edge.certainty.is_none()
                        && edge.confidence.is_none()
                        && edge.callsite_identity.as_deref().is_some_and(|identity| {
                            identity.contains("|receiver-owner:")
                                && identity.split('|').any(|segment| {
                                    segment.starts_with("syntax:") && segment.ends_with("-call")
                                })
                        })))
        })
    }

    pub(crate) fn proof_edge_ids_for_requirements(
        &self,
        citation: &AgentCitationDto,
        flow_requirements: &[FlowRequirement],
    ) -> Vec<EdgeId> {
        let mut selected = Vec::new();
        for requirement in flow_requirements
            .iter()
            .filter(|requirement| requirement.evidence.citation_proves(citation))
        {
            if let Some(edge_id) = self
                .proof_edge_ids_for_requirement(citation, requirement)
                .into_iter()
                .next()
                && !selected.contains(&edge_id)
            {
                selected.push(edge_id);
            }
        }
        selected
    }

    fn selected_edge_ids_for_requirements(
        &self,
        citation: &AgentCitationDto,
        flow_requirements: &[FlowRequirement],
        limit: usize,
    ) -> Vec<EdgeId> {
        let mut selected = self.proof_edge_ids_for_requirements(citation, flow_requirements);
        let selected_set = selected.iter().cloned().collect::<HashSet<_>>();
        let Some(graph) = self.graph.as_ref() else {
            return selected;
        };
        let admissible_call_ids = flow_requirements
            .iter()
            .filter(|requirement| requirement.evidence.citation_proves(citation))
            .flat_map(|requirement| {
                self.proof_edge_ids_for_requirement(citation, requirement)
                    .into_iter()
            })
            .collect::<HashSet<_>>();
        let has_applicable_call_requirement = flow_requirements.iter().any(|requirement| {
            requirement.evidence.citation_proves(citation)
                && (requirement
                    .evidence
                    .call_boundary_target(citation)
                    .is_some()
                    || requirement
                        .evidence
                        .ordered_call_boundary(citation)
                        .is_some())
        });
        let graph_edges = graph
            .edges
            .iter()
            .map(|edge| (&edge.id, edge))
            .collect::<std::collections::HashMap<_, _>>();
        let mut context = self
            .graph_provenance
            .iter()
            .map(|provenance| provenance.edge_id.clone())
            .filter(|edge_id| {
                graph_edges.get(edge_id).is_some_and(|edge| {
                    !selected_set.contains(edge_id)
                        && (edge.kind != EdgeKind::CALL
                            || !has_applicable_call_requirement
                            || admissible_call_ids.contains(edge_id))
                })
            })
            .collect::<Vec<_>>();
        context.sort_by(|left, right| left.0.cmp(&right.0));
        context.dedup();
        selected.extend(context);
        selected.dedup();
        selected.truncate(limit);
        selected
    }

    fn graph_for_requirements(
        &self,
        citation: &AgentCitationDto,
        flow_requirements: &[FlowRequirement],
    ) -> Option<GraphResponse> {
        let graph = self.graph.as_ref()?;
        let mut selected_edge_ids =
            self.proof_edge_ids_for_requirements(citation, flow_requirements);
        let selected_set = selected_edge_ids.iter().cloned().collect::<HashSet<_>>();
        let graph_edge_ids = graph
            .edges
            .iter()
            .map(|edge| &edge.id)
            .collect::<HashSet<_>>();
        let mut context = self
            .graph_provenance
            .iter()
            .map(|provenance| provenance.edge_id.clone())
            .filter(|edge_id| graph_edge_ids.contains(edge_id) && !selected_set.contains(edge_id))
            .collect::<Vec<_>>();
        context.sort_by(|left, right| left.0.cmp(&right.0));
        context.dedup();
        selected_edge_ids.extend(context);
        selected_edge_ids.dedup();
        selected_edge_ids.truncate(PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        let selected_order = selected_edge_ids
            .iter()
            .enumerate()
            .map(|(index, edge_id)| (edge_id, index))
            .collect::<std::collections::HashMap<_, _>>();
        let mut edges = graph
            .edges
            .iter()
            .filter(|edge| selected_order.contains_key(&edge.id))
            .cloned()
            .collect::<Vec<_>>();
        edges.sort_by_key(|edge| selected_order[&edge.id]);
        if edges.is_empty() {
            return None;
        }

        let retained_node_ids = edges
            .iter()
            .flat_map(|edge| [edge.source.clone(), edge.target.clone()])
            .chain(std::iter::once(graph.center_id.clone()))
            .collect::<HashSet<_>>();
        let nodes = graph
            .nodes
            .iter()
            .filter(|node| retained_node_ids.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>();
        let candidate_omitted = graph.edges.len().saturating_sub(edges.len());
        Some(GraphResponse {
            center_id: graph.center_id.clone(),
            nodes,
            edges,
            truncated: graph.truncated || candidate_omitted > 0,
            omitted_edge_count: graph
                .omitted_edge_count
                .saturating_add(u32::try_from(candidate_omitted).unwrap_or(u32::MAX)),
            canonical_layout: None,
        })
    }

    fn proof_edge_ids_for_requirement(
        &self,
        citation: &AgentCitationDto,
        requirement: &FlowRequirement,
    ) -> Vec<EdgeId> {
        let Some(graph) = self.graph.as_ref() else {
            return Vec::new();
        };
        let provenance_ids = self
            .graph_provenance
            .iter()
            .map(|provenance| &provenance.edge_id)
            .collect::<HashSet<_>>();
        let mut matches = graph
            .edges
            .iter()
            .filter(|edge| provenance_ids.contains(&edge.id))
            .filter(|edge| {
                receipt_neighbor(graph, citation, edge).is_some_and(|(label, kind)| {
                    flow_requirement_call_receipt_is_valid(requirement, citation, edge, label, kind)
                })
            })
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| left.0.cmp(&right.0));
        matches.dedup();
        matches
    }
}

fn receipt_neighbor<'a>(
    graph: &'a GraphResponse,
    citation: &AgentCitationDto,
    edge: &codestory_contracts::api::GraphEdgeDto,
) -> Option<(&'a str, codestory_contracts::api::NodeKind)> {
    let neighbor_id = if edge.source == citation.node_id {
        &edge.target
    } else if edge.target == citation.node_id {
        &edge.source
    } else {
        return None;
    };
    graph
        .nodes
        .iter()
        .find(|node| node.id == *neighbor_id)
        .map(|node| (node.label.as_str(), node.kind))
}

impl Deref for PacketSearchHit {
    type Target = SearchHit;

    fn deref(&self) -> &Self::Target {
        &self.hit
    }
}

/// Preserve one capped candidate view as one graph artifact. `GraphResponse::omitted_edge_count`
/// is artifact-local: it describes only the bounded source view fingerprinted into this artifact
/// and must not be summed across candidate artifacts. The ID is stable lineage for that original
/// view; a later output cap may remove a known retained edge and increment the local count without
/// changing lineage. Keeping overlapping views separate avoids inventing union arithmetic for
/// opaque omissions whose edge identities are unavailable.
#[cfg(test)]
pub(crate) fn merge_packet_candidate_graph(answer: &mut AgentAnswerDto, hit: &PacketSearchHit) {
    merge_packet_candidate_graph_for_requirements(answer, hit, &[]);
}

pub(crate) fn merge_packet_candidate_graph_for_requirements(
    answer: &mut AgentAnswerDto,
    hit: &PacketSearchHit,
    flow_requirements: &[FlowRequirement],
) {
    let citation = hit.citation_for_requirements(true, flow_requirements);
    let Some(candidate_graph) = hit.graph_for_requirements(&citation, flow_requirements) else {
        return;
    };
    let artifact_id = packet_candidate_selection_view_id(&candidate_graph);
    if !answer.graphs.iter().any(|artifact| match artifact {
        GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => {
            id == &artifact_id
        }
    }) {
        answer.graphs.push(GraphArtifactDto::Uml {
            id: artifact_id.clone(),
            title: "Packet search graph provenance".to_string(),
            graph: candidate_graph,
        });
    }
    if !answer.subgraph_ids.contains(&artifact_id) {
        answer.subgraph_ids.push(artifact_id);
    }
}

/// Immutable identity of the original bounded selection view, computed before any downstream
/// presentation cap. This is lineage, not a checksum of the graph's current serialized rows: a
/// later output cap may remove known rows and increase that view's omission count while the ID
/// remains stable. Replaying the same candidate therefore finds the existing lineage and must not
/// restore optional rows deliberately removed to meet the packet budget.
fn packet_candidate_selection_view_id(graph: &GraphResponse) -> String {
    let mut edge_ids = graph
        .edges
        .iter()
        .map(|edge| edge.id.0.as_str())
        .collect::<Vec<_>>();
    edge_ids.sort_unstable();

    let mut digest = Sha256::new();
    hash_graph_id_component(&mut digest, "immutable-candidate-selection-view-v1");
    hash_graph_id_component(&mut digest, &graph.center_id.0);
    for edge_id in edge_ids {
        hash_graph_id_component(&mut digest, edge_id);
    }
    digest.update([u8::from(graph.truncated)]);
    digest.update(graph.omitted_edge_count.to_le_bytes());
    let fingerprint = digest.finalize();
    format!("{PACKET_CANDIDATE_SELECTION_VIEW_ID}-{fingerprint:x}")
}

pub(crate) fn is_packet_candidate_selection_view_id(id: &str) -> bool {
    id.strip_prefix(PACKET_CANDIDATE_SELECTION_VIEW_ID_PREFIX)
        .is_some_and(|fingerprint| {
            fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn hash_graph_id_component(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_budget::cap_packet_graph_edges_for_test;
    use codestory_agent::packet_flow_requirements::packet_flow_requirements_for_terms;
    use codestory_agent::packet_terms::packet_probe_terms;
    use codestory_contracts::api::{
        AgentRetrievalTraceDto, GraphEdgeDto, GraphNodeDto, NodeId, NodeKind,
        PacketEvidenceResolutionDto, PacketEvidenceTierDto, PacketTaskClassDto, SearchHitOrigin,
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

    fn server_requirement(id: &str) -> FlowRequirement {
        let terms = packet_probe_terms(
            "Trace how a server application registers middleware, handles a request, and sends the response.",
        );
        packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing)
            .into_iter()
            .find(|requirement| requirement.id == id)
            .unwrap_or_else(|| panic!("missing server requirement {id}"))
    }

    fn boundary_hit(
        carrier: &str,
        target_label: &str,
        callsite_identity: Option<&str>,
        certainty: Option<&str>,
        outgoing: bool,
    ) -> PacketSearchHit {
        let center_id = NodeId("carrier".into());
        let neighbor_id = NodeId("neighbor".into());
        let (source, target) = if outgoing {
            (center_id.clone(), neighbor_id.clone())
        } else {
            (neighbor_id.clone(), center_id.clone())
        };
        PacketSearchHit {
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: carrier.into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/server.js".into()),
                line: Some(10),
                score: 0.8,
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
                score_breakdown: None,
            },
            graph_provenance: vec![PacketGraphEdgeProvenance {
                edge_id: EdgeId("boundary".into()),
                direction: if outgoing {
                    PacketGraphDirection::Outgoing
                } else {
                    PacketGraphDirection::Incoming
                },
                hop: 1,
                producers: vec!["core_incident_call".into()],
                certainty: certainty.map(str::to_string),
            }],
            graph: Some(GraphResponse {
                center_id: center_id.clone(),
                nodes: vec![
                    GraphNodeDto {
                        id: center_id,
                        label: carrier.into(),
                        kind: NodeKind::METHOD,
                        depth: 0,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/server.js".into()),
                        qualified_name: Some(carrier.into()),
                        member_access: None,
                    },
                    GraphNodeDto {
                        id: neighbor_id,
                        label: target_label.into(),
                        kind: if certainty == Some("certain") {
                            NodeKind::METHOD
                        } else {
                            NodeKind::UNKNOWN
                        },
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/server.js".into()),
                        qualified_name: None,
                        member_access: None,
                    },
                ],
                edges: vec![GraphEdgeDto {
                    id: EdgeId("boundary".into()),
                    source,
                    target,
                    kind: EdgeKind::CALL,
                    confidence: certainty.map(|_| 1.0),
                    certainty: certainty.map(str::to_string),
                    callsite_identity: callsite_identity.map(str::to_string),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        }
    }

    fn overlapping_candidate_hit(
        center: &str,
        edge_specs: &[(&str, &str, &str)],
        omitted_edge_count: u32,
    ) -> PacketSearchHit {
        let center_id = NodeId(center.into());
        let mut node_ids = edge_specs
            .iter()
            .flat_map(|(_, source, target)| [*source, *target])
            .collect::<Vec<_>>();
        node_ids.sort_unstable();
        node_ids.dedup();
        let edges = edge_specs
            .iter()
            .map(|(id, source, target)| GraphEdgeDto {
                id: EdgeId((*id).into()),
                source: NodeId((*source).into()),
                target: NodeId((*target).into()),
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".into()),
                callsite_identity: None,
                candidate_targets: Vec::new(),
            })
            .collect::<Vec<_>>();
        PacketSearchHit {
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: center.into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/overlap.js".into()),
                line: Some(1),
                score: 0.8,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
                evidence_producer: Some("core_incident_call".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance: edges
                .iter()
                .map(|edge| PacketGraphEdgeProvenance {
                    edge_id: edge.id.clone(),
                    direction: if edge.source == center_id {
                        PacketGraphDirection::Outgoing
                    } else {
                        PacketGraphDirection::Incoming
                    },
                    hop: 1,
                    producers: vec!["core_incident_call".into()],
                    certainty: edge.certainty.clone(),
                })
                .collect(),
            graph: Some(GraphResponse {
                center_id: center_id.clone(),
                nodes: node_ids
                    .into_iter()
                    .map(|id| GraphNodeDto {
                        id: NodeId(id.into()),
                        label: id.into(),
                        kind: NodeKind::METHOD,
                        depth: u32::from(id != center),
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/overlap.js".into()),
                        qualified_name: Some(id.into()),
                        member_access: None,
                    })
                    .collect(),
                edges,
                truncated: omitted_edge_count > 0,
                omitted_edge_count,
                canonical_layout: None,
            }),
        }
    }

    #[test]
    fn citation_and_graph_keep_exact_packet_candidate_provenance() {
        let hit = packet_hit("edge-1");
        let citation = hit.citation(true);
        assert_eq!(citation.evidence_edge_ids, [EdgeId("edge-1".into())]);
        assert!(hit.has_proof_call_provenance());

        let mut answer = answer();
        merge_packet_candidate_graph(&mut answer, &hit);
        merge_packet_candidate_graph(&mut answer, &hit);
        let GraphArtifactDto::Uml { id, graph, .. } = &answer.graphs[0] else {
            panic!("expected UML graph");
        };
        assert_eq!(answer.graphs.len(), 1, "exact replay must be idempotent");
        assert_eq!(graph.edges.len(), 1);
        assert!(id.starts_with(PACKET_CANDIDATE_SELECTION_VIEW_ID));
        assert_eq!(answer.subgraph_ids, [id.clone()]);
    }

    #[test]
    fn syntax_only_call_proof_requires_the_requirement_receiver_owner() {
        for (requirement_id, carrier, target, receiver_owner) in [
            ("request_dispatch", "app.handle", "handle", "app.router"),
            ("request_entrypoint", "app.route", "route", "app.router"),
            ("request_terminal", "res.send", "end", "res"),
            ("request_terminal", "reply.send", "finish", "reply"),
        ] {
            let requirement = server_requirement(requirement_id);
            let identity = format!(
                "src/server.js:10:1:20|syntax:js-member-call|receiver-owner:{receiver_owner}"
            );
            let hit = boundary_hit(carrier, target, Some(&identity), None, true);
            let citation = hit.citation_for_requirements(true, &[requirement]);
            let graph = hit.graph.as_ref().expect("graph");
            let edge = &graph.edges[0];
            let (neighbor_label, neighbor_kind) =
                receipt_neighbor(graph, &citation, edge).expect("neighbor");
            assert!(
                flow_requirement_call_receipt_is_valid(
                    &requirement,
                    &citation,
                    edge,
                    neighbor_label,
                    neighbor_kind,
                ),
                "{receiver_owner}.{target} must prove {requirement_id}"
            );
            assert!(hit.has_proof_call_provenance_for_requirement(&citation, &requirement));
            assert_eq!(citation.evidence_edge_ids[0], EdgeId("boundary".into()));
        }

        for (requirement_id, carrier, target, receiver_owner) in [
            ("request_entrypoint", "app.use", "use", "Metrics"),
            ("request_dispatch", "app.handle", "handle", "Telemetry"),
            ("request_terminal", "res.send", "end", "Telemetry"),
            ("request_terminal", "res.send", "write", "Cache"),
        ] {
            let requirement = server_requirement(requirement_id);
            let identity = format!(
                "src/server.js:10:1:20|syntax:js-member-call|receiver-owner:{receiver_owner}"
            );
            let hit = boundary_hit(carrier, target, Some(&identity), None, true);
            let citation = hit.citation_for_requirements(true, &[requirement]);
            assert!(
                !hit.has_proof_call_provenance_for_requirement(&citation, &requirement),
                "{receiver_owner}.{target} must not prove {requirement_id}"
            );
            assert!(
                citation.evidence_edge_ids.is_empty(),
                "owner-invalid unresolved CALLs must not leak back as citation context"
            );
        }
    }

    #[test]
    fn dense_only_carrier_promotes_only_with_a_strict_requirement_proof() {
        let requirement = server_requirement("request_entrypoint");
        let mut lawful = boundary_hit(
            "app.route",
            "route",
            Some("src/server.js:10|syntax:js-member-call|receiver-owner:app.router"),
            None,
            true,
        );
        lawful.hit.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        lawful.hit.evidence_producer = Some("dense_anchor".into());
        lawful.hit.eligible_for_sufficiency = Some(false);
        lawful.hit.score_breakdown = Some(codestory_contracts::api::RetrievalScoreBreakdownDto {
            lexical: 0.0,
            semantic: 0.8,
            graph: 0.0,
            total: 0.8,
            tier_cap: Some(0.4),
            boosts: Vec::new(),
            dampening: vec!["dense_only".into()],
            final_rank_reason: Some("dense anchor".into()),
            provenance: vec!["dense_anchor".into()],
        });
        assert!(
            lawful
                .proof_edge_ids_for_requirement(
                    &codestory_agent::citation::to_citation_from_hit(
                        &lawful.hit,
                        None,
                        None,
                        true,
                    ),
                    &requirement,
                )
                .contains(&EdgeId("boundary".into()))
        );
        let base = codestory_agent::citation::to_citation_from_hit(&lawful.hit, None, None, true);
        let promoted = lawful.citation_for_requirements_from_base(
            base,
            true,
            std::slice::from_ref(&requirement),
        );
        assert_eq!(
            promoted.evidence_tier,
            Some(PacketEvidenceTierDto::ResolvedGraph)
        );
        assert_eq!(
            promoted.evidence_producer.as_deref(),
            Some("core_incident_call")
        );
        assert_eq!(promoted.eligible_for_sufficiency, Some(true));
        assert_eq!(promoted.evidence_edge_ids, [EdgeId("boundary".into())]);
        let breakdown = promoted
            .retrieval_score_breakdown
            .as_ref()
            .expect("promoted score breakdown");
        assert_eq!(breakdown.graph, 0.8);
        assert_eq!(breakdown.tier_cap, None);
        assert!(
            !breakdown
                .dampening
                .iter()
                .any(|reason| reason == "dense_only")
        );
        assert!(
            breakdown
                .provenance
                .iter()
                .any(|producer| producer == "core_incident_call")
        );

        let mut explicit_probable =
            boundary_hit("app.route", "route", None, Some("probable"), true);
        explicit_probable.graph.as_mut().expect("graph").edges[0].confidence = None;
        let negative_shapes = [
            boundary_hit(
                "app.route",
                "route",
                Some("src/server.js:10|syntax:js-member-call|receiver-owner:metrics"),
                None,
                true,
            ),
            boundary_hit(
                "app.route",
                "record",
                Some("src/server.js:10|syntax:js-member-call|receiver-owner:app.router"),
                None,
                true,
            ),
            explicit_probable,
            boundary_hit(
                "app.route",
                "route",
                Some("src/server.js:10|syntax:js-member-call|receiver-owner:app.router"),
                None,
                false,
            ),
            boundary_hit("app.route", "route", None, None, true),
        ];
        for mut negative in negative_shapes {
            negative.hit.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
            negative.hit.evidence_producer = Some("dense_anchor".into());
            negative.hit.eligible_for_sufficiency = Some(false);
            let base =
                codestory_agent::citation::to_citation_from_hit(&negative.hit, None, None, true);
            let citation = negative.citation_for_requirements_from_base(
                base,
                true,
                std::slice::from_ref(&requirement),
            );
            assert_eq!(
                citation.evidence_tier,
                Some(PacketEvidenceTierDto::DenseSemantic)
            );
            assert_eq!(citation.evidence_producer.as_deref(), Some("dense_anchor"));
            assert_eq!(citation.eligible_for_sufficiency, Some(false));
            assert!(citation.evidence_edge_ids.is_empty());
        }

        let mut confidence_only = boundary_hit(
            "app.route",
            "route",
            Some("src/server.js:10|syntax:js-member-call|receiver-owner:metrics"),
            None,
            true,
        );
        confidence_only.hit.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        confidence_only.hit.evidence_producer = Some("dense_anchor".into());
        confidence_only.hit.eligible_for_sufficiency = Some(false);
        confidence_only.graph.as_mut().expect("graph").edges[0].confidence = Some(1.0);
        let base =
            codestory_agent::citation::to_citation_from_hit(&confidence_only.hit, None, None, true);
        let citation = confidence_only.citation_for_requirements_from_base(
            base,
            true,
            std::slice::from_ref(&requirement),
        );
        assert_eq!(
            citation.evidence_tier,
            Some(PacketEvidenceTierDto::DenseSemantic)
        );
        assert_eq!(citation.eligible_for_sufficiency, Some(false));
        assert!(citation.evidence_edge_ids.is_empty());
    }

    #[test]
    fn certain_target_keeps_target_predicate_while_invalid_edges_fail_closed() {
        let requirement = server_requirement("request_dispatch");
        let certain = boundary_hit("app.handle", "Router.handle", None, Some("certain"), true);
        let citation = certain.citation_for_requirements(true, &[requirement]);
        assert!(certain.has_proof_call_provenance_for_requirement(&citation, &requirement));

        let mut resolved = boundary_hit("app.handle", "Router.handle", None, None, true);
        resolved.graph.as_mut().expect("graph").nodes[1].kind = NodeKind::METHOD;
        let resolved_citation = resolved.citation_for_requirements(true, &[requirement]);
        assert!(
            resolved.has_proof_call_provenance_for_requirement(&resolved_citation, &requirement)
        );

        let incoming = boundary_hit("app.handle", "Router.handle", None, Some("certain"), false);
        let incoming_citation = incoming.citation_for_requirements(true, &[requirement]);
        assert!(
            !incoming.has_proof_call_provenance_for_requirement(&incoming_citation, &requirement)
        );

        let wrong_target = boundary_hit(
            "app.handle",
            "telemetry.record",
            None,
            Some("certain"),
            true,
        );
        let wrong_citation = wrong_target.citation_for_requirements(true, &[requirement]);
        assert!(
            !wrong_target.has_proof_call_provenance_for_requirement(&wrong_citation, &requirement)
        );

        let mut speculative =
            boundary_hit("app.handle", "Router.handle", None, Some("probable"), true);
        speculative.graph.as_mut().expect("graph").edges[0].confidence = Some(0.7);
        let speculative_citation = speculative.citation_for_requirements(true, &[requirement]);
        assert!(
            !speculative
                .has_proof_call_provenance_for_requirement(&speculative_citation, &requirement)
        );

        let no_callsite = boundary_hit("app.handle", "handle", None, None, true);
        let no_callsite_citation = no_callsite.citation_for_requirements(true, &[requirement]);
        assert!(
            !no_callsite
                .has_proof_call_provenance_for_requirement(&no_callsite_citation, &requirement)
        );
        assert!(no_callsite_citation.evidence_edge_ids.is_empty());
    }

    #[test]
    fn more_than_twenty_incoming_edges_cannot_evict_a_lawful_outgoing_boundary() {
        let center_id = NodeId("response-send".into());
        let end_id = NodeId("response-end".into());
        let wrong_id = NodeId("response-buffer".into());
        let caller_id = NodeId("response-json".into());
        let mut nodes = [
            (center_id.clone(), "response.send"),
            (end_id.clone(), "end"),
            (wrong_id.clone(), "buffer"),
            (caller_id.clone(), "response.json"),
        ]
        .into_iter()
        .map(|(id, label)| GraphNodeDto {
            id,
            label: label.into(),
            kind: NodeKind::METHOD,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        })
        .collect::<Vec<_>>();
        nodes[0].depth = 0;
        nodes[1].kind = NodeKind::UNKNOWN;

        let mut edges = (0..24)
            .map(|index| GraphEdgeDto {
                id: EdgeId(format!("context-{index:02}")),
                source: caller_id.clone(),
                target: center_id.clone(),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: Some(format!("server.js:{}|syntax:js-member-call", index + 1)),
                candidate_targets: Vec::new(),
            })
            .collect::<Vec<_>>();
        edges.extend([
            GraphEdgeDto {
                id: EdgeId("incoming-only".into()),
                source: caller_id.clone(),
                target: center_id.clone(),
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".into()),
                callsite_identity: Some("server.js:20|syntax:js-member-call".into()),
                candidate_targets: Vec::new(),
            },
            GraphEdgeDto {
                id: EdgeId("speculative-end".into()),
                source: center_id.clone(),
                target: end_id.clone(),
                kind: EdgeKind::CALL,
                confidence: Some(0.7),
                certainty: Some("probable".into()),
                callsite_identity: Some("server.js:21|syntax:js-member-call".into()),
                candidate_targets: Vec::new(),
            },
            GraphEdgeDto {
                id: EdgeId("unbound-end".into()),
                source: center_id.clone(),
                target: end_id.clone(),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            },
            GraphEdgeDto {
                id: EdgeId("zz-proof-end".into()),
                source: center_id.clone(),
                target: end_id,
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: Some(
                    "server.js:23|syntax:js-member-call|receiver-owner:res".into(),
                ),
                candidate_targets: Vec::new(),
            },
        ]);
        let graph_provenance = edges
            .iter()
            .map(|edge| PacketGraphEdgeProvenance {
                edge_id: edge.id.clone(),
                direction: if edge.source == center_id {
                    PacketGraphDirection::Outgoing
                } else {
                    PacketGraphDirection::Incoming
                },
                hop: 1,
                producers: vec!["core_incident_call".into()],
                certainty: edge.certainty.clone(),
            })
            .collect();
        let hit = PacketSearchHit {
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: "response.send".into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/response.js".into()),
                line: Some(10),
                score: 0.8,
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
                score_breakdown: None,
            },
            graph_provenance,
            graph: Some(GraphResponse {
                center_id,
                nodes,
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        };
        let terms = packet_probe_terms(
            "Trace how a server application registers middleware, handles a request, and sends the response.",
        );
        let requirements =
            packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing);
        let terminal = requirements
            .iter()
            .find(|requirement| requirement.id == "request_terminal")
            .expect("terminal requirement");
        let citation = hit.citation_for_requirements(true, &requirements);

        assert_eq!(citation.evidence_edge_ids[0], EdgeId("zz-proof-end".into()));
        assert_eq!(
            citation.evidence_edge_ids.len(),
            1,
            "unrelated CALL context stays graph-only"
        );
        assert!(hit.has_proof_call_provenance_for_requirement(&citation, terminal));

        let mut capped = answer();
        merge_packet_candidate_graph_for_requirements(&mut capped, &hit, &requirements);
        let GraphArtifactDto::Uml { graph, .. } = &capped.graphs[0] else {
            panic!("expected candidate graph");
        };
        assert_eq!(graph.edges.len(), PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        assert_eq!(graph.edges[0].id, EdgeId("zz-proof-end".into()));
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 8);

        let mut negative = hit.clone();
        negative
            .graph
            .as_mut()
            .expect("graph")
            .edges
            .retain(|edge| edge.id != EdgeId("zz-proof-end".into()));
        assert!(!negative.has_proof_call_provenance_for_requirement(&citation, terminal));
    }

    #[test]
    fn lawful_outgoing_target_after_old_cutoff_is_selected_and_merge_keeps_omissions() {
        let requirement = server_requirement("request_terminal");
        let mut hit = boundary_hit(
            "res.send",
            "end",
            Some("src/server.js:50:1:20|syntax:js-member-call|receiver-owner:res"),
            None,
            true,
        );
        hit.graph_provenance[0].edge_id = EdgeId("zz-proof-end".into());
        let graph = hit.graph.as_mut().expect("graph");
        graph.edges[0].id = EdgeId("zz-proof-end".into());
        let wrong_id = NodeId("wrong".into());
        graph.nodes.push(GraphNodeDto {
            id: wrong_id.clone(),
            label: "observe".into(),
            kind: NodeKind::UNKNOWN,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: Some("src/server.js".into()),
            qualified_name: None,
            member_access: None,
        });
        for index in 0..25 {
            let edge_id = EdgeId(format!("context-{index:02}"));
            graph.edges.push(GraphEdgeDto {
                id: edge_id.clone(),
                source: NodeId("carrier".into()),
                target: wrong_id.clone(),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: Some(format!(
                    "src/server.js:{}:1:20|syntax:js-member-call|receiver-owner:metrics",
                    index + 1
                )),
                candidate_targets: Vec::new(),
            });
            hit.graph_provenance.push(PacketGraphEdgeProvenance {
                edge_id,
                direction: PacketGraphDirection::Outgoing,
                hop: 1,
                producers: vec!["core_incident_call".into()],
                certainty: None,
            });
        }
        graph.truncated = true;
        graph.omitted_edge_count = 7;

        let citation = hit.citation_for_requirements(true, &[requirement]);
        assert_eq!(citation.evidence_edge_ids[0], EdgeId("zz-proof-end".into()));
        assert!(hit.has_proof_call_provenance_for_requirement(&citation, &requirement));

        let mut merged = answer();
        merge_packet_candidate_graph_for_requirements(&mut merged, &hit, &[requirement]);
        merge_packet_candidate_graph_for_requirements(&mut merged, &hit, &[requirement]);
        let GraphArtifactDto::Uml {
            id: selection_view_id,
            graph,
            ..
        } = &merged.graphs[0]
        else {
            panic!("expected merged candidate graph");
        };
        assert_eq!(graph.edges.len(), PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        assert_eq!(graph.edges[0].id, EdgeId("zz-proof-end".into()));
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 13);

        let immutable_selection_view_id = selection_view_id.clone();
        let mut downstream_capped = merged.clone();
        assert!(cap_packet_graph_edges_for_test(
            &mut downstream_capped,
            1,
            &[EdgeId("zz-proof-end".into())],
        ));
        let capped_snapshot = serde_json::to_value(&downstream_capped).expect("capped answer");
        let GraphArtifactDto::Uml { id, graph, .. } = &downstream_capped.graphs[0] else {
            panic!("expected capped selection view");
        };
        assert_eq!(id, &immutable_selection_view_id);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].id, EdgeId("zz-proof-end".into()));
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 32);

        // Replaying the same source candidate after presentation capping finds the immutable
        // selection-view lineage and must not restore its budget-dropped optional rows.
        merge_packet_candidate_graph_for_requirements(&mut downstream_capped, &hit, &[requirement]);
        assert_eq!(
            serde_json::to_value(&downstream_capped).expect("replayed answer"),
            capped_snapshot
        );

        let candidate_graph = hit
            .graph_for_requirements(&citation, &[requirement])
            .expect("capped candidate graph");
        let mut preexisting_graph = candidate_graph.clone();
        preexisting_graph.truncated = false;
        preexisting_graph.omitted_edge_count = 0;
        let mut duplicate_owner = answer();
        duplicate_owner.graphs.push(GraphArtifactDto::Uml {
            id: "existing-neighborhood".into(),
            title: "Existing neighborhood".into(),
            graph: preexisting_graph,
        });
        merge_packet_candidate_graph_for_requirements(&mut duplicate_owner, &hit, &[requirement]);
        merge_packet_candidate_graph_for_requirements(&mut duplicate_owner, &hit, &[requirement]);
        assert_eq!(duplicate_owner.graphs.len(), 2);
        let GraphArtifactDto::Uml { id, graph, .. } = &duplicate_owner.graphs[0] else {
            panic!("expected existing graph");
        };
        assert_eq!(id, "existing-neighborhood");
        assert_eq!(graph.edges.len(), PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        assert!(!graph.truncated);
        assert_eq!(graph.omitted_edge_count, 0);
        let GraphArtifactDto::Uml {
            id: candidate_id,
            graph: preserved,
            ..
        } = &duplicate_owner.graphs[1]
        else {
            panic!("expected candidate-local graph");
        };
        assert!(preserved.truncated);
        assert_eq!(
            preserved.omitted_edge_count,
            candidate_graph.omitted_edge_count
        );
        assert_eq!(duplicate_owner.subgraph_ids, [candidate_id.clone()]);
    }

    #[test]
    fn overlapping_candidate_omissions_remain_artifact_local_and_replay_is_idempotent() {
        // A retains {a,b} and omits {c}; B retains {b,c} and omits {a}. The retained union is
        // complete, but opaque counts cannot prove whether the hidden identities overlap. Keep
        // the two bounded views separate instead of publishing a false aggregate omission of 2.
        let first = overlapping_candidate_hit(
            "candidate-a",
            &[
                ("a", "caller-a", "candidate-a"),
                ("b", "candidate-a", "candidate-b"),
            ],
            1,
        );
        let second = overlapping_candidate_hit(
            "candidate-b",
            &[
                ("b", "candidate-a", "candidate-b"),
                ("c", "candidate-b", "target-c"),
            ],
            1,
        );
        let complete = overlapping_candidate_hit(
            "candidate-complete",
            &[("complete", "candidate-complete", "target-complete")],
            0,
        );

        let mut merged = answer();
        for hit in [&first, &second, &first, &second, &complete, &complete] {
            merge_packet_candidate_graph(&mut merged, hit);
        }

        assert_eq!(
            merged.graphs.len(),
            3,
            "exact replays must not add artifacts"
        );
        assert_eq!(merged.subgraph_ids.len(), 3);
        assert_eq!(merged.subgraph_ids.iter().collect::<HashSet<_>>().len(), 3);

        let mut overlapping_views = merged
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. }
                    if graph.center_id.0 == "candidate-a" || graph.center_id.0 == "candidate-b" =>
                {
                    let mut ids = graph
                        .edges
                        .iter()
                        .map(|edge| edge.id.0.as_str())
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    Some((graph.center_id.0.as_str(), ids, graph))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        overlapping_views.sort_by_key(|(center, _, _)| *center);
        assert_eq!(overlapping_views.len(), 2);
        assert_eq!(overlapping_views[0].1, ["a", "b"]);
        assert_eq!(overlapping_views[1].1, ["b", "c"]);
        for (_, _, graph) in &overlapping_views {
            assert!(graph.truncated, "one edge remains omitted from this view");
            assert_eq!(graph.omitted_edge_count, 1);
        }
        let retained_union = overlapping_views
            .iter()
            .flat_map(|(_, ids, _)| ids.iter().copied())
            .collect::<HashSet<_>>();
        assert_eq!(retained_union, HashSet::from(["a", "b", "c"]));
        assert!(
            overlapping_views
                .iter()
                .all(|(_, _, graph)| graph.omitted_edge_count != 2),
            "no artifact may claim a synthetic aggregate omission"
        );

        let complete_graph = merged
            .graphs
            .iter()
            .find_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. }
                    if graph.center_id.0 == "candidate-complete" =>
                {
                    Some(graph)
                }
                _ => None,
            })
            .expect("complete candidate view");
        assert!(!complete_graph.truncated);
        assert_eq!(complete_graph.omitted_edge_count, 0);
    }
}
