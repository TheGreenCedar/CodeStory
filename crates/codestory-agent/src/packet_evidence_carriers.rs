//! Domain evidence-carrier predicates were removed after Phase 9 leakage FAIL
//! (CX-02). Packet selection and sufficiency use repository-evidence planning
//! only. This module retains a zeroed rank-bonus stub so call sites compile
//! without reintroducing ownership predicates.

use codestory_contracts::api::AgentCitationDto;

/// Always zero — no domain vocabulary may boost ranking.
pub fn packet_server_dispatch_callable_rank_bonus(
    _citation: &AgentCitationDto,
    _terms: &[String],
) -> f32 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{NodeId, NodeKind, SearchHitOrigin};

    #[test]
    fn rank_bonus_is_always_zero() {
        let citation = AgentCitationDto {
            node_id: NodeId("n1".to_string()),
            display_name: "HTTPAdapter.send".to_string(),
            file_path: Some("src/requests/adapters.py".to_string()),
            kind: NodeKind::METHOD,
            line: Some(1),
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            score: 1.0,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
        };
        assert_eq!(
            packet_server_dispatch_callable_rank_bonus(
                &citation,
                &["http".into(), "adapter".into()]
            ),
            0.0
        );
    }
}
