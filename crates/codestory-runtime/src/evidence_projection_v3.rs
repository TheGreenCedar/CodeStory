//! Public evidence-only projection facade for CodeStory schema 3.

use std::collections::{HashMap, HashSet};

use codestory_contracts::{
    api::{
        AgentAnswerDto, AgentPacketDto, AgentPacketRequestDto, PacketDispositionKindDto, SearchHit,
        SearchResultsDto, SupportUnitDto, SupportUnitKindDto,
    },
    packet_projection_v3::{
        BoundedVecV3, ContextEvidenceRowV3Dto, ContextProjectionV3Dto, ContextTargetV3Dto,
        ContinuationStateV3Dto, DiagnosticCategoryV3Dto, DiagnosticCodeTextV3,
        DiagnosticsCapabilityV3Dto, EvidenceIdentityV3Dto, EvidenceKindV3Dto, GapIdentityV3Dto,
        GapKindV3Dto, IdentityTextV3, PacketEvidenceRowV3Dto, PacketProjectionV3Dto, PathTextV3,
        ProjectionGapRowV3Dto, RetrievalStateDescriptorV3Dto, RetrievalStateV3Dto,
        SearchEvidenceRowV3Dto, SearchProjectionV3Dto, Sha256DigestV3Dto, SummaryTextV3,
        SymbolIdTextV3,
    },
};
use sha2::{Digest, Sha256};

use crate::{
    agent::{
        packet_execution_record_v3::{
            CapturedPacketPublicationV3, FinalizedDiagnosticSourceRowV3,
            FinalizedPacketExecutionInputV3, PacketRequestFingerprintV3,
            build_packet_execution_record_v3, build_packet_execution_record_v3_with_id,
            capture_packet_publication_v3,
        },
        packet_projection_v3::{
            DiagnosticArtifactBuildV3, FinalizedContextProjectionInputV3,
            FinalizedSearchProjectionInputV3, build_context_projection_v3,
            build_diagnostic_artifact_v3, build_packet_projection_v3, build_search_projection_v3,
            finalize_packet_projection_v3, packet_budget_exceeded_projection_v3,
        },
    },
    services::{ActivePacketRepresentationContextV3, PublicOperationService},
};

/// Immutable bytes and identities the transport needs to mint one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketDiagnosticProjectionV3 {
    pub bytes: Vec<u8>,
    pub packet_id: String,
    pub project_identity: String,
    pub core_generation: String,
    pub core_run: String,
    pub retrieval_generation: Option<String>,
    pub request_digest: String,
}

/// One finalized packet projection plus its immutable diagnostic artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketEvidenceProductV3 {
    pub(crate) protected_source_witnesses:
        Vec<crate::agent::packet_served_sources::ServedSourceWitness>,
    pub(crate) source_issuance_seal:
        Option<crate::agent::packet_served_sources::PacketSourceIssuanceSeal>,
    pub projection: PacketProjectionV3Dto,
    pub diagnostics: PacketDiagnosticProjectionV3,
}

/// A single immutable UUID reserved for all unserved previews and final delivery.
#[derive(Debug, Clone)]
pub struct PacketProjectionIdentityV3(String);
impl PacketProjectionIdentityV3 {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
impl Default for PacketProjectionIdentityV3 {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct RepresentedPacketProductV3 {
    pub packet: AgentPacketDto,
    pub product: PacketEvidenceProductV3,
}

/// Re-apply the packet budget against an adapter's complete serialized
/// representation. This is required when the adapter adds publication
/// metadata or mirrors the root after the runtime projection was built.
pub fn finalize_packet_projection_v3_for_representation(
    projection: &mut PacketProjectionV3Dto,
    measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<usize, codestory_contracts::api::ApiError> {
    finalize_packet_projection_v3(projection, measure)
        .map_err(|error| projection_error("packet representation budget", error))
}

/// Build the typed public fallback when an adapter receives an overbound
/// internal complete projection that cannot be deserialized into the closed
/// 16-row DTO. Adapters supply only the already-typed envelope; runtime owns
/// the public gap vocabulary and result shape.
pub fn packet_budget_exceeded_projection_v3_from_envelope(
    schema_version: u16,
    identity: codestory_contracts::packet_projection_v3::PacketRequestIdentityV3Dto,
    publication: codestory_contracts::packet_projection_v3::PublicationIdentityV3Dto,
    retrieval: RetrievalStateDescriptorV3Dto,
    diagnostics: DiagnosticsCapabilityV3Dto,
    required_complete_bytes: usize,
) -> PacketProjectionV3Dto {
    packet_budget_exceeded_projection_v3(
        schema_version,
        identity,
        publication,
        retrieval,
        diagnostics,
        required_complete_bytes,
    )
}

/// Convert the runtime's finalized packet execution into the only public v3
/// packet vocabulary. The legacy disposition is consumed only to retain gaps
/// and a bounded continuation; it is never serialized or treated as proof.
pub fn project_packet_v3(
    service: &PublicOperationService,
    caller_id: &str,
    request: &AgentPacketRequestDto,
    packet: &AgentPacketDto,
    measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<PacketEvidenceProductV3, codestory_contracts::api::ApiError> {
    project_packet_v3_using_record_builder(
        &service.active_project_identity_v3()?.project_id,
        caller_id,
        request,
        packet,
        measure,
        |input| build_packet_execution_record_v3(service, input),
    )
}

// Whole-request bound derived from the existing proposal bound and finalizer
// bound (initial/all-label/minimal, bounded labels, bounded binary probes,
// fallback and caller revalidation). One additional baseline projection is
// allowed; these are transport probes, never neural or retrieval counters.
pub(crate) const PACKET_TRANSPORT_MEASUREMENTS_MAX_V1: usize =
    (8 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1 + 2)
        * (codestory_contracts::packet_projection_v3::GAP_ROWS_MAX_V3 + 11);

pub(crate) fn bounded_packet_transport_measure_v3(
    mut measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()> {
    let mut calls = 0usize;
    move |projection| {
        if calls >= PACKET_TRANSPORT_MEASUREMENTS_MAX_V1 {
            return Err(());
        }
        calls += 1;
        measure(projection)
    }
}

/// Private per-proposal baseline binding, never a public wire policy.
pub(crate) struct PacketRepresentationBaselineV1 {
    pub(crate) packet: AgentPacketDto,
    pub(crate) resolved_gap_witnesses: std::collections::BTreeMap<String, SupportUnitDto>,
    pub(crate) added_source_units: Vec<SupportUnitDto>,
}

/// Compare the final typed frame that the transport actually measures. Row
/// ordinal IDs can change; full source/graph content and action authority cannot.
pub(crate) fn packet_projection_preserves_baseline_v3(
    baseline: &PacketProjectionV3Dto,
    candidate: &PacketProjectionV3Dto,
    resolved_witnesses: &std::collections::BTreeMap<String, SupportUnitDto>,
    added_source_units: &[SupportUnitDto],
) -> bool {
    let PacketProjectionV3Dto::Complete {
        identity: old_id,
        publication: old_pub,
        retrieval: old_retrieval,
        evidence: old_rows,
        gaps: old_gaps,
        continuation: old_continuation,
        ..
    } = baseline
    else {
        return false;
    };
    let PacketProjectionV3Dto::Complete {
        identity,
        publication,
        retrieval,
        evidence: rows,
        gaps,
        continuation,
        ..
    } = candidate
    else {
        return false;
    };
    if old_id != identity || old_pub != publication || old_retrieval != retrieval {
        return false;
    }
    // An internal support unit resolves an action or earns a gain only if
    // its exact authenticated source content is delivered in this final frame.
    let delivered = |unit: &SupportUnitDto| {
        let Some(expected) = packet_evidence_row(0, unit) else {
            return false;
        };
        if unit.kind != SupportUnitKindDto::SourceRange || unit.source_delivery.is_none() {
            return false;
        }
        rows.as_slice().iter().any(|row| {
            let mut normalized = row.clone();
            normalized.identity = expected.identity.clone();
            // A newly delivered partial page can carry its own normal next-page
            // action. Existing baseline action authority is compared below.
            normalized.packet_request_patch = None;
            normalized == expected
        })
    };
    if added_source_units.iter().any(|unit| !delivered(unit)) {
        return false;
    }
    let resolved = resolved_witnesses
        .iter()
        .filter(|(_, witness)| delivered(witness))
        .map(|(id, _)| id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for old in old_rows.as_slice() {
        if !rows.as_slice().iter().any(|row| {
            let mut normalized = row.clone();
            normalized.identity = old.identity.clone();
            normalized == *old
        }) {
            return false;
        }
    }
    let unresolved = old_gaps
        .as_slice()
        .iter()
        .filter(|gap| {
            gap.kind == GapKindV3Dto::ContinuationRequired
                && !resolved.contains(gap.identity.gap_id.as_str())
        })
        .collect::<Vec<_>>();
    for gap in old_gaps.as_slice() {
        if gap.kind == GapKindV3Dto::OutputBudgetExceeded
            || (gap.kind == GapKindV3Dto::ContinuationRequired
                && resolved.contains(gap.identity.gap_id.as_str()))
        {
            continue;
        }
        if !gaps.as_slice().contains(gap) {
            return false;
        }
    }
    if !unresolved.is_empty() {
        match (old_continuation, continuation) {
            (None, None) => {}
            (Some(old), Some(current)) => {
                if old.continuation_id != current.continuation_id
                    || old.remaining_rounds != current.remaining_rounds
                {
                    return false;
                }
                for gap in &unresolved {
                    if old.gap_ids.as_slice().contains(&gap.identity)
                        && !current.gap_ids.as_slice().contains(&gap.identity)
                    {
                        return false;
                    }
                }
                match (&old.packet_request_patch, &current.packet_request_patch) {
                    (None, None) => {}
                    (Some(old_patch), Some(patch)) => {
                        if old_patch.parent_packet_id != patch.parent_packet_id
                            || old_patch.core_generation_id != patch.core_generation_id
                            || old_patch.retrieval_generation != patch.retrieval_generation
                            || old_patch.probes != patch.probes
                        {
                            return false;
                        }
                        for gap in &unresolved {
                            if old_patch
                                .option_ids
                                .as_slice()
                                .contains(&gap.identity.gap_id)
                                && !patch.option_ids.as_slice().contains(&gap.identity.gap_id)
                            {
                                return false;
                            }
                        }
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
    }
    true
}

pub fn project_packet_v3_with_identity(
    service: &PublicOperationService,
    caller_id: &str,
    request: &AgentPacketRequestDto,
    packet: &AgentPacketDto,
    reserved: &PacketProjectionIdentityV3,
    measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<PacketEvidenceProductV3, codestory_contracts::api::ApiError> {
    PacketRepresentationCaptureV3::new(service)?
        .project(caller_id, request, packet, reserved, measure)
}

/// All unserved proposals share one full identity observation under their actual owner.
pub(crate) struct PacketRepresentationCaptureV3<'a> {
    service: &'a PublicOperationService,
    owner: ActivePacketRepresentationContextV3,
    publication: CapturedPacketPublicationV3,
}

impl<'a> PacketRepresentationCaptureV3<'a> {
    pub(crate) fn new(
        service: &'a PublicOperationService,
    ) -> Result<Self, codestory_contracts::api::ApiError> {
        let owner = service
            .active_packet_representation_context_v3()
            .ok_or_else(|| {
                codestory_contracts::api::ApiError::internal(
                    "Representation capture requires the actual fresh packet owner.",
                )
            })?;
        let publication = capture_packet_publication_v3(service)
            .map_err(|error| projection_error("packet publication", error))?;
        let capture = Self {
            service,
            owner,
            publication,
        };
        capture.check_owner()?;
        Ok(capture)
    }

    fn check_owner(&self) -> Result<(), codestory_contracts::api::ApiError> {
        if self
            .service
            .active_packet_representation_context_v3()
            .as_ref()
            != Some(&self.owner)
        {
            return Err(codestory_contracts::api::ApiError::new(
                "publication_changed",
                "packet representation owner changed after identity capture",
            ));
        }
        Ok(())
    }

    pub(crate) fn project(
        &self,
        caller_id: &str,
        request: &AgentPacketRequestDto,
        packet: &AgentPacketDto,
        reserved: &PacketProjectionIdentityV3,
        measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
    ) -> Result<PacketEvidenceProductV3, codestory_contracts::api::ApiError> {
        self.check_owner()?;
        let product = project_packet_v3_using_record_builder(
            self.publication.project_id(),
            caller_id,
            request,
            packet,
            measure,
            |input| build_packet_execution_record_v3_with_id(&self.publication, input, &reserved.0),
        )?;
        self.check_owner()?;
        Ok(product)
    }
}

fn project_packet_v3_using_record_builder(
    project_id: &str,
    caller_id: &str,
    request: &AgentPacketRequestDto,
    packet: &AgentPacketDto,
    mut measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
    build_record: impl FnOnce(
        &FinalizedPacketExecutionInputV3,
    ) -> Result<
        crate::agent::packet_execution_record_v3::PacketExecutionRecordV3,
        crate::agent::packet_execution_record_v3::RecordValidationErrorV3,
    >,
) -> Result<PacketEvidenceProductV3, codestory_contracts::api::ApiError> {
    for unit in &packet.support {
        validate_packet_source_page(unit)?;
    }
    let PacketEvidenceSelectionV3 {
        rows: mut evidence,
        was_bounded: evidence_was_bounded,
    } = packet_evidence_selection(&packet.support);

    attach_source_page_actions(project_id, packet, &mut evidence)?;
    let mut gaps = packet_gaps(packet);
    if evidence_was_bounded {
        gaps.push(gap_row(
            "evidence-projection-bounded",
            GapKindV3Dto::OutputBudgetExceeded,
            Some(
                "Additional internal support rows were omitted from the bounded public projection.",
            ),
        )?);
    }
    canonicalize_gaps(&mut gaps);
    let continuation = packet_continuation(packet, &mut gaps)?;
    canonicalize_gaps(&mut gaps);
    let diagnostics = diagnostic_rows(&packet.answer, &gaps)?;
    let retrieval = retrieval_state(packet.answer.retrieval_trace.retrieval_publication.as_ref());
    let input = FinalizedPacketExecutionInputV3::new(
        identity(caller_id, "caller_id")?,
        // `packet_id` is the immutable execution request identity on the
        // internal packet. The trace copy is optional diagnostics and the hard
        // public-budget path may remove it before this projection is built.
        identity(&packet.packet_id, "request_id")?,
        PacketRequestFingerprintV3::from_current_request(request),
        evidence,
        gaps,
        continuation,
        retrieval,
        diagnostics,
    );
    let record = build_record(&input).map_err(|error| projection_error("packet record", error))?;
    let artifact = build_diagnostic_artifact_v3(&record)
        .map_err(|error| projection_error("packet diagnostics", error))?;
    let (bytes, reference) = match artifact {
        DiagnosticArtifactBuildV3::Complete {
            bytes, reference, ..
        } => (bytes.as_slice().to_vec(), reference),
        DiagnosticArtifactBuildV3::TooLarge { .. } => {
            return Err(codestory_contracts::api::ApiError::internal(
                "Packet diagnostic projection exceeded its closed v3 bound.",
            ));
        }
    };
    let projection = build_packet_projection_v3(
        &record,
        DiagnosticsCapabilityV3Dto::Available { reference },
        &mut measure,
    )
    .map_err(|error| projection_error("packet projection", error))?;
    Ok(PacketEvidenceProductV3 {
        protected_source_witnesses: Vec::new(),
        source_issuance_seal: crate::agent::packet_served_sources::seal(caller_id, &projection),
        projection,
        diagnostics: PacketDiagnosticProjectionV3 {
            bytes,
            packet_id: record.packet_id().as_str().to_owned(),
            project_identity: record.core_publication().project_id.as_str().to_owned(),
            core_generation: record.core_publication().generation_id.as_str().to_owned(),
            core_run: record.core_publication().run_id.as_str().to_owned(),
            retrieval_generation: record
                .retrieval_publication()
                .map(|publication| publication.retrieval_generation.as_str().to_owned()),
            request_digest: record.request_sha256().as_str().to_owned(),
        },
    })
}

struct PacketEvidenceSelectionV3 {
    rows: Vec<PacketEvidenceRowV3Dto>,
    was_bounded: bool,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct PacketEvidenceContentKeyV3 {
    delivery: Option<codestory_contracts::compilation::PacketSourceDeliveryV1>,
    kind: u8,
    path: Option<String>,
    symbol_id: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    summary: Option<String>,
}

fn packet_evidence_selection(support: &[SupportUnitDto]) -> PacketEvidenceSelectionV3 {
    let source_symbols = support
        .iter()
        .filter(|unit| {
            unit.kind == SupportUnitKindDto::SourceRange
                && unit
                    .snippet
                    .as_deref()
                    .is_some_and(|snippet| !snippet.trim().is_empty())
        })
        .filter_map(|unit| Some((unit.path.as_deref()?, unit.symbol_id.as_deref()?)))
        .collect::<HashSet<_>>();
    let mut seen: HashMap<PacketEvidenceContentKeyV3, usize> = HashMap::new();
    let mut distinct: Vec<(SupportUnitKindDto, PacketEvidenceRowV3Dto)> = Vec::new();
    for unit in support
        .iter()
        .filter(|unit| unit.kind != SupportUnitKindDto::CompleteQueryNegative)
    {
        if unit.kind == SupportUnitKindDto::SymbolLocation
            && unit
                .path
                .as_deref()
                .zip(unit.symbol_id.as_deref())
                .is_some_and(|identity| source_symbols.contains(&identity))
        {
            continue;
        }
        let Some(row) = packet_evidence_row(0, unit) else {
            continue;
        };
        let content_key = packet_evidence_content_key(&row);
        if let std::collections::hash_map::Entry::Vacant(entry) = seen.entry(content_key) {
            entry.insert(distinct.len());
            distinct.push((unit.kind, row));
        }
    }

    let distinct_rows = distinct.len();
    let selected = select_packet_evidence_indices(&distinct);
    let mut rows = Vec::with_capacity(selected.len());
    for index in selected {
        let mut row = distinct[index].1.clone();
        row.identity = evidence_identity(&format!("packet-evidence-{:03}", rows.len()));
        rows.push(row);
    }

    PacketEvidenceSelectionV3 {
        rows,
        was_bounded: distinct_rows > PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3,
    }
}

fn select_packet_evidence_indices(
    distinct: &[(SupportUnitKindDto, PacketEvidenceRowV3Dto)],
) -> Vec<usize> {
    // The supplied rows already reflect retrieval and explicit structural traversal order. Public
    // projection may deduplicate and enforce kind/path diversity, but it must not reinterpret the
    // question or external policy to rerank repository evidence.
    let mut selected = Vec::with_capacity(distinct.len().min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3));
    let mut selected_indices = HashSet::new();
    let source_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::SourceRange);
    let location_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::SymbolLocation);
    let relation_order = packet_evidence_kind_order(distinct, SupportUnitKindDto::TypedGraphEdge);

    let mut source_paths = HashSet::new();
    let mut source_count = 0;
    for index in &source_order {
        if source_count >= PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3 {
            break;
        }
        let row = &distinct[*index].1;
        if row
            .path
            .as_ref()
            .is_some_and(|path| source_paths.insert(path.as_str()))
        {
            selected.push(*index);
            selected_indices.insert(*index);
            source_count += 1;
        }
    }

    for index in &source_order {
        if source_count >= PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3 {
            break;
        }
        if selected_indices.insert(*index) {
            selected.push(*index);
            source_count += 1;
        }
    }

    let location_target = selected
        .len()
        .saturating_add(PACKET_PUBLIC_LOCATION_ROWS_TARGET_V3)
        .min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
    select_packet_evidence_kind(
        &location_order,
        location_target,
        &mut selected,
        &mut selected_indices,
    );

    let relation_floor = selected
        .len()
        .saturating_add(PACKET_PUBLIC_RELATION_ROWS_TARGET_V3)
        .min(PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
    let relation_target = relation_floor;
    let mut relation_callers = HashSet::new();
    for index in &selected {
        if distinct[*index].0 != SupportUnitKindDto::TypedGraphEdge {
            continue;
        }
        let caller = distinct[*index]
            .1
            .summary
            .as_ref()
            .and_then(|summary| summary.as_str().split_once(" -[").map(|(caller, _)| caller));
        relation_callers.insert(caller);
    }
    for index in &relation_order {
        if selected.len() >= relation_target {
            break;
        }
        let caller = distinct[*index]
            .1
            .summary
            .as_ref()
            .and_then(|summary| summary.as_str().split_once(" -[").map(|(caller, _)| caller));
        if !selected_indices.contains(index) && relation_callers.insert(caller) {
            selected.push(*index);
            selected_indices.insert(*index);
        }
    }
    select_packet_evidence_kind(
        &relation_order,
        relation_target,
        &mut selected,
        &mut selected_indices,
    );

    let mut remainder = (0..distinct.len())
        .filter(|index| !selected_indices.contains(index))
        .collect::<Vec<_>>();
    remainder.sort_by_key(|index| (packet_evidence_kind_priority(distinct[*index].0), *index));
    for index in remainder {
        if selected.len() == PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            break;
        }
        selected.push(index);
        selected_indices.insert(index);
    }
    selected
}

fn packet_evidence_kind_priority(kind: SupportUnitKindDto) -> u8 {
    match kind {
        SupportUnitKindDto::SourceRange => 0,
        SupportUnitKindDto::TypedGraphEdge => 1,
        SupportUnitKindDto::SymbolLocation => 2,
        SupportUnitKindDto::CompleteQueryNegative => 3,
    }
}

fn select_packet_evidence_kind(
    ordered_indices: &[usize],
    maximum_total: usize,
    selected: &mut Vec<usize>,
    selected_indices: &mut HashSet<usize>,
) {
    for index in ordered_indices {
        if selected.len() == maximum_total {
            break;
        }
        if selected_indices.insert(*index) {
            selected.push(*index);
        }
    }
}

fn packet_evidence_kind_order(
    distinct: &[(SupportUnitKindDto, PacketEvidenceRowV3Dto)],
    selected_kind: SupportUnitKindDto,
) -> Vec<usize> {
    distinct
        .iter()
        .enumerate()
        .filter_map(|(index, (kind, _))| (*kind == selected_kind).then_some(index))
        .collect()
}

fn packet_evidence_content_key(row: &PacketEvidenceRowV3Dto) -> PacketEvidenceContentKeyV3 {
    PacketEvidenceContentKeyV3 {
        delivery: row.source_delivery.clone(),
        kind: match row.kind {
            EvidenceKindV3Dto::ExactSource => 0,
            EvidenceKindV3Dto::SourceLocation => 1,
            EvidenceKindV3Dto::StructuralSource => 2,
            EvidenceKindV3Dto::GraphRelation => 3,
            EvidenceKindV3Dto::RetrievalExcerpt => 4,
        },
        path: row.path.as_ref().map(|value| value.as_str().to_owned()),
        symbol_id: row
            .symbol_id
            .as_ref()
            .map(|value| value.as_str().to_owned()),
        start_line: row.start_line,
        end_line: row.end_line,
        summary: row.summary.as_ref().map(|value| value.as_str().to_owned()),
    }
}

#[cfg(test)]
fn packet_evidence_rows(support: &[SupportUnitDto]) -> Vec<PacketEvidenceRowV3Dto> {
    packet_evidence_selection(support).rows
}

#[cfg(test)]
fn packet_evidence_rows_for_request(
    support: &[SupportUnitDto],
    _question: &str,
    _option_ids: &[String],
) -> Vec<PacketEvidenceRowV3Dto> {
    packet_evidence_selection(support).rows
}

#[cfg(test)]
fn packet_evidence_was_bounded(support: &[SupportUnitDto]) -> bool {
    packet_evidence_selection(support).was_bounded
}

const PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3: usize = 8;
const PACKET_PUBLIC_LOCATION_ROWS_TARGET_V3: usize = 0;
const PACKET_PUBLIC_RELATION_ROWS_TARGET_V3: usize = 4;
const PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3: usize = 16;

/// Project a context answer without exposing answer confidence or claim state.
pub fn project_context_v3(
    service: &PublicOperationService,
    caller_id: &str,
    target_path: Option<&str>,
    target_symbol_id: Option<&str>,
    answer: &AgentAnswerDto,
) -> Result<ContextProjectionV3Dto, codestory_contracts::api::ApiError> {
    let (identity, publication, retrieval) = projection_envelope(
        service,
        caller_id,
        &answer.prompt,
        &answer.retrieval_trace.request_id,
        answer.retrieval_trace.retrieval_publication.as_ref(),
    )?;
    let mut evidence: Vec<_> = answer
        .citations
        .iter()
        .enumerate()
        .filter_map(|(index, citation)| {
            let path = citation.file_path.as_deref()?;
            let line = citation.line.filter(|line| *line > 0);
            Some(ContextEvidenceRowV3Dto {
                identity: evidence_identity(&format!("context-{index}-{}", citation.node_id.0)),
                path: path_text(path),
                symbol_id: citation
                    .resolvable
                    .then(|| symbol_text(Some(citation.node_id.0.as_str())))
                    .flatten(),
                start_line: line,
                end_line: line,
                excerpt: citation.source_excerpt.as_deref().map(bounded_excerpt),
            })
        })
        .take(codestory_contracts::packet_projection_v3::EVIDENCE_ROWS_MAX_V3)
        .collect();
    let mut gaps = answer_gap_rows(answer)?;
    if let Some(source) = &answer.focused_source {
        let line_count = source
            .end_line
            .checked_sub(source.start_line)
            .and_then(|count| count.checked_add(1));
        let bound = source.start_line > 0
            && line_count == u32::try_from(source.excerpt.split('\n').count()).ok()
            && target_symbol_id == Some(source.node_id.0.as_str())
            && service.focused_source_matches_current(source, target_path);
        if bound {
            let window = crate::snippets::cap_source_window(
                &source.excerpt,
                source.start_line,
                codestory_contracts::packet_projection_v3::EXCERPT_MAX_BYTES_V3,
            );
            let source_row = ContextEvidenceRowV3Dto {
                identity: evidence_identity(&format!("context-source-{}", source.node_id.0)),
                path: path_text(&source.path),
                symbol_id: symbol_text(Some(&source.node_id.0)),
                start_line: Some(window.start_line),
                end_line: Some(window.end_line),
                excerpt: Some(bounded_excerpt(&window.text)),
            };
            if let Some(row) = evidence.iter_mut().find(|row| {
                row.symbol_id == source_row.symbol_id
                    && row.path == source_row.path
                    && row.excerpt.is_none()
            }) {
                row.start_line = source_row.start_line;
                row.end_line = source_row.end_line;
                row.excerpt = source_row.excerpt;
            } else if evidence.len()
                < codestory_contracts::packet_projection_v3::EVIDENCE_ROWS_MAX_V3
            {
                evidence.push(source_row);
            } else {
                gaps.push(gap_row(
                    "context-focused-source-rows",
                    GapKindV3Dto::OutputBudgetExceeded,
                    Some("The focused source window did not fit the response row budget."),
                )?);
            }
            if source.truncated || window.truncated {
                gaps.push(gap_row(
                    "context-focused-source-bytes",
                    GapKindV3Dto::OutputBudgetExceeded,
                    Some("The focused source window was truncated to its byte budget."),
                )?);
            }
        } else {
            gaps.push(gap_row(
                "context-focused-source-binding",
                GapKindV3Dto::SourceUnavailable,
                Some("Focused source did not match the selected target and pinned publication."),
            )?);
        }
    } else if answer.retrieval_trace.steps.iter().any(|step| {
        step.kind == codestory_contracts::api::AgentRetrievalStepKindDto::SourceRead
            && step.status == codestory_contracts::api::AgentRetrievalStepStatusDto::Error
    }) {
        gaps.push(gap_row(
            "context-focused-source-read",
            GapKindV3Dto::SourceUnavailable,
            Some("Focused source could not be read from the pinned publication."),
        )?);
    }
    build_context_projection_v3(&FinalizedContextProjectionInputV3::new(
        identity,
        publication,
        retrieval,
        ContextTargetV3Dto {
            path: target_path.map(path_text),
            symbol_id: symbol_text(target_symbol_id),
        },
        evidence,
        gaps,
        None,
        DiagnosticsCapabilityV3Dto::Unavailable,
        Vec::new(),
    ))
    .map_err(|error| projection_error("context projection", error))
}

/// Project ranked search rows as evidence locations without exposing scores,
/// ranking rationale, or sufficiency fields.
pub fn project_search_v3(
    service: &PublicOperationService,
    caller_id: &str,
    results: &SearchResultsDto,
) -> Result<SearchProjectionV3Dto, codestory_contracts::api::ApiError> {
    let request_id = format!("search-{}", digest_hex(results.query.as_bytes()));
    let (identity, publication, mut retrieval) = projection_envelope(
        service,
        caller_id,
        &results.query,
        &request_id,
        results.retrieval_publication.as_ref(),
    )?;
    let symbolic = results.retrieval.mode == codestory_contracts::api::RetrievalModeDto::Symbolic;
    if symbolic {
        retrieval = RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Symbolic,
            generation_id: None,
        };
    }
    let SearchEvidenceSelectionV3 {
        rows: evidence,
        missing_source,
        was_bounded,
    } = search_evidence_selection(results);
    let mut gaps = Vec::new();
    if symbolic
        && results.repo_text_mode == codestory_contracts::api::SearchRepoTextMode::Off
        && !results.repo_text_enabled
        && results.hits.is_empty()
        && results.indexed_symbol_hits.is_empty()
        && results.repo_text_hits.is_empty()
        && evidence.is_empty()
        && let Some(message) = results.retrieval.fallback_message.as_deref()
    {
        gaps.push(gap_row(
            "search-core-name-no-match",
            GapKindV3Dto::EvidenceMissing,
            Some(message),
        )?);
    }
    if missing_source {
        gaps.push(gap_row(
            "search-source-unavailable",
            GapKindV3Dto::SourceUnavailable,
            Some("A search result had no pinned source path and could not be projected."),
        )?);
    }
    if was_bounded {
        gaps.push(gap_row(
            "search-projection-bounded",
            GapKindV3Dto::OutputBudgetExceeded,
            Some("Additional search rows were omitted from the bounded public projection."),
        )?);
    }
    if results.repo_text_enabled
        && results
            .repo_text_stats
            .as_ref()
            .is_some_and(|stats| stats.truncated || stats.skipped_large_file_count > 0)
    {
        gaps.push(gap_row(
            "search-repo-text-incomplete",
            GapKindV3Dto::EvidenceMissing,
            Some(
                "Repository-text search did not inspect all files within its scan budget. Additional matches may exist in unscanned files.",
            ),
        )?);
    }
    if !symbolic
        && results.retrieval_shadow.as_ref().is_some_and(|shadow| {
            shadow.retrieval_mode == "full"
                && (matches!(
                    shadow.cancel_reason.as_deref(),
                    Some("deadline" | "stage_deadline" | "cancelled")
                ) || shadow.stage_timings.iter().any(|stage| {
                    matches!(
                        stage.completion_status.as_str(),
                        "pending_after_deadline" | "cancelled_before_start"
                    )
                }))
        })
    {
        gaps.push(gap_row(
            "search-retrieval-interrupted",
            GapKindV3Dto::EvidenceMissing,
            Some("Retrieval reported a deadline or cancellation. Additional matches may exist."),
        )?);
    }
    if results.retrieval_publication.is_none() && !symbolic {
        gaps.push(gap_row(
            "search-retrieval-unavailable",
            GapKindV3Dto::RetrievalUnavailable,
            Some("No complete retrieval publication was available for this search."),
        )?);
    }
    build_search_projection_v3(&FinalizedSearchProjectionInputV3::new(
        identity,
        publication,
        retrieval,
        evidence,
        gaps,
        None,
        DiagnosticsCapabilityV3Dto::Unavailable,
        Vec::new(),
    ))
    .map_err(|error| projection_error("search projection", error))
}

struct SearchEvidenceSelectionV3 {
    rows: Vec<SearchEvidenceRowV3Dto>,
    missing_source: bool,
    was_bounded: bool,
}

fn search_evidence_selection(results: &SearchResultsDto) -> SearchEvidenceSelectionV3 {
    let row_limit = (results.limit_per_source as usize)
        .min(codestory_contracts::packet_projection_v3::EVIDENCE_ROWS_MAX_V3);
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut missing_source = false;
    let mut was_bounded = false;
    for hit in results.hits.iter().chain(&results.repo_text_hits) {
        // A derived report has no physical source to lose. Ordinary unlocated
        // source hits still produce the explicit source-unavailable gap.
        if crate::search_scoring::is_diagnostic_component_report_hit(hit) {
            continue;
        }
        let Some(path) = hit.file_path.as_deref().filter(|path| !path.is_empty()) else {
            missing_source = true;
            continue;
        };
        // The same location may be present in the merged and source-specific
        // result lists. Different lines in one file are separate evidence.
        if !seen.insert((hit.node_id.0.as_str(), path, hit.line)) {
            continue;
        }
        if rows.len() == row_limit {
            was_bounded = true;
            continue;
        }
        rows.push(search_evidence_row(rows.len(), hit).expect("search hit has a path"));
    }
    SearchEvidenceSelectionV3 {
        rows,
        missing_source,
        was_bounded,
    }
}

fn projection_envelope(
    service: &PublicOperationService,
    caller_id: &str,
    question: &str,
    request_id: &str,
    retrieval_publication: Option<&codestory_contracts::api::EmbeddingVectorPublicationIdentityDto>,
) -> Result<
    (
        codestory_contracts::packet_projection_v3::PacketRequestIdentityV3Dto,
        codestory_contracts::packet_projection_v3::PublicationIdentityV3Dto,
        RetrievalStateDescriptorV3Dto,
    ),
    codestory_contracts::api::ApiError,
> {
    let request = AgentPacketRequestDto {
        question: question.to_owned(),
        budget: Default::default(),
        probes: Vec::new(),
        latency_budget_ms: None,
        parent_packet_id: None,
        option_ids: Vec::new(),
        core_generation_id: None,
        retrieval_generation: None,
    };
    let retrieval = retrieval_state(retrieval_publication);
    let input = FinalizedPacketExecutionInputV3::new(
        identity(caller_id, "caller_id")?,
        identity(request_id, "request_id")?,
        PacketRequestFingerprintV3::from_current_request(&request),
        Vec::new(),
        Vec::new(),
        None,
        retrieval.clone(),
        Vec::new(),
    );
    let record = build_packet_execution_record_v3(service, &input)
        .map_err(|error| projection_error("projection envelope", error))?;
    let packet =
        build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |_| Ok(0))
            .map_err(|error| projection_error("projection envelope", error))?;
    let (identity, publication) = match packet {
        PacketProjectionV3Dto::Complete {
            identity,
            publication,
            ..
        }
        | PacketProjectionV3Dto::BudgetExceeded {
            identity,
            publication,
            ..
        } => (identity, publication),
    };
    Ok((identity, publication, retrieval))
}

fn validate_packet_source_page(
    unit: &SupportUnitDto,
) -> Result<(), codestory_contracts::api::ApiError> {
    let Some(delivery) = &unit.source_delivery else {
        return Ok(());
    };
    let source_fits = unit.kind == SupportUnitKindDto::SourceRange
        && unit.snippet.as_deref().is_some_and(|source| {
            !source.trim().is_empty()
                && source.len()
                    <= codestory_contracts::compilation::INTERIM_SOURCE_CONTINUATION_UPPER_BOUND
        })
        && unit
            .start_line
            .zip(unit.end_line)
            .is_some_and(|(start, end)| delivery.matches_delivery(start, end));
    if !source_fits {
        return Err(codestory_contracts::api::ApiError::internal(
            "Packet source page and delivery receipt cannot be represented without loss.",
        ));
    }
    Ok(())
}

fn packet_evidence_row(index: usize, unit: &SupportUnitDto) -> Option<PacketEvidenceRowV3Dto> {
    validate_packet_source_page(unit).ok()?;
    if unit.kind == SupportUnitKindDto::CompleteQueryNegative {
        return None;
    }
    if unit.kind == SupportUnitKindDto::SymbolLocation && unit.path.is_none() {
        return None;
    }
    Some(PacketEvidenceRowV3Dto {
        packet_request_patch: None,
        source_delivery: unit.source_delivery.clone(),
        // The execution record canonicalizes by identity. A fixed-width rank
        // prefix keeps that canonical order equal to the projection's useful-
        // context order while avoiding repository-shaped identifiers.
        identity: evidence_identity(&format!("packet-evidence-{index:03}")),
        kind: match unit.kind {
            SupportUnitKindDto::SymbolLocation => EvidenceKindV3Dto::SourceLocation,
            SupportUnitKindDto::SourceRange => EvidenceKindV3Dto::ExactSource,
            SupportUnitKindDto::TypedGraphEdge => EvidenceKindV3Dto::GraphRelation,
            SupportUnitKindDto::CompleteQueryNegative => return None,
        },
        path: unit.path.as_deref().map(path_text),
        symbol_id: symbol_text(unit.symbol_id.as_deref()),
        start_line: (unit.kind != SupportUnitKindDto::SymbolLocation)
            .then_some(unit.start_line)
            .flatten(),
        end_line: (unit.kind != SupportUnitKindDto::SymbolLocation)
            .then_some(unit.end_line)
            .flatten(),
        summary: packet_evidence_summary(unit),
    })
}

const PACKET_EVIDENCE_SUMMARY_MAX_BYTES_V3: usize = 512;
fn packet_evidence_summary(unit: &SupportUnitDto) -> Option<SummaryTextV3> {
    let text = match unit.kind {
        SupportUnitKindDto::SourceRange => unit
            .snippet
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|text| {
                unit.source_delivery
                    .as_ref()
                    .and_then(|_| {
                        compact_numbered_source_page(text, unit.start_line?, unit.end_line?)
                    })
                    .unwrap_or_else(|| text.to_owned())
            })
            .unwrap_or_else(|| unit.summary.clone()),
        SupportUnitKindDto::TypedGraphEdge => {
            match (
                unit.from_symbol.as_deref(),
                unit.edge_kind.as_deref(),
                unit.to_symbol.as_deref(),
            ) {
                (Some(from), Some(edge), Some(to)) => format!("{from} -[{edge}]-> {to}"),
                _ => unit.summary.clone(),
            }
        }
        SupportUnitKindDto::SymbolLocation => packet_symbol_location_summary(unit),
        SupportUnitKindDto::CompleteQueryNegative => return None,
    };
    if unit.kind == SupportUnitKindDto::SourceRange && unit.source_delivery.is_some() {
        // The receipt describes the complete retained page, not a summary.
        // The response measurement owns whole-row omission; never clip source
        // text while carrying its original delivery range and cursor.
        return SummaryTextV3::new(text).ok();
    }
    Some(summary_text_bounded(
        &text,
        PACKET_EVIDENCE_SUMMARY_MAX_BYTES_V3,
    ))
}

/// Remove display alignment only. Every line number, focus marker, physical
/// source byte after the delimiter, and the page fence remain intact.
pub(crate) fn compact_numbered_source_page(text: &str, start: u32, end: u32) -> Option<String> {
    let body = text.strip_prefix("```text\n")?.strip_suffix("\n```")?;
    let mut compact = String::from("```text\n");
    let mut count = 0u32;
    for line in body.split('\n') {
        let (prefix, source) = line.split_once(" | ")?;
        let marker = prefix.get(..1)?;
        if marker != ">" && marker != " " {
            return None;
        }
        let number = prefix.get(1..)?.trim();
        let actual = number.parse::<u32>().ok()?;
        if actual != start.checked_add(count)? || actual > end || number != actual.to_string() {
            return None;
        }
        compact.push_str(marker);
        compact.push_str(number);
        compact.push_str(" | ");
        compact.push_str(source);
        compact.push('\n');
        count = count.checked_add(1)?;
    }
    if start.checked_add(count.checked_sub(1)?)? != end {
        return None;
    }
    compact.push_str("```");
    Some(compact)
}

fn packet_symbol_location_summary(unit: &SupportUnitDto) -> String {
    let Some((label, _)) = unit.summary.rsplit_once(" at ") else {
        return unit.summary.clone();
    };
    let label_is_absolute = label.starts_with('/')
        || label
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if !label_is_absolute {
        return unit.summary.clone();
    }
    match (unit.path.as_deref(), unit.start_line) {
        (Some(path), Some(line)) => format!("{path}:{line}"),
        (Some(path), None) => path.to_owned(),
        _ => "project source location".to_owned(),
    }
}

fn continuation_source_message(
    option: &codestory_contracts::api::DrillOptionDto,
) -> Option<String> {
    let label = option.source_label.as_ref()?;
    let mut text = label.path.clone().unwrap_or_default();
    if let Some(range) = option.source_range {
        if text.is_empty() {
            text = format!("lines:{}-{}", range.start_line, range.end_line);
        } else {
            text.push_str(&format!(":{}-{}", range.start_line, range.end_line));
        }
    }
    if let Some(symbol) = &label.symbol {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(symbol);
    }
    (!text.is_empty()).then_some(text)
}

fn packet_gaps(packet: &AgentPacketDto) -> Vec<ProjectionGapRowV3Dto> {
    let mut gaps = packet
        .support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::CompleteQueryNegative)
        .map(|unit| {
            gap_row(
                &format!("negative-not-authority-{}", unit.id),
                GapKindV3Dto::EvidenceMissing,
                Some("A bounded negative query was retained as a gap, not proof of absence."),
            )
            .expect("static bounded gap")
        })
        .collect::<Vec<_>>();
    for (index, receipt) in packet.disposition.omission_receipts.iter().enumerate() {
        gaps.push(
            gap_row(
                &format!("packet-source-refusal-{index}"),
                GapKindV3Dto::SourceUnavailable,
                Some(receipt),
            )
            .expect("bounded admitted source refusal"),
        );
    }
    for (index, unit) in packet.support.iter().enumerate() {
        if let Some(delivery) = &unit.source_delivery
            && delivery.remaining_source_bytes > 0
        {
            let blocked = delivery.blocked_line_rendered_bytes.map_or_else(String::new, |bytes| format!("; next complete line requires {bytes} rendered bytes, exceeding the reserved source-page cap"));
            gaps.push(gap_row(
                &format!("packet-source-unread-{index}"),
                GapKindV3Dto::EvidenceMissing,
                Some(&format!("Requested lines {}-{}; complete lines {}-{} delivered; {} source bytes remain{}.",
                    delivery.requested_range.start_line, delivery.requested_range.end_line,
                    unit.start_line.unwrap_or_default(), unit.end_line.unwrap_or_default(),
                    delivery.remaining_source_bytes, blocked)),
            ).expect("bounded source delivery gap"));
        }
    }
    match packet.disposition.kind {
        PacketDispositionKindDto::Supported => {}
        PacketDispositionKindDto::DrillOnce => {
            if let Some(drill) = &packet.disposition.drill {
                gaps.extend(drill.options.iter().map(|option| {
                    gap_row(
                        &option.id,
                        GapKindV3Dto::ContinuationRequired,
                        continuation_source_message(option)
                            .as_deref()
                            .or(packet.disposition.reason.as_deref()),
                    )
                    .expect("bounded disposition gap")
                }));
            }
        }
        PacketDispositionKindDto::NotEstablished => gaps.push(
            gap_row(
                "packet-evidence-not-established",
                GapKindV3Dto::EvidenceMissing,
                packet.disposition.reason.as_deref(),
            )
            .expect("bounded disposition gap"),
        ),
        PacketDispositionKindDto::Unavailable => gaps.push(
            gap_row(
                "packet-evidence-unavailable",
                GapKindV3Dto::RetrievalUnavailable,
                packet.disposition.reason.as_deref(),
            )
            .expect("bounded disposition gap"),
        ),
    }
    gaps
}

fn packet_continuation(
    packet: &AgentPacketDto,
    gaps: &mut Vec<ProjectionGapRowV3Dto>,
) -> Result<Option<ContinuationStateV3Dto>, codestory_contracts::api::ApiError> {
    let Some(drill) = packet.disposition.drill.as_ref() else {
        return Ok(None);
    };
    if drill.remaining_rounds == 0 {
        return Ok(None);
    }
    if drill.gap_ids.is_empty() {
        gaps.push(gap_row(
            "packet-continuation-required",
            GapKindV3Dto::ContinuationRequired,
            packet.disposition.reason.as_deref(),
        )?);
    }
    canonicalize_gaps(gaps);
    let references = if drill.gap_ids.is_empty() {
        gaps.iter()
            .filter(|gap| gap.kind == GapKindV3Dto::ContinuationRequired)
            .map(|gap| gap.identity.clone())
            .take(codestory_contracts::packet_projection_v3::REFERENCE_ROWS_MAX_V3)
            .collect()
    } else {
        // Structural gap IDs carry compiler priority; public gap identities
        // carry executable option IDs. Resolve through the offered options
        // without borrowing canonical storage order as selection priority.
        drill
            .gap_ids
            .iter()
            .filter_map(|gap_id| drill.options.iter().find(|option| &option.gap_id == gap_id))
            .filter_map(|option| {
                gaps.iter().find(|gap| {
                    gap.kind == GapKindV3Dto::ContinuationRequired
                        && gap.identity.gap_id.as_str() == option.id
                })
            })
            .map(|gap| gap.identity.clone())
            .take(codestory_contracts::packet_projection_v3::REFERENCE_ROWS_MAX_V3)
            .collect()
    };
    let mut continuation = ContinuationStateV3Dto::new(
        identity(&drill.parent_packet_id, "continuation_id")?,
        u16::try_from(drill.remaining_rounds).unwrap_or(u16::MAX),
        BoundedVecV3::new(references).expect("bounded continuation references"),
    )
    .map_err(|error| projection_error("packet continuation", error))?;
    if drill.options.is_empty() || drill.core_generation_id.is_empty() {
        return Ok(Some(continuation));
    }
    let patch = codestory_contracts::packet_projection_v3::PacketRequestPatchV3Dto {
        probes: BoundedVecV3::new(Vec::new()).expect("empty probes"),
        parent_packet_id: Some(identity(&drill.parent_packet_id, "parent_packet_id")?),
        option_ids: BoundedVecV3::new(
            continuation
                .gap_ids
                .as_slice()
                .iter()
                .map(|gap| gap.gap_id.clone())
                .collect(),
        )
        .map_err(|error| projection_error("packet options", error))?,
        core_generation_id: identity(&drill.core_generation_id, "core_generation_id")?,
        retrieval_generation: drill
            .retrieval_generation
            .as_deref()
            .map(|id| identity(id, "retrieval_generation"))
            .transpose()?,
    };
    patch
        .validate()
        .map_err(|error| projection_error("packet request patch", error))?;
    continuation.packet_request_patch = Some(patch);
    Ok(Some(continuation))
}

fn attach_source_page_actions(
    project_id: &str,
    packet: &AgentPacketDto,
    rows: &mut [PacketEvidenceRowV3Dto],
) -> Result<(), codestory_contracts::api::ApiError> {
    use codestory_contracts::{
        api::PacketProbeDto,
        compilation::{
            PacketContinuationSelectorV1, PacketSourceRangeV1, PacketStructuralGapReasonV1,
        },
        packet_projection_v3::PacketRequestPatchV3Dto,
    };
    let Some(publication) = packet.answer.retrieval_trace.retrieval_publication.as_ref() else {
        return Ok(());
    };
    for row in rows {
        let (Some(delivery), Some(symbol_id)) = (&row.source_delivery, &row.symbol_id) else {
            continue;
        };
        let Some(next) = delivery.next_start_line else {
            continue;
        };
        let probe = PacketProbeDto::Continuation {
            contract_version: codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION,
            project_id: project_id.to_owned(),
            core_generation_id: publication.core_generation_id.clone(),
            retrieval_generation: Some(publication.retrieval_generation.clone()),
            selector: PacketContinuationSelectorV1 {
                stable_identity: format!("node:{}", symbol_id.as_str()),
                symbol_id: Some(symbol_id.as_str().to_owned()),
                // Stable node identity authenticates the file independently.
                // Public paths can exceed the probe's smaller text limit.
                path: None,
                source_range: Some(PacketSourceRangeV1 {
                    start_line: next,
                    end_line: delivery.requested_range.end_line,
                }),
                reason: PacketStructuralGapReasonV1::SourceBudgetExceeded,
            },
        };
        let mut patch = PacketRequestPatchV3Dto {
            probes: BoundedVecV3::new(vec![probe]).expect("one source probe"),
            parent_packet_id: None,
            option_ids: BoundedVecV3::new(Vec::new()).expect("empty options"),
            core_generation_id: identity(&publication.core_generation_id, "core_generation_id")?,
            retrieval_generation: Some(identity(
                &publication.retrieval_generation,
                "retrieval_generation",
            )?),
        };
        // The existing numeric range reference dispatches the same pinned
        // typed source probe without repeating its publication envelope.
        // Other node spellings retain the original typed action.
        let option = codestory_contracts::api::DrillOptionDto::omitted_symbol(
            "source-page",
            symbol_id.as_str(),
        )
        .with_source_range(PacketSourceRangeV1 {
            start_line: next,
            end_line: delivery.requested_range.end_line,
        });
        if option.id.starts_with("ms1:") {
            patch.probes = BoundedVecV3::new(Vec::new()).expect("empty probes");
            patch.parent_packet_id = Some(identity(&packet.packet_id, "parent_packet_id")?);
            patch.option_ids = BoundedVecV3::new(vec![identity(&option.id, "source_option_id")?])
                .expect("one source option");
        }
        patch
            .validate()
            .map_err(|error| projection_error("source page action", error))?;
        row.packet_request_patch = Some(patch);
    }
    Ok(())
}

fn diagnostic_rows(
    answer: &AgentAnswerDto,
    gaps: &[ProjectionGapRowV3Dto],
) -> Result<Vec<FinalizedDiagnosticSourceRowV3>, codestory_contracts::api::ApiError> {
    let mut rows = Vec::new();
    for (index, annotation) in answer.retrieval_trace.annotations.iter().enumerate() {
        let gap_ids = if annotation.is_gap() {
            gaps.iter().map(|gap| gap.identity.clone()).collect()
        } else {
            Vec::new()
        };
        rows.push(FinalizedDiagnosticSourceRowV3::new(
            identity(&format!("diagnostic-{index}"), "diagnostic_id")?,
            DiagnosticCategoryV3Dto::Retrieval,
            DiagnosticCodeTextV3::new(if annotation.is_gap() {
                "retrieval_gap"
            } else {
                "retrieval_observation"
            })
            .expect("static diagnostic code"),
            Vec::new(),
            gap_ids,
        ));
    }
    Ok(rows)
}

fn answer_gap_rows(
    answer: &AgentAnswerDto,
) -> Result<Vec<ProjectionGapRowV3Dto>, codestory_contracts::api::ApiError> {
    answer
        .retrieval_trace
        .annotations
        .iter()
        .enumerate()
        .filter(|(_, annotation)| annotation.is_gap())
        .map(|(index, _)| {
            gap_row(
                &format!("context-gap-{index}"),
                GapKindV3Dto::EvidenceMissing,
                Some("Context retrieval reported an evidence gap; details are diagnostic-only."),
            )
        })
        .collect()
}

fn search_evidence_row(index: usize, hit: &SearchHit) -> Option<SearchEvidenceRowV3Dto> {
    let path = hit.file_path.as_deref().filter(|path| !path.is_empty())?;
    Some(SearchEvidenceRowV3Dto {
        identity: evidence_identity(&format!("search-{index}-{}", hit.node_id.0)),
        path: path_text(path),
        symbol_id: hit.resolvable.then(|| symbol_id_text(&hit.node_id.0)),
        display_name: (hit.resolvable && !hit.display_name.trim().is_empty())
            .then(|| symbol_id_text(&hit.display_name)),
        start_line: hit.line,
        end_line: hit.line,
        excerpt: hit.source_excerpt.as_deref().map(bounded_excerpt),
    })
}

fn retrieval_state(
    publication: Option<&codestory_contracts::api::EmbeddingVectorPublicationIdentityDto>,
) -> RetrievalStateDescriptorV3Dto {
    match publication {
        Some(publication) => RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Full,
            generation_id: Some(identity_text(&publication.retrieval_generation)),
        },
        None => RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Degraded,
            generation_id: None,
        },
    }
}

fn canonicalize_gaps(gaps: &mut Vec<ProjectionGapRowV3Dto>) {
    gaps.sort_by(|left, right| left.identity.cmp(&right.identity));
    gaps.dedup_by(|left, right| left.identity == right.identity);
    gaps.truncate(codestory_contracts::packet_projection_v3::GAP_ROWS_MAX_V3);
}

fn gap_row(
    id: &str,
    kind: GapKindV3Dto,
    message: Option<&str>,
) -> Result<ProjectionGapRowV3Dto, codestory_contracts::api::ApiError> {
    Ok(ProjectionGapRowV3Dto {
        identity: GapIdentityV3Dto {
            gap_id: identity(id, "gap_id")?,
        },
        kind,
        message: message.map(|value| {
            codestory_contracts::packet_projection_v3::MessageTextV3::new(truncate_utf8(
                value,
                codestory_contracts::packet_projection_v3::MESSAGE_MAX_BYTES_V3,
            ))
            .expect("truncated gap message is bounded")
        }),
    })
}

fn evidence_identity(value: &str) -> EvidenceIdentityV3Dto {
    EvidenceIdentityV3Dto {
        evidence_id: identity_text(value),
    }
}

fn identity(
    value: &str,
    field: &str,
) -> Result<IdentityTextV3, codestory_contracts::api::ApiError> {
    if value.trim().is_empty() {
        return Err(codestory_contracts::api::ApiError::internal(format!(
            "The finalized v3 {field} was empty."
        )));
    }
    Ok(identity_text(value))
}

fn identity_text(value: &str) -> IdentityTextV3 {
    let bounded = if value.len() <= codestory_contracts::packet_projection_v3::IDENTITY_MAX_BYTES_V3
    {
        value.to_owned()
    } else {
        format!("sha256-{}", digest_hex(value.as_bytes()))
    };
    IdentityTextV3::new(bounded).expect("bounded identity")
}

fn path_text(value: &str) -> PathTextV3 {
    PathTextV3::new(truncate_utf8(
        value,
        codestory_contracts::packet_projection_v3::PATH_MAX_BYTES_V3,
    ))
    .expect("truncated path is bounded")
}

fn symbol_text(value: Option<&str>) -> Option<SymbolIdTextV3> {
    value.map(symbol_id_text)
}

fn symbol_id_text(value: &str) -> SymbolIdTextV3 {
    SymbolIdTextV3::new(truncate_utf8(
        value,
        codestory_contracts::packet_projection_v3::SYMBOL_ID_MAX_BYTES_V3,
    ))
    .expect("truncated symbol id is bounded")
}

fn summary_text_bounded(value: &str, maximum: usize) -> SummaryTextV3 {
    SummaryTextV3::new(truncate_utf8(value, maximum)).expect("truncated summary is bounded")
}

fn bounded_excerpt(value: &str) -> codestory_contracts::packet_projection_v3::ExcerptTextV3 {
    codestory_contracts::packet_projection_v3::ExcerptTextV3::new(truncate_utf8(
        value,
        codestory_contracts::packet_projection_v3::EXCERPT_MAX_BYTES_V3,
    ))
    .expect("truncated excerpt is bounded")
}

fn truncate_utf8(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn digest_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn projection_error(
    stage: &str,
    error: impl std::fmt::Debug,
) -> codestory_contracts::api::ApiError {
    codestory_contracts::api::ApiError::internal(format!(
        "Failed to build the closed v3 {stage}: {error:?}"
    ))
}

#[allow(dead_code)]
fn _sha_type_is_public(_: Sha256DigestV3Dto) {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, atomic::AtomicBool};

    use codestory_contracts::api::{IndexMode, SearchRepoTextMode, SearchRequest};
    use serde_json::json;

    use super::*;

    fn support_unit(kind: SupportUnitKindDto) -> SupportUnitDto {
        SupportUnitDto {
            source_delivery: None,
            id: "unit-1".to_string(),
            kind,
            summary: "fixture".to_string(),
            path: None,
            symbol_id: None,
            start_line: None,
            end_line: None,
            snippet: None,
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        }
    }

    fn continuation_progress_packet() -> AgentPacketDto {
        let trace = json!({"request_id":"continuation-request", "resolved_profile":"investigate",
            "policy_mode":"latency_first", "total_latency_ms":0, "sla_missed":false,
            "semantic_fallback_count":0, "semantic_fallbacks":[], "semantic_stage_timeout_zero_hits":0,
            "semantic_abstained_count":0, "annotations":[], "steps":[], "packet_sidecar_diagnostics":[]});
        serde_json::from_value(json!({
            "packet_id":"continuation-request", "question":"repository investigation",
            "plan":{"queries":[],"trace":[]},
            "answer":{"answer_id":"continuation-answer", "prompt":"repository investigation",
                "summary":"fixture", "sections":[], "citations":[], "subgraph_ids":[],
                "retrieval_version":"fixture", "graphs":[], "retrieval_trace":trace},
            "support":[], "disposition":{"kind":"not_established"},
            "budget":{"requested":"standard", "limits":{"max_anchors":16,"max_files":16,
                "max_snippets":16,"max_trail_edges":16,"max_output_bytes":65536},
                "used":{"anchors":0,"files":0,"snippets":0,"trail_edges":0,"output_bytes":0},
                "truncated":false,"next_deeper_command":null},
            "retrieval_trace_summary":{"retrieval_trace":trace,"source_read_steps":0,
                "search_steps":0,"trail_steps":0}
        }))
        .expect("internal packet envelope")
    }

    #[test]
    fn joint_served_guard_requires_delivered_gain_and_resolution() {
        use codestory_contracts::api::{
            DrillOptionDto, DrillSourceLabelDto, EmbeddingVectorPublicationIdentityDto,
        };
        use codestory_contracts::compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1};
        let make_source = |id: &str| {
            let mut source = support_unit(SupportUnitKindDto::SourceRange);
            source.path = Some(format!("src/{id}.rs"));
            source.symbol_id = Some(id.into());
            source.start_line = Some(1);
            source.end_line = Some(1);
            source.snippet = Some(format!("```text\n>    1 | fn {id}() {{}}\n```"));
            source.source_delivery = Some(PacketSourceDeliveryV1 {
                requested_range: PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 1,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 0,
                next_start_line: None,
                blocked_line_rendered_bytes: None,
            });
            source
        };
        let old_source = make_source("a");
        let incoming = make_source("b");
        let mut packet = continuation_progress_packet();
        packet.support = vec![old_source];
        packet.answer.retrieval_trace.retrieval_publication =
            Some(EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-generation-1".into(),
                core_run_id: "core-run-1".into(),
                retrieval_generation: "retrieval-generation-1".into(),
                retrieval_input_hash: "a".repeat(64),
                semantic_generation: "semantic-generation-1".into(),
            });
        let request: AgentPacketRequestDto =
            serde_json::from_value(json!({"question":"inspect"})).unwrap();
        let mut baseline = project_packet_v3_using_record_builder(
            "project-1",
            "test",
            &request,
            &packet,
            |_| Ok(0),
            |input| {
                crate::agent::packet_execution_record_v3::build_packet_execution_record_fixture_v3(
                    input, true,
                )
            },
        )
        .unwrap()
        .projection;
        // Failed renderer attempts also consume the finite transport bound;
        // exhaustion must refuse before invoking the caller again.
        let invocations = std::cell::Cell::new(0usize);
        let mut meter = bounded_packet_transport_measure_v3(|_| {
            invocations.set(invocations.get() + 1);
            Err(())
        });
        for _ in 0..PACKET_TRANSPORT_MEASUREMENTS_MAX_V1 {
            assert!(meter(&baseline).is_err());
        }
        assert_eq!(invocations.get(), PACKET_TRANSPORT_MEASUREMENTS_MAX_V1);
        assert!(meter(&baseline).is_err());
        assert_eq!(invocations.get(), PACKET_TRANSPORT_MEASUREMENTS_MAX_V1);
        let mut option = DrillOptionDto::omitted_symbol("source_budget_exceeded:node:b", "b")
            .with_source_range(PacketSourceRangeV1 {
                start_line: 1,
                end_line: 1,
            });
        option.source_label = Some(DrillSourceLabelDto {
            path: Some("src/b.rs".into()),
            symbol: Some("b".into()),
        });
        packet.disposition = codestory_contracts::api::PacketDispositionDto::drill_once(
            "bounded",
            continuation_progress_plan(vec![option.clone()]),
        );
        let mut labels = packet_gaps(&packet);
        let action = packet_continuation(&packet, &mut labels).unwrap();
        let PacketProjectionV3Dto::Complete {
            gaps, continuation, ..
        } = &mut baseline
        else {
            panic!("baseline complete")
        };
        *gaps = BoundedVecV3::new(labels).unwrap();
        *continuation = action;
        let none = std::collections::BTreeMap::new();
        assert!(packet_projection_preserves_baseline_v3(
            &baseline,
            &baseline,
            &none,
            &[]
        ));
        // Identical baseline remains valid even with a truthful non-executable gap.
        let mut no_action = baseline.clone();
        if let PacketProjectionV3Dto::Complete { continuation, .. } = &mut no_action {
            *continuation = None;
        }
        assert!(packet_projection_preserves_baseline_v3(
            &no_action,
            &no_action,
            &none,
            &[]
        ));
        let witnesses = std::collections::BTreeMap::from([(option.id.clone(), incoming.clone())]);
        let mut lost = baseline.clone();
        if let PacketProjectionV3Dto::Complete {
            gaps, continuation, ..
        } = &mut lost
        {
            *gaps = BoundedVecV3::new(vec![]).unwrap();
            *continuation = None;
        }
        // Internal resolution without delivered source must keep the old action.
        assert!(!packet_projection_preserves_baseline_v3(
            &baseline,
            &lost,
            &witnesses,
            &[]
        ));
        // Internal gain without delivered source earns no admission, even if gaps remain.
        assert!(!packet_projection_preserves_baseline_v3(
            &baseline,
            &baseline,
            &none,
            std::slice::from_ref(&incoming)
        ));
        let mut served = lost.clone();
        if let PacketProjectionV3Dto::Complete { evidence, .. } = &mut served {
            let mut rows = evidence.as_slice().to_vec();
            rows.push(packet_evidence_row(1, &incoming).unwrap());
            *evidence = BoundedVecV3::new(rows).unwrap();
        }
        assert!(packet_projection_preserves_baseline_v3(
            &baseline,
            &served,
            &witnesses,
            std::slice::from_ref(&incoming)
        ));
        for hostile in [
            "source-text",
            "digest",
            "baseline-text",
            "label",
            "authority",
        ] {
            let mut bad = if hostile == "label" || hostile == "authority" {
                baseline.clone()
            } else {
                served.clone()
            };
            if let PacketProjectionV3Dto::Complete {
                evidence,
                gaps,
                continuation,
                ..
            } = &mut bad
            {
                if hostile == "label" {
                    let mut rows = gaps.as_slice().to_vec();
                    rows[0].message = Some(
                        codestory_contracts::packet_projection_v3::MessageTextV3::new("changed")
                            .unwrap(),
                    );
                    *gaps = BoundedVecV3::new(rows).unwrap();
                } else if hostile == "authority" {
                    continuation
                        .as_mut()
                        .unwrap()
                        .packet_request_patch
                        .as_mut()
                        .unwrap()
                        .core_generation_id = identity_text("other");
                } else {
                    let mut rows = evidence.as_slice().to_vec();
                    let index = if hostile == "baseline-text" { 0 } else { 1 };
                    if hostile == "digest" {
                        rows[index].source_delivery.as_mut().unwrap().source_sha256 =
                            "b".repeat(64);
                    } else {
                        rows[index].summary = Some(SummaryTextV3::new("changed").unwrap());
                    }
                    *evidence = BoundedVecV3::new(rows).unwrap();
                }
            }
            assert!(
                !packet_projection_preserves_baseline_v3(
                    &baseline,
                    &bad,
                    &witnesses,
                    if hostile == "label" || hostile == "authority" {
                        &[]
                    } else {
                        std::slice::from_ref(&incoming)
                    }
                ),
                "{hostile}"
            );
        }
    }

    fn continuation_progress_plan(
        options: Vec<codestory_contracts::api::DrillOptionDto>,
    ) -> codestory_contracts::api::BoundedDrillPlanDto {
        use codestory_contracts::api::*;
        BoundedDrillPlanDto {
            parent_packet_id: "continuation-request".into(),
            core_generation_id: "core".into(),
            retrieval_generation: Some("retrieval".into()),
            gap_ids: options.iter().map(|option| option.gap_id.clone()).collect(),
            options,
            max_bytes: PACKET_DRILL_MAX_BYTES,
            max_hits: PACKET_DRILL_MAX_HITS,
            max_depth: PACKET_DRILL_MAX_DEPTH,
            remaining_rounds: 1,
        }
    }

    #[test]
    fn continuation_source_labels_only_change_gap_messages_and_fit_mirrored_serialization() {
        use codestory_contracts::{
            api::{DrillOptionDto, DrillSourceLabelDto, PacketDispositionDto},
            compilation::PacketSourceRangeV1,
        };
        let mut packet = continuation_progress_packet();
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.path = Some("src/worker.rs".into());
        source.symbol_id = Some("7".into());
        source.start_line = Some(12);
        source.end_line = Some(12);
        source.snippet = Some("fn run() {}".into());
        packet.support = vec![source];
        let option = DrillOptionDto::omitted_symbol("source_budget_exceeded:node:7", "7")
            .with_source_range(PacketSourceRangeV1 {
                start_line: 12,
                end_line: 20,
            });
        packet.disposition = PacketDispositionDto::drill_once(
            "bounded structural continuation available",
            continuation_progress_plan(vec![option]),
        );
        let mut baseline_gaps = packet_gaps(&packet);
        let baseline_patch = packet_continuation(&packet, &mut baseline_gaps).unwrap();
        packet.disposition.drill.as_mut().unwrap().options[0].source_label =
            Some(DrillSourceLabelDto {
                path: Some("src/worker.rs".into()),
                symbol: Some("Worker.run".into()),
            });
        let mut labeled_gaps = packet_gaps(&packet);
        assert_eq!(
            labeled_gaps[0].message.as_ref().unwrap().as_str(),
            "src/worker.rs:12-20 Worker.run"
        );
        assert_eq!(
            packet_continuation(&packet, &mut labeled_gaps).unwrap(),
            baseline_patch
        );
        labeled_gaps[0].message = baseline_gaps[0].message.clone();
        assert_eq!(labeled_gaps, baseline_gaps);
        packet.answer.retrieval_trace.retrieval_publication = Some(
            codestory_contracts::api::EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core".into(),
                core_run_id: "core-run".into(),
                retrieval_generation: "retrieval".into(),
                retrieval_input_hash: "a".repeat(64),
                semantic_generation: "semantic".into(),
            },
        );
        let retrieval_publication = packet
            .answer
            .retrieval_trace
            .retrieval_publication
            .clone()
            .unwrap();
        let compilation_publication =
            codestory_contracts::compilation::PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: retrieval_publication.core_generation_id.clone(),
                retrieval_generation: Some(retrieval_publication.retrieval_generation.clone()),
            };
        let request: AgentPacketRequestDto =
            serde_json::from_value(json!({"question":"inspect"})).unwrap();
        let mirror_size = |projection: &PacketProjectionV3Dto| -> Result<usize, ()> {
            Ok(serde_json::to_vec(&json!({
                "content":[{"type":"text","text":serde_json::to_string(projection).unwrap()}],
                "structuredContent":projection, "isError":false
            }))
            .unwrap()
            .len())
        };
        for path in ["src/worker.rs".to_owned(), "é\\\"".repeat(2500)] {
            let short_label = path == "src/worker.rs";
            packet.disposition.drill.as_mut().unwrap().options[0]
                .source_label
                .as_mut()
                .unwrap()
                .path = Some(path);
            let product = project_packet_v3_using_record_builder("project-1", "test", &request,
                &packet, mirror_size,
                |input| crate::agent::packet_execution_record_v3::build_packet_execution_record_replay_v3(
                    input, &compilation_publication, &retrieval_publication.core_run_id,
                    Some(retrieval_publication.clone()), "00000000-0000-4000-8000-000000000001")).unwrap();
            assert!(
                mirror_size(&product.projection).unwrap()
                    <= codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES
            );
            if short_label {
                let PacketProjectionV3Dto::Complete {
                    gaps,
                    evidence,
                    continuation,
                    ..
                } = &product.projection
                else {
                    panic!("short label fits with existing evidence");
                };
                assert_eq!(
                    gaps.as_slice()[0].message.as_ref().unwrap().as_str(),
                    "src/worker.rs:12-20 Worker.run"
                );
                assert_eq!(evidence.as_slice().len(), 1);
                assert_eq!(
                    evidence.as_slice()[0].path.as_ref().unwrap().as_str(),
                    "src/worker.rs"
                );
                assert_eq!(evidence.as_slice()[0].start_line, Some(12));
                assert_eq!(continuation, &baseline_patch);
            }
        }
    }

    #[test]
    fn continuation_source_labels_leave_unknown_fields_and_ranges_unknown() {
        use codestory_contracts::api::{DrillOptionDto, DrillSourceLabelDto};
        let mut option = DrillOptionDto::omitted_symbol("gap", "7");
        assert!(continuation_source_message(&option).is_none());
        option.source_label = Some(DrillSourceLabelDto {
            path: Some("src/a.rs".into()),
            symbol: None,
        });
        assert_eq!(
            continuation_source_message(&option).as_deref(),
            Some("src/a.rs")
        );
        assert!(option.source_range.is_none());
        option.source_label = Some(DrillSourceLabelDto {
            path: None,
            symbol: Some("run".into()),
        });
        assert_eq!(continuation_source_message(&option).as_deref(), Some("run"));
        option.source_label = Some(DrillSourceLabelDto {
            path: None,
            symbol: None,
        });
        assert!(continuation_source_message(&option).is_none());
    }

    #[test]
    fn continuation_progress_projects_priority_as_executable_references_with_canonical_gaps() {
        use codestory_contracts::api::{DrillOptionDto, PacketDispositionDto};
        let options = vec![
            DrillOptionDto::omitted_symbol("source_budget_exceeded:node:z", "z"),
            DrillOptionDto::omitted_symbol("disconnected_seed:node:a", "a"),
        ];
        let expected_ids = options
            .iter()
            .map(|option| option.id.clone())
            .collect::<Vec<_>>();
        let mut packet = continuation_progress_packet();
        let mut drill = continuation_progress_plan(options);
        // Gap order is authoritative even if option storage is reordered.
        drill.options.reverse();
        packet.disposition = PacketDispositionDto::drill_once("structural gap", drill);
        let mut gaps = packet_gaps(&packet);
        gaps.push(gap_row("unrelated-gap", GapKindV3Dto::EvidenceMissing, None).unwrap());
        let continuation = packet_continuation(&packet, &mut gaps).unwrap().unwrap();
        assert_eq!(
            continuation
                .gap_ids
                .as_slice()
                .iter()
                .map(|gap| gap.gap_id.as_str())
                .collect::<Vec<_>>(),
            expected_ids.iter().map(String::as_str).collect::<Vec<_>>(),
            "continuation refs must retain priority and remain executable option identities"
        );
        assert_eq!(continuation.remaining_rounds, 1);
        assert!(
            gaps.windows(2)
                .all(|pair| pair[0].identity < pair[1].identity)
        );
        assert!(
            continuation
                .gap_ids
                .as_slice()
                .iter()
                .all(|reference| gaps.iter().any(|gap| &gap.identity == reference))
        );
        assert_eq!(gaps.len(), 3);
    }

    #[test]
    fn continuation_progress_preserves_terminal_fallback_and_reference_caps() {
        use codestory_contracts::api::{DrillOptionDto, PacketDispositionDto};
        use codestory_contracts::packet_projection_v3::{GAP_ROWS_MAX_V3, REFERENCE_ROWS_MAX_V3};
        let mut packet = continuation_progress_packet();
        packet.disposition = PacketDispositionDto::drill_once(
            "structural gap",
            continuation_progress_plan(Vec::new()),
        );
        let mut gaps = packet_gaps(&packet);
        let fallback = packet_continuation(&packet, &mut gaps).unwrap().unwrap();
        assert_eq!(fallback.gap_ids.as_slice().len(), 1);
        assert_eq!(
            fallback.gap_ids.as_slice()[0].gap_id.as_str(),
            "packet-continuation-required"
        );
        packet.disposition.drill.as_mut().unwrap().remaining_rounds = 0;
        assert!(packet_continuation(&packet, &mut gaps).unwrap().is_none());

        let options = (0..GAP_ROWS_MAX_V3 + 2)
            .rev()
            .map(|index| {
                DrillOptionDto::omitted_symbol(
                    format!("disconnected_seed:node:{index:03}"),
                    format!("{index:03}"),
                )
            })
            .collect::<Vec<_>>();
        packet.disposition =
            PacketDispositionDto::drill_once("structural gap", continuation_progress_plan(options));
        let mut gaps = packet_gaps(&packet);
        // An invalid internal plan cannot advertise a broad action exceeding
        // the real packet request's eight-option limit. Gap bounds remain intact.
        assert!(packet_continuation(&packet, &mut gaps).is_err());
        assert_eq!(gaps.len(), GAP_ROWS_MAX_V3);
        let drill = packet.disposition.drill.as_mut().unwrap();
        drill
            .options
            .truncate(codestory_contracts::api::PACKET_DRILL_MAX_OPTIONS);
        drill.gap_ids = drill
            .options
            .iter()
            .map(|option| option.gap_id.clone())
            .collect();
        let mut gaps = packet_gaps(&packet);
        let bounded = packet_continuation(&packet, &mut gaps).unwrap().unwrap();
        assert!(bounded.gap_ids.as_slice().len() <= REFERENCE_ROWS_MAX_V3);
        assert_eq!(
            bounded.gap_ids.as_slice().len(),
            codestory_contracts::api::PACKET_DRILL_MAX_OPTIONS
        );
        assert!(
            bounded
                .gap_ids
                .as_slice()
                .iter()
                .all(|reference| gaps.iter().any(|gap| &gap.identity == reference))
        );
    }

    fn indexed_search_fixture() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        tempfile::TempDir,
        crate::AppController,
        crate::services::PublicOperationService,
    ) {
        indexed_search_fixture_with_files(1)
    }

    fn indexed_search_fixture_with_files(
        file_count: usize,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        tempfile::TempDir,
        crate::AppController,
        crate::services::PublicOperationService,
    ) {
        indexed_search_fixture_with_file_contents(file_count, "body { color: blue; }\n")
    }

    fn indexed_search_fixture_with_file_contents(
        file_count: usize,
        additional_source: &str,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        tempfile::TempDir,
        crate::AppController,
        crate::services::PublicOperationService,
    ) {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("isolated cache");
        let source = project.path().join("src/layout.css");
        fs::create_dir_all(source.parent().expect("source parent")).expect("source directory");
        fs::write(&source, "/* REPO_TEXT_TOKEN */\nbody { color: red; }\n").expect("source");
        for index in 1..file_count {
            fs::write(
                project.path().join(format!("src/file_{index:04}.css")),
                additional_source,
            )
            .expect("additional source");
        }
        let controller = crate::AppController::new_with_owned_cache_root(process_cache.path());
        controller
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                state.path().join("codestory.db"),
            )
            .expect("open isolated project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("complete core index");
        let service = crate::services::PublicOperationService::new(controller.clone());
        (project, state, process_cache, controller, service)
    }

    /// Supply real pinned test publications for projection tests whose public
    /// operation requires Full. No retrieval execution or model activation runs.
    fn full_projection_fixture_services(
        project: &std::path::Path,
        storage_path: &std::path::Path,
        process_cache: &std::path::Path,
    ) -> (
        crate::AppController,
        crate::services::PublicOperationService,
    ) {
        use codestory_retrieval::{SidecarProfile, SidecarRuntimeConfig, test_support};

        let config =
            codestory_retrieval::with_test_cache_root(&process_cache.join("retrieval"), || {
                SidecarRuntimeConfig::for_project_profile(Some(project), SidecarProfile::Agent)
            });
        let manifest =
            test_support::publish_zero_dense_pinned_query_fixture(project, storage_path, &config)
                .expect("test-only zero-dense Full publication");
        assert_eq!(manifest.dense_projection_count, Some(0));
        let controller = crate::AppController::new_with_config(config);
        controller
            .open_project_summary_with_storage_path(
                project.to_path_buf(),
                storage_path.to_path_buf(),
            )
            .expect("bind matching owned Full projection fixture");
        let service = crate::services::PublicOperationService::new(controller.clone());
        // The real Full operation pin validates owner, source coverage, vector
        // proof and generation coherence. This is an observer, not a query.
        let operation = service
            .run_observational_with_cancel("search", Arc::new(AtomicBool::new(false)), || Ok(()))
            .expect("strict Full fixture pin");
        let core = operation
            .core_publication
            .expect("complete fixture Core pin");
        let retrieval = operation
            .retrieval_publication
            .expect("complete fixture retrieval pin");
        assert_eq!(retrieval.core_generation_id, core.generation_id);
        assert_eq!(retrieval.core_run_id, core.run_id);
        assert_eq!(
            Some(retrieval.retrieval_generation.as_str()),
            manifest.sidecar_generation.as_deref(),
        );
        (controller, service)
    }

    fn packet_identity_fixture() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        crate::AppController,
        PublicOperationService,
    ) {
        use crate::test_support::git;
        let project = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        git(project.path(), &["init", "-q"]);
        git(project.path(), &["config", "user.name", "CodeStory Tests"]);
        git(
            project.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(
            project.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/packet/identity.git",
            ],
        );
        git(project.path(), &["commit", "--allow-empty", "-qm", "empty"]);
        fs::write(
            project.path().join("lib.rs"),
            "// packet identity fixture\n",
        )
        .unwrap();
        git(project.path(), &["add", "lib.rs"]);
        git(project.path(), &["commit", "-qm", "source"]);
        let storage = cache.path().join("core.db");
        let controller = crate::AppController::new_with_owned_cache_root(cache.path());
        controller
            .open_project_summary_with_storage_path(project.path().into(), storage.clone())
            .unwrap();
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .unwrap();
        drop(controller);
        let (controller, service) =
            full_projection_fixture_services(project.path(), &storage, cache.path());
        (project, cache, controller, service)
    }

    #[test]
    fn packet_representation_capture_observes_git_once_and_rejects_other_owners() {
        use codestory_workspace::{
            with_repository_metadata_observation_limit_for_test,
            with_repository_metadata_tree_traversal_count_for_test,
        };
        let (project, _cache, _controller, service) = packet_identity_fixture();
        let (_other_project, _other_cache, _other_controller, other_service) =
            packet_identity_fixture();
        let before = service.active_project_identity_v3().unwrap();
        let other = other_service.active_project_identity_v3().unwrap();
        assert!(before.portable_reuse_eligible);
        assert_eq!(before.project_id, other.project_id);
        assert_ne!(before.workspace_id, other.workspace_id);
        let request: AgentPacketRequestDto =
            serde_json::from_value(json!({"question":"inspect"})).unwrap();
        let reserved = PacketProjectionIdentityV3::new();
        let mut packet = continuation_progress_packet();
        let capture = service
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                packet.answer.retrieval_trace.retrieval_publication =
                    service.active_publication().unwrap().retrieval_publication;
                let expected = capture_packet_publication_v3(&service).unwrap();
                let (capture, walks) =
                    with_repository_metadata_tree_traversal_count_for_test(|| {
                        with_repository_metadata_observation_limit_for_test(1, || {
                            let capture = PacketRepresentationCaptureV3::new(&service).unwrap();
                            assert_eq!(capture.publication, expected);
                            for count in 0..3 {
                                packet.support = (0..count)
                                    .map(|index| {
                                        let mut row =
                                            support_unit(SupportUnitKindDto::SymbolLocation);
                                        row.id = format!("symbol:node:{index}");
                                        row.symbol_id = Some(index.to_string());
                                        row.path = Some("lib.rs".into());
                                        row.start_line = Some(1);
                                        row
                                    })
                                    .collect();
                                let product = capture
                                    .project("test", &request, &packet, &reserved, |p| {
                                        serde_json::to_vec(p).map(|v| v.len()).map_err(|_| ())
                                    })
                                    .unwrap();
                                assert_eq!(product.diagnostics.project_identity, before.project_id);
                                assert_eq!(product.diagnostics.packet_id, reserved.0);
                                assert!(matches!(
                                    product.projection,
                                    PacketProjectionV3Dto::Complete { .. }
                                ));
                            }
                            capture
                        })
                    });
                assert!(
                    walks > 0,
                    "the real Git observation must exercise the expensive reader"
                );
                other_service.run_with_cancel(
                    "packet",
                    Arc::new(AtomicBool::new(false)),
                    || {
                        assert_eq!(
                            capture
                                .project("test", &request, &packet, &reserved, |_| Ok(0))
                                .unwrap_err()
                                .code,
                            "publication_changed"
                        );
                        Ok(())
                    },
                )?;
                capture.check_owner()?;
                Ok(capture)
            })
            .unwrap()
            .value;
        assert_eq!(
            capture
                .project("test", &request, &packet, &reserved, |_| Ok(0))
                .unwrap_err()
                .code,
            "publication_changed"
        );
        service
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                assert_eq!(
                    capture
                        .project("test", &request, &packet, &reserved, |_| Ok(0))
                        .unwrap_err()
                        .code,
                    "publication_changed"
                );
                let next = PacketRepresentationCaptureV3::new(&service)?;
                assert_eq!(next.publication, capture.publication);
                Ok(())
            })
            .unwrap();

        // Only HEAD moves: indexed source and the index inventory remain unchanged.
        crate::test_support::git(project.path(), &["reset", "--soft", "HEAD^"]);
        let after = service.active_project_identity_v3().unwrap();
        assert_eq!(before.project_id, after.project_id);
        assert_eq!(before.workspace_id, after.workspace_id);
        assert!(!after.portable_reuse_eligible);
        assert_ne!(before.artifact_scope_id, after.artifact_scope_id);
        assert_eq!(
            service
                .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || Ok(()))
                .unwrap_err()
                .code,
            "retrieval_unavailable"
        );
        let (_dirty_controller, dirty_service) = full_projection_fixture_services(
            project.path(),
            &_cache.path().join("core.db"),
            _cache.path(),
        );
        dirty_service
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                assert_eq!(
                    capture
                        .project("test", &request, &packet, &reserved, |_| Ok(0))
                        .unwrap_err()
                        .code,
                    "publication_changed"
                );
                let expected = capture_packet_publication_v3(&dirty_service).unwrap();
                let next = PacketRepresentationCaptureV3::new(&dirty_service)?;
                assert_eq!(next.publication, expected);
                assert_ne!(
                    next.publication, capture.publication,
                    "the next operation must recapture portability"
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn packet_representation_capture_preserves_cancellation_and_source_fences() {
        for cancel in [false, true] {
            let (project, _cache, _controller, service) = packet_identity_fixture();
            let cancelled = Arc::new(AtomicBool::new(false));
            let request: AgentPacketRequestDto =
                serde_json::from_value(json!({"question":"inspect"})).unwrap();
            let mut builds = 0;
            let error = service
                .run_with_cancel("packet", cancelled.clone(), || {
                    builds += 1;
                    let mut packet = continuation_progress_packet();
                    packet.answer.retrieval_trace.retrieval_publication =
                        service.active_publication().unwrap().retrieval_publication;
                    let capture = PacketRepresentationCaptureV3::new(&service)?;
                    capture.project(
                        "test",
                        &request,
                        &packet,
                        &PacketProjectionIdentityV3::new(),
                        |p| {
                            if cancel {
                                cancelled.store(true, std::sync::atomic::Ordering::Release);
                            } else {
                                fs::write(
                                    project.path().join("lib.rs"),
                                    "// changed packet source\n",
                                )
                                .unwrap();
                            }
                            serde_json::to_vec(p).map(|v| v.len()).map_err(|_| ())
                        },
                    )
                })
                .unwrap_err();
            assert_eq!(builds, 1);
            assert_eq!(
                error.code,
                if cancel {
                    "cancelled"
                } else {
                    "project_unavailable"
                },
                "{error:?}"
            );
        }
    }

    fn indexed_full_search_fixture_with_files(
        file_count: usize,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        tempfile::TempDir,
        crate::AppController,
        crate::services::PublicOperationService,
    ) {
        // Empty unrelated fillers retain the actual 2,003-file scan boundary
        // without allocating a data block per file. The queried source remains.
        let (project, state, process_cache, core_controller, core_service) =
            indexed_search_fixture_with_file_contents(file_count, "");
        assert_eq!(
            core_controller
                .open_storage_read_only()
                .unwrap()
                .get_files()
                .unwrap()
                .len(),
            file_count,
        );
        drop(core_service);
        drop(core_controller);
        let (controller, service) = full_projection_fixture_services(
            project.path(),
            &state.path().join("codestory.db"),
            process_cache.path(),
        );
        (project, state, process_cache, controller, service)
    }

    fn core_search_request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_owned(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        }
    }

    #[test]
    fn search_projection_retains_bounded_symbol_labels_without_fabricating_excerpts() {
        let (project, _state, _process_cache, controller, service) = indexed_search_fixture();
        fs::write(
            project.path().join("src/labels.rs"),
            "pub fn retained_label_anchor() {}\npub fn neighboring_symbol_control() {}\n",
        )
        .expect("callable source");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index real callable labels");
        service
            .run_observational_with_cancel("exact_search", Arc::new(AtomicBool::new(false)), || {
                let names = ["neighboring_symbol_control", "retained_label_anchor"];
                let mut results = controller.search_results(core_search_request(names[0]))?;
                let mut hits = Vec::new();
                for name in names {
                    let found = controller.search_results(core_search_request(name))?;
                    let mut hit = found
                        .hits
                        .into_iter()
                        .find(|hit| hit.display_name == name)
                        .expect("fixture callable is a real indexed hit");
                    assert!(hit.resolvable);
                    assert!(
                        std::path::Path::new(hit.file_path.as_deref().expect("located callable"))
                            .ends_with("src/labels.rs")
                    );
                    // Broad search can retain a label without capturing source text.
                    hit.source_excerpt = None;
                    hits.push(hit);
                }
                results.hits = hits.clone();
                results.indexed_symbol_hits = hits.clone();
                results.repo_text_hits.clear();
                let projected =
                    serde_json::to_value(project_search_v3(&service, "test", &results)?)
                        .expect("public search JSON");
                let rows = projected["evidence"].as_array().expect("evidence rows");
                assert_eq!(rows.len(), 2);
                for ((row, hit), name) in rows.iter().zip(&hits).zip(names) {
                    assert_eq!(row["symbol_id"].as_str(), Some(hit.node_id.0.as_str()));
                    assert_eq!(row["path"].as_str(), hit.file_path.as_deref());
                    assert_eq!(row["start_line"].as_u64(), hit.line.map(u64::from));
                    assert!(row["excerpt"].is_null());
                    assert_eq!(
                        row["display_name"], name,
                        "located symbol labels must survive when no excerpt was captured"
                    );
                }

                for (name, resolvable) in [("src/text-only.rs", false), (" \t", true)] {
                    let mut metadata_only = results.clone();
                    metadata_only.hits[0].display_name = name.into();
                    metadata_only.hits[0].resolvable = resolvable;
                    let value =
                        serde_json::to_value(project_search_v3(&service, "test", &metadata_only)?)
                            .expect("nullable label JSON");
                    assert!(value["evidence"][0]["display_name"].is_null());
                    assert!(value["evidence"][0]["excerpt"].is_null());
                    assert_eq!(value["evidence"][0]["path"], rows[0]["path"]);
                    assert_eq!(value["evidence"][0]["identity"], rows[0]["identity"]);
                }

                let mut overbound = results;
                overbound.hits[0].display_name = "界".repeat(342);
                let value = serde_json::to_value(project_search_v3(&service, "test", &overbound)?)
                    .expect("bounded UTF-8 label JSON");
                assert_eq!(value["evidence"][0]["display_name"], "界".repeat(341));
                assert!(value["evidence"][0]["excerpt"].is_null());
                assert_eq!(value["evidence"][0]["symbol_id"], rows[0]["symbol_id"]);
                assert_eq!(value["evidence"][0]["identity"], rows[0]["identity"]);
                assert_eq!(value["evidence"][0]["path"], rows[0]["path"]);
                assert_eq!(value["evidence"][1], rows[1]);
                Ok(())
            })
            .expect("symbol labels are presentation metadata within the pinned publication");
    }

    #[test]
    fn compound_core_name_miss_preserves_separate_target_lookups() {
        use codestory_contracts::packet_projection_v3::EvidenceAvailabilityV3Dto;

        let (_project, _state, _process_cache, controller, service) =
            indexed_search_fixture_with_files(2);
        service
            .run_observational_with_cancel("exact_search", Arc::new(AtomicBool::new(false)), || {
                let query = "layout.css file_0001.css";
                let missed = controller.search_results(core_search_request(query))?;
                assert_eq!(missed.query, query);
                assert!(missed.hits.is_empty());
                let projected = project_search_v3(&service, "test", &missed)?;
                assert_eq!(
                    projected.status,
                    EvidenceAvailabilityV3Dto::NoUsefulEvidence
                );
                assert_eq!(projected.gaps.as_slice().len(), 1);
                assert_eq!(
                    projected.gaps.as_slice()[0].identity.gap_id.as_str(),
                    "search-core-name-no-match"
                );
                assert!(projected.publication.retrieval.is_none());

                for target in ["layout.css", "file_0001.css"] {
                    let found = controller.search_results(core_search_request(target))?;
                    assert_eq!(found.query, target);
                    assert!(!found.hits.is_empty());
                    let projected = project_search_v3(&service, "test", &found)?;
                    assert!(!projected.evidence.as_slice().is_empty());
                    assert!(
                        !projected
                            .gaps
                            .as_slice()
                            .iter()
                            .any(|gap| gap.identity.gap_id.as_str() == "search-core-name-no-match")
                    );
                    assert!(projected.publication.retrieval.is_none());
                }
                Ok(())
            })
            .expect("compound miss and separate targets remain Core-only");
    }

    #[test]
    fn search_projection_discloses_empty_core_name_scope_without_claiming_unavailability() {
        use codestory_contracts::api::RetrievalModeDto;
        use codestory_contracts::packet_projection_v3::{
            EvidenceAvailabilityV3Dto, SearchProjectionKindV3Dto,
        };

        let (_project, _state, _process_cache, controller, service) = indexed_search_fixture();
        service
            .run_observational_with_cancel("exact_search", Arc::new(AtomicBool::new(false)), || {
                // The fixture contains this source literal, but it is not a
                // core name. An empty name search cannot prove its absence.
                let results = controller.search_results(core_search_request("REPO_TEXT_TOKEN"))?;
                assert!(results.hits.is_empty());
                let expected_message = results
                    .retrieval
                    .fallback_message
                    .as_deref()
                    .expect("core search describes its actual scope");
                let projected = project_search_v3(&service, "test", &results)?;
                let gaps = projected.gaps.as_slice();
                assert_eq!(gaps.len(), 1);
                assert_eq!(
                    gaps[0].identity.gap_id.as_str(),
                    "search-core-name-no-match"
                );
                assert_eq!(gaps[0].kind, GapKindV3Dto::EvidenceMissing);
                assert_eq!(gaps[0].message.as_ref().unwrap().as_str(), expected_message);
                assert_eq!(
                    projected.status,
                    EvidenceAvailabilityV3Dto::NoUsefulEvidence
                );
                assert_eq!(projected.kind, SearchProjectionKindV3Dto::Complete);
                assert_eq!(projected.retrieval.state, RetrievalStateV3Dto::Symbolic);
                assert!(projected.evidence.as_slice().is_empty());
                assert!(projected.continuation.is_none());
                assert_eq!(
                    projected.diagnostics,
                    DiagnosticsCapabilityV3Dto::Unavailable
                );
                let serialized = serde_json::to_value(&projected).expect("public JSON");
                assert_eq!(serialized["gaps"][0]["message"], expected_message);
                assert_eq!(serialized["status"], "no_useful_evidence");
                assert!(projected.publication.retrieval.is_none());
                assert!(
                    serialized["publication"]
                        .get("retrieval")
                        .unwrap()
                        .is_null()
                );

                // A returned core FILE lead remains useful evidence and must
                // not inherit the empty-result notice.
                let found = controller.search_results(core_search_request("layout.css"))?;
                assert!(!found.hits.is_empty());
                let found_projection = project_search_v3(&service, "test", &found)?;
                assert!(!found_projection.evidence.as_slice().is_empty());
                assert_eq!(
                    found_projection.status,
                    EvidenceAvailabilityV3Dto::Available
                );
                assert_eq!(found_projection.publication, projected.publication);
                assert!(
                    !found_projection
                        .gaps
                        .as_slice()
                        .iter()
                        .any(|gap| { gap.identity.gap_id.as_str() == "search-core-name-no-match" })
                );

                // Projection-only negative metadata cases; these do not
                // execute or claim full retrieval readiness.
                for (mode, enabled, retrieval_mode) in [
                    (SearchRepoTextMode::Auto, false, RetrievalModeDto::Symbolic),
                    (SearchRepoTextMode::Off, true, RetrievalModeDto::Symbolic),
                    (SearchRepoTextMode::Off, false, RetrievalModeDto::Hybrid),
                ] {
                    let mut other_lane = results.clone();
                    other_lane.repo_text_mode = mode;
                    other_lane.repo_text_enabled = enabled;
                    other_lane.retrieval.mode = retrieval_mode;
                    let other_projection = project_search_v3(&service, "test", &other_lane)?;
                    assert!(!other_projection.gaps.as_slice().iter().any(|gap| {
                        gap.identity.gap_id.as_str() == "search-core-name-no-match"
                    }));
                }

                // Missing source for an existing hit is a different failure;
                // empty projected rows must not masquerade as no core match.
                let mut missing_source = found.clone();
                for hit in &mut missing_source.hits {
                    hit.file_path = None;
                }
                let missing_projection = project_search_v3(&service, "test", &missing_source)?;
                assert!(missing_projection.evidence.as_slice().is_empty());
                assert_eq!(
                    missing_projection.status,
                    EvidenceAvailabilityV3Dto::Unavailable
                );
                assert!(
                    missing_projection
                        .gaps
                        .as_slice()
                        .iter()
                        .any(|gap| { gap.kind == GapKindV3Dto::SourceUnavailable })
                );
                assert!(
                    !missing_projection
                        .gaps
                        .as_slice()
                        .iter()
                        .any(|gap| { gap.identity.gap_id.as_str() == "search-core-name-no-match" })
                );
                Ok(())
            })
            .expect("real core result and public projection contract");
    }

    #[test]
    fn search_projection_discloses_interrupted_retrieval_without_changing_availability() {
        use codestory_contracts::api::{RetrievalModeDto, RetrievalShadowDto};
        use codestory_contracts::packet_projection_v3::{
            EvidenceAvailabilityV3Dto, SearchProjectionKindV3Dto,
        };

        let fixture = crate::services::activation_tests::ready_activation_fixture();
        let service = fixture.runtime.public_operation_service();
        let browser = fixture.runtime.browser_service();
        let mut results = service
            .run_with_cancel("exact_search", Arc::new(AtomicBool::new(false)), || {
                browser.search_results(core_search_request("metadata.rs"))
            })
            .expect("located core search evidence")
            .value;

        for query in [
            "TF_VAR",
            "decode module settings",
            "src/layout.css",
            "Handler::Load",
        ] {
            results.query = query.into();
            let baseline = service
                .run_with_cancel("search", Arc::new(AtomicBool::new(false)), || {
                    let mut admitted = results.clone();
                    admitted.retrieval.mode = RetrievalModeDto::Hybrid;
                    admitted.retrieval_publication = service
                        .active_publication()
                        .expect("full search owns a publication snapshot")
                        .retrieval_publication;
                    assert!(admitted.retrieval_publication.is_some());
                    project_search_v3(&service, "test", &admitted)
                })
                .expect("complete search projection")
                .value;
            assert!(!baseline.evidence.as_slice().is_empty());
            assert_eq!(baseline.retrieval.state, RetrievalStateV3Dto::Full);

            for (name, aggregate_stop, stage_status, stage_stop, full, expected_gap) in [
                ("completed", None, Some("completed"), None, true, false),
                (
                    "witnessed lexical deadline",
                    Some("stage_deadline"),
                    Some("pending_after_deadline"),
                    Some("stage_deadline"),
                    true,
                    true,
                ),
                (
                    "unfinished stage without aggregate stop",
                    None,
                    Some("pending_after_deadline"),
                    None,
                    true,
                    true,
                ),
                (
                    "cancelled before execution",
                    None,
                    Some("cancelled_before_start"),
                    Some("cancelled"),
                    true,
                    true,
                ),
                (
                    "early request deadline",
                    Some("deadline"),
                    None,
                    None,
                    true,
                    true,
                ),
                (
                    "cancelled without stage rows",
                    Some("cancelled"),
                    None,
                    None,
                    true,
                    true,
                ),
                (
                    "all stages completed before cancellation",
                    Some("cancelled"),
                    Some("completed"),
                    None,
                    true,
                    true,
                ),
                (
                    "completed late",
                    None,
                    Some("completed_late"),
                    None,
                    true,
                    false,
                ),
                (
                    "exact definition skips broad lane",
                    None,
                    Some("skipped"),
                    Some("unique_exact_definition"),
                    true,
                    false,
                ),
                (
                    "empty dense lane skipped",
                    None,
                    Some("skipped"),
                    Some("zero_dense_anchors"),
                    true,
                    false,
                ),
                (
                    "normal marginal gain stop",
                    Some("marginal_gain"),
                    Some("completed"),
                    None,
                    true,
                    false,
                ),
                (
                    "opaque stop reason",
                    Some("unknown_internal_reason"),
                    Some("completed"),
                    None,
                    true,
                    false,
                ),
                (
                    "core-only ignores foreign shadow",
                    Some("stage_deadline"),
                    Some("pending_after_deadline"),
                    Some("stage_deadline"),
                    false,
                    false,
                ),
            ] {
                let mut case = results.clone();
                let stages = stage_status
                    .map(|status| {
                        vec![json!({
                            "stage": "stage1_lexical",
                            "deadline_ms": 96,
                            "elapsed_ms": 97,
                            "queue_wait_ms": 0,
                            "candidates_added": 0,
                            "cancel_reason": stage_stop,
                            "completion_status": status,
                            "cache_hit": false,
                        })]
                    })
                    .unwrap_or_default();
                case.retrieval_shadow = Some(
                    serde_json::from_value::<RetrievalShadowDto>(json!({
                        "retrieval_mode": "full",
                        "retrieval_total_ms": 4754,
                        "total_budget_ms": 420,
                        "cancel_reason": aggregate_stop,
                        "cache_hit": false,
                        "stage_timings": stages,
                    }))
                    .expect("existing execution shadow contract"),
                );
                let projected = service
                    .run_with_cancel(
                        if full { "search" } else { "exact_search" },
                        Arc::new(AtomicBool::new(false)),
                        || {
                            let mut admitted = case.clone();
                            admitted.retrieval.mode = if full {
                                RetrievalModeDto::Hybrid
                            } else {
                                RetrievalModeDto::Symbolic
                            };
                            admitted.retrieval_publication = service
                                .active_publication()
                                .expect("search owns a publication snapshot")
                                .retrieval_publication;
                            assert_eq!(admitted.retrieval_publication.is_some(), full);
                            project_search_v3(&service, "test", &admitted)
                        },
                    )
                    .expect("search projection")
                    .value;
                let interruption_gaps: Vec<_> = projected
                    .gaps
                    .as_slice()
                    .iter()
                    .filter(|gap| gap.identity.gap_id.as_str() == "search-retrieval-interrupted")
                    .collect();
                assert_eq!(
                    interruption_gaps.len(),
                    usize::from(expected_gap),
                    "{name}; query={query}"
                );
                for gap in interruption_gaps {
                    assert_eq!(gap.kind, GapKindV3Dto::EvidenceMissing);
                    let summary = gap
                        .message
                        .as_ref()
                        .expect("factual interruption disclosure")
                        .as_str();
                    assert!(
                        summary.contains("deadline") && summary.contains("cancellation"),
                        "{name}: {summary}"
                    );
                    assert!(
                        !summary.contains("publication"),
                        "a query stop is not a partial publication"
                    );
                    assert!(
                        !summary.contains("lexical"),
                        "aggregate stops do not identify a failed stage"
                    );
                }
                assert_eq!(
                    projected.evidence, baseline.evidence,
                    "retain admitted locations: {name}"
                );
                assert_eq!(
                    projected.publication.core, baseline.publication.core,
                    "retain pinned core: {name}"
                );
                assert_eq!(
                    projected.status,
                    EvidenceAvailabilityV3Dto::Available,
                    "{name}"
                );
                assert_eq!(
                    projected.kind,
                    SearchProjectionKindV3Dto::Complete,
                    "{name}"
                );
                assert_eq!(
                    projected.diagnostics,
                    DiagnosticsCapabilityV3Dto::Unavailable,
                    "{name}"
                );
                if full {
                    assert_eq!(
                        projected.publication, baseline.publication,
                        "retain retrieval publication: {name}"
                    );
                    assert_eq!(
                        projected.retrieval, baseline.retrieval,
                        "retain publication eligibility: {name}"
                    );
                } else {
                    assert_eq!(projected.retrieval.state, RetrievalStateV3Dto::Symbolic);
                }
            }
        }
    }

    #[test]
    fn search_projection_discloses_real_bounded_repo_text_scan() {
        for file_count in [1, crate::repo_text::REPO_TEXT_SCAN_FILE_CAP + 3] {
            let (project, _state, _process_cache, controller, service) =
                indexed_full_search_fixture_with_files(file_count);
            let projected = service
                .run_observational_with_cancel("search", Arc::new(AtomicBool::new(false)), || {
                    let mut results =
                        controller.search_results(core_search_request("ABSENT_TOKEN"))?;
                    let storage = controller.open_storage_read_only()?;
                    let scan = crate::AppController::collect_repo_text_hits(
                        &storage,
                        Some(project.path()),
                        &codestory_workspace::SourceIndexPolicy::default(),
                        "ABSENT_TOKEN",
                        10,
                        &HashSet::new(),
                    )?;
                    assert!(scan.hits.is_empty());
                    assert_eq!(scan.stats.truncated, file_count > 1);
                    results.repo_text_hits = scan.hits;
                    results.repo_text_stats = Some(scan.stats);
                    results.repo_text_mode = SearchRepoTextMode::On;
                    results.repo_text_enabled = true;
                    // Bind projection-only data to this operation's actual Full pin.
                    results.retrieval.mode = codestory_contracts::api::RetrievalModeDto::Hybrid;
                    results.retrieval_publication = service
                        .active_publication()
                        .expect("Full operation publication")
                        .retrieval_publication;
                    assert!(results.retrieval_publication.is_some());
                    project_search_v3(&service, "test", &results)
                })
                .expect("schema-3 projection")
                .value;
            assert_eq!(
                projected.gaps.as_slice().iter().any(|gap| {
                    gap.identity.gap_id.as_str() == "search-repo-text-incomplete"
                        && gap.kind == GapKindV3Dto::EvidenceMissing
                }),
                file_count > 1,
                "an empty bounded scan cannot establish source absence: {projected:#?}"
            );
        }
    }

    #[test]
    fn search_projection_discloses_skipped_files_only_for_requested_text_lane() {
        let (_project, _state, _process_cache, controller, service) =
            indexed_full_search_fixture_with_files(1);
        for (enabled, truncated, expected_gap) in [(true, false, true), (false, true, false)] {
            let projected = service
                .run_observational_with_cancel("search", Arc::new(AtomicBool::new(false)), || {
                    let mut results =
                        controller.search_results(core_search_request("layout.css"))?;
                    results.repo_text_enabled = enabled;
                    results.repo_text_stats =
                        Some(codestory_contracts::api::RepoTextScanStatsDto {
                            scanned_file_count: 1,
                            scanned_byte_count: 0,
                            skipped_large_file_count: 1,
                            file_cap: 2_000,
                            byte_cap: 32 * 1024 * 1024,
                            time_cap_ms: 500,
                            duration_ms: 0,
                            truncated,
                            reason: None,
                            action: None,
                        });
                    // Bind projection-only data to this operation's actual Full pin.
                    results.retrieval.mode = codestory_contracts::api::RetrievalModeDto::Hybrid;
                    results.retrieval_publication = service
                        .active_publication()
                        .expect("Full operation publication")
                        .retrieval_publication;
                    assert!(results.retrieval_publication.is_some());
                    project_search_v3(&service, "test", &results)
                })
                .expect("schema-3 projection")
                .value;
            assert_eq!(
                projected
                    .gaps
                    .as_slice()
                    .iter()
                    .any(|gap| { gap.identity.gap_id.as_str() == "search-repo-text-incomplete" }),
                expected_gap,
                "skipped files limit requested text evidence, not core-only search"
            );
            assert!(
                !projected.evidence.as_slice().is_empty(),
                "retain located source"
            );
        }
    }

    #[test]
    fn search_projection_keeps_real_repo_text_only_match() {
        let (project, _state, _process_cache, controller, service) =
            indexed_full_search_fixture_with_files(1);
        let projected = service
            .run_observational_with_cancel("search", Arc::new(AtomicBool::new(false)), || {
                let mut results =
                    controller.search_results(core_search_request("REPO_TEXT_TOKEN"))?;
                assert!(
                    results.hits.is_empty(),
                    "the core symbol lane has no literal hit"
                );
                let storage = controller.open_storage_read_only()?;
                let scan = crate::AppController::collect_repo_text_hits(
                    &storage,
                    Some(project.path()),
                    &codestory_workspace::SourceIndexPolicy::default(),
                    "REPO_TEXT_TOKEN",
                    10,
                    &HashSet::new(),
                )?;
                assert_eq!(scan.hits.len(), 1, "source-backed repository-text result");
                results.repo_text_hits = scan.hits;
                results.repo_text_mode = SearchRepoTextMode::On;
                results.repo_text_enabled = true;
                // Bind projection-only data to this operation's actual Full pin.
                results.retrieval.mode = codestory_contracts::api::RetrievalModeDto::Hybrid;
                results.retrieval_publication = service
                    .active_publication()
                    .expect("Full operation publication")
                    .retrieval_publication;
                assert!(results.retrieval_publication.is_some());
                project_search_v3(&service, "test", &results)
            })
            .expect("schema-3 projection")
            .value;
        assert!(
            projected.evidence.as_slice().iter().any(|row| {
                row.path.as_str().ends_with("src/layout.css") && row.start_line == Some(1)
            }),
            "real repo-text match must reach public schema-3 evidence: {projected:#?}"
        );
    }

    #[test]
    fn search_projection_keeps_parentless_core_file_match() {
        let (_project, _state, _process_cache, controller, service) = indexed_search_fixture();
        let projected = service
            .run_observational_with_cancel(
                crate::services::search_operation_name(SearchRepoTextMode::Off),
                Arc::new(AtomicBool::new(false)),
                || {
                    let results = controller.search_results(core_search_request("layout.css"))?;
                    let file_hit = results
                        .hits
                        .iter()
                        .find(|hit| hit.kind == codestory_contracts::api::NodeKind::FILE)
                        .expect("core search found indexed FILE");
                    let storage = controller.open_storage_read_only()?;
                    let file_node = storage
                        .get_node(file_hit.node_id.to_core()?)
                        .expect("indexed FILE lookup")
                        .expect("indexed FILE node");
                    assert_eq!(file_node.file_node_id, None, "FILE has no parent file");
                    assert!(
                        file_hit.file_path.is_some(),
                        "pinned FILE path: {file_hit:#?}"
                    );
                    project_search_v3(&service, "test", &results)
                },
            )
            .expect("schema-3 projection")
            .value;
        assert!(
            projected
                .evidence
                .as_slice()
                .iter()
                .any(|row| { row.path.as_str() == "src/layout.css" }),
            "core FILE match must reach public schema-3 evidence: {projected:#?}"
        );
    }

    #[test]
    fn search_projection_does_not_turn_component_diagnostics_into_source_or_missing_source() {
        use codestory_contracts::api::{PacketEvidenceResolutionDto, PacketEvidenceTierDto};
        let (_project, _state, _process_cache, controller, service) = indexed_search_fixture();
        service
            .run_observational_with_cancel(
                crate::services::search_operation_name(SearchRepoTextMode::Off),
                Arc::new(AtomicBool::new(false)),
                || {
                    let mut results =
                        controller.search_results(core_search_request("layout.css"))?;
                    let source = results.hits[0].clone();
                    let mut report = source.clone();
                    report.node_id.0 = "derived-report".into();
                    report.file_path = None;
                    report.evidence_tier = Some(PacketEvidenceTierDto::ComponentReport);
                    report.resolution_status = Some(PacketEvidenceResolutionDto::DiagnosticOnly);
                    results.hits.insert(0, report.clone());
                    // A representative path is report metadata, never source truth.
                    report.file_path = source.file_path.clone();
                    results.repo_text_hits = vec![report];
                    let projected = project_search_v3(&service, "test", &results)?;
                    assert_eq!(projected.evidence.as_slice().len(), 1);
                    assert!(
                        !projected
                            .gaps
                            .as_slice()
                            .iter()
                            .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable)
                    );
                    let mut missing = source;
                    missing.node_id.0 = "ordinary-missing-source".into();
                    missing.file_path = None;
                    results.hits.push(missing);
                    let projected = project_search_v3(&service, "test", &results)?;
                    assert!(
                        projected
                            .gaps
                            .as_slice()
                            .iter()
                            .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable)
                    );
                    Ok(())
                },
            )
            .expect("pinned search projection");
    }

    #[test]
    fn search_projection_limits_combined_unique_hits_without_changing_source_order() {
        let (_project, _state, _process_cache, controller, service) = indexed_search_fixture();
        service
            .run_observational_with_cancel(
                crate::services::search_operation_name(SearchRepoTextMode::Off),
                Arc::new(AtomicBool::new(false)),
                || {
                    for limit in [1, 2, 10, 15] {
                        let mut request = core_search_request("layout.css");
                        request.limit_per_source = limit;
                        let mut results = controller.search_results(request)?;
                        assert_eq!(results.limit_per_source, limit);
                        let mut first = results.hits[0].clone();
                        first.node_id.0 = "indexed-first".into();
                        first.file_path = Some("src/indexed-first.css".into());
                        let mut second = first.clone();
                        second.node_id.0 = "indexed-second".into();
                        second.file_path = Some("src/indexed-second.css".into());
                        results.hits = vec![first.clone()];
                        if limit > 1 {
                            results.hits.push(second);
                        }
                        results.repo_text_hits = (0..limit)
                            .map(|index| {
                                if limit > 1 && index == 0 {
                                    return first.clone();
                                }
                                let mut hit = first.clone();
                                hit.node_id.0 = format!("text-{index}");
                                hit.file_path = Some(format!("src/text-{index}.css"));
                                hit
                            })
                            .collect();
                        let projected = project_search_v3(&service, "test", &results)?;
                        let rows = projected.evidence.as_slice();
                        assert_eq!(rows.len(), limit as usize, "combined limit {limit}");
                        assert_eq!(rows[0].path.as_str(), "src/indexed-first.css");
                        if limit > 1 {
                            assert_eq!(rows[1].path.as_str(), "src/indexed-second.css");
                        }
                        for (index, row) in rows.iter().skip(2).enumerate() {
                            assert_eq!(row.path.as_str(), format!("src/text-{}.css", index + 1));
                        }
                        assert!(
                            projected
                                .gaps
                                .as_slice()
                                .iter()
                                .any(|gap| { gap.kind == GapKindV3Dto::OutputBudgetExceeded })
                        );

                        // A duplicate-only tail does not mean another source row was omitted.
                        results.repo_text_hits = vec![first];
                        let projected = project_search_v3(&service, "test", &results)?;
                        assert_eq!(projected.evidence.as_slice().len(), limit.min(2) as usize);
                        assert!(
                            !projected
                                .gaps
                                .as_slice()
                                .iter()
                                .any(|gap| { gap.kind == GapKindV3Dto::OutputBudgetExceeded })
                        );
                    }
                    Ok(())
                },
            )
            .expect("mixed search sources obey one public limit");
    }

    #[test]
    fn search_projection_deduplicates_mixed_hits_and_accounts_for_missing_or_bounded_rows() {
        let (_project, _state, _process_cache, controller, service) = indexed_search_fixture();
        let results = service
            .run_observational_with_cancel(
                crate::services::search_operation_name(SearchRepoTextMode::Off),
                Arc::new(AtomicBool::new(false)),
                || controller.search_results(core_search_request("layout.css")),
            )
            .expect("core search")
            .value;
        let file_hit = results
            .hits
            .iter()
            .find(|hit| hit.kind == codestory_contracts::api::NodeKind::FILE)
            .expect("indexed FILE")
            .clone();

        let mut duplicate = results.clone();
        duplicate.hits = vec![file_hit.clone()];
        duplicate.repo_text_hits = vec![file_hit.clone()];
        let selection = search_evidence_selection(&duplicate);
        assert_eq!(
            selection.rows.len(),
            1,
            "one source location across both lists"
        );
        assert!(!selection.missing_source && !selection.was_bounded);

        let mut missing = duplicate.clone();
        missing.hits[0].file_path = None;
        missing.repo_text_hits.clear();
        let missing_projection = service
            .run_observational_with_cancel(
                crate::services::search_operation_name(SearchRepoTextMode::Off),
                Arc::new(AtomicBool::new(false)),
                || project_search_v3(&service, "test", &missing),
            )
            .expect("source-unavailable projection")
            .value;
        assert!(missing_projection.evidence.as_slice().is_empty());
        assert!(
            missing_projection
                .gaps
                .as_slice()
                .iter()
                .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable)
        );

        let mut bounded = duplicate;
        bounded.hits = (0..=bounded.limit_per_source)
            .map(|index| {
                let mut hit = file_hit.clone();
                hit.node_id.0 = format!("file-{index}");
                hit.file_path = Some(format!("src/file-{index}.css"));
                hit
            })
            .collect();
        bounded.repo_text_hits = vec![bounded.hits[0].clone()];
        let bounded_projection = service
            .run_observational_with_cancel(
                crate::services::search_operation_name(SearchRepoTextMode::Off),
                Arc::new(AtomicBool::new(false)),
                || project_search_v3(&service, "test", &bounded),
            )
            .expect("bounded projection")
            .value;
        assert_eq!(bounded_projection.evidence.as_slice().len(), 10);
        assert!(
            bounded_projection
                .gaps
                .as_slice()
                .iter()
                .any(|gap| gap.kind == GapKindV3Dto::OutputBudgetExceeded)
        );
    }

    #[test]
    fn file_search_path_comes_from_pinned_file_identity() {
        let (_project, state, _process_cache, controller, _service) = indexed_search_fixture();
        let file_hit = controller
            .search_results(core_search_request("layout.css"))
            .expect("core search")
            .hits
            .into_iter()
            .find(|hit| hit.kind == codestory_contracts::api::NodeKind::FILE)
            .expect("indexed FILE");
        let node_id = file_hit.node_id.to_core().expect("numeric file identity");
        let published =
            codestory_store::Store::open(state.path().join("codestory.db")).expect("pinned store");
        let scratch_path = state.path().join("isolated-path-test.db");
        published
            .get_connection()
            .execute(
                "VACUUM INTO ?1",
                [scratch_path.to_str().expect("UTF-8 scratch path")],
            )
            .expect("snapshot into isolated writable test store");
        let storage = codestory_store::Store::open_with_mode(
            &scratch_path,
            codestory_store::StorageOpenMode::Build,
        )
        .expect("isolated writable test store");
        let pinned_path = storage
            .get_file_by_id(node_id.0)
            .expect("file record query")
            .expect("file record")
            .path;
        storage
            .get_connection()
            .execute(
                "UPDATE node SET serialized_name = 'forged/outside.css' WHERE id = ?1",
                [node_id.0],
            )
            .expect("hostile display-name mutation");
        let path_bound =
            crate::AppController::build_search_hit(&storage, &HashMap::new(), node_id, 1.0)
                .expect("search hit")
                .expect("FILE hit");
        assert_eq!(
            path_bound.file_path.as_deref(),
            Some(pinned_path.to_str().expect("UTF-8 path")),
            "untrusted display name cannot replace pinned FILE path"
        );
    }

    #[test]
    fn complete_query_negative_is_a_gap_not_a_bounded_evidence_row() {
        let support = vec![support_unit(SupportUnitKindDto::CompleteQueryNegative)];

        assert!(!packet_evidence_was_bounded(&support));
        assert!(!packet_evidence_was_bounded(&[support_unit(
            SupportUnitKindDto::SourceRange
        )]));
    }

    #[test]
    fn packet_evidence_keeps_ranked_source_and_relation_context_in_compact_identities() {
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.id = "repository-shaped-source-id".to_owned();
        source.summary = "source range".to_owned();
        source.snippet = Some(format!("fn useful() {{}}\n{}", "x".repeat(700)));
        source.path = Some("src/useful.rs".to_owned());
        let source = packet_evidence_row(7, &source).expect("source evidence");
        assert_eq!(source.identity.evidence_id.as_str(), "packet-evidence-007");
        assert_eq!(source.summary.as_ref().unwrap().as_str().len(), 512);
        assert!(
            source
                .summary
                .as_ref()
                .unwrap()
                .as_str()
                .starts_with("fn useful()")
        );

        let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
        relation.from_symbol = Some("caller".to_owned());
        relation.edge_kind = Some("CALL".to_owned());
        relation.to_symbol = Some("callee".to_owned());
        let relation = packet_evidence_row(8, &relation).expect("relation evidence");
        assert_eq!(
            relation.summary.as_ref().unwrap().as_str(),
            "caller -[CALL]-> callee"
        );
    }

    #[test]
    fn source_page_actions_survive_long_paths_and_exhausted_broad_round() {
        use codestory_contracts::{
            api::EmbeddingVectorPublicationIdentityDto,
            compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1},
        };
        for path in [
            format!("src/{}reader.rs", "component/".repeat(40)),
            format!("src/{}reader.rs", "ééé/".repeat(70)),
        ] {
            let mut packet = continuation_progress_packet();
            packet.answer.retrieval_trace.retrieval_publication =
                Some(EmbeddingVectorPublicationIdentityDto {
                    core_generation_id: "core-generation-1".into(),
                    core_run_id: "core-run-1".into(),
                    retrieval_generation: "retrieval-generation-1".into(),
                    retrieval_input_hash: "a".repeat(64),
                    semantic_generation: "semantic-generation-1".into(),
                });
            let mut source = support_unit(SupportUnitKindDto::SourceRange);
            source.path = Some(path.clone());
            source.symbol_id = Some("7".into());
            source.start_line = Some(1);
            source.end_line = Some(1);
            source.snippet = Some("```text\n>    1 | fn reader() {\n```".into());
            source.source_delivery = Some(PacketSourceDeliveryV1 {
                requested_range: PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 20,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 700,
                next_start_line: Some(2),
                blocked_line_rendered_bytes: None,
            });
            packet.support = vec![source];
            let request: AgentPacketRequestDto =
                serde_json::from_value(json!({"question":"inspect source"})).unwrap();
            let product = project_packet_v3_using_record_builder("project-1", "test", &request, &packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()), |input| crate::agent::packet_execution_record_v3::build_packet_execution_record_fixture_v3(input, true)).unwrap();
            let PacketProjectionV3Dto::Complete {
                evidence,
                continuation,
                ..
            } = product.projection
            else {
                panic!("long path action fits");
            };
            assert!(continuation.is_none());
            let row = &evidence.as_slice()[0];
            assert_eq!(row.path.as_ref().unwrap().as_str(), path);
            let patch = row.packet_request_patch.as_ref().unwrap();
            let applied = patch.apply_to(&request).unwrap();
            assert_eq!(
                applied.parent_packet_id.as_deref(),
                Some(packet.packet_id.as_str())
            );
            assert!(applied.probes.is_empty());
            let dispatched = crate::agent::packet_compiler::continuation_probes_from_options(
                &applied,
                "project-1",
            )
            .unwrap();
            let codestory_contracts::api::PacketProbeDto::Continuation { selector, .. } =
                &dispatched[0]
            else {
                panic!("typed exact page action");
            };
            assert!(selector.path.is_none());
            assert_eq!(
                selector.source_range,
                Some(PacketSourceRangeV1 {
                    start_line: 2,
                    end_line: 20
                })
            );
        }
    }

    #[test]
    fn packet_source_number_compaction_keeps_source_indentation_and_rejects_bad_receipts() {
        let page = "```text\n>   99 |     café()\n   100 | \tassert(x)\n```";
        assert_eq!(
            compact_numbered_source_page(page, 99, 100).as_deref(),
            Some("```text\n>99 |     café()\n 100 | \tassert(x)\n```")
        );
        for (text, start, end) in [
            (page, 98, 100),
            (page, 99, 101),
            ("```text\n>   99 | a\n   101 | b\n```", 99, 100),
            ("```text\nsource without numbered lines\n```", 1, 1),
        ] {
            assert!(compact_numbered_source_page(text, start, end).is_none());
        }
    }

    #[test]
    #[ignore = "requires retained v429 public packet fixture paths"]
    fn packet_source_retained_v429_context_and_assertion_fit_without_coverage_loss() {
        let path = std::env::var("CODESTORY_CPU_LABEL_REPRESENTATION").unwrap();
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            fixture["schema"],
            "CodeStory-v429-source-coverage-regression/v430"
        );
        let initial: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture["initial"].as_str().unwrap()).unwrap())
                .unwrap();
        let mut continued: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture["continued"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(initial["publication"], continued["publication"]);
        let context = initial["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["path"] == "cron.go" && row["start_line"] == 239)
            .unwrap();
        assert_eq!(context["end_line"], 265);
        let rows = continued["evidence"].as_array_mut().unwrap();
        let current = rows
            .iter_mut()
            .find(|row| row["path"] == "cron.go" && row["start_line"] == 239)
            .unwrap();
        assert_eq!(
            current["end_line"], 258,
            "retained regression reproduces seven missing lines"
        );
        let identity = current["identity"].clone();
        *current = context.clone();
        current["identity"] = identity;
        let full_before = serde_json::to_vec(&continued).unwrap().len() + 1;
        assert!(
            full_before > 16384,
            "restoring old accepted context exceeds the original wire limit before compaction"
        );
        let source_lines = |text: &str| {
            text.strip_prefix("```text\n")
                .unwrap()
                .strip_suffix("\n```")
                .unwrap()
                .split('\n')
                .map(|line| {
                    let (prefix, source) = line.split_once(" | ").unwrap();
                    (
                        prefix
                            .trim_start_matches('>')
                            .trim()
                            .parse::<u32>()
                            .unwrap(),
                        source.to_owned(),
                    )
                })
                .collect::<Vec<_>>()
        };
        for row in continued["evidence"].as_array_mut().unwrap() {
            if row["kind"] != "exact_source" {
                continue;
            }
            let original = row["summary"].as_str().unwrap();
            let compact = compact_numbered_source_page(
                original,
                row["start_line"].as_u64().unwrap() as u32,
                row["end_line"].as_u64().unwrap() as u32,
            )
            .unwrap();
            assert_eq!(source_lines(original), source_lines(&compact));
            row["summary"] = compact.into();
        }
        let bytes = serde_json::to_vec(&continued).unwrap().len() + 1;
        assert!(bytes <= 16384);
        let assertion = continued["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["symbol_id"] == "7339483592651562902")
            .unwrap();
        assert_eq!(
            (
                assertion["start_line"].as_u64(),
                assertion["end_line"].as_u64()
            ),
            (Some(101), Some(153))
        );
        assert_eq!(assertion["source_delivery"]["remaining_source_bytes"], 0);
        println!(
            "retained-v429 restored-context-and-complete-assertion: original_wire={full_before}, compact_wire={bytes}, source_lines_identical=true"
        );
    }

    #[test]
    fn packet_source_page_projection_preserves_text_and_refuses_loss() {
        use codestory_contracts::compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1};
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.path = Some("src/page.rs".into());
        source.start_line = Some(1);
        source.end_line = Some(2);
        let page = format!("```text\n>    1 | {}\n     2 | done\n```", "é".repeat(300));
        source.snippet = Some(page.clone());
        source.source_delivery = Some(PacketSourceDeliveryV1 {
            requested_range: PacketSourceRangeV1 {
                start_line: 1,
                end_line: 2,
            },
            source_sha256: "a".repeat(64),
            remaining_source_bytes: 0,
            next_start_line: None,
            blocked_line_rendered_bytes: None,
        });
        let row = packet_evidence_row(0, &source).expect("bounded complete source page");
        assert_eq!(
            row.summary.as_ref().unwrap().as_str(),
            format!("```text\n>1 | {}\n 2 | done\n```", "é".repeat(300))
        );
        assert_eq!(source.snippet.as_deref(), Some(page.as_str()));
        assert_eq!(row.source_delivery, source.source_delivery);
        assert_eq!(row.end_line, Some(2));

        for hostile in ["oversized", "missing-source", "changed-bounds"] {
            let mut unit = source.clone();
            match hostile {
                "oversized" => {
                    unit.snippet = Some("x".repeat(
                        codestory_contracts::compilation::INTERIM_SOURCE_CONTINUATION_UPPER_BOUND
                            + 1,
                    ))
                }
                "missing-source" => unit.snippet = None,
                _ => unit.end_line = Some(1),
            }
            assert!(validate_packet_source_page(&unit).is_err(), "{hostile}");
            assert!(packet_evidence_row(0, &unit).is_none(), "{hostile}");
            let mut packet = continuation_progress_packet();
            packet.support = vec![unit];
            let request: AgentPacketRequestDto =
                serde_json::from_value(json!({"question":"show repository source"})).unwrap();
            assert!(
                project_packet_v3_using_record_builder(
                    "project-1",
                    "test",
                    &request,
                    &packet,
                    |_| Ok(0),
                    |_| panic!("invalid page must fail before record construction")
                )
                .is_err(),
                "{hostile}"
            );
        }
    }

    #[test]
    fn packet_evidence_prioritizes_source_excerpts_locations_and_relations() {
        let mut location = support_unit(SupportUnitKindDto::SymbolLocation);
        location.summary = "location".to_owned();
        location.path = Some("src/location.rs".to_owned());
        let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
        relation.from_symbol = Some("caller".to_owned());
        relation.edge_kind = Some("CALL".to_owned());
        relation.to_symbol = Some("callee".to_owned());
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.snippet = Some("fn useful() {}".to_owned());

        let rows = packet_evidence_rows(&[location, relation, source]);
        assert_eq!(
            rows.iter()
                .map(|row| row.summary.as_ref().unwrap().as_str())
                .collect::<Vec<_>>(),
            ["fn useful() {}", "caller -[CALL]-> callee", "location"]
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.identity.evidence_id.as_str())
                .collect::<Vec<_>>(),
            [
                "packet-evidence-000",
                "packet-evidence-001",
                "packet-evidence-002"
            ]
        );
        assert_eq!(rows[2].kind, EvidenceKindV3Dto::SourceLocation);
        assert_eq!((rows[2].start_line, rows[2].end_line), (None, None));
    }

    #[test]
    fn packet_evidence_deduplicates_identical_public_rows_before_applying_the_bound() {
        let duplicate = || {
            let mut unit = support_unit(SupportUnitKindDto::SourceRange);
            unit.path = Some("src/repeated.rs".to_owned());
            unit.symbol_id = Some("repeated".to_owned());
            unit.start_line = Some(7);
            unit.end_line = Some(9);
            unit.snippet = Some("fn repeated() {}".to_owned());
            unit
        };
        let mut support = (0..40)
            .map(|index| {
                let mut unit = duplicate();
                unit.id = format!("duplicate-{index}");
                unit
            })
            .collect::<Vec<_>>();
        let mut distinct = duplicate();
        distinct.id = "distinct".to_owned();
        distinct.path = Some("src/distinct.rs".to_owned());
        support.push(distinct);

        let evidence = packet_evidence_rows(&support);

        assert_eq!(evidence.len(), 2);
        assert_eq!(
            evidence
                .iter()
                .map(|row| row.path.as_ref().unwrap().as_str())
                .collect::<Vec<_>>(),
            ["src/repeated.rs", "src/distinct.rs"]
        );
        assert!(
            !packet_evidence_was_bounded(&support),
            "discarding duplicate renderings must not report missing evidence"
        );
    }

    #[test]
    fn packet_evidence_drops_a_location_when_the_same_symbol_has_source_content() {
        let mut location = support_unit(SupportUnitKindDto::SymbolLocation);
        location.path = Some("src/mapper.cs".to_owned());
        location.symbol_id = Some("Mapper.Map".to_owned());
        location.start_line = Some(12);
        let mut source = support_unit(SupportUnitKindDto::SourceRange);
        source.path = location.path.clone();
        source.symbol_id = location.symbol_id.clone();
        source.start_line = location.start_line;
        source.end_line = Some(18);
        source.snippet = Some("public object Map(...) => MapCore(...);".to_owned());

        let evidence = packet_evidence_rows(&[location, source]);

        assert_eq!(evidence.len(), 1);
        assert_eq!(
            evidence[0].summary.as_ref().unwrap().as_str(),
            "public object Map(...) => MapCore(...);"
        );
    }

    #[test]
    fn packet_evidence_relation_floor_spans_distinct_callers_before_repeats() {
        let repeated = (0..8).map(|index| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("repeated-{index}");
            unit.from_symbol = Some("one_caller".to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(format!("callee-{index}"));
            unit
        });
        let distinct = (0..4).map(|index| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("distinct-{index}");
            unit.from_symbol = Some(format!("caller-{index}"));
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(format!("target-{index}"));
            unit
        });

        let evidence = packet_evidence_rows(&repeated.chain(distinct).collect::<Vec<_>>());
        let first_callers = evidence
            .iter()
            .take(PACKET_PUBLIC_RELATION_ROWS_TARGET_V3)
            .filter_map(|row| {
                row.summary
                    .as_ref()
                    .and_then(|summary| summary.as_str().split(" -[").next())
            })
            .collect::<HashSet<_>>();

        assert_eq!(first_callers.len(), PACKET_PUBLIC_RELATION_ROWS_TARGET_V3);
    }

    #[test]
    fn packet_evidence_closes_a_diverse_content_bearing_identity_envelope() {
        let support = (0..15)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/source-{index}.rs"));
                unit.snippet = Some(format!("fn source_{index}() {{}}"));
                unit
            })
            .chain((0..24).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
                unit.id = format!("edge-{index}");
                unit.from_symbol = Some(format!("caller-{index}"));
                unit.edge_kind = Some("CALL".to_owned());
                unit.to_symbol = Some(format!("callee-{index}"));
                unit
            }))
            .chain((0..14).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SymbolLocation);
                unit.id = format!("location-{index}");
                unit.path = Some(format!("src/location-{index}.rs"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence = packet_evidence_rows(&support);

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            evidence
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::ExactSource)
                .count(),
            12,
            "the closed envelope keeps twelve source excerpts"
        );
        assert_eq!(
            evidence
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::GraphRelation)
                .count(),
            4,
            "the closed envelope reserves four relation receipts"
        );
        assert_eq!(
            evidence
                .iter()
                .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
                .collect::<Vec<_>>(),
            [
                "fn source_0() {}",
                "fn source_1() {}",
                "fn source_2() {}",
                "fn source_3() {}",
                "fn source_4() {}",
                "fn source_5() {}",
                "fn source_6() {}",
                "fn source_7() {}",
                "caller-0 -[CALL]-> callee-0",
                "caller-1 -[CALL]-> callee-1",
                "caller-2 -[CALL]-> callee-2",
                "caller-3 -[CALL]-> callee-3",
                "fn source_8() {}",
                "fn source_9() {}",
                "fn source_10() {}",
                "fn source_11() {}",
            ]
        );
        assert!(packet_evidence_was_bounded(&support));
    }

    #[test]
    fn packet_evidence_keeps_relevance_as_the_tie_breaker_inside_the_path_envelope() {
        let support = (0..8)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("route-stage-{index}");
                unit.path = Some("src/router.rs".to_owned());
                unit.symbol_id = Some(format!("route-stage-{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn route_stage_{index}() {{}}"));
                unit
            })
            .chain((0..8).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("noise-{index}");
                unit.path = Some(format!("src/noise-{index}.rs"));
                unit.symbol_id = Some(format!("noise-{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn noise_{index}() {{}}"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence =
            packet_evidence_rows_for_request(&support, "Explain the route stages in order.", &[]);
        let first_eight = evidence
            .iter()
            .take(8)
            .map(|row| {
                (
                    row.path.as_ref().map(|path| path.as_str()),
                    row.symbol_id.as_ref().map(|symbol| symbol.as_str()),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(first_eight[0].1, Some("route-stage-0"));
        assert_eq!(
            first_eight
                .iter()
                .filter_map(|(path, _)| *path)
                .collect::<HashSet<_>>()
                .len(),
            PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3,
            "path diversity owns the reserved envelope while relevance selects the best row for a repeated path: {first_eight:?}"
        );
    }

    #[test]
    fn packet_evidence_keeps_repository_order_without_external_reranking() {
        let mut support = (0..10)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("relevant-{index}");
                unit.path = Some(format!("src/relevant-{index}.rs"));
                unit.symbol_id = Some(format!("relevant-stage-{index}"));
                unit.snippet = Some(format!("fn relevant_stage_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();

        let mut node_carrier = support_unit(SupportUnitKindDto::SourceRange);
        node_carrier.id = "node-carrier".to_owned();
        node_carrier.path = Some("src/flow.rs".to_owned());
        node_carrier.symbol_id = Some("Flow.dispatch".to_owned());
        node_carrier.snippet = Some(format!(
            "fn dispatch() {{\n{}\nthis.router.handle(request);\n}}",
            "x".repeat(700)
        ));
        support.push(node_carrier);

        let mut path_carrier = support_unit(SupportUnitKindDto::SourceRange);
        path_carrier.id = "path-carrier".to_owned();
        path_carrier.path = Some("src/material-path.rs".to_owned());
        path_carrier.start_line = Some(4);
        path_carrier.end_line = Some(8);
        path_carrier.snippet = Some("material path source".to_owned());
        support.push(path_carrier);

        for index in 0..5 {
            let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
            relation.id = format!("edge:ordinary-{index}");
            relation.from_symbol = Some(format!("ordinary-caller-{index}"));
            relation.edge_kind = Some("CALL".to_owned());
            relation.to_symbol = Some(format!("ordinary-target-{index}"));
            support.push(relation);
        }
        let mut edge_carrier = support_unit(SupportUnitKindDto::TypedGraphEdge);
        edge_carrier.id = "edge:material-edge".to_owned();
        edge_carrier.from_symbol = Some("Flow.dispatch".to_owned());
        edge_carrier.edge_kind = Some("CALL".to_owned());
        edge_carrier.to_symbol = Some("Router.handle".to_owned());
        support.push(edge_carrier);

        let evidence = packet_evidence_rows(&support);

        assert_eq!(
            evidence[0].symbol_id.as_ref().unwrap().as_str(),
            "relevant-stage-0"
        );
        assert_eq!(
            evidence[1].symbol_id.as_ref().unwrap().as_str(),
            "relevant-stage-1"
        );
        assert!(
            evidence
                .iter()
                .find(|row| row
                    .symbol_id
                    .as_ref()
                    .is_some_and(|id| id.as_str() == "Flow.dispatch"))
                .expect("repository-ordered carrier remains available")
                .summary
                .as_ref()
                .unwrap()
                .as_str()
                .len()
                <= PACKET_EVIDENCE_SUMMARY_MAX_BYTES_V3,
            "external policy must not expand a source row's byte authority"
        );
        assert!(
            evidence
                .iter()
                .all(|row| row.summary.as_ref().is_none_or(|summary| {
                    summary.as_str() != "Flow.dispatch -[CALL]-> Router.handle"
                })),
            "a later edge must not displace earlier repository relations"
        );
        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
    }

    #[test]
    fn packet_evidence_keeps_each_material_source_with_an_exact_call_witness_under_the_cap() {
        let mut support = (0..8)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/flow-{index}.rs"));
                unit.symbol_id = Some(format!("Flow.stage{index}"));
                unit.snippet = Some(format!("fn stage_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        support[0].symbol_id = Some("Loop.drive".to_owned());
        support[1].symbol_id = Some("Command.route".to_owned());

        let relation = |id: &str, caller: &str, target: &str| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("edge:{id}");
            unit.from_symbol = Some(caller.to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(target.to_owned());
            unit
        };
        for index in 0..5 {
            support.push(relation(
                &format!("loop-{index}"),
                "Loop.drive",
                &format!("Loop.tick{index}"),
            ));
        }
        support.push(relation("command-route", "Command.route", "Command.check"));
        for index in 0..4 {
            support.push(relation(
                &format!("ordinary-{index}"),
                &format!("Ordinary{index}"),
                &format!("OrdinaryTarget{index}"),
            ));
        }

        let evidence = packet_evidence_rows(&support);
        let summaries = evidence
            .iter()
            .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
            .collect::<HashSet<_>>();
        let symbols = evidence
            .iter()
            .filter_map(|row| row.symbol_id.as_ref().map(|symbol| symbol.as_str()))
            .collect::<HashSet<_>>();

        assert!(symbols.contains("Loop.drive"));
        assert!(symbols.contains("Command.route"));
        assert!(summaries.contains("Loop.drive -[CALL]-> Loop.tick0"));
        assert!(summaries.contains("Command.route -[CALL]-> Command.check"));
        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
    }

    #[test]
    fn packet_evidence_keeps_repository_relations_for_retained_sources() {
        let relation = |id: &str, caller: &str, target: &str| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("edge:{id}");
            unit.from_symbol = Some(caller.to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(target.to_owned());
            unit
        };
        let mut support = (0..12)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/filler-{index}.rs"));
                unit.symbol_id = Some(format!("Filler{index}"));
                unit.snippet = Some(format!("fn filler_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        support[0].symbol_id = Some("Runtime.main".to_owned());
        support[1].symbol_id = Some("EventLoop.processEvents".to_owned());
        support.push(relation(
            "main-loop",
            "Runtime.main",
            "EventLoop.processEvents",
        ));
        support.push(relation(
            "iteration",
            "EventLoop.processEvents",
            "EventLoop.processTimeEvents",
        ));
        let evidence = packet_evidence_rows(&support);
        let summaries = evidence
            .iter()
            .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
            .collect::<HashSet<_>>();

        assert!(summaries.contains("Runtime.main -[CALL]-> EventLoop.processEvents"));
    }

    #[test]
    fn packet_evidence_keeps_the_closed_structural_mix() {
        let mut support = Vec::new();
        for index in 0..PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3 {
            let symbol = format!("Flow.stage{index}");
            let edge_id = format!("edge-{index}");
            let mut source = support_unit(SupportUnitKindDto::SourceRange);
            source.id = format!("source-{index}");
            source.path = Some(format!("src/stage-{index}.rs"));
            source.symbol_id = Some(symbol.clone());
            source.snippet = Some(format!("fn stage_{index}() {{}}"));
            support.push(source);

            let mut relation = support_unit(SupportUnitKindDto::TypedGraphEdge);
            relation.id = format!("edge:{edge_id}");
            relation.from_symbol = Some(symbol.clone());
            relation.edge_kind = Some("CALL".to_owned());
            relation.to_symbol = Some(format!("Flow.target{index}"));
            support.push(relation);
        }

        let evidence = packet_evidence_rows(&support);
        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            evidence
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::ExactSource)
                .count(),
            12
        );
        assert_eq!(
            evidence
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::GraphRelation)
                .count(),
            PACKET_PUBLIC_RELATION_ROWS_TARGET_V3
        );
    }

    #[test]
    fn packet_evidence_prefers_distinct_source_paths_before_repeats() {
        let mut support = (0..24)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("repeated-stage-{index}");
                unit.path = Some("src/repeated-flow.rs".to_owned());
                unit.symbol_id = Some(format!("RepeatedFlow.stage{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn dispatch_route_stage_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut unique_stage = support_unit(SupportUnitKindDto::SourceRange);
        unique_stage.id = "unique-material-stage".to_owned();
        unique_stage.path = Some("src/unique-material-stage.rs".to_owned());
        unique_stage.symbol_id = Some("UniqueMaterialStage.run".to_owned());
        unique_stage.start_line = Some(1);
        unique_stage.snippet = Some("fn run() {}".to_owned());
        support.push(unique_stage);

        let evidence = packet_evidence_rows(&support);

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            evidence
                .iter()
                .take(2)
                .filter_map(|row| row.path.as_ref().map(|path| path.as_str()))
                .collect::<HashSet<_>>(),
            HashSet::from(["src/repeated-flow.rs", "src/unique-material-stage.rs"]),
            "repeated ranges must not displace another path's source"
        );
    }

    #[test]
    fn packet_evidence_deduplicates_relations_without_external_reranking() {
        let relation = |id: &str, caller: &str, target: &str| {
            let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
            unit.id = format!("edge:{id}");
            unit.from_symbol = Some(caller.to_owned());
            unit.edge_kind = Some("CALL".to_owned());
            unit.to_symbol = Some(target.to_owned());
            unit
        };
        let mut support = vec![
            relation("a", "zirconium_plutonium", "a_target"),
            relation("shared-o0", "unrelated_shared", "shared_target"),
            // These two rows intentionally collapse to one public relation. Their distinct edge
            // identities must merge O0 and O1 onto the shared carrier before selection.
            relation("shared-o1", "unrelated_shared", "shared_target"),
            relation("c", "zirconium_plutonium_manganese", "c_target"),
        ];
        for index in 0..14 {
            let edge_id = format!("mandatory-{index}");
            support.push(relation(
                &edge_id,
                &format!("mandatory_caller_{index}"),
                &format!("mandatory_target_{index}"),
            ));
        }

        let evidence = packet_evidence_rows(&support);
        let summaries = evidence
            .iter()
            .filter_map(|row| row.summary.as_ref().map(|summary| summary.as_str()))
            .collect::<HashSet<_>>();

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert!(summaries.contains("zirconium_plutonium -[CALL]-> a_target"));
        assert!(summaries.contains("zirconium_plutonium_manganese -[CALL]-> c_target"));
        assert!(summaries.contains("unrelated_shared -[CALL]-> shared_target"));
    }

    #[test]
    fn packet_evidence_maximizes_distinct_source_paths_before_same_path_repeats() {
        let support = (0..24)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("repeated-path-{index}");
                unit.path = Some("src/repeated.rs".to_owned());
                unit.symbol_id = Some(format!("Repeated.stage{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(format!("fn repeated_route_stage_{index}() {{}}"));
                unit
            })
            .chain((0..7).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("distinct-path-{index}");
                unit.path = Some(format!("src/distinct-{index}.rs"));
                unit.symbol_id = Some(format!("Distinct{index}.run"));
                unit.start_line = Some(1);
                unit.snippet = Some(format!("fn run_{index}() {{}}"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence =
            packet_evidence_rows_for_request(&support, "Explain the repeated route stages.", &[]);
        let paths = evidence
            .iter()
            .filter_map(|row| row.path.as_ref().map(|path| path.as_str()))
            .collect::<HashSet<_>>();

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            paths.len(),
            PACKET_PUBLIC_SOURCE_ROWS_TARGET_V3,
            "same-path repeats must not consume the reserved distinct-path envelope"
        );
        for index in 0..7 {
            assert!(paths.contains(format!("src/distinct-{index}.rs").as_str()));
        }
    }

    #[test]
    fn packet_evidence_selection_is_deterministic_bounded_and_ignores_question_wording() {
        let support = (0..20)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/source-{index}.rs"));
                unit.symbol_id = Some(format!("stage-{index}"));
                unit.start_line = Some(index + 1);
                unit.snippet = Some(if index == 17 {
                    "fn selected_route_target() {}".to_owned()
                } else {
                    format!("fn unrelated_{index}() {{}}")
                });
                unit
            })
            .collect::<Vec<_>>();

        let first =
            packet_evidence_rows_for_request(&support, "Explain the selected route target.", &[]);
        let second =
            packet_evidence_rows_for_request(&support, "Describe unrelated components.", &[]);

        assert_eq!(first, second);
        assert_eq!(first.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(first[0].path.as_ref().unwrap().as_str(), "src/source-0.rs");
        assert_eq!(first[1].path.as_ref().unwrap().as_str(), "src/source-1.rs");
    }

    #[test]
    fn packet_evidence_does_not_prioritize_question_terms_inside_an_evidence_class() {
        let mut support = (0..10)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("unrelated-{index}");
                unit.path = Some(format!("src/unrelated-{index}.rs"));
                unit.symbol_id = Some(format!("unrelated-{index}"));
                unit.snippet = Some(format!("fn unrelated_{index}() {{}}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut relevant = support_unit(SupportUnitKindDto::SourceRange);
        relevant.id = "process-command".to_owned();
        relevant.path = Some("src/server.c".to_owned());
        relevant.symbol_id = Some("process-command".to_owned());
        relevant.summary = "source for processCommand".to_owned();
        relevant.snippet = Some("int processCommand(client *c) { return execute(c); }".to_owned());
        support.push(relevant);

        let evidence = packet_evidence_rows_for_request(
            &support,
            "Trace how the server routes a command for execution.",
            &[],
        );

        assert_eq!(
            evidence[0].path.as_ref().unwrap().as_str(),
            "src/unrelated-0.rs",
            "public capping must preserve the supplied evidence order rather than rerank from prompt words"
        );
    }

    #[test]
    fn packet_evidence_spends_unreserved_rows_on_the_most_relevant_evidence() {
        let support = (0..12)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("source-{index}");
                unit.path = Some(format!("src/source-{index}.rs"));
                unit.symbol_id = Some(format!("format_stage_{index}"));
                unit.snippet = Some(format!("fn format_stage_{index}() {{}}"));
                unit
            })
            .chain((0..8).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SymbolLocation);
                unit.id = format!("location-{index}");
                unit.path = Some(format!("src/location-{index}.rs"));
                unit.summary = format!("unrelated location {index}");
                unit
            }))
            .chain((0..8).map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::TypedGraphEdge);
                unit.id = format!("edge-{index}");
                unit.from_symbol = Some(format!("caller-{index}"));
                unit.edge_kind = Some("CALL".to_owned());
                unit.to_symbol = Some(format!("callee-{index}"));
                unit
            }))
            .collect::<Vec<_>>();

        let evidence =
            packet_evidence_rows_for_request(&support, "Explain the format stages.", &[]);

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert!(
            evidence
                .iter()
                .filter(|row| {
                    row.path
                        .as_ref()
                        .is_some_and(|path| path.as_str().starts_with("src/source-"))
                })
                .count()
                >= 12,
            "unreserved rows should prefer relevant source evidence over low-value duplicate locations"
        );
    }

    #[test]
    fn packet_evidence_does_not_rank_question_text_ahead_of_repository_evidence() {
        let mut support = (0..PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("downstream-{index}");
                unit.path = Some(format!("src/downstream-{index}.rs"));
                unit.symbol_id = Some(format!("Processor.handle{index}"));
                unit.snippet = Some(format!("fn handle_{index}() {{ process(); }}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut upstream = support_unit(SupportUnitKindDto::SourceRange);
        upstream.id = "public-registration".to_owned();
        upstream.path = Some("src/public-facade.rs".to_owned());
        upstream.symbol_id = Some("PublicFacade.registerRequest".to_owned());
        upstream.snippet = Some("fn register_request() { route.store(); }".to_owned());
        support.push(upstream);

        let evidence = packet_evidence_rows_for_request(
            &support,
            "Explain the complete lifecycle and public request registration.",
            &[],
        );

        assert_eq!(evidence.len(), PACKET_PUBLIC_EVIDENCE_ROWS_MAX_V3);
        assert_eq!(
            evidence[0].symbol_id.as_ref().map(|symbol| symbol.as_str()),
            Some("Processor.handle0"),
            "question text must not rerank the public evidence envelope"
        );
    }

    #[test]
    fn packet_evidence_does_not_rerank_from_continuation_diagnostic_text() {
        let mut support = (0..10)
            .map(|index| {
                let mut unit = support_unit(SupportUnitKindDto::SourceRange);
                unit.id = format!("base-{index}");
                unit.path = Some(format!("source/base-{index}.css"));
                unit.symbol_id = Some(format!("base-{index}"));
                unit.snippet = Some(format!(".base-{index} {{ animation-duration: 1s; }}"));
                unit
            })
            .collect::<Vec<_>>();
        let mut bounce = support_unit(SupportUnitKindDto::SourceRange);
        bounce.id = "bounce-keyframes".to_owned();
        bounce.path = Some("source/attention_seekers/bounce.css".to_owned());
        bounce.symbol_id = Some("bounce-keyframes".to_owned());
        bounce.summary = "source for @keyframes bounce".to_owned();
        bounce.snippet = Some("@keyframes bounce { from { transform: none; } }".to_owned());
        support.push(bounce);
        let option_id = codestory_contracts::api::encode_drill_option_id(
            codestory_contracts::api::DrillGapKindDto::OmittedMandatorySupport,
            "symbol:packet::css_import::source/attention_seekers/bounce.css::@keyframes bounce",
        );

        let evidence = packet_evidence_rows_for_request(
            &support,
            "Explain how the animation classes connect.",
            &[option_id],
        );

        assert_eq!(
            evidence[0].path.as_ref().unwrap().as_str(),
            "source/base-0.css"
        );
        assert!(
            evidence
                .iter()
                .any(|row| row.path.as_ref().is_some_and(|path| {
                    path.as_str() == "source/attention_seekers/bounce.css"
                })),
            "the continuation target remains evidence without gaining ranking authority"
        );
    }

    #[test]
    fn packet_symbol_locations_do_not_repeat_absolute_project_paths_in_summary_text() {
        let mut location = support_unit(SupportUnitKindDto::SymbolLocation);
        location.summary = "/private/project/src/lib.rs at src/lib.rs:7".to_owned();
        location.path = Some("src/lib.rs".to_owned());
        location.start_line = Some(7);
        location.end_line = Some(9);

        let row = packet_evidence_row(0, &location).expect("location evidence");
        assert_eq!(row.kind, EvidenceKindV3Dto::SourceLocation);
        assert_eq!(row.path.as_ref().unwrap().as_str(), "src/lib.rs");
        assert_eq!((row.start_line, row.end_line), (None, None));
        assert_eq!(row.summary.as_ref().unwrap().as_str(), "src/lib.rs:7");

        location.summary = "Navigation only: no bounded source range for node:17".into();
        let row = packet_evidence_row(1, &location).expect("bounded locator");
        assert_eq!(row.kind, EvidenceKindV3Dto::SourceLocation);
        assert!(
            row.summary
                .unwrap()
                .as_str()
                .starts_with("Navigation only:")
        );

        location.path = None;
        assert!(packet_evidence_row(2, &location).is_none());
    }

    #[test]
    fn context_projection_retains_the_focused_source_read() {
        use codestory_contracts::api::{
            AgentAskRequest, AgentResponseBlockDto, AgentResponseModeDto, AgentRetrievalPresetDto,
            AgentRetrievalProfileSelectionDto, AgentRetrievalStepKindDto,
            AgentRetrievalStepStatusDto, NodeId,
        };
        use codestory_contracts::graph::{Node, NodeId as CoreNodeId, NodeKind};
        use codestory_retrieval::{SidecarProfile, SidecarRuntimeConfig, test_support};
        use codestory_store::{
            FileInfo, FileRole, IndexPublicationMode, IndexPublicationRecord, Store,
        };
        use sha2::{Digest, Sha256};

        for source in [
            "function focused_source_anchor() {\n    return \"retained-source-body\";\n}\n"
                .to_owned(),
            format!(
                "function focused_source_anchor() {{\n    return \"retained-source-body{}\";\n}}\n",
                "界".repeat(12_000)
            ),
        ] {
            let project = tempfile::tempdir().expect("project");
            let state = tempfile::tempdir().expect("isolated state");
            let source_path = project.path().join("fixture.js");
            fs::write(&source_path, &source).expect("source");
            let storage_path = state.path().join("codestory.db");
            let mut store = Store::open(&storage_path).expect("store");
            let file = FileInfo {
                id: 10,
                path: source_path.clone(),
                language: "javascript".into(),
                modification_time: codestory_workspace::clamp_system_time_to_epoch_millis(
                    fs::metadata(&source_path)
                        .expect("metadata")
                        .modified()
                        .expect("mtime"),
                ),
                indexed: true,
                complete: true,
                line_count: 3,
                file_role: FileRole::Source,
            };
            store.insert_file(&file).expect("file");
            store
                .update_file_metadata(
                    &file,
                    Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
                )
                .expect("source hash");
            store
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(10),
                        kind: NodeKind::FILE,
                        serialized_name: source_path.to_string_lossy().into_owned(),
                        file_node_id: Some(CoreNodeId(10)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(20),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "focused_source_anchor".into(),
                        file_node_id: Some(CoreNodeId(10)),
                        start_line: Some(1),
                        end_line: Some(3),
                        ..Default::default()
                    },
                ])
                .expect("nodes");
            let publication = IndexPublicationRecord {
                generation: 1,
                generation_id: "11111111-1111-4111-8111-111111111111".into(),
                run_id: "focused-source-run".into(),
                mode: IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            };
            store
                .publish_structural_text_unit_generation(&publication)
                .expect("complete structural text unit publication");
            test_support::publish_complete_core_fixture(&mut store, project.path(), &publication)
                .expect("complete core");
            drop(store);
            let config =
                codestory_retrieval::with_test_cache_root(&state.path().join("retrieval"), || {
                    SidecarRuntimeConfig::for_project_profile(
                        Some(project.path()),
                        SidecarProfile::Agent,
                    )
                });
            test_support::publish_zero_dense_pinned_query_fixture(
                project.path(),
                &storage_path,
                &config,
            )
            .expect("test-only zero-dense retrieval publication");
            let controller = crate::AppController::new_with_config(config);
            {
                let mut controller_state = controller.state.lock();
                controller_state.project_root = Some(project.path().to_path_buf());
                controller_state.storage_path = Some(storage_path);
            }
            let service = crate::services::PublicOperationService::new(controller.clone());
            let operation = service
                .run_with_cancel("context", Arc::new(AtomicBool::new(false)), || {
                    let reads_before = crate::search_evidence::verified_source_read_count();
                    let mut request = AgentAskRequest {
                        prompt: "focused_source_anchor".into(),
                        retrieval_profile: AgentRetrievalProfileSelectionDto::Preset {
                            preset: AgentRetrievalPresetDto::Investigate,
                        },
                        focus_node_id: Some(NodeId("20".into())),
                        max_results: Some(8),
                        response_mode: AgentResponseModeDto::Structured,
                        latency_budget_ms: None,
                        include_evidence: true,
                        hybrid_weights: None,
                    };
                    let answer = controller.agent_ask(request.clone())?;
                    assert!(
                        answer.retrieval_trace.steps.iter().any(|step| {
                            step.kind == AgentRetrievalStepKindDto::SourceRead
                                && step.status == AgentRetrievalStepStatusDto::Ok
                        }),
                        "the real source-read branch must succeed before testing projection"
                    );
                    assert!(
                        answer
                            .sections
                            .iter()
                            .flat_map(|section| &section.blocks)
                            .any(|block| {
                                matches!(block, AgentResponseBlockDto::Markdown { markdown }
                        if markdown.contains("retained-source-body"))
                            }),
                        "the runtime already produced the source body"
                    );
                    let focused = answer
                        .focused_source
                        .as_ref()
                        .expect("typed focused source");
                    if source.len() <= 16 * 1024 {
                        assert_eq!((focused.start_line, focused.end_line), (1, 3));
                        assert_eq!(focused.excerpt, source.trim_end_matches('\n'));
                        assert!(!focused.truncated);
                    } else {
                        assert_eq!((focused.start_line, focused.end_line), (1, 2));
                        assert!(
                            focused.excerpt.len() <= 16 * 1024
                                && focused.excerpt.len() > 16 * 1024 - 4
                        );
                        assert!(focused.truncated);
                    }
                    let reads_after = crate::search_evidence::verified_source_read_count();
                    assert_eq!(
                        reads_after - reads_before,
                        1,
                        "one verified focused-file read"
                    );

                    let has_source = |projection: &ContextProjectionV3Dto| {
                        projection.evidence.as_slice().iter().any(|row| {
                            row.excerpt
                                .as_ref()
                                .is_some_and(|text| text.as_str().contains("retained-source-body"))
                        })
                    };
                    for mutation in [
                        "node",
                        "file",
                        "path",
                        "project",
                        "generation",
                        "run",
                        "hash",
                        "start",
                        "end",
                    ] {
                        let mut altered = answer.clone();
                        let source = altered.focused_source.as_mut().expect("source");
                        match mutation {
                            "node" => source.node_id = NodeId("10".into()),
                            "file" => source.file_id = 999,
                            "path" => {
                                source.path = project
                                    .path()
                                    .join("other.rs")
                                    .to_string_lossy()
                                    .into_owned()
                            }
                            "project" => source.project_id.push_str("-other"),
                            "generation" => source.core_generation_id.push_str("-other"),
                            "run" => source.core_run_id.push_str("-other"),
                            "hash" => source.content_sha256 = "0".repeat(64),
                            "start" => source.start_line = 0,
                            "end" => source.end_line = 20,
                            _ => unreachable!(),
                        }
                        let projected = project_context_v3(
                            &service,
                            "test",
                            source_path.to_str(),
                            Some("20"),
                            &altered,
                        )?;
                        assert!(!has_source(&projected), "reject mismatched {mutation}");
                        assert!(
                            projected
                                .gaps
                                .as_slice()
                                .iter()
                                .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable)
                        );
                    }
                    for (path, id) in [
                        (Some("other.rs"), Some("20")),
                        (source_path.to_str(), Some("10")),
                    ] {
                        let projected = project_context_v3(&service, "test", path, id, &answer)?;
                        assert!(
                            !has_source(&projected),
                            "reject a different requested target"
                        );
                    }
                    let mut with_excerpt = answer.clone();
                    with_excerpt.citations.push(serde_json::from_value(json!({
                    "node_id":"20", "display_name":"focused_source_anchor", "kind":"FUNCTION",
                    "file_path":source_path, "line":1, "score":1.0, "origin":"indexed_symbol",
                    "resolvable":true, "source_excerpt":"existing citation excerpt"
                })).expect("citation"));
                    let projected = project_context_v3(
                        &service,
                        "test",
                        source_path.to_str(),
                        Some("20"),
                        &with_excerpt,
                    )?;
                    assert!(has_source(&projected));
                    assert!(
                        projected.evidence.as_slice().iter().any(|row| {
                            row.excerpt
                                .as_ref()
                                .is_some_and(|text| text.as_str() == "existing citation excerpt")
                        }),
                        "existing citation excerpts remain intact"
                    );

                    let round_trip: AgentAnswerDto = serde_json::from_value(
                        serde_json::to_value(&answer).expect("serialize answer"),
                    )
                    .expect("deserialize answer");
                    let projection = project_context_v3(
                        &service,
                        "test",
                        source_path.to_str(),
                        Some("20"),
                        &round_trip,
                    )?;
                    assert_eq!(
                        crate::search_evidence::verified_source_read_count(),
                        reads_after,
                        "projection and internal transport never reopen the source"
                    );
                    let mut failed = answer.clone();
                    failed.focused_source = None;
                    failed
                        .retrieval_trace
                        .steps
                        .iter_mut()
                        .filter(|step| step.kind == AgentRetrievalStepKindDto::SourceRead)
                        .for_each(|step| step.status = AgentRetrievalStepStatusDto::Error);
                    let failed = project_context_v3(
                        &service,
                        "test",
                        source_path.to_str(),
                        Some("20"),
                        &failed,
                    )?;
                    assert!(!has_source(&failed));
                    assert!(
                        failed
                            .gaps
                            .as_slice()
                            .iter()
                            .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable)
                    );

                    request.retrieval_profile = AgentRetrievalProfileSelectionDto::Custom {
                        config: codestory_contracts::api::AgentCustomRetrievalConfigDto {
                            enable_source_reads: false,
                            ..Default::default()
                        },
                    };
                    let skipped = controller.agent_ask(request)?;
                    assert!(skipped.focused_source.is_none());
                    assert!(
                        skipped
                            .retrieval_trace
                            .steps
                            .iter()
                            .any(|step| step.kind == AgentRetrievalStepKindDto::SourceRead
                                && step.status == AgentRetrievalStepStatusDto::Skipped)
                    );
                    let skipped = project_context_v3(
                        &service,
                        "test",
                        source_path.to_str(),
                        Some("20"),
                        &skipped,
                    )?;
                    assert!(!has_source(&skipped));
                    assert!(
                        !skipped
                            .gaps
                            .as_slice()
                            .iter()
                            .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable)
                    );
                    assert_eq!(
                        crate::search_evidence::verified_source_read_count(),
                        reads_after
                    );
                    Ok(projection)
                })
                .expect("real context operation");
            assert!(
                operation.value.evidence.as_slice().iter().any(|row| {
                    row.symbol_id.as_ref().is_some_and(|id| id.as_str() == "20")
                        && row.excerpt.as_ref().is_some_and(|excerpt| {
                            excerpt.as_str().contains("retained-source-body")
                        })
                }),
                "v3 must retain the focused source the runtime already read"
            );
            if source.len() > 8 * 1024 {
                assert!(
                    operation
                        .value
                        .gaps
                        .as_slice()
                        .iter()
                        .any(|gap| gap.kind == GapKindV3Dto::OutputBudgetExceeded)
                );
                let row = operation
                    .value
                    .evidence
                    .as_slice()
                    .iter()
                    .find(|row| {
                        row.excerpt
                            .as_ref()
                            .is_some_and(|text| text.as_str().contains("retained-source-body"))
                    })
                    .expect("source row");
                assert_eq!((row.start_line, row.end_line), (Some(1), Some(2)));
                assert!(row.excerpt.as_ref().expect("excerpt").as_str().len() <= 8 * 1024);
            }
        }
    }

    #[test]
    fn context_projection_keeps_the_resolved_target_with_or_without_citations() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project");
        // This projection fixture consumes a resolved target; it does not resolve
        // symbols. A source-only file gives a real zero-dense Full publication.
        fs::create_dir_all(project.path().join("tests")).expect("test source directory");
        let source = project.path().join("tests/resolved.rs");
        fs::write(&source, "// source for the resolved request target\n").expect("source");
        let storage = project.path().join("codestory.db");
        let controller = crate::AppController::new_with_owned_cache_root(process_cache.path());
        controller
            .open_project_summary_with_storage_path(project.path().to_path_buf(), storage.clone())
            .expect("open project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index project");
        drop(controller);
        let (_controller, service) =
            full_projection_fixture_services(project.path(), &storage, process_cache.path());

        for citations in [
            json!([]),
            json!([{
                "node_id":"citation-node",
                "display_name":"different_citation",
                "kind":"FUNCTION",
                "file_path":"different/citation.rs",
                "line":7,
                "score":1.0,
                "origin":"indexed_symbol",
                "resolvable":true,
                "evidence_edge_ids":[]
            }]),
        ] {
            let answer: AgentAnswerDto = serde_json::from_value(json!({
                "answer_id":"context-answer",
                "prompt":"resolved_target",
                "summary":"fixture",
                "source_coverage":[],
                "sections":[],
                "citations":citations,
                "subgraph_ids":[],
                "retrieval_version":"fixture",
                "graphs":[],
                "retrieval_trace":{
                    "request_id":"context-request",
                    "resolved_profile":"investigate",
                    "policy_mode":"latency_first",
                    "total_latency_ms":0,
                    "sla_missed":false,
                    "semantic_fallback_count":0,
                    "semantic_fallbacks":[],
                    "semantic_stage_timeout_zero_hits":0,
                    "semantic_abstained_count":0,
                    "annotations":[],
                    "steps":[],
                    "packet_sidecar_diagnostics":[]
                }
            }))
            .expect("answer fixture");
            let projection = service
                .run_observational_with_cancel("context", Arc::new(AtomicBool::new(false)), || {
                    let mut answer = answer.clone();
                    answer.retrieval_trace.retrieval_publication = service
                        .active_publication()
                        .expect("Full operation publication")
                        .retrieval_publication;
                    assert!(answer.retrieval_trace.retrieval_publication.is_some());
                    project_context_v3(
                        &service,
                        "test",
                        Some("tests/resolved.rs"),
                        Some("resolved-node"),
                        &answer,
                    )
                })
                .expect("project context")
                .value;

            assert_eq!(
                projection.target.path.as_ref().map(PathTextV3::as_str),
                Some("tests/resolved.rs"),
                "a citation cannot replace the already resolved request target"
            );
            assert_eq!(
                projection
                    .target
                    .symbol_id
                    .as_ref()
                    .map(SymbolIdTextV3::as_str),
                Some("resolved-node")
            );
        }
    }

    #[test]
    fn context_projection_preserves_path_only_unresolved_uncertainty() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project");
        // This projection fixture consumes a resolved target; it does not resolve
        // symbols. A source-only file gives a real zero-dense Full publication.
        fs::create_dir_all(project.path().join("tests")).expect("test source directory");
        let source = project.path().join("tests/unresolved.rs");
        fs::write(&source, "// source for the path-only request target\n").expect("source");
        let storage = project.path().join("codestory.db");
        let controller = crate::AppController::new_with_owned_cache_root(process_cache.path());
        controller
            .open_project_summary_with_storage_path(project.path().to_path_buf(), storage.clone())
            .expect("open project");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index project");
        drop(controller);
        let (_controller, service) =
            full_projection_fixture_services(project.path(), &storage, process_cache.path());
        let answer: AgentAnswerDto = serde_json::from_value(json!({
            "answer_id":"context-answer",
            "prompt":"unresolved_target",
            "summary":"fixture",
            "source_coverage":[],
            "sections":[],
            "citations":[{
                "node_id":"unresolved-node",
                "display_name":"unresolved_target",
                "kind":"FUNCTION",
                "file_path":"tests/unresolved.rs",
                "line":null,
                "score":1.0,
                "origin":"indexed_symbol",
                "resolvable":false,
                "evidence_edge_ids":[]
            }],
            "subgraph_ids":[],
            "retrieval_version":"fixture",
            "graphs":[],
            "retrieval_trace":{
                "request_id":"context-request",
                "resolved_profile":"investigate",
                "policy_mode":"latency_first",
                "total_latency_ms":0,
                "sla_missed":false,
                "semantic_fallback_count":0,
                "semantic_fallbacks":[],
                "semantic_stage_timeout_zero_hits":0,
                "semantic_abstained_count":0,
                "annotations":[],
                "steps":[],
                "packet_sidecar_diagnostics":[]
            }
        }))
        .expect("answer fixture");
        let projection = service
            .run_observational_with_cancel("context", Arc::new(AtomicBool::new(false)), || {
                let mut answer = answer.clone();
                answer.retrieval_trace.retrieval_publication = service
                    .active_publication()
                    .expect("Full operation publication")
                    .retrieval_publication;
                assert!(answer.retrieval_trace.retrieval_publication.is_some());
                project_context_v3(&service, "test", Some("tests/unresolved.rs"), None, &answer)
            })
            .expect("project context")
            .value;
        let serialized = serde_json::to_value(projection).expect("serialize context");
        let evidence = &serialized["evidence"][0];

        assert_eq!(evidence["path"], json!("tests/unresolved.rs"));
        assert_eq!(evidence["start_line"], serde_json::Value::Null);
        assert_eq!(evidence["end_line"], serde_json::Value::Null);
        assert_eq!(evidence["symbol_id"], serde_json::Value::Null);
    }
}

/// Fixtures use the production compiler, shared proposal application, normal
/// validated record construction and public finalizer; only hydration and the
/// captured publication are supplied by the test. No private runtime is opened.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod representation_budget_test_support {
    pub const MAX_TRANSPORT_MEASUREMENTS: usize = super::PACKET_TRANSPORT_MEASUREMENTS_MAX_V1;
    pub const MAX_MEASURED_CANDIDATES: usize =
        8 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1 + 1;

    use super::*;
    use crate::agent::packet_compiler::{
        PreparedPacketCompilationV1, commit_measured_packet_compilation,
    };
    use codestory_contracts::api::*;
    use codestory_contracts::compilation::PacketCompilationInputV1;

    fn empty_packet(question: &str, max_output_bytes: u32) -> AgentPacketDto {
        let answer = AgentAnswerDto {
            focused_source: None,
            source_coverage: Vec::new(),
            answer_id: "packet-budget-test".to_string(),
            prompt: question.to_string(),
            summary: "Packet budget test answer.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Bounded source-backed evidence.".to_string(),
                }],
            }],
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "packet-budget-test".to_string(),
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
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        };
        let budget = PacketBudgetDto {
            requested: PacketBudgetModeDto::Compact,
            limits: PacketBudgetLimitsDto {
                max_anchors: 16,
                max_files: 16,
                max_snippets: 16,
                max_trail_edges: 20,
                max_output_bytes,
            },
            used: PacketBudgetUsageDto {
                anchors: 0,
                files: 0,
                snippets: 0,
                trail_edges: 0,
                output_bytes: 0,
            },
            truncated: false,
            omitted_sections: Vec::new(),
            next_deeper_command: None,
        };
        let retrieval_trace_summary = PacketRetrievalTraceSummaryDto {
            retrieval_trace: answer.retrieval_trace.clone(),
            source_read_steps: 0,
            search_steps: 0,
            trail_steps: 0,
        };

        AgentPacketDto {
            packet_id: answer.answer_id.clone(),
            question: question.to_string(),
            plan: PacketPlanDto {
                queries: Vec::new(),
                probe_resolutions: Vec::new(),
                trace: Vec::new(),
            },
            answer,
            budget,
            support: Vec::new(),
            disposition: PacketDispositionDto::supported(),
            retrieval_trace_summary,
            answer_sufficiency: Default::default(),
        }
    }

    fn shell(request: &AgentPacketRequestDto) -> AgentPacketDto {
        let mut packet = empty_packet(&request.question, 16_384);
        packet.answer.freshness = Some(crate::agent::packet_freshness::fresh_index_observation());
        packet.answer.retrieval_trace.retrieval_publication =
            Some(EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-generation-1".into(),
                core_run_id: "core-run-1".into(),
                retrieval_generation: "retrieval-generation-1".into(),
                retrieval_input_hash:
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                semantic_generation: "semantic-generation-1".into(),
            });
        packet.retrieval_trace_summary.retrieval_trace = packet.answer.retrieval_trace.clone();
        packet
    }

    pub fn default_packet(
        input: PacketCompilationInputV1,
        request: &AgentPacketRequestDto,
    ) -> (AgentPacketDto, usize) {
        let prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        let compiled = prepared.default_product();
        let support_bytes = serde_json::to_vec(&compiled.support).unwrap().len();
        let mut packet = shell(request);
        prepared.apply_proposal(&mut packet, Some(request), &compiled);
        (packet, support_bytes)
    }

    pub fn project_packet(
        request: &AgentPacketRequestDto,
        packet: &AgentPacketDto,
        measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
    ) -> Result<PacketEvidenceProductV3, ApiError> {
        project_packet_v3_using_record_builder(
            "project-1",
            "codestory-stdio",
            request,
            packet,
            measure,
            |input| {
                crate::agent::packet_execution_record_v3::build_packet_execution_record_fixture_v3(
                    input,
                    packet
                        .answer
                        .retrieval_trace
                        .retrieval_publication
                        .is_some(),
                )
            },
        )
    }

    /// Full captured-state CPU consumer. No project/controller/service is opened.
    pub fn replay_captured(
        capture: &serde_json::Value,
        public_packet_id: &str,
        measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
    ) -> Result<(RepresentedPacketProductV3, Vec<serde_json::Value>), ApiError> {
        replay_captured_observed(capture, public_packet_id, measure, |_, _| {})
    }

    pub fn replay_captured_observed(
        capture: &serde_json::Value,
        public_packet_id: &str,
        measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
        mut observe: impl FnMut(&RepresentedPacketProductV3, usize),
    ) -> Result<(RepresentedPacketProductV3, Vec<serde_json::Value>), ApiError> {
        let mut measure = bounded_packet_transport_measure_v3(measure);
        use crate::agent::{
            orchestrator::append_compiled_source_evidence,
            packet_budget::enforce_packet_output_budget, packet_replay::replay_digest,
        };
        let prepared: PreparedPacketCompilationV1 =
            serde_json::from_value(capture["input"]["prepared"].clone())
                .map_err(|e| ApiError::internal(e.to_string()))?;
        let request: AgentPacketRequestDto =
            serde_json::from_value(capture["input"]["request"].clone())
                .map_err(|e| ApiError::internal(e.to_string()))?;
        let base: AgentPacketDto = serde_json::from_value(capture["input"]["base_packet"].clone())
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let publication: codestory_contracts::compilation::PacketCompilationPublicationV1 =
            serde_json::from_value(capture["input"]["scope"]["publication"].clone())
                .map_err(|e| ApiError::internal(e.to_string()))?;
        let run = capture["input"]["scope"]["core_run_id"]
            .as_str()
            .ok_or_else(|| ApiError::internal("replay Core run missing"))?;
        let root = std::path::Path::new(
            capture["input"]["scope"]["project_root"]
                .as_str()
                .ok_or_else(|| ApiError::internal("replay root missing"))?,
        );
        let navigation: Vec<codestory_contracts::compilation::PacketDirectedRelationV1> =
            serde_json::from_value(
                capture["input"]["prepared"]["call_navigation"]
                    .as_array()
                    .map(|rows| serde_json::json!(rows))
                    .unwrap_or_else(|| serde_json::json!([])),
            )
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let mut rows = Vec::new();
        let mut full = None;
        let mut accepted = None;
        let mut baseline_projection = None;
        let mut baseline_snapshot_recorded = false;
        let frozen = prepared.select_for_representation_with_baseline(|prepared, compiled, baseline| {
            let mut packet = base.clone();
            prepared.apply_proposal(&mut packet, Some(&request), compiled);
            append_compiled_source_evidence(&mut packet.answer, &compiled.support);
            let before = packet.clone();
            enforce_packet_output_budget(root, &mut packet);
            if packet.support != compiled.support { return Err(ApiError::internal("replay internal support changed")); }
            let baseline_packet = baseline.map(|compiled_baseline| {
                let mut baseline_packet = base.clone();
                prepared.apply_proposal(&mut baseline_packet, Some(&request), compiled_baseline);
                append_compiled_source_evidence(&mut baseline_packet.answer, &compiled_baseline.support);
                enforce_packet_output_budget(root, &mut baseline_packet);
                if baseline_packet.support != compiled_baseline.support { return Err(ApiError::internal("replay baseline support changed")); }
                Ok(baseline_packet)
            }).transpose()?;
            if baseline_packet.as_ref().is_some_and(|base| !prepared.retain_baseline_continuation_actions(base, &mut packet)) { return Ok(None); }
            let product = project_packet_v3_using_record_builder(&publication.project_id, "codestory-stdio", &request, &packet, &mut measure,
                |input| crate::agent::packet_execution_record_v3::build_packet_execution_record_replay_v3(input, &publication, run, packet.answer.retrieval_trace.retrieval_publication.clone(), public_packet_id))?;
            let guard_ok = if let Some(baseline_packet) = baseline_packet.as_ref() {
                if baseline_projection.is_none() {
                    let projected = if baseline_packet.support == packet.support && baseline_packet.disposition == packet.disposition {
                        product.clone()
                    } else {
                        project_packet_v3_using_record_builder(&publication.project_id, "codestory-stdio", &request,
                            baseline_packet, &mut measure, |input| crate::agent::packet_execution_record_v3::build_packet_execution_record_replay_v3(
                                input, &publication, run, baseline_packet.answer.retrieval_trace.retrieval_publication.clone(), public_packet_id))?
                    };
                    baseline_projection = Some(projected.projection);
                }
                packet_projection_preserves_baseline_v3(baseline_projection.as_ref().unwrap(),
                    &product.projection, &prepared.resolved_baseline_option_witnesses(baseline_packet, &packet),
                    &packet.support.iter().filter(|row| row.kind == SupportUnitKindDto::SourceRange && !baseline_packet.support.contains(row)).cloned().collect::<Vec<_>>())
            } else { true };
            let bytes = if guard_ok && matches!(product.projection, PacketProjectionV3Dto::Complete { .. }) {
                Some(measure(&product.projection).map_err(|_| ApiError::internal("replay measurement error"))?)
            } else { None };
            let baseline_snapshot = if baseline.is_some() && !baseline_snapshot_recorded {
                baseline_snapshot_recorded = true;
                Some(product.projection.clone())
            } else { None };
            rows.push(serde_json::json!({"same_request_baseline_projection":baseline_snapshot,"ordinal":rows.len(), "extent_sha256":replay_digest(prepared.replay_input()),
                "selection_phase":if baseline.is_some(){"joint"}else{"baseline"},"served_baseline_preserved":guard_ok,"support_rows":before.support.len(), "support_sha256":replay_digest(&before.support),
                "selection_rank":format!("{:?}", codestory_agent::evidence_compiler::repository_evidence_selection_rank(prepared.replay_input(), &navigation, compiled)),
                "support_frontier":before.support.iter().map(|row| serde_json::json!({"id":row.id,
                    "symbol_id":row.symbol_id,"start":row.start_line,"end":row.end_line,
                    "snippet_bytes":row.snippet.as_ref().map_or(0,String::len)})).collect::<Vec<_>>(),
                "before_internal_budget_sha256":replay_digest(&before), "after_internal_budget_sha256":replay_digest(&packet),
                "before_internal_budget_bytes":serde_json::to_vec(&before).unwrap().len(), "after_internal_budget_bytes":serde_json::to_vec(&packet).unwrap().len(),
                "internal_support_unchanged":before.support==packet.support,
                "outcome":bytes.map_or_else(||serde_json::json!({"kind":"refused"}),|n|serde_json::json!({"kind":"measured","bytes":n}))}));
            let proposal = RepresentedPacketProductV3 {packet,product};
            if full.is_none() { full=Some(proposal.clone()); }
            if let Some(n)=bytes.filter(|n|*n<=16_384) {observe(&proposal,n);accepted=Some(proposal);}
            Ok(bytes)
        })?;
        let selected = accepted
            .or(full)
            .ok_or_else(|| ApiError::internal("replay had no proposals"))?;
        commit_measured_packet_compilation(&selected.packet, frozen)?;
        Ok((selected, rows))
    }

    pub fn replay_with_published_call_metadata(
        capture: &serde_json::Value,
        core: &std::path::Path,
        owned_fixture: &std::path::Path,
    ) -> Result<serde_json::Value, ApiError> {
        use codestory_contracts::compilation::*;
        let root = std::path::Path::new(
            capture["input"]["scope"]["project_root"]
                .as_str()
                .ok_or_else(|| ApiError::internal("root missing"))?,
        );
        let publication: PacketCompilationPublicationV1 =
            serde_json::from_value(capture["input"]["scope"]["publication"].clone())
                .map_err(|e| ApiError::internal(e.to_string()))?;
        let ranked: Vec<PacketCandidateDescriptorV1> = serde_json::from_value(
            capture["input"]["descriptor_queries"]["ranked_eligible_descriptors"].clone(),
        )
        .map_err(|e| ApiError::internal(e.to_string()))?;
        let input_hash=capture["input"]["descriptor_queries"]["queries"][0]["publication"]["sidecar_input_hash"].as_str().ok_or_else(||ApiError::internal("input hash missing"))?;
        let queries = capture["input"]["descriptor_queries"]["queries"]
            .as_array()
            .ok_or_else(|| ApiError::internal("queries missing"))?
            .iter()
            .map(|q| {
                let policy = match q["lexical_policy"].as_str() {
                    Some("BroadQuestionV1") => {
                        Ok(codestory_retrieval::LexicalAdmissionPolicy::BroadQuestionV1)
                    }
                    Some("StrictLiteralV1") => {
                        Ok(codestory_retrieval::LexicalAdmissionPolicy::StrictLiteralV1)
                    }
                    _ => Err(ApiError::internal("captured policy unknown")),
                }?;
                Ok((
                    q["query"]
                        .as_str()
                        .ok_or_else(|| ApiError::internal("query missing"))?
                        .to_owned(),
                    policy,
                ))
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let metadata = codestory_retrieval::replay_published_call_metadata(
            root,
            core,
            &owned_fixture.join("lexical"),
            &publication.project_id,
            input_hash,
            &ranked,
            &queries,
        )
        .map_err(|e| ApiError::internal(e.to_string()))?;
        let (admissions, call_navigation) =
            crate::agent::retrieval_primary::replay_published_call_admissions(
                &metadata, input_hash,
            );
        let prepared = crate::agent::packet_compiler::replay_prepare_from_sealed_admissions(
            root,
            core,
            &owned_fixture.join("cache"),
            publication,
            admissions,
            call_navigation,
        )?;
        let mut result = capture.clone();
        result["input"]["prepared"] =
            serde_json::to_value(prepared).map_err(|e| ApiError::internal(e.to_string()))?;
        result["input"]["descriptor_queries"]["ranked_eligible_descriptors"] =
            serde_json::to_value(metadata).map_err(|e| ApiError::internal(e.to_string()))?;
        // CPU-only counterfactual: no leased/live publication is modified or
        // declared Full. Core and scores remain captured; normal new metadata
        // producer+reader, fusion, seal and hydration supply the changed input.
        Ok(result)
    }

    pub fn select_prepared(
        input: PacketCompilationInputV1,
        request: &AgentPacketRequestDto,
        measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
    ) -> Result<RepresentedPacketProductV3, ApiError> {
        let mut measure = bounded_packet_transport_measure_v3(measure);
        let prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        let shell = shell(request);
        let mut full = None;
        let mut accepted = None;
        let mut baseline_projection = None;
        let frozen =
            prepared.select_for_representation_with_baseline(|prepared, compiled, baseline| {
                let mut packet = shell.clone();
                prepared.apply_proposal(&mut packet, Some(request), compiled);
                let baseline_packet = baseline
                    .map(|compiled_baseline| {
                        let mut base = shell.clone();
                        prepared.apply_proposal(&mut base, Some(request), compiled_baseline);
                        Ok(base)
                    })
                    .transpose()?;
                if baseline_packet.as_ref().is_some_and(|base| {
                    !prepared.retain_baseline_continuation_actions(base, &mut packet)
                }) {
                    return Ok(None);
                }
                let product = project_packet(request, &packet, &mut measure)?;
                let preserved = if let Some(base) = baseline_packet.as_ref() {
                    if baseline_projection.is_none() {
                        let projected = if base.support == packet.support
                            && base.disposition == packet.disposition
                        {
                            product.clone()
                        } else {
                            project_packet(request, base, &mut measure)?
                        };
                        baseline_projection = Some(projected.projection);
                    }
                    packet_projection_preserves_baseline_v3(
                        baseline_projection.as_ref().unwrap(),
                        &product.projection,
                        &prepared.resolved_baseline_option_witnesses(base, &packet),
                        &packet
                            .support
                            .iter()
                            .filter(|row| {
                                row.kind == SupportUnitKindDto::SourceRange
                                    && !base.support.contains(row)
                            })
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                } else {
                    true
                };
                let bytes = if preserved
                    && matches!(&product.projection, PacketProjectionV3Dto::Complete { .. })
                {
                    Some(
                        measure(&product.projection)
                            .map_err(|_| ApiError::internal("fixture measure failed"))?,
                    )
                } else {
                    None
                };
                let proposal = RepresentedPacketProductV3 { packet, product };
                if full.is_none() {
                    full = Some(proposal.clone());
                }
                if bytes.is_some_and(|bytes| bytes <= 16_384) {
                    accepted = Some(proposal);
                }
                Ok(bytes)
            })?;
        let selected = accepted
            .or(full)
            .ok_or_else(|| ApiError::internal("fixture had no proposal"))?;
        commit_measured_packet_compilation(&selected.packet, frozen)?;
        Ok(selected)
    }
}
