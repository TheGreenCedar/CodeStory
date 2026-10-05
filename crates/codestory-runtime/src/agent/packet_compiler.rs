//! Runtime adapter for the pure repository-derived packet compiler.
//!
//! Runtime owns publication checks and converts pinned repository records into
//! [`PacketCompilationInputV1`]. Selection itself lives in
//! `codestory-agent` and cannot see the question.

use crate::AppController;
use crate::agent::packet_candidate::PacketProofSession;
use crate::agent::packet_coverage::PacketCoverageInput;
use crate::agent::packet_freshness::PacketFreshnessInput;
use codestory_agent::evidence_compiler::{
    RepositoryDerivedCompilationV1, RepresentationBudgetSelectionV1, compile_repository_evidence,
    compile_repository_evidence_for_representation,
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
    PacketRelationKindV1, PacketSourceDeliveryV1, PacketSourceRangeV1, PacketStructuralGapReasonV1,
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

const FILE_NAVIGATION_VERIFICATION_MAX_BYTES: usize = 1024 * 1024;

pub(crate) struct PreparedPacketCompilationV1 {
    input: PacketCompilationInputV1,
    pub(crate) source_coverage: Vec<SourceCoverageObservationDto>,
    publication: PacketCompilationPublicationV1,
    source_refusals: Vec<String>,
    file_navigation_paths: HashMap<String, String>,
}

impl PreparedPacketCompilationV1 {
    #[cfg(feature = "test-support")]
    pub(crate) fn from_fixture_input(input: PacketCompilationInputV1) -> Self {
        Self {
            publication: input.publication.clone(),
            input,
            source_coverage: Vec::new(),
            source_refusals: Vec::new(),
            file_navigation_paths: HashMap::new(),
        }
    }

    pub(crate) fn default_product(&self) -> RepositoryDerivedCompilationV1 {
        let mut product = compile_repository_evidence(&self.input);
        attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
        product
    }

    pub(crate) fn into_frozen(
        self,
        mut product: RepositoryDerivedCompilationV1,
    ) -> FrozenPacketCompilationV1 {
        attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
        FrozenPacketCompilationV1 {
            product,
            source_coverage: self.source_coverage,
            publication: self.publication,
            source_refusals: self.source_refusals,
        }
    }

    pub(crate) fn apply_proposal(
        &self,
        packet: &mut AgentPacketDto,
        request: Option<&AgentPacketRequestDto>,
        product: &RepositoryDerivedCompilationV1,
    ) {
        apply_compilation_state(
            packet,
            request,
            product,
            &self.publication,
            &self.source_refusals,
        );
    }

    pub(crate) fn select_for_representation(
        self,
        mut measure: impl FnMut(
            &Self,
            &RepositoryDerivedCompilationV1,
        ) -> Result<Option<usize>, codestory_contracts::api::ApiError>,
    ) -> Result<FrozenPacketCompilationV1, codestory_contracts::api::ApiError> {
        let selection = compile_repository_evidence_for_representation(&self.input, |product| {
            let mut product = product.clone();
            // File-only navigation paths are public bytes, not post-measurement decoration.
            attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
            measure(&self, &product)
        })?;
        let product = match selection {
            RepresentationBudgetSelectionV1::Complete { product, .. }
            | RepresentationBudgetSelectionV1::BudgetExceeded { product, .. } => product,
        };
        Ok(self.into_frozen(product))
    }
}

pub(crate) struct FrozenPacketCompilationV1 {
    pub(crate) product: RepositoryDerivedCompilationV1,
    pub(crate) source_coverage: Vec<SourceCoverageObservationDto>,
    publication: PacketCompilationPublicationV1,
    source_refusals: Vec<String>,
}

#[derive(Debug, Clone)]
struct AuthenticatedPacketAdmissionV1 {
    receipt: PacketAdmissionReceiptV1,
    core_node_id: CoreNodeId,
}

/// Hydrate exactly the packet-wide admitted identities and compile their
/// repository evidence while the core/retrieval publication is pinned. This
/// runs before any presentation or output-budget mutation.
#[cfg(test)]
pub(crate) fn freeze_packet_compilation(
    controller: &AppController,
    project_id: &str,
    probe_resolutions: &[PacketProbeResolutionDto],
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
    session: &PacketProofSession,
    request: Option<&AgentPacketRequestDto>,
) -> Result<FrozenPacketCompilationV1, codestory_contracts::api::ApiError> {
    let prepared = prepare_packet_compilation(
        controller,
        project_id,
        probe_resolutions,
        publication,
        session,
        request,
    )?;
    let product = prepared.default_product();
    Ok(prepared.into_frozen(product))
}

pub(crate) fn prepare_packet_compilation(
    controller: &AppController,
    project_id: &str,
    probe_resolutions: &[PacketProbeResolutionDto],
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
    session: &PacketProofSession,
    request: Option<&AgentPacketRequestDto>,
) -> Result<PreparedPacketCompilationV1, codestory_contracts::api::ApiError> {
    if let Some(request) = request {
        validate_requested_publication(controller, request, publication)?;
    }
    let admissions = session.receipts();
    let mut admission_gaps = session.gaps();
    let storage = controller.open_storage_read_only()?;
    let mut source_refusals = Vec::new();
    let ranges =
        authenticated_continuation_ranges(controller, project_id, probe_resolutions, publication)?;
    let (authenticated_admissions, mut sources, file_navigation_paths) =
        hydrate_admitted_sources_with_ranges(
            controller,
            &storage,
            &admissions,
            &mut admission_gaps,
            &ranges,
            &mut source_refusals,
        )?;
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
            .unwrap_or_else(|| {
                controller
                    .active_core_publication()
                    .map(|pin| pin.generation_id)
                    .unwrap_or_default()
            }),
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
    Ok(PreparedPacketCompilationV1 {
        input,
        source_coverage,
        publication,
        source_refusals,
        file_navigation_paths,
    })
}

pub(crate) fn validate_requested_publication(
    controller: &AppController,
    request: &AgentPacketRequestDto,
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
) -> Result<(), codestory_contracts::api::ApiError> {
    let actual_core = controller
        .active_core_publication()
        .map(|pin| pin.generation_id);
    if request
        .core_generation_id
        .as_deref()
        .is_some_and(|expected| Some(expected) != actual_core.as_deref())
        || request
            .retrieval_generation
            .as_deref()
            .is_some_and(|expected| {
                publication.is_none_or(|pin| pin.retrieval_generation != expected)
            })
    {
        return Err(codestory_contracts::api::ApiError::invalid_argument(
            "pinned continuation publication changed before source hydration",
        ));
    }
    Ok(())
}

fn authenticated_continuation_ranges(
    controller: &AppController,
    project_id: &str,
    resolutions: &[PacketProbeResolutionDto],
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
) -> Result<HashMap<String, PacketSourceRangeV1>, codestory_contracts::api::ApiError> {
    let mut ranges = HashMap::new();
    for resolution in resolutions {
        let codestory_contracts::api::PacketProbeDto::Continuation {
            contract_version,
            project_id: expected_project,
            core_generation_id,
            retrieval_generation,
            selector,
        } = &resolution.probe
        else {
            continue;
        };
        let Some(range) = selector.source_range else {
            continue;
        };
        let core = controller
            .active_core_publication()
            .map(|pin| pin.generation_id);
        if *contract_version != codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION
            || expected_project != project_id
            || core.as_deref() != Some(core_generation_id.as_str())
            || retrieval_generation.as_deref()
                != publication.map(|pin| pin.retrieval_generation.as_str())
            || resolution.status != PacketProbeResolutionStatusDto::Continuation
            || !range.is_valid()
            || resolution
                .symbol_id
                .as_ref()
                .is_none_or(|id| selector.stable_identity != format!("node:{id}"))
        {
            return Err(codestory_contracts::api::ApiError::invalid_argument(
                "source continuation is not authenticated against the pinned publication and node",
            ));
        }
        if ranges
            .insert(selector.stable_identity.clone(), range)
            .is_some_and(|previous| previous != range)
        {
            return Err(codestory_contracts::api::ApiError::invalid_argument(
                "conflicting source ranges for one admitted identity",
            ));
        }
    }
    Ok(ranges)
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
    apply_compilation_state(
        packet,
        request,
        &frozen.product,
        &frozen.publication,
        &frozen.source_refusals,
    );
    // Rejected previews never enter final-support telemetry.
    crate::agent::packet_batch::observe_packet_raf_final_support(
        &final_support_identities_for_observation(&packet.support),
    );
}

/// Commit a proposal whose shared compilation state has already been applied,
/// presentation-bounded and validated by the public record builder. Do not
/// reclassify or mutate the exact measured packet at the freeze boundary.
pub(crate) fn commit_measured_packet_compilation(
    packet: &AgentPacketDto,
    frozen: FrozenPacketCompilationV1,
) -> Result<(), codestory_contracts::api::ApiError> {
    if packet.support != frozen.product.support
        || packet
            .answer
            .source_coverage
            .iter()
            .any(|observation| !frozen.source_coverage.contains(observation))
    {
        return Err(codestory_contracts::api::ApiError::internal(
            "Frozen compilation differs from measured source delivery.",
        ));
    }
    crate::agent::packet_batch::observe_packet_raf_final_support(
        &final_support_identities_for_observation(&packet.support),
    );
    Ok(())
}

fn apply_compilation_state(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    product: &RepositoryDerivedCompilationV1,
    publication: &PacketCompilationPublicationV1,
    source_refusals: &[String],
) {
    packet.support = product.support.clone();
    packet.disposition = classify_packet_disposition(
        packet,
        request,
        &product.continuation,
        publication.core_generation_id.clone(),
        publication.retrieval_generation.clone(),
    );
    if !source_refusals.is_empty()
        && packet.disposition.kind == codestory_contracts::api::PacketDispositionKindDto::Supported
    {
        packet.disposition =
            PacketDispositionDto::not_established("an admitted source could not be delivered");
    }
    packet
        .disposition
        .omission_receipts
        .extend(source_refusals.iter().cloned());
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

type HydratedAdmittedSources = (
    Vec<AuthenticatedPacketAdmissionV1>,
    Vec<PacketHydratedSourceRangeV1>,
    HashMap<String, String>,
);

#[cfg(test)]
fn hydrate_admitted_sources(
    controller: &AppController,
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
) -> Result<HydratedAdmittedSources, codestory_contracts::api::ApiError> {
    hydrate_admitted_sources_with_ranges(
        controller,
        storage,
        admissions,
        admission_gaps,
        &HashMap::new(),
        &mut Vec::new(),
    )
}

fn hydrate_admitted_sources_with_ranges(
    controller: &AppController,
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
    ranges: &HashMap<String, PacketSourceRangeV1>,
    source_refusals: &mut Vec<String>,
) -> Result<HydratedAdmittedSources, codestory_contracts::api::ApiError> {
    let project_root = controller.require_project_root()?;
    let mut authenticated = Vec::new();
    let mut sources = Vec::new();
    let mut file_navigation_paths = HashMap::new();
    for admission in admissions {
        let previous_refusal_count = source_refusals.len();
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
                    hydrate_admitted_node_source_with_range(
                        controller,
                        storage,
                        admission,
                        &node,
                        ranges.get(&admission.stable_identity).copied(),
                        source_refusals,
                    ),
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
                if admission.reserved_source_bytes as usize >= INTERIM_SOURCE_ROW_UPPER_BOUND
                    && source_refusals.len() == previous_refusal_count
                {
                    source_refusals.push(format!(
                        "Admitted source {} did not fit the {}-byte source-row cap; no source range was delivered under the pinned publication.",
                        admission.packet_ordinal, INTERIM_SOURCE_ROW_UPPER_BOUND,
                    ));
                }
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
                let missing_bounds = kind == PacketAdmissionGapKindV1::SourceBoundMissing;
                source_refusals.push(format!(
                    "Admitted source {}: {}; no source range was delivered under the pinned publication.",
                    admission.packet_ordinal,
                    if missing_bounds { "source bounds are unavailable" } else { "source bytes are unavailable" },
                ));
                // Descriptor allocation bounds can be missing before admission.
                // Only a failed authenticated hydration becomes terminal here.
                let kind = if missing_bounds {
                    PacketAdmissionGapKindV1::SourceUnavailable
                } else {
                    kind
                };
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

fn hydrate_admitted_node_source_with_range(
    controller: &AppController,
    storage: &Store,
    admission: &PacketAdmissionReceiptV1,
    node: &CoreNode,
    requested: Option<PacketSourceRangeV1>,
    source_refusals: &mut Vec<String>,
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
        if requested.is_some() {
            return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
        }
        return hydrate_admitted_file_source(controller, storage, admission, &file);
    }
    let (start_line, end_line) = valid_source_bounds(node.start_line, node.end_line)
        .ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?;
    let requested = requested.unwrap_or(PacketSourceRangeV1 {
        start_line,
        end_line,
    });
    if !requested.is_valid() || requested.start_line < start_line || requested.end_line > end_line {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    let project_root = controller
        .require_project_root()
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    let source = crate::search_evidence::verified_file_checked(storage, Some(&project_root), &file)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    if source.content.lines().count() >= requested.end_line as usize {
        let raw = source
            .content
            .split_inclusive('\n')
            .nth(requested.start_line as usize - 1)
            .ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?;
        let line = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(raw);
        let required = format!(">{:>5} | ", requested.start_line).len() + line.len() + 11;
        if required > INTERIM_SOURCE_ROW_UPPER_BOUND {
            let remaining = source
                .content
                .split_inclusive('\n')
                .skip(requested.start_line as usize - 1)
                .take((requested.end_line - requested.start_line + 1) as usize)
                .map(str::len)
                .sum::<usize>();
            source_refusals.push(format!(
                "{}: requested lines {}-{}; zero complete lines delivered; {} source bytes remain; first complete line requires {} rendered bytes, exceeding the 512-byte row cap; indexed SHA-256 {}.",
                admission.stable_identity, requested.start_line, requested.end_line,
                remaining, required, source.content_sha256,
            ));
            return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
        }
    }
    let (markdown, delivered_end, delivery) = complete_node_page(
        &source.content,
        requested,
        source_byte_cap(admission),
        &source.content_sha256,
    )?;
    Ok(PacketHydratedSourceRangeV1 {
        stable_identity: admission.stable_identity.clone(),
        path: authenticated_file_navigation_path(controller, &file)
            .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?,
        symbol: Some(
            node.qualified_name
                .clone()
                .unwrap_or_else(|| node.serialized_name.clone()),
        ),
        start_line: requested.start_line,
        end_line: delivered_end,
        source: markdown,
        parser_completeness: PacketParserCompletenessV1::Unknown,
        delivery: Some(delivery),
    })
}

/// Packet pages never clip a numbered physical line or infer bounds from prose.
fn complete_node_page(
    text: &str,
    requested: PacketSourceRangeV1,
    cap: usize,
    source_sha256: &str,
) -> Result<(String, u32, PacketSourceDeliveryV1), PacketAdmissionGapKindV1> {
    if !requested.is_valid() || text.lines().count() < requested.end_line as usize {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    let mut markdown = String::from("```text\n");
    let mut end = None;
    let mut remaining = 0_u64;
    let mut next_line_bytes = None;
    for (index, raw) in text
        .split_inclusive('\n')
        .enumerate()
        .skip(requested.start_line as usize - 1)
        .take((requested.end_line - requested.start_line + 1) as usize)
    {
        let line_number = index as u32 + 1;
        let line = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(raw);
        let prefix = format!(
            "{}{:>5} | ",
            if end.is_none() { ">" } else { " " },
            line_number
        );
        let rendered_bytes = prefix.len().saturating_add(line.len()).saturating_add(1);
        if next_line_bytes.is_none()
            && markdown
                .len()
                .saturating_add(rendered_bytes)
                .saturating_add(3)
                <= cap
        {
            markdown.push_str(&prefix);
            markdown.push_str(line);
            markdown.push('\n');
            end = Some(line_number);
        } else {
            remaining = remaining.saturating_add(raw.len() as u64);
            next_line_bytes.get_or_insert(rendered_bytes.saturating_add(10) as u64);
        }
    }
    let end = end.ok_or(PacketAdmissionGapKindV1::SourceBudgetExceeded)?;
    markdown.push_str("```");
    let blocked = next_line_bytes.filter(|bytes| *bytes > INTERIM_SOURCE_ROW_UPPER_BOUND as u64);
    let next_start_line = (remaining > 0 && blocked.is_none()).then(|| end + 1);
    let delivery = PacketSourceDeliveryV1 {
        requested_range: requested,
        source_sha256: source_sha256.to_owned(),
        remaining_source_bytes: remaining,
        next_start_line,
        blocked_line_rendered_bytes: blocked,
    };
    if !delivery.matches_delivery(requested.start_line, end) {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    Ok((markdown, end, delivery))
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
    let canonical_path = codestory_workspace::workspace_relative_path(&project_root, &resolved)
        .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?
        .to_string_lossy()
        .replace('\\', "/");
    hydrated_source(admission, &canonical_path, None, &markdown)
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
        path: path.to_string(),
        symbol,
        start_line,
        end_line,
        source: source.to_string(),
        delivery: None,
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

pub(crate) fn observe_admitted_source_coverage(
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
    let mut observations = Vec::new();
    for path in paths {
        let canonical = match codestory_workspace::resolve_project_relative_path(
            &project_root,
            Path::new(path),
        ) {
            Ok(
                codestory_workspace::ProjectRelativePathResolution::Existing { relative, .. }
                | codestory_workspace::ProjectRelativePathResolution::Missing { relative, .. },
            ) => Some(relative.to_string_lossy().replace('\\', "/")),
            _ => None,
        };
        let identity = canonical.as_deref().unwrap_or(path);
        if !seen.insert(identity.to_string()) {
            continue;
        }
        observations.push(if canonical.is_some() {
            observe_one_admitted_source_coverage(storage, &project_root, identity)
                .unwrap_or_else(|_| source_coverage_not_established(identity))
        } else {
            source_coverage_not_established(path)
        });
    }
    observations
}

fn observe_one_admitted_source_coverage(
    storage: &Store,
    project_root: &Path,
    path: &str,
) -> Result<SourceCoverageObservationDto, codestory_store::StorageError> {
    // Policy exclusions carry no `file` row (excluded sources are never
    // registered as parser-backed coverage), so the exclusion table is
    // consulted on the normalized relative spelling before the file lookup.
    for candidate in admitted_file_lookup_paths(project_root, path) {
        let relative_path = codestory_workspace::workspace_relative_path(project_root, &candidate)
            .unwrap_or_else(|| candidate.clone())
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
    }

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
        .find(|observation| observation.path == path)
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
        request.parent_packet_id.is_some()
            || !request.option_ids.is_empty()
            || request.probes.iter().any(|probe| {
                matches!(
                    probe,
                    codestory_contracts::api::PacketProbeDto::Continuation { .. }
                )
            })
    });
    if !already_drilled {
        let mut seen_options = BTreeSet::new();
        let options = continuation
            .iter()
            .filter_map(drill_option_from_selector)
            .filter(|option| seen_options.insert(option.id.clone()))
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
    } else if (already_drilled && !continuation.is_empty())
        || packet.support.iter().any(|unit| {
            unit.source_delivery
                .as_ref()
                .is_some_and(|delivery| delivery.remaining_source_bytes > 0)
        })
    {
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
        .filter_map(|unit| unit.path.as_deref())
        .collect::<BTreeSet<_>>();
    let blocking = observations
        .iter()
        .filter(|observation| {
            observation.status != SourceCoverageStatusDto::Incomplete
                || observation.reason != Some(FileCoverageReason::ParserPartial)
                || !verified_source_paths.contains(observation.path.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    PacketCoverageInput::from_observations(&blocking)
}

fn drill_option_from_selector(selector: &PacketContinuationSelectorV1) -> Option<DrillOptionDto> {
    let mut gap_id = format!(
        "{}:{}",
        structural_reason_label(selector.reason),
        selector.stable_identity
    );
    if let Some(range) = selector.source_range {
        gap_id.push_str(&format!(":lines:{}:{}", range.start_line, range.end_line));
    }
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
    if let Some(range) = selector.source_range {
        if !range.is_valid() {
            return None;
        }
        option = option.with_source_range(range);
    }
    if option.id.len() > codestory_contracts::packet_projection_v3::IDENTITY_MAX_BYTES_V3 {
        return None;
    }
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

pub(crate) fn continuation_probes_from_options(
    req: &AgentPacketRequestDto,
    project_id: &str,
) -> Result<Vec<codestory_contracts::api::PacketProbeDto>, codestory_contracts::api::ApiError> {
    use codestory_contracts::api::{ApiError, PacketProbeDto};
    let decoded_options = drill_options_from_ids(&req.option_ids);
    if decoded_options.len() != req.option_ids.len() {
        return Err(ApiError::invalid_argument(
            "packet contains a malformed continuation option ID",
        ));
    }
    let mut probes = Vec::new();
    for option in decoded_options {
        if let Some(range) = option.source_range {
            let symbol_id = option.symbol_id.ok_or_else(|| {
                ApiError::invalid_argument("source range continuation requires a node identity")
            })?;
            let core_generation_id = req.core_generation_id.clone().ok_or_else(|| {
                ApiError::invalid_argument(
                    "source range continuation requires a pinned core generation",
                )
            })?;
            probes.push(PacketProbeDto::Continuation {
                contract_version: codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION,
                project_id: project_id.to_owned(),
                core_generation_id,
                retrieval_generation: req.retrieval_generation.clone(),
                selector: codestory_contracts::compilation::PacketContinuationSelectorV1 {
                    stable_identity: format!("node:{symbol_id}"),
                    path: None,
                    symbol_id: Some(symbol_id),
                    reason: codestory_contracts::compilation::PacketStructuralGapReasonV1::SourceBudgetExceeded,
                    source_range: Some(range),
                },
            });
        } else if let Some(path) = option.path {
            probes.push(PacketProbeDto::ExactPath { path });
        } else if let Some(symbol_id) = option.symbol_id {
            probes.push(PacketProbeDto::SymbolId { id: symbol_id });
        }
    }
    Ok(probes)
}

/// Decode only stable path or symbol continuations. Historical query-text
/// options are deliberately not reintroduced as retrieval policy.
pub fn drill_options_from_ids(option_ids: &[String]) -> Vec<DrillOptionDto> {
    option_ids
        .iter()
        .filter_map(|id| {
            let (kind, target) = decode_drill_option_id(id)?;
            let (target, range) = if kind == DrillGapKindDto::OmittedMandatorySupport {
                if let Some(encoded) = target.strip_prefix("range:") {
                    let mut parts = encoded.splitn(3, ':');
                    let start = parts.next()?;
                    let end = parts.next()?;
                    let range = PacketSourceRangeV1 {
                        start_line: start.parse().ok()?,
                        end_line: end.parse().ok()?,
                    };
                    if !range.is_valid()
                        || range.start_line.to_string() != start
                        || range.end_line.to_string() != end
                    {
                        return None;
                    }
                    (parts.next()?.to_owned(), Some(range))
                } else {
                    (target, None)
                }
            } else {
                (target, None)
            };
            let option = match kind {
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
            }?;
            let option = if let Some(range) = range {
                option.with_source_range(range)
            } else {
                option
            };
            // Reject noncanonical escapes and malformed range IDs rather than
            // quietly turning them into a different executable action.
            (option.id == *id).then_some(option)
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
    fn complete_source_pages_preserve_physical_lines_and_terminal_bounds() {
        let hash = "a".repeat(64);
        for text in ["éé\n\nlast", "éé\r\n\r\nlast", "a\n", "a", "a\r"] {
            let requested = PacketSourceRangeV1 {
                start_line: 1,
                end_line: text.lines().count() as u32,
            };
            let (full, end, delivery) = complete_node_page(text, requested, 512, &hash).unwrap();
            assert_eq!(end, requested.end_line);
            assert_eq!(delivery.remaining_source_bytes, 0);
            assert_eq!(delivery.next_start_line, None);
            assert_eq!(delivery.blocked_line_rendered_bytes, None);
            assert!(full.ends_with("\n```"));
            let (exact, exact_end, _) =
                complete_node_page(text, requested, full.len(), &hash).unwrap();
            assert_eq!((exact, exact_end), (full.clone(), end));
            if requested.end_line == 1 {
                assert_eq!(
                    complete_node_page(text, requested, full.len() - 1, &hash),
                    Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
                );
            } else {
                let (short, delivered, receipt) =
                    complete_node_page(text, requested, full.len() - 1, &hash).unwrap();
                assert!(short.ends_with("\n```"));
                assert_eq!(delivered, end - 1);
                assert_eq!(receipt.next_start_line, Some(end));
                assert_eq!(
                    receipt.remaining_source_bytes,
                    text.split_inclusive('\n').last().unwrap().len() as u64
                );
            }
        }
        let text = format!("first\r\n{}\r\nlast", "é".repeat(300));
        let (page, end, receipt) = complete_node_page(
            &text,
            PacketSourceRangeV1 {
                start_line: 1,
                end_line: 3,
            },
            512,
            &hash,
        )
        .unwrap();
        assert_eq!(end, 1);
        assert!(page.contains("first") && !page.contains('é'));
        assert_eq!(receipt.remaining_source_bytes, 600 + 2 + 4);
        assert_eq!(receipt.next_start_line, None);
        assert!(receipt.blocked_line_rendered_bytes.unwrap() > 512);
        assert_eq!(
            complete_node_page(
                &text,
                PacketSourceRangeV1 {
                    start_line: 2,
                    end_line: 3
                },
                512,
                &hash
            ),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        );
        assert_eq!(
            complete_node_page(
                "one\n",
                PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 2
                },
                512,
                &hash
            ),
            Err(PacketAdmissionGapKindV1::SourceBoundMissing)
        );
        assert_eq!(
            complete_node_page(
                "one",
                PacketSourceRangeV1 {
                    start_line: 0,
                    end_line: 1
                },
                512,
                &hash
            ),
            Err(PacketAdmissionGapKindV1::SourceBoundMissing)
        );
    }

    #[test]
    fn source_range_option_ids_round_trip_and_reject_malformed_cursors() {
        let range = PacketSourceRangeV1 {
            start_line: 12,
            end_line: 80,
        };
        let original = DrillOptionDto::omitted_symbol("gap", "-42").with_source_range(range);
        let decoded = drill_options_from_ids(&[original.id.clone()]);
        assert_eq!(decoded[0].id, original.id);
        assert_eq!(decoded[0].symbol_id.as_deref(), Some("-42"));
        assert_eq!(decoded[0].source_range, Some(range));
        for target in [
            "range:0:9:symbol:1",
            "range:10:9:symbol:1",
            "range:01:9:symbol:1",
            "range:1:999999999999:symbol:1",
            "range:1:9:query:guess",
        ] {
            let id = codestory_contracts::api::encode_drill_option_id(
                DrillGapKindDto::OmittedMandatorySupport,
                target,
            );
            assert!(drill_options_from_ids(&[id]).is_empty(), "{target}");
        }
        assert!(
            drill_options_from_ids(
                &["omitted_mandatory_support:range%3a1%3A9%3Asymbol%3A1".into()]
            )
            .is_empty()
        );
    }

    #[test]
    fn canonical_coverage_keeps_nested_files_and_parser_status_distinct() {
        let process_cache = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        // The host path contains a source marker before the actual project root.
        let root = parent.path().join("src/checkout");
        std::fs::create_dir_all(&root).unwrap();
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(root.clone());
        let storage = Store::new_in_memory().unwrap();
        for (id, relative, complete) in [
            (1, "src/main.rs", true),
            (2, "tools/repos/worker/src/main.rs", false),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = format!("fn item_{id}() {{}}\n");
            std::fs::write(&path, &text).unwrap();
            let file = FileInfo {
                id,
                path,
                language: "rust".into(),
                modification_time: 0,
                indexed: true,
                complete,
                line_count: 1,
                file_role: FileRole::Source,
            };
            storage.insert_file(&file).unwrap();
            storage
                .update_file_metadata(
                    &file,
                    Some(&format!("{:x}", Sha256::digest(text.as_bytes()))),
                )
                .unwrap();
        }
        let observations = observe_admitted_source_coverage(
            &controller,
            &storage,
            &[
                root.join("src/main.rs").to_string_lossy().into_owned(),
                "./src/main.rs".into(),
                root.join("tools/repos/worker/src/main.rs")
                    .to_string_lossy()
                    .into_owned(),
                "tools/repos/worker/src/main.rs".into(),
            ],
        );
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].path, "src/main.rs");
        assert_eq!(observations[1].path, "tools/repos/worker/src/main.rs");
        assert_eq!(
            parser_completeness_for_path("src/main.rs", &observations),
            PacketParserCompletenessV1::Complete
        );
        assert_eq!(
            parser_completeness_for_path("tools/repos/worker/src/main.rs", &observations),
            PacketParserCompletenessV1::Partial
        );
        assert_eq!(
            parser_completeness_for_path("repos/other/src/main.rs", &observations),
            PacketParserCompletenessV1::Unknown
        );
        let outside = parent.path().join("outside.rs");
        std::fs::write(&outside, "fn outside() {}\n").unwrap();
        let unavailable = observe_admitted_source_coverage(
            &controller,
            &storage,
            &[outside.to_string_lossy().into_owned()],
        );
        assert_eq!(
            unavailable[0].status,
            SourceCoverageStatusDto::NotEstablished
        );
    }

    #[test]
    fn parser_partial_source_exception_never_crosses_nested_file_identity() {
        let observation = SourceCoverageObservationDto {
            path: "tools/repos/worker/src/main.rs".into(),
            status: SourceCoverageStatusDto::Incomplete,
            reason: Some(FileCoverageReason::ParserPartial),
            not_established_cause: None,
            observed_size: None,
            byte_cap: None,
        };
        let mut source = SupportUnitDto {
            source_delivery: None,
            id: "source:1".into(),
            kind: SupportUnitKindDto::SourceRange,
            summary: "bounded source".into(),
            path: Some("src/main.rs".into()),
            symbol_id: Some("1".into()),
            start_line: Some(1),
            end_line: Some(1),
            snippet: Some("fn root() {}".into()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        };
        assert!(
            packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                std::slice::from_ref(&source)
            )
            .blocks_packet_availability()
        );
        source.path = Some(observation.path.clone());
        assert!(
            !packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                std::slice::from_ref(&source)
            )
            .blocks_packet_availability()
        );
        source.snippet = None;
        assert!(
            packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                std::slice::from_ref(&source)
            )
            .blocks_packet_availability()
        );
    }

    #[test]
    fn registered_nested_source_refuses_missing_stale_and_directory_replacements() {
        let cache = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(cache.path());
        controller.state.lock().project_root = Some(project.path().to_owned());
        let mut storage = Store::new_in_memory().unwrap();
        let path = project.path().join("tools/repos/worker/src/main.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = "fn action() {}\n";
        std::fs::write(&path, text).unwrap();
        let file = FileInfo {
            id: 1,
            path: path.clone(),
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).unwrap();
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(text.as_bytes()))),
            )
            .unwrap();
        storage
            .insert_nodes_batch(&[
                CoreNode {
                    id: CoreNodeId(1),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "main.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(2),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "action".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..Default::default()
                },
            ])
            .unwrap();
        let admissions = ["node:2", "path:tools/repos/worker/src/main.rs"]
            .into_iter()
            .enumerate()
            .map(|(ordinal, identity)| PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal as u32,
                stable_identity: identity.into(),
                score_version: "test".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::ExactTypedSelector,
            })
            .collect::<Vec<_>>();
        let mut gaps = Vec::new();
        let (authenticated, sources, _) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps).unwrap();
        assert_eq!(authenticated.len(), 2);
        assert_eq!(sources.len(), 2);
        assert!(gaps.is_empty());
        assert!(
            sources
                .iter()
                .all(|source| source.path == "tools/repos/worker/src/main.rs")
        );
        let refuse = |label: &str| {
            let mut gaps = Vec::new();
            let (authenticated, sources, navigation) =
                hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps).unwrap();
            assert!(
                authenticated.is_empty() && sources.is_empty() && navigation.is_empty(),
                "{label}"
            );
            assert_eq!(gaps.len(), 2, "{label}");
            assert!(
                gaps.iter()
                    .all(|gap| gap.kind == PacketAdmissionGapKindV1::SourceUnavailable),
                "{label}"
            );
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project".into(),
                    core_generation_id: "generation".into(),
                    retrieval_generation: None,
                },
                admissions: vec![],
                sources,
                relations: vec![],
                ambiguities: vec![],
                admission_gaps: gaps,
            };
            assert!(
                compile_repository_evidence(&input).support.is_empty(),
                "{label}"
            );
        };
        std::fs::remove_file(&path).unwrap();
        refuse("deleted registered file");
        std::fs::write(&path, "fn changed() {}\n").unwrap();
        refuse("stale registered hash");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        refuse("registered file replaced by directory");
        std::fs::remove_dir(&path).unwrap();
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            let target = outside.path().join("main.rs");
            std::fs::write(&target, text).unwrap();
            // Identical pinned bytes do not authorize an outside target.
            std::os::unix::fs::symlink(&target, &path).unwrap();
            refuse("escaping symlink with unchanged source bytes");
        }
    }

    #[test]
    fn authenticated_node_pages_advance_and_refuse_changed_or_out_of_bound_source() {
        let process_cache = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let storage = Store::new_in_memory().unwrap();
        let text = (1..=30)
            .map(|line| format!("let stage_{line:02} = \"{}\";\r\n", "x".repeat(40)))
            .collect::<String>();
        let path = project.path().join("stages.rs");
        std::fs::write(&path, &text).unwrap();
        let file = FileInfo {
            id: 1,
            path,
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 30,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).unwrap();
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        storage.update_file_metadata(&file, Some(&hash)).unwrap();
        let node = CoreNode {
            id: CoreNodeId(2),
            kind: CoreNodeKind::FUNCTION,
            serialized_name: "stages".into(),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(1),
            end_line: Some(30),
            ..Default::default()
        };
        let admission = PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: "node:2".into(),
            score_version: "test".into(),
            reserved_source_bytes: 512,
            origin: PacketAdmissionOriginV1::ExactTypedSelector,
        };
        let first = hydrate_admitted_node_source_with_range(
            &controller,
            &storage,
            &admission,
            &node,
            None,
            &mut Vec::new(),
        )
        .unwrap();
        let input = PacketCompilationInputV1 {
            contract_version: 1,
            publication: PacketCompilationPublicationV1 {
                project_id: "p".into(),
                core_generation_id: "g".into(),
                retrieval_generation: None,
            },
            admissions: vec![admission.clone()],
            sources: vec![first.clone()],
            relations: vec![],
            ambiguities: vec![],
            admission_gaps: vec![],
        };
        let product = compile_repository_evidence(&input);
        let selector = product
            .continuation
            .iter()
            .find(|selector| selector.source_range.is_some())
            .unwrap();
        let option = drill_option_from_selector(selector).unwrap();
        let decoded = drill_options_from_ids(&[option.id]);
        let second = hydrate_admitted_node_source_with_range(
            &controller,
            &storage,
            &admission,
            &node,
            decoded[0].source_range,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(second.start_line, first.end_line + 1);
        assert!(second.end_line > first.end_line);
        assert_eq!(second.delivery.as_ref().unwrap().source_sha256, hash);
        assert!(
            second.delivery.as_ref().unwrap().remaining_source_bytes
                < first.delivery.as_ref().unwrap().remaining_source_bytes
        );
        assert!(!second.source.contains("stage_01"));
        for range in [
            PacketSourceRangeV1 {
                start_line: 0,
                end_line: 30,
            },
            PacketSourceRangeV1 {
                start_line: 10,
                end_line: 31,
            },
        ] {
            assert_eq!(
                hydrate_admitted_node_source_with_range(
                    &controller,
                    &storage,
                    &admission,
                    &node,
                    Some(range),
                    &mut Vec::new()
                ),
                Err(PacketAdmissionGapKindV1::SourceBoundMissing)
            );
        }
        std::fs::write(&file.path, text.replace("stage_01", "changed!")).unwrap();
        assert_eq!(
            hydrate_admitted_node_source_with_range(
                &controller,
                &storage,
                &admission,
                &node,
                decoded[0].source_range,
                &mut Vec::new()
            ),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        );
    }

    #[test]
    fn emitted_range_reaches_pinned_freeze_and_public_projection() {
        for first_overlong in [false, true] {
            use codestory_contracts::api::{PacketDispositionKindDto, PacketProbeDto};
            use codestory_contracts::packet_projection_v3::{GapKindV3Dto, PacketProjectionV3Dto};
            use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
            use std::sync::{Arc, atomic::AtomicBool};
            let project = tempfile::tempdir().unwrap();
            let process_cache = tempfile::tempdir().unwrap();
            let database = process_cache.path().join("core.sqlite3");
            let mut storage = Store::open(&database).unwrap();
            let path = project.path().join("stages.rs");
            let text = (1..=30)
                .map(|line| format!("let stage_{line:02} = \"{}\";\r\n", "x".repeat(40)))
                .collect::<String>();
            let text = if first_overlong {
                format!("{}\n{}", "é".repeat(300), "\n".repeat(29))
            } else {
                text
            };
            std::fs::write(&path, &text).unwrap();
            let file = FileInfo {
                id: 1,
                path,
                language: "rust".into(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 30,
                file_role: FileRole::Source,
            };
            storage.insert_file(&file).unwrap();
            let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
            storage.update_file_metadata(&file, Some(&hash)).unwrap();
            let unavailable_path = project.path().join("unavailable.h");
            let unavailable_text = "#define FLAG 1\n";
            std::fs::write(&unavailable_path, unavailable_text).unwrap();
            let unavailable_file = FileInfo {
                id: 3,
                path: unavailable_path,
                language: "c".into(),
                modification_time: 0,
                indexed: true,
                complete: false,
                line_count: 1,
                file_role: FileRole::Source,
            };
            storage.insert_file(&unavailable_file).unwrap();
            storage
                .update_file_metadata(
                    &unavailable_file,
                    Some(&format!(
                        "{:x}",
                        Sha256::digest(unavailable_text.as_bytes())
                    )),
                )
                .unwrap();
            storage
                .insert_nodes_batch(&[
                    CoreNode {
                        id: CoreNodeId(1),
                        kind: CoreNodeKind::FILE,
                        serialized_name: "stages.rs".into(),
                        ..Default::default()
                    },
                    CoreNode {
                        id: CoreNodeId(2),
                        kind: CoreNodeKind::FUNCTION,
                        serialized_name: "stages".into(),
                        file_node_id: Some(CoreNodeId(1)),
                        start_line: Some(1),
                        end_line: Some(30),
                        ..Default::default()
                    },
                    CoreNode {
                        id: CoreNodeId(3),
                        kind: CoreNodeKind::FILE,
                        serialized_name: "unavailable.h".into(),
                        ..Default::default()
                    },
                    CoreNode {
                        id: CoreNodeId(4),
                        kind: CoreNodeKind::FUNCTION,
                        serialized_name: "boundless".into(),
                        file_node_id: Some(CoreNodeId(1)),
                        ..Default::default()
                    },
                ])
                .unwrap();
            let publication = IndexPublicationRecord {
                generation: 1,
                generation_id: "11111111-1111-4111-8111-111111111111".into(),
                run_id: "source-page-run".into(),
                mode: IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            };
            // Publish a genuine tiny lexical identity generation inside this test's tempdir.
            let search_path = crate::search_publication::search_index_path_for_publication(
                &database,
                Some(&publication),
            )
            .unwrap();
            let mut engine = crate::search::engine::SearchEngine::new(Some(&search_path)).unwrap();
            let mut symbol_session = engine.begin_symbol_index().unwrap();
            let (_, stats) = crate::search_publication::load_canonical_search_symbols(
                &storage,
                16,
                None,
                |batch| {
                    symbol_session
                        .add_nodes(
                            batch
                                .into_iter()
                                .map(|entry| (entry.node_id, entry.display_name)),
                        )
                        .map(|_| ())
                        .map_err(|error| {
                            codestory_contracts::api::ApiError::internal(error.to_string())
                        })
                },
            )
            .unwrap();
            symbol_session.finish().unwrap();
            crate::search_publication::write_search_generation_completion(
                &search_path,
                &publication,
                stats.search_symbol_stream_rows as usize,
                engine.tantivy_doc_count(),
            )
            .unwrap();
            drop(engine);
            storage.put_index_publication(&publication).unwrap();
            drop(storage);
            let controller = AppController::new_with_owned_cache_root(process_cache.path());
            {
                let mut state = controller.state.lock();
                state.project_root = Some(project.path().to_owned());
                state.storage_path = Some(database);
            }
            let service = crate::services::PublicOperationService::new(controller.clone());
            let project_id = codestory_workspace::project_identity_v3(project.path()).project_id;
            service.run_observational_with_cancel("source-pages", Arc::new(AtomicBool::new(false)), || {
            let initial_request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({"question":"show repository source"})).unwrap();
            let session = PacketProofSession::new();
            session.admit("node:2", 512);
            session.admit("path:unavailable.h", 512);
            session.admit("node:3", 512);
            session.admit("node:4", 512);
            session.admit("path:stages.rs", 512);
            let first = freeze_packet_compilation(&controller, &project_id, &[], None, &session, Some(&initial_request))?;
            if first_overlong {
                assert!(first.product.support.iter().all(|unit| unit.kind != SupportUnitKindDto::SourceRange));
                assert!(first.product.continuation.is_empty());
                assert_eq!(first.source_refusals.len(), 5);
                assert!(first.source_refusals[0].contains("zero complete lines delivered"));
                assert!(first.source_refusals[0].contains("630 source bytes remain"));
                assert!(first.source_refusals[0].contains(&hash));
                let mut packet = source_page_test_packet();
                apply_frozen_packet_compilation(&mut packet, Some(&initial_request), first);
                assert_eq!(packet.disposition.kind, PacketDispositionKindDto::NotEstablished);
                let public = crate::evidence_projection_v3::project_packet_v3(&service, "refusal-test-caller", &initial_request, &packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
                let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("small refusal fits") };
                assert!(continuation.is_none());
                assert!(evidence.as_slice().iter().all(|row| row.kind != codestory_contracts::packet_projection_v3::EvidenceKindV3Dto::ExactSource));
                assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("630 source bytes remain"))));
                return Ok(());
            }
            let selector = first.product.continuation.iter().find(|selector| selector.source_range.is_some()).unwrap();
            let first_end = first.product.support.iter().find(|unit| unit.source_delivery.is_some()).unwrap().end_line.unwrap();
            let first_unread = first.product.support.iter().find(|unit| unit.source_delivery.is_some()).unwrap().source_delivery.as_ref().unwrap().remaining_source_bytes;
            let option = drill_option_from_selector(selector).unwrap();
            assert_eq!(first.product.continuation.len(), 1, "only the verified cursor can progress");
            assert_eq!(first.source_refusals.len(), 4);
            assert!(first.source_refusals.iter().any(|receipt| receipt.contains("source bounds")));
            assert!(first.product.support.iter().all(|unit| unit.symbol_id.as_deref() == Some("2") || (unit.kind == SupportUnitKindDto::SymbolLocation && unit.path.as_deref() == Some("stages.rs") && unit.snippet.is_none())), "only verified file navigation and the bounded function survive");
            let mut initial_packet = source_page_test_packet();
            initial_packet.answer.source_coverage = first.source_coverage.clone();
            apply_frozen_packet_compilation(&mut initial_packet, Some(&initial_request), first);
            assert_eq!(initial_packet.disposition.kind, PacketDispositionKindDto::DrillOnce);
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "mixed-refusal-caller", &initial_request, &initial_packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            let PacketProjectionV3Dto::Complete { gaps, continuation, .. } = public.projection else { panic!("bounded mixed refusal fits") };
            assert_eq!(gaps.as_slice().iter().filter(|gap| gap.kind == GapKindV3Dto::SourceUnavailable).count(), 4);
            assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("512-byte source-row cap"))));
            assert!(continuation.is_some());

            let navigation_session = PacketProofSession::new();
            navigation_session.admit("path:stages.rs", 512);
            let navigation = freeze_packet_compilation(&controller, &project_id, &[], None, &navigation_session, Some(&initial_request))?;
            assert_eq!(navigation.product.support.len(), 1);
            assert!(navigation.product.support.iter().all(|unit| unit.kind == SupportUnitKindDto::SymbolLocation && unit.path.as_deref() == Some("stages.rs") && unit.snippet.is_none()));
            assert!(navigation.product.continuation.is_empty());
            let mut navigation_packet = source_page_test_packet();
            apply_frozen_packet_compilation(&mut navigation_packet, Some(&initial_request), navigation);
            assert_eq!(navigation_packet.disposition.kind, PacketDispositionKindDto::NotEstablished);
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "navigation-refusal-caller", &initial_request, &navigation_packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("bounded file refusal fits") };
            assert!(continuation.is_none());
            assert!(evidence.as_slice().iter().all(|row| row.kind != codestory_contracts::packet_projection_v3::EvidenceKindV3Dto::ExactSource));
            assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("512-byte source-row cap"))));

            let refused_session = PacketProofSession::new();
            refused_session.admit("path:unavailable.h", 512);
            refused_session.admit("node:3", 512);
            refused_session.admit("node:4", 512);
            let refused = freeze_packet_compilation(&controller, &project_id, &[], None, &refused_session, Some(&initial_request))?;
            assert!(refused.product.support.is_empty() && refused.product.continuation.is_empty());
            let mut refused_packet = source_page_test_packet();
            apply_frozen_packet_compilation(&mut refused_packet, Some(&initial_request), refused);
            assert_eq!(refused_packet.disposition.kind, PacketDispositionKindDto::NotEstablished);
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "empty-refusal-caller", &initial_request, &refused_packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("bounded empty refusal fits") };
            assert!(evidence.as_slice().is_empty() && continuation.is_none());
            assert_eq!(gaps.as_slice().iter().filter(|gap| gap.kind == GapKindV3Dto::SourceUnavailable).count(), 3);
            let request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({
                "question":"show repository source", "parent_packet_id":"first-source-page",
                "core_generation_id":"11111111-1111-4111-8111-111111111111", "option_ids":[option.id],
            })).unwrap();
            let probes = continuation_probes_from_options(&request, &project_id)?;
            assert!(matches!(&probes[0], PacketProbeDto::Continuation { selector, .. } if selector.source_range.unwrap().start_line == first_end + 1));
            let resolutions = crate::agent::packet_probe::resolve_packet_probes(&controller, probes.clone());
            assert_eq!(resolutions[0].status, PacketProbeResolutionStatusDto::Continuation, "{:?}", resolutions[0]);
            let second_session = PacketProofSession::new(); second_session.admit("node:2", 512);
            let second = freeze_packet_compilation(&controller, &project_id, &resolutions, None, &second_session, Some(&request))?;
            let source = second.product.support.iter().find(|unit| unit.source_delivery.is_some()).unwrap();
            assert_eq!(source.start_line, Some(first_end + 1));
            assert!(source.end_line.unwrap() > first_end);
            assert!(source.source_delivery.as_ref().unwrap().remaining_source_bytes < first_unread);
            let expected_receipt = source.source_delivery.clone();
            let mut packet = source_page_test_packet();
            packet.answer.source_coverage = second.source_coverage.clone();
            apply_frozen_packet_compilation(&mut packet, Some(&request), second);
            assert_eq!(packet.disposition.kind, PacketDispositionKindDto::NotEstablished);
            assert!(packet.disposition.drill.is_none(), "one permitted round cannot emit another drill");
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "page-test-caller", &request, &packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("small page fits") };
            assert!(continuation.is_none());
            assert_eq!(evidence.as_slice().iter().find(|row| row.source_delivery.is_some()).unwrap().source_delivery, expected_receipt);
            assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::EvidenceMissing && gap.message.as_ref().is_some_and(|message| message.as_str().contains("source bytes remain"))));
            for mutation in ["core", "project", "status", "node", "retrieval"] {
                let mut hostile = resolutions.clone();
                match mutation {
                    "status" => hostile[0].status = PacketProbeResolutionStatusDto::Rejected,
                    "node" => hostile[0].symbol_id = Some("3".into()),
                    _ => if let PacketProbeDto::Continuation { project_id, core_generation_id, retrieval_generation, .. } = &mut hostile[0].probe { match mutation { "core" => *core_generation_id = "different".into(), "project" => *project_id = "different".into(), _ => *retrieval_generation = Some("different".into()) } },
                }
                let reads = crate::search_evidence::verified_source_read_count();
                assert!(freeze_packet_compilation(&controller, &project_id, &hostile, None, &second_session, Some(&request)).is_err(), "{mutation}");
                assert_eq!(crate::search_evidence::verified_source_read_count(), reads, "refuse {mutation} before source hydration");
            }
            let mut conflict = resolutions.clone(); let mut different = resolutions[0].clone();
            if let PacketProbeDto::Continuation { selector, .. } = &mut different.probe { selector.source_range.as_mut().unwrap().start_line += 1; }
            conflict.push(different);
            assert!(freeze_packet_compilation(&controller, &project_id, &conflict, None, &second_session, Some(&request)).is_err());
            let mut stale_request = request.clone(); stale_request.core_generation_id = Some("different".into());
            let reads = crate::search_evidence::verified_source_read_count();
            assert!(freeze_packet_compilation(&controller, &project_id, &resolutions, None, &second_session, Some(&stale_request)).is_err());
            assert_eq!(crate::search_evidence::verified_source_read_count(), reads);
            Ok(())
        }).unwrap();
        }
    }

    fn source_page_test_packet() -> AgentPacketDto {
        let trace = serde_json::json!({"request_id":"source-page-request", "resolved_profile":"investigate",
            "policy_mode":"latency_first", "total_latency_ms":0, "sla_missed":false,
            "semantic_fallback_count":0, "semantic_fallbacks":[], "semantic_stage_timeout_zero_hits":0,
            "semantic_abstained_count":0, "annotations":[], "steps":[], "packet_sidecar_diagnostics":[]});
        serde_json::from_value(serde_json::json!({
            "packet_id":"source-page-request", "question":"show repository source", "plan":{"queries":[],"trace":[]},
            "answer":{"answer_id":"source-page-answer", "prompt":"show repository source", "summary":"fixture",
                "sections":[], "citations":[], "subgraph_ids":[], "retrieval_version":"fixture", "graphs":[], "retrieval_trace":trace,
                "freshness":{"status":"fresh", "changed_file_count":0, "new_file_count":0, "removed_file_count":0,
                    "checked_file_count":0, "indexed_file_count":0, "duration_ms":0}},
            "support":[], "disposition":{"kind":"not_established"},
            "budget":{"requested":"standard", "limits":{"max_anchors":16,"max_files":16,"max_snippets":16,"max_trail_edges":16,"max_output_bytes":65536},
                "used":{"anchors":0,"files":0,"snippets":0,"trail_edges":0,"output_bytes":0}, "truncated":false, "next_deeper_command":null},
            "retrieval_trace_summary":{"retrieval_trace":trace,"source_read_steps":0,"search_steps":0,"trail_steps":0}
        })).unwrap()
    }

    #[test]
    fn continuation_budget_counts_distinct_actions_in_compiler_order() {
        use codestory_contracts::api::PacketDispositionKindDto;
        use serde_json::json;

        let trace = json!({"request_id":"continuation-request", "resolved_profile":"investigate",
            "policy_mode":"latency_first", "total_latency_ms":0, "sla_missed":false,
            "semantic_fallback_count":0, "semantic_fallbacks":[], "semantic_stage_timeout_zero_hits":0,
            "semantic_abstained_count":0, "annotations":[], "steps":[], "packet_sidecar_diagnostics":[]});
        let packet: AgentPacketDto = serde_json::from_value(json!({
            "packet_id":"continuation-request", "question":"repository investigation",
            "plan":{"queries":[],"trace":[]},
            "answer":{"answer_id":"continuation-answer", "prompt":"repository investigation",
                "summary":"fixture", "sections":[], "citations":[], "subgraph_ids":[],
                "retrieval_version":"fixture", "graphs":[], "retrieval_trace":trace,
                "freshness":{"status":"fresh", "changed_file_count":0, "new_file_count":0,
                    "removed_file_count":0, "checked_file_count":0, "indexed_file_count":0,
                    "duration_ms":0}},
            "support":[], "disposition":{"kind":"not_established"},
            "budget":{"requested":"standard", "limits":{"max_anchors":16,"max_files":16,
                "max_snippets":16,"max_trail_edges":16,"max_output_bytes":65536},
                "used":{"anchors":0,"files":0,"snippets":0,"trail_edges":0,"output_bytes":0},
                "truncated":false,"next_deeper_command":null},
            "retrieval_trace_summary":{"retrieval_trace":trace,"source_read_steps":0,
                "search_steps":0,"trail_steps":0}
        }))
        .expect("fresh internal packet envelope");

        // An ambiguous exact selector can lack descriptor allocation bounds
        // before admission. Both recoverable structural reasons map to the
        // same executable symbol-read action.
        let candidate_ids = [9, 2, 10, 8, 3, 7, 4, 6, 5];
        for duplicate_reasons in [false, true] {
            let hydrated_ids = candidate_ids.iter().skip(usize::from(duplicate_reasons));
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project".into(),
                    core_generation_id: "core".into(),
                    retrieval_generation: Some("retrieval".into()),
                },
                admissions: hydrated_ids
                    .clone()
                    .enumerate()
                    .map(|(ordinal, id)| PacketAdmissionReceiptV1 {
                        packet_ordinal: ordinal as u32,
                        stable_identity: format!("node:{id}"),
                        score_version:
                            codestory_contracts::compilation::PACKET_RETRIEVAL_SCORE_VERSION_V1
                                .into(),
                        reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
                        origin: PacketAdmissionOriginV1::ExactTypedSelector,
                    })
                    .collect(),
                sources: hydrated_ids
                    .map(|id| PacketHydratedSourceRangeV1 {
                        stable_identity: format!("node:{id}"),
                        path: format!("src/symbol_{id}.rs"),
                        symbol: Some("shared_symbol".into()),
                        start_line: 1,
                        end_line: 1,
                        source: "fn shared_symbol() {}".into(),
                        delivery: None,
                        parser_completeness: PacketParserCompletenessV1::Complete,
                    })
                    .collect(),
                relations: Vec::new(),
                admission_gaps: if duplicate_reasons {
                    vec![PacketAdmissionGapV1 {
                        kind: PacketAdmissionGapKindV1::SourceBoundMissing,
                        stable_identity: Some("node:9".into()),
                        exact_selector_ordinal: Some(0),
                    }]
                } else {
                    Vec::new()
                },
                ambiguities: vec![PacketIdentityAmbiguityV1 {
                    selector: "shared_symbol".into(),
                    candidate_identities: candidate_ids
                        .iter()
                        .map(|id| format!("node:{id}"))
                        .collect(),
                }],
            };
            let compiled = compile_repository_evidence(&input);
            assert_eq!(
                compiled.continuation.len(),
                if duplicate_reasons { 10 } else { 9 }
            );
            let mut packet = packet.clone();
            packet.support = compiled.support.clone();
            let disposition = classify_packet_disposition(
                &packet,
                None,
                &compiled.continuation,
                "core".into(),
                Some("retrieval".into()),
            );
            let drill = disposition
                .drill
                .expect("first packet offers one continuation");
            assert_eq!(drill.options.len(), PACKET_DRILL_MAX_OPTIONS);
            assert_eq!(
                drill
                    .options
                    .iter()
                    .map(|option| option.symbol_id.clone())
                    .collect::<Vec<_>>(),
                candidate_ids[..8]
                    .iter()
                    .map(|id| Some(id.to_string()))
                    .collect::<Vec<_>>(),
                "duplicate reasons must not displace the eighth distinct action",
            );
            assert_eq!(
                drill
                    .options
                    .iter()
                    .map(|option| &option.id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                8
            );
            assert_eq!(
                drill.options[0].structural_reason,
                Some(if duplicate_reasons {
                    PacketStructuralGapReasonV1::SourceUnavailable
                } else {
                    PacketStructuralGapReasonV1::AmbiguousSelector
                })
            );
            assert_eq!(
                drill.gap_ids,
                drill
                    .options
                    .iter()
                    .map(|option| option.gap_id.clone())
                    .collect::<Vec<_>>()
            );
            assert_eq!(drill.core_generation_id, "core");
            assert_eq!(drill.retrieval_generation.as_deref(), Some("retrieval"));
            assert_eq!(drill.remaining_rounds, 1);

            for request_json in [
                json!({"question":"repository investigation", "parent_packet_id":packet.packet_id}),
                json!({"question":"repository investigation", "option_ids":[drill.options[0].id]}),
                json!({"question":"repository investigation", "core_generation_id":"other-core"}),
                json!({"question":"repository investigation", "retrieval_generation":"other-retrieval"}),
            ] {
                let request: AgentPacketRequestDto = serde_json::from_value(request_json).unwrap();
                let terminal = classify_packet_disposition(
                    &packet,
                    Some(&request),
                    &compiled.continuation,
                    "core".into(),
                    Some("retrieval".into()),
                );
                assert!(terminal.drill.is_none());
                if request.parent_packet_id.is_some()
                    || !request.option_ids.is_empty()
                    || request.probes.iter().any(|probe| {
                        matches!(
                            probe,
                            codestory_contracts::api::PacketProbeDto::Continuation { .. }
                        )
                    })
                {
                    assert_eq!(
                        terminal.reason.as_deref(),
                        Some("the bounded continuation left a structural gap")
                    );
                }
                assert_eq!(
                    terminal.kind,
                    if request.parent_packet_id.is_some()
                        || !request.option_ids.is_empty()
                        || request.probes.iter().any(|probe| matches!(
                            probe,
                            codestory_contracts::api::PacketProbeDto::Continuation { .. }
                        ))
                    {
                        PacketDispositionKindDto::NotEstablished
                    } else {
                        PacketDispositionKindDto::Unavailable
                    }
                );
            }
        }
    }

    #[test]
    fn frozen_public_packet_keeps_only_admitted_sources_and_induced_relations() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        use crate::agent::packet_candidate::PacketAdmissionDecision;
        use codestory_contracts::packet_projection_v3::{EvidenceKindV3Dto, PacketProjectionV3Dto};
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
        use serde_json::json;
        use std::sync::{Arc, atomic::AtomicBool};

        let project = tempfile::tempdir().expect("isolated source project");
        let cache = tempfile::tempdir().expect("isolated Store");
        let database = cache.path().join("core.sqlite3");
        let mut storage = Store::open(&database).expect("real file-backed Store");
        // Source bodies and bounds are stored independently of admission. The
        // unrelated rows must remain present throughout the test.
        let files = [
            (10, "src/alpha.rs", "fn alpha() {}\n"),
            (20, "tools/repos/worker/src/beta.rs", "fn beta() {}\n"),
            (30, "tools/runner/src/alpha.rs", "fn gamma() {}\n"),
            (
                40,
                "tools/runner/src/reference.rs",
                "const REFERENCE: usize = 1;\n",
            ),
            (90, "noise/outside.rs", "fn NOISE_OUTSIDE() {}\n"),
            (190, "noise/unrelated.rs", "fn NOISE_UNRELATED() {}\n"),
        ];
        for (id, relative, source) in files {
            let path = project.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("source directory");
            std::fs::write(&path, source).expect("real source bytes");
            let file = FileInfo {
                id,
                path,
                language: "rust".into(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            };
            storage.insert_file(&file).expect("file row");
            storage
                .update_file_metadata(
                    &file,
                    Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
                )
                .expect("pinned source digest");
            storage
                .insert_nodes_batch(&[CoreNode {
                    id: CoreNodeId(id),
                    kind: CoreNodeKind::FILE,
                    serialized_name: relative.into(),
                    file_node_id: Some(CoreNodeId(id)),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..Default::default()
                }])
                .expect("file node");
        }
        for (id, file_id, name) in [
            (11, 10, "alpha"),
            (21, 20, "beta"),
            (31, 30, "gamma"),
            (99, 90, "NOISE_OUTSIDE"),
            (199, 190, "NOISE_UNRELATED"),
        ] {
            storage
                .insert_nodes_batch(&[CoreNode {
                    id: CoreNodeId(id),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: name.into(),
                    file_node_id: Some(CoreNodeId(file_id)),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..Default::default()
                }])
                .expect("stored symbol");
        }
        let kinds = [
            (CoreEdgeKind::MEMBER, PacketRelationKindV1::Member),
            (CoreEdgeKind::TYPE_USAGE, PacketRelationKindV1::TypeUsage),
            (CoreEdgeKind::USAGE, PacketRelationKindV1::Usage),
            (CoreEdgeKind::CALL, PacketRelationKindV1::Call),
            (CoreEdgeKind::INHERITANCE, PacketRelationKindV1::Inheritance),
            (CoreEdgeKind::OVERRIDE, PacketRelationKindV1::Override),
            (
                CoreEdgeKind::TYPE_ARGUMENT,
                PacketRelationKindV1::TypeArgument,
            ),
            (
                CoreEdgeKind::TEMPLATE_SPECIALIZATION,
                PacketRelationKindV1::TemplateSpecialization,
            ),
            (CoreEdgeKind::INCLUDE, PacketRelationKindV1::Include),
            (CoreEdgeKind::IMPORT, PacketRelationKindV1::Import),
            (CoreEdgeKind::MACRO_USAGE, PacketRelationKindV1::MacroUsage),
            (
                CoreEdgeKind::ANNOTATION_USAGE,
                PacketRelationKindV1::AnnotationUsage,
            ),
            (CoreEdgeKind::UNKNOWN, PacketRelationKindV1::Unknown),
        ];
        let mut expected_relations = Vec::new();
        for (base, from, to) in [(100, 11, 21), (200, 21, 31)] {
            for (offset, (kind, expected_kind)) in kinds.into_iter().enumerate() {
                let id = base + offset as i64;
                for representative in [id, id + 1_000] {
                    storage
                        .insert_edge(&CoreEdge {
                            id: CoreEdgeId(representative),
                            source: CoreNodeId(from),
                            target: CoreNodeId(to),
                            kind,
                            certainty: Some(ResolutionCertainty::Certain),
                            ..Default::default()
                        })
                        .expect("certain kind/pair and duplicate");
                }
                expected_relations.push((
                    id.to_string(),
                    format!("node:{from}"),
                    format!("node:{to}"),
                    expected_kind,
                ));
            }
        }
        // Both directions of a one-admitted-endpoint edge, a wholly unrelated
        // pair, and an earlier probable duplicate cannot become induced evidence.
        for (id, from, to, certainty) in [
            (1, 11, 99, ResolutionCertainty::Certain),
            (2, 199, 21, ResolutionCertainty::Certain),
            (3, 99, 199, ResolutionCertainty::Certain),
            (4, 11, 21, ResolutionCertainty::Probable),
        ] {
            storage
                .insert_edge(&CoreEdge {
                    id: CoreEdgeId(id),
                    source: CoreNodeId(from),
                    target: CoreNodeId(to),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(certainty),
                    ..Default::default()
                })
                .expect("hostile stored edge");
        }
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "h7-core-generation".into(),
            run_id: "h7-core-run".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        storage
            .put_index_publication(&publication)
            .expect("complete fixture publication");
        assert_eq!(storage.get_nodes().unwrap().len(), 11);
        assert_eq!(storage.get_edges().unwrap().len(), 56);
        drop(storage);

        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        {
            let mut state = controller.state.lock();
            state.project_root = Some(project.path().to_path_buf());
            state.storage_path = Some(database);
        }
        let service = crate::services::PublicOperationService::new(controller.clone());
        let session = PacketProofSession::new();
        let admitted = [
            "node:11",
            "node:21",
            "node:31",
            "path:tools/runner/src/reference.rs",
        ];
        for identity in admitted {
            assert_eq!(
                session.admit(identity, INTERIM_SOURCE_ROW_UPPER_BOUND),
                PacketAdmissionDecision::Admitted
            );
        }
        let request: AgentPacketRequestDto =
            serde_json::from_value(json!({"question":"stored evidence fixture"})).unwrap();
        let trace = json!({"request_id":"h7-request", "resolved_profile":"investigate",
            "policy_mode":"latency_first", "total_latency_ms":0, "sla_missed":false,
            "semantic_fallback_count":0, "semantic_fallbacks":[], "semantic_stage_timeout_zero_hits":0,
            "semantic_abstained_count":0, "annotations":[], "steps":[], "packet_sidecar_diagnostics":[]});
        // Only the input packet envelope is a fixture. Evidence comes from the
        // real runtime hydration/compiler and real v3 projection below.
        let mut packet: AgentPacketDto = serde_json::from_value(json!({
            "packet_id":"h7-request", "question":request.question, "plan":{"queries":[],"trace":[]},
            "answer":{"answer_id":"h7-answer", "prompt":request.question, "summary":"fixture",
                "sections":[], "citations":[], "subgraph_ids":[], "retrieval_version":"fixture",
                "graphs":[], "retrieval_trace":trace}, "support":[],
            "disposition":{"kind":"not_established"},
            "budget":{"requested":"standard", "limits":{"max_anchors":16,"max_files":16,
                "max_snippets":16,"max_trail_edges":16,"max_output_bytes":65536},
                "used":{"anchors":0,"files":0,"snippets":0,"trail_edges":0,"output_bytes":0},
                "truncated":false, "next_deeper_command":null},
            "retrieval_trace_summary":{"retrieval_trace":trace,"source_read_steps":0,"search_steps":0,"trail_steps":0}
        })).expect("typed internal packet envelope");
        // Missing freshness remains unavailable. The hydration fixture then
        // opts into the existing synthetic freshness control so that its
        // source refusal assertions exercise hydration rather than that cap.
        let unknown = classify_packet_disposition(
            &packet,
            Some(&request),
            &[],
            publication.generation_id.clone(),
            None,
        );
        assert_eq!(
            unknown.kind,
            codestory_contracts::api::PacketDispositionKindDto::Unavailable
        );
        assert!(
            unknown
                .reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("freshness unknown"))
        );
        packet.answer.freshness =
            Some(codestory_agent::packet_freshness::fresh_index_observation());
        let projected = service
            .run_observational_with_cancel(
                "h7-packet-compilation",
                Arc::new(AtomicBool::new(false)),
                || {
                    assert_eq!(controller.active_core_publication().unwrap(), publication);
                    let pinned = controller.open_storage_read_only()?;
                    let mut gaps = Vec::new();
                    let (authenticated, sources, _) = hydrate_admitted_sources(
                        &controller,
                        &pinned,
                        &session.receipts(),
                        &mut gaps,
                    )?;
                    assert!(gaps.is_empty());
                    assert_eq!(
                        authenticated
                            .iter()
                            .map(|item| item.receipt.stable_identity.as_str())
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from(admitted)
                    );
                    assert_eq!(
                        sources
                            .iter()
                            .map(|source| source.stable_identity.as_str())
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from(admitted)
                    );
                    let induced = hydrate_induced_relations(&pinned, &authenticated)?;
                    assert!(
                        induced
                            .iter()
                            .all(|edge| edge.certainty == PacketRelationCertaintyV1::Certain)
                    );
                    let mut observed = induced
                        .iter()
                        .map(|edge| {
                            (
                                edge.relation_id.clone(),
                                edge.from_identity.clone(),
                                edge.to_identity.clone(),
                                edge.relation_kind,
                            )
                        })
                        .collect::<Vec<_>>();
                    observed.sort();
                    expected_relations.sort();
                    assert_eq!(observed, expected_relations);
                    assert_eq!(
                        induced.len(),
                        26,
                        "all kinds for both admitted directed pairs before forest selection"
                    );
                    let frozen = freeze_packet_compilation(
                        &controller,
                        "h7-project",
                        &[],
                        None,
                        &session,
                        None,
                    )?;
                    assert_eq!(
                        frozen
                            .source_coverage
                            .iter()
                            .map(|row| row.path.as_str())
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from([
                            "src/alpha.rs",
                            "tools/repos/worker/src/beta.rs",
                            "tools/runner/src/alpha.rs",
                            "tools/runner/src/reference.rs"
                        ])
                    );
                    packet.answer.source_coverage = frozen.source_coverage.clone();
                    apply_frozen_packet_compilation(&mut packet, Some(&request), frozen);
                    assert_eq!(
                        packet.support.len(),
                        6,
                        "four real source rows and two forest relations fit caps"
                    );
                    assert_eq!(
                        final_support_identities_for_observation(&packet.support)
                            .into_iter()
                            .collect::<BTreeSet<_>>(),
                        admitted.into_iter().map(str::to_owned).collect()
                    );
                    let source_units = packet
                        .support
                        .iter()
                        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                        .collect::<Vec<_>>();
                    assert_eq!(
                        source_units
                            .iter()
                            .map(|unit| (unit.symbol_id.as_deref(), unit.path.as_deref()))
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from([
                            (Some("11"), Some("src/alpha.rs")),
                            (Some("21"), Some("tools/repos/worker/src/beta.rs")),
                            (Some("31"), Some("tools/runner/src/alpha.rs")),
                            (None, Some("tools/runner/src/reference.rs"))
                        ])
                    );
                    assert!(
                        source_units
                            .iter()
                            .all(|unit| unit.start_line == Some(1) && unit.end_line == Some(1))
                    );
                    for (path, body) in [
                        ("src/alpha.rs", "fn alpha() {}"),
                        ("tools/repos/worker/src/beta.rs", "fn beta() {}"),
                        ("tools/runner/src/alpha.rs", "fn gamma() {}"),
                        ("tools/runner/src/reference.rs", "const REFERENCE: usize = 1;"),
                    ] {
                        let source = source_units
                            .iter()
                            .find(|unit| unit.path.as_deref() == Some(path))
                            .expect("expected admitted source row");
                        assert!(
                            source
                                .snippet
                                .as_ref()
                                .is_some_and(|text| text.contains(body)),
                            "source body must match its admitted path: {path}"
                        );
                    }
                    assert_eq!(
                        packet
                            .support
                            .iter()
                            .filter(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge)
                            .map(|unit| (
                                unit.id.as_str(),
                                unit.from_symbol.as_deref(),
                                unit.to_symbol.as_deref(),
                                unit.edge_kind.as_deref()
                            ))
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from([
                            ("edge:100", Some("11"), Some("21"), Some("member")),
                            ("edge:200", Some("21"), Some("31"), Some("member"))
                        ])
                    );
                    let positive = crate::evidence_projection_v3::project_packet_v3(
                        &service,
                        "h7-caller",
                        &request,
                        &packet,
                        |projection| {
                            serde_json::to_vec(projection)
                                .map(|bytes| bytes.len())
                                .map_err(|_| ())
                        },
                    )?;
                    let path = project.path().join("tools/repos/worker/src/beta.rs");
                    let original = std::fs::read(&path).unwrap();
                    let check_refusal = |label: &str| -> Result<(), codestory_contracts::api::ApiError> {
                        let session = PacketProofSession::new();
                        session.admit("node:21", INTERIM_SOURCE_ROW_UPPER_BOUND);
                        session.admit("path:tools/repos/worker/src/beta.rs", INTERIM_SOURCE_ROW_UPPER_BOUND);
                        let frozen = freeze_packet_compilation(&controller, "h7-project", &[], None, &session, Some(&request))?;
                        assert!(frozen.product.support.is_empty(), "{label}");
                        assert!(frozen.product.continuation.is_empty(), "{label}");
                        assert_eq!(frozen.source_refusals.len(), 2, "{label}");
                        let mut refused = packet.clone();
                        refused.packet_id = format!("h7-refusal-{label}");
                        refused.answer.source_coverage = frozen.source_coverage.clone();
                        apply_frozen_packet_compilation(&mut refused, Some(&request), frozen);
                        assert_eq!(refused.disposition.kind, codestory_contracts::api::PacketDispositionKindDto::NotEstablished, "{label}");
                        let public = crate::evidence_projection_v3::project_packet_v3(&service, "h7-refusal-caller", &request, &refused, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
                        let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, answer_sufficiency, .. } = public.projection else { panic!("bounded refusal fits: {label}") };
                        assert!(evidence.as_slice().is_empty(), "{label}");
                        assert!(continuation.is_none(), "{label}");
                        assert_eq!(gaps.as_slice().iter().filter(|gap| gap.kind == codestory_contracts::packet_projection_v3::GapKindV3Dto::SourceUnavailable).count(), 2, "{label}");
                        assert_eq!(serde_json::to_value(answer_sufficiency).unwrap(), json!("not_asserted"), "{label}");
                        Ok(())
                    };
                    std::fs::remove_file(&path).unwrap();
                    check_refusal("deleted")?;
                    std::fs::write(&path, "fn changed() {}\n").unwrap();
                    check_refusal("stale")?;
                    std::fs::remove_file(&path).unwrap();
                    std::fs::create_dir(&path).unwrap();
                    check_refusal("directory")?;
                    std::fs::remove_dir(&path).unwrap();
                    #[cfg(unix)]
                    {
                        let outside = tempfile::tempdir().unwrap();
                        let target = outside.path().join("beta.rs");
                        std::fs::write(&target, &original).unwrap();
                        std::os::unix::fs::symlink(&target, &path).unwrap();
                        check_refusal("escaping-symlink")?;
                        std::fs::remove_file(&path).unwrap();
                    }
                    std::fs::write(&path, &original).unwrap();
                    Ok(positive)
                },
            )
            .expect("normal pinned freeze-to-public projection")
            .value;
        let PacketProjectionV3Dto::Complete { evidence, gaps, .. } = &projected.projection else {
            panic!("small admitted fixture must not exceed public output budget")
        };
        assert_eq!(evidence.as_slice().len(), 6);
        assert!(!gaps.as_slice().iter().any(|gap| gap.kind
            == codestory_contracts::packet_projection_v3::GapKindV3Dto::OutputBudgetExceeded));
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::ExactSource)
                .map(|row| (
                    row.symbol_id.as_ref().map(|id| id.as_str()),
                    row.path.as_ref().map(|path| path.as_str())
                ))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                (Some("11"), Some("src/alpha.rs")),
                (Some("21"), Some("tools/repos/worker/src/beta.rs")),
                (Some("31"), Some("tools/runner/src/alpha.rs")),
                (None, Some("tools/runner/src/reference.rs"))
            ])
        );
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::GraphRelation)
                .map(|row| row.summary.as_ref().unwrap().as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["11 -[member]-> 21", "21 -[member]-> 31"])
        );
        let public_json =
            serde_json::to_string(&projected.projection).expect("actual public serialization");
        for noise in ["NOISE_", "noise/"] {
            assert!(
                !public_json.contains(noise),
                "unadmitted stored evidence leaked: {noise}"
            );
        }
        let retained = controller.open_storage_read_only().unwrap();
        assert!(retained.get_node(CoreNodeId(99)).unwrap().is_some());
        assert_eq!(
            retained.get_edges().unwrap().len(),
            56,
            "exclusion is not deletion of stored noise"
        );
    }

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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let storage = Store::new_in_memory().expect("store");
        let path = project.path().join("tools/repos/worker/src/settings.rs");
        let admission = PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: "path:tools/repos/worker/src/settings.rs".into(),
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
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let short = "const ENABLED: bool = true;\nconst LIMIT: usize = 2;\n";
        std::fs::write(&path, short).expect("short source");
        storage.insert_file(&file).expect("file");
        let short_hash = format!("{:x}", Sha256::digest(short.as_bytes()));
        storage
            .update_file_metadata(&file, Some(&short_hash))
            .expect("pinned source hash");
        let whole = hydrate_admitted_file_source(&controller, &storage, &admission, &file)
            .expect("complete short file source");
        assert_eq!(whole.path, "tools/repos/worker/src/settings.rs");
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
            source_delivery: None,
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
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
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
        assert!(
            product.continuation.is_empty(),
            "the same pinned oversized file cannot fit a retry at the same row limit"
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

    fn test_admission(
        packet_ordinal: u32,
        stable_identity: &str,
        node_id: i64,
    ) -> AuthenticatedPacketAdmissionV1 {
        AuthenticatedPacketAdmissionV1 {
            receipt: PacketAdmissionReceiptV1 {
                packet_ordinal,
                stable_identity: stable_identity.into(),
                score_version: "test".into(),
                reserved_source_bytes: 1,
                origin: PacketAdmissionOriginV1::Retrieval,
            },
            core_node_id: CoreNodeId(node_id),
        }
    }

    #[test]
    fn uncertain_relation_is_not_compiler_evidence() {
        assert_eq!(
            relation_certainty(Some(ResolutionCertainty::Uncertain)),
            PacketRelationCertaintyV1::Uncertain
        );

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
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "crate::run".into(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                CoreEdge {
                    id: CoreEdgeId(10),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
                CoreEdge {
                    id: CoreEdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::CALL,
                    certainty: Some(ResolutionCertainty::Uncertain),
                    ..Default::default()
                },
            ])
            .expect("insert edges");
        let admissions = [
            test_admission(0, "path:src/a.rs", 1),
            test_admission(1, "node:2", 2),
        ];

        let relations = hydrate_induced_relations(&storage, &admissions).expect("relations");

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].relation_id, "10");
        assert_eq!(relations[0].certainty, PacketRelationCertaintyV1::Certain);
    }

    #[test]
    fn numeric_confidence_cannot_upgrade_missing_certainty() {
        assert_eq!(relation_certainty(None), PacketRelationCertaintyV1::Unknown);

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
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "crate::run".into(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                CoreEdge {
                    id: CoreEdgeId(10),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
                CoreEdge {
                    id: CoreEdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::CALL,
                    confidence: Some(0.99),
                    certainty: None,
                    ..Default::default()
                },
            ])
            .expect("insert edges");
        let admissions = [
            test_admission(0, "path:src/a.rs", 1),
            test_admission(1, "node:2", 2),
        ];

        let relations = hydrate_induced_relations(&storage, &admissions).expect("relations");

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].relation_id, "10");
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
