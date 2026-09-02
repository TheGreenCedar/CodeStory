//! Runtime adapter for the pure repository-derived packet compiler.
//!
//! Runtime owns publication checks and converts pinned repository records into
//! [`PacketCompilationInputV1`]. Selection itself lives in
//! `codestory-agent` and cannot see the question.

use crate::agent::packet_candidate::{PacketProofSession, active_packet_proof_session};
use crate::agent::packet_coverage::PacketCoverageInput;
use crate::agent::packet_freshness::PacketFreshnessInput;
use crate::agent::packet_scoring::packet_display_path;
use codestory_agent::evidence_compiler::compile_repository_evidence;
use codestory_contracts::api::{
    AgentPacketDto, AgentPacketRequestDto, BoundedDrillPlanDto, DrillGapKindDto, DrillOptionDto,
    GraphArtifactDto, PACKET_DRILL_MAX_BYTES, PACKET_DRILL_MAX_DEPTH, PACKET_DRILL_MAX_HITS,
    PACKET_DRILL_MAX_OPTIONS, PacketDispositionDto, PacketProbeResolutionStatusDto,
    SourceCoverageObservationDto, SourceCoverageStatusDto, SupportUnitDto, SupportUnitKindDto,
    decode_drill_option_id,
};
use codestory_contracts::compilation::{
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PacketAdmissionGapKindV1, PacketAdmissionGapV1,
    PacketAdmissionOriginV1, PacketCompilationInputV1, PacketCompilationPublicationV1,
    PacketContinuationSelectorV1, PacketDirectedRelationV1, PacketHydratedSourceRangeV1,
    PacketIdentityAmbiguityV1, PacketParserCompletenessV1, PacketRelationCertaintyV1,
    PacketRelationKindV1, PacketStructuralGapReasonV1,
};
use codestory_contracts::graph::FileCoverageReason;
use std::collections::BTreeSet;

pub fn apply_compiled_evidence_for_project(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    project_id: &str,
    relations: Vec<PacketDirectedRelationV1>,
) {
    let session = active_packet_proof_session();
    let input = packet_compilation_input(packet, project_id, session.as_deref(), relations);
    let compiled = compile_repository_evidence(&input);
    packet.support = compiled.support;
    packet.disposition = classify_packet_disposition(
        packet,
        request,
        &compiled.continuation,
        input.publication.core_generation_id,
        input.publication.retrieval_generation,
    );
}

fn packet_compilation_input(
    packet: &AgentPacketDto,
    project_id: &str,
    session: Option<&PacketProofSession>,
    relations: Vec<PacketDirectedRelationV1>,
) -> PacketCompilationInputV1 {
    let publication = packet.answer.retrieval_trace.retrieval_publication.as_ref();
    let mut admission_gaps = session.map(PacketProofSession::gaps).unwrap_or_default();
    let admissions = session
        .map(PacketProofSession::receipts)
        .unwrap_or_default();
    let sources = hydrated_sources(&packet.support, &packet.answer.source_coverage, &admissions);
    let source_identities = sources
        .iter()
        .map(|source| source.stable_identity.as_str())
        .collect::<BTreeSet<_>>();
    for admission in admissions
        .iter()
        .filter(|admission| admission.origin == PacketAdmissionOriginV1::ExactTypedSelector)
    {
        if !source_identities.contains(admission.stable_identity.as_str()) {
            admission_gaps.push(PacketAdmissionGapV1 {
                kind: PacketAdmissionGapKindV1::SourceUnavailable,
                stable_identity: Some(admission.stable_identity.clone()),
                exact_selector_ordinal: Some(admission.packet_ordinal),
            });
        }
    }

    PacketCompilationInputV1 {
        contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
        publication: PacketCompilationPublicationV1 {
            project_id: project_id.to_string(),
            core_generation_id: publication
                .map(|publication| publication.core_generation_id.clone())
                .unwrap_or_default(),
            retrieval_generation: publication
                .map(|publication| publication.retrieval_generation.clone()),
        },
        relations,
        ambiguities: probe_ambiguities(packet),
        admissions,
        sources,
        admission_gaps,
    }
}

fn hydrated_sources(
    support: &[SupportUnitDto],
    source_coverage: &[SourceCoverageObservationDto],
    admissions: &[codestory_contracts::compilation::PacketAdmissionReceiptV1],
) -> Vec<PacketHydratedSourceRangeV1> {
    support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
        .filter_map(|unit| {
            let path = unit.path.as_deref().map(packet_display_path)?;
            let source = unit.snippet.clone()?;
            let stable_identity = support_stable_identity(unit, &path, admissions)?;
            let parser_completeness = source_coverage
                .iter()
                .find(|observation| packet_display_path(&observation.path) == path)
                .map(|observation| match observation.status {
                    SourceCoverageStatusDto::Indexed => PacketParserCompletenessV1::Complete,
                    SourceCoverageStatusDto::Incomplete => PacketParserCompletenessV1::Partial,
                    SourceCoverageStatusDto::PolicyExcluded
                    | SourceCoverageStatusDto::NotEstablished => {
                        PacketParserCompletenessV1::Unknown
                    }
                })
                .unwrap_or(PacketParserCompletenessV1::Unknown);
            Some(PacketHydratedSourceRangeV1 {
                stable_identity,
                path,
                symbol: (!unit.summary.is_empty()).then_some(unit.summary.clone()),
                start_line: unit.start_line.unwrap_or(1),
                end_line: unit.end_line.or(unit.start_line).unwrap_or(1),
                source,
                parser_completeness,
            })
        })
        .collect()
}

fn support_stable_identity(
    unit: &SupportUnitDto,
    path: &str,
    admissions: &[codestory_contracts::compilation::PacketAdmissionReceiptV1],
) -> Option<String> {
    if let Some(symbol_id) = unit.symbol_id.as_deref() {
        for candidate in [symbol_id.to_string(), format!("node:{symbol_id}")] {
            if admissions
                .iter()
                .any(|admission| admission.stable_identity == candidate)
            {
                return Some(candidate);
            }
        }
    }
    admissions
        .iter()
        .find(|admission| admission.stable_identity == format!("path:{path}"))
        .map(|admission| admission.stable_identity.clone())
}

pub(crate) fn directed_relations_from_graphs(
    graphs: &[GraphArtifactDto],
    admissions: &[codestory_contracts::compilation::PacketAdmissionReceiptV1],
) -> Vec<PacketDirectedRelationV1> {
    let admitted = admissions
        .iter()
        .map(|admission| admission.stable_identity.as_str())
        .collect::<BTreeSet<_>>();
    let mut relations = Vec::new();
    let mut seen = BTreeSet::new();
    for artifact in graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        for edge in &graph.edges {
            let from_identity = format!("node:{}", edge.source.0);
            let to_identity = format!("node:{}", edge.target.0);
            if !admitted.contains(from_identity.as_str())
                || !admitted.contains(to_identity.as_str())
            {
                continue;
            }
            if !seen.insert(edge.id.0.clone()) {
                continue;
            }
            relations.push(PacketDirectedRelationV1 {
                relation_id: edge.id.0.clone(),
                from_identity,
                to_identity,
                relation_kind: packet_relation_kind(edge.kind),
                certainty: relation_certainty(edge.certainty.as_deref(), edge.confidence),
            });
        }
    }
    relations
}

fn packet_relation_kind(kind: codestory_contracts::api::EdgeKind) -> PacketRelationKindV1 {
    use codestory_contracts::api::EdgeKind;
    match kind {
        EdgeKind::MEMBER => PacketRelationKindV1::Member,
        EdgeKind::TYPE_USAGE => PacketRelationKindV1::TypeUsage,
        EdgeKind::USAGE => PacketRelationKindV1::Usage,
        EdgeKind::CALL => PacketRelationKindV1::Call,
        EdgeKind::INHERITANCE => PacketRelationKindV1::Inheritance,
        EdgeKind::OVERRIDE => PacketRelationKindV1::Override,
        EdgeKind::TYPE_ARGUMENT => PacketRelationKindV1::TypeArgument,
        EdgeKind::TEMPLATE_SPECIALIZATION => PacketRelationKindV1::TemplateSpecialization,
        EdgeKind::INCLUDE => PacketRelationKindV1::Include,
        EdgeKind::IMPORT => PacketRelationKindV1::Import,
        EdgeKind::MACRO_USAGE => PacketRelationKindV1::MacroUsage,
        EdgeKind::ANNOTATION_USAGE => PacketRelationKindV1::AnnotationUsage,
        EdgeKind::UNKNOWN => PacketRelationKindV1::Unknown,
    }
}

fn relation_certainty(label: Option<&str>, _confidence: Option<f32>) -> PacketRelationCertaintyV1 {
    match label {
        Some("certain") => PacketRelationCertaintyV1::Certain,
        Some("probable") => PacketRelationCertaintyV1::Probable,
        Some("uncertain") => PacketRelationCertaintyV1::Uncertain,
        _ => PacketRelationCertaintyV1::Unknown,
    }
}

fn probe_ambiguities(packet: &AgentPacketDto) -> Vec<PacketIdentityAmbiguityV1> {
    packet
        .plan
        .probe_resolutions
        .iter()
        .filter(|resolution| resolution.status == PacketProbeResolutionStatusDto::Ambiguous)
        .map(|resolution| PacketIdentityAmbiguityV1 {
            selector: format!("probe:{}", resolution.input_index),
            candidate_identities: resolution
                .candidates
                .iter()
                .map(|candidate| format!("node:{}", candidate.symbol_id))
                .collect(),
        })
        .collect()
}

fn classify_packet_disposition(
    packet: &AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    continuation: &[PacketContinuationSelectorV1],
    core_generation_id: String,
    retrieval_generation: Option<String>,
) -> PacketDispositionDto {
    if let Some(request) = request {
        if let Some(expected) = request.core_generation_id.as_deref()
            && expected != core_generation_id
        {
            return PacketDispositionDto::unavailable("pinned core publication changed");
        }
        if let Some(expected) = request.retrieval_generation.as_deref()
            && Some(expected) != retrieval_generation.as_deref()
        {
            return PacketDispositionDto::unavailable("pinned retrieval publication changed");
        }
    }

    let freshness = PacketFreshnessInput::from_observation(packet.answer.freshness.as_ref());
    if freshness.blocks_packet_availability() {
        return PacketDispositionDto::unavailable(
            freshness
                .gap()
                .unwrap_or_else(|| "publication freshness is not established".to_string()),
        );
    }
    let coverage = packet_coverage_for_disposition(&packet.answer.source_coverage, &packet.support);
    if coverage.blocks_packet_availability() {
        return PacketDispositionDto::unavailable(
            coverage
                .gaps()
                .into_iter()
                .next()
                .unwrap_or_else(|| "source coverage is not established".to_string()),
        );
    }
    if packet.answer.retrieval_trace.steps.iter().any(|step| {
        matches!(
            step.status,
            codestory_contracts::api::AgentRetrievalStepStatusDto::Error
        )
    }) {
        return PacketDispositionDto::unavailable("retrieval recorded a hard error");
    }

    let already_drilled = request.is_some_and(|request| {
        request.parent_packet_id.is_some() || !request.option_ids.is_empty()
    });
    if !already_drilled {
        let options = continuation
            .iter()
            .filter_map(drill_option_from_selector)
            .take(PACKET_DRILL_MAX_OPTIONS)
            .collect::<Vec<_>>();
        if !options.is_empty() {
            return PacketDispositionDto::drill_once(
                "bounded structural continuation available",
                BoundedDrillPlanDto {
                    parent_packet_id: packet.packet_id.clone(),
                    core_generation_id,
                    retrieval_generation,
                    gap_ids: options.iter().map(|option| option.gap_id.clone()).collect(),
                    options,
                    max_bytes: PACKET_DRILL_MAX_BYTES,
                    max_hits: PACKET_DRILL_MAX_HITS,
                    max_depth: PACKET_DRILL_MAX_DEPTH,
                    remaining_rounds: 1,
                },
            );
        }
    }

    if packet.support.is_empty() {
        PacketDispositionDto::not_established("no bounded repository evidence was retained")
    } else if already_drilled && !continuation.is_empty() {
        PacketDispositionDto::not_established("the bounded continuation left a structural gap")
    } else {
        // This legacy internal state means only that positive evidence exists.
        // Public v3 never projects it as answer sufficiency.
        PacketDispositionDto::supported()
    }
}

fn packet_coverage_for_disposition(
    observations: &[codestory_contracts::api::SourceCoverageObservationDto],
    support: &[SupportUnitDto],
) -> PacketCoverageInput {
    let verified_source_paths = support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
        .filter(|unit| {
            unit.snippet
                .as_deref()
                .is_some_and(|snippet| !snippet.is_empty())
        })
        .filter_map(|unit| unit.path.as_deref().map(packet_display_path))
        .collect::<BTreeSet<_>>();
    let blocking = observations
        .iter()
        .filter(|observation| {
            observation.status != SourceCoverageStatusDto::Incomplete
                || observation.reason != Some(FileCoverageReason::ParserPartial)
                || !verified_source_paths.contains(&packet_display_path(&observation.path))
        })
        .cloned()
        .collect::<Vec<_>>();
    PacketCoverageInput::from_observations(&blocking)
}

fn drill_option_from_selector(selector: &PacketContinuationSelectorV1) -> Option<DrillOptionDto> {
    let gap_id = format!(
        "{}:{}",
        structural_reason_label(selector.reason),
        selector.stable_identity
    );
    let mut option = if let Some(path) = selector
        .path
        .as_deref()
        .or_else(|| selector.stable_identity.strip_prefix("path:"))
    {
        DrillOptionDto::bounded_source_read(gap_id, path)
    } else {
        let symbol_id = selector
            .symbol_id
            .as_deref()
            .or_else(|| selector.stable_identity.strip_prefix("node:"))?;
        DrillOptionDto::omitted_symbol(gap_id, symbol_id)
    };
    option.structural_reason = Some(selector.reason);
    Some(option)
}

fn structural_reason_label(reason: PacketStructuralGapReasonV1) -> &'static str {
    match reason {
        PacketStructuralGapReasonV1::CandidateCountExceeded => "candidate_count_exceeded",
        PacketStructuralGapReasonV1::SourceBudgetExceeded => "source_budget_exceeded",
        PacketStructuralGapReasonV1::SourceUnavailable => "source_unavailable",
        PacketStructuralGapReasonV1::AmbiguousSelector => "ambiguous_selector",
        PacketStructuralGapReasonV1::DisconnectedSeed => "disconnected_seed",
    }
}

/// Decode only stable path or symbol continuations. Historical query-text
/// options are deliberately not reintroduced as retrieval policy.
pub fn drill_options_from_ids(option_ids: &[String]) -> Vec<DrillOptionDto> {
    option_ids
        .iter()
        .filter_map(|id| {
            let (kind, target) = decode_drill_option_id(id)?;
            match kind {
                DrillGapKindDto::BoundedSourceRead => Some(DrillOptionDto::bounded_source_read(
                    format!("source_unavailable:{target}"),
                    target,
                )),
                DrillGapKindDto::OmittedMandatorySupport => {
                    if let Some(path) = target.strip_prefix("path:") {
                        Some(DrillOptionDto::omitted_source_path(
                            format!("disconnected_seed:{path}"),
                            path,
                        ))
                    } else {
                        let symbol = target.strip_prefix("symbol:")?;
                        Some(DrillOptionDto::omitted_symbol(
                            format!("disconnected_seed:{symbol}"),
                            symbol,
                        ))
                    }
                }
            }
        })
        .take(PACKET_DRILL_MAX_OPTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{EdgeId, EdgeKind, GraphEdgeDto, GraphResponse, NodeId};
    use codestory_contracts::compilation::{PacketAdmissionOriginV1, PacketAdmissionReceiptV1};

    #[test]
    fn unknown_query_continuations_are_not_decoded() {
        assert!(drill_options_from_ids(&["deadline_lost_candidate:diagnostic".into()]).is_empty());
    }

    #[test]
    fn stable_symbol_continuation_round_trips_without_query_text() {
        let original = DrillOptionDto::omitted_symbol("gap", "node-1");
        let decoded = drill_options_from_ids(&[original.id]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].symbol_id.as_deref(), Some("node-1"));
        assert!(decoded[0].structural_reason.is_some());
    }

    #[test]
    fn uncertain_relation_is_not_compiler_evidence() {
        assert_eq!(
            relation_certainty(Some("uncertain"), Some(0.99)),
            PacketRelationCertaintyV1::Uncertain
        );
    }

    #[test]
    fn numeric_confidence_cannot_upgrade_missing_certainty() {
        assert_eq!(
            relation_certainty(None, Some(0.99)),
            PacketRelationCertaintyV1::Unknown
        );
    }

    #[test]
    fn adapter_forwards_every_admitted_hydrated_source_and_no_unadmitted_source() {
        let admissions = (0..16)
            .map(|index| PacketAdmissionReceiptV1 {
                packet_ordinal: index,
                stable_identity: format!("node:{index}"),
                score_version: "retrieval-score/v1".into(),
                reserved_source_bytes: 32,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect::<Vec<_>>();
        let mut support = (0..16)
            .map(|index| SupportUnitDto {
                id: format!("source-{index}"),
                kind: SupportUnitKindDto::SourceRange,
                summary: format!("symbol-{index}"),
                path: Some(format!("src/{index}.rs")),
                symbol_id: Some(index.to_string()),
                start_line: Some(1),
                end_line: Some(1),
                snippet: Some(format!("fn symbol_{index}() {{}}")),
                edge_kind: None,
                from_symbol: None,
                to_symbol: None,
                query: None,
            })
            .collect::<Vec<_>>();
        support.push(SupportUnitDto {
            id: "source-unadmitted".into(),
            kind: SupportUnitKindDto::SourceRange,
            summary: "unadmitted".into(),
            path: Some("src/unadmitted.rs".into()),
            symbol_id: Some("unadmitted".into()),
            start_line: Some(1),
            end_line: Some(1),
            snippet: Some("fn unadmitted() {}".into()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        });

        let hydrated = hydrated_sources(&support, &[], &admissions);
        assert_eq!(hydrated.len(), 16);
        assert!(
            hydrated
                .iter()
                .all(|source| source.stable_identity.starts_with("node:"))
        );
        assert!(
            hydrated
                .iter()
                .all(|source| source.path != "src/unadmitted.rs")
        );
    }

    #[test]
    fn compiler_relation_capture_precedes_presentation_edge_caps() {
        let admissions = (0..16)
            .map(|index| PacketAdmissionReceiptV1 {
                packet_ordinal: index,
                stable_identity: format!("node:{index}"),
                score_version: "retrieval-score/v1".into(),
                reserved_source_bytes: 32,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect::<Vec<_>>();
        let mut edges = (0..20)
            .map(|index| GraphEdgeDto {
                id: EdgeId(format!("a-dense-{index:02}")),
                source: NodeId("0".into()),
                target: NodeId("0".into()),
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".into()),
                callsite_identity: None,
                candidate_targets: Vec::new(),
            })
            .collect::<Vec<_>>();
        edges.extend((1..16).map(|index| GraphEdgeDto {
            id: EdgeId(format!("z-connect-{index:02}")),
            source: NodeId((index - 1).to_string()),
            target: NodeId(index.to_string()),
            kind: EdgeKind::CALL,
            confidence: Some(1.0),
            certainty: Some("certain".into()),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }));
        let graphs = vec![GraphArtifactDto::Uml {
            id: "graph".into(),
            title: "graph".into(),
            graph: GraphResponse {
                center_id: NodeId("0".into()),
                nodes: Vec::new(),
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        }];

        let captured = directed_relations_from_graphs(&graphs, &admissions);
        assert_eq!(captured.len(), 35);
        assert_eq!(
            captured
                .iter()
                .filter(|relation| relation.relation_id.starts_with("z-connect-"))
                .count(),
            15,
            "dense early edge IDs must not hide the admitted-seed connecting forest"
        );
    }
}
