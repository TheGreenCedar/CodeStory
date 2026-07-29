use crate::qualification::request::{
    QualificationContracts, QualificationExecutable, QualificationRuntime, QualificationSource,
    canonical_existing, compiled_asset_target, is_lower_hex, qualification_executable,
    read_private_request, required_absolute_directory, sha256_bytes, validate_direct_child,
    validate_private_directory, validate_project, validate_runtime, validate_source,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const CALIBRATION_DIR_ENV: &str = "CODESTORY_EMBED_CONSTANT_CALIBRATION_DIR";
const CALIBRATION_NONCE_ENV: &str = "CODESTORY_EMBED_CONSTANT_CALIBRATION_NONCE";
const ARCHIVE_SHA256_ENV: &str = "CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256";
const MANIFEST_PATH_ENV: &str = "CODESTORY_PLUGIN_CLI_MANIFEST_PATH";
const NATIVE_MANIFEST_FILE: &str = "codestory-native-manifest.json";
pub(super) const REQUIRED_RUNS: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ConstantCalibrationRequest {
    pub(super) schema_version: u32,
    pub(super) calibration_nonce: String,
    pub(super) calibration_nonce_sha256: String,
    pub(super) source: QualificationSource,
    pub(super) package: ConstantCalibrationPackage,
    pub(super) contracts: QualificationContracts,
    pub(super) runtime: QualificationRuntime,
    pub(super) project: PathBuf,
    pub(super) required_runs: u32,
    pub(super) output_directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ConstantCalibrationPackage {
    pub(super) archive_sha256: String,
    pub(super) executable_sha256: String,
    pub(super) asset_target: String,
    pub(super) release_version: String,
    pub(super) model_sha256: String,
}

pub(super) struct ValidatedRequest {
    pub(super) request: ConstantCalibrationRequest,
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
    let request: ConstantCalibrationRequest = serde_json::from_slice(&request_bytes)
        .context("parse embedding constant calibration request")?;
    validate_request(
        request,
        &request_bytes,
        request_path,
        output_path,
        executable,
    )
}

fn validate_request(
    request: ConstantCalibrationRequest,
    request_bytes: &[u8],
    request_path: &Path,
    output_path: &Path,
    executable: QualificationExecutable,
) -> Result<ValidatedRequest> {
    validate_request_shape(&request)?;
    let calibration_directory = required_absolute_directory(CALIBRATION_DIR_ENV)?;
    validate_private_directory(&calibration_directory)?;
    let nonce = std::env::var(CALIBRATION_NONCE_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("embedding_constant_calibration_gate_closed"))?;
    if request.calibration_nonce != nonce {
        bail!("embedding_constant_calibration_nonce_mismatch");
    }
    let nonce_sha256 = sha256_bytes(nonce.as_bytes());
    if request.calibration_nonce_sha256 != nonce_sha256 {
        bail!("embedding_constant_calibration_nonce_hash_mismatch");
    }
    if canonical_existing(&request.output_directory)? != calibration_directory {
        bail!("embedding_constant_calibration_output_directory_mismatch");
    }
    validate_direct_child(request_path, &calibration_directory, true)?;
    let output_path = validate_direct_child(output_path, &calibration_directory, false)?;
    if output_path.exists() {
        bail!("embedding_constant_calibration_output_exists");
    }
    validate_source(&request.source)?;
    validate_package_and_contracts(&request, &executable)?;
    validate_runtime(&request.runtime)?;
    validate_accelerated_runtime(&request.runtime)?;
    let project = validate_project(&request.project)?;
    let mut request = request;
    request.project = project;
    Ok(ValidatedRequest {
        request,
        executable,
        output_directory: calibration_directory,
        output_path,
        nonce_sha256,
        request_sha256: sha256_bytes(request_bytes),
    })
}

fn validate_request_shape(request: &ConstantCalibrationRequest) -> Result<()> {
    if request.schema_version != 1 || request.required_runs != REQUIRED_RUNS {
        bail!("embedding_constant_calibration_schema_invalid");
    }
    Ok(())
}

fn validate_accelerated_runtime(runtime: &QualificationRuntime) -> Result<()> {
    if runtime.engine_policy != "accelerated"
        || !runtime.offline
        || !matches!(
            runtime.expected_backend.to_ascii_lowercase().as_str(),
            "metal" | "vulkan"
        )
    {
        bail!("embedding_constant_calibration_runtime_invalid");
    }
    Ok(())
}

fn validate_package_and_contracts(
    request: &ConstantCalibrationRequest,
    executable: &QualificationExecutable,
) -> Result<()> {
    for value in [
        request.package.archive_sha256.as_str(),
        request.package.executable_sha256.as_str(),
        request.package.model_sha256.as_str(),
        request.contracts.protocol_sha256.as_str(),
        request.contracts.constant_set_sha256.as_str(),
        request.contracts.measurement_protocol_sha256.as_str(),
    ] {
        if !is_lower_hex(value, 64) {
            bail!("embedding_constant_calibration_hash_invalid");
        }
    }
    if request.package.executable_sha256 != executable.sha256
        || request.package.release_version != executable.version
        || request.package.asset_target != compiled_asset_target()
    {
        bail!("embedding_constant_calibration_package_mismatch");
    }
    let archive_sha256 = std::env::var(ARCHIVE_SHA256_ENV)
        .ok()
        .filter(|value| is_lower_hex(value, 64))
        .ok_or_else(|| {
            anyhow::anyhow!("embedding_constant_calibration_archive_identity_unavailable")
        })?;
    if request.package.archive_sha256 != archive_sha256 {
        bail!("embedding_constant_calibration_archive_mismatch");
    }
    if request.contracts.protocol_sha256 != codestory_retrieval::PER_USER_EMBEDDING_PROTOCOL_SHA256
        || request.contracts.constant_set_sha256
            != codestory_retrieval::PER_USER_EMBEDDING_CONSTANT_SET_SHA256
        || request.contracts.measurement_protocol_sha256
            != codestory_retrieval::PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256
    {
        bail!("embedding_constant_calibration_contract_mismatch");
    }
    validate_manifest(request, executable)
}

fn validate_manifest(
    request: &ConstantCalibrationRequest,
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
        || manifest.pointer("/runtime_executable/sha256")
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
        || manifest.pointer("/model/sha256")
            != Some(&serde_json::Value::String(
                request.package.model_sha256.clone(),
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
        || manifest.pointer("/accelerator/cpu_fallback")
            != Some(&serde_json::Value::String("unsupported".into()))
        || manifest.pointer("/accelerator/expected_protected_backend")
            != Some(&serde_json::Value::String(
                request.runtime.expected_backend.clone(),
            ))
    {
        bail!("embedding_constant_calibration_manifest_mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ConstantCalibrationPackage, ConstantCalibrationRequest, REQUIRED_RUNS,
        validate_accelerated_runtime, validate_request_shape,
    };
    use crate::qualification::request::{
        QualificationContracts, QualificationRuntime, QualificationSource,
    };
    use std::path::PathBuf;

    fn request() -> ConstantCalibrationRequest {
        ConstantCalibrationRequest {
            schema_version: 1,
            calibration_nonce: "nonce".into(),
            calibration_nonce_sha256: "a".repeat(64),
            source: QualificationSource {
                commit: "b".repeat(40),
                tree: "c".repeat(40),
                tracked_dirty: false,
            },
            package: ConstantCalibrationPackage {
                archive_sha256: "d".repeat(64),
                executable_sha256: "e".repeat(64),
                asset_target: "macos-arm64".into(),
                release_version: "0.16.3".into(),
                model_sha256: "f".repeat(64),
            },
            contracts: QualificationContracts {
                protocol_sha256: "1".repeat(64),
                constant_set_sha256: "2".repeat(64),
                measurement_protocol_sha256: "3".repeat(64),
            },
            runtime: QualificationRuntime {
                engine_policy: "accelerated".into(),
                expected_backend: "metal".into(),
                offline: true,
                matrix_cell_id: "protected_macos_arm64_metal".into(),
                cache_state: "reused".into(),
                residency_state: "resident".into(),
            },
            project: PathBuf::from("/private/synthetic"),
            required_runs: REQUIRED_RUNS,
            output_directory: PathBuf::from("/private/calibration"),
        }
    }

    #[test]
    fn request_shape_requires_exactly_three_runs() {
        let mut value = request();
        validate_request_shape(&value).expect("three-run request");
        for count in [0, 1, 2, 4] {
            value.required_runs = count;
            assert!(
                validate_request_shape(&value).is_err(),
                "{count} runs must fail"
            );
        }
    }

    #[test]
    fn request_schema_cannot_reintroduce_qualification_scenarios_or_metrics() {
        for (field, value) in [
            ("required_scenarios", serde_json::json!([])),
            ("required_metrics", serde_json::json!([])),
            ("proof_tier", serde_json::json!("calibration")),
        ] {
            let mut encoded = serde_json::to_value(request()).expect("serialize request");
            encoded
                .as_object_mut()
                .expect("request object")
                .insert(field.into(), value);
            assert!(
                serde_json::from_value::<ConstantCalibrationRequest>(encoded).is_err(),
                "{field} must remain outside the constant-only request schema"
            );
        }
    }

    #[test]
    fn runtime_accepts_only_offline_accelerated_metal_or_vulkan() {
        let mut runtime = request().runtime;
        validate_accelerated_runtime(&runtime).expect("Metal runtime");
        runtime.expected_backend = "vulkan".into();
        validate_accelerated_runtime(&runtime).expect("Vulkan runtime");
        for (policy, backend, offline) in [
            ("cpu_explicit", "cpu", true),
            ("accelerated", "cpu", true),
            ("accelerated", "cuda", true),
            ("accelerated", "metal", false),
        ] {
            runtime.engine_policy = policy.into();
            runtime.expected_backend = backend.into();
            runtime.offline = offline;
            assert!(validate_accelerated_runtime(&runtime).is_err());
        }
    }
}
