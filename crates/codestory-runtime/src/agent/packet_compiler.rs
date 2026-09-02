//! Runtime adapter for the pure repository-derived packet compiler.
//!
//! Runtime owns publication checks and converts pinned repository records into
//! [`PacketCompilationInputV1`]. Selection itself lives in
//! `codestory-agent` and cannot see the question.

use crate::agent::packet_candidate::PacketProofSession;
use crate::agent::packet_coverage::PacketCoverageInput;
use crate::agent::packet_freshness::PacketFreshnessInput;
use crate::agent::packet_scoring::packet_display_path;
use crate::{AppController, BoundedSnippetRangeOptions};
use codestory_agent::evidence_compiler::{
    RepositoryDerivedCompilationV1, compile_repository_evidence,
};
use codestory_contracts::api::{
    AgentPacketDto, AgentPacketRequestDto, BoundedDrillPlanDto, DrillGapKindDto, DrillOptionDto,
    EmbeddingVectorPublicationIdentityDto, PACKET_DRILL_MAX_BYTES, PACKET_DRILL_MAX_DEPTH,
    PACKET_DRILL_MAX_HITS, PACKET_DRILL_MAX_OPTIONS, PacketDispositionDto,
    PacketProbeResolutionDto, PacketProbeResolutionStatusDto, SourceCoverageNotEstablishedCauseDto,
    SourceCoverageObservationDto, SourceCoverageStatusDto, SupportUnitDto, SupportUnitKindDto,
    decode_drill_option_id,
};
use codestory_contracts::compilation::{
    INTERIM_SOURCE_ROW_UPPER_BOUND, PACKET_COMPILATION_CONTRACT_VERSION_V1,
    PacketAdmissionGapKindV1, PacketAdmissionGapV1, PacketAdmissionOriginV1,
    PacketAdmissionReceiptV1, PacketCompilationInputV1, PacketCompilationPublicationV1,
    PacketContinuationSelectorV1, PacketDirectedRelationV1, PacketHydratedSourceRangeV1,
    PacketIdentityAmbiguityV1, PacketParserCompletenessV1, PacketRelationCertaintyV1,
    PacketRelationKindV1, PacketStructuralGapReasonV1,
};
use codestory_contracts::graph::{
    EdgeKind as CoreEdgeKind, FileCoverageReason, Node as CoreNode, NodeId as CoreNodeId,
    NodeKind as CoreNodeKind, ResolutionCertainty,
};
use codestory_store::{FileInfo, Store};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const COMPILER_SOURCE_TRUNCATION_SUFFIX: &str = "\n// ... source truncated by packet row cap\n```";

pub(crate) struct FrozenPacketCompilationV1 {
    pub(crate) product: RepositoryDerivedCompilationV1,
    pub(crate) source_coverage: Vec<SourceCoverageObservationDto>,
    publication: PacketCompilationPublicationV1,
}

/// Hydrate exactly the packet-wide admitted identities and compile their
/// repository evidence while the core/retrieval publication is pinned. This
/// runs before any presentation or output-budget mutation.
pub(crate) fn freeze_packet_compilation(
    controller: &AppController,
    project_id: &str,
    probe_resolutions: &[PacketProbeResolutionDto],
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
    session: &PacketProofSession,
) -> Result<FrozenPacketCompilationV1, codestory_contracts::api::ApiError> {
    let admissions = session.receipts();
    let mut admission_gaps = session.gaps();
    let storage = controller.open_storage_read_only()?;
    let (admissions, mut sources) =
        hydrate_admitted_sources(controller, &storage, &admissions, &mut admission_gaps)?;
    let source_paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let source_coverage = observe_admitted_source_coverage(controller, &storage, &source_paths);
    for source in &mut sources {
        source.parser_completeness = parser_completeness_for_path(&source.path, &source_coverage);
    }
    let relations = hydrate_induced_relations(&storage, &admissions)?;
    let publication = PacketCompilationPublicationV1 {
        project_id: project_id.to_string(),
        core_generation_id: publication
            .map(|publication| publication.core_generation_id.clone())
            .unwrap_or_default(),
        retrieval_generation: publication
            .map(|publication| publication.retrieval_generation.clone()),
    };
    let input = PacketCompilationInputV1 {
        contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
        publication: publication.clone(),
        admissions,
        sources,
        relations,
        ambiguities: probe_ambiguities(probe_resolutions),
        admission_gaps,
    };
    let product = compile_repository_evidence(&input);
    Ok(FrozenPacketCompilationV1 {
        product,
        source_coverage,
        publication,
    })
}

pub(crate) fn apply_frozen_packet_compilation(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    frozen: FrozenPacketCompilationV1,
) {
    packet.support = frozen.product.support;
    packet.disposition = classify_packet_disposition(
        packet,
        request,
        &frozen.product.continuation,
        frozen.publication.core_generation_id,
        frozen.publication.retrieval_generation,
    );
}

fn hydrate_admitted_sources(
    controller: &AppController,
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
) -> Result<
    (
        Vec<PacketAdmissionReceiptV1>,
        Vec<PacketHydratedSourceRangeV1>,
    ),
    codestory_contracts::api::ApiError,
> {
    let project_root = controller.require_project_root()?;
    let mut authenticated = Vec::new();
    let mut sources = Vec::new();
    for admission in admissions {
        let result = if let Some(raw_id) = admission.stable_identity.strip_prefix("node:") {
            let Some(node) = authenticated_node(storage, raw_id)? else {
                push_admission_gap(
                    admission_gaps,
                    admission,
                    PacketAdmissionGapKindV1::StableIdentityMissing,
                    false,
                );
                continue;
            };
            authenticated.push(admission.clone());
            hydrate_admitted_node_source(controller, storage, admission, &node)
        } else if let Some(path) = admission.stable_identity.strip_prefix("path:") {
            let Some(file) = find_admitted_file(storage, &project_root, path)? else {
                push_admission_gap(
                    admission_gaps,
                    admission,
                    PacketAdmissionGapKindV1::StableIdentityMissing,
                    false,
                );
                continue;
            };
            authenticated.push(admission.clone());
            hydrate_admitted_file_source(controller, admission, &file)
        } else {
            push_admission_gap(
                admission_gaps,
                admission,
                PacketAdmissionGapKindV1::StableIdentityMissing,
                false,
            );
            continue;
        };
        match result {
            Ok(source) => sources.push(source),
            Err(kind) => push_admission_gap(admission_gaps, admission, kind, true),
        }
    }
    Ok((authenticated, sources))
}

fn push_admission_gap(
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
    admission: &PacketAdmissionReceiptV1,
    kind: PacketAdmissionGapKindV1,
    expose_authenticated_identity: bool,
) {
    admission_gaps.push(PacketAdmissionGapV1 {
        kind,
        stable_identity: expose_authenticated_identity.then(|| admission.stable_identity.clone()),
        exact_selector_ordinal: (admission.origin == PacketAdmissionOriginV1::ExactTypedSelector)
            .then_some(admission.packet_ordinal),
    });
}

fn authenticated_node(
    storage: &Store,
    raw_id: &str,
) -> Result<Option<CoreNode>, codestory_contracts::api::ApiError> {
    let Ok(node_id) = raw_id.parse::<i64>() else {
        return Ok(None);
    };
    storage.get_node(CoreNodeId(node_id)).map_err(|error| {
        codestory_contracts::api::ApiError::internal(format!(
            "Failed to authenticate admitted packet node: {error}"
        ))
    })
}

fn hydrate_admitted_node_source(
    controller: &AppController,
    storage: &Store,
    admission: &PacketAdmissionReceiptV1,
    node: &CoreNode,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    let file_id = if node.kind == CoreNodeKind::FILE {
        node.id
    } else {
        node.file_node_id
            .ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?
    };
    let file = storage
        .get_file_by_id(file_id.0)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?
        .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?;
    if node.kind == CoreNodeKind::FILE {
        return hydrate_admitted_file_source(controller, admission, &file);
    }
    let (start_line, end_line) = valid_source_bounds(node.start_line, node.end_line)
        .ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?;
    let (_, bounded) = controller
        .bounded_file_snippet_range(
            &file.path.to_string_lossy(),
            BoundedSnippetRangeOptions {
                focus_line: start_line,
                start_line,
                end_line,
                context_lines: 0,
                max_bytes: source_byte_cap(admission),
                truncation_suffix: COMPILER_SOURCE_TRUNCATION_SUFFIX,
            },
        )
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    hydrated_source(
        admission,
        &file.path.to_string_lossy(),
        Some(
            node.qualified_name
                .clone()
                .unwrap_or_else(|| node.serialized_name.clone()),
        ),
        &bounded.markdown,
    )
}

fn hydrate_admitted_file_source(
    controller: &AppController,
    admission: &PacketAdmissionReceiptV1,
    file: &FileInfo,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    let path = file.path.to_string_lossy();
    let (_, bounded) = controller
        .bounded_file_snippet(
            &path,
            1,
            8,
            source_byte_cap(admission),
            COMPILER_SOURCE_TRUNCATION_SUFFIX,
        )
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    hydrated_source(admission, &path, None, &bounded.markdown)
}

fn hydrated_source(
    admission: &codestory_contracts::compilation::PacketAdmissionReceiptV1,
    path: &str,
    symbol: Option<String>,
    source: &str,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    if source.trim().is_empty() {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let (start_line, end_line) =
        source_receipt_line_range(source).ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?;
    Ok(PacketHydratedSourceRangeV1 {
        stable_identity: admission.stable_identity.clone(),
        path: packet_display_path(path),
        symbol,
        start_line,
        end_line,
        source: source.to_string(),
        parser_completeness: PacketParserCompletenessV1::Unknown,
    })
}

fn source_byte_cap(admission: &PacketAdmissionReceiptV1) -> usize {
    (admission.reserved_source_bytes as usize).clamp(1, INTERIM_SOURCE_ROW_UPPER_BOUND)
}

fn find_admitted_file(
    storage: &Store,
    project_root: &Path,
    path: &str,
) -> Result<Option<FileInfo>, codestory_contracts::api::ApiError> {
    for candidate in admitted_file_lookup_paths(project_root, path) {
        let file = storage.get_file_by_path(&candidate).map_err(|error| {
            codestory_contracts::api::ApiError::internal(format!(
                "Failed to authenticate admitted packet path: {error}"
            ))
        })?;
        if file.is_some() {
            return Ok(file);
        }
    }
    Ok(None)
}

fn admitted_file_lookup_paths(project_root: &Path, path: &str) -> Vec<PathBuf> {
    let candidate = PathBuf::from(path);
    let mut paths = vec![candidate.clone()];
    if !candidate.is_absolute() {
        let joined = project_root.join(candidate);
        if !paths.contains(&joined) {
            paths.push(joined);
        }
    }
    paths
}

fn valid_source_bounds(start_line: Option<u32>, end_line: Option<u32>) -> Option<(u32, u32)> {
    let start_line = start_line.filter(|line| *line > 0)?;
    let end_line = end_line.filter(|line| *line >= start_line)?;
    Some((start_line, end_line))
}

fn source_receipt_line_range(markdown: &str) -> Option<(u32, u32)> {
    let mut start = None;
    let mut end = None;
    for line in markdown.lines() {
        let line = line
            .trim_start()
            .strip_prefix("> ")
            .unwrap_or(line.trim_start());
        let Some((line_number, _)) = line.split_once(" | ") else {
            continue;
        };
        let Ok(line_number) = line_number.trim().parse::<u32>() else {
            continue;
        };
        start = Some(start.map_or(line_number, |current: u32| current.min(line_number)));
        end = Some(end.map_or(line_number, |current: u32| current.max(line_number)));
    }
    start.zip(end)
}

fn observe_admitted_source_coverage(
    controller: &AppController,
    storage: &Store,
    paths: &[String],
) -> Vec<SourceCoverageObservationDto> {
    let Ok(project_root) = controller.require_project_root() else {
        return paths
            .iter()
            .map(|path| source_coverage_not_established(path))
            .collect();
    };
    let mut seen = BTreeSet::new();
    paths
        .iter()
        .filter(|path| seen.insert(packet_display_path(path)))
        .map(|path| {
            observe_one_admitted_source_coverage(storage, &project_root, path)
                .unwrap_or_else(|_| source_coverage_not_established(path))
        })
        .collect()
}

fn observe_one_admitted_source_coverage(
    storage: &Store,
    project_root: &Path,
    path: &str,
) -> Result<SourceCoverageObservationDto, codestory_store::StorageError> {
    let mut file = None;
    for candidate in admitted_file_lookup_paths(project_root, path) {
        if let Some(found) = storage.get_file_by_path(&candidate)? {
            file = Some(found);
            break;
        }
    }
    let Some(file) = file else {
        return Ok(source_coverage_not_established(path));
    };
    let relative_path = codestory_workspace::workspace_relative_path(project_root, &file.path)
        .unwrap_or_else(|| file.path.clone())
        .to_string_lossy()
        .replace('\\', "/");
    if storage.has_source_policy_exclusion_path(&relative_path)? {
        return Ok(SourceCoverageObservationDto {
            path: path.to_string(),
            status: SourceCoverageStatusDto::PolicyExcluded,
            reason: None,
            not_established_cause: None,
            observed_size: None,
            byte_cap: None,
        });
    }

    let verified_source = storage.get_file_content_hash(file.id)?.is_some();
    let structural_projection = if file.language == "openapi" {
        storage.has_file_owned_openapi_endpoint_projection(file.id)?
    } else if codestory_indexer::structural::is_structural_candidate_path(&file.path) {
        storage.has_structural_text_projection_for_file(file.id)?
    } else {
        true
    };
    let errors = storage.get_file_coverage_reasons(file.id)?;
    let reason = if !file.complete || !file.indexed || !verified_source || !structural_projection {
        errors
            .first()
            .copied()
            .or_else(|| {
                (file.indexed && verified_source && !file.complete)
                    .then_some(FileCoverageReason::ParserPartial)
            })
            .or(Some(FileCoverageReason::CollectorFailure))
    } else {
        None
    };
    Ok(SourceCoverageObservationDto {
        path: path.to_string(),
        status: if reason.is_some() {
            SourceCoverageStatusDto::Incomplete
        } else {
            SourceCoverageStatusDto::Indexed
        },
        reason,
        not_established_cause: None,
        observed_size: None,
        byte_cap: None,
    })
}

fn source_coverage_not_established(path: &str) -> SourceCoverageObservationDto {
    SourceCoverageObservationDto {
        path: path.to_string(),
        status: SourceCoverageStatusDto::NotEstablished,
        reason: None,
        not_established_cause: Some(SourceCoverageNotEstablishedCauseDto::LookupUnavailable),
        observed_size: None,
        byte_cap: None,
    }
}

fn parser_completeness_for_path(
    path: &str,
    source_coverage: &[SourceCoverageObservationDto],
) -> PacketParserCompletenessV1 {
    source_coverage
        .iter()
        .find(|observation| packet_display_path(&observation.path) == packet_display_path(path))
        .map(|observation| match observation.status {
            SourceCoverageStatusDto::Indexed => PacketParserCompletenessV1::Complete,
            SourceCoverageStatusDto::Incomplete => PacketParserCompletenessV1::Partial,
            SourceCoverageStatusDto::PolicyExcluded | SourceCoverageStatusDto::NotEstablished => {
                PacketParserCompletenessV1::Unknown
            }
        })
        .unwrap_or(PacketParserCompletenessV1::Unknown)
}

fn hydrate_induced_relations(
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
) -> Result<Vec<PacketDirectedRelationV1>, codestory_contracts::api::ApiError> {
    let node_ids = admissions
        .iter()
        .filter_map(|admission| admission.stable_identity.strip_prefix("node:"))
        .filter_map(|raw_id| raw_id.parse::<i64>().ok())
        .map(CoreNodeId)
        .collect::<Vec<_>>();
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    storage
        .get_certain_edge_representatives_between_node_ids(&node_ids)
        .map_err(|error| {
            codestory_contracts::api::ApiError::internal(format!(
                "Failed to load admitted packet relations: {error}"
            ))
        })
        .map(|edges| {
            edges
                .into_iter()
                .map(|edge| {
                    let (from, to) = edge.effective_endpoints();
                    PacketDirectedRelationV1 {
                        relation_id: edge.id.0.to_string(),
                        from_identity: format!("node:{}", from.0),
                        to_identity: format!("node:{}", to.0),
                        relation_kind: packet_relation_kind(edge.kind),
                        certainty: relation_certainty(edge.certainty),
                    }
                })
                .collect()
        })
}

fn packet_relation_kind(kind: CoreEdgeKind) -> PacketRelationKindV1 {
    use CoreEdgeKind as EdgeKind;
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

fn relation_certainty(certainty: Option<ResolutionCertainty>) -> PacketRelationCertaintyV1 {
    match certainty {
        Some(ResolutionCertainty::Certain) => PacketRelationCertaintyV1::Certain,
        Some(ResolutionCertainty::Probable) => PacketRelationCertaintyV1::Probable,
        Some(ResolutionCertainty::Uncertain) => PacketRelationCertaintyV1::Uncertain,
        _ => PacketRelationCertaintyV1::Unknown,
    }
}

fn probe_ambiguities(
    probe_resolutions: &[PacketProbeResolutionDto],
) -> Vec<PacketIdentityAmbiguityV1> {
    probe_resolutions
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
            relation_certainty(Some(ResolutionCertainty::Uncertain)),
            PacketRelationCertaintyV1::Uncertain
        );
    }

    #[test]
    fn numeric_confidence_cannot_upgrade_missing_certainty() {
        assert_eq!(relation_certainty(None), PacketRelationCertaintyV1::Unknown);
    }

    #[test]
    fn missing_or_invalid_source_bounds_never_become_line_one_source() {
        for (start_line, end_line) in [
            (None, Some(3)),
            (Some(3), None),
            (Some(0), Some(3)),
            (Some(4), Some(3)),
        ] {
            assert_eq!(valid_source_bounds(start_line, end_line), None);
        }
        assert_eq!(valid_source_bounds(Some(3), Some(4)), Some((3, 4)));
    }

    #[test]
    fn public_symbol_identity_requires_a_node_in_the_pinned_core() {
        let mut storage = Store::new_in_memory().expect("store");
        assert!(
            authenticated_node(&storage, "not-an-id")
                .expect("invalid identities are rejected")
                .is_none()
        );
        assert!(
            authenticated_node(&storage, "-42")
                .expect("missing identities are rejected")
                .is_none()
        );
        storage
            .insert_nodes_batch(&[CoreNode {
                id: CoreNodeId(-42),
                kind: CoreNodeKind::FUNCTION,
                serialized_name: "crate::real".into(),
                ..Default::default()
            }])
            .expect("insert authenticated node");
        assert_eq!(
            authenticated_node(&storage, "-42")
                .expect("lookup")
                .expect("authenticated node")
                .id,
            CoreNodeId(-42)
        );
    }

    #[test]
    fn source_receipts_require_observed_numbered_lines() {
        assert_eq!(source_receipt_line_range("source without a receipt"), None);
        assert_eq!(
            source_receipt_line_range("```text\n>    7 | fn run() {}\n     8 | }\n```"),
            Some((7, 8))
        );
    }
}
