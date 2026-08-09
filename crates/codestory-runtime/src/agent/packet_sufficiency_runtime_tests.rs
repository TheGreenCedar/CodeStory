//! Runtime integration tests for the agent-owned packet sufficiency assembly.
//!
//! The budget-cap proof below drives `packet_budget`, which stays pinned to the
//! runtime crate (it reaches `AppController` state the planning crate must
//! never see), so the test lives beside the budget rather than teaching
//! `codestory-agent` a dev-dependency on the runtime.

use crate::agent::packet_budget::{apply_packet_budget, packet_budget_limits};
use crate::agent::packet_sufficiency::{PacketSufficiencyInput, assemble_packet_sufficiency};
use codestory_contracts::api::*;
use std::collections::HashSet;
use std::path::Path;

fn cited_anchor(name: &str) -> AgentCitationDto {
    AgentCitationDto {
        node_id: NodeId(name.to_string()),
        display_name: name.to_string(),
        kind: NodeKind::FUNCTION,
        file_path: Some(format!("src/{name}.rs")),
        line: Some(1),
        score: 1.0,
        origin: SearchHitOrigin::IndexedSymbol,
        target: None,
        resolvable: true,
        subgraph_id: None,
        evidence_edge_ids: Vec::new(),
        retrieval_score_breakdown: None,
        evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
        evidence_producer: Some("test".to_string()),
        resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: Some(true),
    }
}
fn cited_claim(
    text: &str,
    coverage_role: Option<&str>,
    citation: AgentCitationDto,
    eligible_for_sufficiency: Option<bool>,
) -> PacketClaimDto {
    PacketClaimDto {
        claim: text.to_string(),
        required_obligation_ids: Vec::new(),
        required_obligation_kinds: Vec::new(),
        proof_status: None,
        required_evidence_role: None,
        citations: vec![citation],
        coverage_role: coverage_role.map(str::to_string),
        eligible_for_sufficiency,
    }
}
fn answer_fixture(question: &str) -> AgentAnswerDto {
    AgentAnswerDto {
        source_coverage: Vec::new(),
        answer_id: "packet-sufficiency-test".to_string(),
        prompt: question.to_string(),
        summary: "Covered by cited anchors.".to_string(),
        freshness: Some(crate::agent::packet_freshness::fresh_index_observation()),
        sections: vec![AgentResponseSectionDto {
            id: "answer".to_string(),
            title: "Answer".to_string(),
            blocks: vec![AgentResponseBlockDto::Markdown {
                markdown: "Covered by cited anchors.".to_string(),
            }],
        }],
        citations: vec![
            cited_anchor("first"),
            cited_anchor("second"),
            cited_anchor("third"),
        ],
        subgraph_ids: Vec::new(),
        retrieval_version: "test".to_string(),
        graphs: Vec::new(),
        retrieval_trace: AgentRetrievalTraceDto {
            request_id: "packet-sufficiency-test".to_string(),
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
fn route_graph_node(id: &str) -> GraphNodeDto {
    GraphNodeDto {
        id: NodeId(id.to_string()),
        label: id.to_string(),
        kind: NodeKind::FUNCTION,
        depth: 1,
        label_policy: None,
        badge_visible_members: None,
        badge_total_members: None,
        merged_symbol_examples: Vec::new(),
        file_path: Some(format!("src/{id}.rs")),
        qualified_name: None,
        member_access: None,
    }
}
fn route_graph_edge(id: &str, source: &str, target: &str) -> GraphEdgeDto {
    route_graph_edge_with_proof(id, source, target, Some("certain"), Some(1.0))
}
fn route_graph_edge_with_proof(
    id: &str,
    source: &str,
    target: &str,
    certainty: Option<&str>,
    confidence: Option<f32>,
) -> GraphEdgeDto {
    GraphEdgeDto {
        id: EdgeId(id.to_string()),
        source: NodeId(source.to_string()),
        target: NodeId(target.to_string()),
        kind: EdgeKind::CALL,
        confidence,
        certainty: certainty.map(str::to_string),
        callsite_identity: None,
        candidate_targets: Vec::new(),
    }
}
fn route_graph(id: &str, nodes: &[&str], edges: &[(&str, &str)]) -> GraphArtifactDto {
    GraphArtifactDto::Uml {
        id: id.to_string(),
        title: "Execution Route".to_string(),
        graph: GraphResponse {
            center_id: NodeId(nodes.first().copied().unwrap_or("route").to_string()),
            nodes: nodes.iter().map(|node| route_graph_node(node)).collect(),
            edges: edges
                .iter()
                .enumerate()
                .map(|(index, (source, target))| {
                    route_graph_edge(&format!("edge-{index}"), source, target)
                })
                .collect(),
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        },
    }
}
fn route_claim(name: &str) -> PacketClaimDto {
    cited_claim(
        &format!("`{name}` is a requested route endpoint and calls into downstream work."),
        Some("route endpoint"),
        cited_anchor(name),
        Some(true),
    )
}
fn route_answer(question: &str, names: &[&str], edges: &[(&str, &str)]) -> AgentAnswerDto {
    let mut answer = answer_fixture(question);
    answer.citations = names.iter().map(|name| cited_anchor(name)).collect();
    answer.graphs = vec![route_graph("route", names, edges)];
    answer
}
fn route_sufficiency(
    question: &str,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
    claims: Vec<PacketClaimDto>,
) -> PacketSufficiencyDto {
    assemble_packet_sufficiency(PacketSufficiencyInput {
        project_root: Path::new("C:/workspace/project"),
        question,
        task_class: PacketTaskClassDto::RouteTracing,
        answer,
        budget,
        supported_claims: claims,
        missing_required_probe_queries: Vec::new(),
        targeted_follow_up_queries: Vec::new(),
    })
}
#[test]
fn route_proof_observes_actual_citation_and_edge_caps_across_packet_budgets() {
    let question = "RouteIngress -> RouteDispatch -> RouteEgress";
    let mut uncapped_answer = route_answer(
        question,
        &["RouteIngress", "RouteDispatch", "RouteEgress"],
        &[
            ("RouteIngress", "RouteDispatch"),
            ("RouteDispatch", "RouteEgress"),
        ],
    );
    uncapped_answer
        .citations
        .extend((0..12).map(|index| cited_anchor(&format!("Filler{index}"))));
    let GraphArtifactDto::Uml { graph, .. } = &mut uncapped_answer.graphs[0] else {
        unreachable!("route fixture must contain UML")
    };
    let route_edges = std::mem::take(&mut graph.edges);
    graph
        .nodes
        .extend((0..22).map(|index| route_graph_node(&format!("Filler{index}"))));
    graph.edges.extend((0..21).map(|index| {
        route_graph_edge(
            &format!("filler-edge-{index}"),
            &format!("Filler{index}"),
            &format!("Filler{}", index + 1),
        )
    }));
    graph.edges.extend(route_edges);

    for requested in [
        PacketBudgetModeDto::Compact,
        PacketBudgetModeDto::Standard,
        PacketBudgetModeDto::Deep,
    ] {
        let mut answer = uncapped_answer.clone();
        let limits = packet_budget_limits(requested);
        let budget = apply_packet_budget(
            Path::new("C:/workspace/project"),
            question,
            PacketTaskClassDto::RouteTracing,
            requested,
            limits.clone(),
            &mut answer,
        );
        let retained = answer
            .citations
            .iter()
            .map(|citation| citation.node_id.0.as_str())
            .collect::<HashSet<_>>();
        let claims = ["RouteIngress", "RouteDispatch", "RouteEgress"]
            .into_iter()
            .filter(|name| retained.contains(name))
            .map(route_claim)
            .collect();
        let sufficiency = route_sufficiency(question, &answer, &budget, claims);

        if requested == PacketBudgetModeDto::Compact {
            assert!(budget.truncated, "compact must exercise real caps");
            assert!(answer.citations.len() <= limits.max_anchors as usize);
            let GraphArtifactDto::Uml { graph, .. } = &answer.graphs[0] else {
                unreachable!("route fixture must retain UML")
            };
            assert_eq!(graph.edges.len(), limits.max_trail_edges as usize);
            assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
        } else {
            assert!(!budget.truncated, "{requested:?} should retain the route");
            assert_eq!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Sufficient,
                "retained route should remain sufficient for {requested:?}: {sufficiency:?}"
            );
            assert!(sufficiency.gaps.is_empty());
        }
    }
}
