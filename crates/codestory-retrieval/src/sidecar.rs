use crate::config::{
    SidecarImagePins, SidecarLayout, SidecarProfile, SidecarRuntimeConfig,
    default_sidecar_image_pins, sidecar_runtime_auto, sidecar_runtime_for_project,
};
use crate::generation::{
    SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED, manifest_has_current_sidecar_contract,
    manifest_staleness_reason_for_runtime, manifest_unavailable_reason_for_runtime,
};
use crate::health::{
    EmbeddingLaunchMetadata, RetrievalStatusReport, attach_manifest_contract, attach_repair_hint,
    probe_sidecar_health_for_runtime, unavailable_status_report_with_embedding_device,
};
use crate::index::{compute_sidecar_input_fingerprint_for_runtime, sidecar_project_id_for_runtime};
use anyhow::{Context, Result, bail};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext,
};
use codestory_store::Store;
use codestory_workspace::{RefreshInputs, WorkspaceManifest};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::process::Command;
#[cfg(not(windows))]
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(
        pid: std::ffi::c_int,
        buffer: *mut std::ffi::c_void,
        buffer_size: u32,
    ) -> std::ffi::c_int;
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sysctl(
        name: *mut std::ffi::c_int,
        name_len: u32,
        old_value: *mut std::ffi::c_void,
        old_len: *mut usize,
        new_value: *mut std::ffi::c_void,
        new_len: usize,
    ) -> std::ffi::c_int;
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct WindowsFileTime {
    low_date_time: u32,
    high_date_time: u32,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> RawHandle;
    fn GetProcessTimes(
        process: RawHandle,
        creation_time: *mut WindowsFileTime,
        exit_time: *mut WindowsFileTime,
        kernel_time: *mut WindowsFileTime,
        user_time: *mut WindowsFileTime,
    ) -> i32;
}

const NATIVE_EMBEDDING_PROCESS_START_TOLERANCE_MS: i64 = 5 * 1000;
#[cfg(windows)]
const WINDOWS_PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
#[cfg(windows)]
const WINDOWS_ERROR_INVALID_PARAMETER: i32 = 87;
#[cfg(windows)]
const WINDOWS_DATETIME_TICKS_AT_FILETIME_EPOCH: u64 = 504_911_232_000_000_000;
#[cfg(windows)]
const WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
#[cfg(target_os = "macos")]
const MACOS_CTL_KERN: std::ffi::c_int = 1;
#[cfg(target_os = "macos")]
const MACOS_KERN_PROCARGS2: std::ffi::c_int = 49;
#[cfg(target_os = "macos")]
const MACOS_PROC_PIDPATH_MAX_SIZE: usize = 4 * 1024;
const LEGACY_LEXICAL_MIGRATION_ENTRY_LIMIT: usize = 4_096;
#[cfg(not(windows))]
const NATIVE_EMBEDDING_STOP_WAIT: Duration = Duration::from_secs(5);
#[cfg(not(windows))]
const NATIVE_EMBEDDING_STOP_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEmbeddingLaunchIdentityStatus {
    Matched { pid: u32 },
    NotRunning { pid: u32 },
    Mismatched { pid: u32, reason: String },
    Unverified { pid: Option<u32>, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Runtime state file written by `sidecar_up`.
///
/// The file records local sidecar endpoints and data roots only. It is not a readiness manifest;
/// callers must use `sidecar_status` or `strict_sidecar_status` before trusting retrieval output.
pub struct SidecarStateFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_identity: Option<codestory_workspace::ProjectIdentityV3>,
    #[serde(default = "default_sidecar_owner")]
    pub owner: String,
    #[serde(default = "default_sidecar_profile")]
    pub profile: String,
    #[serde(default = "default_sidecar_namespace")]
    pub namespace: String,
    #[serde(default = "default_sidecar_namespace")]
    pub compose_project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub qdrant_http_port: u16,
    pub qdrant_grpc_port: u16,
    #[serde(default = "default_embed_http_port")]
    pub embed_http_port: u16,
    #[serde(default = "default_embed_url")]
    pub embed_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_endpoint_origin: Option<crate::config::EmbeddingEndpointOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_endpoint_fingerprint_sha256: Option<String>,
    #[serde(default = "default_embedding_device_policy")]
    pub embedding_device_policy: String,
    #[serde(default = "default_embedding_device_state")]
    pub embedding_device_state: String,
    #[serde(default = "default_embedding_device_observation_source")]
    pub embedding_device_observation_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_detected_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_detected_gpu: Option<String>,
    #[serde(default)]
    pub embedding_accelerator_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_accelerator_request_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_accelerator_request_device: Option<String>,
    #[serde(default)]
    pub embedding_cpu_allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_launch: Option<EmbeddingLaunchMetadata>,
    #[serde(default)]
    pub embedding_launch_ownership: EmbeddingLaunchOwnership,
    #[serde(default = "default_sidecar_image_pins")]
    pub sidecar_images: SidecarImagePins,
    // v0.15 migration-only read alias; serialization is canonical. Remove the alias in v0.16.
    #[serde(alias = "zoekt_data_dir")]
    pub lexical_data_dir: String,
    pub qdrant_data_dir: String,
    pub scip_artifacts_root: String,
    #[serde(default)]
    pub compose_file: Option<String>,
    /// Whether this bootstrap started an empty Compose project. Failure cleanup may only tear
    /// down projects it started; explicit operator cleanup still targets the recorded project.
    #[serde(default = "default_true")]
    pub compose_started_by_bootstrap: bool,
    #[serde(default)]
    pub cleanup_command: String,
    pub started_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingLaunchOwnership {
    /// This state created the native process and is responsible for stopping it.
    #[default]
    Owner,
    /// This state borrows a broker-verified native process owned by another state.
    Attached,
}

impl SidecarStateFile {
    pub fn owns_embedding_launch(&self) -> bool {
        self.embedding_launch.is_some()
            && self.embedding_launch_ownership == EmbeddingLaunchOwnership::Owner
    }
}

pub fn sidecar_up() -> Result<SidecarStateFile> {
    sidecar_up_with_runtime(&SidecarRuntimeConfig::local(), None)
}

pub fn sidecar_up_with_runtime(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
) -> Result<SidecarStateFile> {
    sidecar_up_with_runtime_and_launch_metadata(runtime, compose_file, None)
}

pub fn sidecar_up_with_runtime_preserving_launch(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
) -> Result<SidecarStateFile> {
    let (embedding_launch, embedding_launch_ownership) =
        reusable_embedding_launch_from_state(runtime).map_or(
            (None, EmbeddingLaunchOwnership::Owner),
            |(launch, ownership)| (Some(launch), ownership),
        );
    sidecar_up_with_runtime_and_launch_metadata_and_ownership(
        runtime,
        compose_file,
        embedding_launch,
        embedding_launch_ownership,
    )
}

pub(crate) fn sidecar_up_with_runtime_and_launch_metadata(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
    embedding_launch: Option<EmbeddingLaunchMetadata>,
) -> Result<SidecarStateFile> {
    sidecar_up_with_runtime_and_launch_metadata_and_ownership(
        runtime,
        compose_file,
        embedding_launch,
        EmbeddingLaunchOwnership::Owner,
    )
}

pub(crate) fn sidecar_up_with_runtime_and_launch_metadata_and_ownership(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
    embedding_launch: Option<EmbeddingLaunchMetadata>,
    embedding_launch_ownership: EmbeddingLaunchOwnership,
) -> Result<SidecarStateFile> {
    sidecar_up_with_runtime_and_launch_metadata_ownership_and_compose_origin(
        runtime,
        compose_file,
        embedding_launch,
        embedding_launch_ownership,
        true,
    )
}

pub(crate) fn sidecar_up_with_runtime_and_launch_metadata_ownership_and_compose_origin(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
    embedding_launch: Option<EmbeddingLaunchMetadata>,
    embedding_launch_ownership: EmbeddingLaunchOwnership,
    compose_started_by_bootstrap: bool,
) -> Result<SidecarStateFile> {
    runtime.ensure_ports_allocated()?;
    let layout = &runtime.layout;
    cleanup_owned_legacy_lexical_artifacts(layout)?;
    layout.ensure_data_dirs()?;
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let ownership = runtime.ownership();
    let embedding_endpoint_fingerprint = runtime.embedding_endpoint_fingerprint()?;
    let state = SidecarStateFile {
        project_identity: runtime.project_identity.clone(),
        owner: "codestory".into(),
        profile: runtime.profile.as_str().into(),
        namespace: runtime.namespace.clone(),
        compose_project: runtime.compose_project.clone(),
        run_id: runtime.run_id.clone(),
        qdrant_http_port: layout.qdrant_http_port,
        qdrant_grpc_port: layout.qdrant_grpc_port,
        embed_http_port: ownership.ports.embed_http,
        embed_url: ownership.ports.embed_url,
        embedding_endpoint_origin: Some(ownership.embedding_endpoint_origin),
        embedding_endpoint_fingerprint_sha256: Some(embedding_endpoint_fingerprint),
        embedding_device_policy: embedding_device.requested_policy.into(),
        embedding_device_state: embedding_device.observed_state.into(),
        embedding_device_observation_source: embedding_device.observation_source.into(),
        embedding_detected_provider: embedding_device.detected_provider,
        embedding_detected_gpu: embedding_device.detected_gpu,
        embedding_accelerator_requested: embedding_device.accelerator_requested,
        embedding_accelerator_request_provider: embedding_device.accelerator_request_provider,
        embedding_accelerator_request_device: embedding_device.accelerator_request_device,
        embedding_cpu_allowed: embedding_device.cpu_allowed,
        embedding_launch,
        embedding_launch_ownership,
        sidecar_images: default_sidecar_image_pins(),
        lexical_data_dir: layout.lexical_data_dir.display().to_string(),
        qdrant_data_dir: layout.qdrant_data_dir.display().to_string(),
        scip_artifacts_root: layout.scip_artifacts_root.display().to_string(),
        compose_file: compose_file.map(|path| path.display().to_string()),
        compose_started_by_bootstrap,
        cleanup_command: runtime.cleanup_command.clone(),
        started_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
    };
    let json = serde_json::to_vec_pretty(&state).context("serialize sidecar state")?;
    codestory_workspace::atomic_file::write_bytes_atomic(
        &layout.state_file,
        "retrieval-sidecars",
        &json,
    )
    .context("write versioned sidecar state")?;
    Ok(state)
}

pub(crate) fn persist_embedding_container_identity(path: &Path, identity: &str) -> Result<()> {
    let mut value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read sidecar state {}", path.display()))?,
    )
    .with_context(|| format!("parse sidecar state {}", path.display()))?;
    value
        .as_object_mut()
        .context("sidecar state must be an object")?
        .insert("embedding_container_identity".into(), identity.into());
    let json = serde_json::to_vec_pretty(&value).context("serialize sidecar state")?;
    codestory_workspace::atomic_file::write_bytes_atomic(path, "retrieval-sidecars", &json)
        .context("persist embedding container identity")
}

fn cleanup_owned_legacy_lexical_artifacts(layout: &SidecarLayout) -> Result<()> {
    let Ok(raw) = std::fs::read_to_string(&layout.state_file) else {
        return Ok(());
    };
    let Ok(state) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(());
    };
    if state.get("owner").and_then(|value| value.as_str()) != Some("codestory") {
        return Ok(());
    }
    let legacy_root = layout.lexical_data_dir.with_file_name("zoekt");
    let legacy_state_matches = state
        .get("zoekt_data_dir")
        .and_then(|value| value.as_str())
        .is_some_and(|path| Path::new(path) == legacy_root);
    if !legacy_state_matches {
        return Ok(());
    }
    let mut remaining = LEGACY_LEXICAL_MIGRATION_ENTRY_LIMIT;
    if !remove_tree_bounded(&legacy_root, &mut remaining)? {
        eprintln!(
            "CodeStory legacy lexical migration cleanup reached its {}-entry limit; remaining owned data will be retried",
            LEGACY_LEXICAL_MIGRATION_ENTRY_LIMIT
        );
    }
    Ok(())
}

fn remove_tree_bounded(path: &Path, remaining: &mut usize) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
    };
    if *remaining == 0 {
        return Ok(false);
    }
    *remaining -= 1;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        std::fs::remove_file(path)
            .with_context(|| format!("remove legacy lexical artifact {}", path.display()))?;
        return Ok(true);
    }
    let mut complete = true;
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("read legacy lexical directory {}", path.display()))?
    {
        if !remove_tree_bounded(&entry?.path(), remaining)? {
            complete = false;
            break;
        }
    }
    if complete {
        std::fs::remove_dir(path)
            .with_context(|| format!("remove empty legacy lexical directory {}", path.display()))?;
    }
    Ok(complete)
}

/// Returns true when a sidecar state file matches the runtime identity used for reuse/handoff.
pub fn sidecar_state_matches_runtime(
    state: &SidecarStateFile,
    runtime: &SidecarRuntimeConfig,
) -> bool {
    validate_sidecar_state_matches_runtime(state, runtime).is_ok()
}

pub fn validate_sidecar_state_matches_runtime(
    state: &SidecarStateFile,
    runtime: &SidecarRuntimeConfig,
) -> Result<()> {
    let ownership = runtime.ownership();
    let embedding_endpoint_fingerprint = runtime.embedding_endpoint_fingerprint()?;
    let ports = &ownership.ports;
    let exact = state.owner == "codestory"
        && state.namespace == runtime.namespace
        && state.compose_project == runtime.compose_project
        && state.profile == runtime.profile.as_str()
        && state.run_id.as_deref() == runtime.run_id.as_deref()
        && state.qdrant_http_port == ports.qdrant_http
        && state.qdrant_grpc_port == ports.qdrant_grpc
        && state.embed_http_port == ports.embed_http
        && state.embed_url == ports.embed_url
        && state.embedding_endpoint_origin == Some(ownership.embedding_endpoint_origin)
        && state.embedding_endpoint_fingerprint_sha256.as_deref()
            == Some(embedding_endpoint_fingerprint.as_str())
        && managed_native_launch_matches_selected_runtime(state, runtime)
        && crate::config::project_identity_matches_runtime(
            state.project_identity.as_ref(),
            runtime.project_identity.as_ref(),
        );
    if !exact {
        anyhow::bail!(
            "sidecar state does not exactly match the retained runtime owner/profile/namespace/run/project/ports/endpoint contract"
        );
    }
    Ok(())
}

fn managed_native_launch_matches_selected_runtime(
    state: &SidecarStateFile,
    runtime: &SidecarRuntimeConfig,
) -> bool {
    let Some(launch) = state.embedding_launch.as_ref() else {
        return true;
    };
    if launch.launch_mode != crate::config::EmbeddingServerLaunchMode::NativeSpawned.as_str() {
        return true;
    }
    runtime.embedding.endpoint_origin == crate::config::EmbeddingEndpointOrigin::ManagedSidecar
        && launch.endpoint == state.embed_url
        && launch.endpoint == runtime.embedding.endpoint
        && crate::config::local_embedding_endpoint_port(&launch.endpoint)
            == Some(state.embed_http_port)
        && native_launch_port(&launch.launch_args) == Some(state.embed_http_port)
}

fn native_launch_port(launch_args: &[String]) -> Option<u16> {
    let mut positions = launch_args
        .iter()
        .enumerate()
        .filter(|(_, argument)| argument.as_str() == "--port");
    let (index, _) = positions.next()?;
    if positions.next().is_some()
        || launch_args
            .iter()
            .any(|argument| argument.starts_with("--port="))
    {
        return None;
    }
    launch_args
        .get(index + 1)?
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
}

fn reusable_embedding_launch_from_state(
    runtime: &SidecarRuntimeConfig,
) -> Option<(EmbeddingLaunchMetadata, EmbeddingLaunchOwnership)> {
    let state = read_sidecar_state(&runtime.layout.state_file)?;
    if !sidecar_state_matches_runtime(&state, runtime) {
        return None;
    }
    state
        .embedding_launch
        .map(|launch| (launch, state.embedding_launch_ownership))
}

pub fn sidecar_down() -> Result<()> {
    sidecar_down_for_runtime(&SidecarRuntimeConfig::local())
}

pub fn sidecar_down_for_project(project_root: &Path, profile: SidecarProfile) -> Result<()> {
    sidecar_down_for_runtime(&sidecar_runtime_for_project(project_root, profile))
}

pub fn sidecar_down_for_runtime(runtime: &SidecarRuntimeConfig) -> Result<()> {
    sidecar_down_for_runtime_inner(runtime, false)
}

/// Cleanup after a failed bootstrap while preserving an exact Compose project that predated the
/// attempt. Native processes and the failed attempt's state publication are still removed.
pub fn sidecar_down_after_failed_bootstrap_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> Result<()> {
    sidecar_down_for_runtime_inner(runtime, true)
}

fn sidecar_down_for_runtime_inner(
    runtime: &SidecarRuntimeConfig,
    preserve_preexisting_compose: bool,
) -> Result<()> {
    let layout = &runtime.layout;
    if layout.state_file.exists() {
        let contents = std::fs::read_to_string(&layout.state_file)
            .with_context(|| format!("read sidecar state {}", layout.state_file.display()))?;
        let state: SidecarStateFile = serde_json::from_str(&contents)
            .with_context(|| format!("parse sidecar state {}", layout.state_file.display()))?;
        validate_sidecar_state_matches_runtime(&state, runtime).with_context(|| {
            format!(
                "preserving mismatched sidecar state at {}; inspect it with `codestory-cli sidecar inventory --project <repo> --format json`",
                layout.state_file.display()
            )
        })?;
        if runtime.profile == SidecarProfile::Agent
            && (!preserve_preexisting_compose || state.compose_started_by_bootstrap)
        {
            crate::compose::docker_compose_down_for_state(&state)?;
        }
        stop_native_embedding_process_for_state(&state)?;
        std::fs::remove_file(&layout.state_file).context("remove versioned sidecar state")?;
    }
    if let Some(legacy_state_file) = crate::config::legacy_state_file_for_runtime(runtime) {
        let message = format!(
            "legacy unversioned sidecar state was discovered and preserved at {}; inspect it with `codestory-cli sidecar inventory --project <repo> --format json` before provenance-aware cleanup",
            legacy_state_file.display()
        );
        if preserve_preexisting_compose {
            tracing::warn!(legacy_state_file = %legacy_state_file.display(), "{message}");
        } else {
            anyhow::bail!(message);
        }
    }
    Ok(())
}

fn stop_native_embedding_process_for_state(state: &SidecarStateFile) -> Result<()> {
    if !state.owns_embedding_launch() {
        return Ok(());
    }
    let Some(launch) = state.embedding_launch.as_ref() else {
        return Ok(());
    };
    stop_native_embedding_process_for_launch(launch)
}

pub fn stop_native_embedding_process_for_launch(launch: &EmbeddingLaunchMetadata) -> Result<()> {
    if launch.launch_mode != crate::config::EmbeddingServerLaunchMode::NativeSpawned.as_str() {
        return Ok(());
    }
    let Some(pid) = launch.pid else {
        return Ok(());
    };
    stop_native_embedding_process(pid, launch)
}

pub fn ensure_native_embedding_launch_identity(launch: &EmbeddingLaunchMetadata) -> Result<u32> {
    match native_embedding_launch_identity_status(launch) {
        NativeEmbeddingLaunchIdentityStatus::Matched { pid } => Ok(pid),
        NativeEmbeddingLaunchIdentityStatus::NotRunning { pid } => {
            bail!("identity_not_running: native embedding pid {pid} is not running")
        }
        NativeEmbeddingLaunchIdentityStatus::Mismatched { pid, reason } => {
            bail!("identity_mismatch: native embedding pid {pid}: {reason}")
        }
        NativeEmbeddingLaunchIdentityStatus::Unverified { pid, reason } => {
            bail!("identity_unverified: native embedding pid {pid:?}: {reason}")
        }
    }
}

pub fn native_embedding_launch_identity_status(
    launch: &EmbeddingLaunchMetadata,
) -> NativeEmbeddingLaunchIdentityStatus {
    if launch.launch_mode != crate::config::EmbeddingServerLaunchMode::NativeSpawned.as_str() {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: launch.pid,
            reason: "native embedding launch mode is not native_spawned".to_string(),
        };
    }
    let Some(pid) = launch.pid else {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: None,
            reason: "recorded native embedding launch is missing pid".to_string(),
        };
    };
    if pid == 0 {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: "native embedding pid is zero".to_string(),
        };
    }
    if pid == std::process::id() {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: format!("native embedding pid {pid} is the current CodeStory process"),
        };
    }
    let start_identity_before = match native_embedding_process_start_identity(pid) {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            return NativeEmbeddingLaunchIdentityStatus::NotRunning { pid };
        }
        Err(error) => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: format!(
                    "query native embedding process start identity before snapshot: {error}"
                ),
            };
        }
    };
    let Some(expected_start_identity) = launch
        .process_start_identity
        .as_deref()
        .filter(|identity| !identity.trim().is_empty())
    else {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: "recorded native embedding launch is missing exact process start identity"
                .to_string(),
        };
    };
    let snapshot = match native_embedding_process_snapshot(pid) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return NativeEmbeddingLaunchIdentityStatus::NotRunning { pid },
        Err(error) => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: error.to_string(),
            };
        }
    };
    match native_embedding_process_start_identity(pid) {
        Ok(Some(actual)) if start_identity_before != actual => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: format!(
                    "native embedding process start identity changed during snapshot: before={start_identity_before}, after={actual}"
                ),
            };
        }
        Ok(Some(actual)) if actual == expected_start_identity => {}
        Ok(Some(actual)) => {
            return NativeEmbeddingLaunchIdentityStatus::Mismatched {
                pid,
                reason: format!(
                    "live process start identity does not match recorded native embedding launch: expected {expected_start_identity}, got {actual}"
                ),
            };
        }
        Ok(None) => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: "live native embedding process start identity is unavailable".to_string(),
            };
        }
        Err(error) => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: format!("query live native embedding process start identity: {error}"),
            };
        }
    }
    native_embedding_process_match_status(launch, &snapshot, pid)
}

fn stop_native_embedding_process(pid: u32, launch: &EmbeddingLaunchMetadata) -> Result<()> {
    if pid == 0 {
        bail!("identity_unverified: native embedding pid is zero");
    }
    if pid == std::process::id() {
        bail!("identity_unverified: native embedding pid {pid} is the current CodeStory process");
    }
    match native_embedding_launch_identity_status(launch) {
        NativeEmbeddingLaunchIdentityStatus::Matched { .. } => {}
        NativeEmbeddingLaunchIdentityStatus::NotRunning { .. } => return Ok(()),
        NativeEmbeddingLaunchIdentityStatus::Mismatched { reason, .. }
        | NativeEmbeddingLaunchIdentityStatus::Unverified { reason, .. } => {
            bail!("identity_unverified: native embedding pid {pid}: {reason}")
        }
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .status()
            .with_context(|| format!("run taskkill for native embedding pid {pid}"))?;
        if !status.success() && native_embedding_process_snapshot(pid)?.is_some() {
            bail!("failed to stop native embedding pid {pid}: taskkill exited with {status}");
        }
    }
    #[cfg(not(windows))]
    {
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .with_context(|| format!("run kill for native embedding pid {pid}"))?;
        if !status.success() {
            if native_embedding_process_snapshot(pid)?.is_some() {
                bail!("failed to stop native embedding pid {pid}: kill exited with {status}");
            }
            return Ok(());
        }
        wait_for_native_embedding_process_exit(pid)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn wait_for_native_embedding_process_exit(pid: u32) -> Result<()> {
    wait_for_native_embedding_process_exit_with(
        pid,
        NATIVE_EMBEDDING_STOP_WAIT,
        NATIVE_EMBEDDING_STOP_POLL,
        || native_embedding_process_snapshot(pid).map(|snapshot| snapshot.is_some()),
    )
}

#[cfg(not(windows))]
fn wait_for_native_embedding_process_exit_with<F>(
    pid: u32,
    timeout: Duration,
    poll: Duration,
    mut process_running: F,
) -> Result<()>
where
    F: FnMut() -> Result<bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if !process_running()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("native embedding pid {pid} did not exit after SIGTERM");
        }
        if !poll.is_zero() {
            std::thread::sleep(poll);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeEmbeddingProcessSnapshot {
    executable_path: Option<String>,
    command_line: Option<String>,
    arguments: Option<Vec<String>>,
    started_at_epoch_ms: Option<i64>,
}

#[cfg(test)]
fn ensure_native_embedding_process_matches(
    launch: &EmbeddingLaunchMetadata,
    snapshot: &NativeEmbeddingProcessSnapshot,
) -> Result<()> {
    match native_embedding_process_match_status(launch, snapshot, 0) {
        NativeEmbeddingLaunchIdentityStatus::Matched { .. } => Ok(()),
        NativeEmbeddingLaunchIdentityStatus::Mismatched { reason, .. }
        | NativeEmbeddingLaunchIdentityStatus::Unverified { reason, .. } => bail!("{reason}"),
        NativeEmbeddingLaunchIdentityStatus::NotRunning { .. } => {
            bail!("native embedding pid is not running")
        }
    }
}

fn native_embedding_process_match_status(
    launch: &EmbeddingLaunchMetadata,
    snapshot: &NativeEmbeddingProcessSnapshot,
    pid: u32,
) -> NativeEmbeddingLaunchIdentityStatus {
    let expected_executable = match launch.executable_path.as_deref() {
        Some(path) => path,
        None => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: "recorded native embedding launch is missing executable_path".to_string(),
            };
        }
    };
    match snapshot.executable_path.as_deref() {
        Some(actual) if same_identity_path(expected_executable, actual) => {}
        Some(actual) => {
            return NativeEmbeddingLaunchIdentityStatus::Mismatched {
                pid,
                reason: format!(
                    "live executable path does not match recorded native embedding launch: expected {expected_executable}, got {actual}"
                ),
            };
        }
        None => {
            let Some(command_line) = snapshot.command_line.as_deref() else {
                return NativeEmbeddingLaunchIdentityStatus::Unverified {
                    pid: Some(pid),
                    reason: "live process has no executable path or command line".to_string(),
                };
            };
            if !command_mentions_path(command_line, expected_executable) {
                return NativeEmbeddingLaunchIdentityStatus::Mismatched {
                    pid,
                    reason: format!(
                        "live command line does not mention recorded executable path {expected_executable}"
                    ),
                };
            }
        }
    }

    if launch.launch_args.is_empty() {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: "recorded native embedding launch is missing launch_args".to_string(),
        };
    }
    if let Some(arguments) = snapshot.arguments.as_deref() {
        let Some(actual_launch_args) = arguments.get(1..) else {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: "live native embedding argv is missing argv[0]".to_string(),
            };
        };
        if actual_launch_args != launch.launch_args.as_slice() {
            return NativeEmbeddingLaunchIdentityStatus::Mismatched {
                pid,
                reason: format!(
                    "live native embedding argv does not exactly match recorded launch args: expected {:?}, got {:?}",
                    launch.launch_args, actual_launch_args
                ),
            };
        }
    } else {
        let Some(command_line) = snapshot.command_line.as_deref() else {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason:
                    "live native embedding process has no command line for launch-arg validation"
                        .to_string(),
            };
        };
        for arg in &launch.launch_args {
            if !arg.is_empty() && !command_line.contains(arg) {
                return NativeEmbeddingLaunchIdentityStatus::Mismatched {
                    pid,
                    reason: format!(
                        "live native embedding command line is missing recorded launch arg {arg:?}"
                    ),
                };
            }
        }
    }
    let Some(expected) = launch.spawned_at_epoch_ms else {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: "recorded native embedding launch is missing spawned_at_epoch_ms".to_string(),
        };
    };
    let Some(actual) = snapshot.started_at_epoch_ms else {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: "live native embedding process start identity is unavailable".to_string(),
        };
    };
    if expected.abs_diff(actual) > NATIVE_EMBEDDING_PROCESS_START_TOLERANCE_MS as u64 {
        return NativeEmbeddingLaunchIdentityStatus::Mismatched {
            pid,
            reason: format!(
                "live process start time does not match recorded native embedding launch: expected around {expected}, got {actual}"
            ),
        };
    }
    NativeEmbeddingLaunchIdentityStatus::Matched { pid }
}

fn command_mentions_path(command_line: &str, expected_path: &str) -> bool {
    normalized_identity_path(command_line).contains(&normalized_identity_path(expected_path))
}

fn same_identity_path(left: &str, right: &str) -> bool {
    codestory_workspace::same_workspace_path(
        Path::new(left.trim_matches('"')),
        Path::new(right.trim_matches('"')),
    )
}

fn normalized_identity_path(path: &str) -> String {
    let normalized = path.trim_matches('"').replace('\\', "/");
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

#[cfg(windows)]
fn windows_process_creation_time(pid: u32) -> Result<Option<WindowsFileTime>> {
    if pid == 0 {
        bail!("native embedding process pid must be greater than zero");
    }
    let raw_handle = unsafe { OpenProcess(WINDOWS_PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw_handle.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(WINDOWS_ERROR_INVALID_PARAMETER) {
            return Ok(None);
        }
        return Err(error).with_context(|| format!("open native embedding process {pid}"));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw_handle) };
    let mut creation_time = WindowsFileTime::default();
    let mut exit_time = WindowsFileTime::default();
    let mut kernel_time = WindowsFileTime::default();
    let mut user_time = WindowsFileTime::default();
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        )
    } == 0
    {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("query native embedding start identity for pid {pid}"));
    }
    Ok(Some(creation_time))
}

#[cfg(windows)]
pub fn native_embedding_process_start_identity(pid: u32) -> Result<Option<String>> {
    let Some(creation_time) = windows_process_creation_time(pid)? else {
        return Ok(None);
    };
    let ticks = windows_datetime_ticks_from_filetime(&creation_time)?;
    Ok(Some(format!("windows:{ticks}")))
}

#[cfg(windows)]
fn windows_filetime_ticks(filetime: &WindowsFileTime) -> u64 {
    (u64::from(filetime.high_date_time) << 32) | u64::from(filetime.low_date_time)
}

#[cfg(windows)]
fn windows_datetime_ticks_from_filetime(filetime: &WindowsFileTime) -> Result<u64> {
    let filetime_ticks = windows_filetime_ticks(filetime);
    // Win32_Process.CreationDate exposes microseconds, so discard sub-microsecond
    // FILETIME ticks to preserve identities serialized by the previous CIM query.
    let legacy_filetime_ticks = filetime_ticks / 10 * 10;
    legacy_filetime_ticks
        .checked_add(WINDOWS_DATETIME_TICKS_AT_FILETIME_EPOCH)
        .context("convert Windows process creation time to DateTime ticks")
}

#[cfg(windows)]
fn windows_epoch_ms_from_filetime(filetime: &WindowsFileTime) -> Result<i64> {
    let elapsed_ticks = windows_filetime_ticks(filetime)
        .checked_sub(WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH)
        .context("convert Windows process creation time to Unix epoch")?;
    i64::try_from(elapsed_ticks / 10_000)
        .context("convert Windows process creation time to epoch milliseconds")
}

#[cfg(target_os = "linux")]
pub fn native_embedding_process_start_identity(pid: u32) -> Result<Option<String>> {
    let stat_path = Path::new("/proc").join(pid.to_string()).join("stat");
    let stat = match fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", stat_path.display())),
    };
    let start_ticks = stat
        .rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .with_context(|| format!("parse process start identity from {}", stat_path.display()))?;
    Ok(Some(format!("linux:{start_ticks}")))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
pub fn native_embedding_process_start_identity(pid: u32) -> Result<Option<String>> {
    let output = Command::new("ps")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .with_context(|| format!("query native embedding start identity for pid {pid}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let identity = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!identity.is_empty()).then(|| format!("unix:{identity}")))
}

#[cfg(windows)]
fn native_embedding_process_snapshot(pid: u32) -> Result<Option<NativeEmbeddingProcessSnapshot>> {
    #[derive(Deserialize)]
    struct WindowsProcessInfo {
        #[serde(rename = "ExecutablePath")]
        executable_path: Option<String>,
        #[serde(rename = "CommandLine")]
        command_line: Option<String>,
    }

    let Some(creation_time) = windows_process_creation_time(pid)? else {
        return Ok(None);
    };
    let script = format!(
        "$p=Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}'; if ($null -eq $p) {{ exit 2 }}; [pscustomobject]@{{ExecutablePath=$p.ExecutablePath;CommandLine=$p.CommandLine}} | ConvertTo-Json -Compress"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .with_context(|| format!("query native embedding pid {pid}"))?;
    if output.status.code() == Some(2) {
        return Ok(None);
    }
    if !output.status.success() {
        bail!(
            "query native embedding pid {pid} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let info: WindowsProcessInfo = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse native embedding process info for pid {pid}"))?;
    Ok(Some(NativeEmbeddingProcessSnapshot {
        executable_path: info.executable_path,
        command_line: info.command_line,
        arguments: None,
        started_at_epoch_ms: Some(windows_epoch_ms_from_filetime(&creation_time)?),
    }))
}

#[cfg(target_os = "linux")]
fn native_embedding_process_snapshot(pid: u32) -> Result<Option<NativeEmbeddingProcessSnapshot>> {
    let process_dir = Path::new("/proc").join(pid.to_string());
    if !process_dir.exists() {
        return Ok(None);
    }
    let Some(process_state) = native_embedding_linux_process_state(&process_dir)? else {
        return Ok(None);
    };
    if process_state == 'Z' {
        return Ok(None);
    }
    let executable_path = fs::read_link(process_dir.join("exe"))
        .ok()
        .map(|path| path.display().to_string());
    let command_line = fs::read(process_dir.join("cmdline")).ok().map(|bytes| {
        bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part))
            .collect::<Vec<_>>()
            .join(" ")
    });
    let started_at_epoch_ms = native_embedding_process_started_at_epoch_ms(pid);
    Ok(Some(NativeEmbeddingProcessSnapshot {
        executable_path,
        command_line,
        arguments: None,
        started_at_epoch_ms,
    }))
}

#[cfg(target_os = "linux")]
fn native_embedding_linux_process_state(process_dir: &Path) -> Result<Option<char>> {
    let stat_path = process_dir.join("stat");
    let stat = match fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", stat_path.display())),
    };
    let state = stat
        .rsplit_once(") ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|state| state.chars().next())
        .with_context(|| format!("parse Linux process state from {}", stat_path.display()))?;
    Ok(Some(state))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn native_embedding_process_snapshot(pid: u32) -> Result<Option<NativeEmbeddingProcessSnapshot>> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "state=", "-o", "command="])
        .output()
        .with_context(|| format!("query native embedding pid {pid}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let mut snapshot =
        native_embedding_non_linux_unix_process_snapshot_from_ps_output(&output.stdout);
    if let Some(snapshot) = &mut snapshot {
        #[cfg(target_os = "macos")]
        {
            let (executable_path, arguments) = native_embedding_macos_process_identity(pid)?;
            snapshot.executable_path = Some(executable_path);
            snapshot.arguments = Some(arguments);
        }
        snapshot.started_at_epoch_ms = native_embedding_process_started_at_epoch_ms(pid);
    }
    Ok(snapshot)
}

#[cfg(target_os = "macos")]
fn native_embedding_macos_process_identity(pid: u32) -> Result<(String, Vec<String>)> {
    let mut executable = vec![0_u8; MACOS_PROC_PIDPATH_MAX_SIZE];
    // SAFETY: `executable` is writable for the supplied length and remains alive for the call.
    let executable_len = unsafe {
        proc_pidpath(
            pid as std::ffi::c_int,
            executable.as_mut_ptr().cast(),
            executable.len() as u32,
        )
    };
    if executable_len <= 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("query executable path for native embedding pid {pid}"));
    }
    executable.truncate(executable_len as usize);
    let executable_path = String::from_utf8(executable)
        .with_context(|| format!("decode executable path for native embedding pid {pid}"))?;

    let mut mib = [MACOS_CTL_KERN, MACOS_KERN_PROCARGS2, pid as std::ffi::c_int];
    let mut arguments_len = 0_usize;
    // SAFETY: the MIB and output-length pointers are valid; a null output buffer requests size.
    if unsafe {
        sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut arguments_len,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("query argv size for native embedding pid {pid}"));
    }
    if arguments_len < std::mem::size_of::<std::ffi::c_int>() {
        bail!("native embedding pid {pid} returned an invalid Darwin argv buffer size");
    }
    let mut arguments = vec![0_u8; arguments_len];
    // SAFETY: `arguments` is writable for `arguments_len`, and all pointer arguments remain valid.
    if unsafe {
        sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            arguments.as_mut_ptr().cast(),
            &mut arguments_len,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("query argv for native embedding pid {pid}"));
    }
    arguments.truncate(arguments_len);
    let arguments = native_embedding_macos_argv_from_procargs(&arguments)
        .with_context(|| format!("parse argv for native embedding pid {pid}"))?;
    Ok((executable_path, arguments))
}

#[cfg(target_os = "macos")]
fn native_embedding_macos_argv_from_procargs(bytes: &[u8]) -> Result<Vec<String>> {
    let argc_size = std::mem::size_of::<std::ffi::c_int>();
    let argc_bytes: [u8; std::mem::size_of::<std::ffi::c_int>()] = bytes
        .get(..argc_size)
        .context("Darwin procargs buffer is missing argc")?
        .try_into()
        .expect("argc slice has exact size");
    let argc = std::ffi::c_int::from_ne_bytes(argc_bytes);
    if argc <= 0 {
        bail!("Darwin procargs buffer has invalid argc {argc}");
    }

    let mut cursor = argc_size;
    let executable_end = bytes[cursor..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|offset| cursor + offset)
        .context("Darwin procargs buffer is missing executable terminator")?;
    cursor = executable_end + 1;
    while bytes.get(cursor) == Some(&0) {
        cursor += 1;
    }

    let mut arguments = Vec::with_capacity(argc as usize);
    for index in 0..argc as usize {
        let end = bytes[cursor..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|offset| cursor + offset)
            .with_context(|| {
                format!("Darwin procargs buffer is missing argv[{index}] terminator")
            })?;
        arguments.push(
            String::from_utf8(bytes[cursor..end].to_vec())
                .with_context(|| format!("Darwin procargs argv[{index}] is not UTF-8"))?,
        );
        cursor = end + 1;
    }
    Ok(arguments)
}

#[cfg(any(test, all(not(windows), not(target_os = "linux"))))]
fn native_embedding_non_linux_unix_process_snapshot_from_ps_output(
    output: &[u8],
) -> Option<NativeEmbeddingProcessSnapshot> {
    let row = String::from_utf8_lossy(output);
    let row = row.trim();
    let state_end = row.find(char::is_whitespace).unwrap_or(row.len());
    let state = &row[..state_end];
    if state.is_empty() || state.starts_with('Z') {
        return None;
    }
    let command_line = row[state_end..].trim().to_string();
    if command_line.is_empty() {
        return None;
    }
    Some(NativeEmbeddingProcessSnapshot {
        executable_path: None,
        command_line: Some(command_line),
        arguments: None,
        started_at_epoch_ms: None,
    })
}

#[cfg(not(windows))]
fn native_embedding_process_started_at_epoch_ms(pid: u32) -> Option<i64> {
    let output = Command::new("ps")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    native_embedding_process_started_at_epoch_ms_from_lstart(&output.stdout)
}

#[cfg(any(test, not(windows)))]
fn native_embedding_process_started_at_epoch_ms_from_lstart(output: &[u8]) -> Option<i64> {
    use chrono::TimeZone;

    let started = chrono::NaiveDateTime::parse_from_str(
        String::from_utf8_lossy(output).trim(),
        "%a %b %e %H:%M:%S %Y",
    )
    .ok()?;
    Some(chrono::Utc.from_utc_datetime(&started).timestamp_millis())
}

/// Probe sidecar health and attach the latest retrieval manifest when storage is available.
///
/// A healthy infrastructure report is still weaker than strict readiness: it may show running
/// services while the manifest is stale for the current worktree.
pub fn sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner(project_root, storage_path, false)
}

/// Probe sidecar health and fail stale manifest identity checks.
///
/// This is the status surface to use before serving `retrieval_mode=full` packet/search evidence.
pub fn strict_sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner(project_root, storage_path, true)
}

pub fn strict_sidecar_status_for_profile(
    project_root: &Path,
    storage_path: Option<&Path>,
    profile: SidecarProfile,
) -> Result<RetrievalStatusReport> {
    strict_sidecar_status_for_runtime(
        project_root,
        storage_path,
        sidecar_runtime_for_project(project_root, profile),
    )
}

pub fn strict_sidecar_status_for_runtime(
    project_root: &Path,
    storage_path: Option<&Path>,
    runtime: SidecarRuntimeConfig,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner_with_runtime(project_root, storage_path, true, runtime)
}

fn sidecar_status_inner(
    project_root: &Path,
    storage_path: Option<&Path>,
    strict: bool,
) -> Result<RetrievalStatusReport> {
    let runtime = sidecar_runtime_auto(project_root);
    sidecar_status_inner_with_runtime(project_root, storage_path, strict, runtime)
}

fn sidecar_status_inner_with_runtime(
    project_root: &Path,
    storage_path: Option<&Path>,
    strict: bool,
    runtime: SidecarRuntimeConfig,
) -> Result<RetrievalStatusReport> {
    let layout = runtime.layout.clone();
    let embedding_runtime_probe = strict.then(|| {
        let probe = crate::embeddings::probe_product_embedding_runtime_for_runtime(&runtime);
        tracing::debug!(
            reachable = probe.reachable,
            detail = %probe.detail,
            endpoint_origin = ?runtime.embedding.endpoint_origin,
            "Probed selected embedding endpoint for strict project runtime"
        );
        probe
    });
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(&runtime);
    let project_id = sidecar_project_id_for_runtime(project_root, &runtime)?;
    let manifest = if let Some(path) = storage_path.filter(|path| path.exists()) {
        let storage = Store::open(path).context("open storage for manifest")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
        if let Some(manifest) = manifest.as_ref()
            && let Some(probe) = embedding_runtime_probe.as_ref()
            && !probe.reachable
        {
            return Ok(attach_stored_status_context(
                unavailable_status_report_with_embedding_device(
                    format!("embedding_runtime_unavailable: {}", probe.detail),
                    Some(manifest.clone()),
                    &embedding_device,
                ),
                project_root,
                &storage,
                &runtime,
            ));
        }
        if strict
            && let Some(manifest) = manifest.as_ref()
            && let Some(reason) = strict_readiness_unavailable_reason_for_runtime(
                project_root,
                path,
                &storage,
                &project_id,
                manifest,
                &runtime,
            )
            .context("check strict sidecar readiness")?
        {
            return Ok(attach_stored_status_context(
                unavailable_status_report_with_embedding_device(
                    format!("sidecar_manifest_stale: {reason}"),
                    Some(manifest.clone()),
                    &embedding_device,
                ),
                project_root,
                &storage,
                &runtime,
            ));
        }
        if let Some(manifest) = manifest.as_ref()
            && let Some(reason) =
                manifest_unavailable_reason_for_runtime(&project_id, &storage, manifest, &runtime)
        {
            return Ok(attach_stored_status_context(
                unavailable_status_report_with_embedding_device(
                    reason,
                    Some(manifest.clone()),
                    &embedding_device,
                ),
                project_root,
                &storage,
                &runtime,
            ));
        }
        let report = probe_sidecar_health_for_runtime(
            &layout,
            &project_id,
            manifest,
            &embedding_device,
            &runtime,
        );
        return Ok(attach_stored_status_context(
            report,
            project_root,
            &storage,
            &runtime,
        ));
    } else {
        None
    };
    Ok(attach_status_ownership(
        attach_repair_hint(
            attach_manifest_contract(
                probe_sidecar_health_for_runtime(
                    &layout,
                    &project_id,
                    manifest,
                    &embedding_device,
                    &runtime,
                ),
                project_root,
            ),
            project_root,
            Some(&runtime),
        ),
        &runtime,
    ))
}

fn attach_stored_status_context(
    report: RetrievalStatusReport,
    project_root: &Path,
    storage: &Store,
    runtime: &SidecarRuntimeConfig,
) -> RetrievalStatusReport {
    attach_status_ownership(
        enrich_status_with_semantic_doc_stats(
            attach_repair_hint(
                attach_manifest_contract(report, project_root),
                project_root,
                Some(runtime),
            ),
            storage,
        ),
        runtime,
    )
}

fn attach_status_ownership(
    mut report: RetrievalStatusReport,
    runtime: &SidecarRuntimeConfig,
) -> RetrievalStatusReport {
    report.ownership = Some(runtime.ownership());
    report.query_embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    if let Some(state) = read_sidecar_state(&runtime.layout.state_file)
        && sidecar_state_matches_runtime(&state, runtime)
    {
        report.embedding_launch = state.embedding_launch.or(report.embedding_launch);
        report.sidecar_images = state.sidecar_images;
    }
    report
}

fn read_sidecar_state(path: &Path) -> Option<SidecarStateFile> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<SidecarStateFile>(&contents).ok())
}

pub(crate) fn embedding_launch_metadata_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> Option<EmbeddingLaunchMetadata> {
    let state = read_sidecar_state(&runtime.layout.state_file)?;
    sidecar_state_matches_runtime(&state, runtime)
        .then_some(state.embedding_launch)
        .flatten()
}

pub(crate) fn live_native_embedding_launch_metadata_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> Result<Option<EmbeddingLaunchMetadata>> {
    if crate::config::embedding_server_launch_mode_for_runtime(runtime)?
        != crate::config::EmbeddingServerLaunchMode::NativeSpawned
    {
        return Ok(None);
    }
    let launch = embedding_launch_metadata_for_runtime(runtime)
        .context("native embedding runtime state has no matching launch metadata")?;
    ensure_native_embedding_launch_identity(&launch)
        .context("validate live native embedding launch identity")?;
    Ok(Some(launch))
}

fn enrich_status_with_semantic_doc_stats(
    mut report: RetrievalStatusReport,
    storage: &Store,
) -> RetrievalStatusReport {
    if let Ok(stats) = storage.get_llm_symbol_doc_stats() {
        report.stored_doc_vector_producer_backend = stats.embedding_backend;
        report.stored_doc_vector_dim = stats.embedding_dim;
        report.stored_doc_vector_mixed_backends = Some(stats.mixed_embedding_backends);
    }
    report
}

#[cfg(test)]
pub(crate) fn validate_strict_sidecar_readiness(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
) -> Result<()> {
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    validate_strict_sidecar_readiness_for_runtime(project_root, storage_path, storage, &runtime)
}

pub(crate) fn validate_strict_sidecar_readiness_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
    runtime: &SidecarRuntimeConfig,
) -> Result<()> {
    let project_id = sidecar_project_id_for_runtime(project_root, runtime)?;
    let Some(manifest) = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load retrieval manifest for strict readiness")?
    else {
        return Ok(());
    };
    if let Some(reason) = strict_readiness_unavailable_reason_for_runtime(
        project_root,
        storage_path,
        storage,
        &project_id,
        &manifest,
        runtime,
    )? {
        anyhow::bail!("sidecar_manifest_stale: {reason}");
    }
    Ok(())
}

#[cfg(test)]
fn strict_readiness_unavailable_reason(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
    project_id: &str,
    manifest: &codestory_store::RetrievalIndexManifest,
) -> Result<Option<String>> {
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    strict_readiness_unavailable_reason_for_runtime(
        project_root,
        storage_path,
        storage,
        project_id,
        manifest,
        &runtime,
    )
}

fn strict_readiness_unavailable_reason_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
    project_id: &str,
    manifest: &codestory_store::RetrievalIndexManifest,
    runtime: &SidecarRuntimeConfig,
) -> Result<Option<String>> {
    if storage
        .has_incomplete_incremental_run()
        .context("inspect incomplete incremental index marker")?
    {
        return Ok(Some("incomplete_incremental_index_run".into()));
    }
    if !manifest_has_current_sidecar_contract(project_id, manifest) {
        return Ok(None);
    }
    if let Some(reason) = manifest_staleness_reason_for_runtime(storage, manifest, runtime)
        && manifest_contract_drift_should_win(&reason)
    {
        return Ok(None);
    }
    let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let expected_doc_backend = crate::embeddings::embedding_backend_label_for_runtime(runtime);
    if let Ok(stats) = storage.get_llm_symbol_doc_stats() {
        if stats.mixed_embedding_backends {
            return Ok(Some("sidecar_symbol_docs_mixed_embedding_backends".into()));
        }
        if stats
            .embedding_backend
            .as_deref()
            .is_some_and(|backend| backend != expected_doc_backend)
        {
            return Ok(Some(format!(
                "sidecar_symbol_doc_embedding_backend_changed: stored={} current={}",
                stats.embedding_backend.as_deref().unwrap_or("<missing>"),
                expected_doc_backend
            )));
        }
    }
    let embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    let current_input = compute_sidecar_input_fingerprint_for_runtime(
        storage,
        storage_path,
        project_root,
        project_id,
        &embedding_backend,
        embedding_dim,
        &runtime.embedding,
    )
    .context("compute strict sidecar input fingerprint")?;
    let stored_files = storage
        .files()
        .inventory()
        .context("load indexed file inventory")?;
    if let Some(file) = stored_files.iter().find(|file| file.retry_required) {
        return Ok(Some(format!(
            "indexed_file_error_retry_required: {}",
            file.path.display()
        )));
    }
    if manifest.sidecar_input_hash.as_deref() == Some(current_input.hash.as_str())
        && manifest.projection_count == Some(current_input.projection_count)
        && manifest.symbol_doc_count == Some(current_input.symbol_doc_count)
        && manifest.dense_projection_count == Some(current_input.dense_projection_count)
        && manifest.semantic_policy_version == current_input.semantic_policy_version
        && manifest.graph_artifact_hash.as_deref()
            == Some(current_input.graph_artifact_hash.as_str())
        && manifest.dense_reason_counts_json.as_deref()
            == Some(current_input.dense_reason_counts_json.as_str())
    {
        return Ok(None);
    }

    let workspace = WorkspaceManifest::open(project_root.to_path_buf())
        .context("open workspace manifest for strict sidecar readiness")?;
    let refresh_inputs = RefreshInputs {
        stored_files,
        inventory: Default::default(),
    };
    let plan = workspace
        .build_execution_plan(&refresh_inputs)
        .context("build strict sidecar freshness plan")?;
    if let Some(path) = plan
        .files_to_index
        .iter()
        .find(|path| graph_indexed_source_path(path))
    {
        return Ok(Some(format!(
            "indexable_file_added_or_changed_after_sidecar_manifest: {}",
            path.display()
        )));
    }
    if let Some(file_id) = plan.files_to_remove.first() {
        return Ok(Some(format!(
            "indexed_file_removed_after_sidecar_manifest: file_id={file_id}"
        )));
    }
    Ok(Some(format!(
        "sidecar_input_hash_changed: manifest={} current={}; symbol_doc_count manifest={} current={}; dense_projection_count manifest={} current={}; projection_count manifest={} current={}",
        manifest
            .sidecar_input_hash
            .as_deref()
            .unwrap_or("<missing>"),
        current_input.hash,
        manifest
            .symbol_doc_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.symbol_doc_count,
        manifest
            .dense_projection_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.dense_projection_count,
        manifest
            .projection_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.projection_count
    )))
}

fn graph_indexed_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(language_support_profile_for_ext)
        .is_some_and(|profile| profile.support_mode == LanguageSupportMode::ParserBackedGraph)
}

fn manifest_contract_drift_should_win(reason: &str) -> bool {
    reason.contains("sidecar_embedding_backend_changed")
        || reason.contains("sidecar_embedding_dim_changed")
        || reason == SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED
}

fn default_sidecar_owner() -> String {
    "codestory".into()
}

fn default_sidecar_profile() -> String {
    "local".into()
}

fn default_sidecar_namespace() -> String {
    "codestory".into()
}

fn default_true() -> bool {
    true
}

fn default_embed_http_port() -> u16 {
    crate::config::DEFAULT_EMBED_HTTP_PORT
}

fn default_embed_url() -> String {
    SidecarLayout::embed_base_url(crate::config::DEFAULT_EMBED_HTTP_PORT)
}

fn default_embedding_device_policy() -> String {
    "accelerator_required".into()
}

fn default_embedding_device_state() -> String {
    "unknown".into()
}

fn default_embedding_device_observation_source() -> String {
    "sidecar_unobserved".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{
        SIDECAR_SCHEMA_VERSION, sidecar_generation_id, sidecar_qdrant_collection,
    };
    use crate::index::project_id_for_root;
    use crate::test_support::retrieval_manifest_fixture;
    use codestory_contracts::graph::{ErrorInfo, IndexStep, Node, NodeId, NodeKind};
    use codestory_store::{FileInfo, FileRole, LlmSymbolDoc};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::TempDir;

    fn test_runtime(root: &TempDir) -> SidecarRuntimeConfig {
        let mut runtime = SidecarRuntimeConfig {
            project_identity: None,
            layout: SidecarLayout {
                qdrant_http_port: 16333,
                qdrant_grpc_port: 16334,
                lexical_data_dir: root.path().join("lexical"),
                qdrant_data_dir: root.path().join("qdrant"),
                scip_artifacts_root: root.path().join("scip"),
                state_file: root.path().join("retrieval-sidecars.json"),
            },
            profile: SidecarProfile::Local,
            run_id: None,
            namespace: "test".to_string(),
            compose_project: "test".to_string(),
            embed_http_port: 18080,
            cleanup_command: "codestory-cli retrieval down".to_string(),
            labels: BTreeMap::new(),
            ..SidecarRuntimeConfig::local()
        };
        runtime.embedding.endpoint = SidecarLayout::embed_base_url(runtime.embed_http_port);
        runtime.embedding.endpoint_origin = crate::config::EmbeddingEndpointOrigin::ManagedSidecar;
        runtime
    }

    fn live_embedding_runtime(
        root: &TempDir,
        request_count: usize,
    ) -> (SidecarRuntimeConfig, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind embedding test server");
        let address = listener
            .local_addr()
            .expect("embedding test server address");
        let server = thread::spawn(move || {
            let body = serde_json::json!({
                "data": [{
                    "index": 0,
                    "embedding": vec![0.1_f32; crate::embeddings::RETRIEVAL_EMBEDDING_DIM],
                }],
            })
            .to_string();
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("accept embedding probe");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut buffer).expect("read embedding probe");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let Some(header_end) = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|index| index + 4)
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len().saturating_sub(header_end) >= content_length {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("write embedding probe response");
            }
        });
        let mut runtime = test_runtime(root);
        runtime.embed_http_port = address.port();
        runtime.embedding.configuration_error = None;
        runtime.embedding.backend = "llamacpp".to_string();
        runtime.embedding.endpoint = format!("http://{address}/v1/embeddings");
        runtime.embedding.expected_dim = Some(crate::embeddings::RETRIEVAL_EMBEDDING_DIM);
        runtime.embedding.allow_remote = false;
        (runtime, server)
    }

    fn semantic_doc_with_backend(backend: &str) -> LlmSymbolDoc {
        LlmSymbolDoc {
            node_id: NodeId(1),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "do_work".into(),
            qualified_name: Some("pkg::do_work".into()),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            doc_text: "semantic doc".into(),
            doc_version: 5,
            doc_hash: "doc-hash".into(),
            embedding_profile: Some("bge-base-en-v1.5".into()),
            embedding_model: format!("BAAI/bge-base-en-v1.5-local|backend={backend}"),
            embedding_backend: Some(backend.into()),
            embedding_dim: crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            doc_shape: Some("semantic_doc_version=5;scope=durable_symbols".into()),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            dense_reason: Some("public_api".into()),
            embedding: vec![0.01; crate::embeddings::RETRIEVAL_EMBEDDING_DIM],
            updated_at_epoch_ms: 123,
        }
    }

    #[test]
    fn status_attaches_embedding_launch_metadata_from_state_file() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let _device = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_DEVICE");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let root = TempDir::new().expect("root");
        let mut runtime = test_runtime(&root);
        runtime.project_identity = Some(codestory_workspace::project_identity_v3(root.path()));
        let launch = EmbeddingLaunchMetadata {
            provider: "llamacpp".to_string(),
            launch_mode: "native_spawned".to_string(),
            endpoint: "http://127.0.0.1:18080/v1/embeddings".to_string(),
            pid: Some(1234),
            spawned_at_epoch_ms: Some(123),
            process_start_identity: None,
            spawn_protocol: None,
            launch_args: vec!["--port".to_string(), "18080".to_string()],
            launch_fingerprint_sha256: Some("fingerprint".to_string()),
            executable_source: Some("managed_cache".to_string()),
            executable_path: Some("C:/cache/llama-server".to_string()),
            model_path: Some("C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string()),
            log_path: Some("C:/cache/llama-server-native.log".to_string()),
            requested_device: None,
        };
        let state =
            sidecar_up_with_runtime_and_launch_metadata(&runtime, None, Some(launch.clone()))
                .expect("write state");
        assert_eq!(state.project_identity, runtime.project_identity);
        let mut foreign_runtime = runtime.clone();
        foreign_runtime
            .project_identity
            .as_mut()
            .expect("foreign identity")
            .workspace_id = "foreign-workspace".to_string();
        assert!(!sidecar_state_matches_runtime(&state, &foreign_runtime));
        let mut foreign_qdrant_runtime = runtime.clone();
        foreign_qdrant_runtime.layout.qdrant_http_port += 1;
        assert!(!sidecar_state_matches_runtime(
            &state,
            &foreign_qdrant_runtime
        ));
        assert_eq!(
            state.embedding_endpoint_origin,
            Some(crate::config::EmbeddingEndpointOrigin::ManagedSidecar)
        );
        let expected_endpoint_fingerprint = runtime
            .embedding_endpoint_fingerprint()
            .expect("keyed endpoint fingerprint");
        assert_eq!(
            state.embedding_endpoint_fingerprint_sha256.as_deref(),
            Some(expected_endpoint_fingerprint.as_str())
        );
        let mut foreign_endpoint_state = state.clone();
        foreign_endpoint_state.embedding_endpoint_fingerprint_sha256 = Some("foreign".to_string());
        assert!(!sidecar_state_matches_runtime(
            &foreign_endpoint_state,
            &runtime
        ));
        let mut foreign_launch_endpoint = state.clone();
        foreign_launch_endpoint
            .embedding_launch
            .as_mut()
            .expect("native launch")
            .endpoint = "http://127.0.0.1:18081/v1/embeddings".to_string();
        assert!(!sidecar_state_matches_runtime(
            &foreign_launch_endpoint,
            &runtime
        ));
        let mut foreign_launch_port = state.clone();
        foreign_launch_port
            .embedding_launch
            .as_mut()
            .expect("native launch")
            .launch_args = vec!["--port".to_string(), "18081".to_string()];
        assert!(!sidecar_state_matches_runtime(
            &foreign_launch_port,
            &runtime
        ));
        assert_eq!(state.embedding_launch, Some(launch.clone()));
        assert_eq!(
            state.embedding_accelerator_request_provider.as_deref(),
            Some("metal")
        );
        assert_eq!(state.embedding_accelerator_request_device, None);

        let report = unavailable_status_report_with_embedding_device(
            "missing",
            None,
            &crate::embeddings::embedding_device_readiness(),
        );
        let report = attach_status_ownership(report, &runtime);

        assert_eq!(report.embedding_launch, Some(launch));
    }

    #[test]
    fn sidecar_up_preserving_launch_keeps_native_embedding_pid() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let _device = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_DEVICE");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let root = TempDir::new().expect("root");
        let runtime = test_runtime(&root);
        let launch = native_embedding_launch_fixture();
        let initial =
            sidecar_up_with_runtime_and_launch_metadata(&runtime, None, Some(launch.clone()))
                .expect("write initial state");
        assert_eq!(initial.embedding_launch, Some(launch.clone()));

        let preserved = sidecar_up_with_runtime_preserving_launch(&runtime, None)
            .expect("rewrite state preserving launch");

        assert_eq!(preserved.embedding_launch, Some(launch));
    }

    #[test]
    fn sidecar_down_removes_attached_state_without_stopping_shared_pid() {
        let _lock = crate::test_support::env_lock();
        let root = TempDir::new().expect("root");
        let runtime = test_runtime(&root);
        let mut launch = native_embedding_launch_fixture();
        launch.pid = Some(std::process::id());
        sidecar_up_with_runtime_and_launch_metadata_and_ownership(
            &runtime,
            None,
            Some(launch),
            EmbeddingLaunchOwnership::Attached,
        )
        .expect("write attached state");

        sidecar_down_for_runtime(&runtime).expect("borrower down must not stop shared pid");

        assert!(!runtime.layout.state_file.exists());
    }

    #[test]
    fn sidecar_down_preserves_state_that_does_not_match_runtime() {
        let _lock = crate::test_support::env_lock();
        let root = TempDir::new().expect("root");
        let runtime = test_runtime(&root);
        let mut state = sidecar_up_with_runtime(&runtime, None).expect("write state");
        state.qdrant_http_port += 1;
        std::fs::write(
            &runtime.layout.state_file,
            serde_json::to_vec_pretty(&state).expect("state json"),
        )
        .expect("replace state");

        let error = sidecar_down_for_runtime(&runtime)
            .expect_err("mismatched state must not authorize destructive cleanup");

        assert!(format!("{error:#}").contains("preserving mismatched sidecar state"));
        assert!(runtime.layout.state_file.exists());
    }

    #[test]
    fn sidecar_down_preserves_recorded_compose_state_when_docker_is_unavailable() {
        let _lock = crate::test_support::env_lock();
        let root = TempDir::new().expect("root");
        let runtime = SidecarRuntimeConfig::for_project_profile_with_run_id_in_cache(
            Some(root.path()),
            SidecarProfile::Agent,
            Some("docker-retry"),
            root.path(),
        );
        let compose_file = root.path().join("retrieval-compose.yml");
        std::fs::write(&compose_file, "services: {}\n").expect("compose file");
        sidecar_up_with_runtime(&runtime, Some(&compose_file)).expect("write agent state");
        let empty_path = root.path().join("empty-path");
        std::fs::create_dir(&empty_path).expect("empty PATH");
        let _path = EnvGuard::set("PATH", &empty_path.display().to_string());

        let error = sidecar_down_for_runtime(&runtime)
            .expect_err("temporary Docker unavailability must retain exact cleanup state");

        assert!(
            format!("{error:#}").contains("docker is unavailable"),
            "{error:#}"
        );
        assert!(
            runtime.layout.state_file.exists(),
            "recorded Compose ownership must survive for a later exact retry"
        );
    }

    #[test]
    fn failed_bootstrap_cleanup_preserves_preexisting_compose_after_state_publication() {
        let _lock = crate::test_support::env_lock();
        let root = TempDir::new().expect("root");
        let runtime = SidecarRuntimeConfig::for_project_profile_with_run_id_in_cache(
            Some(root.path()),
            SidecarProfile::Agent,
            Some("preexisting-compose"),
            root.path(),
        );
        let compose_file = root.path().join("retrieval-compose.yml");
        std::fs::write(&compose_file, "services: {}\n").expect("compose file");
        let state = sidecar_up_with_runtime_and_launch_metadata_ownership_and_compose_origin(
            &runtime,
            Some(&compose_file),
            None,
            EmbeddingLaunchOwnership::Owner,
            false,
        )
        .expect("publish failed-attempt state");
        assert!(!state.compose_started_by_bootstrap);
        let legacy_state_file = crate::config::legacy_state_path_for_runtime(&runtime)
            .expect("calculated legacy state path");
        let legacy_namespace = legacy_state_file
            .parent()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .expect("legacy namespace");
        let legacy_identity = codestory_workspace::project_identity_v2(root.path());
        std::fs::create_dir_all(legacy_state_file.parent().expect("legacy state parent"))
            .expect("legacy state directory");
        std::fs::write(
            &legacy_state_file,
            serde_json::to_vec_pretty(&serde_json::json!({
                "project_identity": legacy_identity,
                "owner": "codestory",
                "profile": "agent",
                "namespace": legacy_namespace,
                "compose_project": legacy_namespace,
                "run_id": runtime.run_id,
            }))
            .expect("legacy state json"),
        )
        .expect("legacy state");
        let empty_path = root.path().join("empty-path");
        std::fs::create_dir(&empty_path).expect("empty PATH");
        let _path = EnvGuard::set("PATH", &empty_path.display().to_string());

        sidecar_down_after_failed_bootstrap_for_runtime(&runtime)
            .expect("failure cleanup must not invoke Docker for a pre-existing project");

        assert!(
            !runtime.layout.state_file.exists(),
            "failed attempt state must be removed after preserving pre-existing Compose"
        );
        assert!(
            legacy_state_file.exists(),
            "legacy state must remain preserved"
        );
    }

    #[test]
    fn sidecar_state_replacement_is_complete_and_leaves_no_temp_file() {
        let _lock = crate::test_support::env_lock();
        let root = TempDir::new().expect("root");
        let runtime = test_runtime(&root);
        sidecar_up_with_runtime(&runtime, None).expect("initial state");
        persist_embedding_container_identity(
            &runtime.layout.state_file,
            "container-id|2026-07-12T00:00:00Z|true",
        )
        .expect("persist initial container identity");
        let initial_raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&runtime.layout.state_file).expect("read initial state"),
        )
        .expect("initial state json");
        assert_eq!(
            initial_raw
                .get("embedding_container_identity")
                .and_then(|value| value.as_str()),
            Some("container-id|2026-07-12T00:00:00Z|true")
        );
        persist_embedding_container_identity(
            &runtime.layout.state_file,
            "recreated-id|2026-07-12T00:01:00Z|true",
        )
        .expect("replace recreated container identity");
        let refreshed_raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&runtime.layout.state_file).expect("read refreshed state"),
        )
        .expect("refreshed state json");
        assert_eq!(
            refreshed_raw
                .get("embedding_container_identity")
                .and_then(|value| value.as_str()),
            Some("recreated-id|2026-07-12T00:01:00Z|true")
        );
        sidecar_up_with_runtime(&runtime, None).expect("replacement state");

        let state = read_sidecar_state(&runtime.layout.state_file).expect("parse state");
        assert!(sidecar_state_matches_runtime(&state, &runtime));
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&runtime.layout.state_file).expect("read state"))
                .expect("state json");
        assert!(raw.get("lexical_data_dir").is_some());
        assert!(raw.get("zoekt_data_dir").is_none());
        assert!(raw.get("zoekt_http_port").is_none());
        assert!(raw.pointer("/sidecar_images/zoekt").is_none());
        assert!(raw.get("embedding_container_identity").is_none());
        assert!(
            std::fs::read_dir(root.path())
                .expect("read root")
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
    }

    #[test]
    fn sidecar_state_reads_legacy_lexical_data_dir_without_reemitting_it() {
        let root = TempDir::new().expect("root");
        let runtime = test_runtime(&root);
        let state = sidecar_up_with_runtime(&runtime, None).expect("state");
        let mut raw = serde_json::to_value(&state).expect("serialize state");
        let object = raw.as_object_mut().expect("state object");
        let lexical = object.remove("lexical_data_dir").expect("lexical path");
        object.insert("zoekt_data_dir".to_string(), lexical);

        let migrated: SidecarStateFile = serde_json::from_value(raw).expect("read legacy state");
        let rewritten = serde_json::to_value(migrated).expect("rewrite state");

        assert!(rewritten.get("lexical_data_dir").is_some());
        assert!(rewritten.get("zoekt_data_dir").is_none());
    }

    #[test]
    fn upgrade_removes_only_state_proven_owned_legacy_lexical_data() {
        let root = TempDir::new().expect("root");
        let runtime = test_runtime(&root);
        let legacy = root.path().join("zoekt");
        std::fs::create_dir_all(legacy.join("shards/generation")).expect("legacy dirs");
        std::fs::write(legacy.join("shards/generation/index"), "legacy").expect("legacy file");
        std::fs::write(
            &runtime.layout.state_file,
            serde_json::to_vec(&serde_json::json!({
                "owner": "codestory",
                "zoekt_data_dir": legacy,
            }))
            .expect("state json"),
        )
        .expect("state");

        cleanup_owned_legacy_lexical_artifacts(&runtime.layout).expect("cleanup");

        assert!(!legacy.exists());

        std::fs::create_dir_all(&legacy).expect("foreign legacy");
        std::fs::write(
            &runtime.layout.state_file,
            serde_json::to_vec(&serde_json::json!({
                "owner": "someone-else",
                "zoekt_data_dir": legacy,
            }))
            .expect("foreign state json"),
        )
        .expect("foreign state");
        cleanup_owned_legacy_lexical_artifacts(&runtime.layout).expect("skip foreign");
        assert!(legacy.exists());
    }

    fn native_embedding_launch_fixture() -> EmbeddingLaunchMetadata {
        let executable = std::env::current_exe()
            .expect("current test executable")
            .display()
            .to_string();
        EmbeddingLaunchMetadata {
            provider: "llamacpp".to_string(),
            launch_mode: "native_spawned".to_string(),
            endpoint: "http://127.0.0.1:18080/v1/embeddings".to_string(),
            pid: Some(1234),
            spawned_at_epoch_ms: Some(123),
            process_start_identity: None,
            spawn_protocol: None,
            launch_args: vec![
                "--model".to_string(),
                "C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string(),
                "--port".to_string(),
                "18080".to_string(),
            ],
            launch_fingerprint_sha256: Some("fingerprint".to_string()),
            executable_source: Some("managed_cache".to_string()),
            executable_path: Some(executable),
            model_path: Some("C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string()),
            log_path: Some("C:/cache/llama-server-native.log".to_string()),
            requested_device: None,
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_native_embedding_identity_and_snapshot_are_stable_and_compatible() -> Result<()> {
        const DOTNET_DATETIME_TICKS_AT_UNIX_EPOCH: u64 = 621_355_968_000_000_000;

        let unix_epoch_with_sub_microsecond_ticks = WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH + 8;
        let unix_epoch = WindowsFileTime {
            low_date_time: unix_epoch_with_sub_microsecond_ticks as u32,
            high_date_time: (unix_epoch_with_sub_microsecond_ticks >> 32) as u32,
        };
        assert_eq!(
            windows_datetime_ticks_from_filetime(&unix_epoch)?,
            DOTNET_DATETIME_TICKS_AT_UNIX_EPOCH
        );
        assert_eq!(windows_epoch_ms_from_filetime(&unix_epoch)?, 0);
        assert!(native_embedding_process_start_identity(0).is_err());

        let before_spawn_epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64;
        let mut child = Command::new("cmd.exe")
            .args(["/D", "/S", "/C", "ping -n 30 127.0.0.1 > nul"])
            .spawn()
            .context("spawn Windows identity test child")?;
        let result = (|| -> Result<()> {
            let first = native_embedding_process_start_identity(child.id())?
                .context("read Windows child process start identity immediately after spawn")?;
            let second = native_embedding_process_start_identity(child.id())?
                .context("repeat Windows child process start identity read")?;
            let snapshot = native_embedding_process_snapshot(child.id())?
                .context("read Windows child process snapshot immediately after spawn")?;
            let after_snapshot_epoch_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as i64;
            assert!(first.starts_with("windows:"));
            assert_eq!(first, second);
            assert!(
                snapshot
                    .started_at_epoch_ms
                    .is_some_and(|started| started >= before_spawn_epoch_ms
                        && started <= after_snapshot_epoch_ms)
            );
            Ok(())
        })();
        let _ = child.kill();
        let _ = child.wait();
        result
    }

    #[test]
    fn native_embedding_identity_accepts_matching_process_snapshot() {
        let launch = native_embedding_launch_fixture();
        let arguments = std::iter::once(
            launch
                .executable_path
                .clone()
                .expect("fixture executable path"),
        )
        .chain(launch.launch_args.clone())
        .collect();
        let snapshot = NativeEmbeddingProcessSnapshot {
            executable_path: launch.executable_path.clone(),
            command_line: None,
            arguments: Some(arguments),
            started_at_epoch_ms: Some(123),
        };

        ensure_native_embedding_process_matches(&launch, &snapshot).expect("matching snapshot");
    }

    #[test]
    fn native_embedding_identity_preserves_spaced_quoted_unicode_argv_tokens() {
        let root = TempDir::new().expect("temp dir");
        let executable_path = root.path().join("Code Story 語").join("llama-server");
        std::fs::create_dir_all(executable_path.parent().expect("fixture parent"))
            .expect("unicode fixture dir");
        std::fs::write(&executable_path, b"fixture").expect("executable fixture");
        let executable = executable_path.display().to_string();
        let model = "/tmp/Code Story 語/models/model \"alpha\".gguf".to_string();
        let mut launch = native_embedding_launch_fixture();
        launch.executable_path = Some(executable.clone());
        launch.model_path = Some(model.clone());
        launch.launch_args = vec![
            "--model".to_string(),
            model.clone(),
            "--port".to_string(),
            "18080".to_string(),
        ];
        let snapshot = NativeEmbeddingProcessSnapshot {
            executable_path: Some(executable.clone()),
            command_line: None,
            arguments: Some(vec![
                executable,
                "--model".to_string(),
                model,
                "--port".to_string(),
                "18080".to_string(),
            ]),
            started_at_epoch_ms: launch.spawned_at_epoch_ms,
        };

        ensure_native_embedding_process_matches(&launch, &snapshot)
            .expect("raw Darwin argv tokens preserve spaces, quotes, and Unicode");
    }

    #[test]
    fn native_embedding_identity_rejects_reused_pid_with_wrong_executable() {
        let launch = native_embedding_launch_fixture();
        let snapshot = NativeEmbeddingProcessSnapshot {
            executable_path: Some("C:/Windows/System32/notepad.exe".to_string()),
            command_line: Some(
                "C:/Windows/System32/notepad.exe --model C:/cache/bge-base-en-v1.5.Q8_0.gguf --port 18080"
                    .to_string(),
            ),
            arguments: None,
            started_at_epoch_ms: Some(123),
        };

        let error = ensure_native_embedding_process_matches(&launch, &snapshot)
            .expect_err("mismatched executable should fail");
        assert!(
            error.to_string().contains("executable path"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn native_embedding_identity_rejects_missing_launch_args() {
        let mut launch = native_embedding_launch_fixture();
        launch.launch_args.clear();
        let snapshot = NativeEmbeddingProcessSnapshot {
            executable_path: launch.executable_path.clone(),
            command_line: Some("\"C:/cache/llama-server.exe\" --port 18080".to_string()),
            arguments: None,
            started_at_epoch_ms: Some(123),
        };

        let error = ensure_native_embedding_process_matches(&launch, &snapshot)
            .expect_err("missing launch args should fail closed");
        assert!(
            error.to_string().contains("missing launch_args"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn native_embedding_stop_refuses_current_process_pid() {
        let mut launch = native_embedding_launch_fixture();
        launch.pid = Some(std::process::id());

        let error = stop_native_embedding_process_for_launch(&launch)
            .expect_err("current process pid should fail closed");
        assert!(
            error.to_string().contains("current CodeStory process"),
            "unexpected error: {error:?}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn native_embedding_stop_wait_retries_until_process_exits() {
        let mut checks = 0;

        wait_for_native_embedding_process_exit_with(
            1234,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_millis(0),
            || {
                checks += 1;
                Ok(checks < 3)
            },
        )
        .expect("process exits after retries");

        assert_eq!(checks, 3);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_embedding_process_snapshot_treats_zombie_child_as_not_running() -> Result<()> {
        let mut child = Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .context("spawn short-lived child")?;
        let pid = child.id();
        let process_dir = Path::new("/proc").join(pid.to_string());

        let test_result = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if matches!(
                    native_embedding_linux_process_state(&process_dir)?,
                    Some('Z')
                ) {
                    break;
                }
                if Instant::now() >= deadline {
                    bail!("child pid {pid} did not become zombie");
                }
                std::thread::sleep(Duration::from_millis(10));
            }

            assert_eq!(native_embedding_process_snapshot(pid)?, None);
            Ok(())
        })();

        let reap_result = child.wait().context("reap zombie child");
        test_result?;
        reap_result?;
        Ok(())
    }

    #[test]
    fn native_embedding_non_linux_unix_ps_parser_treats_zombie_as_not_running() {
        assert_eq!(
            native_embedding_non_linux_unix_process_snapshot_from_ps_output(
                b"Z    /tmp/llama-server --port 18080\n"
            ),
            None
        );
        assert_eq!(
            native_embedding_non_linux_unix_process_snapshot_from_ps_output(
                b"Z+   /tmp/llama-server --port 18080\n"
            ),
            None
        );
        assert_eq!(
            native_embedding_non_linux_unix_process_snapshot_from_ps_output(
                b"S    /tmp/llama-server --port 18080\n"
            ),
            Some(NativeEmbeddingProcessSnapshot {
                executable_path: None,
                command_line: Some("/tmp/llama-server --port 18080".to_string()),
                arguments: None,
                started_at_epoch_ms: None,
            })
        );
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn native_embedding_bsd_process_snapshot_includes_start_identity() -> Result<()> {
        let before_spawn = chrono::Utc::now().timestamp_millis();
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .context("spawn process identity fixture")?;
        let pid = child.id();

        let snapshot_result = native_embedding_process_snapshot(pid);
        let _ = child.kill();
        let _ = child.wait();
        let snapshot = snapshot_result?.context("live process snapshot")?;
        let started_at = snapshot
            .started_at_epoch_ms
            .context("BSD ps lstart process start identity")?;

        assert!(started_at >= before_spawn - 1_000);
        assert!(started_at <= chrono::Utc::now().timestamp_millis());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_embedding_macos_process_identity_preserves_live_argv_tokens() -> Result<()> {
        let expected = [
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 1; done",
            "CodeStory argv with spaces",
            "模型-語",
        ]
        .map(str::to_string);
        let mut child = Command::new("/bin/sh")
            .args(&expected)
            .spawn()
            .context("spawn Darwin argv identity fixture")?;
        let pid = child.id();

        let identity_result = native_embedding_macos_process_identity(pid);
        let _ = child.kill();
        let _ = child.wait();
        let (executable, arguments) = identity_result?;

        assert!(
            executable.ends_with("/sh"),
            "unexpected executable: {executable}"
        );
        assert_eq!(arguments.get(1..), Some(expected.as_slice()));
        Ok(())
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn native_embedding_identity_without_exact_start_distinguishes_live_from_dead() -> Result<()> {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd.exe");
            command.args(["/D", "/S", "/C", "ping -n 60 127.0.0.1 >NUL"]);
            command
        };
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("/bin/sleep");
            command.arg("60");
            command
        };
        let mut child = command.spawn().context("spawn missing-identity fixture")?;
        let mut launch = native_embedding_launch_fixture();
        launch.pid = Some(child.id());
        launch.process_start_identity = None;
        let live_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match native_embedding_launch_identity_status(&launch) {
                NativeEmbeddingLaunchIdentityStatus::Unverified { reason, .. }
                    if reason.contains("missing exact process start identity") =>
                {
                    break;
                }
                status if std::time::Instant::now() < live_deadline => {
                    if child.try_wait()?.is_some() {
                        anyhow::bail!(
                            "missing-identity fixture exited before live probe: {status:?}"
                        );
                    }
                    thread::sleep(std::time::Duration::from_millis(20));
                }
                status => anyhow::bail!("live process was not rejected as unverified: {status:?}"),
            }
        }

        child.kill().context("kill missing-identity fixture")?;
        child.wait().context("wait for missing-identity fixture")?;
        assert!(matches!(
            native_embedding_launch_identity_status(&launch),
            NativeEmbeddingLaunchIdentityStatus::NotRunning { .. }
        ));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_embedding_identity_rejects_exact_start_identity_mismatch() -> Result<()> {
        let args = [
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 1; done",
            "codestory-start-identity",
        ]
        .map(str::to_string);
        let spawned_at_epoch_ms = chrono::Utc::now().timestamp_millis();
        let mut child = Command::new("/bin/sh")
            .args(&args)
            .spawn()
            .context("spawn exact start-identity fixture")?;
        let mut launch = native_embedding_launch_fixture();
        launch.pid = Some(child.id());
        launch.spawned_at_epoch_ms = Some(spawned_at_epoch_ms);
        launch.process_start_identity = Some("unix:wrong-start-identity".to_string());
        launch.executable_path = Some("/bin/sh".to_string());
        launch.launch_args = args.to_vec();

        let status = native_embedding_launch_identity_status(&launch);
        let _ = child.kill();
        let _ = child.wait();

        assert!(matches!(
            status,
            NativeEmbeddingLaunchIdentityStatus::Mismatched { reason, .. }
                if reason.contains("start identity")
        ));
        Ok(())
    }

    #[test]
    fn native_embedding_identity_rejects_reused_pid_with_same_command() {
        let launch = native_embedding_launch_fixture();
        let snapshot = NativeEmbeddingProcessSnapshot {
            executable_path: launch.executable_path.clone(),
            command_line: Some(
                "\"C:/cache/llama-server.exe\" --model C:/cache/bge-base-en-v1.5.Q8_0.gguf --port 18080"
                    .to_string(),
            ),
            arguments: None,
            started_at_epoch_ms: Some(
                launch.spawned_at_epoch_ms.unwrap()
                    + NATIVE_EMBEDDING_PROCESS_START_TOLERANCE_MS
                    + 1,
            ),
        };

        let error = ensure_native_embedding_process_matches(&launch, &snapshot)
            .expect_err("same command with reused pid start time should fail");
        assert!(
            error.to_string().contains("start time"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn native_embedding_identity_status_distinguishes_mismatch_from_unverified() {
        let launch = native_embedding_launch_fixture();
        let mismatch = NativeEmbeddingProcessSnapshot {
            executable_path: Some("C:/Windows/System32/notepad.exe".to_string()),
            command_line: Some(
                "C:/Windows/System32/notepad.exe --model C:/cache/bge-base-en-v1.5.Q8_0.gguf --port 18080"
                    .to_string(),
            ),
            arguments: None,
            started_at_epoch_ms: Some(123),
        };
        assert!(matches!(
            native_embedding_process_match_status(&launch, &mismatch, 1234),
            NativeEmbeddingLaunchIdentityStatus::Mismatched { .. }
        ));

        let unverified = NativeEmbeddingProcessSnapshot {
            executable_path: launch.executable_path.clone(),
            command_line: None,
            arguments: None,
            started_at_epoch_ms: Some(123),
        };
        assert!(matches!(
            native_embedding_process_match_status(&launch, &unverified, 1234),
            NativeEmbeddingLaunchIdentityStatus::Unverified { .. }
        ));
    }

    #[test]
    fn status_reports_dead_endpoint_before_stale_manifest() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = project_id_for_root(project.path());
        let hash = "deadbeefcafebabe";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.projection_count = Some(10);
            manifest.symbol_doc_count = Some(10);
            manifest.dense_projection_count = Some(10);
            manifest.dense_reason_counts_json = Some("{\"public_api\":10}".into());
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        let report = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("sidecar status report");

        assert_eq!(report.retrieval_mode, "full");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .starts_with("embedding_runtime_unavailable:"),
            "live infrastructure failure must win before manifest classification: {report:?}"
        );
        assert_eq!(
            report.repair.as_ref().map(|repair| repair.reason.as_str()),
            Some("embedding_runtime_unavailable")
        );
    }

    #[test]
    fn strict_readiness_rejects_stored_doc_backend_mismatch() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = project_id_for_root(project.path());
        let hash = "badc0ffee0ddf00d";
        let mut manifest = retrieval_manifest_fixture(&project_id, hash);
        manifest.projection_count = Some(1);
        manifest.dense_projection_count = Some(1);
        manifest.dense_reason_counts_json = Some("{\"public_api\":1}".into());

        let mut storage = Store::open(&storage_path).expect("open db");
        storage
            .insert_nodes_batch(&[Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "do_work".into(),
                ..Default::default()
            }])
            .expect("node");
        storage
            .upsert_llm_symbol_docs_batch(&[semantic_doc_with_backend("onnx")])
            .expect("semantic doc");

        let reason = strict_readiness_unavailable_reason(
            project.path(),
            &storage_path,
            &storage,
            &project_id,
            &manifest,
        )
        .expect("strict readiness")
        .expect("backend mismatch should degrade");

        assert!(
            reason.contains("sidecar_symbol_doc_embedding_backend_changed"),
            "unexpected reason: {reason}"
        );
        assert!(
            reason.contains("stored=onnx current=llamacpp"),
            "unexpected reason: {reason}"
        );
    }

    #[test]
    fn strict_readiness_rejects_incomplete_run_before_manifest_fast_paths() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = project_id_for_root(project.path());
        let manifest = retrieval_manifest_fixture(&project_id, "current");
        let storage = Store::open(&storage_path).expect("open db");
        storage
            .begin_incremental_run()
            .expect("mark incomplete run");

        let reason = strict_readiness_unavailable_reason(
            project.path(),
            &storage_path,
            &storage,
            &project_id,
            &manifest,
        )
        .expect("strict readiness")
        .expect("incomplete run must degrade");

        assert_eq!(reason, "incomplete_incremental_index_run");
    }

    #[test]
    fn strict_readiness_distinguishes_parser_partial_from_file_error() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("lib.rs");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let mut storage = Store::open(&storage_path).expect("open db");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path.clone(),
                language: "rust".into(),
                modification_time: indexed_mtime,
                indexed: true,
                complete: false,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert incomplete file");
        let input = crate::index::compute_sidecar_input_fingerprint(
            &storage,
            &storage_path,
            project.path(),
            &project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("sidecar input");
        let mut manifest = retrieval_manifest_fixture(&project_id, &input.hash);
        manifest.built_at_epoch_ms = indexed_mtime;
        manifest.projection_count = Some(input.projection_count);
        manifest.symbol_doc_count = Some(input.symbol_doc_count);
        manifest.dense_projection_count = Some(input.dense_projection_count);
        manifest.semantic_policy_version = input.semantic_policy_version;
        manifest.graph_artifact_hash = Some(input.graph_artifact_hash);
        manifest.dense_reason_counts_json = Some(input.dense_reason_counts_json);
        storage
            .upsert_retrieval_index_manifest(&manifest)
            .expect("manifest");

        validate_strict_sidecar_readiness(project.path(), &storage_path, &storage)
            .expect("parser coverage must not masquerade as an interrupted transaction");

        storage
            .insert_error(&ErrorInfo {
                message: "read failed".into(),
                file_id: Some(NodeId(1)),
                line: None,
                column: None,
                is_fatal: true,
                index_step: IndexStep::Indexing,
            })
            .expect("file error");
        let reason = strict_readiness_unavailable_reason(
            project.path(),
            &storage_path,
            &storage,
            &project_id,
            &manifest,
        )
        .expect("strict readiness")
        .expect("file error must degrade");
        assert_eq!(
            reason,
            format!(
                "indexed_file_error_retry_required: {}",
                source_path.display()
            )
        );
    }

    #[test]
    fn status_rejects_manifest_when_live_indexed_file_changes_or_is_removed() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let runtime_root = TempDir::new().expect("runtime");
        let (runtime, embedding_server) = live_embedding_runtime(&runtime_root, 2);
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "feedfacecafebeef";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&source_path, "pub fn indexed() -> usize { 1 }\n").expect("mutate source");
        let changed =
            strict_sidecar_status_for_runtime(project.path(), Some(&storage_path), runtime.clone())
                .expect("changed sidecar status");
        assert_eq!(changed.retrieval_mode, "full");
        assert!(!changed.is_live_ready());
        assert!(
            changed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "changed indexed file should make sidecar status fail closed: {changed:?}"
        );

        std::fs::remove_file(&source_path).expect("remove source");
        let removed =
            strict_sidecar_status_for_runtime(project.path(), Some(&storage_path), runtime)
                .expect("removed sidecar status");
        assert_eq!(removed.retrieval_mode, "full");
        assert!(!removed.is_live_ready());
        assert!(
            removed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexed_file_removed_after_sidecar_manifest"),
            "removed indexed file should make sidecar status fail closed: {removed:?}"
        );
        embedding_server.join().expect("embedding test server");
    }

    #[test]
    fn lightweight_status_does_not_scan_live_indexable_inventory() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let runtime_root = TempDir::new().expect("runtime");
        let (runtime, embedding_server) = live_embedding_runtime(&runtime_root, 1);
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "1ead1e55cafebeef";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let lightweight =
            sidecar_status(project.path(), Some(&storage_path)).expect("lightweight status");
        let strict =
            strict_sidecar_status_for_runtime(project.path(), Some(&storage_path), runtime)
                .expect("strict status");

        assert!(
            !lightweight
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "lightweight status should leave live inventory scans to strict callers: {lightweight:?}"
        );
        assert!(
            strict
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar_manifest_stale"),
            "strict status should fail closed on new indexable files: {strict:?}"
        );
        embedding_server.join().expect("embedding test server");
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_indexable_file_is_added() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let runtime_root = TempDir::new().expect("runtime");
        let (runtime, embedding_server) = live_embedding_runtime(&runtime_root, 1);
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "ba5eba11cafebeef";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let report =
            strict_sidecar_status_for_runtime(project.path(), Some(&storage_path), runtime)
                .expect("strict status");

        assert_eq!(report.retrieval_mode, "full");
        assert!(!report.is_live_ready());
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new indexable file should make strict status fail closed: {report:?}"
        );
        embedding_server.join().expect("embedding test server");
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_parser_backed_language_file_is_added() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let runtime_root = TempDir::new().expect("runtime");
        let (runtime, embedding_server) = live_embedding_runtime(&runtime_root, 1);
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "ba5eba11feedface";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("Routes.kt"),
            "fun routeUsers() = Unit\n",
        )
        .expect("write kotlin source");

        let report =
            strict_sidecar_status_for_runtime(project.path(), Some(&storage_path), runtime)
                .expect("strict status");

        assert_eq!(report.retrieval_mode, "full");
        assert!(!report.is_live_ready());
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new registry-backed parser file should make strict status fail closed: {report:?}"
        );
        embedding_server.join().expect("embedding test server");
    }

    #[test]
    fn strict_readiness_accepts_markdown_covered_by_sidecar_fingerprint() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        std::fs::write(project.path().join("AGENTS.md"), "# Agent guidance\n")
            .expect("write markdown");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());

        let mut storage = Store::open(&storage_path).expect("open db");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path.clone(),
                language: "rust".into(),
                modification_time: indexed_mtime,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            })
            .expect("insert indexed file");
        let input = crate::index::compute_sidecar_input_fingerprint(
            &storage,
            &storage_path,
            project.path(),
            &project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("sidecar input");
        storage
            .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                project_id: project_id.clone(),
                lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
                qdrant_collection: sidecar_qdrant_collection(&project_id, &input.hash),
                scip_revision: Some("graph-test".into()),
                built_at_epoch_ms: indexed_mtime,
                disk_bytes: None,
                degraded_modes_json: "[]".into(),
                embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                embedding_dim: Some(768),
                sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
                sidecar_input_hash: Some(input.hash.clone()),
                sidecar_generation: Some(sidecar_generation_id(&project_id, &input.hash)),
                projection_count: Some(input.projection_count),
                symbol_doc_count: Some(input.symbol_doc_count),
                dense_projection_count: Some(input.dense_projection_count),
                semantic_policy_version: input.semantic_policy_version.clone(),
                graph_artifact_hash: Some(input.graph_artifact_hash.clone()),
                dense_reason_counts_json: Some(input.dense_reason_counts_json.clone()),
                precise_semantic_import_status: None,
                precise_semantic_import_reason: None,
                precise_semantic_import_revision: None,
                precise_semantic_import_producer: None,
            })
            .expect("manifest");

        validate_strict_sidecar_readiness(project.path(), &storage_path, &storage)
            .expect("markdown already covered by sidecar input should not look stale");

        std::fs::write(project.path().join("README.md"), "# New docs\n").expect("write new docs");
        let stale = validate_strict_sidecar_readiness(project.path(), &storage_path, &storage)
            .expect_err("new sidecar-only docs should stale the manifest");
        assert!(
            stale.to_string().contains("sidecar_input_hash_changed"),
            "docs-only sidecar drift should report input-hash drift, got: {stale:?}"
        );
    }

    fn live_mtime_millis(path: &Path) -> i64 {
        std::fs::metadata(path)
            .expect("metadata")
            .modified()
            .expect("modified")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("mtime since epoch")
            .as_millis()
            .min(i64::MAX as u128) as i64
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests that mutate process environment hold crate::test_support::env_lock().
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests that mutate process environment hold crate::test_support::env_lock().
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: tests that mutate process environment hold crate::test_support::env_lock().
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}
