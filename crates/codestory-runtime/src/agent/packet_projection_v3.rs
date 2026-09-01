//! Pure v3 packet, context, and search projection builders.

#![allow(dead_code)]

use std::collections::BTreeSet;

use codestory_contracts::packet_projection_v3::{
    BoundViolationV3, BoundedVecV3, ContextEvidenceRowV3Dto, ContextProjectionKindV3Dto,
    ContextProjectionV3Dto, ContextTargetV3Dto, DIAGNOSTIC_ROWS_MAX_V3,
    DiagnosticArtifactKindV3Dto, DiagnosticArtifactV3Dto, DiagnosticReferenceV3Dto,
    DiagnosticRowV3Dto, DiagnosticsCapabilityV3Dto, EvidenceAvailabilityV3Dto, GAP_ROWS_MAX_V3,
    GapIdentityV3Dto, GapKindV3Dto, IdentityTextV3, PACKET_PROJECTION_V3_SCHEMA_VERSION,
    PacketProjectionV3Dto, PacketRequestIdentityV3Dto, ProjectionGapRowV3Dto,
    PublicationIdentityV3Dto, RetrievalStateDescriptorV3Dto, RetrievalStateV3Dto,
    SearchEvidenceRowV3Dto, SearchProjectionKindV3Dto, SearchProjectionV3Dto, Sha256DigestV3Dto,
};
use sha2::{Digest, Sha256};

use super::packet_execution_record_v3::PacketExecutionRecordV3;

pub(crate) const PACKET_PUBLIC_RESULT_MAX_BYTES_V3: usize = 16 * 1024;
pub(crate) const DIAGNOSTIC_ARTIFACT_MAX_BYTES_V3: usize = 1024 * 1024;

const DIAGNOSTIC_ARTIFACT_ID_DOMAIN_V3: &[u8] = b"codestory.packet_diagnostic_v3.id\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectionBuildErrorV3 {
    MeasurementFailed,
    FallbackTooLarge { required_bytes: usize },
    InvalidInput(ProjectionInputErrorV3),
    BoundViolation(BoundViolationV3),
    CanonicalJson(String),
    InvalidDigest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectionInputErrorV3 {
    EmptyContextTarget,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiagnosticArtifactBuildV3 {
    Complete {
        artifact: Box<DiagnosticArtifactV3Dto>,
        bytes: DiagnosticArtifactBytesV3,
        reference: DiagnosticReferenceV3Dto,
    },
    TooLarge {
        required_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticArtifactBytesV3(Box<[u8]>);

impl DiagnosticArtifactBytesV3 {
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalizedContextProjectionInputV3 {
    identity: PacketRequestIdentityV3Dto,
    publication: PublicationIdentityV3Dto,
    retrieval: RetrievalStateDescriptorV3Dto,
    target: ContextTargetV3Dto,
    evidence: Vec<ContextEvidenceRowV3Dto>,
    gaps: Vec<ProjectionGapRowV3Dto>,
    continuation: Option<codestory_contracts::packet_projection_v3::ContinuationStateV3Dto>,
    diagnostics: DiagnosticsCapabilityV3Dto,
    diagnostic_rows: Vec<DiagnosticRowV3Dto>,
}

impl FinalizedContextProjectionInputV3 {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        identity: PacketRequestIdentityV3Dto,
        publication: PublicationIdentityV3Dto,
        retrieval: RetrievalStateDescriptorV3Dto,
        target: ContextTargetV3Dto,
        evidence: Vec<ContextEvidenceRowV3Dto>,
        gaps: Vec<ProjectionGapRowV3Dto>,
        continuation: Option<codestory_contracts::packet_projection_v3::ContinuationStateV3Dto>,
        diagnostics: DiagnosticsCapabilityV3Dto,
        diagnostic_rows: Vec<DiagnosticRowV3Dto>,
    ) -> Self {
        Self {
            identity,
            publication,
            retrieval,
            target,
            evidence,
            gaps,
            continuation,
            diagnostics,
            diagnostic_rows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalizedSearchProjectionInputV3 {
    identity: PacketRequestIdentityV3Dto,
    publication: PublicationIdentityV3Dto,
    retrieval: RetrievalStateDescriptorV3Dto,
    evidence: Vec<SearchEvidenceRowV3Dto>,
    gaps: Vec<ProjectionGapRowV3Dto>,
    continuation: Option<codestory_contracts::packet_projection_v3::ContinuationStateV3Dto>,
    diagnostics: DiagnosticsCapabilityV3Dto,
    diagnostic_rows: Vec<DiagnosticRowV3Dto>,
}

impl FinalizedSearchProjectionInputV3 {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        identity: PacketRequestIdentityV3Dto,
        publication: PublicationIdentityV3Dto,
        retrieval: RetrievalStateDescriptorV3Dto,
        evidence: Vec<SearchEvidenceRowV3Dto>,
        gaps: Vec<ProjectionGapRowV3Dto>,
        continuation: Option<codestory_contracts::packet_projection_v3::ContinuationStateV3Dto>,
        diagnostics: DiagnosticsCapabilityV3Dto,
        diagnostic_rows: Vec<DiagnosticRowV3Dto>,
    ) -> Self {
        Self {
            identity,
            publication,
            retrieval,
            evidence,
            gaps,
            continuation,
            diagnostics,
            diagnostic_rows,
        }
    }
}

pub(crate) fn build_packet_projection_v3(
    record: &PacketExecutionRecordV3,
    diagnostics: DiagnosticsCapabilityV3Dto,
    measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<PacketProjectionV3Dto, ProjectionBuildErrorV3> {
    let mut projection = packet_complete_candidate_v3(record, diagnostics)?;
    finalize_packet_projection_v3(&mut projection, measure)?;
    Ok(projection)
}

pub(crate) fn finalize_packet_projection_v3(
    projection: &mut PacketProjectionV3Dto,
    mut measure: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<usize, ProjectionBuildErrorV3> {
    let required_complete_bytes =
        measure(projection).map_err(|_| ProjectionBuildErrorV3::MeasurementFailed)?;
    if required_complete_bytes <= PACKET_PUBLIC_RESULT_MAX_BYTES_V3 {
        return Ok(required_complete_bytes);
    }
    let PacketProjectionV3Dto::Complete {
        schema_version,
        identity,
        publication,
        retrieval,
        diagnostics,
        evidence,
        gaps,
        continuation,
        ..
    } = projection
    else {
        return Err(ProjectionBuildErrorV3::FallbackTooLarge {
            required_bytes: required_complete_bytes,
        });
    };

    let original_evidence = evidence.as_slice().to_vec();
    let original_gaps = gaps.as_slice().to_vec();
    let original_continuation = continuation.clone();
    let schema_version = *schema_version;
    let identity = identity.clone();
    let publication = publication.clone();
    let retrieval = retrieval.clone();
    let diagnostics = diagnostics.clone();

    let candidate = |detailed_rows: usize| {
        compact_packet_candidate_v3(
            schema_version,
            identity.clone(),
            publication.clone(),
            retrieval.clone(),
            diagnostics.clone(),
            &original_evidence,
            &original_gaps,
            original_continuation.as_ref(),
            detailed_rows,
        )
    };

    // Every evidence and gap identity is mandatory. Optional row context and
    // prose are the only fields compacted. Measure the mandatory complete
    // envelope first, then binary-search the largest relevance-ordered prefix
    // that can retain its useful source or relation context.
    let minimal = candidate(0)?;
    let minimal_bytes = measure(&minimal).map_err(|_| ProjectionBuildErrorV3::MeasurementFailed)?;
    if minimal_bytes <= PACKET_PUBLIC_RESULT_MAX_BYTES_V3 {
        let mut low = 0_usize;
        let mut high = original_evidence.len();
        let mut best = minimal;
        let mut best_bytes = minimal_bytes;
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            let next = candidate(middle)?;
            let next_bytes =
                measure(&next).map_err(|_| ProjectionBuildErrorV3::MeasurementFailed)?;
            if next_bytes <= PACKET_PUBLIC_RESULT_MAX_BYTES_V3 {
                low = middle;
                best = next;
                best_bytes = next_bytes;
            } else {
                high = middle - 1;
            }
        }
        *projection = best;
        return Ok(best_bytes);
    }

    let fallback = PacketProjectionV3Dto::BudgetExceeded {
        schema_version,
        identity,
        publication,
        status: EvidenceAvailabilityV3Dto::Unavailable,
        retrieval,
        diagnostics,
        gaps: packet_budget_exceeded_gaps_v3(),
        maximum_bytes: PACKET_PUBLIC_RESULT_MAX_BYTES_V3 as u64,
        required_complete_bytes: required_complete_bytes as u64,
        answer_sufficiency: Default::default(),
    };
    let fallback_bytes =
        measure(&fallback).map_err(|_| ProjectionBuildErrorV3::MeasurementFailed)?;
    if fallback_bytes > PACKET_PUBLIC_RESULT_MAX_BYTES_V3 {
        return Err(ProjectionBuildErrorV3::FallbackTooLarge {
            required_bytes: fallback_bytes,
        });
    }
    *projection = fallback;
    Ok(fallback_bytes)
}

fn packet_budget_exceeded_gaps_v3() -> BoundedVecV3<ProjectionGapRowV3Dto, GAP_ROWS_MAX_V3> {
    BoundedVecV3::new(vec![ProjectionGapRowV3Dto {
        identity: GapIdentityV3Dto {
            gap_id: IdentityTextV3::new("packet-output-budget-exceeded")
                .expect("static budget gap identity is bounded"),
        },
        kind: GapKindV3Dto::OutputBudgetExceeded,
        message: None,
    }])
    .expect("one budget gap fits the closed projection")
}

fn packet_complete_candidate_v3(
    record: &PacketExecutionRecordV3,
    diagnostics: DiagnosticsCapabilityV3Dto,
) -> Result<PacketProjectionV3Dto, ProjectionBuildErrorV3> {
    let evidence = record.evidence().to_vec();
    let gaps = record.gaps().to_vec();
    let continuation = canonical_continuation_v3(record.continuation(), &gaps)?;
    Ok(PacketProjectionV3Dto::Complete {
        schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
        identity: identity_from_record(record),
        publication: publication_from_record(record),
        status: evidence_availability_v3(
            continuation.is_some(),
            !evidence.is_empty(),
            record.retrieval(),
            &gaps,
        ),
        retrieval: record.retrieval().clone(),
        evidence: BoundedVecV3::new(evidence).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        gaps: BoundedVecV3::new(gaps).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        continuation,
        diagnostics,
        answer_sufficiency: Default::default(),
    })
}

#[allow(clippy::too_many_arguments)]
fn compact_packet_candidate_v3(
    schema_version: u16,
    identity: PacketRequestIdentityV3Dto,
    publication: PublicationIdentityV3Dto,
    retrieval: RetrievalStateDescriptorV3Dto,
    diagnostics: DiagnosticsCapabilityV3Dto,
    original_evidence: &[codestory_contracts::packet_projection_v3::PacketEvidenceRowV3Dto],
    original_gaps: &[ProjectionGapRowV3Dto],
    original_continuation: Option<
        &codestory_contracts::packet_projection_v3::ContinuationStateV3Dto,
    >,
    detailed_rows: usize,
) -> Result<PacketProjectionV3Dto, ProjectionBuildErrorV3> {
    let mut evidence = original_evidence.to_vec();
    for row in evidence.iter_mut().skip(detailed_rows) {
        row.summary = None;
    }
    let mut gaps = original_gaps.to_vec();
    for row in &mut gaps {
        row.message = None;
    }
    if gaps.len() < GAP_ROWS_MAX_V3 {
        gaps.push(ProjectionGapRowV3Dto {
            identity: GapIdentityV3Dto {
                gap_id: IdentityTextV3::new("packet-optional-context-omitted")
                    .expect("static budget gap identity is bounded"),
            },
            kind: GapKindV3Dto::OutputBudgetExceeded,
            message: None,
        });
    }
    gaps.sort_by(|left, right| left.identity.cmp(&right.identity));
    gaps.dedup_by(|left, right| left.identity == right.identity);
    let continuation = canonical_continuation_v3(original_continuation, &gaps)?;
    Ok(PacketProjectionV3Dto::Complete {
        schema_version,
        identity,
        publication,
        status: evidence_availability_v3(
            continuation.is_some(),
            !evidence.is_empty(),
            &retrieval,
            &gaps,
        ),
        retrieval,
        evidence: BoundedVecV3::new(evidence).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        gaps: BoundedVecV3::new(gaps).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        continuation,
        diagnostics,
        answer_sufficiency: Default::default(),
    })
}

pub(crate) fn build_context_projection_v3(
    input: &FinalizedContextProjectionInputV3,
) -> Result<ContextProjectionV3Dto, ProjectionBuildErrorV3> {
    if input.target.path.is_none() && input.target.symbol_id.is_none() {
        return Err(ProjectionBuildErrorV3::InvalidInput(
            ProjectionInputErrorV3::EmptyContextTarget,
        ));
    }
    let mut evidence = input.evidence.clone();
    evidence.sort_by(|left, right| left.identity.cmp(&right.identity));
    reject_duplicate_evidence_v3(&evidence, |row| &row.identity)?;
    let gaps = canonical_gaps_v3(&input.gaps)?;
    let continuation = canonical_continuation_v3(input.continuation.as_ref(), &gaps)?;
    let diagnostic_rows =
        BoundedVecV3::<_, DIAGNOSTIC_ROWS_MAX_V3>::new(input.diagnostic_rows.clone())
            .map_err(ProjectionBuildErrorV3::BoundViolation)?;
    validate_diagnostic_references_v3(diagnostic_rows.as_slice(), &evidence, &gaps, |row| {
        &row.identity
    })?;
    Ok(ContextProjectionV3Dto {
        kind: ContextProjectionKindV3Dto::Complete,
        schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
        identity: input.identity.clone(),
        publication: input.publication.clone(),
        status: evidence_availability_v3(
            input.continuation.is_some(),
            !evidence.is_empty(),
            &input.retrieval,
            &gaps,
        ),
        target: input.target.clone(),
        evidence: BoundedVecV3::new(evidence).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        gaps: BoundedVecV3::new(gaps).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        continuation,
        diagnostics: input.diagnostics.clone(),
    })
}

pub(crate) fn build_search_projection_v3(
    input: &FinalizedSearchProjectionInputV3,
) -> Result<SearchProjectionV3Dto, ProjectionBuildErrorV3> {
    let mut evidence = input.evidence.clone();
    evidence.sort_by(|left, right| left.identity.cmp(&right.identity));
    reject_duplicate_evidence_v3(&evidence, |row| &row.identity)?;
    let gaps = canonical_gaps_v3(&input.gaps)?;
    let continuation = canonical_continuation_v3(input.continuation.as_ref(), &gaps)?;
    let diagnostic_rows =
        BoundedVecV3::<_, DIAGNOSTIC_ROWS_MAX_V3>::new(input.diagnostic_rows.clone())
            .map_err(ProjectionBuildErrorV3::BoundViolation)?;
    validate_diagnostic_references_v3(diagnostic_rows.as_slice(), &evidence, &gaps, |row| {
        &row.identity
    })?;
    Ok(SearchProjectionV3Dto {
        kind: SearchProjectionKindV3Dto::Complete,
        schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
        identity: input.identity.clone(),
        publication: input.publication.clone(),
        status: evidence_availability_v3(
            input.continuation.is_some(),
            !evidence.is_empty(),
            &input.retrieval,
            &gaps,
        ),
        evidence: BoundedVecV3::new(evidence).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        gaps: BoundedVecV3::new(gaps).map_err(ProjectionBuildErrorV3::BoundViolation)?,
        continuation,
        retrieval: input.retrieval.clone(),
        diagnostics: input.diagnostics.clone(),
    })
}

fn reject_duplicate_evidence_v3<T>(
    evidence: &[T],
    identity: impl Fn(&T) -> &codestory_contracts::packet_projection_v3::EvidenceIdentityV3Dto,
) -> Result<(), ProjectionBuildErrorV3> {
    for pair in evidence.windows(2) {
        let left = identity(&pair[0]);
        let right = identity(&pair[1]);
        if left == right {
            return Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateEvidenceIdentity(
                    left.evidence_id.as_str().to_owned(),
                ),
            ));
        }
    }
    Ok(())
}

fn canonical_gaps_v3(
    source: &[ProjectionGapRowV3Dto],
) -> Result<Vec<ProjectionGapRowV3Dto>, ProjectionBuildErrorV3> {
    let mut gaps = source.to_vec();
    gaps.sort_by(|left, right| left.identity.cmp(&right.identity));
    for pair in gaps.windows(2) {
        if pair[0].identity == pair[1].identity {
            return Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateGapIdentity(
                    pair[0].identity.gap_id.as_str().to_owned(),
                ),
            ));
        }
    }
    Ok(gaps)
}

fn canonical_continuation_v3(
    source: Option<&codestory_contracts::packet_projection_v3::ContinuationStateV3Dto>,
    gaps: &[ProjectionGapRowV3Dto],
) -> Result<
    Option<codestory_contracts::packet_projection_v3::ContinuationStateV3Dto>,
    ProjectionBuildErrorV3,
> {
    let Some(source) = source else {
        return Ok(None);
    };
    source.validate().map_err(|_| {
        ProjectionBuildErrorV3::InvalidInput(ProjectionInputErrorV3::ZeroContinuationRounds)
    })?;
    let gap_ids = gaps
        .iter()
        .map(|gap| gap.identity.gap_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut references = source.gap_ids.as_slice().to_vec();
    references.sort();
    for pair in references.windows(2) {
        if pair[0] == pair[1] {
            return Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateContinuationGapReference(
                    pair[0].gap_id.as_str().to_owned(),
                ),
            ));
        }
    }
    for reference in &references {
        if !gap_ids.contains(reference.gap_id.as_str()) {
            return Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::UnknownContinuationGap(
                    reference.gap_id.as_str().to_owned(),
                ),
            ));
        }
    }
    Ok(Some(
        codestory_contracts::packet_projection_v3::ContinuationStateV3Dto::new(
            source.continuation_id.clone(),
            source.remaining_rounds,
            BoundedVecV3::new(references).expect("bounded references remain bounded"),
        )
        .expect("validated continuation remains positive"),
    ))
}

fn validate_diagnostic_references_v3<T>(
    diagnostics: &[DiagnosticRowV3Dto],
    evidence: &[T],
    gaps: &[ProjectionGapRowV3Dto],
    identity: impl Fn(&T) -> &codestory_contracts::packet_projection_v3::EvidenceIdentityV3Dto,
) -> Result<(), ProjectionBuildErrorV3> {
    let mut ordered_diagnostics = diagnostics.iter().collect::<Vec<_>>();
    ordered_diagnostics.sort_by(|left, right| left.diagnostic_id.cmp(&right.diagnostic_id));
    for pair in ordered_diagnostics.windows(2) {
        if pair[0].diagnostic_id == pair[1].diagnostic_id {
            return Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateDiagnosticIdentity(
                    pair[0].diagnostic_id.as_str().to_owned(),
                ),
            ));
        }
    }
    let evidence_ids = evidence
        .iter()
        .map(|row| identity(row).evidence_id.as_str())
        .collect::<BTreeSet<_>>();
    let gap_ids = gaps
        .iter()
        .map(|gap| gap.identity.gap_id.as_str())
        .collect::<BTreeSet<_>>();
    for diagnostic in ordered_diagnostics {
        let mut evidence_references = diagnostic.evidence_ids.as_slice().to_vec();
        evidence_references.sort();
        for pair in evidence_references.windows(2) {
            if pair[0] == pair[1] {
                return Err(ProjectionBuildErrorV3::InvalidInput(
                    ProjectionInputErrorV3::DuplicateDiagnosticEvidenceReference(
                        pair[0].evidence_id.as_str().to_owned(),
                    ),
                ));
            }
        }
        for reference in &evidence_references {
            if !evidence_ids.contains(reference.evidence_id.as_str()) {
                return Err(ProjectionBuildErrorV3::InvalidInput(
                    ProjectionInputErrorV3::UnknownDiagnosticEvidence(
                        reference.evidence_id.as_str().to_owned(),
                    ),
                ));
            }
        }

        let mut gap_references = diagnostic.gap_ids.as_slice().to_vec();
        gap_references.sort();
        for pair in gap_references.windows(2) {
            if pair[0] == pair[1] {
                return Err(ProjectionBuildErrorV3::InvalidInput(
                    ProjectionInputErrorV3::DuplicateDiagnosticGapReference(
                        pair[0].gap_id.as_str().to_owned(),
                    ),
                ));
            }
        }
        for reference in &gap_references {
            if !gap_ids.contains(reference.gap_id.as_str()) {
                return Err(ProjectionBuildErrorV3::InvalidInput(
                    ProjectionInputErrorV3::UnknownDiagnosticGap(
                        reference.gap_id.as_str().to_owned(),
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn build_diagnostic_artifact_v3(
    record: &PacketExecutionRecordV3,
) -> Result<DiagnosticArtifactBuildV3, ProjectionBuildErrorV3> {
    let artifact_id = diagnostic_artifact_id_v3(record)?;
    let mut rows = record
        .diagnostics()
        .iter()
        .map(|row| {
            let mut evidence_ids = row.evidence_ids().to_vec();
            evidence_ids.sort();
            let mut gap_ids = row.gap_ids().to_vec();
            gap_ids.sort();
            DiagnosticRowV3Dto {
                diagnostic_id: row.diagnostic_id().clone(),
                category: row.category(),
                code: row.code().clone(),
                evidence_ids: BoundedVecV3::new(evidence_ids)
                    .expect("validated record reference bound"),
                gap_ids: BoundedVecV3::new(gap_ids).expect("validated record reference bound"),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.diagnostic_id.cmp(&right.diagnostic_id));
    let artifact = DiagnosticArtifactV3Dto {
        kind: DiagnosticArtifactKindV3Dto::Complete,
        schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
        artifact_id: artifact_id.clone(),
        packet_id: record.packet_id().clone(),
        publication: publication_from_record(record),
        rows: BoundedVecV3::<_, DIAGNOSTIC_ROWS_MAX_V3>::new(rows)
            .expect("validated record diagnostic bound"),
    };
    let bytes = codestory_agent::packet_execution_plan_v3::canonical_json_bytes_v3(&artifact)
        .map_err(ProjectionBuildErrorV3::CanonicalJson)?;
    if bytes.len() > DIAGNOSTIC_ARTIFACT_MAX_BYTES_V3 {
        return Ok(DiagnosticArtifactBuildV3::TooLarge {
            required_bytes: bytes.len() as u64,
        });
    }
    let sha256 = Sha256DigestV3Dto::new(format!("{:x}", Sha256::digest(&bytes)))
        .map_err(|_| ProjectionBuildErrorV3::InvalidDigest)?;
    let reference = DiagnosticReferenceV3Dto {
        artifact_id,
        sha256,
        byte_length: bytes.len() as u64,
        wall_expiry_epoch_ms: None,
    };
    Ok(DiagnosticArtifactBuildV3::Complete {
        artifact: Box::new(artifact),
        bytes: DiagnosticArtifactBytesV3(bytes.into_boxed_slice()),
        reference,
    })
}

fn identity_from_record(record: &PacketExecutionRecordV3) -> PacketRequestIdentityV3Dto {
    PacketRequestIdentityV3Dto {
        packet_id: record.packet_id().clone(),
        request_id: record.request_id().clone(),
        question_sha256: record.question_sha256().clone(),
    }
}

fn publication_from_record(record: &PacketExecutionRecordV3) -> PublicationIdentityV3Dto {
    PublicationIdentityV3Dto {
        core: record.core_publication().clone(),
        retrieval: record.retrieval_publication().cloned(),
    }
}

/// Classify only whether evidence can be consumed or continued.
///
/// Continuation wins, then an existing evidence row, then a typed unavailable
/// retrieval/source state. With none of those, the projection has no useful
/// evidence. This helper deliberately has no request, score, claim, or query
/// input from which it could infer answer authority.
fn evidence_availability_v3(
    has_continuation: bool,
    has_evidence: bool,
    retrieval: &RetrievalStateDescriptorV3Dto,
    gaps: &[ProjectionGapRowV3Dto],
) -> EvidenceAvailabilityV3Dto {
    if has_continuation {
        EvidenceAvailabilityV3Dto::ContinuationAvailable
    } else if has_evidence {
        EvidenceAvailabilityV3Dto::Available
    } else if retrieval.state == RetrievalStateV3Dto::Unavailable
        || gaps.iter().any(|gap| {
            matches!(
                gap.kind,
                GapKindV3Dto::RetrievalUnavailable | GapKindV3Dto::SourceUnavailable
            )
        })
    {
        EvidenceAvailabilityV3Dto::Unavailable
    } else {
        EvidenceAvailabilityV3Dto::NoUsefulEvidence
    }
}

fn diagnostic_artifact_id_v3(
    record: &PacketExecutionRecordV3,
) -> Result<IdentityTextV3, ProjectionBuildErrorV3> {
    let publication = publication_from_record(record);
    let publication_bytes =
        codestory_agent::packet_execution_plan_v3::canonical_json_bytes_v3(&publication)
            .map_err(ProjectionBuildErrorV3::CanonicalJson)?;
    let mut hasher = Sha256::new();
    hasher.update(DIAGNOSTIC_ARTIFACT_ID_DOMAIN_V3);
    for field in [
        record.packet_id().as_str().as_bytes(),
        record.request_id().as_str().as_bytes(),
        record.request_sha256().as_str().as_bytes(),
        publication_bytes.as_slice(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    IdentityTextV3::new(format!("diagnostic-{:x}", hasher.finalize()))
        .map_err(ProjectionBuildErrorV3::BoundViolation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_execution_record_v3::{
        FinalizedDiagnosticSourceRowV3, FinalizedPacketExecutionInputV3, PacketProfileV3,
        PacketRequestFingerprintV3, build_packet_execution_record_fixture_v3,
    };
    use codestory_contracts::{
        api::{AgentPacketRequestDto, PacketBudgetModeDto},
        packet_projection_v3::{
            BoundedVecV3, ContextEvidenceRowV3Dto, ContextProjectionKindV3Dto, ContextTargetV3Dto,
            ContinuationStateV3Dto, DiagnosticArtifactKindV3Dto, DiagnosticArtifactV3Dto,
            DiagnosticCategoryV3Dto, DiagnosticCodeTextV3, DiagnosticRowV3Dto,
            DiagnosticsCapabilityV3Dto, EvidenceAvailabilityV3Dto, EvidenceIdentityV3Dto,
            EvidenceKindV3Dto, GapIdentityV3Dto, GapKindV3Dto, IdentityTextV3, MessageTextV3,
            PacketEvidenceRowV3Dto, PacketProjectionV3Dto, PathTextV3, ProjectionGapRowV3Dto,
            PublicationIdentityV3Dto, RetrievalStateDescriptorV3Dto, RetrievalStateV3Dto,
            SearchEvidenceRowV3Dto, SearchProjectionKindV3Dto, SummaryTextV3,
        },
    };

    fn identity(value: &str) -> IdentityTextV3 {
        IdentityTextV3::new(value).expect("bounded identity")
    }

    fn record_fixture(
        question: &str,
    ) -> crate::agent::packet_execution_record_v3::PacketExecutionRecordV3 {
        record_fixture_with(
            question,
            PacketBudgetModeDto::Standard,
            PacketProfileV3::Auto,
            vec![packet_evidence("evidence-1", Some("dispatches once"))],
            Vec::new(),
            None,
            RetrievalStateDescriptorV3Dto {
                state: RetrievalStateV3Dto::Full,
                generation_id: Some(identity("retrieval-generation-1")),
            },
            Vec::new(),
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_fixture_with(
        question: &str,
        budget: PacketBudgetModeDto,
        profile: PacketProfileV3,
        evidence: Vec<PacketEvidenceRowV3Dto>,
        gaps: Vec<ProjectionGapRowV3Dto>,
        continuation: Option<ContinuationStateV3Dto>,
        retrieval: RetrievalStateDescriptorV3Dto,
        diagnostics: Vec<FinalizedDiagnosticSourceRowV3>,
        with_retrieval_publication: bool,
    ) -> crate::agent::packet_execution_record_v3::PacketExecutionRecordV3 {
        let request = AgentPacketRequestDto {
            question: question.to_owned(),
            budget,
            probes: Vec::new(),
            extra_probes: Vec::new(),
            latency_budget_ms: None,
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        };
        let input = FinalizedPacketExecutionInputV3::new(
            identity("caller-1"),
            identity("request-1"),
            PacketRequestFingerprintV3::from_current_request(&request, profile),
            evidence,
            gaps,
            continuation,
            retrieval,
            diagnostics,
        );
        build_packet_execution_record_fixture_v3(&input, with_retrieval_publication)
            .expect("valid record fixture")
    }

    fn packet_evidence(id: &str, summary: Option<&str>) -> PacketEvidenceRowV3Dto {
        PacketEvidenceRowV3Dto {
            identity: EvidenceIdentityV3Dto {
                evidence_id: identity(id),
            },
            kind: EvidenceKindV3Dto::ExactSource,
            path: None,
            symbol_id: None,
            start_line: Some(7),
            end_line: Some(7),
            summary: summary.map(|summary| SummaryTextV3::new(summary).unwrap()),
        }
    }

    fn projection_gap(
        id: &str,
        kind: GapKindV3Dto,
        message: Option<&str>,
    ) -> ProjectionGapRowV3Dto {
        ProjectionGapRowV3Dto {
            identity: GapIdentityV3Dto {
                gap_id: identity(id),
            },
            kind,
            message: message.map(|message| MessageTextV3::new(message).unwrap()),
        }
    }

    fn diagnostic_source(
        id: &str,
        evidence_ids: &[&str],
        gap_ids: &[&str],
    ) -> FinalizedDiagnosticSourceRowV3 {
        FinalizedDiagnosticSourceRowV3::new(
            identity(id),
            DiagnosticCategoryV3Dto::Coverage,
            DiagnosticCodeTextV3::new("coverage_gap").unwrap(),
            evidence_ids
                .iter()
                .map(|id| EvidenceIdentityV3Dto {
                    evidence_id: identity(id),
                })
                .collect(),
            gap_ids
                .iter()
                .map(|id| GapIdentityV3Dto {
                    gap_id: identity(id),
                })
                .collect(),
        )
    }

    fn diagnostics_capability_fixture() -> DiagnosticsCapabilityV3Dto {
        DiagnosticsCapabilityV3Dto::Available {
            reference: DiagnosticReferenceV3Dto {
                artifact_id: identity("diagnostic-artifact-1"),
                sha256: Sha256DigestV3Dto::new("d".repeat(64)).unwrap(),
                byte_length: 512,
                wall_expiry_epoch_ms: None,
            },
        }
    }

    fn fixed_length_identity(prefix: &str, index: usize, length: usize) -> String {
        let base = format!("{prefix}-{index:03}-");
        assert!(base.len() <= length);
        format!("{base}{}", "x".repeat(length - base.len()))
    }

    fn diagnostic_cap_record(
        final_code_length: usize,
    ) -> crate::agent::packet_execution_record_v3::PacketExecutionRecordV3 {
        let evidence_ids = (0..256)
            .map(|index| fixed_length_identity("evidence", index, 128))
            .collect::<Vec<_>>();
        let evidence = evidence_ids
            .iter()
            .map(|id| packet_evidence(id, None))
            .collect::<Vec<_>>();
        let all_evidence_references = evidence_ids
            .iter()
            .map(|id| EvidenceIdentityV3Dto {
                evidence_id: identity(id),
            })
            .collect::<Vec<_>>();
        let mut diagnostics = (0..27)
            .map(|index| {
                FinalizedDiagnosticSourceRowV3::new(
                    identity(&fixed_length_identity("diagnostic", index, 32)),
                    DiagnosticCategoryV3Dto::Coverage,
                    DiagnosticCodeTextV3::new("c".repeat(12)).unwrap(),
                    all_evidence_references.clone(),
                    Vec::new(),
                )
            })
            .collect::<Vec<_>>();
        diagnostics.push(FinalizedDiagnosticSourceRowV3::new(
            identity(&fixed_length_identity("diagnostic", 27, 32)),
            DiagnosticCategoryV3Dto::Coverage,
            DiagnosticCodeTextV3::new("c".repeat(final_code_length)).unwrap(),
            all_evidence_references[..193].to_vec(),
            Vec::new(),
        ));

        record_fixture_with(
            "diagnostic cap fixture",
            PacketBudgetModeDto::Standard,
            PacketProfileV3::Auto,
            evidence,
            Vec::new(),
            None,
            RetrievalStateDescriptorV3Dto {
                state: RetrievalStateV3Dto::Full,
                generation_id: Some(identity("retrieval-generation-1")),
            },
            diagnostics,
            true,
        )
    }

    fn packet_identity(
        record: &crate::agent::packet_execution_record_v3::PacketExecutionRecordV3,
    ) -> codestory_contracts::packet_projection_v3::PacketRequestIdentityV3Dto {
        codestory_contracts::packet_projection_v3::PacketRequestIdentityV3Dto {
            packet_id: record.packet_id().clone(),
            request_id: record.request_id().clone(),
            question_sha256: record.question_sha256().clone(),
        }
    }

    fn publication(
        record: &crate::agent::packet_execution_record_v3::PacketExecutionRecordV3,
    ) -> PublicationIdentityV3Dto {
        PublicationIdentityV3Dto {
            core: record.core_publication().clone(),
            retrieval: record.retrieval_publication().cloned(),
        }
    }

    #[test]
    fn packet_projection_v3_builds_from_the_immutable_record_without_raw_question() {
        let question = "escape-heavy \"question\" \\ with 控制\n\t\u{0001}";
        let record = record_fixture(question);
        let before = record.clone();

        let projection =
            build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |_| Ok(1))
                .expect("complete packet projection");

        let PacketProjectionV3Dto::Complete {
            status,
            evidence,
            gaps,
            ..
        } = &projection
        else {
            panic!("one evidence row should fit the complete projection");
        };
        assert_eq!(status, &EvidenceAvailabilityV3Dto::Available);
        assert_eq!(evidence.as_slice().len(), 1);
        assert!(gaps.as_slice().is_empty());
        assert_eq!(record, before, "projection must not mutate the record");
        assert!(
            !serde_json::to_string(&projection)
                .expect("serialize projection")
                .contains(question),
            "raw question must stay out of the packet projection"
        );
    }

    #[test]
    fn packet_projection_v3_availability_is_evidence_only_and_ordered() {
        let full = RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Full,
            generation_id: Some(identity("retrieval-generation-1")),
        };
        let unavailable = RetrievalStateDescriptorV3Dto {
            state: RetrievalStateV3Dto::Unavailable,
            generation_id: None,
        };
        let source_gap = projection_gap("gap-source", GapKindV3Dto::SourceUnavailable, None);

        assert_eq!(
            evidence_availability_v3(true, true, &unavailable, std::slice::from_ref(&source_gap)),
            EvidenceAvailabilityV3Dto::ContinuationAvailable
        );
        assert_eq!(
            evidence_availability_v3(false, true, &unavailable, std::slice::from_ref(&source_gap),),
            EvidenceAvailabilityV3Dto::Available
        );
        assert_eq!(
            evidence_availability_v3(false, false, &unavailable, &[]),
            EvidenceAvailabilityV3Dto::Unavailable
        );
        assert_eq!(
            evidence_availability_v3(false, false, &full, &[source_gap]),
            EvidenceAvailabilityV3Dto::Unavailable
        );
        assert_eq!(
            evidence_availability_v3(false, false, &full, &[]),
            EvidenceAvailabilityV3Dto::NoUsefulEvidence
        );
    }

    #[test]
    fn packet_projection_v3_trims_only_optional_context_and_retains_every_identity() {
        let gaps = vec![
            projection_gap("gap-b", GapKindV3Dto::EvidenceMissing, Some("second gap")),
            projection_gap(
                "gap-a",
                GapKindV3Dto::ContinuationRequired,
                Some("first gap"),
            ),
        ];
        let continuation = ContinuationStateV3Dto {
            continuation_id: identity("continuation-1"),
            remaining_rounds: 1,
            gap_ids: BoundedVecV3::new(vec![GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            }])
            .unwrap(),
        };
        let record = record_fixture_with(
            "trim only display text",
            PacketBudgetModeDto::Compact,
            PacketProfileV3::Callflow,
            vec![
                packet_evidence("evidence-b", Some("second summary")),
                packet_evidence("evidence-a", Some("first summary")),
            ],
            gaps,
            Some(continuation.clone()),
            RetrievalStateDescriptorV3Dto {
                state: RetrievalStateV3Dto::Full,
                generation_id: Some(identity("retrieval-generation-1")),
            },
            Vec::new(),
            true,
        );
        let before = record.clone();
        let mut measurements = 0;
        let diagnostics = diagnostics_capability_fixture();
        let projection = build_packet_projection_v3(&record, diagnostics.clone(), |candidate| {
            measurements += 1;
            let PacketProjectionV3Dto::Complete { evidence, gaps, .. } = candidate else {
                return Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3);
            };
            let detailed = evidence
                .as_slice()
                .iter()
                .filter(|row| row.summary.is_some())
                .count();
            if detailed > 1 {
                Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 2)
            } else if gaps.as_slice().iter().any(|row| row.message.is_some()) {
                Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1)
            } else {
                Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3)
            }
        })
        .expect("optional text stages should fit");

        let PacketProjectionV3Dto::Complete {
            evidence,
            gaps,
            continuation: projected_continuation,
            diagnostics: projected_diagnostics,
            ..
        } = projection
        else {
            panic!("optional-context trimming should retain a complete projection");
        };
        assert_eq!(
            measurements, 4,
            "measure complete, mandatory envelope, and the binary-search probes"
        );
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .map(|row| row.identity.evidence_id.as_str())
                .collect::<Vec<_>>(),
            ["evidence-a", "evidence-b"]
        );
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .map(|row| row.summary.as_ref().map(|value| value.as_str()))
                .collect::<Vec<_>>(),
            [Some("first summary"), None]
        );
        assert_eq!(evidence.as_slice()[0].start_line, Some(7));
        assert_eq!(
            evidence.as_slice()[1].start_line,
            Some(7),
            "compaction removes optional prose without erasing the evidence locator"
        );
        assert_eq!(
            gaps.as_slice()
                .iter()
                .map(|row| row.identity.gap_id.as_str())
                .collect::<Vec<_>>(),
            ["gap-a", "gap-b", "packet-optional-context-omitted"]
        );
        assert!(gaps.as_slice().iter().all(|row| row.message.is_none()));
        assert_eq!(projected_continuation, Some(continuation));
        assert_eq!(projected_diagnostics, diagnostics);
        assert_eq!(
            record, before,
            "budget projection must not mutate its record"
        );
    }

    #[test]
    fn modern_packet_compaction_keeps_the_relevance_prefix_and_all_sixteen_identities() {
        let evidence = (0..16)
            .map(|index| {
                let id = format!("packet-evidence-{index:03}");
                let summary = format!("relevance-ranked flow evidence {index}");
                packet_evidence(&id, Some(&summary))
            })
            .collect::<Vec<_>>();
        let record = record_fixture_with(
            "retain the relevance-ranked prefix",
            PacketBudgetModeDto::Compact,
            PacketProfileV3::Callflow,
            evidence,
            Vec::new(),
            None,
            RetrievalStateDescriptorV3Dto {
                state: RetrievalStateV3Dto::Full,
                generation_id: Some(identity("retrieval-generation-1")),
            },
            Vec::new(),
            true,
        );
        let diagnostics = diagnostics_capability_fixture();
        let four_detail_shape =
            build_packet_projection_v3(&record, diagnostics.clone(), |candidate| match candidate {
                PacketProjectionV3Dto::Complete { evidence, .. }
                    if evidence
                        .as_slice()
                        .iter()
                        .filter(|row| row.summary.is_some())
                        .count()
                        > 4 =>
                {
                    Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1)
                }
                PacketProjectionV3Dto::Complete { .. } => Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3),
                PacketProjectionV3Dto::BudgetExceeded { .. } => {
                    Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3)
                }
            })
            .expect("four-detail fixture");
        let fixed_envelope_bytes = PACKET_PUBLIC_RESULT_MAX_BYTES_V3
            - planned_transport_size(PlannedTransportShape::June2025, &four_detail_shape);

        let projection = build_packet_projection_v3(&record, diagnostics, |candidate| {
            Ok(
                planned_transport_size(PlannedTransportShape::June2025, candidate)
                    + fixed_envelope_bytes,
            )
        })
        .expect("modern mirrored projection should compact to a complete result");
        let PacketProjectionV3Dto::Complete { evidence, .. } = projection else {
            panic!("the sixteen-identity envelope should fit after optional compaction");
        };

        assert_eq!(evidence.as_slice().len(), 16);
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .filter(|row| row.summary.is_some())
                .count(),
            4
        );
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .take(4)
                .map(|row| row.identity.evidence_id.as_str())
                .collect::<Vec<_>>(),
            [
                "packet-evidence-000",
                "packet-evidence-001",
                "packet-evidence-002",
                "packet-evidence-003",
            ]
        );
        assert!(
            evidence
                .as_slice()
                .iter()
                .skip(4)
                .all(|row| row.summary.is_none())
        );
    }

    #[test]
    fn modern_packet_compaction_retains_every_multifile_source_locator() {
        let evidence = (0..16)
            .map(|index| {
                let id = format!("packet-evidence-{index:03}");
                let mut row = packet_evidence(
                    &id,
                    Some(&format!(
                        "source-backed stage {index}: {}",
                        "implementation evidence ".repeat(24)
                    )),
                );
                row.path = Some(
                    PathTextV3::new(format!("lib/src/stage_{index}.dart"))
                        .expect("bounded source path"),
                );
                row.symbol_id = Some(
                    codestory_contracts::packet_projection_v3::SymbolIdTextV3::new(format!(
                        "Stage{index}.send"
                    ))
                    .expect("bounded symbol identity"),
                );
                row.start_line = Some(index + 1);
                row.end_line = Some(index + 4);
                row
            })
            .collect::<Vec<_>>();
        let record = record_fixture_with(
            "explain the complete multi-file request flow",
            PacketBudgetModeDto::Compact,
            PacketProfileV3::Callflow,
            evidence,
            Vec::new(),
            None,
            RetrievalStateDescriptorV3Dto {
                state: RetrievalStateV3Dto::Full,
                generation_id: Some(identity("retrieval-generation-1")),
            },
            Vec::new(),
            true,
        );
        let diagnostics = diagnostics_capability_fixture();
        let four_summary_shape =
            build_packet_projection_v3(&record, diagnostics.clone(), |candidate| match candidate {
                PacketProjectionV3Dto::Complete { evidence, .. }
                    if evidence
                        .as_slice()
                        .iter()
                        .filter(|row| row.summary.is_some())
                        .count()
                        > 4 =>
                {
                    Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1)
                }
                PacketProjectionV3Dto::Complete { .. } => Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3),
                PacketProjectionV3Dto::BudgetExceeded { .. } => {
                    Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3)
                }
            })
            .expect("four-summary packet shape");
        let fixed_envelope_bytes = PACKET_PUBLIC_RESULT_MAX_BYTES_V3
            - planned_transport_size(PlannedTransportShape::June2025, &four_summary_shape);

        let projection = build_packet_projection_v3(&record, diagnostics, |candidate| {
            Ok(
                planned_transport_size(PlannedTransportShape::June2025, candidate)
                    + fixed_envelope_bytes,
            )
        })
        .expect("the multi-file locator envelope should fit at sixteen KiB");
        let PacketProjectionV3Dto::Complete { evidence, gaps, .. } = projection else {
            panic!("the bounded multi-file packet should remain complete");
        };

        assert_eq!(evidence.as_slice().len(), 16);
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .filter(|row| row.summary.is_some())
                .count(),
            4
        );
        assert!(
            evidence.as_slice().iter().all(|row| {
                row.path.is_some()
                    && row.symbol_id.is_some()
                    && row.start_line.is_some()
                    && row.end_line.is_some()
            }),
            "compaction must not turn preserved evidence identities into contentless rows"
        );
        assert_eq!(
            evidence.as_slice()[15]
                .path
                .as_ref()
                .map(|path| path.as_str()),
            Some("lib/src/stage_15.dart"),
            "the only locator for a later upstream file must survive summary trimming"
        );
        assert!(gaps.as_slice().iter().any(|gap| {
            gap.identity.gap_id.as_str() == "packet-optional-context-omitted"
                && gap.kind == GapKindV3Dto::OutputBudgetExceeded
        }));
    }

    #[derive(Clone, Copy)]
    enum PlannedTransportShape {
        November2024,
        March2025,
        June2025,
        November2025,
    }

    fn planned_transport_size(
        shape: PlannedTransportShape,
        projection: &PacketProjectionV3Dto,
    ) -> usize {
        let text = serde_json::to_string(projection).expect("compact mirrored projection");
        let result = match shape {
            PlannedTransportShape::November2024 | PlannedTransportShape::March2025 => {
                serde_json::json!({"content": [{"type": "text", "text": text}]})
            }
            PlannedTransportShape::June2025 | PlannedTransportShape::November2025 => {
                serde_json::json!({
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": projection
                })
            }
        };
        serde_json::to_vec(&result)
            .expect("planned transport fixture")
            .len()
    }

    #[test]
    fn packet_projection_v3_accepts_exactly_16_kib_for_all_planned_transport_shapes() {
        let record = record_fixture("all four planned transport shapes");
        for shape in [
            PlannedTransportShape::November2024,
            PlannedTransportShape::March2025,
            PlannedTransportShape::June2025,
            PlannedTransportShape::November2025,
        ] {
            let probe = build_packet_projection_v3(
                &record,
                DiagnosticsCapabilityV3Dto::Unavailable,
                |_| Ok(0),
            )
            .unwrap();
            let shape_bytes = planned_transport_size(shape, &probe);
            assert!(shape_bytes < PACKET_PUBLIC_RESULT_MAX_BYTES_V3);
            let fixed_envelope_bytes = PACKET_PUBLIC_RESULT_MAX_BYTES_V3 - shape_bytes;

            let exact = build_packet_projection_v3(
                &record,
                DiagnosticsCapabilityV3Dto::Unavailable,
                |candidate| Ok(planned_transport_size(shape, candidate) + fixed_envelope_bytes),
            )
            .expect("exact cap is admitted");
            assert!(matches!(exact, PacketProjectionV3Dto::Complete { .. }));
        }
    }

    #[test]
    fn packet_projection_v3_cap_plus_one_is_a_whole_budget_fallback_for_every_mode() {
        for budget in [
            PacketBudgetModeDto::Tiny,
            PacketBudgetModeDto::Compact,
            PacketBudgetModeDto::Standard,
            PacketBudgetModeDto::Deep,
        ] {
            for profile in [
                PacketProfileV3::Auto,
                PacketProfileV3::Architecture,
                PacketProfileV3::Callflow,
                PacketProfileV3::Impact,
                PacketProfileV3::Inheritance,
                PacketProfileV3::Investigate,
            ] {
                let record = record_fixture_with(
                    "one cap for every current request mode",
                    budget,
                    profile,
                    vec![packet_evidence("evidence-1", None)],
                    Vec::new(),
                    None,
                    RetrievalStateDescriptorV3Dto {
                        state: RetrievalStateV3Dto::Full,
                        generation_id: Some(identity("retrieval-generation-1")),
                    },
                    Vec::new(),
                    true,
                );
                let projection = build_packet_projection_v3(
                    &record,
                    diagnostics_capability_fixture(),
                    |candidate| match candidate {
                        PacketProjectionV3Dto::Complete { .. } => {
                            Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1)
                        }
                        PacketProjectionV3Dto::BudgetExceeded { .. } => {
                            Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3)
                        }
                    },
                )
                .expect("whole budget fallback fits");
                let PacketProjectionV3Dto::BudgetExceeded {
                    status,
                    diagnostics,
                    maximum_bytes,
                    required_complete_bytes,
                    ..
                } = projection
                else {
                    panic!("cap plus one must discard the complete projection");
                };
                assert_eq!(status, EvidenceAvailabilityV3Dto::Unavailable);
                assert_eq!(diagnostics, diagnostics_capability_fixture());
                assert_eq!(maximum_bytes, PACKET_PUBLIC_RESULT_MAX_BYTES_V3 as u64);
                assert_eq!(
                    required_complete_bytes,
                    (PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1) as u64
                );
                let serialized = serde_json::to_value(PacketProjectionV3Dto::BudgetExceeded {
                    schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
                    identity: packet_identity(&record),
                    publication: publication(&record),
                    status,
                    retrieval: record.retrieval().clone(),
                    diagnostics,
                    gaps: packet_budget_exceeded_gaps_v3(),
                    maximum_bytes,
                    required_complete_bytes,
                    answer_sufficiency: Default::default(),
                })
                .unwrap();
                let gaps = serialized["gaps"].as_array().expect("typed budget gap");
                assert_eq!(gaps.len(), 1, "fallback must carry exactly one gap");
                assert_eq!(gaps[0]["kind"], "output_budget_exceeded");
                assert_eq!(
                    gaps[0]["identity"]["gap_id"],
                    "packet-output-budget-exceeded"
                );
                for absent in ["evidence", "continuation", "summary"] {
                    assert!(serialized.get(absent).is_none(), "fallback leaked {absent}");
                }
            }
        }
    }

    #[test]
    fn packet_projection_v3_measurement_failures_and_oversized_fallback_return_no_dto() {
        let record = record_fixture("measurement failures");
        assert_eq!(
            build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |_| Err(
                ()
            )),
            Err(ProjectionBuildErrorV3::MeasurementFailed)
        );

        assert_eq!(
            build_packet_projection_v3(
                &record,
                DiagnosticsCapabilityV3Dto::Unavailable,
                |candidate| match candidate {
                    PacketProjectionV3Dto::Complete { .. } => {
                        Ok(PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1)
                    }
                    PacketProjectionV3Dto::BudgetExceeded { .. } => Err(()),
                }
            ),
            Err(ProjectionBuildErrorV3::MeasurementFailed)
        );

        assert_eq!(
            build_packet_projection_v3(&record, DiagnosticsCapabilityV3Dto::Unavailable, |_| Ok(
                PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1
            )),
            Err(ProjectionBuildErrorV3::FallbackTooLarge {
                required_bytes: PACKET_PUBLIC_RESULT_MAX_BYTES_V3 + 1
            })
        );
    }

    #[test]
    fn packet_projection_v3_exposes_separate_context_and_search_builders() {
        let record = record_fixture("typed target only");
        let context_input = FinalizedContextProjectionInputV3::new(
            packet_identity(&record),
            publication(&record),
            record.retrieval().clone(),
            ContextTargetV3Dto {
                path: Some(PathTextV3::new("src/lib.rs").unwrap()),
                symbol_id: None,
            },
            vec![ContextEvidenceRowV3Dto {
                identity: EvidenceIdentityV3Dto {
                    evidence_id: identity("evidence-context"),
                },
                path: PathTextV3::new("src/lib.rs").unwrap(),
                symbol_id: None,
                start_line: Some(3),
                end_line: Some(5),
                excerpt: None,
            }],
            Vec::new(),
            None,
            DiagnosticsCapabilityV3Dto::Unavailable,
            Vec::new(),
        );
        let context = build_context_projection_v3(&context_input).expect("context projection");
        assert_eq!(context.kind, ContextProjectionKindV3Dto::Complete);
        assert_eq!(context.status, EvidenceAvailabilityV3Dto::Available);

        let search_input = FinalizedSearchProjectionInputV3::new(
            packet_identity(&record),
            publication(&record),
            record.retrieval().clone(),
            vec![SearchEvidenceRowV3Dto {
                identity: EvidenceIdentityV3Dto {
                    evidence_id: identity("evidence-search"),
                },
                path: PathTextV3::new("src/search.rs").unwrap(),
                symbol_id: None,
                start_line: Some(8),
                end_line: Some(9),
                excerpt: None,
            }],
            Vec::new(),
            None,
            DiagnosticsCapabilityV3Dto::Unavailable,
            Vec::new(),
        );
        let search = build_search_projection_v3(&search_input).expect("search projection");
        assert_eq!(search.kind, SearchProjectionKindV3Dto::Complete);
        assert_eq!(search.status, EvidenceAvailabilityV3Dto::Available);

        let context_keys = serde_json::to_value(&context).unwrap();
        let search_keys = serde_json::to_value(&search).unwrap();
        for forbidden in ["supported", "proof_disposition", "eligible_for_sufficiency"] {
            assert!(!context_keys.to_string().contains(forbidden));
            assert!(!search_keys.to_string().contains(forbidden));
        }
    }

    fn context_input_fixture(
        record: &crate::agent::packet_execution_record_v3::PacketExecutionRecordV3,
    ) -> FinalizedContextProjectionInputV3 {
        FinalizedContextProjectionInputV3::new(
            packet_identity(record),
            publication(record),
            record.retrieval().clone(),
            ContextTargetV3Dto {
                path: Some(PathTextV3::new("src/lib.rs").unwrap()),
                symbol_id: None,
            },
            vec![
                ContextEvidenceRowV3Dto {
                    identity: EvidenceIdentityV3Dto {
                        evidence_id: identity("evidence-b"),
                    },
                    path: PathTextV3::new("src/b.rs").unwrap(),
                    symbol_id: None,
                    start_line: Some(7),
                    end_line: Some(8),
                    excerpt: None,
                },
                ContextEvidenceRowV3Dto {
                    identity: EvidenceIdentityV3Dto {
                        evidence_id: identity("evidence-a"),
                    },
                    path: PathTextV3::new("src/a.rs").unwrap(),
                    symbol_id: None,
                    start_line: Some(3),
                    end_line: Some(4),
                    excerpt: None,
                },
            ],
            vec![
                projection_gap("gap-b", GapKindV3Dto::EvidenceMissing, None),
                projection_gap("gap-a", GapKindV3Dto::ContinuationRequired, None),
            ],
            Some(ContinuationStateV3Dto {
                continuation_id: identity("continuation-1"),
                remaining_rounds: 1,
                gap_ids: BoundedVecV3::new(vec![GapIdentityV3Dto {
                    gap_id: identity("gap-b"),
                }])
                .unwrap(),
            }),
            DiagnosticsCapabilityV3Dto::Unavailable,
            vec![DiagnosticRowV3Dto {
                diagnostic_id: identity("diagnostic-context"),
                category: DiagnosticCategoryV3Dto::Coverage,
                code: DiagnosticCodeTextV3::new("coverage_gap").unwrap(),
                evidence_ids: BoundedVecV3::new(vec![EvidenceIdentityV3Dto {
                    evidence_id: identity("evidence-a"),
                }])
                .unwrap(),
                gap_ids: BoundedVecV3::new(vec![GapIdentityV3Dto {
                    gap_id: identity("gap-a"),
                }])
                .unwrap(),
            }],
        )
    }

    #[test]
    fn packet_projection_v3_context_and_search_canonicalize_and_reject_dangling_references() {
        let record = record_fixture("finalized typed context and search");
        let context_input = context_input_fixture(&record);
        let context = build_context_projection_v3(&context_input).expect("canonical context");
        assert_eq!(
            context
                .evidence
                .as_slice()
                .iter()
                .map(|row| row.identity.evidence_id.as_str())
                .collect::<Vec<_>>(),
            ["evidence-a", "evidence-b"]
        );
        assert_eq!(
            context
                .gaps
                .as_slice()
                .iter()
                .map(|row| row.identity.gap_id.as_str())
                .collect::<Vec<_>>(),
            ["gap-a", "gap-b"]
        );

        let mut duplicate_evidence = context_input.clone();
        duplicate_evidence.evidence[1].identity = duplicate_evidence.evidence[0].identity.clone();
        assert!(matches!(
            build_context_projection_v3(&duplicate_evidence),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateEvidenceIdentity(_)
            ))
        ));

        let mut empty_target = context_input.clone();
        empty_target.target = ContextTargetV3Dto {
            path: None,
            symbol_id: None,
        };
        assert_eq!(
            build_context_projection_v3(&empty_target),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::EmptyContextTarget
            ))
        );

        let mut duplicate_gap = context_input.clone();
        duplicate_gap.gaps[1].identity = duplicate_gap.gaps[0].identity.clone();
        assert!(matches!(
            build_context_projection_v3(&duplicate_gap),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateGapIdentity(_)
            ))
        ));

        let mut dangling_continuation = context_input.clone();
        dangling_continuation.continuation.as_mut().unwrap().gap_ids =
            BoundedVecV3::new(vec![GapIdentityV3Dto {
                gap_id: identity("gap-missing"),
            }])
            .unwrap();
        assert!(matches!(
            build_context_projection_v3(&dangling_continuation),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::UnknownContinuationGap(_)
            ))
        ));

        let mut zero_round_continuation = context_input.clone();
        zero_round_continuation
            .continuation
            .as_mut()
            .unwrap()
            .remaining_rounds = 0;
        assert_eq!(
            build_context_projection_v3(&zero_round_continuation),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::ZeroContinuationRounds
            ))
        );

        let mut dangling_diagnostic = context_input.clone();
        dangling_diagnostic.diagnostic_rows[0].evidence_ids =
            BoundedVecV3::new(vec![EvidenceIdentityV3Dto {
                evidence_id: identity("evidence-missing"),
            }])
            .unwrap();
        assert!(matches!(
            build_context_projection_v3(&dangling_diagnostic),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::UnknownDiagnosticEvidence(_)
            ))
        ));

        let mut duplicate_diagnostic = context_input.clone();
        duplicate_diagnostic
            .diagnostic_rows
            .push(duplicate_diagnostic.diagnostic_rows[0].clone());
        assert!(matches!(
            build_context_projection_v3(&duplicate_diagnostic),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateDiagnosticIdentity(_)
            ))
        ));

        let search_input = FinalizedSearchProjectionInputV3::new(
            packet_identity(&record),
            publication(&record),
            record.retrieval().clone(),
            vec![
                SearchEvidenceRowV3Dto {
                    identity: EvidenceIdentityV3Dto {
                        evidence_id: identity("evidence-b"),
                    },
                    path: PathTextV3::new("src/b.rs").unwrap(),
                    symbol_id: None,
                    start_line: Some(7),
                    end_line: Some(8),
                    excerpt: None,
                },
                SearchEvidenceRowV3Dto {
                    identity: EvidenceIdentityV3Dto {
                        evidence_id: identity("evidence-a"),
                    },
                    path: PathTextV3::new("src/a.rs").unwrap(),
                    symbol_id: None,
                    start_line: Some(3),
                    end_line: Some(4),
                    excerpt: None,
                },
            ],
            context_input.gaps.clone(),
            context_input.continuation.clone(),
            DiagnosticsCapabilityV3Dto::Unavailable,
            context_input.diagnostic_rows.clone(),
        );
        let search = build_search_projection_v3(&search_input).expect("canonical search");
        assert_eq!(
            search
                .evidence
                .as_slice()
                .iter()
                .map(|row| row.identity.evidence_id.as_str())
                .collect::<Vec<_>>(),
            ["evidence-a", "evidence-b"]
        );

        let mut zero_round_continuation = search_input.clone();
        zero_round_continuation
            .continuation
            .as_mut()
            .unwrap()
            .remaining_rounds = 0;
        assert_eq!(
            build_search_projection_v3(&zero_round_continuation),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::ZeroContinuationRounds
            ))
        );

        let mut duplicate_diagnostic_reference = search_input;
        duplicate_diagnostic_reference.diagnostic_rows[0].gap_ids = BoundedVecV3::new(vec![
            GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            },
            GapIdentityV3Dto {
                gap_id: identity("gap-a"),
            },
        ])
        .unwrap();
        assert!(matches!(
            build_search_projection_v3(&duplicate_diagnostic_reference),
            Err(ProjectionBuildErrorV3::InvalidInput(
                ProjectionInputErrorV3::DuplicateDiagnosticGapReference(_)
            ))
        ));
    }

    #[test]
    fn packet_projection_v3_materializes_one_exact_diagnostic_artifact() {
        let record = record_fixture("diagnostic identity");

        let built = build_diagnostic_artifact_v3(&record).expect("diagnostic artifact build");
        let DiagnosticArtifactBuildV3::Complete {
            artifact,
            bytes,
            reference,
        } = built
        else {
            panic!("small diagnostic artifact should fit");
        };
        assert_eq!(artifact.kind, DiagnosticArtifactKindV3Dto::Complete);
        assert_eq!(reference.byte_length, bytes.len() as u64);
        assert_eq!(
            reference.sha256.as_str(),
            format!("{:x}", sha2::Sha256::digest(bytes.as_slice()))
        );
        assert_eq!(
            serde_json::from_slice::<DiagnosticArtifactV3Dto>(bytes.as_slice()).unwrap(),
            *artifact
        );
    }

    #[test]
    fn packet_projection_v3_diagnostic_rows_retain_typed_identity_and_are_repeatable() {
        let record = record_fixture_with(
            "canonical diagnostics",
            PacketBudgetModeDto::Standard,
            PacketProfileV3::Auto,
            vec![
                packet_evidence("evidence-b", None),
                packet_evidence("evidence-a", None),
            ],
            vec![
                projection_gap("gap-b", GapKindV3Dto::EvidenceMissing, None),
                projection_gap("gap-a", GapKindV3Dto::EvidenceMissing, None),
            ],
            None,
            RetrievalStateDescriptorV3Dto {
                state: RetrievalStateV3Dto::Full,
                generation_id: Some(identity("retrieval-generation-1")),
            },
            vec![
                diagnostic_source("diagnostic-b", &["evidence-b"], &["gap-b"]),
                diagnostic_source(
                    "diagnostic-a",
                    &["evidence-b", "evidence-a"],
                    &["gap-b", "gap-a"],
                ),
            ],
            true,
        );
        let before = record.clone();
        let first = build_diagnostic_artifact_v3(&record).unwrap();
        let second = build_diagnostic_artifact_v3(&record).unwrap();
        assert_eq!(
            first, second,
            "one frozen record must produce exact repeatable bytes"
        );
        assert_eq!(
            record, before,
            "diagnostic projection must not mutate the record"
        );

        let DiagnosticArtifactBuildV3::Complete {
            artifact, bytes, ..
        } = first
        else {
            panic!("small diagnostic artifact should fit");
        };
        assert_eq!(
            artifact
                .rows
                .as_slice()
                .iter()
                .map(|row| row.diagnostic_id.as_str())
                .collect::<Vec<_>>(),
            ["diagnostic-a", "diagnostic-b"]
        );
        assert_eq!(
            artifact.rows.as_slice()[0]
                .evidence_ids
                .as_slice()
                .iter()
                .map(|reference| reference.evidence_id.as_str())
                .collect::<Vec<_>>(),
            ["evidence-a", "evidence-b"]
        );
        assert_eq!(
            artifact.rows.as_slice()[0]
                .gap_ids
                .as_slice()
                .iter()
                .map(|reference| reference.gap_id.as_str())
                .collect::<Vec<_>>(),
            ["gap-a", "gap-b"]
        );
        let text = String::from_utf8(bytes.as_slice().to_vec()).unwrap();
        for forbidden in [
            "capability_uri",
            "token",
            "session_secret",
            "hmac",
            "expiry",
            "cache_path",
            "executable_path",
            "source_text",
            "/Users/",
        ] {
            assert!(
                !text.contains(forbidden),
                "diagnostic bytes leaked {forbidden}: {text}"
            );
        }
    }

    #[test]
    fn packet_projection_v3_diagnostic_artifact_is_whole_at_one_mib_and_absent_at_cap_plus_one() {
        let exact = diagnostic_cap_record(31);
        let DiagnosticArtifactBuildV3::Complete {
            artifact,
            bytes,
            reference,
        } = build_diagnostic_artifact_v3(&exact).expect("exact-cap diagnostic build")
        else {
            panic!("exactly one MiB must be admitted");
        };
        assert_eq!(bytes.len(), DIAGNOSTIC_ARTIFACT_MAX_BYTES_V3);
        assert_eq!(
            reference.byte_length,
            DIAGNOSTIC_ARTIFACT_MAX_BYTES_V3 as u64
        );
        assert_eq!(artifact.rows.as_slice().len(), 28);

        let over = diagnostic_cap_record(32);
        assert_eq!(
            build_diagnostic_artifact_v3(&over).expect("typed over-cap result"),
            DiagnosticArtifactBuildV3::TooLarge {
                required_bytes: (DIAGNOSTIC_ARTIFACT_MAX_BYTES_V3 + 1) as u64
            },
            "over-cap diagnostics must return no bytes, reference, or partial DTO"
        );
    }
}
