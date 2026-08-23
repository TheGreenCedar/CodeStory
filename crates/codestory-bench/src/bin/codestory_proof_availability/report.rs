use super::contracts::{
    ActivationDecisionReportV1, ActualProductResultV1, CaseReportV1, CaseValidationFailure,
    CohortPathFileV1, CorpusV1, DECISION_REPORT_SCHEMA, EnvironmentReportV1, FailureFunnelReportV1,
    FunnelOutcomeV1, InventoryReportV1, ProductDispositionKindV1, ProductRefutationBasisV1,
    ProjectedReceiptReferenceV1, ProvenanceV1, QualificationSummaryV1, REPORT_SCHEMA,
    ReceiptOracleComparisonV1, RoleThresholdsV1, StepQualificationOutcomeV1, ThresholdsV1,
    TrailReportV1, TransportErrorV1, TransportEvidenceV1, canonical_corpus_sha256,
    canonical_observations_sha256, canonical_thresholds_sha256, results_evidence_sha256,
};
use super::thresholds::{derive_observations, evaluate_activation_decision};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const PUBLIC_ARTIFACT_NAMES: [&str; 8] = [
    "cases.json",
    "decision.json",
    "environment.json",
    "failure-funnel.json",
    "findings.md",
    "inventory.json",
    "summary.json",
    "trails.json",
];
const OWNER_MARKER: &str = ".codestory-proof-availability-report-staging";
const MAX_PUBLIC_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const CASE_DIAGNOSTIC_SCHEMA: &str = "codestory.proof-availability.invalid-case-diagnostic.v1";
const PUBLIC_ARTIFACT_BUILD_DIAGNOSTIC_SCHEMA: &str =
    "codestory.proof-availability.public-artifact-build-diagnostic.v1";
const MAX_CASE_DIAGNOSTIC_BYTES: usize = 1024 * 1024;
const MAX_DIAGNOSTIC_POINTER_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivateDiagnosticFileKind {
    InvalidCaseV1,
    PublicArtifactBuildV1,
}

impl PrivateDiagnosticFileKind {
    const fn file_name(self) -> &'static str {
        match self {
            Self::InvalidCaseV1 => "invalid-case-v1.json",
            Self::PublicArtifactBuildV1 => "public-artifact-build-v1.json",
        }
    }

    const fn temporary_prefix(self) -> &'static str {
        match self {
            Self::InvalidCaseV1 => ".invalid-case-v1",
            Self::PublicArtifactBuildV1 => ".public-artifact-build-v1",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicArtifactName {
    Environment,
    Inventory,
    Trails,
    Cases,
    FailureFunnel,
    Summary,
    Decision,
    Findings,
}

impl PublicArtifactName {
    const fn file_name(self) -> &'static str {
        match self {
            Self::Environment => "environment.json",
            Self::Inventory => "inventory.json",
            Self::Trails => "trails.json",
            Self::Cases => "cases.json",
            Self::FailureFunnel => "failure-funnel.json",
            Self::Summary => "summary.json",
            Self::Decision => "decision.json",
            Self::Findings => "findings.md",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicArtifactBuildStage {
    SummaryValidation,
    ObservationDerivation,
    ObservationCanonicalization,
    DecisionEvaluation,
    DecisionValidation,
    FindingsRendering,
    Serialization,
    BundleValidation,
}

impl PublicArtifactBuildStage {
    const fn name(self) -> &'static str {
        match self {
            Self::SummaryValidation => "summary_validation",
            Self::ObservationDerivation => "observation_derivation",
            Self::ObservationCanonicalization => "observation_canonicalization",
            Self::DecisionEvaluation => "decision_evaluation",
            Self::DecisionValidation => "decision_validation",
            Self::FindingsRendering => "findings_rendering",
            Self::Serialization => "serialization",
            Self::BundleValidation => "bundle_validation",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicArtifactBuildReason {
    PathLeak,
    SecretField,
    ControlCharacter,
    FindingsValidation,
    ObjectFieldNotInFixedVocabulary,
    ObjectPointerBudgetExceeded,
    ArrayPointerBudgetExceeded,
    ExistingInvariant,
    Canonicalization,
    Serialization,
}

impl PublicArtifactBuildReason {
    const fn name(self) -> &'static str {
        match self {
            Self::PathLeak => "path_leak",
            Self::SecretField => "secret_field",
            Self::ControlCharacter => "control_character",
            Self::FindingsValidation => "findings_validation",
            Self::ObjectFieldNotInFixedVocabulary => "object_field_not_in_fixed_vocabulary",
            Self::ObjectPointerBudgetExceeded => "object_pointer_budget_exceeded",
            Self::ArrayPointerBudgetExceeded => "array_pointer_budget_exceeded",
            Self::ExistingInvariant => "existing_invariant",
            Self::Canonicalization => "canonicalization",
            Self::Serialization => "serialization",
        }
    }
}

pub(crate) struct PublicArtifactBuildFailure {
    stage: PublicArtifactBuildStage,
    reason: PublicArtifactBuildReason,
    artifact: Option<PublicArtifactName>,
    case_ordinal: Option<usize>,
    evidence: PublicArtifactBuildEvidence,
}

enum PublicArtifactBuildEvidence {
    None,
    RejectedString {
        json_pointer: Option<String>,
        utf8_byte_length: usize,
        sha256: String,
    },
    ObjectField {
        parent_json_pointer: String,
        utf8_byte_length: usize,
        sha256: String,
    },
    PointerBudget {
        parent_json_pointer: String,
    },
}

impl PublicArtifactBuildFailure {
    fn closed(stage: PublicArtifactBuildStage, reason: PublicArtifactBuildReason) -> Self {
        Self {
            stage,
            reason,
            artifact: None,
            case_ordinal: None,
            evidence: PublicArtifactBuildEvidence::None,
        }
    }

    fn rejected_string(
        stage: PublicArtifactBuildStage,
        reason: PublicArtifactBuildReason,
        artifact: PublicArtifactName,
        case_ordinal: Option<usize>,
        json_pointer: String,
        rejected: &str,
    ) -> Self {
        Self {
            stage,
            reason,
            artifact: Some(artifact),
            case_ordinal,
            evidence: PublicArtifactBuildEvidence::RejectedString {
                json_pointer: Some(json_pointer),
                utf8_byte_length: rejected.len(),
                sha256: domain_sha256(
                    b"codestory.proof-availability.public-artifact-build-rejected-string.v1\0",
                    rejected.as_bytes(),
                ),
            },
        }
    }

    fn object_field_not_in_fixed_vocabulary(
        artifact: PublicArtifactName,
        case_ordinal: Option<usize>,
        parent_json_pointer: &str,
        field: &str,
    ) -> Self {
        debug_assert!(parent_json_pointer.len() <= MAX_DIAGNOSTIC_POINTER_BYTES);
        Self {
            stage: PublicArtifactBuildStage::BundleValidation,
            reason: PublicArtifactBuildReason::ObjectFieldNotInFixedVocabulary,
            artifact: Some(artifact),
            case_ordinal,
            evidence: PublicArtifactBuildEvidence::ObjectField {
                parent_json_pointer: parent_json_pointer.to_owned(),
                utf8_byte_length: field.len(),
                sha256: domain_sha256(
                    b"codestory.proof-availability.public-artifact-build-object-field-name.v1\0",
                    field.as_bytes(),
                ),
            },
        }
    }

    fn pointer_budget_exceeded(
        reason: PublicArtifactBuildReason,
        artifact: PublicArtifactName,
        case_ordinal: Option<usize>,
        parent_json_pointer: &str,
    ) -> Self {
        debug_assert!(matches!(
            reason,
            PublicArtifactBuildReason::ObjectPointerBudgetExceeded
                | PublicArtifactBuildReason::ArrayPointerBudgetExceeded
        ));
        debug_assert!(parent_json_pointer.len() <= MAX_DIAGNOSTIC_POINTER_BYTES);
        Self {
            stage: PublicArtifactBuildStage::BundleValidation,
            reason,
            artifact: Some(artifact),
            case_ordinal,
            evidence: PublicArtifactBuildEvidence::PointerBudget {
                parent_json_pointer: parent_json_pointer.to_owned(),
            },
        }
    }

    fn rejected_without_pointer(
        stage: PublicArtifactBuildStage,
        reason: PublicArtifactBuildReason,
        artifact: PublicArtifactName,
        case_ordinal: Option<usize>,
        rejected: &str,
    ) -> Self {
        Self {
            stage,
            reason,
            artifact: Some(artifact),
            case_ordinal,
            evidence: PublicArtifactBuildEvidence::RejectedString {
                json_pointer: None,
                utf8_byte_length: rejected.len(),
                sha256: domain_sha256(
                    b"codestory.proof-availability.public-artifact-build-rejected-string.v1\0",
                    rejected.as_bytes(),
                ),
            },
        }
    }

    #[cfg(test)]
    fn json_pointer(&self) -> Option<&str> {
        match &self.evidence {
            PublicArtifactBuildEvidence::RejectedString { json_pointer, .. } => {
                json_pointer.as_deref()
            }
            PublicArtifactBuildEvidence::None
            | PublicArtifactBuildEvidence::ObjectField { .. }
            | PublicArtifactBuildEvidence::PointerBudget { .. } => None,
        }
    }

    #[cfg(test)]
    fn path_leak(
        stage: PublicArtifactBuildStage,
        artifact: PublicArtifactName,
        case_ordinal: Option<usize>,
        json_pointer: &str,
        rejected: &str,
    ) -> Self {
        Self::rejected_string(
            stage,
            PublicArtifactBuildReason::PathLeak,
            artifact,
            case_ordinal,
            json_pointer.to_owned(),
            rejected,
        )
    }
}

impl fmt::Display for PublicArtifactBuildFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("proof_availability_public_artifact_build_failed")
    }
}

impl fmt::Debug for PublicArtifactBuildFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for PublicArtifactBuildFailure {}

/// A process-owned, initially empty directory that makes a qualification ID
/// single-use for the private invalid-case recovery artifact. It is never a
/// public result artifact and it is intentionally left behind after a crash.
/// A process-owned single-use directory. The held directory handle, rather
/// than the discoverable pathname, is the only authority permitted to create
/// or publish the private diagnostic file.
#[derive(Debug)]
pub(crate) struct CaseDiagnosticReservation {
    path: PathBuf,
    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    directory: File,
    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    device: u64,
    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    inode: u64,
}

impl CaseDiagnosticReservation {
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
pub(crate) fn reserve_case_diagnostic(
    output_parent: &Path,
    qualification_id: &str,
) -> Result<CaseDiagnosticReservation> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;

    let parent = open_directory_nofollow(output_parent)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_parent_invalid"))?;
    if !parent
        .metadata()
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        bail!("proof_availability_case_diagnostic_parent_invalid")
    }
    let fixed_name = format!(".codestory-proof-availability-case-diagnostic-{qualification_id}");
    let path = output_parent.join(&fixed_name);
    let fixed_component = CString::new(fixed_name.as_bytes())
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_create_failed"))?;
    let staging_name = random_staging_component()
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_create_failed"))?;
    let staging_component = CString::new(staging_name.as_bytes())
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_create_failed"))?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), staging_component.as_ptr(), 0o700) } != 0 {
        return Err(anyhow::anyhow!(
            "proof_availability_case_diagnostic_create_failed"
        ));
    }
    let directory = open_directory_at_nofollow(&parent, &staging_component)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_create_failed"))?;
    let metadata = directory
        .metadata()
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_create_failed"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("proof_availability_case_diagnostic_create_failed")
    }
    let reservation = CaseDiagnosticReservation {
        path,
        directory,
        device: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.dev()
        },
        inode: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino()
        },
    };
    if rename_noreplace_at(
        parent.as_raw_fd(),
        &staging_component,
        parent.as_raw_fd(),
        &fixed_component,
    )
    .is_err()
    {
        return Err(anyhow::anyhow!("proof_availability_case_diagnostic_exists"));
    }
    if !path_matches_held_directory(&reservation).unwrap_or(false) {
        bail!("proof_availability_case_diagnostic_create_failed")
    }
    Ok(reservation)
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
fn random_staging_component() -> std::io::Result<String> {
    use std::io::Read as _;

    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing into String cannot fail");
    }
    Ok(format!(
        ".codestory-proof-availability-case-staging-{encoded}"
    ))
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
fn open_directory_nofollow(path: &Path) -> std::io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("path contains NUL"))?;
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
fn open_directory_at_nofollow(
    parent: &File,
    component: &std::ffi::CString,
) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[cfg(not(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
)))]
pub(crate) fn reserve_case_diagnostic(_: &Path, _: &str) -> Result<CaseDiagnosticReservation> {
    bail!("proof_availability_case_diagnostic_unsupported")
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
pub(crate) fn write_invalid_case_diagnostic(
    reservation: &CaseDiagnosticReservation,
    qualification_id: &str,
    source_commit: &str,
    source_tree: &str,
    failure: &CaseValidationFailure,
    forbidden_values: &[String],
) -> Result<()> {
    // This compares only the recovery path with the held identity. It must
    // never determine the write target: a pathname replacement cannot divert
    // the private artifact away from the directory we opened at reservation.
    ensure_discoverable_reservation(reservation)?;
    let unredacted_case = serde_json::to_value(&failure.case)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))?;
    let project_materialization = serde_json::to_value(&failure.project)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))?;
    let artifact = build_invalid_case_diagnostic_artifact(
        qualification_id,
        source_commit,
        source_tree,
        failure.case_ordinal,
        &failure.case.case_id,
        &failure.case.repository_id,
        unredacted_case,
        project_materialization,
        forbidden_values,
    )?;
    let bytes = canonical_json_file(&artifact)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))?;
    if bytes.len() > MAX_CASE_DIAGNOSTIC_BYTES {
        bail!("proof_availability_case_diagnostic_write_failed")
    }
    write_private_diagnostic_file(
        reservation,
        PrivateDiagnosticFileKind::InvalidCaseV1,
        &bytes,
    )
    .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))?;
    reservation
        .directory
        .sync_all()
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
pub(crate) fn write_public_artifact_build_diagnostic(
    reservation: &CaseDiagnosticReservation,
    qualification_id: &str,
    source_commit: &str,
    source_tree: &str,
    failure: &PublicArtifactBuildFailure,
) -> Result<()> {
    ensure_discoverable_reservation(reservation)?;
    let artifact = build_public_artifact_build_diagnostic_artifact(
        qualification_id,
        source_commit,
        source_tree,
        failure,
    )?;
    let bytes = canonical_json_file(&artifact)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))?;
    if bytes.len() > MAX_CASE_DIAGNOSTIC_BYTES {
        bail!("proof_availability_case_diagnostic_write_failed")
    }
    write_private_diagnostic_file(
        reservation,
        PrivateDiagnosticFileKind::PublicArtifactBuildV1,
        &bytes,
    )
    .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))?;
    reservation
        .directory
        .sync_all()
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))
}

#[cfg(not(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
)))]
pub(crate) fn write_public_artifact_build_diagnostic(
    _: &CaseDiagnosticReservation,
    _: &str,
    _: &str,
    _: &str,
    _: &PublicArtifactBuildFailure,
) -> Result<()> {
    bail!("proof_availability_case_diagnostic_unsupported")
}

fn build_public_artifact_build_diagnostic_artifact(
    qualification_id: &str,
    source_commit: &str,
    source_tree: &str,
    failure: &PublicArtifactBuildFailure,
) -> Result<Value> {
    let (json_pointer, parent_json_pointer, rejected_string, field_name) = match &failure.evidence {
        PublicArtifactBuildEvidence::None => (None, None, None, None),
        PublicArtifactBuildEvidence::RejectedString {
            json_pointer,
            utf8_byte_length,
            sha256,
        } => (
            json_pointer.as_deref(),
            None,
            Some(json!({
                "utf8_byte_length": utf8_byte_length,
                "sha256": sha256,
            })),
            None,
        ),
        PublicArtifactBuildEvidence::ObjectField {
            parent_json_pointer,
            utf8_byte_length,
            sha256,
        } => (
            None,
            Some(parent_json_pointer.as_str()),
            None,
            Some(json!({
                "utf8_byte_length": utf8_byte_length,
                "sha256": sha256,
            })),
        ),
        PublicArtifactBuildEvidence::PointerBudget {
            parent_json_pointer,
        } => (None, Some(parent_json_pointer.as_str()), None, None),
    };
    let artifact = json!({
        "schema": PUBLIC_ARTIFACT_BUILD_DIAGNOSTIC_SCHEMA,
        "classification": "non_evidence",
        "qualification_id": qualification_id,
        "validator_source_commit": source_commit,
        "validator_source_tree": source_tree,
        "failure": {
            "stage": failure.stage.name(),
            "reason": failure.reason.name(),
            "artifact": failure.artifact.map(PublicArtifactName::file_name),
            "case_ordinal": failure.case_ordinal,
            "json_pointer": json_pointer,
            "parent_json_pointer": parent_json_pointer,
            "rejected_string": rejected_string,
            "field_name": field_name,
        },
    });
    validate_private_diagnostic_value(&artifact, &[])?;
    Ok(artifact)
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
fn ensure_discoverable_reservation(reservation: &CaseDiagnosticReservation) -> Result<()> {
    if !matches!(path_matches_held_directory(reservation), Ok(true))
        || !reservation_handle_is_directory(reservation)?
    {
        bail!("proof_availability_case_diagnostic_write_failed")
    }
    Ok(())
}

#[cfg(not(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
)))]
pub(crate) fn write_invalid_case_diagnostic(
    _: &CaseDiagnosticReservation,
    _: &str,
    _: &str,
    _: &str,
    _: &CaseValidationFailure,
    _: &[String],
) -> Result<()> {
    bail!("proof_availability_case_diagnostic_unsupported")
}

#[allow(clippy::too_many_arguments)]
fn build_invalid_case_diagnostic_artifact(
    qualification_id: &str,
    source_commit: &str,
    source_tree: &str,
    case_ordinal: usize,
    case_id: &str,
    repository_id: &str,
    unredacted_case: Value,
    project_materialization: Value,
    forbidden_values: &[String],
) -> Result<Value> {
    let case_sha256 = domain_sha256(
        b"codestory.proof-availability.invalid-case-unredacted.v1\\0",
        &canonical_json_bytes(&unredacted_case)?,
    );
    let mut redacted_case = unredacted_case;
    let mut text_commitments = Vec::new();
    redact_text_values(&mut redacted_case, "", &mut text_commitments)?;
    let artifact = json!({
        "schema": CASE_DIAGNOSTIC_SCHEMA,
        "classification": "non_evidence",
        "qualification_id": qualification_id,
        "validator_source_commit": source_commit,
        "validator_source_tree": source_tree,
        "case_ordinal": case_ordinal,
        "case_id": case_id,
        "repository_id": repository_id,
        "failure_code": "proof_availability_case_invalid",
        "unredacted_case_sha256": case_sha256,
        "project_materialization": project_materialization,
        "case": redacted_case,
        "removed_text_commitments": text_commitments,
    });
    validate_private_diagnostic_value(&artifact, forbidden_values)?;
    Ok(artifact)
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
fn reservation_handle_is_directory(reservation: &CaseDiagnosticReservation) -> Result<bool> {
    let metadata = reservation.directory.metadata()?;
    if !metadata.is_dir() {
        return Ok(false);
    }
    use std::os::unix::fs::MetadataExt as _;
    Ok(metadata.dev() == reservation.device && metadata.ino() == reservation.inode)
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
fn path_matches_held_directory(reservation: &CaseDiagnosticReservation) -> Result<bool> {
    let metadata = fs::symlink_metadata(&reservation.path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    use std::os::unix::fs::MetadataExt as _;
    Ok(metadata.dev() == reservation.device && metadata.ino() == reservation.inode)
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>> {
    codestory_agent::proof_qualification_support::canonical_json_bytes(value)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_write_failed"))
}

fn domain_sha256(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn redact_text_values(
    value: &mut Value,
    pointer: &str,
    commitments: &mut Vec<Value>,
) -> Result<()> {
    match value {
        Value::Object(object) => {
            if let Some(text) = object.remove("text") {
                let text = text.as_str().ok_or_else(|| {
                    anyhow::anyhow!("proof_availability_case_diagnostic_write_failed")
                })?;
                commitments.push(json!({
                    "json_pointer": format!("{pointer}/text"),
                    "utf8_byte_length": text.len(),
                    "sha256": domain_sha256(b"codestory.proof-availability.removed-text.v1\\0", text.as_bytes()),
                }));
            }
            for (key, nested) in object.iter_mut() {
                redact_text_values(
                    nested,
                    &format!("{pointer}/{}", json_pointer_escape(key)),
                    commitments,
                )?;
            }
        }
        Value::Array(array) => {
            for (index, nested) in array.iter_mut().enumerate() {
                redact_text_values(nested, &format!("{pointer}/{index}"), commitments)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn json_pointer_escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn validate_private_diagnostic_value(value: &Value, forbidden_values: &[String]) -> Result<()> {
    validate_private_diagnostic_value_at(value, forbidden_values, "")
}

fn validate_private_diagnostic_value_at(
    value: &Value,
    forbidden_values: &[String],
    pointer: &str,
) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, nested) in object {
                let lower = key.to_ascii_lowercase();
                if matches!(lower.as_str(), "text" | "source_text")
                    || [
                        "secret",
                        "token",
                        "password",
                        "private-key",
                        "private_key",
                        "api-key",
                        "api_key",
                    ]
                    .iter()
                    .any(|needle| lower.contains(needle))
                {
                    bail!("proof_availability_case_diagnostic_unsafe_value")
                }
                validate_private_diagnostic_value_at(
                    nested,
                    forbidden_values,
                    &format!("{pointer}/{}", json_pointer_escape(key)),
                )?;
            }
        }
        Value::Array(array) => {
            for (index, nested) in array.iter().enumerate() {
                validate_private_diagnostic_value_at(
                    nested,
                    forbidden_values,
                    &format!("{pointer}/{index}"),
                )?;
            }
        }
        Value::String(string) => {
            let forbidden = forbidden_values
                .iter()
                .any(|needle| !needle.is_empty() && string.contains(needle));
            if is_removed_text_commitment_pointer_field(pointer) {
                if !valid_json_pointer(string) || forbidden {
                    bail!("proof_availability_case_diagnostic_unsafe_value")
                }
            } else if is_absolute_path(string) || forbidden {
                bail!("proof_availability_case_diagnostic_unsafe_value")
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_removed_text_commitment_pointer_field(pointer: &str) -> bool {
    (pointer.starts_with("/removed_text_commitments/") && pointer.ends_with("/json_pointer"))
        || matches!(
            pointer,
            "/failure/json_pointer" | "/failure/parent_json_pointer"
        )
}

fn valid_json_pointer(value: &str) -> bool {
    value.is_empty()
        || (value.starts_with('/')
            && value.split('/').skip(1).all(|segment| {
                let mut bytes = segment.bytes();
                while let Some(byte) = bytes.next() {
                    if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
                        return false;
                    }
                    if byte.is_ascii_control() {
                        return false;
                    }
                }
                true
            }))
}

fn is_absolute_path(value: &str) -> bool {
    Path::new(value).is_absolute()
        || value.starts_with("\\\\")
        || value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
static CASE_DIAGNOSTIC_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(all(
    unix,
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )
))]
fn write_private_diagnostic_file(
    reservation: &CaseDiagnosticReservation,
    kind: PrivateDiagnosticFileKind,
    bytes: &[u8],
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let sequence = CASE_DIAGNOSTIC_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary_name = format!(
        "{}-{}-{sequence}",
        kind.temporary_prefix(),
        std::process::id()
    );
    let temporary_c = CString::new(temporary_name.as_bytes())
        .map_err(|_| std::io::Error::other("invalid temporary name"))?;
    let final_c =
        CString::new(kind.file_name()).map_err(|_| std::io::Error::other("invalid final name"))?;
    let directory_fd = reservation.directory.as_raw_fd();
    let descriptor = unsafe {
        libc::openat(
            directory_fd,
            temporary_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut temporary = unsafe { File::from_raw_fd(descriptor) };
    if let Err(error) = temporary
        .write_all(bytes)
        .and_then(|_| temporary.sync_all())
    {
        drop(temporary);
        let _ = unsafe { libc::unlinkat(directory_fd, temporary_c.as_ptr(), 0) };
        return Err(error);
    }
    drop(temporary);
    let rename_result = rename_noreplace_at(directory_fd, &temporary_c, directory_fd, &final_c);
    if let Err(error) = rename_result {
        let _ = unsafe { libc::unlinkat(directory_fd, temporary_c.as_ptr(), 0) };
        return Err(error);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_noreplace_at(
    from_fd: std::os::fd::RawFd,
    from: &std::ffi::CString,
    to_fd: std::os::fd::RawFd,
    to: &std::ffi::CString,
) -> std::io::Result<()> {
    let result = unsafe {
        libc::renameat2(
            from_fd,
            from.as_ptr(),
            to_fd,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_noreplace_at(
    from_fd: std::os::fd::RawFd,
    from: &std::ffi::CString,
    to_fd: std::os::fd::RawFd,
    to: &std::ffi::CString,
) -> std::io::Result<()> {
    let result = unsafe {
        libc::renameatx_np(
            from_fd,
            from.as_ptr(),
            to_fd,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[derive(Debug)]
pub(crate) struct QualificationReportInputV1 {
    pub qualification_id: String,
    pub source_commit: String,
    pub source_tree: String,
    pub environment: EnvironmentReportV1,
    pub inventory: Vec<InventoryReportV1>,
    pub trails: Vec<TrailReportV1>,
    pub cases: Vec<CaseReportV1>,
    pub failure_funnel: FailureFunnelReportV1,
}

#[derive(Debug, Clone)]
pub(crate) struct PublicArtifactBundle {
    environment: Value,
    inventory: Value,
    trails: Value,
    cases: Value,
    failure_funnel: Value,
    summary: Value,
    decision: Value,
    findings: String,
}

impl PublicArtifactBundle {
    fn files(&self) -> Result<Vec<(&'static str, Vec<u8>)>> {
        Ok(vec![
            ("environment.json", canonical_json_file(&self.environment)?),
            ("inventory.json", canonical_json_file(&self.inventory)?),
            ("trails.json", canonical_json_file(&self.trails)?),
            ("cases.json", canonical_json_file(&self.cases)?),
            (
                "failure-funnel.json",
                canonical_json_file(&self.failure_funnel)?,
            ),
            ("summary.json", canonical_json_file(&self.summary)?),
            ("decision.json", canonical_json_file(&self.decision)?),
            ("findings.md", normalized_findings(&self.findings)?),
        ])
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PublicLeakPolicy {
    forbidden_values: Vec<String>,
}

impl PublicLeakPolicy {
    pub(crate) fn new<I, S>(values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            forbidden_values: values
                .into_iter()
                .map(|value| value.as_ref().to_owned())
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ReportPublishError {
    pub code: &'static str,
    pub destination: PathBuf,
    pub staging: Option<PathBuf>,
}

impl ReportPublishError {
    fn before_staging(code: &'static str, destination: &Path) -> Self {
        Self {
            code,
            destination: destination.to_path_buf(),
            staging: None,
        }
    }

    fn recoverable(code: &'static str, destination: &Path, staging: &Path) -> Self {
        Self {
            code,
            destination: destination.to_path_buf(),
            staging: Some(staging.to_path_buf()),
        }
    }
}

impl fmt::Display for ReportPublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: destination={}",
            self.code,
            self.destination.display()
        )?;
        if let Some(staging) = &self.staging {
            write!(formatter, " staging={}", staging.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for ReportPublishError {}

pub(crate) fn build_summary(
    mut input: QualificationReportInputV1,
    corpus: &CorpusV1,
    thresholds: &ThresholdsV1,
) -> Result<QualificationSummaryV1> {
    corpus.validate_against_thresholds(thresholds)?;
    sort_evidence(
        &mut input.environment,
        &mut input.inventory,
        &mut input.trails,
        &mut input.cases,
        &mut input.failure_funnel,
    )?;
    let corpus_sha256 = canonical_corpus_sha256(corpus)?;
    let thresholds_sha256 = canonical_thresholds_sha256(thresholds)?;
    if input.environment.invocation.corpus_sha256 != corpus_sha256
        || input.environment.invocation.thresholds_sha256 != thresholds_sha256
    {
        bail!("proof_availability_environment_input_binding_invalid")
    }
    let results_sha256 = results_evidence_sha256(
        &input.environment,
        &input.inventory,
        &input.trails,
        &input.cases,
        &input.failure_funnel,
    )?;
    let summary = QualificationSummaryV1 {
        schema: REPORT_SCHEMA.to_owned(),
        qualification_id: input.qualification_id,
        provenance: ProvenanceV1 {
            source_commit: input.source_commit,
            source_tree: input.source_tree,
            binary_sha256: input.environment.binary_sha256.clone(),
            corpus_sha256,
            thresholds_sha256,
            results_sha256,
        },
        environment: input.environment,
        inventory: input.inventory,
        trails: input.trails,
        cases: input.cases,
        failure_funnel: input.failure_funnel,
    };
    summary.validate_against_inputs(corpus, thresholds)?;
    Ok(summary)
}

pub(crate) fn build_public_artifacts(
    summary: &QualificationSummaryV1,
    corpus: &CorpusV1,
    thresholds: &ThresholdsV1,
) -> std::result::Result<PublicArtifactBundle, PublicArtifactBuildFailure> {
    summary
        .validate_against_inputs(corpus, thresholds)
        .map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::SummaryValidation,
                PublicArtifactBuildReason::ExistingInvariant,
            )
        })?;
    let observations = derive_observations(summary, thresholds).map_err(|_| {
        PublicArtifactBuildFailure::closed(
            PublicArtifactBuildStage::ObservationDerivation,
            PublicArtifactBuildReason::ExistingInvariant,
        )
    })?;
    let observations_sha256 = canonical_observations_sha256(
        &summary.provenance.results_sha256,
        &summary.provenance.thresholds_sha256,
        &observations,
    )
    .map_err(|_| {
        PublicArtifactBuildFailure::closed(
            PublicArtifactBuildStage::ObservationCanonicalization,
            PublicArtifactBuildReason::Canonicalization,
        )
    })?;
    let decision = evaluate_activation_decision(summary, corpus, thresholds).map_err(|_| {
        PublicArtifactBuildFailure::closed(
            PublicArtifactBuildStage::DecisionEvaluation,
            PublicArtifactBuildReason::ExistingInvariant,
        )
    })?;
    let decision_report = ActivationDecisionReportV1 {
        schema: DECISION_REPORT_SCHEMA.to_owned(),
        results_sha256: summary.provenance.results_sha256.clone(),
        thresholds_sha256: summary.provenance.thresholds_sha256.clone(),
        observations,
        observations_sha256,
        decision,
    };
    decision_report.validate().map_err(|_| {
        PublicArtifactBuildFailure::closed(
            PublicArtifactBuildStage::DecisionValidation,
            PublicArtifactBuildReason::ExistingInvariant,
        )
    })?;
    let findings = render_findings(summary, thresholds, &decision_report).map_err(|_| {
        PublicArtifactBuildFailure::closed(
            PublicArtifactBuildStage::FindingsRendering,
            PublicArtifactBuildReason::ExistingInvariant,
        )
    })?;
    let bundle = PublicArtifactBundle {
        environment: serde_json::to_value(&summary.environment).map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::Serialization,
                PublicArtifactBuildReason::Serialization,
            )
        })?,
        inventory: serde_json::to_value(&summary.inventory).map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::Serialization,
                PublicArtifactBuildReason::Serialization,
            )
        })?,
        trails: serde_json::to_value(&summary.trails).map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::Serialization,
                PublicArtifactBuildReason::Serialization,
            )
        })?,
        cases: serde_json::to_value(&summary.cases).map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::Serialization,
                PublicArtifactBuildReason::Serialization,
            )
        })?,
        failure_funnel: serde_json::to_value(&summary.failure_funnel).map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::Serialization,
                PublicArtifactBuildReason::Serialization,
            )
        })?,
        summary: serde_json::to_value(summary).map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::Serialization,
                PublicArtifactBuildReason::Serialization,
            )
        })?,
        decision: serde_json::to_value(decision_report).map_err(|_| {
            PublicArtifactBuildFailure::closed(
                PublicArtifactBuildStage::Serialization,
                PublicArtifactBuildReason::Serialization,
            )
        })?,
        findings,
    };
    validate_public_bundle_for_build(&bundle)?;
    Ok(bundle)
}

pub(crate) fn build_and_publish(
    destination: &Path,
    summary: &QualificationSummaryV1,
    corpus: &CorpusV1,
    thresholds: &ThresholdsV1,
    leak_policy: &PublicLeakPolicy,
    diagnostic: PublicArtifactDiagnosticContext<'_>,
) -> std::result::Result<(), ReportPublishError> {
    require_result_directory_identity(destination, &summary.qualification_id).map_err(|_| {
        ReportPublishError::before_staging(
            "proof_availability_result_directory_qualification_id_mismatch",
            destination,
        )
    })?;
    let bundle = match build_public_artifacts(summary, corpus, thresholds) {
        Ok(bundle) => bundle,
        Err(failure) => {
            write_public_artifact_build_diagnostic(
                diagnostic.reservation,
                diagnostic.qualification_id,
                diagnostic.source_commit,
                diagnostic.source_tree,
                &failure,
            )
            .map_err(|_| {
                ReportPublishError::before_staging(
                    "proof_availability_public_artifact_build_failed",
                    destination,
                )
            })?;
            return Err(ReportPublishError::before_staging(
                "proof_availability_public_artifact_build_failed",
                destination,
            ));
        }
    };
    publish_bundle(destination, &bundle, leak_policy)
}

pub(crate) struct PublicArtifactDiagnosticContext<'a> {
    reservation: &'a CaseDiagnosticReservation,
    qualification_id: &'a str,
    source_commit: &'a str,
    source_tree: &'a str,
}

impl<'a> PublicArtifactDiagnosticContext<'a> {
    pub(crate) fn new(
        reservation: &'a CaseDiagnosticReservation,
        qualification_id: &'a str,
        source_commit: &'a str,
        source_tree: &'a str,
    ) -> Self {
        Self {
            reservation,
            qualification_id,
            source_commit,
            source_tree,
        }
    }
}

pub(crate) fn verify_published(
    destination: &Path,
    corpus: &CorpusV1,
    thresholds: &ThresholdsV1,
    path_files: &[CohortPathFileV1],
    leak_policy: &PublicLeakPolicy,
) -> Result<()> {
    require_exact_artifact_set(destination)?;
    let read_json = |name: &str| -> Result<(Value, Vec<u8>)> {
        let bytes = read_bounded(&destination.join(name))?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse proof availability artifact {name}"))?;
        if canonical_json_file(&value)? != bytes {
            bail!("proof_availability_artifact_not_canonical")
        }
        Ok((value, bytes))
    };
    let (environment_value, _) = read_json("environment.json")?;
    let (inventory_value, _) = read_json("inventory.json")?;
    let (trails_value, _) = read_json("trails.json")?;
    let (cases_value, _) = read_json("cases.json")?;
    let (funnel_value, _) = read_json("failure-funnel.json")?;
    let (summary_value, _) = read_json("summary.json")?;
    let (decision_value, _) = read_json("decision.json")?;
    let findings_bytes = read_bounded(&destination.join("findings.md"))?;

    let summary: QualificationSummaryV1 = serde_json::from_value(summary_value)?;
    require_result_directory_identity(destination, &summary.qualification_id)?;
    let reconstructed = QualificationSummaryV1 {
        schema: summary.schema.clone(),
        qualification_id: summary.qualification_id.clone(),
        provenance: summary.provenance.clone(),
        environment: serde_json::from_value(environment_value)?,
        inventory: serde_json::from_value(inventory_value)?,
        trails: serde_json::from_value(trails_value)?,
        cases: serde_json::from_value(cases_value)?,
        failure_funnel: serde_json::from_value(funnel_value)?,
    };
    if canonical_json_file(&summary)? != canonical_json_file(&reconstructed)? {
        bail!("proof_availability_split_artifact_mismatch")
    }
    reconstructed.validate_against_oracle(corpus, path_files)?;
    let recomputed = build_summary(
        QualificationReportInputV1 {
            qualification_id: reconstructed.qualification_id.clone(),
            source_commit: reconstructed.provenance.source_commit.clone(),
            source_tree: reconstructed.provenance.source_tree.clone(),
            environment: reconstructed.environment.clone(),
            inventory: reconstructed.inventory.clone(),
            trails: reconstructed.trails.clone(),
            cases: reconstructed.cases.clone(),
            failure_funnel: reconstructed.failure_funnel.clone(),
        },
        corpus,
        thresholds,
    )?;
    recomputed.validate_against_oracle(corpus, path_files)?;
    if canonical_json_file(&recomputed)? != canonical_json_file(&reconstructed)? {
        bail!("proof_availability_summary_recomputation_mismatch")
    }
    let expected = build_public_artifacts(&recomputed, corpus, thresholds)?;
    validate_public_bundle(&expected, leak_policy)?;
    let actual = PublicArtifactBundle {
        environment: serde_json::to_value(&recomputed.environment)?,
        inventory: serde_json::to_value(&recomputed.inventory)?,
        trails: serde_json::to_value(&recomputed.trails)?,
        cases: serde_json::to_value(&recomputed.cases)?,
        failure_funnel: serde_json::to_value(&recomputed.failure_funnel)?,
        summary: serde_json::to_value(&recomputed)?,
        decision: decision_value.clone(),
        findings: String::from_utf8(findings_bytes)?,
    };
    validate_public_bundle(&actual, leak_policy)?;
    if artifact_file_map(&actual)? != artifact_file_map(&expected)? {
        bail!("proof_availability_artifact_recomputation_mismatch")
    }
    let decision: ActivationDecisionReportV1 = serde_json::from_value(decision_value)?;
    decision.validate()?;
    Ok(())
}

pub(crate) fn require_result_directory_identity(
    destination: &Path,
    qualification_id: &str,
) -> Result<()> {
    if destination.file_name().and_then(|name| name.to_str()) != Some(qualification_id) {
        bail!("proof_availability_result_directory_qualification_id_mismatch")
    }
    Ok(())
}

fn sort_evidence(
    environment: &mut EnvironmentReportV1,
    inventory: &mut [InventoryReportV1],
    trails: &mut [TrailReportV1],
    cases: &mut [CaseReportV1],
    failure_funnel: &mut FailureFunnelReportV1,
) -> Result<()> {
    environment
        .projects
        .sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    inventory.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    trails.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    for trail in trails {
        trail.lengths.sort_by_key(|length| length.length);
    }
    cases.sort_by(|left, right| left.case_id.cmp(&right.case_id));
    for case in cases {
        case.proof_trace
            .selectors
            .sort_by_key(|selector| selector.selector_index);
        case.proof_trace.steps.sort_by_key(|step| step.step_index);
        for step in &mut case.proof_trace.steps {
            step.candidate_edge_ids.sort_unstable();
            match &mut step.outcome {
                StepQualificationOutcomeV1::SelectorBlocked { .. } => {}
                StepQualificationOutcomeV1::Admitted { edge_ids } => edge_ids.sort_unstable(),
                StepQualificationOutcomeV1::FirstZeroSurvivor { histogram, .. } => {
                    for bucket in histogram.iter_mut() {
                        bucket.edge_ids.sort_unstable();
                    }
                    histogram.sort_by(|left, right| {
                        canonical_json_file(&left.reason)
                            .expect("closed candidate failure serializes")
                            .cmp(
                                &canonical_json_file(&right.reason)
                                    .expect("closed candidate failure serializes"),
                            )
                    });
                }
                StepQualificationOutcomeV1::CandidateLimitExceeded { .. } => {}
            }
        }
        case.unclassified_step_indices.sort_unstable();
        case.receipt_evidence
            .observed_receipts
            .sort_by(|left, right| {
                (left.step_index, left.edge_id, left.receipt_id.as_str()).cmp(&(
                    right.step_index,
                    right.edge_id,
                    right.receipt_id.as_str(),
                ))
            });
        for receipt in &mut case.receipt_evidence.observed_receipts {
            if let ReceiptOracleComparisonV1::Mismatched { mismatches, .. } =
                &mut receipt.oracle_comparison
            {
                mismatches.sort();
            }
        }
        case.receipt_evidence
            .missing_oracle_steps
            .sort_by_key(|missing| missing.step_index);
        case.product_disposition.authoritative_receipts.sort();
        case.product_disposition.gaps.sort();
        sort_actual_product_result(&mut case.product_disposition.actual);
        if let TransportEvidenceV1::Measurements { measurements } = &mut case.transport {
            measurements.measurements.sort_by_key(|measurement| {
                serde_json::to_string(&measurement.revision)
                    .expect("closed MCP revision serializes")
            });
        }
        case.negative_mutations.sort_by(|left, right| {
            left.mutation_id
                .cmp(&right.mutation_id)
                .then_with(|| left.step_index.cmp(&right.step_index))
        });
    }
    for bucket in &mut failure_funnel.buckets {
        if let FunnelOutcomeV1::FirstZeroSurvivor { histogram, .. } = &mut bucket.outcome {
            for failure in histogram.iter_mut() {
                failure.edge_ids.sort_unstable();
            }
            histogram.sort_by(|left, right| {
                canonical_json_file(&left.reason)
                    .expect("closed candidate failure serializes")
                    .cmp(
                        &canonical_json_file(&right.reason)
                            .expect("closed candidate failure serializes"),
                    )
            });
        }
    }
    failure_funnel.buckets.sort_by(|left, right| {
        canonical_json_file(&left.outcome)
            .expect("closed funnel outcome serializes")
            .cmp(&canonical_json_file(&right.outcome).expect("closed funnel outcome serializes"))
    });
    Ok(())
}

fn sort_projected_receipts(receipts: &mut [ProjectedReceiptReferenceV1]) {
    receipts.sort_by(|left, right| {
        left.receipt_id
            .cmp(&right.receipt_id)
            .then_with(|| {
                left.edge_id
                    .parse::<i64>()
                    .ok()
                    .cmp(&right.edge_id.parse::<i64>().ok())
            })
            .then_with(|| left.edge_id.cmp(&right.edge_id))
    });
}

fn sort_actual_product_result(result: &mut ActualProductResultV1) {
    match result {
        ActualProductResultV1::ContractProven { receipts, .. } => sort_projected_receipts(receipts),
        ActualProductResultV1::ContractRefuted { basis, .. } => match basis {
            ProductRefutationBasisV1::PositiveContradiction {
                connected_receipts, ..
            }
            | ProductRefutationBasisV1::CertifiedAbsence {
                connected_receipts, ..
            } => sort_projected_receipts(connected_receipts),
        },
        ActualProductResultV1::Unknown {
            gaps,
            connected_receipts,
            ..
        } => {
            gaps.sort();
            sort_projected_receipts(connected_receipts);
        }
        ActualProductResultV1::Unavailable { reasons, .. } => reasons.sort_by_key(|reason| {
            serde_json::to_string(reason).expect("closed unavailable reason serializes")
        }),
        ActualProductResultV1::Invalid { .. } => {}
    }
}

#[derive(Debug)]
struct FindingsRoleThresholdV1 {
    role: String,
    minimum_full_proofs: u16,
    minimum_full_proofs_per_cohort: u16,
    minimum_full_proof_wilson_lower_milli: u16,
    minimum_cohort_wilson_lower_milli: u16,
    minimum_positive_step_recall_milli: u16,
    minimum_full_or_useful_partial_milli: u16,
    minimum_actionable_exact_gap_milli: u16,
    maximum_unknown_p95_ms: u64,
    maximum_transport_p95_ms: u64,
    maximum_complete_response_p95_bytes: u64,
    maximum_unknown_response_p95_bytes: u64,
    maximum_response_bytes: u64,
}

impl FindingsRoleThresholdV1 {
    fn from_threshold(role: &str, threshold: &RoleThresholdsV1) -> Self {
        Self {
            role: role.to_owned(),
            minimum_full_proofs: threshold.minimum_full_proofs,
            minimum_full_proofs_per_cohort: threshold.minimum_full_proofs_per_cohort,
            minimum_full_proof_wilson_lower_milli: threshold.minimum_full_proof_wilson_lower_milli,
            minimum_cohort_wilson_lower_milli: threshold.minimum_cohort_wilson_lower_milli,
            minimum_positive_step_recall_milli: threshold.minimum_positive_step_recall_milli,
            minimum_full_or_useful_partial_milli: threshold.minimum_full_or_useful_partial_milli,
            minimum_actionable_exact_gap_milli: threshold.minimum_actionable_exact_gap_milli,
            maximum_unknown_p95_ms: threshold.maximum_unknown_p95_ms,
            maximum_transport_p95_ms: threshold.maximum_transport_p95_ms,
            maximum_complete_response_p95_bytes: threshold.maximum_complete_response_p95_bytes,
            maximum_unknown_response_p95_bytes: threshold.maximum_unknown_response_p95_bytes,
            maximum_response_bytes: threshold.maximum_response_bytes,
        }
    }
}

#[derive(Debug)]
struct FindingsDocumentV1 {
    qualification_id: String,
    source_commit: String,
    source_tree: String,
    binary_sha256: String,
    corpus_sha256: String,
    thresholds_sha256: String,
    results_sha256: String,
    positive_requests: u64,
    attempted_positive_steps: u16,
    exact_positive_steps: u16,
    contract_proven_cases: u64,
    negative_contract_proven: u64,
    authoritative_receipts: u64,
    exact_authoritative_receipts: u64,
    unclassified_positive_steps: u16,
    maximum_response_bytes: u64,
    cohort_full_proofs: Vec<(String, u64)>,
    inventory: Vec<InventoryReportV1>,
    trails: Vec<TrailReportV1>,
    thresholds_id: String,
    methodology_sha256: String,
    hard_gate_summary: String,
    roles: Vec<FindingsRoleThresholdV1>,
    outcome: String,
    automatic_thresholds_met: Option<bool>,
    failed_gates: Vec<(String, String)>,
}

fn render_findings(
    summary: &QualificationSummaryV1,
    thresholds: &ThresholdsV1,
    decision_report: &ActivationDecisionReportV1,
) -> Result<String> {
    let decision = &decision_report.decision;
    let receipt_metrics = summary.receipt_metrics()?;
    let mut cohort_full_proofs = summary
        .environment
        .projects
        .iter()
        .map(|project| (project.repository_id.clone(), 0u64))
        .collect::<BTreeMap<_, _>>();
    let mut contract_proven_cases = 0u64;
    let mut negative_contract_proven = 0u64;
    let mut maximum_response_bytes = 0u64;
    for case in &summary.cases {
        let full = case.evaluable_facts()?.contract_proven_supported
            && matches!(
                case.product_disposition.kind,
                ProductDispositionKindV1::ContractProven
            );
        if full {
            contract_proven_cases = contract_proven_cases
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_findings_count_overflow"))?;
            *cohort_full_proofs
                .get_mut(&case.repository_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_findings_cohort_missing"))? += 1;
        }
        negative_contract_proven = negative_contract_proven
            .checked_add(u64::try_from(
                case.negative_mutations
                    .iter()
                    .filter(|mutation| mutation.contract_proven)
                    .count(),
            )?)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_findings_count_overflow"))?;
        maximum_response_bytes = maximum_response_bytes.max(case.complete_projection_bytes);
        match &case.transport {
            TransportEvidenceV1::Measurements { measurements } => {
                for measurement in &measurements.measurements {
                    maximum_response_bytes = maximum_response_bytes.max(measurement.actual_bytes);
                }
            }
            TransportEvidenceV1::Error {
                error:
                    TransportErrorV1::ResultExceedsBudget {
                        maximum_bytes,
                        actual_bytes,
                    },
            } => {
                maximum_response_bytes = maximum_response_bytes
                    .max(*maximum_bytes)
                    .max(*actual_bytes);
            }
            TransportEvidenceV1::Error { .. } => {}
        }
    }
    let hard = &thresholds.hard_gates;
    let mut inventory = summary.inventory.clone();
    inventory.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    let mut trails = summary.trails.clone();
    trails.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    let document = FindingsDocumentV1 {
        qualification_id: summary.qualification_id.clone(),
        source_commit: summary.provenance.source_commit.clone(),
        source_tree: summary.provenance.source_tree.clone(),
        binary_sha256: summary.provenance.binary_sha256.clone(),
        corpus_sha256: summary.provenance.corpus_sha256.clone(),
        thresholds_sha256: summary.provenance.thresholds_sha256.clone(),
        results_sha256: summary.provenance.results_sha256.clone(),
        positive_requests: u64::try_from(summary.cases.len())?,
        attempted_positive_steps: summary.failure_funnel.attempted_positive_steps,
        exact_positive_steps: receipt_metrics.exact_oracle_step_count,
        contract_proven_cases,
        negative_contract_proven,
        authoritative_receipts: receipt_metrics.authoritative_receipt_count,
        exact_authoritative_receipts: receipt_metrics.authoritative_exact_receipt_count,
        unclassified_positive_steps: summary.failure_funnel.unclassified_positive_steps,
        maximum_response_bytes,
        cohort_full_proofs: cohort_full_proofs.into_iter().collect(),
        inventory,
        trails,
        thresholds_id: thresholds.thresholds_id.clone(),
        methodology_sha256: thresholds.methodology_sha256.clone(),
        hard_gate_summary: format!(
            "false_proofs<={}; exact_receipts={}; certified_absence<={}; complete_funnel={}; complete_provenance={}; invalid<={}; over_cap<={}; transport_errors<={}; maximum_bytes<={}; each_cohort={}; disposition_match={}",
            hard.maximum_false_contract_proven,
            hard.require_exact_receipt_matches,
            hard.maximum_certified_absence,
            hard.require_complete_failure_funnel,
            hard.require_complete_provenance,
            hard.maximum_invalid_results,
            hard.maximum_over_cap_results,
            hard.maximum_transport_errors,
            hard.maximum_proof_bytes,
            hard.require_each_cohort,
            hard.require_product_disposition_match,
        ),
        roles: vec![
            FindingsRoleThresholdV1::from_threshold("automatic", &thresholds.automatic),
            FindingsRoleThresholdV1::from_threshold("stable_explicit", &thresholds.stable_explicit),
            FindingsRoleThresholdV1::from_threshold("experimental", &thresholds.experimental),
        ],
        outcome: closed_enum_name(&decision.outcome)?,
        automatic_thresholds_met: decision.automatic_thresholds_met,
        failed_gates: decision
            .failed_gates
            .iter()
            .map(|gate| Ok((gate.gate_id.clone(), closed_enum_name(&gate.kind)?)))
            .collect::<Result<_>>()?,
    };
    let mut findings = render_findings_document(&document)?;
    findings.push_str(&render_derived_observations(&decision_report.observations));
    Ok(findings)
}

fn render_derived_observations(observations: &super::contracts::DerivedObservationsV1) -> String {
    let mut text = format!(
        "\n## Recomputed decision observations\n\n| Metric | Raw | Presentation |\n| --- | ---: | ---: |\n| Full proofs | {} / {} | {} milli |\n| Full-proof Wilson 95% | {} / {} | lower {:.17}, upper {:.17}, floor {} milli |\n| Positive-step recall | {} / {} | {} milli |\n| Full or useful partial | {} / {} | {} milli |\n| Actionable incomplete gap | {} / {} | {} milli |\n| Unknown warm p95 | - | {} ms |\n| Complete response p95 | - | {} bytes |\n| Unknown response p95 | - | {} bytes |\n| Maximum response | - | {} bytes |\n",
        observations.full_proofs.numerator,
        observations.full_proofs.denominator,
        observations.full_proofs.milli,
        observations.full_proof_wilson.numerator,
        observations.full_proof_wilson.denominator,
        observations.full_proof_wilson.lower,
        observations.full_proof_wilson.upper,
        observations.full_proof_wilson.lower_milli,
        observations.positive_step_recall.numerator,
        observations.positive_step_recall.denominator,
        observations.positive_step_recall.milli,
        observations.full_or_useful_partial.numerator,
        observations.full_or_useful_partial.denominator,
        observations.full_or_useful_partial.milli,
        observations.actionable_incomplete_gap.numerator,
        observations.actionable_incomplete_gap.denominator,
        observations.actionable_incomplete_gap.milli,
        observations.unknown_warm_p95_ms,
        observations.complete_response_p95_bytes,
        observations.unknown_response_p95_bytes,
        observations.maximum_response_bytes,
    );
    text.push_str("\n### Cohort Wilson observations\n\n| Cohort | Full proofs | Wilson 95% |\n| --- | ---: | ---: |\n");
    for cohort in &observations.cohorts {
        text.push_str(&format!(
            "| `{}` | {} / {} ({} milli) | lower {:.17}, upper {:.17}, floor {} milli |\n",
            cohort.repository_id,
            cohort.full_proofs.numerator,
            cohort.full_proofs.denominator,
            cohort.full_proofs.milli,
            cohort.wilson.lower,
            cohort.wilson.upper,
            cohort.wilson.lower_milli,
        ));
    }
    text.push_str("\n### Transport p95\n\n| Revision | Nanoseconds |\n| --- | ---: |\n");
    for transport in &observations.transport_p95 {
        text.push_str(&format!(
            "| `{}` | {} |\n",
            closed_enum_name(&transport.revision).expect("closed MCP revision"),
            transport.elapsed_ns,
        ));
    }
    text
}

fn render_findings_document(document: &FindingsDocumentV1) -> Result<String> {
    for value in [
        &document.qualification_id,
        &document.source_commit,
        &document.source_tree,
        &document.binary_sha256,
        &document.corpus_sha256,
        &document.thresholds_sha256,
        &document.results_sha256,
        &document.thresholds_id,
        &document.methodology_sha256,
        &document.outcome,
    ] {
        require_findings_atom(value)?;
    }
    for (repository, _) in &document.cohort_full_proofs {
        require_findings_atom(repository)?;
    }
    for inventory in &document.inventory {
        require_findings_atom(&inventory.repository_id)?;
    }
    for trail in &document.trails {
        require_findings_atom(&trail.repository_id)?;
    }
    for role in &document.roles {
        require_findings_atom(&role.role)?;
    }
    for (gate_id, kind) in &document.failed_gates {
        require_findings_atom(gate_id)?;
        require_findings_atom(kind)?;
    }

    let mut text = format!(
        "# Proof availability findings\n\nQualification: `{}`\n\n## Reproduced measurements\n\n| Measurement | Observed |\n| --- | ---: |\n| Positive requests | {} |\n| ContractProven cases with exact authoritative evidence | {} / {} |\n| Exact positive steps | {} / {} |\n| Negative mutations reaching ContractProven | {} |\n| Exact authoritative receipts | {} / {} |\n| Unclassified positive steps | {} |\n| Maximum response bytes | {} |\n",
        document.qualification_id,
        document.positive_requests,
        document.contract_proven_cases,
        document.positive_requests,
        document.exact_positive_steps,
        document.attempted_positive_steps,
        document.negative_contract_proven,
        document.exact_authoritative_receipts,
        document.authoritative_receipts,
        document.unclassified_positive_steps,
        document.maximum_response_bytes,
    );
    text.push_str("\n### Raw CALL inventory\n\n| Cohort | Stored | Effective endpoints | Exact resolved | Strictly admitted | Unresolved placeholders |\n| --- | ---: | ---: | ---: | ---: | ---: |\n");
    for inventory in &document.inventory {
        text.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            inventory.repository_id,
            inventory.stored_call_rows,
            inventory.effective_endpoint_rows,
            inventory.exact_resolved_rows,
            inventory.admitted_rows,
            inventory.unresolved_placeholder_rows,
        ));
    }
    text.push_str("\n### Raw edge-distinct trails\n\n| Cohort | Length | Effective endpoints | Exact resolved | Strictly admitted |\n| --- | ---: | ---: | ---: | ---: |\n");
    for trail in &document.trails {
        for counts in &trail.lengths {
            text.push_str(&format!(
                "| `{}` | {} | {} | {} | {} |\n",
                trail.repository_id,
                counts.length,
                counts.effective_endpoint,
                counts.exact_resolved,
                counts.strictly_admitted,
            ));
        }
    }
    text.push_str(
        "\n### Full proofs by cohort\n\n| Cohort | ContractProven cases |\n| --- | ---: |\n",
    );
    for (repository, count) in &document.cohort_full_proofs {
        text.push_str(&format!("| `{repository}` | {count} |\n"));
    }
    let incomplete = document
        .positive_requests
        .checked_sub(document.contract_proven_cases)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_findings_count_invalid"))?;
    text.push_str(&format!(
        "\n## Inferences\n\n- The evaluator selected `{}` from these reproduced measurements and the frozen thresholds below.\n- {} of {} cases satisfy the report contract's evidence-backed full-proof predicate.\n- {} cases do not satisfy that predicate.\n\n## Frozen thresholds\n\nThreshold set: `{}`  \nMethodology SHA-256: `{}`\n\nHard gates: `{}`\n\n| Role | Full proofs min | Cohort min | Full Wilson min milli | Cohort Wilson min milli | Step recall min milli | Full/useful min milli | Actionable gap min milli | Unknown p95 max ms | Transport p95 max ms | Complete p95 max bytes | Unknown p95 max bytes | Absolute max bytes |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
        document.outcome,
        document.contract_proven_cases,
        document.positive_requests,
        incomplete,
        document.thresholds_id,
        document.methodology_sha256,
        document.hard_gate_summary,
    ));
    for role in &document.roles {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            role.role,
            role.minimum_full_proofs,
            role.minimum_full_proofs_per_cohort,
            role.minimum_full_proof_wilson_lower_milli,
            role.minimum_cohort_wilson_lower_milli,
            role.minimum_positive_step_recall_milli,
            role.minimum_full_or_useful_partial_milli,
            role.minimum_actionable_exact_gap_milli,
            role.maximum_unknown_p95_ms,
            role.maximum_transport_p95_ms,
            role.maximum_complete_response_p95_bytes,
            role.maximum_unknown_response_p95_bytes,
            role.maximum_response_bytes,
        ));
    }
    let automatic = document
        .automatic_thresholds_met
        .map_or("not_applicable", |met| if met { "true" } else { "false" });
    text.push_str(&format!(
        "\n## Decision\n\nOutcome: `{}`  \nAutomatic thresholds met: `{automatic}`\n\n### Failed gates\n\n",
        document.outcome,
    ));
    if document.failed_gates.is_empty() {
        text.push_str("None.\n");
    } else {
        text.push_str("| Gate | Kind |\n| --- | --- |\n");
        for (gate_id, kind) in &document.failed_gates {
            text.push_str(&format!("| `{gate_id}` | `{kind}` |\n"));
        }
    }
    text.push_str(&format!(
        "\n### Provenance\n\n| Identity | Value |\n| --- | --- |\n| Source commit | `{}` |\n| Source tree | `{}` |\n| Binary SHA-256 | `{}` |\n| Corpus SHA-256 | `{}` |\n| Thresholds SHA-256 | `{}` |\n| Results SHA-256 | `{}` |\n\n### Nonclaims\n\n- This qualification does not prove runtime execution, temporal order, arbitrary reachability, ownership, data flow, extraction completeness, or subsystem non-participation.\n- It is source-built benchmark evidence for the dark exact-call-path kernel. It is not installed-host qualification, public proof availability, or release evidence.\n",
        document.source_commit,
        document.source_tree,
        document.binary_sha256,
        document.corpus_sha256,
        document.thresholds_sha256,
        document.results_sha256,
    ));
    Ok(text)
}

fn require_findings_atom(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        bail!("proof_availability_findings_atom_invalid")
    }
    Ok(())
}

fn closed_enum_name<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_findings_enum_invalid"))
}

fn canonical_json_file<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = codestory_agent::proof_qualification_support::canonical_json_bytes(value)
        .map_err(|error| anyhow::anyhow!(error))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn normalized_findings(findings: &str) -> Result<Vec<u8>> {
    if findings.contains('\0') || findings.contains('\r') {
        bail!("proof_availability_findings_invalid")
    }
    let mut normalized = findings.trim_end_matches('\n').as_bytes().to_vec();
    normalized.push(b'\n');
    Ok(normalized)
}

fn artifact_file_map(bundle: &PublicArtifactBundle) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let mut files = bundle.files()?;
    files.sort_by_key(|(name, _)| *name);
    Ok(files)
}

fn validate_public_bundle_for_build(
    bundle: &PublicArtifactBundle,
) -> std::result::Result<(), PublicArtifactBuildFailure> {
    for (name, value) in [
        (PublicArtifactName::Environment, &bundle.environment),
        (PublicArtifactName::Inventory, &bundle.inventory),
        (PublicArtifactName::Trails, &bundle.trails),
        (PublicArtifactName::Cases, &bundle.cases),
        (PublicArtifactName::FailureFunnel, &bundle.failure_funnel),
        (PublicArtifactName::Summary, &bundle.summary),
        (PublicArtifactName::Decision, &bundle.decision),
    ] {
        validate_public_json_for_build(name, value, "", None)?;
    }
    if bundle.findings.contains('\0') || bundle.findings.contains('\r') {
        return Err(PublicArtifactBuildFailure::rejected_string(
            PublicArtifactBuildStage::BundleValidation,
            PublicArtifactBuildReason::FindingsValidation,
            PublicArtifactName::Findings,
            None,
            "/text".to_owned(),
            &bundle.findings,
        ));
    }
    Ok(())
}

fn validate_public_json_for_build(
    artifact: PublicArtifactName,
    value: &Value,
    pointer: &str,
    case_ordinal: Option<usize>,
) -> std::result::Result<(), PublicArtifactBuildFailure> {
    match value {
        Value::Object(object) => {
            for (field, child) in object {
                if secret_field(field) {
                    let rejected = child.as_str().unwrap_or(field);
                    return Err(PublicArtifactBuildFailure::rejected_without_pointer(
                        PublicArtifactBuildStage::BundleValidation,
                        PublicArtifactBuildReason::SecretField,
                        artifact,
                        case_ordinal,
                        rejected,
                    ));
                }
                if field.bytes().any(|byte| byte.is_ascii_control()) {
                    return Err(PublicArtifactBuildFailure::rejected_without_pointer(
                        PublicArtifactBuildStage::BundleValidation,
                        PublicArtifactBuildReason::ControlCharacter,
                        artifact,
                        case_ordinal,
                        field,
                    ));
                }
                let Some(escaped) = fixed_json_pointer_segment(field) else {
                    return Err(
                        PublicArtifactBuildFailure::object_field_not_in_fixed_vocabulary(
                            artifact,
                            case_ordinal,
                            pointer,
                            field,
                        ),
                    );
                };
                let child_pointer =
                    append_known_object_pointer(pointer, escaped).ok_or_else(|| {
                        PublicArtifactBuildFailure::pointer_budget_exceeded(
                            PublicArtifactBuildReason::ObjectPointerBudgetExceeded,
                            artifact,
                            case_ordinal,
                            pointer,
                        )
                    })?;
                validate_public_json_for_build(artifact, child, &child_pointer, case_ordinal)?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                let child_pointer =
                    append_array_index_pointer(pointer, index).ok_or_else(|| {
                        PublicArtifactBuildFailure::pointer_budget_exceeded(
                            PublicArtifactBuildReason::ArrayPointerBudgetExceeded,
                            artifact,
                            case_ordinal,
                            pointer,
                        )
                    })?;
                let child_case_ordinal =
                    if artifact == PublicArtifactName::Cases && pointer.is_empty() {
                        Some(index)
                    } else {
                        case_ordinal
                    };
                validate_public_json_for_build(
                    artifact,
                    child,
                    &child_pointer,
                    child_case_ordinal,
                )?;
            }
        }
        Value::String(text) if !pointer.ends_with("/text") && absolute_path(text) => {
            return Err(PublicArtifactBuildFailure::rejected_string(
                PublicArtifactBuildStage::BundleValidation,
                PublicArtifactBuildReason::PathLeak,
                artifact,
                case_ordinal,
                pointer.to_owned(),
                text,
            ));
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
fn append_fixed_object_pointer(pointer: &str, segment: &str) -> Option<String> {
    let escaped = fixed_json_pointer_segment(segment)?;
    append_known_object_pointer(pointer, escaped)
}

fn append_known_object_pointer(pointer: &str, escaped: &str) -> Option<String> {
    let length = pointer.len().checked_add(1)?.checked_add(escaped.len())?;
    (length <= MAX_DIAGNOSTIC_POINTER_BYTES).then(|| format!("{pointer}/{escaped}"))
}

fn append_array_index_pointer(pointer: &str, index: usize) -> Option<String> {
    let index = index.to_string();
    let length = pointer.len().checked_add(1)?.checked_add(index.len())?;
    (length <= MAX_DIAGNOSTIC_POINTER_BYTES).then(|| format!("{pointer}/{index}"))
}

fn fixed_json_pointer_segment(segment: &str) -> Option<&'static str> {
    fixed_json_pointer_segments().find(|candidate| *candidate == segment)
}

fn fixed_json_pointer_segments() -> impl Iterator<Item = &'static str> {
    const FIXED: &str = "actionable_exact_gap actionable_incomplete_gap actual actual_bytes admitted_rows after_step_count architecture attempted_positive_steps attempted_step_count authoritative_receipts automatic_thresholds_met basis binary_name binary_sha256 boundary buckets build byte_end byte_start caller callsite_identity callsite_line candidate_edge_ids canonical_id canonical_id_binding_sha256 cargo_profile case_id cases certainty certified_absence classified_positive_steps code cohorts complete_projection_bytes complete_response_p95_bytes connected_receipts containment contract_digest contract_proven core_generation core_generation_id core_run_id corpus_sha256 count database_sha256 decision denominator detail edge_count edge_id edge_ids effective_endpoint effective_endpoint_rows elapsed_ns end_byte end_line enumeration_receipt_id environment environment_id error evidence exact_resolved exact_resolved_rows exclude_from_projection extractor_capability_receipt_id failed_gates failure failure_funnel false_contract_proven file_byte_length file_count file_node_id finalization freshness full_or_useful_partial full_proof_wilson full_proofs gap gaps gate gate_id hard_gates histogram identity incomplete_provenance indexed_sha256 invalid_results inventory invocation kind length lengths line_window lower lower_milli maximum_bytes maximum_candidate_edges maximum_response_bytes measurements message milli mismatches missing_oracle_steps mutated_spec mutation_id negative_mutations node_count node_id non_exact_authoritative_receipts numerator observations observations_sha256 observed observed_candidate_edges_at_least observed_receipts observed_sha256 operation oracle_comparison oracle_step oracle_step_index os outcome over_cap_results owner_node_id path path_id pinned positive_step_recall prescribed_argv product_disposition product_disposition_mismatches profile prohibit_traversal_through prohibition_index project_file_components project_id projection projection_bytes projects proof_trace provenance qualification_id qualification_source_commit qualification_source_tree qualified_name range reason reasons receipt_evidence receipt_file_sha256 receipt_id receipt_line_window receipts recorded_at repository_id required results_sha256 revision rust_host rustc_vv schema selector selector_early_return selector_index selectors sha256 source source_commit source_dirty source_head source_tree stage stage_durations_ms start start_byte start_line step_index steps store_schema stored_call_rows strictly_admitted symbol target text thresholds_sha256 trails transport transport_errors transport_p95 unclassified_positive_steps unclassified_step_indices unknown_response_p95_bytes unknown_warm_p95_ms unresolved_placeholder_rows upper validation warm_end_to_end_ms wilson";
    FIXED.split_ascii_whitespace()
}

fn validate_public_bundle(bundle: &PublicArtifactBundle, policy: &PublicLeakPolicy) -> Result<()> {
    for value in [
        &bundle.environment,
        &bundle.inventory,
        &bundle.trails,
        &bundle.cases,
        &bundle.failure_funnel,
        &bundle.summary,
        &bundle.decision,
    ] {
        validate_public_json(value, None)?;
        if policy
            .forbidden_values
            .iter()
            .any(|forbidden| json_contains(value, forbidden))
        {
            bail!("proof_availability_public_forbidden_value")
        }
    }
    if policy
        .forbidden_values
        .iter()
        .any(|forbidden| bundle.findings.contains(forbidden))
    {
        bail!("proof_availability_public_forbidden_value")
    }
    Ok(())
}

fn json_contains(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(values) => values.iter().any(|value| json_contains(value, needle)),
        Value::Object(object) => object
            .iter()
            .any(|(key, value)| key.contains(needle) || json_contains(value, needle)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn validate_public_json(value: &Value, key: Option<&str>) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (field, child) in object {
                if secret_field(field) {
                    bail!("proof_availability_public_secret_leak")
                }
                validate_public_json(child, Some(field))?;
            }
        }
        Value::Array(values) => {
            for child in values {
                validate_public_json(child, key)?;
            }
        }
        Value::String(text) if key != Some("text") && absolute_path(text) => {
            bail!("proof_availability_public_path_leak")
        }
        _ => {}
    }
    Ok(())
}

fn secret_field(field: &str) -> bool {
    let normalized = field.to_ascii_lowercase();
    ["secret", "token", "password", "private_key", "api_key"]
        .iter()
        .any(|needle| normalized == *needle || normalized.ends_with(&format!("_{needle}")))
}

fn absolute_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("\\\\")
        || value.starts_with("\\\\?\\")
        || (value.len() >= 3
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
}

pub(crate) fn publish_bundle(
    destination: &Path,
    bundle: &PublicArtifactBundle,
    policy: &PublicLeakPolicy,
) -> std::result::Result<(), ReportPublishError> {
    publish_bundle_with_hook(destination, bundle, policy, || Ok(()))
}

fn publish_bundle_with_hook<F>(
    destination: &Path,
    bundle: &PublicArtifactBundle,
    policy: &PublicLeakPolicy,
    before_publish: F,
) -> std::result::Result<(), ReportPublishError>
where
    F: FnOnce() -> std::io::Result<()>,
{
    if destination.exists() {
        return Err(ReportPublishError::before_staging(
            "proof_availability_output_exists",
            destination,
        ));
    }
    validate_public_bundle(bundle, policy).map_err(|error| {
        let code = match error.to_string().as_str() {
            "proof_availability_public_path_leak" => "proof_availability_public_path_leak",
            "proof_availability_public_secret_leak" => "proof_availability_public_secret_leak",
            "proof_availability_public_forbidden_value" => {
                "proof_availability_public_forbidden_value"
            }
            _ => "proof_availability_public_artifact_invalid",
        };
        ReportPublishError::before_staging(code, destination)
    })?;
    let parent = destination.parent().ok_or_else(|| {
        ReportPublishError::before_staging("proof_availability_output_parent_missing", destination)
    })?;
    if !parent.is_dir() {
        return Err(ReportPublishError::before_staging(
            "proof_availability_output_parent_missing",
            destination,
        ));
    }
    let mut staging_builder = tempfile::Builder::new();
    staging_builder.prefix(".codestory-proof-availability-report-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        staging_builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let staging = staging_builder
        .tempdir_in(parent)
        .map(tempfile::TempDir::keep)
        .map_err(|_| {
            ReportPublishError::before_staging(
                "proof_availability_staging_create_failed",
                destination,
            )
        })?;
    set_private_directory(&staging).map_err(|_| {
        ReportPublishError::recoverable(
            "proof_availability_staging_permissions_failed",
            destination,
            &staging,
        )
    })?;
    if stage_bundle(&staging, bundle).is_err() {
        return Err(ReportPublishError::recoverable(
            "proof_availability_staging_write_failed",
            destination,
            &staging,
        ));
    }
    if before_publish().is_err() {
        return Err(ReportPublishError::recoverable(
            "proof_availability_publish_hook_failed",
            destination,
            &staging,
        ));
    }
    fs::remove_file(staging.join(OWNER_MARKER)).map_err(|_| {
        ReportPublishError::recoverable(
            "proof_availability_staging_finalize_failed",
            destination,
            &staging,
        )
    })?;
    if sync_directory(&staging).is_err() {
        let _ = write_private_file(&staging.join(OWNER_MARKER), b"v1\n");
        return Err(ReportPublishError::recoverable(
            "proof_availability_staging_sync_failed",
            destination,
            &staging,
        ));
    }
    if rename_directory_noreplace(&staging, destination).is_err() {
        let _ = write_private_file(&staging.join(OWNER_MARKER), b"v1\n");
        let _ = sync_directory(&staging);
        return Err(ReportPublishError::recoverable(
            "proof_availability_publish_collision",
            destination,
            &staging,
        ));
    }
    sync_directory(parent).map_err(|_| {
        ReportPublishError::before_staging(
            "proof_availability_publish_parent_sync_failed",
            destination,
        )
    })?;
    Ok(())
}

fn stage_bundle(staging: &Path, bundle: &PublicArtifactBundle) -> Result<()> {
    write_private_file(&staging.join(OWNER_MARKER), b"v1\n")?;
    for (name, bytes) in bundle.files()? {
        write_private_file(&staging.join(name), &bytes)?;
    }
    sync_directory(staging)?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn set_private_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}

fn require_exact_artifact_set(destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(destination)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("proof_availability_output_not_directory")
    }
    let mut names = fs::read_dir(destination)?
        .map(|entry| {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                bail!("proof_availability_artifact_not_regular")
            }
            Ok(entry.file_name().to_string_lossy().into_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    if names
        != PUBLIC_ARTIFACT_NAMES
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    {
        bail!("proof_availability_artifact_set_invalid")
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_PUBLIC_ARTIFACT_BYTES
    {
        bail!("proof_availability_artifact_invalid")
    }
    fs::read(path).map_err(Into::into)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_directory_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename source contains NUL"))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename target contains NUL"))?;
    // SAFETY: both pointers remain valid NUL-terminated strings for the call.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_directory_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename source contains NUL"))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename target contains NUL"))?;
    // SAFETY: both pointers remain valid NUL-terminated strings for the call.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_directory_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))
))]
fn rename_directory_noreplace(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace directory rename unsupported",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::fs;

    fn fixture_bundle() -> PublicArtifactBundle {
        PublicArtifactBundle {
            environment: json!({"schema": "fixture"}),
            inventory: json!([]),
            trails: json!([]),
            cases: json!([]),
            failure_funnel: json!({}),
            summary: json!({}),
            decision: json!({}),
            findings: "# Findings\n\nNone.\n".to_owned(),
        }
    }

    #[test]
    fn result_directory_basename_is_the_qualification_identity() {
        let root = tempfile::tempdir().unwrap();
        let qualification_id = "20260821T120000Z-222222222222";
        require_result_directory_identity(&root.path().join(qualification_id), qualification_id)
            .unwrap();
        assert!(
            require_result_directory_identity(&root.path().join("results"), qualification_id)
                .unwrap_err()
                .to_string()
                .contains("result_directory_qualification_id_mismatch")
        );
    }

    #[test]
    fn canonical_json_is_compact_sorted_and_newline_terminated() {
        assert_eq!(
            canonical_json_file(&json!({"z":2,"a":1})).unwrap(),
            b"{\"a\":1,\"z\":2}\n"
        );
    }

    #[test]
    fn public_artifact_build_failure_keeps_only_a_path_commitment() {
        let failure = PublicArtifactBuildFailure::path_leak(
            PublicArtifactBuildStage::BundleValidation,
            PublicArtifactName::Cases,
            Some(0),
            "/0/canonical_id",
            "/Users/albert/private/canonical-id",
        );

        assert_eq!(
            failure.to_string(),
            "proof_availability_public_artifact_build_failed"
        );
        assert_eq!(
            format!("{failure:?}"),
            "proof_availability_public_artifact_build_failed"
        );
        let artifact = build_public_artifact_build_diagnostic_artifact(
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            &failure,
        )
        .unwrap();
        let bytes = canonical_json_file(&artifact).unwrap();

        assert_eq!(artifact["classification"], "non_evidence");
        assert_eq!(artifact["failure"]["stage"], "bundle_validation");
        assert_eq!(artifact["failure"]["reason"], "path_leak");
        assert_eq!(artifact["failure"]["artifact"], "cases.json");
        assert_eq!(artifact["failure"]["case_ordinal"], 0);
        assert_eq!(artifact["failure"]["json_pointer"], "/0/canonical_id");
        assert_eq!(
            artifact["failure"]["rejected_string"]["utf8_byte_length"],
            "/Users/albert/private/canonical-id".len()
        );
        assert_eq!(
            artifact["failure"]["rejected_string"]["sha256"],
            domain_sha256(
                b"codestory.proof-availability.public-artifact-build-rejected-string.v1\0",
                b"/Users/albert/private/canonical-id",
            )
        );
        assert!(
            !String::from_utf8(bytes)
                .unwrap()
                .contains("/Users/albert/private")
        );
    }

    #[test]
    fn public_artifact_validation_keeps_first_artifact_and_case_in_depth_first_order() {
        let mut bundle = fixture_bundle();
        bundle.environment = json!({"source_head": "C:\\\\private"});
        bundle.cases = json!([
            {"canonical_id": "\\\\server\\share\\private"},
            {"canonical_id": "\\\\?\\\\C:\\\\private"}
        ]);
        let failure = validate_public_bundle_for_build(&bundle).unwrap_err();
        assert_eq!(failure.artifact, Some(PublicArtifactName::Environment));
        assert_eq!(failure.case_ordinal, None);
        assert_eq!(failure.json_pointer(), Some("/source_head"));

        bundle.environment = json!({"source_head": "relative"});
        let failure = validate_public_bundle_for_build(&bundle).unwrap_err();
        assert_eq!(failure.artifact, Some(PublicArtifactName::Cases));
        assert_eq!(failure.case_ordinal, Some(0));
        assert_eq!(failure.json_pointer(), Some("/0/canonical_id"));
    }

    #[test]
    fn public_artifact_validation_commits_secret_field_without_retaining_it() {
        let mut bundle = fixture_bundle();
        bundle.cases = json!([{"api_token": "private\\0TOKEN=do-not-publish"}]);
        let failure = validate_public_bundle_for_build(&bundle).unwrap_err();
        let artifact = build_public_artifact_build_diagnostic_artifact(
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            &failure,
        )
        .unwrap();
        let bytes = canonical_json_file(&artifact).unwrap();

        assert_eq!(artifact["failure"]["reason"], "secret_field");
        assert_eq!(artifact["failure"]["case_ordinal"], 0);
        assert!(artifact["failure"]["json_pointer"].is_null());
        assert!(
            artifact["failure"]["rejected_string"]["sha256"]
                .as_str()
                .is_some()
        );
        let rendered = String::from_utf8(bytes).unwrap();
        assert!(!rendered.contains("TOKEN=do-not-publish"));
    }

    #[test]
    fn public_artifact_validation_rejects_hostile_object_keys_without_retaining_them() {
        for key in [
            "private prose /Users/albert/secret",
            "token-like-key=private",
        ] {
            let mut bundle = fixture_bundle();
            bundle.cases = Value::Array(vec![Value::Object(
                [(key.to_owned(), Value::String("relative".to_owned()))]
                    .into_iter()
                    .collect(),
            )]);
            let failure = validate_public_bundle_for_build(&bundle).unwrap_err();
            let artifact = build_public_artifact_build_diagnostic_artifact(
                "20260821T120000Z-222222222222",
                &"a".repeat(40),
                &"b".repeat(40),
                &failure,
            )
            .unwrap();
            let rendered = String::from_utf8(canonical_json_file(&artifact).unwrap()).unwrap();

            assert_eq!(
                artifact["failure"]["reason"],
                "object_field_not_in_fixed_vocabulary"
            );
            assert_eq!(artifact["failure"]["case_ordinal"], 0);
            assert_eq!(artifact["failure"]["parent_json_pointer"], "/0");
            assert_eq!(
                artifact["failure"]["field_name"]["utf8_byte_length"],
                key.len()
            );
            assert_eq!(
                artifact["failure"]["field_name"]["sha256"],
                domain_sha256(
                    b"codestory.proof-availability.public-artifact-build-object-field-name.v1\0",
                    key.as_bytes(),
                )
            );
            assert!(!rendered.contains(key));
            assert!(!rendered.contains("relative"));
        }
    }

    #[test]
    fn public_artifact_control_character_rejection_is_typed_and_redacted() {
        let mut bundle = fixture_bundle();
        bundle.cases = Value::Object(
            [("hostile\u{0}control".to_owned(), Value::Null)]
                .into_iter()
                .collect(),
        );
        let failure = validate_public_bundle_for_build(&bundle).unwrap_err();
        let artifact = build_public_artifact_build_diagnostic_artifact(
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            &failure,
        )
        .unwrap();
        let rendered = String::from_utf8(canonical_json_file(&artifact).unwrap()).unwrap();

        assert_eq!(artifact["failure"]["reason"], "control_character");
        assert!(artifact["failure"]["rejected_string"]["sha256"].is_string());
        assert!(!rendered.contains("hostile"));
    }

    #[test]
    fn public_artifact_findings_rejection_commits_nul_and_carriage_return_text() {
        for findings in ["private\u{0}token", "private\rpath"] {
            let mut bundle = fixture_bundle();
            bundle.findings = findings.to_owned();
            let failure = validate_public_bundle_for_build(&bundle).unwrap_err();
            let artifact = build_public_artifact_build_diagnostic_artifact(
                "20260821T120000Z-222222222222",
                &"a".repeat(40),
                &"b".repeat(40),
                &failure,
            )
            .unwrap();
            let rendered = String::from_utf8(canonical_json_file(&artifact).unwrap()).unwrap();

            assert_eq!(artifact["failure"]["artifact"], "findings.md");
            assert_eq!(artifact["failure"]["reason"], "findings_validation");
            assert_eq!(
                artifact["failure"]["rejected_string"]["utf8_byte_length"],
                findings.len()
            );
            assert_eq!(
                artifact["failure"]["rejected_string"]["sha256"],
                domain_sha256(
                    b"codestory.proof-availability.public-artifact-build-rejected-string.v1\0",
                    findings.as_bytes(),
                )
            );
            assert!(!rendered.contains(findings));
        }
    }

    #[test]
    fn public_artifact_pointer_cap_accepts_the_exact_limit_and_rejects_the_next_byte() {
        let exact = "x".repeat(4083);
        let expected = format!("{exact}/canonical_id");
        assert_eq!(
            append_fixed_object_pointer(&exact, "canonical_id").as_deref(),
            Some(expected.as_str())
        );
        assert!(append_fixed_object_pointer(&format!("{exact}/canonical_id"), "schema").is_none());
    }

    #[test]
    fn public_artifact_pointer_budget_failures_distinguish_object_and_array_paths() {
        let object_parent = format!("/{}", "x".repeat(4092));
        let object_failure = validate_public_json_for_build(
            PublicArtifactName::Cases,
            &json!({"schema": "fixture"}),
            &object_parent,
            Some(7),
        )
        .unwrap_err();
        let object_artifact = build_public_artifact_build_diagnostic_artifact(
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            &object_failure,
        )
        .unwrap();
        assert_eq!(
            object_artifact["failure"]["reason"],
            "object_pointer_budget_exceeded"
        );
        assert_eq!(
            object_artifact["failure"]["parent_json_pointer"],
            object_parent
        );

        let array_parent = format!("/{}", "x".repeat(4095));
        let array_failure = validate_public_json_for_build(
            PublicArtifactName::Cases,
            &json!([null]),
            &array_parent,
            Some(8),
        )
        .unwrap_err();
        let array_artifact = build_public_artifact_build_diagnostic_artifact(
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            &array_failure,
        )
        .unwrap();
        assert_eq!(
            array_artifact["failure"]["reason"],
            "array_pointer_budget_exceeded"
        );
        assert_eq!(
            array_artifact["failure"]["parent_json_pointer"],
            array_parent
        );
    }

    #[test]
    fn public_artifact_pointer_admits_numeric_segments_only_from_arrays() {
        assert!(append_fixed_object_pointer("", "7").is_none());
        assert_eq!(append_array_index_pointer("", 7).as_deref(), Some("/7"));
        let mut bundle = fixture_bundle();
        bundle.cases = json!({"7": "/Users/albert/private"});
        let failure = validate_public_bundle_for_build(&bundle).unwrap_err();
        let artifact = build_public_artifact_build_diagnostic_artifact(
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            &failure,
        )
        .unwrap();
        assert_eq!(
            artifact["failure"]["reason"],
            "object_field_not_in_fixed_vocabulary"
        );
        assert_eq!(artifact["failure"]["parent_json_pointer"], "");
        assert_eq!(artifact["failure"]["field_name"]["utf8_byte_length"], 1);
    }

    #[test]
    fn public_artifact_path_classification_covers_each_supported_absolute_form() {
        for path in [
            "/Users/albert/private",
            "C:\\\\private",
            "\\\\server\\share\\private",
            "\\\\?\\C:\\\\private",
        ] {
            assert!(absolute_path(path), "{path}");
        }
    }

    #[test]
    fn fixed_pointer_vocabulary_accepts_the_full_closed_summary() {
        let (report, corpus, thresholds) =
            super::super::thresholds::tests::accepted_fixture::values();
        let summary = QualificationSummaryV1::from_json(report).unwrap();
        let corpus = CorpusV1::from_json(corpus).unwrap();
        let thresholds = ThresholdsV1::from_json(thresholds).unwrap();

        summary
            .validate_against_inputs(&corpus, &thresholds)
            .unwrap();
        for value in [
            serde_json::to_value(&summary.environment).unwrap(),
            serde_json::to_value(&summary.inventory).unwrap(),
            serde_json::to_value(&summary.trails).unwrap(),
            serde_json::to_value(&summary.cases).unwrap(),
            serde_json::to_value(&summary.failure_funnel).unwrap(),
            serde_json::to_value(&summary).unwrap(),
        ] {
            assert_fixed_object_keys(&value);
        }
        build_public_artifacts(&summary, &corpus, &thresholds)
            .expect("every typed report field has a fixed pointer segment");
    }

    #[test]
    fn fixed_pointer_vocabulary_exactly_matches_closed_public_schema_properties() {
        let mut schema_fields = BTreeSet::new();
        collect_schema_property_names(
            &serde_json::to_value(schemars::schema_for!(QualificationSummaryV1)).unwrap(),
            &mut schema_fields,
        );
        collect_schema_property_names(
            &serde_json::to_value(schemars::schema_for!(ActivationDecisionReportV1)).unwrap(),
            &mut schema_fields,
        );
        let vocabulary_fields = fixed_json_pointer_segments().collect::<Vec<_>>();
        assert!(
            vocabulary_fields.windows(2).all(|pair| pair[0] < pair[1]),
            "fixed vocabulary must be unique and sorted"
        );
        let vocabulary = vocabulary_fields
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();

        for field in [
            "after_step_count",
            "basis",
            "connected_receipts",
            "enumeration_receipt_id",
            "error",
            "extractor_capability_receipt_id",
            "failure",
            "gate",
            "histogram",
            "maximum_bytes",
            "maximum_candidate_edges",
            "message",
            "mismatches",
            "observed_candidate_edges_at_least",
            "prohibition_index",
            "projection",
            "reasons",
            "observed",
            "required",
            "evidence",
        ] {
            assert!(schema_fields.contains(field), "schema omitted {field}");
            assert!(vocabulary.contains(field), "vocabulary omitted {field}");
        }
        assert_eq!(vocabulary, schema_fields);
    }

    fn collect_schema_property_names(value: &Value, fields: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                    fields.extend(properties.keys().cloned());
                }
                for child in object.values() {
                    collect_schema_property_names(child, fields);
                }
            }
            Value::Array(values) => {
                for child in values {
                    collect_schema_property_names(child, fields);
                }
            }
            _ => {}
        }
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn builder_accepts_a_legitimate_transport_error_variant() {
        let (mut report, corpus, thresholds) =
            super::super::thresholds::tests::accepted_fixture::values();
        report["cases"][0]["transport"] = json!({
            "kind": "error",
            "error": {
                "kind": "result_exceeds_budget",
                "maximum_bytes": 65_536,
                "actual_bytes": 65_537,
            },
        });
        super::super::thresholds::tests::refresh_results_digest(&mut report);
        let summary = QualificationSummaryV1::from_json(report).unwrap();
        let corpus = CorpusV1::from_json(corpus).unwrap();
        let thresholds = ThresholdsV1::from_json(thresholds).unwrap();
        summary
            .validate_against_inputs(&corpus, &thresholds)
            .unwrap();

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join(&summary.qualification_id);
        let reservation = reserve_case_diagnostic(root.path(), &summary.qualification_id).unwrap();
        build_and_publish(
            &destination,
            &summary,
            &corpus,
            &thresholds,
            &PublicLeakPolicy::default(),
            PublicArtifactDiagnosticContext::new(
                &reservation,
                &summary.qualification_id,
                &summary.provenance.source_commit,
                &summary.provenance.source_tree,
            ),
        )
        .expect("every field of a legitimate closed transport error is publishable");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 8);
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn builder_readback_verifier_preserves_unsafe_graph_ids_exactly() {
        let (report, corpus, thresholds) =
            super::super::thresholds::tests::accepted_fixture::values();
        let path_files =
            super::super::thresholds::tests::accepted_fixture::oracle_path_file_values()
                .into_iter()
                .map(CohortPathFileV1::from_json)
                .collect::<Result<Vec<_>>>()
                .unwrap();
        let mut parsed = QualificationSummaryV1::from_json(report).unwrap();
        let case = &mut parsed.cases[0];

        let selector_node_ids = (0..=case.attempted_step_count)
            .map(|index| i64::MIN + 100 + i64::from(index))
            .collect::<Vec<_>>();
        for (selector, node_id) in case
            .proof_trace
            .selectors
            .iter_mut()
            .zip(selector_node_ids.iter().copied())
        {
            let super::super::contracts::SelectorGateOutcomeV1::Resolved { node_id: selected } =
                &mut selector.outcome
            else {
                panic!("accepted fixture selector")
            };
            *selected = node_id;
        }

        let mut edge_by_receipt = BTreeMap::new();
        for step_index in 0..case.attempted_step_count {
            let edge_id = 9_007_199_254_740_993_i64 + i64::from(step_index);
            let step = case
                .proof_trace
                .steps
                .iter_mut()
                .find(|step| step.step_index == u64::from(step_index))
                .unwrap();
            step.candidate_edge_ids = vec![edge_id];
            step.outcome = StepQualificationOutcomeV1::Admitted {
                edge_ids: vec![edge_id],
            };

            let receipt = case
                .receipt_evidence
                .observed_receipts
                .iter_mut()
                .find(|receipt| receipt.step_index == step_index)
                .unwrap();
            receipt.edge_id = edge_id;
            receipt.source.pinned.node_id = selector_node_ids[usize::from(step_index)].to_string();
            receipt.target.pinned.node_id =
                selector_node_ids[usize::from(step_index) + 1].to_string();
            receipt.containment.file_node_id = i64::MAX - 100;
            receipt.containment.owner_node_id = selector_node_ids[usize::from(step_index)];
            edge_by_receipt.insert(receipt.receipt_id.clone(), edge_id);
        }
        for reference in &mut case.product_disposition.authoritative_receipts {
            reference.edge_id = edge_by_receipt[&reference.receipt_id];
        }
        let ActualProductResultV1::ContractProven { receipts, .. } =
            &mut case.product_disposition.actual
        else {
            panic!("accepted fixture disposition")
        };
        for reference in receipts {
            reference.edge_id = edge_by_receipt[&reference.receipt_id].to_string();
        }

        let corpus = CorpusV1::from_json(corpus).unwrap();
        let thresholds = ThresholdsV1::from_json(thresholds).unwrap();
        let summary = build_summary(
            QualificationReportInputV1 {
                qualification_id: parsed.qualification_id,
                source_commit: parsed.provenance.source_commit,
                source_tree: parsed.provenance.source_tree,
                environment: parsed.environment,
                inventory: parsed.inventory,
                trails: parsed.trails,
                cases: parsed.cases,
                failure_funnel: parsed.failure_funnel,
            },
            &corpus,
            &thresholds,
        )
        .unwrap();

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join(&summary.qualification_id);
        let reservation = reserve_case_diagnostic(root.path(), &summary.qualification_id).unwrap();
        build_and_publish(
            &destination,
            &summary,
            &corpus,
            &thresholds,
            &PublicLeakPolicy::default(),
            PublicArtifactDiagnosticContext::new(
                &reservation,
                &summary.qualification_id,
                &summary.provenance.source_commit,
                &summary.provenance.source_tree,
            ),
        )
        .unwrap();

        verify_published(
            &destination,
            &corpus,
            &thresholds,
            &path_files,
            &PublicLeakPolicy::default(),
        )
        .expect("split readback must preserve every exact graph ID");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 8);
    }

    fn assert_fixed_object_keys(value: &Value) {
        match value {
            Value::Object(object) => {
                for (field, child) in object {
                    assert!(
                        fixed_json_pointer_segment(field).is_some(),
                        "unlisted fixture field: {field}"
                    );
                    assert_fixed_object_keys(child);
                }
            }
            Value::Array(values) => {
                for child in values {
                    assert_fixed_object_keys(child);
                }
            }
            _ => {}
        }
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn task6_receipt_paths_are_bound_before_public_artifact_publication() {
        use codestory_agent::proof_qualification_support::{
            CallableContainmentEvidence, IndexedCallEdgeReceipt, IndexedLineWindow,
            PinnedNodeIdentity, ReceiptRef, ResolvedNodeIdentity,
        };
        use codestory_contracts::graph::NodeId;

        const SOURCE_CANONICAL_ID: &str = "/Users/private/worktree/src/caller.rs::caller";
        const TARGET_CANONICAL_ID: &str = r"C:\private\worktree\src\target.rs::target";

        let (mut report, corpus, thresholds) =
            super::super::thresholds::tests::accepted_fixture::values();
        let path_files =
            super::super::thresholds::tests::accepted_fixture::oracle_path_file_values()
                .into_iter()
                .map(CohortPathFileV1::from_json)
                .collect::<Result<Vec<_>>>()
                .unwrap();
        let oracle = &path_files[0].paths[0].oracle_steps[0];
        let prior: super::super::contracts::ObservedReceiptV1 = serde_json::from_value(
            report["cases"][0]["receipt_evidence"]["observed_receipts"][0].clone(),
        )
        .unwrap();
        let resolved = |identity: &super::super::contracts::ResolvedNodeIdentityV1,
                        canonical_id: &str| {
            ResolvedNodeIdentity {
                pinned: PinnedNodeIdentity {
                    project_id: identity.pinned.project_id.clone(),
                    core_generation_id: identity.pinned.core_generation_id.clone(),
                    core_run_id: identity.pinned.core_run_id.clone(),
                    node_id: identity.pinned.node_id.clone(),
                },
                canonical_id: canonical_id.to_owned(),
                qualified_name: identity.qualified_name.clone(),
                project_file_components: identity.project_file_components.clone(),
            }
        };
        let task6_receipt = IndexedCallEdgeReceipt {
            receipt: ReceiptRef {
                receipt_id: prior.receipt_id.clone(),
                edge_id: prior.edge_id.to_string(),
            },
            source: resolved(&prior.source, SOURCE_CANONICAL_ID),
            target: resolved(&prior.target, TARGET_CANONICAL_ID),
            resolution_fact_id: "a".repeat(64),
            resolution_evidence_sha256: "b".repeat(64),
            exact_callsite_start_byte: 0,
            callsite_identity: prior.callsite_identity.clone(),
            column_or_ordinal: 0,
            containment: CallableContainmentEvidence {
                file_node_id: NodeId(prior.containment.file_node_id),
                owner_node_id: NodeId(prior.containment.owner_node_id),
                start_line: prior.containment.start_line,
                end_line: prior.containment.end_line,
            },
            line_window: IndexedLineWindow {
                kind: "indexed_line_v1",
                project_file_components: prior.line_window.project_file_components.clone(),
                indexed_sha256: prior.line_window.indexed_sha256.clone(),
                observed_sha256: prior.line_window.observed_sha256.clone(),
                anchor_line: prior.callsite_line,
                byte_start: usize::try_from(prior.line_window.byte_start).unwrap(),
                byte_end: usize::try_from(prior.line_window.byte_end).unwrap(),
                text: prior.line_window.text.clone(),
            },
        };
        let observed =
            super::super::contracts::observed_receipt_from_task6(0, &task6_receipt, oracle)
                .unwrap();
        report["cases"][0]["receipt_evidence"]["observed_receipts"][0] =
            serde_json::to_value(observed).unwrap();
        super::super::thresholds::tests::refresh_results_digest(&mut report);

        let parsed = QualificationSummaryV1::from_json(report).unwrap();
        let corpus = CorpusV1::from_json(corpus).unwrap();
        let thresholds = ThresholdsV1::from_json(thresholds).unwrap();
        let summary = build_summary(
            QualificationReportInputV1 {
                qualification_id: parsed.qualification_id,
                source_commit: parsed.provenance.source_commit,
                source_tree: parsed.provenance.source_tree,
                environment: parsed.environment,
                inventory: parsed.inventory,
                trails: parsed.trails,
                cases: parsed.cases,
                failure_funnel: parsed.failure_funnel,
            },
            &corpus,
            &thresholds,
        )
        .unwrap();
        summary
            .validate_against_inputs(&corpus, &thresholds)
            .unwrap();

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join(&summary.qualification_id);
        let reservation = reserve_case_diagnostic(root.path(), &summary.qualification_id).unwrap();
        build_and_publish(
            &destination,
            &summary,
            &corpus,
            &thresholds,
            &PublicLeakPolicy::default(),
            PublicArtifactDiagnosticContext::new(
                &reservation,
                &summary.qualification_id,
                &summary.provenance.source_commit,
                &summary.provenance.source_tree,
            ),
        )
        .unwrap();

        let cases = fs::read_to_string(destination.join("cases.json")).unwrap();
        let summary = fs::read_to_string(destination.join("summary.json")).unwrap();
        for raw in [SOURCE_CANONICAL_ID, TARGET_CANONICAL_ID] {
            assert!(!cases.contains(raw));
            assert!(!summary.contains(raw));
        }
        verify_published(
            &destination,
            &corpus,
            &thresholds,
            &path_files,
            &PublicLeakPolicy::default(),
        )
        .unwrap();
        assert_eq!(fs::read_dir(destination).unwrap().count(), 8);
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn builder_path_writes_a_non_receipt_path_failure_diagnostic() {
        let (mut report, corpus, thresholds) =
            super::super::thresholds::tests::accepted_fixture::values();
        report["environment"]["build"]["rustc_vv"] = json!(format!(
            "/private/compiler\n{}",
            report["environment"]["build"]["rustc_vv"].as_str().unwrap()
        ));
        super::super::thresholds::tests::refresh_results_digest(&mut report);
        let summary = QualificationSummaryV1::from_json(report).unwrap();
        let corpus = CorpusV1::from_json(corpus).unwrap();
        let thresholds = ThresholdsV1::from_json(thresholds).unwrap();
        summary
            .validate_against_inputs(&corpus, &thresholds)
            .unwrap();

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join(&summary.qualification_id);
        let reservation = reserve_case_diagnostic(root.path(), &summary.qualification_id).unwrap();
        let error = build_and_publish(
            &destination,
            &summary,
            &corpus,
            &thresholds,
            &PublicLeakPolicy::default(),
            PublicArtifactDiagnosticContext::new(
                &reservation,
                &summary.qualification_id,
                &summary.provenance.source_commit,
                &summary.provenance.source_tree,
            ),
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            "proof_availability_public_artifact_build_failed"
        );
        assert!(!destination.exists());
        let diagnostic: Value = serde_json::from_slice(
            &fs::read(
                reservation
                    .path()
                    .join(PrivateDiagnosticFileKind::PublicArtifactBuildV1.file_name()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(diagnostic["failure"]["reason"], "path_leak");
        assert_eq!(diagnostic["failure"]["artifact"], "environment.json");
        assert!(diagnostic["failure"]["case_ordinal"].is_null());
        assert_eq!(diagnostic["failure"]["json_pointer"], "/build/rustc_vv");
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn public_artifact_diagnostic_uses_the_fixed_owner_private_file_kind() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let reservation =
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222").unwrap();
        let failure = PublicArtifactBuildFailure::path_leak(
            PublicArtifactBuildStage::BundleValidation,
            PublicArtifactName::Cases,
            Some(0),
            "/0/canonical_id",
            "/private",
        );
        write_public_artifact_build_diagnostic(
            &reservation,
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            &failure,
        )
        .unwrap();
        let target = reservation
            .path()
            .join(PrivateDiagnosticFileKind::PublicArtifactBuildV1.file_name());
        let bytes = fs::read(&target).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            write_public_artifact_build_diagnostic(
                &reservation,
                "20260821T120000Z-222222222222",
                &"a".repeat(40),
                &"b".repeat(40),
                &failure,
            )
            .is_err()
        );
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn case_diagnostic_reservation_is_private_and_no_replace() {
        let root = tempfile::tempdir().unwrap();
        let qualification_id = "20260821T120000Z-222222222222";
        let reservation = reserve_case_diagnostic(root.path(), qualification_id).unwrap();
        assert!(reservation.path().is_dir());
        let reservation_path = reservation.path().to_path_buf();
        assert_eq!(
            reserve_case_diagnostic(root.path(), qualification_id)
                .unwrap_err()
                .to_string(),
            "proof_availability_case_diagnostic_exists"
        );
        drop(reservation);
        assert!(
            reservation_path.is_dir(),
            "an empty reservation is single-use"
        );
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn case_diagnostic_reservation_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let reservation =
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222").unwrap();
        assert_eq!(
            fs::metadata(reservation.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        drop(reservation);
    }

    #[test]
    fn case_diagnostic_redacts_text_and_rejects_unsafe_values() {
        let mut value = json!({
            "receipt": {"text": "a\nλ", "other": [ {"text": "b"} ]}
        });
        let mut commitments = Vec::new();
        redact_text_values(&mut value, "", &mut commitments).unwrap();
        assert_eq!(value, json!({"receipt": {"other": [{}]}}));
        assert_eq!(
            commitments,
            vec![
                json!({
                    "json_pointer": "/receipt/text",
                    "utf8_byte_length": 4,
                    "sha256": domain_sha256(b"codestory.proof-availability.removed-text.v1\\0", "a\nλ".as_bytes()),
                }),
                json!({
                    "json_pointer": "/receipt/other/0/text",
                    "utf8_byte_length": 1,
                    "sha256": domain_sha256(b"codestory.proof-availability.removed-text.v1\\0", b"b"),
                }),
            ]
        );
        for unsafe_value in [
            json!({"source_text": "private"}),
            json!({"api_token": "private"}),
            json!({"location": "/Users/albert/private"}),
            json!({"location": "C:\\\\private"}),
        ] {
            assert!(validate_private_diagnostic_value(&unsafe_value, &[]).is_err());
        }
        let pointer_artifact = json!({
            "removed_text_commitments": [{
                "json_pointer": "/receipt/source_line/text",
                "utf8_byte_length": 1,
                "sha256": "a"
            }]
        });
        validate_private_diagnostic_value(&pointer_artifact, &[])
            .expect("the typed RFC6901 field may begin with a slash");
        let invalid_pointer = json!({
            "removed_text_commitments": [{"json_pointer": "/bad~2pointer"}]
        });
        assert!(validate_private_diagnostic_value(&invalid_pointer, &[]).is_err());
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn case_diagnostic_file_is_newline_terminated_and_no_clobber() {
        let root = tempfile::tempdir().unwrap();
        let reservation =
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222").unwrap();
        let target = reservation
            .path()
            .join(PrivateDiagnosticFileKind::InvalidCaseV1.file_name());
        write_private_diagnostic_file(
            &reservation,
            PrivateDiagnosticFileKind::InvalidCaseV1,
            b"{}\n",
        )
        .unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"{}\n");
        assert!(
            write_private_diagnostic_file(
                &reservation,
                PrivateDiagnosticFileKind::InvalidCaseV1,
                b"changed\n",
            )
            .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"{}\n");
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn complete_private_artifact_redacts_real_receipt_text_before_handle_relative_write() {
        let root = tempfile::tempdir().unwrap();
        let reservation =
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222").unwrap();
        let artifact = build_invalid_case_diagnostic_artifact(
            "20260821T120000Z-222222222222",
            &"a".repeat(40),
            &"b".repeat(40),
            0,
            "case-1",
            "repository-1",
            json!({
                "receipt_evidence": {"observed_receipts": [{
                    "source_line": {"text": "call(target);\n"}
                }]}
            }),
            Value::Null,
            &[],
        )
        .unwrap();
        let bytes = canonical_json_file(&artifact).unwrap();
        write_private_diagnostic_file(
            &reservation,
            PrivateDiagnosticFileKind::InvalidCaseV1,
            &bytes,
        )
        .unwrap();
        let written: Value = serde_json::from_slice(
            &fs::read(
                reservation
                    .path()
                    .join(PrivateDiagnosticFileKind::InvalidCaseV1.file_name()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(written["schema"], CASE_DIAGNOSTIC_SCHEMA);
        assert_eq!(written["classification"], "non_evidence");
        assert!(
            written
                .pointer("/case/receipt_evidence/observed_receipts/0/source_line/text")
                .is_none()
        );
        assert_eq!(
            written["removed_text_commitments"][0]["json_pointer"],
            "/receipt_evidence/observed_receipts/0/source_line/text"
        );
        assert!(written["unredacted_case_sha256"].as_str().is_some());
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn reservation_handle_cannot_be_redirected_by_a_path_swap() {
        let root = tempfile::tempdir().unwrap();
        let reservation =
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222").unwrap();
        let original = root.path().join("held-directory");
        fs::rename(reservation.path(), &original).unwrap();
        fs::create_dir(reservation.path()).unwrap();
        assert!(!path_matches_held_directory(&reservation).unwrap());
        assert_eq!(
            ensure_discoverable_reservation(&reservation)
                .unwrap_err()
                .to_string(),
            "proof_availability_case_diagnostic_write_failed"
        );
        assert!(
            !original
                .join(PrivateDiagnosticFileKind::InvalidCaseV1.file_name())
                .exists()
        );
        assert!(
            !reservation
                .path()
                .join(PrivateDiagnosticFileKind::InvalidCaseV1.file_name())
                .exists()
        );
    }

    #[cfg(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    ))]
    #[test]
    fn nofollow_parent_and_replaced_reservation_paths_cannot_redirect_creation_or_writes() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let actual_parent = root.path().join("actual-parent");
        fs::create_dir(&actual_parent).unwrap();
        let swapped_parent = root.path().join("swapped-parent");
        symlink(&actual_parent, &swapped_parent).unwrap();
        assert_eq!(
            reserve_case_diagnostic(&swapped_parent, "20260821T120000Z-222222222222")
                .unwrap_err()
                .to_string(),
            "proof_availability_case_diagnostic_parent_invalid"
        );
        assert!(fs::read_dir(&actual_parent).unwrap().next().is_none());

        let reservation =
            reserve_case_diagnostic(&actual_parent, "20260821T120000Z-222222222222").unwrap();
        let held = root.path().join("held-reservation");
        fs::rename(reservation.path(), &held).unwrap();
        symlink(&actual_parent, reservation.path()).unwrap();
        assert!(ensure_discoverable_reservation(&reservation).is_err());
        assert!(
            !held
                .join(PrivateDiagnosticFileKind::InvalidCaseV1.file_name())
                .exists()
        );
        assert!(
            !actual_parent
                .join(PrivateDiagnosticFileKind::InvalidCaseV1.file_name())
                .exists()
        );
    }

    #[cfg(not(all(
        unix,
        any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos"
        )
    )))]
    #[test]
    fn unsupported_target_rejects_before_runner_or_artifact_creation() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222")
                .unwrap_err()
                .to_string(),
            "proof_availability_case_diagnostic_unsupported"
        );
    }

    #[test]
    fn publishing_is_whole_directory_no_replace_and_exactly_eight_files() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("result");
        publish_bundle(
            &destination,
            &fixture_bundle(),
            &PublicLeakPolicy::default(),
        )
        .unwrap();
        let mut names = fs::read_dir(&destination)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, PUBLIC_ARTIFACT_NAMES);
        let error = publish_bundle(
            &destination,
            &fixture_bundle(),
            &PublicLeakPolicy::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, "proof_availability_output_exists");
        assert!(error.staging.is_none());
        assert_eq!(error.destination, destination);
    }

    #[cfg(unix)]
    #[test]
    fn published_directory_and_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("result");
        publish_bundle(
            &destination,
            &fixture_bundle(),
            &PublicLeakPolicy::default(),
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in PUBLIC_ARTIFACT_NAMES {
            assert_eq!(
                fs::metadata(destination.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{name}"
            );
        }
    }

    #[test]
    fn a_publish_collision_preserves_owned_staging_for_recovery() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("result");
        let error = publish_bundle_with_hook(
            &destination,
            &fixture_bundle(),
            &PublicLeakPolicy::default(),
            || fs::create_dir(&destination),
        )
        .unwrap_err();
        assert_eq!(error.code, "proof_availability_publish_collision");
        assert!(destination.is_dir());
        let staging = error
            .staging
            .as_deref()
            .expect("typed recovery staging path");
        assert!(staging.is_dir());
        assert!(staging.join(OWNER_MARKER).is_file());
    }

    #[test]
    fn public_artifacts_reject_absolute_paths_and_secret_fields() {
        let root = tempfile::tempdir().unwrap();
        let mut path_leak = fixture_bundle();
        path_leak.summary = json!({"workspace":"/Users/albert/private"});
        assert_eq!(
            publish_bundle(
                &root.path().join("path-leak"),
                &path_leak,
                &PublicLeakPolicy::default(),
            )
            .unwrap_err()
            .code,
            "proof_availability_public_path_leak"
        );

        let mut secret = fixture_bundle();
        secret.cases = json!({"api_token":"do-not-publish"});
        assert_eq!(
            publish_bundle(
                &root.path().join("secret"),
                &secret,
                &PublicLeakPolicy::default(),
            )
            .unwrap_err()
            .code,
            "proof_availability_public_secret_leak"
        );
    }

    #[test]
    fn caller_supplied_private_needles_are_rejected_without_logging_values() {
        let root = tempfile::tempdir().unwrap();
        let mut bundle = fixture_bundle();
        bundle.findings = "# Findings\n\nprivate-database-location\n".to_owned();
        let policy = PublicLeakPolicy::new(["private-database-location"]);
        let error = publish_bundle(&root.path().join("result"), &bundle, &policy).unwrap_err();
        assert_eq!(error.code, "proof_availability_public_forbidden_value");
        assert!(!error.to_string().contains("private-database-location"));
    }

    #[test]
    fn findings_separate_measurements_inferences_thresholds_and_decision() {
        let document = FindingsDocumentV1 {
            qualification_id: "qualification-1".into(),
            source_commit: "a".repeat(40),
            source_tree: "b".repeat(40),
            binary_sha256: "c".repeat(64),
            corpus_sha256: "d".repeat(64),
            thresholds_sha256: "e".repeat(64),
            results_sha256: "f".repeat(64),
            positive_requests: 120,
            attempted_positive_steps: 312,
            exact_positive_steps: 250,
            contract_proven_cases: 60,
            negative_contract_proven: 0,
            authoritative_receipts: 250,
            exact_authoritative_receipts: 250,
            unclassified_positive_steps: 0,
            maximum_response_bytes: 32_768,
            cohort_full_proofs: vec![("a".into(), 10), ("b".into(), 20)],
            inventory: vec![InventoryReportV1 {
                repository_id: "a".into(),
                stored_call_rows: 10,
                effective_endpoint_rows: 10,
                exact_resolved_rows: 8,
                admitted_rows: 7,
                unresolved_placeholder_rows: 2,
            }],
            trails: vec![TrailReportV1 {
                repository_id: "a".into(),
                lengths: vec![super::super::contracts::TrailLengthCountsV1 {
                    length: 1,
                    effective_endpoint: 10,
                    exact_resolved: 8,
                    strictly_admitted: 7,
                }],
            }],
            thresholds_id: "thresholds-v1".into(),
            methodology_sha256: "1".repeat(64),
            hard_gate_summary: "false_proofs<=0; exact_receipts=true; certified_absence<=0; complete_funnel=true; complete_provenance=true; invalid<=0; over_cap<=0; transport_errors<=0; maximum_bytes<=65536; each_cohort=true; disposition_match=true".into(),
            roles: vec![FindingsRoleThresholdV1 {
                role: "automatic".into(),
                minimum_full_proofs: 96,
                minimum_full_proofs_per_cohort: 21,
                minimum_full_proof_wilson_lower_milli: 720,
                minimum_cohort_wilson_lower_milli: 500,
                minimum_positive_step_recall_milli: 900,
                minimum_full_or_useful_partial_milli: 950,
                minimum_actionable_exact_gap_milli: 950,
                maximum_unknown_p95_ms: 500,
                maximum_transport_p95_ms: 1_500,
                maximum_complete_response_p95_bytes: 32_768,
                maximum_unknown_response_p95_bytes: 16_384,
                maximum_response_bytes: 65_536,
            }],
            outcome: "public_exact_verifier".into(),
            automatic_thresholds_met: Some(true),
            failed_gates: vec![("stable.full_proofs".into(), "stable_threshold".into())],
        };
        let findings = render_findings_document(&document).unwrap();
        let section_positions = [
            "## Reproduced measurements",
            "## Inferences",
            "## Frozen thresholds",
            "## Decision",
        ]
        .map(|section| findings.find(section).expect(section));
        assert!(section_positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(findings.contains("| Exact positive steps | 250 / 312 |"));
        assert!(findings.contains("| `a` | 10 | 10 | 8 | 7 | 2 |"));
        assert!(findings.contains("| `a` | 1 | 10 | 8 | 7 |"));
        assert!(findings.contains("| automatic | 96 | 21 | 720 | 500 | 900 | 950 | 950 | 500 | 1500 | 32768 | 16384 | 65536 |"));
        assert!(findings.contains("The evaluator selected `public_exact_verifier`"));
        assert!(findings.contains("| `stable.full_proofs` | `stable_threshold` |"));
        assert!(findings.contains("### Provenance"));
        assert!(findings.contains("### Nonclaims"));
        assert!(findings.contains("does not prove runtime execution"));
    }
}
