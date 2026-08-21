use super::contracts::{
    ActivationDecisionV1, ActualProductResultV1, CaseReportV1, CohortPathFileV1, CorpusV1,
    EnvironmentReportV1, FailureFunnelReportV1, FunnelOutcomeV1, InventoryReportV1,
    ProductRefutationBasisV1, ProjectedReceiptReferenceV1, ProvenanceV1, QualificationSummaryV1,
    REPORT_SCHEMA, ReceiptOracleComparisonV1, StepQualificationOutcomeV1, ThresholdsV1,
    TrailReportV1, TransportEvidenceV1, canonical_corpus_sha256, canonical_thresholds_sha256,
    results_evidence_sha256,
};
use super::thresholds::evaluate_activation_decision;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

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
    let decision = evaluate_activation_decision(summary, corpus, thresholds, None)?;
    let findings = render_findings(summary, &decision)?;
    let bundle = PublicArtifactBundle {
        environment: serde_json::to_value(&summary.environment)?,
        inventory: serde_json::to_value(&summary.inventory)?,
        trails: serde_json::to_value(&summary.trails)?,
        cases: serde_json::to_value(&summary.cases)?,
        failure_funnel: serde_json::to_value(&summary.failure_funnel)?,
        summary: serde_json::to_value(summary)?,
        decision: serde_json::to_value(decision)?,
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
    let decision: ActivationDecisionV1 = serde_json::from_value(decision_value)?;
    decision.validate()?;
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

fn render_findings(
    summary: &QualificationSummaryV1,
    decision: &ActivationDecisionV1,
) -> Result<String> {
    let outcome = serde_json::to_value(&decision.outcome)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_decision_outcome_invalid"))?;
    let metrics = summary.receipt_metrics()?;
    let mut text = format!(
        "# Proof availability findings\n\nQualification: `{}`\n\nDecision: `{outcome}`\n\nExact positive steps: {}/312\n\nFailed gates: {}\n",
        summary.qualification_id,
        metrics.exact_oracle_step_count,
        decision.failed_gates.len()
    );
    for gate in &decision.failed_gates {
        let kind = serde_json::to_value(&gate.kind)?
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_gate_kind_invalid"))?;
        text.push_str(&format!("\n- `{}` ({kind})\n", gate.gate_id));
    }
    Ok(text)
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
    fn canonical_json_is_compact_sorted_and_newline_terminated() {
        assert_eq!(
            canonical_json_file(&json!({"z":2,"a":1})).unwrap(),
            b"{\"a\":1,\"z\":2}\n"
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
}
