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
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

const COMPILER_SOURCE_TRUNCATION_SUFFIX: &str = "\n// ... source truncated by packet row cap\n```";
const FILE_NAVIGATION_VERIFICATION_MAX_BYTES: usize = 1024 * 1024;

pub(crate) struct FrozenPacketCompilationV1 {
    pub(crate) product: RepositoryDerivedCompilationV1,
    pub(crate) source_coverage: Vec<SourceCoverageObservationDto>,
    publication: PacketCompilationPublicationV1,
}

#[derive(Debug, Clone)]
struct AuthenticatedPacketAdmissionV1 {
    receipt: PacketAdmissionReceiptV1,
    core_node_id: CoreNodeId,
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
    let (authenticated_admissions, mut sources, file_navigation_paths) =
        hydrate_admitted_sources(controller, &storage, &admissions, &mut admission_gaps)?;
    let source_paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let source_coverage = observe_admitted_source_coverage(controller, &storage, &source_paths);
    for source in &mut sources {
        source.parser_completeness = parser_completeness_for_path(&source.path, &source_coverage);
    }
    let relations = hydrate_induced_relations(&storage, &authenticated_admissions)?;
    let admissions = authenticated_admissions
        .into_iter()
        .map(|admission| admission.receipt)
        .collect();
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
    let mut product = compile_repository_evidence(&input);
    attach_file_navigation_paths(&mut product.support, &file_navigation_paths);
    Ok(FrozenPacketCompilationV1 {
        product,
        source_coverage,
        publication,
    })
}

fn attach_file_navigation_paths(support: &mut [SupportUnitDto], paths: &HashMap<String, String>) {
    for unit in support {
        if unit.kind == SupportUnitKindDto::SymbolLocation
            && let Some(identity) = unit.id.strip_prefix("symbol:")
            && let Some(path) = paths.get(identity)
        {
            unit.path = Some(path.clone());
        }
    }
}

pub(crate) fn apply_frozen_packet_compilation(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    frozen: FrozenPacketCompilationV1,
) {
    packet.support = frozen.product.support;
    crate::agent::packet_batch::observe_packet_raf_final_support(
        &final_support_identities_for_observation(&packet.support),
    );
    packet.disposition = classify_packet_disposition(
        packet,
        request,
        &frozen.product.continuation,
        frozen.publication.core_generation_id,
        frozen.publication.retrieval_generation,
    );
}

fn final_support_identities_for_observation(support: &[SupportUnitDto]) -> Vec<String> {
    let mut identities = Vec::new();
    let mut seen = BTreeSet::new();
    for unit in support {
        let Some(identity) = final_support_stable_identity(unit) else {
            continue;
        };
        if seen.insert(identity.clone()) {
            identities.push(identity);
        }
    }
    identities
}

fn final_support_stable_identity(unit: &SupportUnitDto) -> Option<String> {
    match unit.kind {
        SupportUnitKindDto::SymbolLocation => unit
            .id
            .strip_prefix("symbol:")
            .map(str::to_string)
            .or_else(|| {
                unit.symbol_id
                    .as_ref()
                    .map(|symbol_id| format!("node:{symbol_id}"))
            }),
        SupportUnitKindDto::SourceRange => unit
            .symbol_id
            .as_ref()
            .map(|symbol_id| format!("node:{symbol_id}"))
            .or_else(|| {
                unit.path
                    .as_ref()
                    .filter(|path| !path.trim().is_empty())
                    .map(|path| format!("path:{path}"))
            }),
        SupportUnitKindDto::TypedGraphEdge | SupportUnitKindDto::CompleteQueryNegative => None,
    }
}

fn hydrate_admitted_sources(
    controller: &AppController,
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
) -> Result<
    (
        Vec<AuthenticatedPacketAdmissionV1>,
        Vec<PacketHydratedSourceRangeV1>,
        HashMap<String, String>,
    ),
    codestory_contracts::api::ApiError,
> {
    let project_root = controller.require_project_root()?;
    let mut authenticated = Vec::new();
    let mut sources = Vec::new();
    let mut file_navigation_paths = HashMap::new();
    for admission in admissions {
        let (core_node_id, result) =
            if let Some(raw_id) = admission.stable_identity.strip_prefix("node:") {
                let Some(node) = authenticated_node(storage, raw_id)? else {
                    push_admission_gap(
                        admission_gaps,
                        admission,
                        PacketAdmissionGapKindV1::StableIdentityMissing,
                        false,
                    );
                    continue;
                };
                if let Some(file_id) = (node.kind == CoreNodeKind::FILE)
                    .then_some(node.id)
                    .or(node.file_node_id)
                    && let Some(file) = storage.get_file_by_id(file_id.0).map_err(|error| {
                        codestory_contracts::api::ApiError::internal(format!(
                            "Failed to authenticate admitted packet file: {error}"
                        ))
                    })?
                    && let Some(path) = authenticated_file_navigation_path(controller, &file)
                {
                    file_navigation_paths.insert(admission.stable_identity.clone(), path);
                }
                (
                    node.id,
                    hydrate_admitted_node_source(controller, storage, admission, &node),
                )
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
                if let Some(path) = authenticated_file_navigation_path(controller, &file) {
                    file_navigation_paths.insert(admission.stable_identity.clone(), path);
                }
                (
                    CoreNodeId(file.id),
                    hydrate_admitted_file_source(controller, storage, admission, &file),
                )
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
            Ok(source) => {
                authenticated.push(AuthenticatedPacketAdmissionV1 {
                    receipt: admission.clone(),
                    core_node_id,
                });
                sources.push(source);
            }
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded) => {
                authenticated.push(AuthenticatedPacketAdmissionV1 {
                    receipt: admission.clone(),
                    core_node_id,
                });
                push_admission_gap(
                    admission_gaps,
                    admission,
                    PacketAdmissionGapKindV1::SourceBudgetExceeded,
                    true,
                );
            }
            Err(kind) => {
                file_navigation_paths.remove(&admission.stable_identity);
                push_admission_gap(admission_gaps, admission, kind, true);
            }
        }
    }
    Ok((authenticated, sources, file_navigation_paths))
}

fn authenticated_file_navigation_path(
    controller: &AppController,
    file: &FileInfo,
) -> Option<String> {
    let path = file.path.to_string_lossy();
    let resolved = controller.resolve_project_file_path(&path, false).ok()?;
    let project_root = controller.require_project_root().ok()?;
    let indexed_path = if file.path.is_absolute() {
        file.path.clone()
    } else {
        project_root.join(&file.path)
    };
    if !codestory_workspace::same_workspace_path(&resolved, &indexed_path) {
        return None;
    }
    codestory_workspace::workspace_relative_path(&project_root, &resolved)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
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
        return hydrate_admitted_file_source(controller, storage, admission, &file);
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
    storage: &Store,
    admission: &PacketAdmissionReceiptV1,
    file: &FileInfo,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    // A file identity has no source focus. Preserve a complete short file when
    // its pinned bytes fit the admission; otherwise leave it as navigation.
    // Reading its first few lines would falsely promote an arbitrary header
    // to a source witness for the retrieval question.
    if !file.indexed || !file.complete || file.line_count == 0 {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let expected_hash = storage
        .get_file_content_hash(file.id)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?
        .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?;
    let path = file.path.to_string_lossy();
    let resolved = controller
        .resolve_project_file_path(&path, false)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    let project_root = controller
        .require_project_root()
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    let indexed_path = if file.path.is_absolute() {
        file.path.clone()
    } else {
        project_root.join(&file.path)
    };
    if !codestory_workspace::same_workspace_path(&resolved, &indexed_path) {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let cap = source_byte_cap(admission);
    let mut bytes = Vec::new();
    std::fs::File::open(&resolved)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?
        .take((FILE_NAVIGATION_VERIFICATION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    if bytes.len() > FILE_NAVIGATION_VERIFICATION_MAX_BYTES {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    if format!("{:x}", Sha256::digest(&bytes)) != expected_hash {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    };
    if text.lines().count() != file.line_count as usize {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    if text.len() > cap {
        return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
    }
    let markdown = complete_file_markdown(&text, file.line_count, cap)?;
    hydrated_source(admission, &path, None, &markdown)
}

fn complete_file_markdown(
    text: &str,
    line_count: u32,
    cap: usize,
) -> Result<String, PacketAdmissionGapKindV1> {
    // The focused-snippet helper caps context at 50 lines. File admission is
    // different: it may claim source only when every line fits the row cap.
    let mut markdown = String::from("```text\n");
    for (index, line) in text.lines().enumerate() {
        let marker = if index == 0 { ">" } else { " " };
        writeln!(markdown, "{marker}{:>5} | {line}", index + 1)
            .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
        if markdown.len().saturating_add(3) > cap {
            return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
        }
    }
    markdown.push_str("```");
    if source_receipt_line_range(&markdown) != Some((1, line_count)) {
        return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
    }
    Ok(markdown)
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
    admissions: &[AuthenticatedPacketAdmissionV1],
) -> Result<Vec<PacketDirectedRelationV1>, codestory_contracts::api::ApiError> {
    let mut stable_identity_by_node = HashMap::new();
    for admission in admissions {
        stable_identity_by_node
            .entry(admission.core_node_id)
            .or_insert_with(|| admission.receipt.stable_identity.clone());
    }
    let node_ids = admissions
        .iter()
        .map(|admission| admission.core_node_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
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
                .filter_map(|edge| {
                    let (from, to) = edge.effective_endpoints();
                    Some(PacketDirectedRelationV1 {
                        relation_id: edge.id.0.to_string(),
                        from_identity: stable_identity_by_node.get(&from)?.clone(),
                        to_identity: stable_identity_by_node.get(&to)?.clone(),
                        relation_kind: packet_relation_kind(edge.kind),
                        certainty: relation_certainty(edge.certainty),
                    })
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
    use codestory_contracts::graph::{Edge as CoreEdge, EdgeId as CoreEdgeId};
    use codestory_store::FileRole;

    #[test]
    fn whole_file_renderer_has_no_fifty_line_focus_limit() {
        let source = "\n".repeat(80);
        let rendered = complete_file_markdown(&source, 80, 1024)
            .expect("all eighty numbered lines fit the larger bounded row");
        assert_eq!(source_receipt_line_range(&rendered), Some((1, 80)));
        assert!(rendered.len() <= 1024);
        assert!(matches!(
            complete_file_markdown("x\n", 1, 8),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        ));
    }

    #[test]
    fn file_admission_retains_only_complete_pinned_source_or_navigation() {
        let project = tempfile::tempdir().expect("project");
        let controller = AppController::new();
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let storage = Store::new_in_memory().expect("store");
        let path = project.path().join("settings.rs");
        let admission = PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: "path:settings.rs".into(),
            score_version: "test".into(),
            reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
            origin: PacketAdmissionOriginV1::Retrieval,
        };
        let mut file = FileInfo {
            id: 1,
            path: path.clone(),
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 2,
            file_role: FileRole::Source,
        };
        let short = "const ENABLED: bool = true;\nconst LIMIT: usize = 2;\n";
        std::fs::write(&path, short).expect("short source");
        storage.insert_file(&file).expect("file");
        let short_hash = format!("{:x}", Sha256::digest(short.as_bytes()));
        storage
            .update_file_metadata(&file, Some(&short_hash))
            .expect("pinned source hash");
        let whole = hydrate_admitted_file_source(&controller, &storage, &admission, &file)
            .expect("complete short file source");
        assert_eq!((whole.start_line, whole.end_line), (1, 2));
        assert!(whole.source.contains("const LIMIT: usize = 2;"));

        let many_short_lines = "\n".repeat(45);
        file.line_count = 45;
        std::fs::write(&path, &many_short_lines).expect("short multiline source");
        storage
            .update_file_metadata(
                &file,
                Some(&format!(
                    "{:x}",
                    Sha256::digest(many_short_lines.as_bytes())
                )),
            )
            .expect("multiline source hash");
        let whole = hydrate_admitted_file_source(&controller, &storage, &admission, &file)
            .expect("every line fits the bounded source row");
        assert_eq!((whole.start_line, whole.end_line), (1, 45));
        assert!(whole.source.len() <= INTERIM_SOURCE_ROW_UPPER_BOUND);

        let tiny_but_unrenderable = "\n".repeat(51);
        file.line_count = 51;
        std::fs::write(&path, &tiny_but_unrenderable).expect("raw bytes fit");
        storage
            .update_file_metadata(
                &file,
                Some(&format!(
                    "{:x}",
                    Sha256::digest(tiny_but_unrenderable.as_bytes())
                )),
            )
            .expect("raw source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        ));

        let long = (1..=120)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        file.line_count = 120;
        std::fs::write(&path, &long).expect("long source");
        let long_hash = format!("{:x}", Sha256::digest(long.as_bytes()));
        storage
            .update_file_metadata(&file, Some(&long_hash))
            .expect("long source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        ));

        std::fs::write(&path, "changed source\n").expect("drifted source");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        ));

        let invalid_utf8 = b"\xff\n";
        file.line_count = 1;
        std::fs::write(&path, invalid_utf8).expect("invalid UTF-8 source");
        storage
            .update_file_metadata(&file, Some(&format!("{:x}", Sha256::digest(invalid_utf8))))
            .expect("invalid UTF-8 source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        ));
        storage
            .update_file_metadata(&file, None)
            .expect("missing source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        ));
    }

    #[test]
    fn file_node_navigation_keeps_authenticated_path_without_source_text() {
        let mut support = vec![SupportUnitDto {
            id: "symbol:node:17".into(),
            kind: SupportUnitKindDto::SymbolLocation,
            summary: "Navigation only: no bounded source range for node:17".into(),
            path: None,
            symbol_id: Some("17".into()),
            start_line: None,
            end_line: None,
            snippet: None,
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        }];
        let paths = HashMap::from([("node:17".into(), "src/large.rs".into())]);
        attach_file_navigation_paths(&mut support, &paths);
        assert_eq!(support[0].path.as_deref(), Some("src/large.rs"));
        assert!(support[0].summary.starts_with("Navigation only:"));
        assert!(support[0].snippet.is_none());
    }

    #[test]
    fn packet_file_admissions_distinguish_verified_budget_from_source_drift() {
        let project = tempfile::tempdir().expect("project");
        let controller = AppController::new();
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let mut storage = Store::new_in_memory().expect("store");
        let path = project.path().join("large.rs");
        let source = (1..=120)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(&path, &source).expect("source");
        let mut file = FileInfo {
            id: 17,
            path: path.clone(),
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 120,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).expect("file");
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
            )
            .expect("pinned hash");
        storage
            .insert_nodes_batch(&[CoreNode {
                id: CoreNodeId(17),
                kind: CoreNodeKind::FILE,
                serialized_name: "large.rs".into(),
                file_node_id: Some(CoreNodeId(17)),
                start_line: Some(1),
                ..Default::default()
            }])
            .expect("file node");
        let admissions = ["path:large.rs", "node:17"]
            .into_iter()
            .enumerate()
            .map(|(ordinal, stable_identity)| PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal as u32,
                stable_identity: stable_identity.into(),
                score_version: "test".into(),
                reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect::<Vec<_>>();
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("packet hydration");
        assert_eq!(authenticated.len(), 2);
        assert!(sources.is_empty());
        assert_eq!(paths.get("node:17").map(String::as_str), Some("large.rs"));
        assert_eq!(gaps.len(), 2);
        assert!(
            gaps.iter()
                .all(|gap| matches!(gap.kind, PacketAdmissionGapKindV1::SourceBudgetExceeded))
        );
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "test".into(),
                core_generation_id: "pinned".into(),
                retrieval_generation: None,
            },
            admissions: authenticated.into_iter().map(|item| item.receipt).collect(),
            sources,
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: gaps,
        };
        let mut product = compile_repository_evidence(&input);
        attach_file_navigation_paths(&mut product.support, &paths);
        assert_eq!(product.support.len(), 2);
        assert!(product.support.iter().all(|unit| {
            unit.kind == SupportUnitKindDto::SymbolLocation
                && unit.path.as_deref() == Some("large.rs")
                && unit.snippet.is_none()
                && unit.summary.starts_with("Navigation only:")
        }));
        assert_eq!(product.continuation.len(), 2);
        assert!(
            product.continuation.iter().all(|option| {
                option.reason == PacketStructuralGapReasonV1::SourceBudgetExceeded
            })
        );

        std::fs::write(&path, source.replace("line 1", "xxxx 1")).expect("drift");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("packet drift check");
        assert!(authenticated.is_empty());
        assert!(sources.is_empty());
        assert!(paths.is_empty());
        assert_eq!(gaps.len(), 2);
        assert!(
            gaps.iter()
                .all(|gap| matches!(gap.kind, PacketAdmissionGapKindV1::SourceUnavailable))
        );

        std::fs::write(&path, &source).expect("restore source");
        storage
            .update_file_metadata(&file, None)
            .expect("remove pinned hash");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("missing hash check");
        assert!(authenticated.is_empty() && sources.is_empty() && paths.is_empty());
        assert_eq!(gaps.len(), 2);

        file.complete = false;
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
            )
            .expect("incomplete file metadata");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("incomplete metadata check");
        assert!(authenticated.is_empty() && sources.is_empty() && paths.is_empty());
        assert_eq!(gaps.len(), 2);

        let outside = tempfile::tempdir().expect("outside project");
        file.complete = true;
        file.path = outside.path().join("outside.rs");
        std::fs::write(&file.path, &source).expect("outside source");
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
            )
            .expect("mismatched file path");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("path containment check");
        assert!(authenticated.is_empty() && sources.is_empty() && paths.is_empty());
        assert_eq!(gaps.len(), 2);
    }

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
    fn path_admissions_participate_in_the_induced_relation_graph() {
        let mut storage = Store::new_in_memory().expect("store");
        storage
            .insert_nodes_batch(&[
                CoreNode {
                    id: CoreNodeId(1),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/a.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(2),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/b.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(3),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "crate::run".into(),
                    ..Default::default()
                },
            ])
            .expect("insert file nodes");
        storage
            .insert_edges_batch(&[
                CoreEdge {
                    id: CoreEdgeId(10),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::IMPORT,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
                CoreEdge {
                    id: CoreEdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(3),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
            ])
            .expect("insert import edge");
        let admissions = [
            AuthenticatedPacketAdmissionV1 {
                receipt: PacketAdmissionReceiptV1 {
                    packet_ordinal: 0,
                    stable_identity: "path:src/a.rs".into(),
                    score_version: "test".into(),
                    reserved_source_bytes: 1,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                core_node_id: CoreNodeId(1),
            },
            AuthenticatedPacketAdmissionV1 {
                receipt: PacketAdmissionReceiptV1 {
                    packet_ordinal: 1,
                    stable_identity: "path:src/b.rs".into(),
                    score_version: "test".into(),
                    reserved_source_bytes: 1,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                core_node_id: CoreNodeId(2),
            },
            AuthenticatedPacketAdmissionV1 {
                receipt: PacketAdmissionReceiptV1 {
                    packet_ordinal: 2,
                    stable_identity: "node:3".into(),
                    score_version: "test".into(),
                    reserved_source_bytes: 1,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                core_node_id: CoreNodeId(3),
            },
        ];

        let relations = hydrate_induced_relations(&storage, &admissions).expect("relations");

        assert_eq!(relations.len(), 2);
        assert_eq!(relations[0].from_identity, "path:src/a.rs");
        assert_eq!(relations[0].to_identity, "path:src/b.rs");
        assert_eq!(relations[0].relation_kind, PacketRelationKindV1::Import);
        assert_eq!(relations[1].from_identity, "path:src/a.rs");
        assert_eq!(relations[1].to_identity, "node:3");
        assert_eq!(relations[1].relation_kind, PacketRelationKindV1::Member);
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
