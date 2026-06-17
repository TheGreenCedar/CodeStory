//! Typed evidence metadata for packet citations.

use codestory_contracts::api::{
    AgentCitationDto, PacketEvidenceResolutionDto, PacketEvidenceTierDto, SearchHit,
    SearchHitOrigin,
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
    hit.evidence_tier = Some(candidate.tier);
    hit.evidence_producer = Some(candidate.producer);
    hit.resolution_status = Some(candidate.resolution);
    if hit.loss_reason.is_none() {
        hit.loss_reason = candidate.loss_reason;
    }
    hit.eligible_for_sufficiency = Some(evidence_is_sufficiency_eligible(
        candidate.tier,
        candidate.resolution,
    ));
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
    if citation.resolvable {
        PacketEvidenceResolution::Resolved
    } else if citation.file_path.is_some() && citation.line.is_some() {
        PacketEvidenceResolution::SourceRangeOnly
    } else {
        PacketEvidenceResolution::Unresolved
    }
}
