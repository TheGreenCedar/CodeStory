//! Immutable v3 packet execution capture.

// Public evidence projection and sealed qualification both consume this
// capture; neither may mutate the finalized execution it records.
#![allow(dead_code)]

use std::collections::BTreeSet;

use codestory_contracts::{
    api::{
        AgentPacketRequestDto, ApiError, EmbeddingVectorPublicationIdentityDto,
        PacketBudgetModeDto, PacketProbeDto,
    },
    packet_projection_v3::{
        ContinuationStateV3Dto, CorePublicationIdentityV3Dto, DIAGNOSTIC_ROWS_MAX_V3,
        DiagnosticCategoryV3Dto, DiagnosticCodeTextV3, EvidenceIdentityV3Dto, GAP_ROWS_MAX_V3,
        GapIdentityV3Dto, IdentityTextV3, PACKET_EVIDENCE_ROWS_MAX_V3, PacketEvidenceRowV3Dto,
        ProjectionGapRowV3Dto, REFERENCE_ROWS_MAX_V3, RetrievalPublicationIdentityV3Dto,
        RetrievalStateDescriptorV3Dto, RetrievalStateV3Dto, Sha256DigestV3Dto,
    },
};
use codestory_workspace::ProjectIdentityV3;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::{Uuid, Variant};

use crate::services::PublicOperationService;

const REQUEST_DIGEST_DOMAIN_V3: &[u8] = b"codestory.packet_execution_record_v3.request\0";
pub(crate) const PACKET_EXECUTION_PLAN_VERSION_V3: u32 = 3;

pub(crate) fn canonical_json_bytes_v3<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json_canonicalizer::to_vec(value).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PacketRequestFingerprintV3 {
    question: String,
    budget: PacketBudgetModeDto,
    typed_probes: Vec<PacketProbeDto>,
    latency_budget_ms: Option<u32>,
    parent_packet_id: Option<String>,
    option_ids: Vec<String>,
    core_generation_id: Option<String>,
    retrieval_generation: Option<String>,
}

impl PacketRequestFingerprintV3 {
    pub(crate) fn from_current_request(request: &AgentPacketRequestDto) -> Self {
        Self {
            question: request.question.clone(),
            budget: request.budget,
            typed_probes: request.probes.clone(),
            latency_budget_ms: request.latency_budget_ms,
            parent_packet_id: request.parent_packet_id.clone(),
            option_ids: request.option_ids.clone(),
            core_generation_id: request.core_generation_id.clone(),
            retrieval_generation: request.retrieval_generation.clone(),
        }
    }

    pub(crate) fn question(&self) -> &str {
        &self.question
    }

    pub(crate) fn budget(&self) -> PacketBudgetModeDto {
        self.budget
    }

    pub(crate) fn typed_probes(&self) -> &[PacketProbeDto] {
        &self.typed_probes
    }

    pub(crate) fn latency_budget_ms(&self) -> Option<u32> {
        self.latency_budget_ms
    }

    pub(crate) fn parent_packet_id(&self) -> Option<&str> {
        self.parent_packet_id.as_deref()
    }

    pub(crate) fn option_ids(&self) -> &[String] {
        &self.option_ids
    }

    pub(crate) fn core_generation_id(&self) -> Option<&str> {
        self.core_generation_id.as_deref()
    }

    pub(crate) fn retrieval_generation(&self) -> Option<&str> {
        self.retrieval_generation.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestHashesV3 {
    question_sha256: Sha256DigestV3Dto,
    request_sha256: Sha256DigestV3Dto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordValidationErrorV3 {
    CanonicalJson(String),
    InvalidDigest,
    NoActiveCorePin,
    ProjectIdentityUnavailable,
    EmptyQuestion,
    EmptyIdentity(&'static str),
    IdentityTooLong(&'static str, usize),
    ProjectMismatch,
    InvalidRequest(String),
    InvalidSourceDelivery(String),
    InvalidPacketRequestPatch(String),
    RequestedCoreGenerationMismatch,
    RequestedRetrievalGenerationMismatch,
    RetrievalCoreSkew,
    InvalidRetrievalHash,
    FullRetrievalWithoutPublication,
    RetrievalStateMismatch,
    InvalidPacketId,
    TooManyEvidenceRows(usize),
    TooManyGapRows(usize),
    TooManyDiagnosticRows(usize),
    TooManyDiagnosticReferences(usize),
    DuplicateEvidenceIdentity(String),
    DuplicateGapIdentity(String),
    DuplicateDiagnosticIdentity(String),
    ZeroContinuationRounds,
    DuplicateContinuationGapReference(String),
    UnknownContinuationGap(String),
    DuplicateDiagnosticEvidenceReference(String),
    DuplicateDiagnosticGapReference(String),
    UnknownDiagnosticEvidence(String),
    UnknownDiagnosticGap(String),
}

impl RecordValidationErrorV3 {
    fn into_api_error(self) -> ApiError {
        ApiError::new("invalid_packet_execution_record_v3", format!("{self:?}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalizedDiagnosticSourceRowV3 {
    diagnostic_id: IdentityTextV3,
    category: DiagnosticCategoryV3Dto,
    code: DiagnosticCodeTextV3,
    evidence_ids: Vec<EvidenceIdentityV3Dto>,
    gap_ids: Vec<GapIdentityV3Dto>,
}

impl FinalizedDiagnosticSourceRowV3 {
    pub(crate) fn new(
        diagnostic_id: IdentityTextV3,
        category: DiagnosticCategoryV3Dto,
        code: DiagnosticCodeTextV3,
        evidence_ids: Vec<EvidenceIdentityV3Dto>,
        gap_ids: Vec<GapIdentityV3Dto>,
    ) -> Self {
        Self {
            diagnostic_id,
            category,
            code,
            evidence_ids,
            gap_ids,
        }
    }

    pub(crate) fn diagnostic_id(&self) -> &IdentityTextV3 {
        &self.diagnostic_id
    }

    pub(crate) fn category(&self) -> DiagnosticCategoryV3Dto {
        self.category
    }

    pub(crate) fn code(&self) -> &DiagnosticCodeTextV3 {
        &self.code
    }

    pub(crate) fn evidence_ids(&self) -> &[EvidenceIdentityV3Dto] {
        &self.evidence_ids
    }

    pub(crate) fn gap_ids(&self) -> &[GapIdentityV3Dto] {
        &self.gap_ids
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalizedPacketExecutionInputV3 {
    caller_id: IdentityTextV3,
    request_id: IdentityTextV3,
    request: PacketRequestFingerprintV3,
    evidence: Vec<PacketEvidenceRowV3Dto>,
    gaps: Vec<ProjectionGapRowV3Dto>,
    continuation: Option<ContinuationStateV3Dto>,
    retrieval: RetrievalStateDescriptorV3Dto,
    diagnostics: Vec<FinalizedDiagnosticSourceRowV3>,
}

impl FinalizedPacketExecutionInputV3 {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        caller_id: IdentityTextV3,
        request_id: IdentityTextV3,
        request: PacketRequestFingerprintV3,
        evidence: Vec<PacketEvidenceRowV3Dto>,
        gaps: Vec<ProjectionGapRowV3Dto>,
        continuation: Option<ContinuationStateV3Dto>,
        retrieval: RetrievalStateDescriptorV3Dto,
        diagnostics: Vec<FinalizedDiagnosticSourceRowV3>,
    ) -> Self {
        Self {
            caller_id,
            request_id,
            request,
            evidence,
            gaps,
            continuation,
            retrieval,
            diagnostics,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedPacketPublicationV3 {
    project: ProjectIdentityV3,
    core_project_id: String,
    core_generation_id: String,
    core_run_id: String,
    retrieval: Option<EmbeddingVectorPublicationIdentityDto>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketExecutionRecordV3 {
    packet_id: IdentityTextV3,
    caller_id: IdentityTextV3,
    request_id: IdentityTextV3,
    question_sha256: Sha256DigestV3Dto,
    request_sha256: Sha256DigestV3Dto,
    plan_version: u32,
    project: ProjectIdentityV3,
    core_publication: CorePublicationIdentityV3Dto,
    retrieval_publication: Option<RetrievalPublicationIdentityV3Dto>,
    request: PacketRequestFingerprintV3,
    evidence: Vec<PacketEvidenceRowV3Dto>,
    gaps: Vec<ProjectionGapRowV3Dto>,
    continuation: Option<ContinuationStateV3Dto>,
    retrieval: RetrievalStateDescriptorV3Dto,
    diagnostics: Vec<FinalizedDiagnosticSourceRowV3>,
}

impl PacketExecutionRecordV3 {
    pub(crate) fn packet_id(&self) -> &IdentityTextV3 {
        &self.packet_id
    }

    pub(crate) fn caller_id(&self) -> &IdentityTextV3 {
        &self.caller_id
    }

    pub(crate) fn request_id(&self) -> &IdentityTextV3 {
        &self.request_id
    }

    pub(crate) fn question_sha256(&self) -> &Sha256DigestV3Dto {
        &self.question_sha256
    }

    pub(crate) fn request_sha256(&self) -> &Sha256DigestV3Dto {
        &self.request_sha256
    }

    pub(crate) fn plan_version(&self) -> u32 {
        self.plan_version
    }

    pub(crate) fn project(&self) -> &ProjectIdentityV3 {
        &self.project
    }

    pub(crate) fn core_publication(&self) -> &CorePublicationIdentityV3Dto {
        &self.core_publication
    }

    pub(crate) fn retrieval_publication(&self) -> Option<&RetrievalPublicationIdentityV3Dto> {
        self.retrieval_publication.as_ref()
    }

    pub(crate) fn request(&self) -> &PacketRequestFingerprintV3 {
        &self.request
    }

    pub(crate) fn evidence(&self) -> &[PacketEvidenceRowV3Dto] {
        &self.evidence
    }

    pub(crate) fn gaps(&self) -> &[ProjectionGapRowV3Dto] {
        &self.gaps
    }

    pub(crate) fn continuation(&self) -> Option<&ContinuationStateV3Dto> {
        self.continuation.as_ref()
    }

    pub(crate) fn diagnostics(&self) -> &[FinalizedDiagnosticSourceRowV3] {
        &self.diagnostics
    }

    pub(crate) fn retrieval(&self) -> &RetrievalStateDescriptorV3Dto {
        &self.retrieval
    }
}

trait PacketIdSourceV3 {
    fn next_packet_id(&mut self) -> String;
}

struct OsPacketIdSourceV3;

impl PacketIdSourceV3 for OsPacketIdSourceV3 {
    fn next_packet_id(&mut self) -> String {
        Uuid::new_v4().to_string()
    }
}

pub(crate) fn build_packet_execution_record_v3_with_id(
    service: &PublicOperationService,
    input: &FinalizedPacketExecutionInputV3,
    packet_id: &str,
) -> Result<PacketExecutionRecordV3, RecordValidationErrorV3> {
    struct ReservedId<'a>(&'a str);
    impl PacketIdSourceV3 for ReservedId<'_> {
        fn next_packet_id(&mut self) -> String {
            self.0.to_owned()
        }
    }
    let capture = capture_packet_publication_v3(service)?;
    // Reuse every normal receipt/publication/identity validator.
    build_record_from_captured_v3(&capture, input, &mut ReservedId(packet_id))
}

pub(crate) fn build_packet_execution_record_v3(
    service: &PublicOperationService,
    input: &FinalizedPacketExecutionInputV3,
) -> Result<PacketExecutionRecordV3, RecordValidationErrorV3> {
    let capture = capture_packet_publication_v3(service)?;
    build_record_from_captured_product_v3(&capture, input)
}

fn capture_packet_publication_v3(
    service: &PublicOperationService,
) -> Result<CapturedPacketPublicationV3, RecordValidationErrorV3> {
    let active = service
        .active_publication()
        .ok_or(RecordValidationErrorV3::NoActiveCorePin)?;
    let project = service
        .active_project_identity_v3()
        .map_err(|_| RecordValidationErrorV3::ProjectIdentityUnavailable)?;
    Ok(CapturedPacketPublicationV3 {
        core_project_id: project.project_id.clone(),
        core_generation_id: active.core_publication.generation_id,
        core_run_id: active.core_publication.run_id,
        project,
        retrieval: active.retrieval_publication,
    })
}

fn build_record_from_captured_product_v3(
    capture: &CapturedPacketPublicationV3,
    input: &FinalizedPacketExecutionInputV3,
) -> Result<PacketExecutionRecordV3, RecordValidationErrorV3> {
    build_record_from_captured_v3(capture, input, &mut OsPacketIdSourceV3)
}

fn build_record_from_captured_v3(
    capture: &CapturedPacketPublicationV3,
    input: &FinalizedPacketExecutionInputV3,
    packet_ids: &mut impl PacketIdSourceV3,
) -> Result<PacketExecutionRecordV3, RecordValidationErrorV3> {
    validate_required_identity("caller_id", input.caller_id.as_str())?;
    validate_required_identity("request_id", input.request_id.as_str())?;
    validate_request(&input.request)?;
    validate_project(&capture.project)?;
    validate_required_identity("core_project_id", &capture.core_project_id)?;
    validate_required_identity("core_generation_id", &capture.core_generation_id)?;
    validate_required_identity("core_run_id", &capture.core_run_id)?;
    if capture.core_project_id != capture.project.project_id {
        return Err(RecordValidationErrorV3::ProjectMismatch);
    }
    if input
        .request
        .core_generation_id
        .as_deref()
        .is_some_and(|requested| requested != capture.core_generation_id)
    {
        return Err(RecordValidationErrorV3::RequestedCoreGenerationMismatch);
    }
    if let Some(requested) = input.request.retrieval_generation.as_deref()
        && capture
            .retrieval
            .as_ref()
            .is_none_or(|retrieval| retrieval.retrieval_generation != requested)
    {
        return Err(RecordValidationErrorV3::RequestedRetrievalGenerationMismatch);
    }

    let retrieval_publication = capture
        .retrieval
        .as_ref()
        .map(|retrieval| validate_retrieval(retrieval, capture))
        .transpose()?;
    validate_retrieval_state(&input.retrieval, retrieval_publication.as_ref())?;

    if input.evidence.len() > PACKET_EVIDENCE_ROWS_MAX_V3 {
        return Err(RecordValidationErrorV3::TooManyEvidenceRows(
            input.evidence.len(),
        ));
    }
    if input.gaps.len() > GAP_ROWS_MAX_V3 {
        return Err(RecordValidationErrorV3::TooManyGapRows(input.gaps.len()));
    }
    if input.diagnostics.len() > DIAGNOSTIC_ROWS_MAX_V3 {
        return Err(RecordValidationErrorV3::TooManyDiagnosticRows(
            input.diagnostics.len(),
        ));
    }

    let mut evidence = input.evidence.clone();
    evidence.sort_by(|left, right| left.identity.cmp(&right.identity));
    for row in &evidence {
        if let Some(patch) = &row.packet_request_patch {
            validate_patch_publication(patch, capture)?;
            let matches_cursor = match (
                patch.probes.as_slice(),
                &row.source_delivery,
                &row.symbol_id,
            ) {
                (
                    [
                        codestory_contracts::api::PacketProbeDto::Continuation {
                            project_id,
                            selector,
                            ..
                        },
                    ],
                    Some(delivery),
                    Some(symbol_id),
                ) => {
                    project_id == &capture.core_project_id
                        && selector.stable_identity == format!("node:{}", symbol_id.as_str())
                        && selector.symbol_id.as_deref() == Some(symbol_id.as_str())
                        && selector.path.as_deref().is_none_or(|path| {
                            Some(path) == row.path.as_ref().map(|path| path.as_str())
                        })
                        && delivery.next_start_line.is_some()
                        && selector.source_range
                            == delivery.next_start_line.map(|start_line| {
                                codestory_contracts::compilation::PacketSourceRangeV1 {
                                    start_line,
                                    end_line: delivery.requested_range.end_line,
                                }
                            })
                }
                ([], Some(delivery), Some(symbol_id)) => {
                    delivery.next_start_line.is_some_and(|next| {
                        let expected = codestory_contracts::api::DrillOptionDto::omitted_symbol(
                            "source-page",
                            symbol_id.as_str(),
                        )
                        .with_source_range(
                            codestory_contracts::compilation::PacketSourceRangeV1 {
                                start_line: next,
                                end_line: delivery.requested_range.end_line,
                            },
                        );
                        expected.id.starts_with("ms1:")
                            && patch.parent_packet_id.as_ref() == Some(&input.request_id)
                            && patch.option_ids.as_slice().len() == 1
                            && patch.option_ids.as_slice()[0].as_str() == expected.id
                    })
                }
                _ => false,
            };
            if !matches_cursor {
                return Err(RecordValidationErrorV3::InvalidPacketRequestPatch(
                    row.identity.evidence_id.as_str().to_owned(),
                ));
            }
        }
        validate_required_identity("evidence_id", row.identity.evidence_id.as_str())?;
        if let Some(delivery) = &row.source_delivery
            && (row.kind
                != codestory_contracts::packet_projection_v3::EvidenceKindV3Dto::ExactSource
                || row.summary.is_none()
                || !row
                    .start_line
                    .zip(row.end_line)
                    .is_some_and(|(start, end)| delivery.matches_delivery(start, end)))
        {
            return Err(RecordValidationErrorV3::InvalidSourceDelivery(
                row.identity.evidence_id.as_str().to_owned(),
            ));
        }
    }
    reject_duplicate_by(
        &evidence,
        |row| row.identity.evidence_id.as_str(),
        RecordValidationErrorV3::DuplicateEvidenceIdentity,
    )?;
    let evidence_ids = evidence
        .iter()
        .map(|row| row.identity.evidence_id.as_str())
        .collect::<BTreeSet<_>>();

    let mut gaps = input.gaps.clone();
    gaps.sort_by(|left, right| left.identity.cmp(&right.identity));
    for row in &gaps {
        validate_required_identity("gap_id", row.identity.gap_id.as_str())?;
    }
    reject_duplicate_by(
        &gaps,
        |row| row.identity.gap_id.as_str(),
        RecordValidationErrorV3::DuplicateGapIdentity,
    )?;
    let gap_ids = gaps
        .iter()
        .map(|row| row.identity.gap_id.as_str())
        .collect::<BTreeSet<_>>();

    if let Some(continuation) = &input.continuation
        && let Some(patch) = &continuation.packet_request_patch
    {
        validate_patch_publication(patch, capture)?;
        if patch.parent_packet_id.as_ref() != Some(&continuation.continuation_id)
            || patch
                .option_ids
                .as_slice()
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                != continuation
                    .gap_ids
                    .as_slice()
                    .iter()
                    .map(|id| id.gap_id.as_str())
                    .collect::<Vec<_>>()
        {
            return Err(RecordValidationErrorV3::InvalidPacketRequestPatch(
                "broad continuation options".into(),
            ));
        }
    }
    let continuation = input
        .continuation
        .as_ref()
        .map(|continuation| canonical_continuation(continuation, &gap_ids))
        .transpose()?;
    let diagnostics = canonical_diagnostics(&input.diagnostics, &evidence_ids, &gap_ids)?;
    let hashes = fingerprint_request_v3(&input.request)?;
    let packet_id = packet_ids.next_packet_id();
    let parsed_packet_id =
        Uuid::parse_str(&packet_id).map_err(|_| RecordValidationErrorV3::InvalidPacketId)?;
    if parsed_packet_id.get_version_num() != 4 || parsed_packet_id.get_variant() != Variant::RFC4122
    {
        return Err(RecordValidationErrorV3::InvalidPacketId);
    }

    Ok(PacketExecutionRecordV3 {
        packet_id: identity_from_required("packet_id", packet_id)?,
        caller_id: input.caller_id.clone(),
        request_id: input.request_id.clone(),
        question_sha256: hashes.question_sha256,
        request_sha256: hashes.request_sha256,
        plan_version: PACKET_EXECUTION_PLAN_VERSION_V3,
        project: capture.project.clone(),
        core_publication: CorePublicationIdentityV3Dto {
            project_id: identity_from_required("core_project_id", capture.core_project_id.clone())?,
            generation_id: identity_from_required(
                "core_generation_id",
                capture.core_generation_id.clone(),
            )?,
            run_id: identity_from_required("core_run_id", capture.core_run_id.clone())?,
        },
        retrieval_publication,
        request: input.request.clone(),
        evidence,
        gaps,
        continuation,
        retrieval: input.retrieval.clone(),
        diagnostics,
    })
}

fn validate_project(project: &ProjectIdentityV3) -> Result<(), RecordValidationErrorV3> {
    validate_required_identity("project_id", &project.project_id)?;
    validate_required_identity("workspace_id", &project.workspace_id)?;
    validate_required_identity("artifact_scope_id", &project.artifact_scope_id)?;
    for (field, value) in [
        (
            "canonical_repository_id",
            project.canonical_repository_id.as_deref(),
        ),
        (
            "legacy_canonical_repository_id",
            project.legacy_canonical_repository_id.as_deref(),
        ),
        (
            "legacy_raw_root_project_id",
            project.legacy_raw_root_project_id.as_deref(),
        ),
        (
            "normalized_root_project_id_alias",
            project.normalized_root_project_id_alias.as_deref(),
        ),
    ] {
        if let Some(value) = value {
            validate_required_identity(field, value)?;
        }
    }
    Ok(())
}

fn validate_request(request: &PacketRequestFingerprintV3) -> Result<(), RecordValidationErrorV3> {
    if request.question.trim().is_empty() {
        return Err(RecordValidationErrorV3::EmptyQuestion);
    }
    codestory_contracts::api::validate_packet_probe_request(&request.typed_probes)
        .map_err(RecordValidationErrorV3::InvalidRequest)?;
    for (field, value) in [
        ("parent_packet_id", request.parent_packet_id.as_deref()),
        (
            "requested_core_generation_id",
            request.core_generation_id.as_deref(),
        ),
        (
            "requested_retrieval_generation",
            request.retrieval_generation.as_deref(),
        ),
    ] {
        if let Some(value) = value {
            validate_required_identity(field, value)?;
        }
    }
    for option_id in &request.option_ids {
        validate_required_identity("option_id", option_id)?;
    }
    Ok(())
}

fn validate_retrieval(
    retrieval: &EmbeddingVectorPublicationIdentityDto,
    capture: &CapturedPacketPublicationV3,
) -> Result<RetrievalPublicationIdentityV3Dto, RecordValidationErrorV3> {
    for (field, value) in [
        (
            "retrieval_core_generation_id",
            retrieval.core_generation_id.as_str(),
        ),
        ("retrieval_core_run_id", retrieval.core_run_id.as_str()),
        (
            "retrieval_generation",
            retrieval.retrieval_generation.as_str(),
        ),
        (
            "semantic_generation",
            retrieval.semantic_generation.as_str(),
        ),
    ] {
        validate_required_identity(field, value)?;
    }
    if retrieval.core_generation_id != capture.core_generation_id
        || retrieval.core_run_id != capture.core_run_id
    {
        return Err(RecordValidationErrorV3::RetrievalCoreSkew);
    }
    let retrieval_input_sha256 = Sha256DigestV3Dto::new(retrieval.retrieval_input_hash.clone())
        .map_err(|_| RecordValidationErrorV3::InvalidRetrievalHash)?;
    Ok(RetrievalPublicationIdentityV3Dto {
        core_generation_id: identity_from_required(
            "retrieval_core_generation_id",
            retrieval.core_generation_id.clone(),
        )?,
        core_run_id: identity_from_required(
            "retrieval_core_run_id",
            retrieval.core_run_id.clone(),
        )?,
        retrieval_generation: identity_from_required(
            "retrieval_generation",
            retrieval.retrieval_generation.clone(),
        )?,
        retrieval_input_sha256,
        semantic_generation: identity_from_required(
            "semantic_generation",
            retrieval.semantic_generation.clone(),
        )?,
    })
}

fn validate_retrieval_state(
    state: &RetrievalStateDescriptorV3Dto,
    publication: Option<&RetrievalPublicationIdentityV3Dto>,
) -> Result<(), RecordValidationErrorV3> {
    if let Some(generation) = &state.generation_id {
        validate_required_identity("retrieval_state_generation_id", generation.as_str())?;
    }
    match (&state.state, publication, state.generation_id.as_ref()) {
        (RetrievalStateV3Dto::Full, None, _) => {
            Err(RecordValidationErrorV3::FullRetrievalWithoutPublication)
        }
        (RetrievalStateV3Dto::Full, Some(publication), Some(generation))
            if generation.as_str() == publication.retrieval_generation.as_str() =>
        {
            Ok(())
        }
        (RetrievalStateV3Dto::Symbolic, None, None)
        | (RetrievalStateV3Dto::Degraded, None, None)
        | (RetrievalStateV3Dto::Unavailable, None, None) => Ok(()),
        (RetrievalStateV3Dto::Degraded, Some(publication), Some(generation))
            if generation.as_str() == publication.retrieval_generation.as_str() =>
        {
            Ok(())
        }
        _ => Err(RecordValidationErrorV3::RetrievalStateMismatch),
    }
}

fn validate_patch_publication(
    patch: &codestory_contracts::packet_projection_v3::PacketRequestPatchV3Dto,
    capture: &CapturedPacketPublicationV3,
) -> Result<(), RecordValidationErrorV3> {
    patch
        .validate()
        .map_err(RecordValidationErrorV3::InvalidPacketRequestPatch)?;
    if patch.core_generation_id.as_str() != capture.core_generation_id
        || patch.retrieval_generation.as_ref().map(|id| id.as_str())
            != capture
                .retrieval
                .as_ref()
                .map(|publication| publication.retrieval_generation.as_str())
    {
        return Err(RecordValidationErrorV3::InvalidPacketRequestPatch(
            "publication mismatch".into(),
        ));
    }
    Ok(())
}

fn canonical_continuation(
    continuation: &ContinuationStateV3Dto,
    gap_ids: &BTreeSet<&str>,
) -> Result<ContinuationStateV3Dto, RecordValidationErrorV3> {
    validate_required_identity("continuation_id", continuation.continuation_id.as_str())?;
    continuation
        .validate()
        .map_err(|_| RecordValidationErrorV3::ZeroContinuationRounds)?;
    let references = continuation.gap_ids.as_slice().to_vec();
    // Continuation references are ordered choices, unlike canonical gap rows.
    let mut seen = BTreeSet::new();
    for reference in &references {
        if !seen.insert(reference) {
            return Err(RecordValidationErrorV3::DuplicateContinuationGapReference(
                reference.gap_id.as_str().to_owned(),
            ));
        }
    }
    for reference in &references {
        if !gap_ids.contains(reference.gap_id.as_str()) {
            return Err(RecordValidationErrorV3::UnknownContinuationGap(
                reference.gap_id.as_str().to_owned(),
            ));
        }
    }
    let mut canonical = ContinuationStateV3Dto::new(
        continuation.continuation_id.clone(),
        continuation.remaining_rounds,
        codestory_contracts::packet_projection_v3::BoundedVecV3::new(references)
            .expect("the source bounded vector remains bounded"),
    )
    .expect("validated continuation remains positive");
    canonical.packet_request_patch = continuation.packet_request_patch.clone();
    Ok(canonical)
}

fn canonical_diagnostics(
    source: &[FinalizedDiagnosticSourceRowV3],
    evidence_ids: &BTreeSet<&str>,
    gap_ids: &BTreeSet<&str>,
) -> Result<Vec<FinalizedDiagnosticSourceRowV3>, RecordValidationErrorV3> {
    let mut diagnostics = source.to_vec();
    diagnostics.sort_by(|left, right| left.diagnostic_id.cmp(&right.diagnostic_id));
    reject_duplicate_by(
        &diagnostics,
        |row| row.diagnostic_id.as_str(),
        RecordValidationErrorV3::DuplicateDiagnosticIdentity,
    )?;
    for diagnostic in &mut diagnostics {
        validate_required_identity("diagnostic_id", diagnostic.diagnostic_id.as_str())?;
        validate_required_identity("diagnostic_code", diagnostic.code.as_str())?;
        if diagnostic.evidence_ids.len() > REFERENCE_ROWS_MAX_V3 {
            return Err(RecordValidationErrorV3::TooManyDiagnosticReferences(
                diagnostic.evidence_ids.len(),
            ));
        }
        if diagnostic.gap_ids.len() > REFERENCE_ROWS_MAX_V3 {
            return Err(RecordValidationErrorV3::TooManyDiagnosticReferences(
                diagnostic.gap_ids.len(),
            ));
        }
        diagnostic.evidence_ids.sort();
        for pair in diagnostic.evidence_ids.windows(2) {
            if pair[0] == pair[1] {
                return Err(
                    RecordValidationErrorV3::DuplicateDiagnosticEvidenceReference(
                        pair[0].evidence_id.as_str().to_owned(),
                    ),
                );
            }
        }
        for reference in &diagnostic.evidence_ids {
            if !evidence_ids.contains(reference.evidence_id.as_str()) {
                return Err(RecordValidationErrorV3::UnknownDiagnosticEvidence(
                    reference.evidence_id.as_str().to_owned(),
                ));
            }
        }
        diagnostic.gap_ids.sort();
        for pair in diagnostic.gap_ids.windows(2) {
            if pair[0] == pair[1] {
                return Err(RecordValidationErrorV3::DuplicateDiagnosticGapReference(
                    pair[0].gap_id.as_str().to_owned(),
                ));
            }
        }
        for reference in &diagnostic.gap_ids {
            if !gap_ids.contains(reference.gap_id.as_str()) {
                return Err(RecordValidationErrorV3::UnknownDiagnosticGap(
                    reference.gap_id.as_str().to_owned(),
                ));
            }
        }
    }
    Ok(diagnostics)
}

fn reject_duplicate_by<T>(
    rows: &[T],
    identity: impl Fn(&T) -> &str,
    error: impl Fn(String) -> RecordValidationErrorV3,
) -> Result<(), RecordValidationErrorV3> {
    for pair in rows.windows(2) {
        if identity(&pair[0]) == identity(&pair[1]) {
            return Err(error(identity(&pair[0]).to_owned()));
        }
    }
    Ok(())
}

fn validate_required_identity(
    field: &'static str,
    value: &str,
) -> Result<(), RecordValidationErrorV3> {
    if value.is_empty() {
        return Err(RecordValidationErrorV3::EmptyIdentity(field));
    }
    if value.len() > codestory_contracts::packet_projection_v3::IDENTITY_MAX_BYTES_V3 {
        return Err(RecordValidationErrorV3::IdentityTooLong(field, value.len()));
    }
    Ok(())
}

fn identity_from_required(
    field: &'static str,
    value: String,
) -> Result<IdentityTextV3, RecordValidationErrorV3> {
    validate_required_identity(field, &value)?;
    let length = value.len();
    IdentityTextV3::new(value).map_err(|_| RecordValidationErrorV3::IdentityTooLong(field, length))
}

fn fingerprint_request_v3(
    request: &PacketRequestFingerprintV3,
) -> Result<RequestHashesV3, RecordValidationErrorV3> {
    let canonical =
        canonical_json_bytes_v3(request).map_err(RecordValidationErrorV3::CanonicalJson)?;
    let mut request_bytes = Vec::with_capacity(REQUEST_DIGEST_DOMAIN_V3.len() + canonical.len());
    request_bytes.extend_from_slice(REQUEST_DIGEST_DOMAIN_V3);
    request_bytes.extend_from_slice(&canonical);
    Ok(RequestHashesV3 {
        question_sha256: digest_v3(request.question.as_bytes())?,
        request_sha256: digest_v3(&request_bytes)?,
    })
}

fn digest_v3(bytes: &[u8]) -> Result<Sha256DigestV3Dto, RecordValidationErrorV3> {
    Sha256DigestV3Dto::new(format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| RecordValidationErrorV3::InvalidDigest)
}

#[cfg(any(
    test,
    feature = "test-support",
    feature = "v3-evidence-separation-support"
))]
pub(crate) fn build_packet_execution_record_fixture_v3(
    input: &FinalizedPacketExecutionInputV3,
    with_retrieval_publication: bool,
) -> Result<PacketExecutionRecordV3, RecordValidationErrorV3> {
    struct FixedPacketIdSourceV3;

    impl PacketIdSourceV3 for FixedPacketIdSourceV3 {
        fn next_packet_id(&mut self) -> String {
            "00000000-0000-4000-8000-000000000001".to_owned()
        }
    }

    let project = ProjectIdentityV3 {
        project_identity_schema_version: 3,
        project_id: "project-1".to_owned(),
        workspace_id: "workspace-1".to_owned(),
        artifact_scope_id: "artifact-1".to_owned(),
        canonical_repository_id: Some("repository-1".to_owned()),
        legacy_canonical_repository_id: None,
        legacy_raw_root_project_id: None,
        normalized_root_project_id_alias: None,
        portable_reuse_eligible: true,
        portable_reuse_reason: "test fixture".to_owned(),
    };
    let capture = CapturedPacketPublicationV3 {
        core_project_id: project.project_id.clone(),
        core_generation_id: "core-generation-1".to_owned(),
        core_run_id: "core-run-1".to_owned(),
        project,
        retrieval: with_retrieval_publication.then(|| EmbeddingVectorPublicationIdentityDto {
            core_generation_id: "core-generation-1".to_owned(),
            core_run_id: "core-run-1".to_owned(),
            retrieval_generation: "retrieval-generation-1".to_owned(),
            retrieval_input_hash:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
            semantic_generation: "semantic-generation-1".to_owned(),
        }),
    };
    build_record_from_captured_v3(&capture, input, &mut FixedPacketIdSourceV3)
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn build_packet_execution_record_replay_v3(
    input: &FinalizedPacketExecutionInputV3,
    publication: &codestory_contracts::compilation::PacketCompilationPublicationV1,
    core_run_id: &str,
    retrieval: Option<EmbeddingVectorPublicationIdentityDto>,
    packet_id: &str,
) -> Result<PacketExecutionRecordV3, RecordValidationErrorV3> {
    struct CapturedId(String);
    impl PacketIdSourceV3 for CapturedId {
        fn next_packet_id(&mut self) -> String {
            self.0.clone()
        }
    }
    let project = ProjectIdentityV3 {
        project_identity_schema_version: 3,
        project_id: publication.project_id.clone(),
        workspace_id: publication.project_id.clone(),
        artifact_scope_id: publication.project_id.clone(),
        canonical_repository_id: None,
        legacy_canonical_repository_id: None,
        legacy_raw_root_project_id: None,
        normalized_root_project_id_alias: None,
        portable_reuse_eligible: false,
        portable_reuse_reason: "CPU replay fixture".into(),
    };
    let capture = CapturedPacketPublicationV3 {
        core_project_id: publication.project_id.clone(),
        core_generation_id: publication.core_generation_id.clone(),
        core_run_id: core_run_id.into(),
        project,
        retrieval,
    };
    build_record_from_captured_v3(&capture, input, &mut CapturedId(packet_id.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use codestory_contracts::api::{
        AgentPacketRequestDto, EmbeddingVectorPublicationIdentityDto, PacketProbeDto,
    };
    use codestory_contracts::packet_projection_v3::{
        BoundedVecV3, ContinuationStateV3Dto, DiagnosticCategoryV3Dto, DiagnosticCodeTextV3,
        EvidenceIdentityV3Dto, EvidenceKindV3Dto, GapIdentityV3Dto, GapKindV3Dto, IdentityTextV3,
        PacketEvidenceRowV3Dto, ProjectionGapRowV3Dto, RetrievalStateDescriptorV3Dto,
        RetrievalStateV3Dto, Sha256DigestV3Dto, SummaryTextV3,
    };
    use codestory_workspace::ProjectIdentityV3;

    use crate::services::activation_tests::ready_activation_fixture;

    fn identity(value: &str) -> IdentityTextV3 {
        IdentityTextV3::new(value).expect("bounded identity")
    }

    fn sha(value: &str) -> Sha256DigestV3Dto {
        Sha256DigestV3Dto::new(value).expect("sha-256")
    }

    fn request_fixture() -> PacketRequestFingerprintV3 {
        PacketRequestFingerprintV3 {
            question: "hello".to_owned(),
            budget: PacketBudgetModeDto::Standard,
            typed_probes: Vec::new(),
            latency_budget_ms: None,
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        }
    }

    fn project_fixture() -> ProjectIdentityV3 {
        ProjectIdentityV3 {
            project_identity_schema_version: 3,
            project_id: "project-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            artifact_scope_id: "artifact-1".to_owned(),
            canonical_repository_id: Some("repository-1".to_owned()),
            legacy_canonical_repository_id: None,
            legacy_raw_root_project_id: Some("legacy-root-1".to_owned()),
            normalized_root_project_id_alias: Some("root-alias-1".to_owned()),
            portable_reuse_eligible: true,
            portable_reuse_reason: "clean tree".to_owned(),
        }
    }

    fn capture_fixture() -> CapturedPacketPublicationV3 {
        CapturedPacketPublicationV3 {
            project: project_fixture(),
            core_project_id: "project-1".to_owned(),
            core_generation_id: "core-generation-1".to_owned(),
            core_run_id: "core-run-1".to_owned(),
            retrieval: Some(EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-generation-1".to_owned(),
                core_run_id: "core-run-1".to_owned(),
                retrieval_generation: "retrieval-generation-1".to_owned(),
                retrieval_input_hash:
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
                semantic_generation: "semantic-generation-1".to_owned(),
            }),
        }
    }

    fn evidence(id: &str) -> PacketEvidenceRowV3Dto {
        PacketEvidenceRowV3Dto {
            packet_request_patch: None,
            source_delivery: None,
            identity: EvidenceIdentityV3Dto {
                evidence_id: identity(id),
            },
            kind: EvidenceKindV3Dto::ExactSource,
            path: None,
            symbol_id: None,
            start_line: Some(1),
            end_line: Some(1),
            summary: Some(SummaryTextV3::new(format!("summary {id}")).unwrap()),
        }
    }

    fn gap(id: &str) -> ProjectionGapRowV3Dto {
        ProjectionGapRowV3Dto {
            identity: GapIdentityV3Dto {
                gap_id: identity(id),
            },
            kind: GapKindV3Dto::EvidenceMissing,
            message: None,
        }
    }

    fn diagnostic(
        id: &str,
        evidence_ids: &[&str],
        gap_ids: &[&str],
    ) -> FinalizedDiagnosticSourceRowV3 {
        FinalizedDiagnosticSourceRowV3 {
            diagnostic_id: identity(id),
            category: DiagnosticCategoryV3Dto::Coverage,
            code: DiagnosticCodeTextV3::new("source_gap").unwrap(),
            evidence_ids: evidence_ids
                .iter()
                .map(|id| EvidenceIdentityV3Dto {
                    evidence_id: identity(id),
                })
                .collect(),
            gap_ids: gap_ids
                .iter()
                .map(|id| GapIdentityV3Dto {
                    gap_id: identity(id),
                })
                .collect(),
        }
    }

    fn finalized_fixture() -> FinalizedPacketExecutionInputV3 {
        FinalizedPacketExecutionInputV3 {
            caller_id: identity("caller-1"),
            request_id: identity("request-1"),
            request: request_fixture(),
            evidence: vec![evidence("evidence-b"), evidence("evidence-a")],
            gaps: vec![gap("gap-b"), gap("gap-a")],
            continuation: Some(ContinuationStateV3Dto {
                packet_request_patch: None,
                continuation_id: identity("continuation-1"),
                remaining_rounds: 1,
                gap_ids: BoundedVecV3::new(vec![
                    GapIdentityV3Dto {
                        gap_id: identity("gap-b"),
                    },
                    GapIdentityV3Dto {
                        gap_id: identity("gap-a"),
                    },
                ])
                .unwrap(),
            }),
            retrieval: RetrievalStateDescriptorV3Dto {
                state: RetrievalStateV3Dto::Full,
                generation_id: Some(identity("retrieval-generation-1")),
            },
            diagnostics: vec![
                diagnostic("diagnostic-b", &["evidence-b"], &["gap-b"]),
                diagnostic("diagnostic-a", &["evidence-a"], &["gap-a"]),
            ],
        }
    }

    struct SequencePacketIdSourceV3 {
        ids: VecDeque<String>,
    }

    impl PacketIdSourceV3 for SequencePacketIdSourceV3 {
        fn next_packet_id(&mut self) -> String {
            self.ids.pop_front().expect("enough packet ids")
        }
    }

    fn deterministic_id_source() -> SequencePacketIdSourceV3 {
        SequencePacketIdSourceV3 {
            ids: VecDeque::from([
                "00000000-0000-4000-8000-000000000001".to_owned(),
                "00000000-0000-4000-8000-000000000002".to_owned(),
            ]),
        }
    }

    #[test]
    fn packet_execution_record_v3_hashes_exact_question_and_canonical_request() {
        let request = request_fixture();

        let hashes = fingerprint_request_v3(&request).expect("valid closed request");

        assert_eq!(
            hashes.question_sha256.as_str(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            hashes.request_sha256.as_str(),
            "8b38090c2b3981740e85b500b0025fabef3ac16f8acbfd6a4c1b5f6efceb1b2c"
        );
    }

    #[test]
    fn packet_execution_record_v3_fingerprints_every_semantic_request_dimension() {
        let base = request_fixture();
        let base_digest = fingerprint_request_v3(&base).unwrap().request_sha256;
        let mut mutations = Vec::new();

        let mut value = base.clone();
        value.question.push('!');
        mutations.push(value);
        let mut value = base.clone();
        value.budget = PacketBudgetModeDto::Deep;
        mutations.push(value);
        let mut value = base.clone();
        value.typed_probes = vec![PacketProbeDto::ExactPath {
            path: "src/lib.rs".to_owned(),
        }];
        mutations.push(value);
        let mut value = base.clone();
        value.latency_budget_ms = Some(5_000);
        mutations.push(value);
        let mut value = base.clone();
        value.parent_packet_id = Some("parent-1".to_owned());
        mutations.push(value);
        let mut value = base.clone();
        value.option_ids = vec!["option-1".to_owned()];
        mutations.push(value);
        let mut value = base.clone();
        value.core_generation_id = Some("core-generation-1".to_owned());
        mutations.push(value);
        let mut value = base;
        value.retrieval_generation = Some("retrieval-generation-1".to_owned());
        mutations.push(value);

        for mutation in mutations {
            assert_ne!(
                fingerprint_request_v3(&mutation).unwrap().request_sha256,
                base_digest
            );
        }

        let mut ordered = request_fixture();
        ordered.option_ids = vec!["first".to_owned(), "second".to_owned()];
        ordered.typed_probes = vec![
            PacketProbeDto::ExactPath {
                path: "src/first.rs".to_owned(),
            },
            PacketProbeDto::ExactPath {
                path: "src/second.rs".to_owned(),
            },
        ];
        let ordered_digest = fingerprint_request_v3(&ordered).unwrap().request_sha256;
        ordered.option_ids.reverse();
        assert_ne!(
            fingerprint_request_v3(&ordered).unwrap().request_sha256,
            ordered_digest,
            "ordered option semantics must survive canonicalization"
        );
        ordered.option_ids.reverse();
        ordered.typed_probes.reverse();
        assert_ne!(
            fingerprint_request_v3(&ordered).unwrap().request_sha256,
            ordered_digest,
            "ordered probe semantics must survive canonicalization"
        );

        let current_request = AgentPacketRequestDto {
            question: "hello".to_owned(),
            budget: PacketBudgetModeDto::Standard,
            probes: Vec::new(),
            latency_budget_ms: None,
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        };
        let fingerprint = PacketRequestFingerprintV3::from_current_request(&current_request);
        assert_eq!(
            fingerprint_request_v3(&fingerprint)
                .unwrap()
                .request_sha256
                .as_str()
                .len(),
            64
        );
    }

    #[test]
    fn compact_source_page_actions_preserve_dispatch_and_reject_cursor_aliases() {
        use codestory_contracts::{
            compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1},
            packet_projection_v3::{PacketRequestPatchV3Dto, PathTextV3, SymbolIdTextV3},
        };
        let mut input = finalized_fixture();
        input.continuation = None;
        input.evidence.truncate(1);
        input.diagnostics.clear();
        input.evidence[0].path = Some(PathTextV3::new("src/reader.rs").unwrap());
        input.evidence[0].symbol_id = Some(SymbolIdTextV3::new("7").unwrap());
        input.evidence[0].source_delivery = Some(PacketSourceDeliveryV1 {
            requested_range: PacketSourceRangeV1 {
                start_line: 1,
                end_line: 30,
            },
            source_sha256: "a".repeat(64),
            remaining_source_bytes: 700,
            next_start_line: Some(2),
            blocked_line_rendered_bytes: None,
        });
        let patch = PacketRequestPatchV3Dto {
            probes: BoundedVecV3::new(vec![]).unwrap(),
            parent_packet_id: Some(input.request_id.clone()),
            option_ids: BoundedVecV3::new(vec![identity("ms1:2:30:7")]).unwrap(),
            core_generation_id: identity("core-generation-1"),
            retrieval_generation: Some(identity("retrieval-generation-1")),
        };
        input.evidence[0].packet_request_patch = Some(patch.clone());
        build_record_from_captured_v3(&capture_fixture(), &input, &mut deterministic_id_source())
            .unwrap();
        let original: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({
            "question":"inspect source", "budget":"compact", "latency_budget_ms":900
        }))
        .unwrap();
        let applied = patch.apply_to(&original).unwrap();
        assert_eq!(applied.question, original.question);
        assert_eq!(applied.budget, original.budget);
        assert_eq!(applied.latency_budget_ms, original.latency_budget_ms);
        let probes =
            crate::agent::packet_compiler::continuation_probes_from_options(&applied, "project-1")
                .unwrap();
        assert_eq!(probes.len(), 1);
        let PacketProbeDto::Continuation {
            project_id,
            core_generation_id,
            retrieval_generation,
            selector,
            ..
        } = &probes[0]
        else {
            panic!("source action must dispatch a typed continuation");
        };
        assert_eq!(project_id, "project-1");
        assert_eq!(core_generation_id, "core-generation-1");
        assert_eq!(
            retrieval_generation.as_deref(),
            Some("retrieval-generation-1")
        );
        assert_eq!(selector.stable_identity, "node:7");
        assert_eq!(selector.symbol_id.as_deref(), Some("7"));
        assert_eq!(
            selector.source_range,
            Some(PacketSourceRangeV1 {
                start_line: 2,
                end_line: 30
            })
        );
        assert_eq!(
            selector.reason,
            codestory_contracts::compilation::PacketStructuralGapReasonV1::SourceBudgetExceeded
        );
        for mutation in [
            "core",
            "retrieval",
            "parent",
            "extra",
            "alias",
            "node",
            "start",
            "end",
            "version",
            "mixed",
        ] {
            let mut hostile = input.clone();
            let bad = hostile.evidence[0].packet_request_patch.as_mut().unwrap();
            match mutation {
                "core" => bad.core_generation_id = identity("old-core"),
                "retrieval" => bad.retrieval_generation = None,
                "parent" => bad.parent_packet_id = Some(identity("other-request")),
                "extra" => {
                    bad.option_ids =
                        BoundedVecV3::new(vec![identity("ms1:2:30:7"), identity("ms1:2:30:8")])
                            .unwrap()
                }
                "mixed" => {
                    bad.probes =
                        BoundedVecV3::new(vec![PacketProbeDto::SymbolId { id: "7".into() }])
                            .unwrap()
                }
                value => {
                    bad.option_ids = BoundedVecV3::new(vec![identity(match value {
                        "alias" => "ms1:02:30:7",
                        "node" => "ms1:2:30:8",
                        "start" => "ms1:3:30:7",
                        "end" => "ms1:2:29:7",
                        _ => "ms2:2:30:7",
                    })])
                    .unwrap()
                }
            }
            assert!(
                build_record_from_captured_v3(
                    &capture_fixture(),
                    &hostile,
                    &mut deterministic_id_source()
                )
                .is_err(),
                "{mutation}"
            );
        }
    }

    #[test]
    fn source_page_request_patches_execute_without_another_broad_round_and_fail_closed() {
        use crate::agent::packet_projection_v3::build_packet_projection_v3;
        use codestory_contracts::{
            compilation::{
                PacketContinuationSelectorV1, PacketSourceDeliveryV1, PacketSourceRangeV1,
                PacketStructuralGapReasonV1,
            },
            packet_projection_v3::{
                DiagnosticsCapabilityV3Dto, PacketProjectionV3Dto, PacketRequestPatchV3Dto,
                PathTextV3, SymbolIdTextV3,
            },
        };
        let mut input = finalized_fixture();
        input.continuation = None; // The broad round is already exhausted.
        input.evidence.truncate(1);
        input.diagnostics.clear(); // This focused fixture removed the referenced rows.
        input.evidence[0].path = Some(PathTextV3::new("src/reader.rs").unwrap());
        input.evidence[0].symbol_id = Some(SymbolIdTextV3::new("7").unwrap());
        input.evidence[0].source_delivery = Some(PacketSourceDeliveryV1 {
            requested_range: PacketSourceRangeV1 {
                start_line: 1,
                end_line: 30,
            },
            source_sha256: "a".repeat(64),
            remaining_source_bytes: 700,
            next_start_line: Some(2),
            blocked_line_rendered_bytes: None,
        });
        let patch = PacketRequestPatchV3Dto {
            probes: BoundedVecV3::new(vec![PacketProbeDto::Continuation {
                contract_version: 1,
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
                selector: PacketContinuationSelectorV1 {
                    stable_identity: "node:7".into(),
                    symbol_id: Some("7".into()),
                    path: Some("src/reader.rs".into()),
                    source_range: Some(PacketSourceRangeV1 {
                        start_line: 2,
                        end_line: 30,
                    }),
                    reason: PacketStructuralGapReasonV1::SourceBudgetExceeded,
                },
            }])
            .unwrap(),
            parent_packet_id: None,
            option_ids: BoundedVecV3::new(vec![]).unwrap(),
            core_generation_id: identity("core-generation-1"),
            retrieval_generation: Some(identity("retrieval-generation-1")),
        };
        input.evidence[0].packet_request_patch = Some(patch.clone());
        let original: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({"question":"inspect source","budget":"compact","latency_budget_ms":900,"probes":[{"kind":"symbol_id","id":"old"}],"parent_packet_id":"old-parent","option_ids":["old-option"]})).unwrap();
        let applied = patch.apply_to(&original).unwrap();
        assert_eq!(applied.question, original.question);
        assert_eq!(applied.budget, original.budget);
        assert_eq!(applied.latency_budget_ms, original.latency_budget_ms);
        assert_eq!(applied.probes.len(), 1);
        assert!(applied.parent_packet_id.is_none() && applied.option_ids.is_empty());
        let json = serde_json::to_value(&patch).unwrap();
        assert!(json["parent_packet_id"].is_null());
        assert_eq!(json["option_ids"], serde_json::json!([]));
        assert_eq!(
            serde_json::from_value::<PacketRequestPatchV3Dto>(json).unwrap(),
            patch
        );
        let record = build_record_from_captured_v3(
            &capture_fixture(),
            &input,
            &mut deterministic_id_source(),
        )
        .unwrap();
        let projection =
            build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |value| {
                Ok(serde_json::to_vec(value).unwrap().len())
            })
            .unwrap();
        let PacketProjectionV3Dto::Complete {
            evidence,
            continuation,
            ..
        } = projection
        else {
            panic!("bounded page action must fit");
        };
        assert!(continuation.is_none());
        assert_eq!(
            evidence.as_slice()[0].packet_request_patch.as_ref(),
            Some(&patch)
        );
        // Action metadata is part of the actual serialized 16-KiB response.
        // A source-preserving overflow must refuse the whole representation,
        // rather than dropping executable actions to conceal its true size.
        let mut observed_action_boundary = false;
        for payload in (3000..3900).step_by(64) {
            let mut large = input.clone();
            large.evidence = (7..11)
                .map(|id| {
                    let mut row = input.evidence[0].clone();
                    row.identity.evidence_id = identity(&format!("evidence-{id}"));
                    row.symbol_id = Some(SymbolIdTextV3::new(id.to_string()).unwrap());
                    row.summary = Some(
                        SummaryTextV3::new(format!(
                            "```text\n>    1 | {}\n```",
                            "x".repeat(payload)
                        ))
                        .unwrap(),
                    );
                    let patch = row.packet_request_patch.as_mut().unwrap();
                    let mut probes = patch.probes.as_slice().to_vec();
                    let PacketProbeDto::Continuation { selector, .. } = &mut probes[0] else {
                        unreachable!()
                    };
                    selector.stable_identity = format!("node:{id}");
                    selector.symbol_id = Some(id.to_string());
                    patch.probes = BoundedVecV3::new(probes).unwrap();
                    row
                })
                .collect();
            let record = build_record_from_captured_v3(
                &capture_fixture(),
                &large,
                &mut deterministic_id_source(),
            )
            .unwrap();
            let mut boundary = false;
            let projected = build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |value| {
                let bytes = serde_json::to_vec(value).unwrap().len();
                if matches!(value, PacketProjectionV3Dto::Complete { .. }) {
                    let mut without = serde_json::to_value(value).unwrap();
                    for row in without["evidence"].as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("packet_request_patch"); }
                    boundary |= bytes > codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES && serde_json::to_vec(&without).unwrap().len() <= codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES;
                }
                Ok(bytes)
            }).unwrap();
            if boundary {
                assert!(matches!(
                    projected,
                    PacketProjectionV3Dto::BudgetExceeded { .. }
                ));
                let mut compact = large.clone();
                for row in &mut compact.evidence {
                    let delivery = row.source_delivery.as_ref().unwrap();
                    let option = codestory_contracts::api::DrillOptionDto::omitted_symbol(
                        "source-page",
                        row.symbol_id.as_ref().unwrap().as_str(),
                    )
                    .with_source_range(PacketSourceRangeV1 {
                        start_line: delivery.next_start_line.unwrap(),
                        end_line: delivery.requested_range.end_line,
                    });
                    let patch = row.packet_request_patch.as_mut().unwrap();
                    patch.probes = BoundedVecV3::new(vec![]).unwrap();
                    patch.parent_packet_id = Some(input.request_id.clone());
                    patch.option_ids = BoundedVecV3::new(vec![identity(&option.id)]).unwrap();
                }
                let record = build_record_from_captured_v3(
                    &capture_fixture(),
                    &compact,
                    &mut deterministic_id_source(),
                )
                .unwrap();
                let compact_projection = build_packet_projection_v3(
                    &record,
                    DiagnosticsCapabilityV3Dto::Unavailable,
                    |value| Ok(serde_json::to_vec(value).unwrap().len()),
                )
                .unwrap();
                let PacketProjectionV3Dto::Complete { evidence, .. } = compact_projection else {
                    panic!("equivalent compact actions must rescue this action-induced overflow");
                };
                assert_eq!(evidence.as_slice().len(), large.evidence.len());
                for old in &large.evidence {
                    let served = evidence
                        .as_slice()
                        .iter()
                        .find(|row| row.identity == old.identity)
                        .unwrap();
                    assert_eq!(served.summary, old.summary);
                    assert_eq!(served.source_delivery, old.source_delivery);
                    assert_eq!(
                        (served.start_line, served.end_line),
                        (old.start_line, old.end_line)
                    );
                    assert_eq!(
                        (&served.path, &served.symbol_id),
                        (&old.path, &old.symbol_id)
                    );
                    assert!(served.packet_request_patch.is_some());
                }
                println!(
                    "equivalent compact source actions rescued {payload}-byte source rows without dropping content"
                );
                observed_action_boundary = true;
                break;
            }
        }
        assert!(
            observed_action_boundary,
            "fixture must exercise an action-induced whole-response overflow"
        );
        for mutation in [
            "core",
            "retrieval",
            "project",
            "node",
            "path",
            "start",
            "end",
            "mixed",
        ] {
            let mut hostile = input.clone();
            let bad = hostile.evidence[0].packet_request_patch.as_mut().unwrap();
            if mutation == "mixed" {
                bad.parent_packet_id = Some(identity("other"));
            } else if mutation == "core" {
                bad.core_generation_id = identity("old-core");
            } else if mutation == "retrieval" {
                bad.retrieval_generation = None;
            } else {
                let mut probes = bad.probes.as_slice().to_vec();
                let PacketProbeDto::Continuation {
                    project_id,
                    selector,
                    ..
                } = &mut probes[0]
                else {
                    unreachable!()
                };
                match mutation {
                    "project" => *project_id = "other".into(),
                    "node" => selector.stable_identity = "node:8".into(),
                    "path" => selector.path = Some("src/other.rs".into()),
                    "start" => selector.source_range.as_mut().unwrap().start_line += 1,
                    _ => selector.source_range.as_mut().unwrap().end_line -= 1,
                };
                bad.probes = BoundedVecV3::new(probes).unwrap();
            }
            assert!(
                matches!(
                    build_record_from_captured_v3(
                        &capture_fixture(),
                        &hostile,
                        &mut deterministic_id_source()
                    ),
                    Err(RecordValidationErrorV3::InvalidPacketRequestPatch(_))
                ),
                "{mutation}"
            );
        }
    }

    #[test]
    fn complete_source_receipts_survive_compaction_and_reject_false_delivery() {
        use crate::agent::packet_projection_v3::build_packet_projection_v3;
        use codestory_contracts::compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1};
        use codestory_contracts::packet_projection_v3::{
            DiagnosticsCapabilityV3Dto, PacketProjectionV3Dto,
        };
        let mut input = finalized_fixture();
        let receipt = PacketSourceDeliveryV1 {
            requested_range: PacketSourceRangeV1 {
                start_line: 1,
                end_line: 10,
            },
            source_sha256: "a".repeat(64),
            remaining_source_bytes: 100,
            next_start_line: Some(2),
            blocked_line_rendered_bytes: None,
        };
        input.evidence[0].source_delivery = Some(receipt.clone());
        input.gaps[0].kind = GapKindV3Dto::SourceUnavailable;
        input.gaps[0].message = Some(
            codestory_contracts::packet_projection_v3::MessageTextV3::new(
                "zero complete lines delivered; source bytes remain",
            )
            .unwrap(),
        );
        let mut ids = deterministic_id_source();
        let record = build_record_from_captured_v3(&capture_fixture(), &input, &mut ids).unwrap();
        let projected = build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |candidate| {
            let extra_optional = matches!(candidate, PacketProjectionV3Dto::Complete { evidence, .. } if evidence.as_slice().iter().any(|row| row.source_delivery.is_none() && row.summary.is_some()));
            Ok(if extra_optional { 20_000 } else { 100 })
        }).unwrap();
        let PacketProjectionV3Dto::Complete { evidence, gaps, .. } = projected else {
            panic!("mandatory receipt fits after unrelated optional prose compaction")
        };
        let delivered = evidence
            .as_slice()
            .iter()
            .find(|row| row.source_delivery.is_some())
            .unwrap();
        assert_eq!(delivered.source_delivery, Some(receipt));
        assert!(
            delivered.summary.is_some(),
            "delivery metadata cannot survive without its actual source page"
        );
        assert!(
            gaps.as_slice()
                .iter()
                .any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.is_some())
        );
        input.evidence[0]
            .source_delivery
            .as_mut()
            .unwrap()
            .next_start_line = Some(1);
        let mut ids = deterministic_id_source();
        assert!(matches!(
            build_record_from_captured_v3(&capture_fixture(), &input, &mut ids),
            Err(RecordValidationErrorV3::InvalidSourceDelivery(_))
        ));
    }

    #[test]
    fn continuation_progress_survives_record_capture_and_public_projection() {
        use crate::agent::packet_projection_v3::build_packet_projection_v3;
        use codestory_contracts::packet_projection_v3::{
            DiagnosticsCapabilityV3Dto, PacketProjectionV3Dto,
        };
        let input = finalized_fixture();
        let mut ids = deterministic_id_source();
        let record = build_record_from_captured_v3(&capture_fixture(), &input, &mut ids).unwrap();
        assert_eq!(
            record
                .continuation()
                .unwrap()
                .gap_ids
                .as_slice()
                .iter()
                .map(|gap| gap.gap_id.as_str())
                .collect::<Vec<_>>(),
            vec!["gap-b", "gap-a"],
            "record capture must preserve semantic continuation priority"
        );
        for compact in [false, true] {
            let projection = build_packet_projection_v3(
                &record,
                DiagnosticsCapabilityV3Dto::Unavailable,
                |candidate| {
                    let has_context = matches!(candidate, PacketProjectionV3Dto::Complete {
                        evidence, ..
                    } if evidence.as_slice().iter().any(|row| row.summary.is_some()));
                    Ok(if compact && has_context { 20_000 } else { 100 })
                },
            )
            .unwrap();
            let PacketProjectionV3Dto::Complete {
                gaps, continuation, ..
            } = projection
            else {
                panic!("mandatory identities fit the public budget");
            };
            assert_eq!(
                gaps.as_slice()
                    .iter()
                    .map(|gap| gap.identity.gap_id.as_str())
                    .collect::<Vec<_>>(),
                if compact {
                    vec!["gap-a", "gap-b", "packet-optional-context-omitted"]
                } else {
                    vec!["gap-a", "gap-b"]
                }
            );
            assert_eq!(
                continuation
                    .unwrap()
                    .gap_ids
                    .as_slice()
                    .iter()
                    .map(|gap| gap.gap_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["gap-b", "gap-a"],
                "public projection and context compaction must preserve continuation priority"
            );
        }

        let mut duplicate = input;
        duplicate.continuation.as_mut().unwrap().gap_ids = BoundedVecV3::new(vec![
            GapIdentityV3Dto {
                gap_id: identity("gap-b"),
            },
            GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            },
            GapIdentityV3Dto {
                gap_id: identity("gap-b"),
            },
        ])
        .unwrap();
        let mut ids = deterministic_id_source();
        assert!(
            matches!(
                build_record_from_captured_v3(&capture_fixture(), &duplicate, &mut ids),
                Err(RecordValidationErrorV3::DuplicateContinuationGapReference(
                    _
                ))
            ),
            "nonadjacent duplicate refs remain invalid when order is preserved"
        );
    }

    #[test]
    fn packet_execution_record_v3_is_canonical_owned_and_immutable() {
        let mut input = finalized_fixture();
        let mut ids = deterministic_id_source();
        let record = build_record_from_captured_v3(&capture_fixture(), &input, &mut ids).unwrap();
        let before_borrow = record.clone();

        assert_eq!(
            record.packet_id().as_str(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(record.caller_id().as_str(), "caller-1");
        assert_eq!(record.request_id().as_str(), "request-1");
        assert_eq!(record.plan_version(), PACKET_EXECUTION_PLAN_VERSION_V3);
        assert_eq!(record.project().project_id, "project-1");
        assert_eq!(record.project().workspace_id, "workspace-1");
        assert_eq!(record.project().artifact_scope_id, "artifact-1");
        assert_eq!(
            record.evidence()[0].identity.evidence_id.as_str(),
            "evidence-a"
        );
        assert_eq!(record.gaps()[0].identity.gap_id.as_str(), "gap-a");
        assert_eq!(
            record.diagnostics()[0].diagnostic_id.as_str(),
            "diagnostic-a"
        );
        assert_eq!(
            record.continuation().unwrap().gap_ids.as_slice()[0]
                .gap_id
                .as_str(),
            "gap-b"
        );

        input.request.question = "caller-mutated".to_owned();
        input.evidence[0].summary = Some(SummaryTextV3::new("caller-mutated").unwrap());
        input.diagnostics[0].code = DiagnosticCodeTextV3::new("caller_mutated").unwrap();
        assert_eq!(record, before_borrow);
        assert_eq!(record.request().question(), "hello");
        assert_eq!(record.retrieval().state, RetrievalStateV3Dto::Full);
        assert_eq!(
            record.diagnostics()[0].category(),
            DiagnosticCategoryV3Dto::Coverage
        );
        assert_eq!(record.diagnostics()[0].code().as_str(), "source_gap");
        assert_eq!(record, record.clone());

        let mut reordered = finalized_fixture();
        reordered.evidence.reverse();
        reordered.gaps.reverse();
        reordered.diagnostics.reverse();
        let mut reordered_ids = deterministic_id_source();
        assert_eq!(
            build_record_from_captured_v3(&capture_fixture(), &reordered, &mut reordered_ids)
                .unwrap(),
            before_borrow,
            "canonical evidence, gap, and diagnostic rows must not depend on caller vector order"
        );
    }

    #[test]
    fn packet_execution_record_v3_rejects_hostile_publication_and_identity_shapes() {
        let input = finalized_fixture();
        let mut ids = deterministic_id_source();

        let mut capture = capture_fixture();
        capture.core_project_id = "other-project".to_owned();
        assert_eq!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::ProjectMismatch)
        );

        let mut capture = capture_fixture();
        capture.core_generation_id.clear();
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::EmptyIdentity("core_generation_id"))
        ));

        let mut capture = capture_fixture();
        capture.core_run_id = "x".repeat(257);
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::IdentityTooLong("core_run_id", 257))
        ));

        let mut capture = capture_fixture();
        capture.retrieval.as_mut().unwrap().core_run_id = "wrong-run".to_owned();
        assert_eq!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::RetrievalCoreSkew)
        );

        let mut capture = capture_fixture();
        capture.retrieval.as_mut().unwrap().retrieval_input_hash = "not-a-hash".to_owned();
        assert_eq!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::InvalidRetrievalHash)
        );

        let mut capture = capture_fixture();
        capture.retrieval = None;
        assert_eq!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::FullRetrievalWithoutPublication)
        );

        let mut unavailable = finalized_fixture();
        unavailable.retrieval = RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Unavailable,
            generation_id: None,
        };
        assert_eq!(
            build_record_from_captured_v3(&capture_fixture(), &unavailable, &mut ids),
            Err(RecordValidationErrorV3::RetrievalStateMismatch)
        );

        let mut capture = capture_fixture();
        capture.project.workspace_id.clear();
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::EmptyIdentity("workspace_id"))
        ));

        let mut input = finalized_fixture();
        input.request.core_generation_id = Some("wrong-core".to_owned());
        assert_eq!(
            build_record_from_captured_v3(&capture_fixture(), &input, &mut ids),
            Err(RecordValidationErrorV3::RequestedCoreGenerationMismatch)
        );

        let mut input = finalized_fixture();
        input.request.retrieval_generation = Some("wrong-retrieval".to_owned());
        assert_eq!(
            build_record_from_captured_v3(&capture_fixture(), &input, &mut ids),
            Err(RecordValidationErrorV3::RequestedRetrievalGenerationMismatch)
        );

        let mut input = finalized_fixture();
        input.continuation.as_mut().unwrap().remaining_rounds = 0;
        assert_eq!(
            build_record_from_captured_v3(&capture_fixture(), &input, &mut ids),
            Err(RecordValidationErrorV3::ZeroContinuationRounds)
        );
    }

    #[test]
    fn packet_execution_record_v3_rejects_duplicate_and_dangling_finalized_rows() {
        let capture = capture_fixture();
        let mut ids = deterministic_id_source();

        let mut input = finalized_fixture();
        input.evidence.push(evidence("evidence-a"));
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::DuplicateEvidenceIdentity(_))
        ));

        let mut input = finalized_fixture();
        input.evidence = (0..=PACKET_EVIDENCE_ROWS_MAX_V3)
            .map(|index| evidence(&format!("evidence-{index:03}")))
            .collect();
        assert_eq!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::TooManyEvidenceRows(
                PACKET_EVIDENCE_ROWS_MAX_V3 + 1
            ))
        );

        let mut input = finalized_fixture();
        input.gaps.push(gap("gap-a"));
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::DuplicateGapIdentity(_))
        ));

        let mut input = finalized_fixture();
        input.diagnostics.push(diagnostic("diagnostic-a", &[], &[]));
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::DuplicateDiagnosticIdentity(_))
        ));

        let mut input = finalized_fixture();
        input.continuation.as_mut().unwrap().gap_ids = BoundedVecV3::new(vec![GapIdentityV3Dto {
            gap_id: identity("missing-gap"),
        }])
        .unwrap();
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::UnknownContinuationGap(_))
        ));

        let mut input = finalized_fixture();
        input.diagnostics[0].evidence_ids = vec![EvidenceIdentityV3Dto {
            evidence_id: identity("missing-evidence"),
        }];
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::UnknownDiagnosticEvidence(_))
        ));

        let mut input = finalized_fixture();
        input.diagnostics[0].gap_ids = vec![
            GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            },
            GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            },
        ];
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::DuplicateDiagnosticGapReference(_))
        ));

        let mut input = finalized_fixture();
        input.continuation.as_mut().unwrap().gap_ids = BoundedVecV3::new(vec![
            GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            },
            GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            },
        ])
        .unwrap();
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::DuplicateContinuationGapReference(
                _
            ))
        ));

        let mut input = finalized_fixture();
        input.diagnostics[0].evidence_ids = vec![
            EvidenceIdentityV3Dto {
                evidence_id: identity("evidence-a"),
            },
            EvidenceIdentityV3Dto {
                evidence_id: identity("evidence-a"),
            },
        ];
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::DuplicateDiagnosticEvidenceReference(_))
        ));

        let mut input = finalized_fixture();
        input.diagnostics[0].gap_ids = vec![GapIdentityV3Dto {
            gap_id: identity("missing-gap"),
        }];
        assert!(matches!(
            build_record_from_captured_v3(&capture, &input, &mut ids),
            Err(RecordValidationErrorV3::UnknownDiagnosticGap(_))
        ));
    }

    #[test]
    fn packet_execution_record_v3_product_ids_are_independent_uuid_v4_values() {
        let capture = capture_fixture();
        let input = finalized_fixture();
        let first = build_record_from_captured_product_v3(&capture, &input).unwrap();
        let second = build_record_from_captured_product_v3(&capture, &input).unwrap();

        assert_ne!(first.packet_id(), second.packet_id());
        for packet_id in [first.packet_id(), second.packet_id()] {
            let decoded = uuid::Uuid::parse_str(packet_id.as_str()).expect("UUID packet id");
            assert_eq!(decoded.get_version_num(), 4);
            assert_eq!(decoded.get_variant(), uuid::Variant::RFC4122);
        }
        assert_eq!(first.question_sha256(), second.question_sha256());
        assert_eq!(first.request_sha256(), second.request_sha256());
    }

    #[test]
    fn packet_execution_record_v3_captures_one_ready_public_operation_pin() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.public_operation_service();
        let mut records = None;

        let operation = service
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                let input = finalized_fixture_for_active_pin(&service, finalized_fixture());
                let first = build_packet_execution_record_v3(&service, &input)
                    .map_err(RecordValidationErrorV3::into_api_error)?;
                let second = build_packet_execution_record_v3(&service, &input)
                    .map_err(RecordValidationErrorV3::into_api_error)?;
                records = Some((first.clone(), second));
                Ok(first)
            })
            .expect("ready pinned capture");

        assert_eq!(operation.attempt, 1);
        let core = operation.core_publication.as_ref().unwrap();
        assert_eq!(
            operation.value.core_publication().generation_id.as_str(),
            core.generation_id
        );
        assert_eq!(
            operation.value.core_publication().run_id.as_str(),
            core.run_id
        );
        let retrieval = operation.retrieval_publication.as_ref().unwrap();
        assert_eq!(
            operation
                .value
                .retrieval_publication()
                .unwrap()
                .retrieval_generation
                .as_str(),
            retrieval.retrieval_generation
        );
        assert_eq!(
            operation
                .value
                .retrieval_publication()
                .unwrap()
                .retrieval_input_sha256
                .as_str(),
            retrieval.retrieval_input_hash
        );
        assert_eq!(
            operation
                .value
                .retrieval_publication()
                .unwrap()
                .semantic_generation
                .as_str(),
            retrieval.semantic_generation
        );
        assert_eq!(retrieval.core_generation_id, core.generation_id);
        assert_eq!(retrieval.core_run_id, core.run_id);
        let (first, second) = records.unwrap();
        assert_ne!(first.packet_id(), second.packet_id());
        assert_eq!(first.question_sha256(), second.question_sha256());
        assert_eq!(first.request_sha256(), second.request_sha256());
    }

    #[test]
    fn packet_execution_record_v3_cannot_capture_without_an_active_pin() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.public_operation_service();
        assert_eq!(
            build_packet_execution_record_v3(&service, &finalized_fixture()),
            Err(RecordValidationErrorV3::NoActiveCorePin)
        );
    }

    #[test]
    fn packet_execution_record_v3_does_not_bypass_mid_operation_freshness_refusal() {
        let fixture =
            crate::services::activation_tests::ready_activation_fixture_with_observer_setup(
                |root, controller| {
                    // Model a notification that has not arrived yet. The content
                    // fence must still prevent a second build after source drift.
                    let observer =
                        crate::tests::freshness_observer_tests::scripted_session(root, |_| {
                            Vec::new()
                        });
                    controller.install_source_observer_for_test(root, Arc::new(observer));
                },
            );
        let service = fixture.runtime.public_operation_service();
        let source = fixture.project.path().join("metadata.rs");
        let mut builds = 0;

        let refusal = service
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                builds += 1;
                assert_eq!(
                    builds, 1,
                    "source drift must refuse the next build before capture"
                );
                let input = finalized_fixture_for_active_pin(&service, finalized_fixture());
                let record = build_packet_execution_record_v3(&service, &input)
                    .map_err(RecordValidationErrorV3::into_api_error)?;
                fs::write(&source, "// CHANGED_PACKET_RECORD_SOURCE\n")
                    .expect("mutate source during capture");
                Ok(record)
            })
            .expect_err("the existing post-operation fence must reject source drift");

        assert_eq!(builds, 1);
        assert_eq!(refusal.code, "project_unavailable");
    }

    fn finalized_fixture_for_active_pin(
        service: &crate::services::PublicOperationService,
        mut input: FinalizedPacketExecutionInputV3,
    ) -> FinalizedPacketExecutionInputV3 {
        let retrieval = service
            .active_publication()
            .expect("active core publication")
            .retrieval_publication
            .expect("active retrieval publication");
        input.retrieval.generation_id = Some(identity(&retrieval.retrieval_generation));
        input
    }
}
