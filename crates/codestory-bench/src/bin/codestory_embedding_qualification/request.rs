use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    PER_USER_EMBEDDING_CONSTANT_SET_FROZEN, PER_USER_EMBEDDING_CONSTANT_SET_SHA256,
    PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256, PER_USER_EMBEDDING_PROTOCOL_SHA256,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const QUALIFICATION_DIR_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIR";
pub(super) const QUALIFICATION_NONCE_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_NONCE";
const ARCHIVE_SHA256_ENV: &str = "CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256";
const MANIFEST_PATH_ENV: &str = "CODESTORY_PLUGIN_CLI_MANIFEST_PATH";
const NATIVE_MANIFEST_FILE: &str = "codestory-native-manifest.json";
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

pub(super) const REQUIRED_SCENARIOS: &[&str] = &[
    "client_death",
    "cold_race",
    "frozen_owner",
    "incompatible_owner",
    "mixed_queue",
    "server_crash",
    "true_idle_respawn",
    "worker_stall",
];

pub(super) const REQUIRED_METRICS: &[&str] = &[
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
pub(super) struct QualificationSuiteRequest {
    pub(super) schema_version: u32,
    pub(super) qualification_nonce: String,
    pub(super) qualification_nonce_sha256: String,
    pub(super) proof_tier: String,
    pub(super) source: QualificationSource,
    pub(super) package: QualificationPackage,
    pub(super) contracts: QualificationContracts,
    pub(super) runtime: QualificationRuntime,
    pub(super) projects: Vec<PathBuf>,
    pub(super) required_scenarios: Vec<String>,
    pub(super) required_metrics: Vec<String>,
    pub(super) output_directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct QualificationSource {
    pub(super) commit: String,
    pub(super) tree: String,
    pub(super) tracked_dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct QualificationPackage {
    pub(super) archive_sha256: String,
    pub(super) executable_sha256: String,
    pub(super) asset_target: String,
    pub(super) release_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct QualificationContracts {
    pub(super) protocol_sha256: String,
    pub(super) constant_set_sha256: String,
    pub(super) measurement_protocol_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct QualificationRuntime {
    pub(super) engine_policy: String,
    pub(super) expected_backend: String,
    pub(super) offline: bool,
    pub(super) matrix_cell_id: String,
    pub(super) cache_state: String,
    pub(super) residency_state: String,
}

#[derive(Debug, Clone)]
pub(super) struct QualificationExecutable {
    pub(super) path: PathBuf,
    pub(super) sha256: String,
    pub(super) version: String,
}

pub(super) struct ValidatedRequest {
    pub(super) request: QualificationSuiteRequest,
    pub(super) executable: QualificationExecutable,
    pub(super) output_directory: PathBuf,
    pub(super) output_path: PathBuf,
    pub(super) nonce_sha256: String,
    pub(super) request_sha256: String,
}

pub(super) fn load(
    cli: PathBuf,
    request_path: &Path,
    output_path: &Path,
) -> Result<ValidatedRequest> {
    let executable = qualification_executable(cli)?;
    let request_bytes = read_private_request(request_path)?;
    let request: QualificationSuiteRequest =
        serde_json::from_slice(&request_bytes).context("parse embedding qualification request")?;
    validate_request(
        request,
        &request_bytes,
        request_path,
        output_path,
        executable,
    )
}

fn validate_request(
    request: QualificationSuiteRequest,
    request_bytes: &[u8],
    request_path: &Path,
    output_path: &Path,
    executable: QualificationExecutable,
) -> Result<ValidatedRequest> {
    if request.schema_version != 1 {
        bail!("embedding_qualification_schema_invalid");
    }
    let qualification_directory = required_absolute_directory(QUALIFICATION_DIR_ENV)?;
    validate_private_directory(&qualification_directory)?;
    let nonce = std::env::var(QUALIFICATION_NONCE_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_closed"))?;
    let (output_directory, output_path, nonce_sha256) = validate_gate_and_paths(
        &request,
        qualification_directory,
        &nonce,
        request_path,
        output_path,
    )?;
    validate_exact_string_list(&request.required_scenarios, REQUIRED_SCENARIOS, "scenarios")?;
    validate_exact_string_list(&request.required_metrics, REQUIRED_METRICS, "metrics")?;
    validate_source(&request.source)?;
    validate_package_and_contracts(&request, &executable)?;
    validate_runtime(&request.runtime)?;
    validate_projects(&request.projects)?;
    Ok(ValidatedRequest {
        request,
        executable,
        output_directory,
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

fn validate_package_and_contracts(
    request: &QualificationSuiteRequest,
    executable: &QualificationExecutable,
) -> Result<()> {
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
    if request.package.executable_sha256 != executable.sha256
        || request.package.release_version != executable.version
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
    validate_manifest(request, executable)
}

fn validate_manifest(
    request: &QualificationSuiteRequest,
    executable: &QualificationExecutable,
) -> Result<()> {
    let manifest_path = std::env::var_os(MANIFEST_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            executable
                .path
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

fn qualification_executable(path: PathBuf) -> Result<QualificationExecutable> {
    if !path.is_absolute() || !path.is_file() {
        bail!("embedding_qualification_executable_invalid");
    }
    let path = fs::canonicalize(path).context("canonicalize qualification executable")?;
    let sha256 = sha256_bytes(
        &fs::read(&path)
            .with_context(|| format!("read qualification executable {}", path.display()))?,
    );
    let output = Command::new(&path)
        .arg("--version")
        .output()
        .context("read qualification executable version")?;
    if !output.status.success() {
        bail!("embedding_qualification_executable_version_unavailable");
    }
    let stdout = String::from_utf8(output.stdout)
        .context("qualification executable version is not UTF-8")?;
    let version = stdout
        .split_whitespace()
        .last()
        .filter(|version| !version.is_empty())
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_executable_version_invalid"))?
        .to_string();
    Ok(QualificationExecutable {
        path,
        sha256,
        version,
    })
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
    let allow_cpu = codestory_retrieval::sidecar_process_defaults().embedding_allow_cpu();
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

pub(super) fn read_private_request(path: &Path) -> Result<Vec<u8>> {
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

pub(super) fn validate_direct_child(
    path: &Path,
    directory: &Path,
    must_exist: bool,
) -> Result<PathBuf> {
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

pub(super) fn required_absolute_directory(name: &str) -> Result<PathBuf> {
    let value = std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_closed"))?;
    if !value.is_absolute() {
        bail!("embedding_qualification_directory_not_absolute");
    }
    canonical_existing(&value)
}

pub(super) fn validate_private_directory(path: &Path) -> Result<()> {
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

pub(super) fn canonical_existing(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("canonicalize {}", path.display()))
}

pub(super) fn sha256_bytes(bytes: &[u8]) -> String {
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
