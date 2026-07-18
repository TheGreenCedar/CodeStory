//! Typed evidence metadata for packet citations.

use codestory_contracts::api::{
    AgentCitationDto, PacketEvidenceResolutionDto, PacketEvidenceTierDto, SearchHit,
    SearchHitOrigin,
};
use codestory_contracts::language_support::{
    is_cargo_manifest_file_path, is_docker_compose_file_path, is_github_actions_workflow_path,
    structural_language_name_for_path,
};

const OPENAPI_ENDPOINT_SCHEMA_PRODUCER: &str = "openapi_endpoint_schema";

pub(crate) type PacketEvidenceTier = PacketEvidenceTierDto;
pub(crate) type PacketEvidenceResolution = PacketEvidenceResolutionDto;

pub(crate) fn decorate_search_hit_evidence(hit: &mut SearchHit) {
    let diagnostic_source_proof = hit_is_diagnostic_source_proof(hit);
    let tier = evidence_tier_for_hit(hit);
    let resolution = evidence_resolution_for_hit(hit);
    let producer = evidence_producer_for_hit(hit);
    hit.evidence_tier = Some(tier);
    hit.evidence_producer = Some(producer);
    hit.resolution_status = Some(resolution);
    hit.eligible_for_sufficiency =
        Some(!diagnostic_source_proof && evidence_is_sufficiency_eligible(tier, resolution));
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
    if citation_is_diagnostic_source_proof(citation) {
        let structural_text = citation_is_structural_source_proof(citation);
        citation.evidence_tier = Some(if structural_text {
            PacketEvidenceTier::StructuralText
        } else {
            PacketEvidenceTier::ExactSource
        });
        citation.resolution_status = Some(evidence_resolution_for_citation(citation));
        if structural_text {
            citation.evidence_producer = citation
                .file_path
                .as_deref()
                .and_then(|path| structural_text_producer_for_path(Some(path)))
                .map(str::to_string);
        }
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
            | PacketEvidenceTier::StructuralText
            | PacketEvidenceTier::SyntheticSourceScan
            | PacketEvidenceTier::GeneratedSummary
    )
}

pub(crate) fn citation_sufficiency_eligible(citation: &AgentCitationDto) -> bool {
    if citation_is_diagnostic_source_proof(citation) {
        return false;
    }
    let tier = citation
        .evidence_tier
        .unwrap_or_else(|| evidence_tier_for_citation(citation));
    let resolution = citation
        .resolution_status
        .unwrap_or_else(|| evidence_resolution_for_citation(citation));
    if !evidence_is_sufficiency_eligible(tier, resolution) {
        return false;
    }
    citation.eligible_for_sufficiency.unwrap_or(true)
}

pub(crate) fn evidence_tier_for_hit(hit: &SearchHit) -> PacketEvidenceTier {
    if hit_is_diagnostic_source_proof(hit) {
        return if hit_is_structural_source_proof(hit) {
            PacketEvidenceTier::StructuralText
        } else {
            PacketEvidenceTier::ExactSource
        };
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
    if hit_is_diagnostic_source_proof(hit) {
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
    if hit_is_openapi_endpoint_schema(hit) {
        return OPENAPI_ENDPOINT_SCHEMA_PRODUCER.to_string();
    }
    if hit_is_structural_source_proof(hit) {
        return structural_text_producer_for_path(hit.file_path.as_deref())
            .unwrap_or("structural_source_proof_collector")
            .to_string();
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

pub(crate) fn evidence_tier_for_citation(citation: &AgentCitationDto) -> PacketEvidenceTier {
    if citation_is_diagnostic_source_proof(citation) {
        return if citation_is_structural_source_proof(citation) {
            PacketEvidenceTier::StructuralText
        } else {
            PacketEvidenceTier::ExactSource
        };
    }
    if let Some(tier) = citation.evidence_tier {
        return tier;
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

pub(crate) fn evidence_resolution_for_citation(
    citation: &AgentCitationDto,
) -> PacketEvidenceResolution {
    if citation_is_diagnostic_source_proof(citation) {
        return if citation.file_path.is_some() && citation.line.is_some() {
            PacketEvidenceResolution::SourceRangeOnly
        } else {
            PacketEvidenceResolution::Unresolved
        };
    }
    if let Some(resolution) = citation.resolution_status {
        return resolution;
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
    structural_text_producer_for_path(hit.file_path.as_deref()).is_some()
}

fn hit_is_diagnostic_source_proof(hit: &SearchHit) -> bool {
    hit_is_structural_source_proof(hit) || hit_is_openapi_endpoint_schema(hit)
}

fn hit_is_openapi_endpoint_schema(hit: &SearchHit) -> bool {
    hit.evidence_producer.as_deref() == Some(OPENAPI_ENDPOINT_SCHEMA_PRODUCER)
}

fn citation_is_structural_source_proof(citation: &AgentCitationDto) -> bool {
    structural_text_producer_for_path(citation.file_path.as_deref()).is_some()
}

fn citation_is_diagnostic_source_proof(citation: &AgentCitationDto) -> bool {
    citation_is_structural_source_proof(citation) || citation_is_openapi_endpoint_schema(citation)
}

fn citation_is_openapi_endpoint_schema(citation: &AgentCitationDto) -> bool {
    citation.evidence_producer.as_deref() == Some(OPENAPI_ENDPOINT_SCHEMA_PRODUCER)
}

/// Return the stable producer label for an already-indexed structural collector
/// path. This intentionally does not admit generic text formats: expansion of
/// the collector inventory belongs to the structural indexing work, not result
/// publication.
pub(crate) fn structural_text_producer_for_path(path: Option<&str>) -> Option<&'static str> {
    let path = path?;
    if is_github_actions_workflow_path(path) {
        return Some("structural_github_actions_workflow_collector");
    }
    if is_docker_compose_file_path(path) {
        return Some("structural_docker_compose_collector");
    }
    if is_cargo_manifest_file_path(path) {
        return Some("structural_cargo_manifest_collector");
    }
    match structural_language_name_for_path(Some(path)) {
        Some("html") => Some("structural_html_collector"),
        Some("css") => Some("structural_css_collector"),
        Some("sql") => Some("structural_sql_collector"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentCitationDto, NodeId, NodeKind, RetrievalScoreBreakdownDto, SearchHitOrigin,
    };

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
    fn github_actions_structural_hit_is_structural_text_diagnostic() {
        let mut hit = workflow_hit();

        decorate_search_hit_evidence(&mut hit);

        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::StructuralText));
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
    fn docker_compose_structural_hit_is_structural_text_diagnostic() {
        let mut hit = workflow_hit();
        hit.node_id = NodeId("compose-web".to_string());
        hit.display_name = "web".to_string();
        hit.file_path = Some("docker/retrieval-compose.yml".to_string());
        hit.line = Some(9);

        decorate_search_hit_evidence(&mut hit);

        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::StructuralText));
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

    #[test]
    fn openapi_endpoint_hit_is_exact_source_diagnostic() {
        let mut hit = workflow_hit();
        hit.node_id = NodeId("openapi-endpoint".to_string());
        hit.display_name = "GET /api/users".to_string();
        hit.file_path = Some("openapi.json".to_string());
        hit.evidence_producer = Some("openapi_endpoint_schema".to_string());

        decorate_search_hit_evidence(&mut hit);

        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::ExactSource));
        assert_eq!(
            hit.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(
            hit.evidence_producer.as_deref(),
            Some("openapi_endpoint_schema")
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn cargo_manifest_structural_hit_is_structural_text_diagnostic() {
        let mut hit = workflow_hit();
        hit.node_id = NodeId("cargo-serde".to_string());
        hit.display_name = "serde".to_string();
        hit.file_path = Some("crates/app/Cargo.toml".to_string());
        hit.line = Some(8);

        decorate_search_hit_evidence(&mut hit);

        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTier::StructuralText));
        assert_eq!(
            hit.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(
            hit.evidence_producer.as_deref(),
            Some("structural_cargo_manifest_collector")
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn structural_text_producers_cover_existing_collectors_without_admitting_generic_text() {
        let cases = [
            (
                ".github/workflows/ci.yml",
                "structural_github_actions_workflow_collector",
            ),
            (
                "docker/stack-compose.yaml",
                "structural_docker_compose_collector",
            ),
            (
                "crates/runtime/Cargo.toml",
                "structural_cargo_manifest_collector",
            ),
            ("web/index.html", "structural_html_collector"),
            ("web/styles.css", "structural_css_collector"),
            ("db/schema.sql", "structural_sql_collector"),
        ];

        for (path, expected_producer) in cases {
            assert_eq!(
                structural_text_producer_for_path(Some(path)),
                Some(expected_producer),
                "expected existing structural collector for {path}"
            );
        }

        for path in [
            "docs/design.md",
            "config/service.yaml",
            "config/service.toml",
        ] {
            assert_eq!(
                structural_text_producer_for_path(Some(path)),
                None,
                "generic text remains outside this publication-only slice: {path}"
            );
        }
    }

    #[test]
    fn structural_text_citation_remains_source_range_only_and_non_sufficient() {
        let mut hit = workflow_hit();
        decorate_search_hit_evidence(&mut hit);
        let mut citation = AgentCitationDto {
            node_id: hit.node_id.clone(),
            display_name: hit.display_name.clone(),
            kind: hit.kind,
            file_path: hit.file_path.clone(),
            line: hit.line,
            score: hit.score,
            origin: hit.origin,
            resolvable: hit.resolvable,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
        };

        decorate_citation_from_hit(&mut citation, &hit);

        assert_eq!(
            citation.evidence_tier,
            Some(PacketEvidenceTier::StructuralText)
        );
        assert_eq!(
            citation.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(
            citation.evidence_producer.as_deref(),
            Some("structural_github_actions_workflow_collector")
        );
        assert_eq!(citation.eligible_for_sufficiency, Some(false));
        assert!(!citation_sufficiency_eligible(&citation));

        citation.eligible_for_sufficiency = Some(true);
        assert!(
            !citation_sufficiency_eligible(&citation),
            "an adapter-provided eligibility flag must not promote structural evidence"
        );
    }

    #[test]
    fn openapi_endpoint_citation_is_not_sufficiency_eligible() {
        let mut hit = workflow_hit();
        hit.node_id = NodeId("openapi-endpoint".to_string());
        hit.display_name = "GET /api/users".to_string();
        hit.file_path = Some("openapi.json".to_string());
        hit.evidence_producer = Some("openapi_endpoint_schema".to_string());
        decorate_search_hit_evidence(&mut hit);

        let mut citation = AgentCitationDto {
            node_id: hit.node_id.clone(),
            display_name: hit.display_name.clone(),
            kind: hit.kind,
            file_path: hit.file_path.clone(),
            line: hit.line,
            score: hit.score,
            origin: hit.origin,
            resolvable: hit.resolvable,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
        };

        decorate_citation_from_hit(&mut citation, &hit);

        assert_eq!(
            citation.evidence_tier,
            Some(PacketEvidenceTier::ExactSource)
        );
        assert_eq!(
            citation.resolution_status,
            Some(PacketEvidenceResolution::SourceRangeOnly)
        );
        assert_eq!(
            citation.evidence_producer.as_deref(),
            Some("openapi_endpoint_schema")
        );
        assert_eq!(citation.eligible_for_sufficiency, Some(false));
        assert!(!citation_sufficiency_eligible(&citation));
    }
}
