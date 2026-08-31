//! Structural packet evidence labels only.
//!
//! Domain `PacketEvidenceRole` variants and ownership predicates were removed
//! after Phase 9 leakage FAIL (CX-01/CX-02). Selection uses repository-evidence
//! planning; roles here are path/kind telemetry only.

use crate::packet_scoring::{
    normalize_identifier, packet_display_name_is_test_like, packet_display_path,
};
use codestory_contracts::api::{AgentCitationDto, NodeKind, SearchHitOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PacketEvidenceRole {
    /// Path-based regression / test surface (not a domain stage).
    TestsAndRegressionCoverage,
    /// Ordinary source-bearing citation without a domain stage label.
    SourceEvidence,
}

impl PacketEvidenceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TestsAndRegressionCoverage => "tests and regression coverage",
            Self::SourceEvidence => "source evidence",
        }
    }

    pub fn is_low_priority_cap_role(self) -> bool {
        matches!(self, Self::TestsAndRegressionCoverage)
    }
}

/// Map a citation to a structural label only. Never assigns domain stages.
pub fn packet_evidence_role(citation: &AgentCitationDto) -> Option<PacketEvidenceRole> {
    if citation.kind == NodeKind::FILE && citation.origin == SearchHitOrigin::TextMatch {
        return None;
    }
    let display = citation.display_name.to_ascii_lowercase();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if path_contains_test_segment(&path)
        || path.ends_with("_test.go")
        || path.ends_with(".test.ts")
        || packet_display_name_is_test_like(&display)
    {
        Some(PacketEvidenceRole::TestsAndRegressionCoverage)
    } else if citation.resolvable
        || citation.file_path.is_some()
        || matches!(
            citation.kind,
            NodeKind::FUNCTION
                | NodeKind::METHOD
                | NodeKind::CLASS
                | NodeKind::STRUCT
                | NodeKind::FILE
                | NodeKind::MACRO
        )
    {
        Some(PacketEvidenceRole::SourceEvidence)
    } else {
        None
    }
}

pub fn packet_claim_key_for_citation(
    role: PacketEvidenceRole,
    citation: &AgentCitationDto,
) -> String {
    let identity = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    format!("{}:{}:{}", role.as_str(), path, identity)
}

fn path_contains_test_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| matches!(segment, "test" | "tests" | "spec" | "specs" | "__tests__"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::NodeId;

    fn citation(display: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(format!("n-{display}")),
            display_name: display.to_string(),
            file_path: Some(path.to_string()),
            kind,
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
        }
    }

    #[test]
    fn http_adapter_display_is_source_evidence_not_transport_role() {
        let c = citation(
            "requests.adapters.HTTPAdapter",
            "src/requests/adapters.py",
            NodeKind::CLASS,
        );
        assert_eq!(
            packet_evidence_role(&c),
            Some(PacketEvidenceRole::SourceEvidence)
        );
        assert_ne!(
            packet_evidence_role(&c).map(|r| r.as_str()),
            Some("transport adapter")
        );
    }

    #[test]
    fn adapter_plus_http_compound_does_not_create_privileged_role() {
        let c = citation(
            "HttpTransportAdapter.send",
            "lib/http/adapter.rs",
            NodeKind::METHOD,
        );
        assert_eq!(
            packet_evidence_role(&c),
            Some(PacketEvidenceRole::SourceEvidence)
        );
    }
}
