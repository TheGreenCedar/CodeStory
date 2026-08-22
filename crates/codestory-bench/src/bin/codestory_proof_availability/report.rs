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
const CASE_DIAGNOSTIC_FILE: &str = "invalid-case-v1.json";
const MAX_CASE_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

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
    let metadata = fs::symlink_metadata(output_parent)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_parent_invalid"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("proof_availability_case_diagnostic_parent_invalid")
    }
    let path = output_parent.join(format!(
        ".codestory-proof-availability-case-diagnostic-{qualification_id}"
    ));
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(&path)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_exists"))?;
    let directory = File::open(&path)
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_create_failed"))?;
    let metadata = directory
        .metadata()
        .map_err(|_| anyhow::anyhow!("proof_availability_case_diagnostic_create_failed"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("proof_availability_case_diagnostic_create_failed")
    }
    Ok(CaseDiagnosticReservation {
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
    })
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
    let _path_still_names_reservation = path_matches_held_directory(reservation);
    if !reservation_handle_is_directory(reservation)? {
        bail!("proof_availability_case_diagnostic_write_failed")
    }
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
    write_private_diagnostic_file(reservation, &bytes)
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
    pointer.starts_with("/removed_text_commitments/") && pointer.ends_with("/json_pointer")
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
    bytes: &[u8],
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let sequence = CASE_DIAGNOSTIC_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary_name = format!(".invalid-case-v1-{}-{sequence}", std::process::id());
    let temporary_c = CString::new(temporary_name.as_bytes())
        .map_err(|_| std::io::Error::other("invalid temporary name"))?;
    let final_c = CString::new(CASE_DIAGNOSTIC_FILE)
        .map_err(|_| std::io::Error::other("invalid final name"))?;
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
) -> Result<PublicArtifactBundle> {
    summary.validate_against_inputs(corpus, thresholds)?;
    let observations = derive_observations(summary, thresholds)?;
    let observations_sha256 = canonical_observations_sha256(
        &summary.provenance.results_sha256,
        &summary.provenance.thresholds_sha256,
        &observations,
    )?;
    let decision = evaluate_activation_decision(summary, corpus, thresholds)?;
    let decision_report = ActivationDecisionReportV1 {
        schema: DECISION_REPORT_SCHEMA.to_owned(),
        results_sha256: summary.provenance.results_sha256.clone(),
        thresholds_sha256: summary.provenance.thresholds_sha256.clone(),
        observations,
        observations_sha256,
        decision,
    };
    decision_report.validate()?;
    let findings = render_findings(summary, thresholds, &decision_report)?;
    let bundle = PublicArtifactBundle {
        environment: serde_json::to_value(&summary.environment)?,
        inventory: serde_json::to_value(&summary.inventory)?,
        trails: serde_json::to_value(&summary.trails)?,
        cases: serde_json::to_value(&summary.cases)?,
        failure_funnel: serde_json::to_value(&summary.failure_funnel)?,
        summary: serde_json::to_value(summary)?,
        decision: serde_json::to_value(decision_report)?,
        findings,
    };
    validate_public_bundle(&bundle, &PublicLeakPolicy::default())?;
    Ok(bundle)
}

pub(crate) fn build_and_publish(
    destination: &Path,
    summary: &QualificationSummaryV1,
    corpus: &CorpusV1,
    thresholds: &ThresholdsV1,
    leak_policy: &PublicLeakPolicy,
) -> std::result::Result<(), ReportPublishError> {
    require_result_directory_identity(destination, &summary.qualification_id).map_err(|_| {
        ReportPublishError::before_staging(
            "proof_availability_result_directory_qualification_id_mismatch",
            destination,
        )
    })?;
    let bundle = build_public_artifacts(summary, corpus, thresholds).map_err(|_| {
        ReportPublishError::before_staging(
            "proof_availability_public_artifact_build_failed",
            destination,
        )
    })?;
    publish_bundle(destination, &bundle, leak_policy)
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
    use std::fs;

    fn fixture_bundle() -> PublicArtifactBundle {
        PublicArtifactBundle {
            environment: json!({"z": 2, "a": 1}),
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

    #[cfg(unix)]
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

    #[test]
    fn case_diagnostic_file_is_newline_terminated_and_no_clobber() {
        let root = tempfile::tempdir().unwrap();
        let reservation =
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222").unwrap();
        let target = reservation.path().join(CASE_DIAGNOSTIC_FILE);
        write_private_diagnostic_file(&reservation, b"{}\n").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"{}\n");
        assert!(write_private_diagnostic_file(&reservation, b"changed\n").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"{}\n");
    }

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
        write_private_diagnostic_file(&reservation, &bytes).unwrap();
        let written: Value = serde_json::from_slice(
            &fs::read(reservation.path().join(CASE_DIAGNOSTIC_FILE)).unwrap(),
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

    #[cfg(unix)]
    #[test]
    fn reservation_handle_cannot_be_redirected_by_a_path_swap() {
        let root = tempfile::tempdir().unwrap();
        let reservation =
            reserve_case_diagnostic(root.path(), "20260821T120000Z-222222222222").unwrap();
        let original = root.path().join("held-directory");
        fs::rename(reservation.path(), &original).unwrap();
        fs::create_dir(reservation.path()).unwrap();
        assert!(!path_matches_held_directory(&reservation).unwrap());
        write_private_diagnostic_file(&reservation, b"{}\n").unwrap();
        assert!(original.join(CASE_DIAGNOSTIC_FILE).is_file());
        assert!(!reservation.path().join(CASE_DIAGNOSTIC_FILE).exists());
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
