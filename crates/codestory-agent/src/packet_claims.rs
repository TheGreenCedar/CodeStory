#[cfg(any(test, feature = "test-support"))]
use crate::eval_probes::eval_citation_shaped_claim;
use crate::packet_evidence::{
    citation_sufficiency_eligible, evidence_resolution_for_citation, evidence_tier_for_citation,
};
use crate::packet_evidence_roles::{
    PacketEvidenceRole, packet_claim_key_for_citation, packet_evidence_role,
};
use crate::packet_profile_telemetry::{PacketClaimSource, PacketClaimTelemetry};
use crate::packet_scoring::{
    normalize_identifier, packet_claim_carry_rank, packet_display_path, sort_by_cached_rank_desc,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, PacketClaimDto, PacketEvidenceResolutionDto,
    PacketEvidenceTierDto, PacketProofStatusDto,
};
use std::collections::HashSet;
use std::fmt::Write as _;

pub fn packet_flow_claims_markdown(claims: &[PacketClaimDto]) -> String {
    let mut markdown = String::new();
    markdown.push_str(
        "Packet claim status: `P` proven, `R` reported lead, `L` likely, `D` diagnostic, `U` unsupported or unclassified. Only `P` claims support sufficiency.\n",
    );
    for claim in claims {
        let citation = claim.citations.first();
        let suffix = citation
            .and_then(|citation| citation.file_path.as_deref())
            .map(packet_display_path)
            .map(|path| format!(" (`{path}`)"))
            .unwrap_or_default();
        let status = match claim.proof_status {
            Some(PacketProofStatusDto::Proven) => "P",
            Some(PacketProofStatusDto::Reported) => "R",
            Some(PacketProofStatusDto::Likely) => "L",
            Some(PacketProofStatusDto::Diagnostic) => "D",
            Some(PacketProofStatusDto::Unsupported) | None => "U",
        };
        let _ = writeln!(markdown, "- [`{status}`] {}{}", claim.claim, suffix);
    }
    markdown
}

pub fn packet_supported_claims(answer: &AgentAnswerDto) -> Vec<PacketClaimDto> {
    packet_supported_claims_with_telemetry(answer).0
}

pub fn packet_supported_claims_with_telemetry(
    answer: &AgentAnswerDto,
) -> (Vec<PacketClaimDto>, PacketClaimTelemetry) {
    let mut claims = Vec::new();
    let mut seen_claims = HashSet::new();
    let mut telemetry = PacketClaimTelemetry::default();
    // Disposition and claim ranking never inspect prompt tokens. Retrieval
    // scores and citation identity already sit on the citation.
    let rank_terms: &[String] = &[];
    let citations = answer.citations.clone();

    let before_role_claims = claims.len();
    append_ranked_citation_claims(
        &answer.prompt,
        &citations,
        rank_terms,
        true,
        &mut claims,
        &mut seen_claims,
    );
    telemetry.record_claim_source(
        PacketClaimSource::RoleTemplate,
        claims.len().saturating_sub(before_role_claims),
    );
    decorate_packet_claims_proof_metadata(&mut claims);
    (claims, telemetry)
}

pub fn decorate_packet_claims_proof_metadata(claims: &mut [PacketClaimDto]) {
    for claim in claims {
        decorate_packet_claim_proof_metadata(claim);
    }
}

fn decorate_packet_claim_proof_metadata(claim: &mut PacketClaimDto) {
    let proven_tier = claim
        .citations
        .iter()
        .find(|citation| citation_sufficiency_eligible(citation))
        .map(evidence_tier_for_citation);
    claim.required_evidence_role = Some(proven_tier.unwrap_or(PacketEvidenceTierDto::ExactSource));
    claim.proof_status = Some(packet_claim_proof_status(claim, proven_tier.is_some()));
}

fn packet_claim_proof_status(
    claim: &PacketClaimDto,
    has_proof_bearing_citation: bool,
) -> PacketProofStatusDto {
    if claim.citations.is_empty() {
        return PacketProofStatusDto::Unsupported;
    }
    if has_proof_bearing_citation && claim.eligible_for_sufficiency != Some(false) {
        return PacketProofStatusDto::Proven;
    }
    if claim
        .citations
        .iter()
        .all(packet_citation_is_diagnostic_only)
    {
        return PacketProofStatusDto::Diagnostic;
    }
    PacketProofStatusDto::Likely
}

fn packet_citation_is_diagnostic_only(citation: &AgentCitationDto) -> bool {
    if citation.eligible_for_sufficiency == Some(false) {
        return true;
    }
    matches!(
        evidence_tier_for_citation(citation),
        PacketEvidenceTierDto::DenseSemantic
            | PacketEvidenceTierDto::StructuralText
            | PacketEvidenceTierDto::GeneratedSummary
            | PacketEvidenceTierDto::SyntheticSourceScan
    ) || matches!(
        evidence_resolution_for_citation(citation),
        PacketEvidenceResolutionDto::DiagnosticOnly
    )
}

/// Counts claims one assembly layer actually added, so `claim_source` totals describe the
/// packet that shipped rather than what a layer offered before dedupe and the claim cap.
pub fn append_ranked_citation_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    rank_terms: &[String],
    prefer_primary_sources: bool,
    claims: &mut Vec<PacketClaimDto>,
    seen_claims: &mut HashSet<String>,
) {
    let mut ordered_citations = citations.to_vec();
    sort_by_cached_rank_desc(&mut ordered_citations, |citation| {
        packet_claim_carry_rank(citation, rank_terms, prefer_primary_sources)
    });
    for citation in &ordered_citations {
        if let Some(shaped) = packet_citation_shaped_claim(citation, prompt) {
            let key = normalize_identifier(&shaped);
            if seen_claims.insert(key) {
                claims.push(PacketClaimDto {
                    claim: shaped,
                    required_obligation_ids: Vec::new(),
                    required_obligation_kinds: Vec::new(),
                    proof_status: None,
                    required_evidence_role: None,
                    citations: vec![citation.clone()],
                    coverage_role: citation.coverage_role.clone(),
                    eligible_for_sufficiency: Some(false),
                });
            }
            continue;
        }
        let role = match packet_evidence_role(citation) {
            Some(PacketEvidenceRole::TestsAndRegressionCoverage) => {
                let lower = prompt.to_ascii_lowercase();
                if lower.contains("test")
                    || lower.contains("regression")
                    || lower.contains("edit")
                    || lower.contains("plan")
                {
                    PacketEvidenceRole::TestsAndRegressionCoverage
                } else {
                    continue;
                }
            }
            Some(PacketEvidenceRole::SourceEvidence) => PacketEvidenceRole::SourceEvidence,
            None => continue,
        };
        let claim_key = packet_claim_key_for_citation(role, citation);
        if !seen_claims.insert(claim_key.clone()) {
            continue;
        }
        claims.push(PacketClaimDto {
            claim: packet_claim_for_role(role, citation, prompt, rank_terms),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: None,
            required_evidence_role: None,
            citations: vec![citation.clone()],
            coverage_role: Some(role.as_str().to_string()),
            eligible_for_sufficiency: Some(citation_sufficiency_eligible(citation)),
        });
        if claims.len() >= 18 {
            break;
        }
    }
}

pub fn packet_claim_for_role(
    role: PacketEvidenceRole,
    citation: &AgentCitationDto,
    prompt: &str,
    _rank_terms: &[String],
) -> String {
    if let Some(shaped) = packet_citation_shaped_claim(citation, prompt) {
        return shaped;
    }
    let symbol = citation.display_name.as_str();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    match role {
        PacketEvidenceRole::TestsAndRegressionCoverage => {
            format!("`{symbol}` in `{path}` is test or regression coverage evidence.")
        }
        PacketEvidenceRole::SourceEvidence => {
            format!("`{symbol}` in `{path}` is cited source evidence for this question.")
        }
    }
}

fn packet_citation_shaped_claim(citation: &AgentCitationDto, prompt: &str) -> Option<String> {
    #[cfg(any(test, feature = "test-support"))]
    {
        let path = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .unwrap_or_default();
        eval_citation_shaped_claim(citation, prompt, &path)
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        let _ = (citation, prompt);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalTraceDto, NodeId,
        NodeKind, PacketProofStatusDto, RetrievalScoreBreakdownDto, SearchHitOrigin,
    };

    #[test]
    fn packet_claim_markdown_distinguishes_reported_leads_from_proven_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "The real dispatch edge is present.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: Some(PacketProofStatusDto::Proven),
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
            },
            PacketClaimDto {
                claim: "RuntimeVariable coordinates state transitions.".to_string(),
                required_obligation_ids: Vec::new(),
                required_obligation_kinds: Vec::new(),
                proof_status: Some(PacketProofStatusDto::Reported),
                required_evidence_role: None,
                citations: Vec::new(),
                coverage_role: None,
                eligible_for_sufficiency: Some(false),
            },
        ];

        let markdown = packet_flow_claims_markdown(&claims);

        assert!(markdown.contains("[`P`] The real dispatch edge is present."));
        assert!(
            markdown.contains("[`R`] RuntimeVariable coordinates state transitions."),
            "{markdown}"
        );
        assert!(!markdown.contains("Supported claims for a compact agent answer"));
    }

    fn test_answer(prompt: &str, citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            source_coverage: Vec::new(),
            answer_id: "packet-claims-test".to_string(),
            prompt: prompt.to_string(),
            summary: "test answer".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations,
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "packet-claims-test".to_string(),
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

    fn test_citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(format!("test::{display_name}")),
            display_name: display_name.to_string(),
            kind: NodeKind::ANNOTATION,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: score,
                semantic: 0.0,
                graph: 0.0,
                total: score,
                tier_cap: None,
                boosts: Vec::new(),
                dampening: Vec::new(),
                final_rank_reason: None,
                provenance: Vec::new(),
            }),
            evidence_tier: None,
            evidence_producer: Some("test".to_string()),
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            source_excerpt: None,
        }
    }

    #[test]
    fn generated_summary_and_dense_claims_need_backing_source_proof() {
        let mut generated = test_citation("generated summary", "target/generated/summary.md", 0.9);
        generated.evidence_tier = Some(PacketEvidenceTierDto::GeneratedSummary);
        generated.resolution_status = Some(PacketEvidenceResolutionDto::DiagnosticOnly);
        generated.eligible_for_sufficiency = None;

        let mut dense = test_citation("dense anchor", "src/runtime.rs", 0.8);
        dense.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        dense.resolution_status = Some(PacketEvidenceResolutionDto::Resolved);
        dense.eligible_for_sufficiency = None;

        let mut claims = vec![PacketClaimDto {
            claim: "Runtime dispatch is covered.".to_string(),
            required_obligation_ids: Vec::new(),
            required_obligation_kinds: Vec::new(),
            proof_status: None,
            required_evidence_role: None,
            citations: vec![generated, dense],
            coverage_role: Some("source evidence".to_string()),
            eligible_for_sufficiency: Some(true),
        }];

        decorate_packet_claims_proof_metadata(&mut claims);

        assert_eq!(
            claims[0].proof_status,
            Some(PacketProofStatusDto::Diagnostic)
        );
        assert_eq!(
            claims[0].required_evidence_role,
            Some(PacketEvidenceTierDto::ExactSource)
        );

        let mut exact_source = test_citation("dispatch", "src/runtime.rs", 1.0);
        exact_source.evidence_tier = Some(PacketEvidenceTierDto::ExactSource);
        exact_source.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
        exact_source.eligible_for_sufficiency = Some(true);
        claims[0].citations.push(exact_source);

        decorate_packet_claims_proof_metadata(&mut claims);

        assert_eq!(claims[0].proof_status, Some(PacketProofStatusDto::Proven));
        assert_eq!(
            claims[0].required_evidence_role,
            Some(PacketEvidenceTierDto::ExactSource)
        );
    }

    #[test]
    fn production_source_evidence_does_not_emit_ties_boilerplate() {
        let answer = test_answer(
            "Explain how Logger.addRecord writes a record through handlers.",
            vec![
                test_citation("Logger.addRecord", "src/Logger.php", 0.9),
                test_citation("AbstractProcessingHandler.handle", "src/Handler.php", 0.8),
            ],
        );
        let claims = packet_supported_claims(&answer);
        for claim in &claims {
            assert!(
                !claim.claim.contains("ties ") && !claim.claim.contains("adjacent ownership"),
                "ranked source-evidence claims must omit navigation boilerplate: {claim:?}"
            );
        }
    }

    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn sql_relationship_claims_attach_to_retained_foreign_key_citations() {
        let answer = test_answer(
            "Explain SQL schema relationships between child and parent rows.",
            vec![
                test_citation("CREATE TABLE Child", "db/schema.sql", 0.9),
                test_citation("FOREIGN KEY", "db/schema.sql", 0.8),
            ],
        );

        let claims = packet_supported_claims(&answer);
        let relationship_claim = claims
            .iter()
            .find(|claim| claim.coverage_role.as_deref() == Some("sql relationship constraint"))
            .unwrap_or_else(|| panic!("expected relationship claim in {claims:?}"));
        assert!(
            relationship_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "FOREIGN KEY"),
            "relationship claim should cite retained FK evidence: {relationship_claim:?}"
        );
        assert!(
            !relationship_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "CREATE TABLE Child"),
            "relationship claim should not stay attached only to table evidence: {relationship_claim:?}"
        );

        let table_claim = claims
            .iter()
            .find(|claim| claim.coverage_role.as_deref() == Some("sql table definition"))
            .unwrap_or_else(|| panic!("expected table claim in {claims:?}"));
        assert!(
            table_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "CREATE TABLE Child"),
            "table claim should keep table-definition evidence: {table_claim:?}"
        );
    }

    #[ignore = "domain role/carrier taxonomy removed (phase9-r2)"]
    #[test]
    fn sql_relationship_claims_can_attach_to_retained_references_citations() {
        let answer = test_answer(
            "Explain SQL schema relationships and references between child and parent rows.",
            vec![
                test_citation("CREATE TABLE Child", "db/schema.sql", 0.9),
                test_citation("REFERENCES", "db/schema.sql", 0.8),
            ],
        );

        let claims = packet_supported_claims(&answer);
        let relationship_claim = claims
            .iter()
            .find(|claim| claim.coverage_role.as_deref() == Some("sql relationship constraint"))
            .unwrap_or_else(|| panic!("expected relationship claim in {claims:?}"));
        assert!(
            relationship_claim
                .citations
                .iter()
                .any(|citation| citation.display_name == "REFERENCES"),
            "relationship claim should cite retained REFERENCES evidence: {relationship_claim:?}"
        );
    }
}
