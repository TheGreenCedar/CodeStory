//! Typed evidence metadata for packet citations.

use codestory_contracts::api::{
    AgentCitationDto, PacketEvidenceResolutionDto, PacketEvidenceTierDto, SearchHit,
    SearchHitOrigin,
};
use codestory_contracts::language_support::{
    is_docker_compose_path, is_github_actions_workflow_path,
};

pub(crate) type PacketEvidenceTier = PacketEvidenceTierDto;
pub(crate) type PacketEvidenceResolution = PacketEvidenceResolutionDto;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct EvidenceCandidate {
    pub display_name: String,
    pub path: Option<String>,
    pub node_id: Option<String>,
    pub tier: PacketEvidenceTier,
    pub resolution: PacketEvidenceResolution,
    pub producer: String,
    pub score: f32,
    pub loss_reason: Option<String>,
}

pub(crate) fn evidence_candidate_from_hit(hit: &SearchHit) -> EvidenceCandidate {
    EvidenceCandidate {
        display_name: hit.display_name.clone(),
        path: hit.file_path.clone(),
        node_id: Some(hit.node_id.0.clone()),
        tier: evidence_tier_for_hit(hit),
        resolution: evidence_resolution_for_hit(hit),
        producer: evidence_producer_for_hit(hit),
        score: hit.score,
        loss_reason: hit.loss_reason.clone(),
    }
}

pub(crate) fn decorate_search_hit_evidence(hit: &mut SearchHit) {
    let candidate = evidence_candidate_from_hit(hit);
    let structural_source_proof = hit_is_structural_source_proof(hit);
    hit.evidence_tier = Some(candidate.tier);
    hit.evidence_producer = Some(candidate.producer);
    hit.resolution_status = Some(candidate.resolution);
    if hit.loss_reason.is_none() {
        hit.loss_reason = candidate.loss_reason;
    }
    hit.eligible_for_sufficiency = Some(
        !structural_source_proof
            && evidence_is_sufficiency_eligible(candidate.tier, candidate.resolution),
    );
}

pub(crate) fn decorate_citation_from_hit(citation: &mut AgentCitationDto, hit: &SearchHit) {
    citation.evidence_tier = hit
        .evidence_tier
        .or_else(|| Some(evidence_tier_for_hit(hit)));
    citation.evidence_producer = hit
        .evidence_producer
        .clone()
        .or_else(|| Some(evidence_producer_for_hit(hit)));
    citation.resolution_status = hit
        .resolution_status
        .or_else(|| Some(evidence_resolution_for_hit(hit)));
    citation.loss_reason = hit.loss_reason.clone();
    citation.coverage_role = hit.coverage_role.clone();
    if citation_is_structural_source_proof(citation) {
        citation.evidence_tier = Some(PacketEvidenceTier::ExactSource);
        citation.resolution_status = Some(evidence_resolution_for_citation(citation));
        citation.evidence_producer = Some(
            structural_source_proof_producer(citation.file_path.as_deref())
                .unwrap_or("structural_source_proof_collector")
                .to_string(),
        );
        citation.eligible_for_sufficiency = Some(false);
        return;
    }
    citation.eligible_for_sufficiency = hit.eligible_for_sufficiency.or_else(|| {
        Some(evidence_is_sufficiency_eligible(
            citation
                .evidence_tier
                .unwrap_or(PacketEvidenceTier::GeneratedSummary),
            citation
                .resolution_status
                .unwrap_or(PacketEvidenceResolution::Unresolved),
        ))
    });
}

pub(crate) fn evidence_is_sufficiency_eligible(
    tier: PacketEvidenceTier,
    resolution: PacketEvidenceResolution,
) -> bool {
    matches!(
        resolution,
        PacketEvidenceResolution::Resolved | PacketEvidenceResolution::SourceRangeOnly
    ) && !matches!(
        tier,
        PacketEvidenceTier::DenseSemantic
            | PacketEvidenceTier::SyntheticSourceScan
            | PacketEvidenceTier::GeneratedSummary
    )
}

pub(crate) fn citation_sufficiency_eligible(citation: &AgentCitationDto) -> bool {
    if citation_is_structural_source_proof(citation) {
        return citation.eligible_for_sufficiency.unwrap_or(false);
    }
    let tier = citation
        .evidence_tier
        .unwrap_or_else(|| evidence_tier_for_citation(citation));
    let resolution = citation
        .resolution_status
        .unwrap_or_else(|| evidence_resolution_for_citation(citation));
    citation
        .eligible_for_sufficiency
        .unwrap_or_else(|| evidence_is_sufficiency_eligible(tier, resolution))
}

pub(crate) fn evidence_tier_for_hit(hit: &SearchHit) -> PacketEvidenceTier {
    if hit_is_structural_source_proof(hit) {
        return PacketEvidenceTier::ExactSource;
    }
    if let Some(tier) = hit.evidence_tier {
        return tier;
    }
    if let Some(breakdown) = hit.score_breakdown.as_ref() {
        if breakdown.provenance.iter().any(|value| {
            value.contains("synthetic")
                || value.contains("source_scan")
                || value.contains("repo_text")
        }) {
            return PacketEvidenceTier::SyntheticSourceScan;
        }
        if breakdown.graph > 0.0 {
            return PacketEvidenceTier::ResolvedGraph;
        }
        if breakdown.semantic > 0.0 && breakdown.lexical <= 0.0 {
            return PacketEvidenceTier::DenseSemantic;
        }
        if breakdown.lexical > 0.0 {
            return PacketEvidenceTier::LexicalSource;
        }
    }
    match hit.origin {
        SearchHitOrigin::IndexedSymbol => PacketEvidenceTier::ResolvedGraph,
        SearchHitOrigin::TextMatch => PacketEvidenceTier::LexicalSource,
    }
}

pub(crate) fn evidence_resolution_for_hit(hit: &SearchHit) -> PacketEvidenceResolution {
    if hit_is_structural_source_proof(hit) {
        return if hit.file_path.is_some() && hit.line.is_some() {
            PacketEvidenceResolution::SourceRangeOnly
        } else {
            PacketEvidenceResolution::Unresolved
        };
    }
    if let Some(resolution) = hit.resolution_status {
        return resolution;
    }
    if hit.resolvable {
        PacketEvidenceResolution::Resolved
    } else if hit.file_path.is_some() && hit.line.is_some() {
        PacketEvidenceResolution::SourceRangeOnly
    } else {
        PacketEvidenceResolution::Unresolved
    }
}

pub(crate) fn evidence_producer_for_hit(hit: &SearchHit) -> String {
    if let Some(producer) = structural_source_proof_producer(hit.file_path.as_deref()) {
        return producer.to_string();
    }
    if let Some(producer) = hit.evidence_producer.as_ref() {
        return producer.clone();
    }
    if let Some(breakdown) = hit.score_breakdown.as_ref()
        && let Some(provenance) = breakdown.provenance.first()
    {
        return provenance.clone();
    }
    match hit.origin {
        SearchHitOrigin::IndexedSymbol => "indexed_symbol".to_string(),
        SearchHitOrigin::TextMatch => "text_match".to_string(),
    }
}

fn evidence_tier_for_citation(citation: &AgentCitationDto) -> PacketEvidenceTier {
    if citation_is_structural_source_proof(citation) {
        return PacketEvidenceTier::ExactSource;
    }
    if let Some(breakdown) = citation.retrieval_score_breakdown.as_ref() {
        if breakdown.provenance.iter().any(|value| {
            value.contains("synthetic")
                || value.contains("source_scan")
                || value.contains("repo_text")
        }) {
            return PacketEvidenceTier::SyntheticSourceScan;
        }
        if breakdown.graph > 0.0 {
            return PacketEvidenceTier::ResolvedGraph;
        }
        if breakdown.semantic > 0.0 && breakdown.lexical <= 0.0 {
            return PacketEvidenceTier::DenseSemantic;
        }
        if breakdown.lexical > 0.0 {
            return PacketEvidenceTier::LexicalSource;
        }
    }
    match citation.origin {
        SearchHitOrigin::IndexedSymbol => PacketEvidenceTier::ResolvedGraph,
        SearchHitOrigin::TextMatch => PacketEvidenceTier::LexicalSource,
    }
}

fn evidence_resolution_for_citation(citation: &AgentCitationDto) -> PacketEvidenceResolution {
    if citation_is_structural_source_proof(citation) {
        return if citation.file_path.is_some() && citation.line.is_some() {
            PacketEvidenceResolution::SourceRangeOnly
        } else {
            PacketEvidenceResolution::Unresolved
        };
    }
    if citation.resolvable {
        PacketEvidenceResolution::Resolved
    } else if citation.file_path.is_some() && citation.line.is_some() {
        PacketEvidenceResolution::SourceRangeOnly
    } else {
        PacketEvidenceResolution::Unresolved
    }
}

fn hit_is_structural_source_proof(hit: &SearchHit) -> bool {
    structural_source_proof_producer(hit.file_path.as_deref()).is_some()
}

fn citation_is_structural_source_proof(citation: &AgentCitationDto) -> bool {
    structural_source_proof_producer(citation.file_path.as_deref()).is_some()
}

fn structural_source_proof_producer(path: Option<&str>) -> Option<&'static str> {
    let path = path?;
    if is_github_actions_workflow_path(path) {
        return Some("structural_github_actions_workflow_collector");
    }
    if is_docker_compose_path(path) {
        return Some("structural_docker_compose_collector");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{NodeId, NodeKind, RetrievalScoreBreakdownDto, SearchHitOrigin};

    fn workflow_hit() -> SearchHit {
        SearchHit {
            node_id: NodeId("workflow-build".to_string()),
            display_name: "build".to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(".github/workflows/ci.yml".to_string()),
            line: Some(5),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.0,
                semantic: 0.0,
                graph: 1.0,
                total: 1.0,
                tier_cap: None,
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: None,
                provenance: Vec::new(),
            }),
        }
    }

    #[test]
    fn github_actions_structural_hit_is_exact_source_diagnostic() {
        let mut hit = workflow_hit();

        decorate_search_hit_evidence(&mut hit);

        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::ExactSource));
        assert_eq!(
            hit.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(
            hit.evidence_producer.as_deref(),
            Some("structural_github_actions_workflow_collector")
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn docker_compose_structural_hit_is_exact_source_diagnostic() {
        let mut hit = workflow_hit();
        hit.node_id = NodeId("compose-qdrant".to_string());
        hit.display_name = "qdrant".to_string();
        hit.file_path = Some("docker/retrieval-compose.yml".to_string());
        hit.line = Some(4);

        decorate_search_hit_evidence(&mut hit);

        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::ExactSource));
        assert_eq!(
            hit.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(
            hit.evidence_producer.as_deref(),
            Some("structural_docker_compose_collector")
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
    }
}
