//! Private, package-bound producer for per-user embedding qualification data.
//!
//! This entrypoint emits raw product-path observations only. The release
//! harness owns cross-process correlation, assertion evaluation, metrics, and
//! retained evidence.

use crate::args::InternalEmbeddingQualificationCommand;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    PER_USER_EMBEDDING_CONSTANT_SET_FROZEN, PER_USER_EMBEDDING_CONSTANT_SET_SHA256,
    PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256, PER_USER_EMBEDDING_PROTOCOL_SHA256,
    SidecarRuntimeConfig, SidecarRuntimeOverrides,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod scenarios;

const QUALIFICATION_DIR_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIR";
const QUALIFICATION_NONCE_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_NONCE";
const DIAGNOSTIC_SCENARIO_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIAGNOSTIC_SCENARIO";
const ARCHIVE_SHA256_ENV: &str = "CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256";
const MANIFEST_PATH_ENV: &str = "CODESTORY_PLUGIN_CLI_MANIFEST_PATH";
const NATIVE_MANIFEST_FILE: &str = "codestory-native-manifest.json";
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

const REQUIRED_SCENARIOS: &[&str] = &[
    "client_death",
    "cold_race",
    "frozen_owner",
    "incompatible_owner",
    "mixed_queue",
    "server_crash",
    "true_idle_respawn",
    "worker_stall",
];

const REQUIRED_METRICS: &[&str] = &[
    "backend_observed_accelerator_residency",
    "bulk_documents_per_second",
    "bulk_tokens_per_second",
    "busy_retry_usefulness",
    "cold_first_vector",
    "existing_owner_connect",
    "first_product_ready",
    "retrieval_quality",
    "spawn_convergence",
    "total_codestory_process_memory",
    "true_idle_exit",
    "warm_bulk_ipc",
    "warm_query_ipc",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct QualificationSuiteRequest {
    schema_version: u32,
    qualification_nonce: String,
    qualification_nonce_sha256: String,
    proof_tier: String,
    source: QualificationSource,
    package: QualificationPackage,
    contracts: QualificationContracts,
    runtime: QualificationRuntime,
    projects: Vec<PathBuf>,
    required_scenarios: Vec<String>,
    required_metrics: Vec<String>,
    output_directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct QualificationSource {
    commit: String,
    tree: String,
    tracked_dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct QualificationPackage {
    archive_sha256: String,
    executable_sha256: String,
    asset_target: String,
    release_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct QualificationContracts {
    protocol_sha256: String,
    constant_set_sha256: String,
    measurement_protocol_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct QualificationRuntime {
    engine_policy: String,
    expected_backend: String,
    offline: bool,
    matrix_cell_id: String,
    cache_state: String,
    residency_state: String,
}

#[derive(Debug, Serialize)]
struct QualificationRawOutput {
    schema_version: u32,
    tier: String,
    source: QualificationSource,
    package: QualificationPackage,
    contracts: QualificationContracts,
    runtime: QualificationRuntime,
    request_sha256: String,
    measurements: QualificationMeasurementsSummary,
    scenarios: BTreeMap<String, QualificationScenarioSummary>,
}

#[derive(Debug, Serialize)]
struct QualificationMeasurementsSummary {
    artifact: String,
    metric_count: u64,
    sample_count: u64,
}

#[derive(Debug, Serialize)]
struct QualificationScenarioSummary {
    artifact: String,
    process_count: u64,
    control_event_count: u64,
    process_observation_count: u64,
    observation_count: u64,
    event_count: u64,
}

struct QualificationValidation {
    output_directory: PathBuf,
    output_path: PathBuf,
    nonce_sha256: String,
    request_sha256: String,
}

pub(crate) fn run_internal_embedding_qualification(
    command: InternalEmbeddingQualificationCommand,
) -> Result<()> {
    let request_bytes = read_private_request(&command.request)?;
    let request: QualificationSuiteRequest =
        serde_json::from_slice(&request_bytes).context("parse embedding qualification request")?;
    let validation = validate_request(&request, &request_bytes, &command.request, &command.output)?;
    let defaults = crate::sidecar_runtime::process_defaults();
    let overrides = SidecarRuntimeOverrides::default();
    let runtimes = request
        .projects
        .iter()
        .map(|project| {
            crate::sidecar_runtime::for_project_auto_with_process_defaults(
                project, &defaults, &overrides,
            )
        })
        .collect::<Vec<_>>();

    if diagnostic_worker_stall_enabled()? {
        let artifact = scenarios::run_scenario(scenarios::ScenarioContext {
            scenario: "worker_stall",
            runtimes: &runtimes,
            projects: &request.projects,
            primary_index: 0,
            contracts: &request.contracts,
            qualification_runtime: &request.runtime,
            output_directory: &validation.output_directory,
            nonce_sha256: &validation.nonce_sha256,
        })
        .context("run diagnostic embedding qualification scenario worker_stall")?;
        return write_atomic_json(&validation.output_path, &artifact)
            .context("write diagnostic worker_stall artifact");
    }

    let measurements_artifact_name = "measurements.raw.json";
    let measurements_artifact = scenarios::run_measurements(scenarios::ScenarioContext {
        scenario: "measurements",
        runtimes: &runtimes,
        projects: &request.projects,
        primary_index: 0,
        contracts: &request.contracts,
        qualification_runtime: &request.runtime,
        output_directory: &validation.output_directory,
        nonce_sha256: &validation.nonce_sha256,
    })
    .context("run embedding qualification measurements")?;
    let measurements = measurements_artifact.summary(measurements_artifact_name.into());
    write_atomic_json(
        &validation.output_directory.join(measurements_artifact_name),
        &measurements_artifact,
    )
    .context("write raw embedding qualification measurements")?;

    let mut scenarios = BTreeMap::new();
    for (index, scenario) in REQUIRED_SCENARIOS.iter().enumerate() {
        let artifact = scenarios::run_scenario(scenarios::ScenarioContext {
            scenario,
            runtimes: &runtimes,
            projects: &request.projects,
            primary_index: index % runtimes.len(),
            contracts: &request.contracts,
            qualification_runtime: &request.runtime,
            output_directory: &validation.output_directory,
            nonce_sha256: &validation.nonce_sha256,
        })
        .with_context(|| format!("run named embedding qualification scenario {scenario}"))?;
        let artifact_name = format!("{scenario}.raw.json");
        write_atomic_json(&validation.output_directory.join(&artifact_name), &artifact)
            .with_context(|| format!("write raw qualification artifact {artifact_name}"))?;
        let summary = artifact.summary(artifact_name);
        scenarios.insert((*scenario).to_string(), summary);
    }

    let output = QualificationRawOutput {
        schema_version: 2,
        tier: request.proof_tier,
        source: request.source,
        package: request.package,
        contracts: request.contracts,
        runtime: request.runtime,
        request_sha256: validation.request_sha256,
        measurements,
        scenarios,
    };
    write_atomic_json(&validation.output_path, &output).context("write raw qualification output")
}

fn diagnostic_worker_stall_enabled() -> Result<bool> {
    match std::env::var_os(DIAGNOSTIC_SCENARIO_ENV) {
        None => Ok(false),
        Some(value) if value == "worker_stall" => Ok(true),
        Some(_) => bail!("embedding_qualification_diagnostic_scenario_invalid"),
    }
}

pub(crate) fn run_internal_embedding_qualification_worker(
    command: InternalEmbeddingQualificationCommand,
) -> Result<()> {
    scenarios::run_worker(command)
}

fn validate_request(
    request: &QualificationSuiteRequest,
    request_bytes: &[u8],
    request_path: &Path,
    output_path: &Path,
) -> Result<QualificationValidation> {
    if request.schema_version != 1 {
        bail!("embedding_qualification_schema_invalid");
    }
    let qualification_directory = required_absolute_directory(QUALIFICATION_DIR_ENV)?;
    validate_private_directory(&qualification_directory)?;
    let nonce = std::env::var(QUALIFICATION_NONCE_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_closed"))?;
    let (qualification_directory, output_path, nonce_sha256) = validate_gate_and_paths(
        request,
        qualification_directory,
        &nonce,
        request_path,
        output_path,
    )?;
    validate_exact_string_list(&request.required_scenarios, REQUIRED_SCENARIOS, "scenarios")?;
    validate_exact_string_list(&request.required_metrics, REQUIRED_METRICS, "metrics")?;
    validate_source(&request.source)?;
    validate_package_and_contracts(request)?;
    validate_runtime(&request.runtime)?;
    validate_projects(&request.projects)?;
    Ok(QualificationValidation {
        output_directory: qualification_directory,
        output_path,
        nonce_sha256,
        request_sha256: sha256_bytes(request_bytes),
    })
}

fn validate_gate_and_paths(
    request: &QualificationSuiteRequest,
    qualification_directory: PathBuf,
    nonce: &str,
    request_path: &Path,
    output_path: &Path,
) -> Result<(PathBuf, PathBuf, String)> {
    if request.qualification_nonce != nonce {
        bail!("embedding_qualification_nonce_mismatch");
    }
    let nonce_sha256 = sha256_bytes(nonce.as_bytes());
    if request.qualification_nonce_sha256 != nonce_sha256 {
        bail!("embedding_qualification_nonce_hash_mismatch");
    }
    if canonical_existing(&request.output_directory)? != qualification_directory {
        bail!("embedding_qualification_output_directory_mismatch");
    }
    validate_direct_child(request_path, &qualification_directory, true)?;
    let output_path = validate_direct_child(output_path, &qualification_directory, false)?;
    if output_path.exists() {
        bail!("embedding_qualification_output_exists");
    }
    Ok((qualification_directory, output_path, nonce_sha256))
}

fn validate_source(source: &QualificationSource) -> Result<()> {
    if !is_lower_hex(&source.commit, 40) || !is_lower_hex(&source.tree, 40) || source.tracked_dirty
    {
        bail!("embedding_qualification_source_invalid");
    }
    Ok(())
}

fn validate_package_and_contracts(request: &QualificationSuiteRequest) -> Result<()> {
    for value in [
        request.package.archive_sha256.as_str(),
        request.package.executable_sha256.as_str(),
        request.contracts.protocol_sha256.as_str(),
        request.contracts.constant_set_sha256.as_str(),
        request.contracts.measurement_protocol_sha256.as_str(),
    ] {
        if !is_lower_hex(value, 64) {
            bail!("embedding_qualification_hash_invalid");
        }
    }
    let executable = crate::embedding_server_transport::ExactExecutable::capture()?;
    if request.package.executable_sha256 != executable.sha256()
        || request.package.release_version != executable.version()
        || request.package.asset_target != compiled_asset_target()
    {
        bail!("embedding_qualification_package_mismatch");
    }
    let archive_sha256 = std::env::var(ARCHIVE_SHA256_ENV)
        .ok()
        .filter(|value| is_lower_hex(value, 64))
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_archive_identity_unavailable"))?;
    if request.package.archive_sha256 != archive_sha256 {
        bail!("embedding_qualification_archive_mismatch");
    }
    if request.contracts.protocol_sha256 != PER_USER_EMBEDDING_PROTOCOL_SHA256
        || request.contracts.constant_set_sha256 != PER_USER_EMBEDDING_CONSTANT_SET_SHA256
        || request.contracts.measurement_protocol_sha256
            != PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256
    {
        bail!("embedding_qualification_contract_mismatch");
    }
    if request.proof_tier != "calibration" && !PER_USER_EMBEDDING_CONSTANT_SET_FROZEN {
        bail!("embedding_qualification_constants_not_frozen");
    }
    if !matches!(
        request.proof_tier.as_str(),
        "calibration" | "hosted_package" | "protected_hardware" | "installed_runtime"
    ) {
        bail!("embedding_qualification_tier_invalid");
    }
    let manifest_path = std::env::var_os(MANIFEST_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            executable
                .path()
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(NATIVE_MANIFEST_FILE)
        });
    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("read native manifest {}", manifest_path.display()))?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).context("parse native package manifest")?;
    if manifest.get("source") != Some(&serde_json::to_value(&request.source)?)
        || manifest.pointer("/binary/sha256")
            != Some(&serde_json::Value::String(
                request.package.executable_sha256.clone(),
            ))
        || manifest.get("asset_target")
            != Some(&serde_json::Value::String(
                request.package.asset_target.clone(),
            ))
        || manifest.get("release_version")
            != Some(&serde_json::Value::String(
                request.package.release_version.clone(),
            ))
        || manifest.pointer("/server_proof/protocol_sha256")
            != Some(&serde_json::Value::String(
                request.contracts.protocol_sha256.clone(),
            ))
        || manifest.pointer("/server_proof/constant_set_sha256")
            != Some(&serde_json::Value::String(
                request.contracts.constant_set_sha256.clone(),
            ))
        || manifest.pointer("/server_proof/measurement_protocol_sha256")
            != Some(&serde_json::Value::String(
                request.contracts.measurement_protocol_sha256.clone(),
            ))
    {
        bail!("embedding_qualification_manifest_mismatch");
    }
    Ok(())
}

fn validate_runtime(runtime: &QualificationRuntime) -> Result<()> {
    if runtime.expected_backend.trim().is_empty()
        || runtime.matrix_cell_id.is_empty()
        || !runtime
            .matrix_cell_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        || runtime.cache_state != "reused"
        || runtime.residency_state != "resident"
        || !matches!(
            runtime.engine_policy.as_str(),
            "accelerated" | "cpu_explicit"
        )
    {
        bail!("embedding_qualification_runtime_invalid");
    }
    let allow_cpu = crate::sidecar_runtime::process_defaults().embedding_allow_cpu();
    if (runtime.engine_policy == "cpu_explicit") != allow_cpu {
        bail!("embedding_qualification_policy_mismatch");
    }
    if runtime.engine_policy == "cpu_explicit"
        && !runtime.expected_backend.eq_ignore_ascii_case("cpu")
    {
        bail!("embedding_qualification_backend_mismatch");
    }
    if runtime.engine_policy == "accelerated"
        && runtime.expected_backend.eq_ignore_ascii_case("cpu")
    {
        bail!("embedding_qualification_backend_mismatch");
    }
    Ok(())
}

fn validate_projects(projects: &[PathBuf]) -> Result<()> {
    if projects.len() != 2 || projects[0] == projects[1] {
        bail!("embedding_qualification_projects_invalid");
    }
    let canonical = projects
        .iter()
        .map(|project| {
            if !project.is_absolute() {
                bail!("embedding_qualification_project_not_absolute");
            }
            let metadata = fs::symlink_metadata(project)
                .with_context(|| format!("inspect qualification project {}", project.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("embedding_qualification_project_untrusted");
            }
            canonical_existing(project)
        })
        .collect::<Result<Vec<_>>>()?;
    if canonical[0] == canonical[1] {
        bail!("embedding_qualification_projects_not_distinct");
    }
    Ok(())
}

fn validate_exact_string_list(actual: &[String], expected: &[&str], field: &str) -> Result<()> {
    if actual.len() != expected.len()
        || !actual
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
    {
        bail!("embedding_qualification_{field}_mismatch");
    }
    Ok(())
}

fn read_private_request(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect qualification request {}", path.display()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_REQUEST_BYTES
    {
        bail!("embedding_qualification_request_file_untrusted");
    }
    validate_private_file_metadata(&metadata)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .with_context(|| format!("open qualification request {}", path.display()))?
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        bail!("embedding_qualification_request_too_large");
    }
    Ok(bytes)
}

fn validate_direct_child(path: &Path, directory: &Path, must_exist: bool) -> Result<PathBuf> {
    let Some(parent) = path.parent() else {
        bail!("embedding_qualification_path_untrusted");
    };
    let Some(file_name) = path.file_name() else {
        bail!("embedding_qualification_path_untrusted");
    };
    if !path.is_absolute() || path.extension().and_then(|value| value.to_str()) != Some("json") {
        bail!("embedding_qualification_path_untrusted");
    }
    if canonical_existing(parent)? != directory {
        bail!("embedding_qualification_parent_replaced");
    }
    let canonical_path = directory.join(file_name);
    if must_exist && canonical_existing(path)? != canonical_path {
        bail!("embedding_qualification_path_untrusted");
    }
    Ok(canonical_path)
}

fn required_absolute_directory(name: &str) -> Result<PathBuf> {
    let value = std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_closed"))?;
    if !value.is_absolute() {
        bail!("embedding_qualification_directory_not_absolute");
    }
    canonical_existing(&value)
}

fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect qualification directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("embedding_qualification_directory_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_qualification_directory_untrusted");
        }
    }
    Ok(())
}

fn validate_private_file_metadata(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_qualification_request_file_untrusted");
        }
    }
    Ok(())
}

fn canonical_existing(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("canonicalize {}", path.display()))
}

fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .context("atomic qualification output has no parent")?;
    validate_private_directory(parent)?;
    if path.exists() {
        bail!("embedding_qualification_output_exists");
    }
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let bytes = serde_json::to_vec_pretty(value).context("serialize qualification output")?;
    for _ in 0..32 {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".codestory-qualification-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&temp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create atomic qualification temp file"),
        };
        let result = (|| {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)?;
            File::open(parent)?.sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(error).context("publish atomic qualification output");
        }
        return Ok(());
    }
    bail!("embedding_qualification_temp_name_exhausted")
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn compiled_asset_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "x86_64") => "macos-x64",
        ("macos", "aarch64") => "macos-arm64",
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        _ => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gated_request(directory: &Path, nonce: &str) -> QualificationSuiteRequest {
        QualificationSuiteRequest {
            schema_version: 1,
            qualification_nonce: nonce.into(),
            qualification_nonce_sha256: sha256_bytes(nonce.as_bytes()),
            proof_tier: "calibration".into(),
            source: QualificationSource {
                commit: "a".repeat(40),
                tree: "b".repeat(40),
                tracked_dirty: false,
            },
            package: QualificationPackage {
                archive_sha256: "c".repeat(64),
                executable_sha256: "d".repeat(64),
                asset_target: compiled_asset_target().into(),
                release_version: env!("CARGO_PKG_VERSION").into(),
            },
            contracts: QualificationContracts {
                protocol_sha256: PER_USER_EMBEDDING_PROTOCOL_SHA256.into(),
                constant_set_sha256: PER_USER_EMBEDDING_CONSTANT_SET_SHA256.into(),
                measurement_protocol_sha256: PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256.into(),
            },
            runtime: QualificationRuntime {
                engine_policy: "cpu_explicit".into(),
                expected_backend: "CPU".into(),
                offline: true,
                matrix_cell_id: "test-calibration-cell".into(),
                cache_state: "reused".into(),
                residency_state: "resident".into(),
            },
            projects: vec![directory.join("a"), directory.join("b")],
            required_scenarios: REQUIRED_SCENARIOS
                .iter()
                .map(|value| (*value).into())
                .collect(),
            required_metrics: REQUIRED_METRICS
                .iter()
                .map(|value| (*value).into())
                .collect(),
            output_directory: directory.into(),
        }
    }

    #[test]
    fn qualification_suite_contract_is_exact_and_bounded() {
        assert_eq!(REQUIRED_SCENARIOS.len(), 8);
        assert_eq!(REQUIRED_METRICS.len(), 13);
        assert!(
            REQUIRED_SCENARIOS
                .iter()
                .all(|scenario| !scenario.is_empty())
        );
        assert!(REQUIRED_METRICS.iter().all(|metric| !metric.is_empty()));
    }

    #[test]
    fn atomic_qualification_output_is_private_and_never_overwrites() {
        let directory = tempfile::tempdir().expect("create output directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("secure output directory");
            let output = directory.path().join("output.json");
            write_atomic_json(&output, &serde_json::json!({"schema_version": 1}))
                .expect("write output");
            assert_eq!(
                fs::metadata(&output).expect("output metadata").mode() & 0o077,
                0
            );
            assert!(write_atomic_json(&output, &serde_json::json!({})).is_err());
        }
        #[cfg(not(unix))]
        {
            let output = directory.path().join("output.json");
            write_atomic_json(&output, &serde_json::json!({"schema_version": 1}))
                .expect("write output");
            assert!(write_atomic_json(&output, &serde_json::json!({})).is_err());
        }
    }

    #[test]
    fn exact_lists_reject_reordering_or_omission() {
        let exact = REQUIRED_SCENARIOS
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        validate_exact_string_list(&exact, REQUIRED_SCENARIOS, "scenarios").expect("exact list");
        let mut reordered = exact.clone();
        reordered.swap(0, 1);
        assert!(validate_exact_string_list(&reordered, REQUIRED_SCENARIOS, "scenarios").is_err());
        assert!(validate_exact_string_list(&exact[..7], REQUIRED_SCENARIOS, "scenarios").is_err());
    }

    #[test]
    fn qualification_gate_binds_raw_nonce_hash_directory_and_paths() {
        let directory = tempfile::tempdir().expect("create qualification directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("secure qualification directory");
        }
        let canonical = fs::canonicalize(directory.path()).expect("canonical directory");
        let request_path = canonical.join("request.json");
        let output_path = canonical.join("output.json");
        fs::write(&request_path, b"{}").expect("write request placeholder");
        let request = gated_request(&canonical, "private-nonce");
        let (_, validated_output, digest) = validate_gate_and_paths(
            &request,
            canonical.clone(),
            "private-nonce",
            &request_path,
            &output_path,
        )
        .expect("valid dual gate");
        assert_eq!(validated_output, output_path);
        assert_eq!(digest, sha256_bytes(b"private-nonce"));

        let mut wrong_raw = request.clone();
        wrong_raw.qualification_nonce = "other".into();
        assert!(
            validate_gate_and_paths(
                &wrong_raw,
                canonical.clone(),
                "private-nonce",
                &request_path,
                &output_path,
            )
            .is_err()
        );
        let mut wrong_hash = request.clone();
        wrong_hash.qualification_nonce_sha256 = "0".repeat(64);
        assert!(
            validate_gate_and_paths(
                &wrong_hash,
                canonical.clone(),
                "private-nonce",
                &request_path,
                &output_path,
            )
            .is_err()
        );
        assert!(
            validate_gate_and_paths(
                &request,
                canonical,
                "private-nonce",
                &request_path,
                &directory.path().join("nested").join("output.json"),
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn qualification_gate_accepts_an_equivalent_native_parent_spelling() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().expect("create qualification directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("secure qualification directory");
        let canonical = fs::canonicalize(directory.path()).expect("canonical directory");
        let request_path = canonical.join("request.json");
        fs::write(&request_path, b"{}").expect("write request placeholder");

        let aliases = tempfile::tempdir().expect("create alias parent");
        let alias = aliases.path().join("qualification-alias");
        symlink(&canonical, &alias).expect("create directory alias");
        let request = gated_request(&canonical, "private-nonce");
        let (_, output_path, _) = validate_gate_and_paths(
            &request,
            canonical.clone(),
            "private-nonce",
            &alias.join("request.json"),
            &alias.join("output.json"),
        )
        .expect("equivalent native parent is accepted");

        assert_eq!(output_path, canonical.join("output.json"));
    }
}
