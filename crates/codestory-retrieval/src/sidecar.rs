use crate::config::{SidecarLayout, SidecarProfile, SidecarRuntimeConfig};
use crate::generation::{
    SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED, manifest_has_current_sidecar_contract,
    manifest_staleness_reason_for_runtime, manifest_unavailable_reason_for_runtime,
};
use crate::health::{
    EmbeddingLaunchMetadata, RetrievalStatusReport, attach_manifest_contract, attach_repair_hint,
    probe_sidecar_health_for_runtime, unavailable_status_report_with_embedding_device,
};
use crate::index::{compute_sidecar_input_fingerprint_for_runtime, sidecar_project_id_for_runtime};
#[cfg(not(target_os = "linux"))]
use crate::process_identity::bounded_process_command_output;
use crate::process_identity::{
    ProcessOwnerState, ProcessStartProbe, native_embedding_process_start_identity,
    probe_process_start_identity, process_owner_state, process_started_at_epoch_ms,
};
#[cfg(all(not(windows), not(target_os = "linux")))]
use crate::process_identity::{PsProbeOutputStatus, classify_ps_probe_output};
use anyhow::{Context, Result, bail};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext,
};
use codestory_store::Store;
use codestory_workspace::{RefreshInputs, WorkspaceManifest};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::fs;
use std::path::{Path, PathBuf};
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

const NATIVE_EMBEDDING_PROCESS_START_TOLERANCE_MS: i64 = 5 * 1000;
#[cfg(target_os = "macos")]
const MACOS_CTL_KERN: std::ffi::c_int = 1;
#[cfg(target_os = "macos")]
const MACOS_KERN_PROCARGS2: std::ffi::c_int = 49;
#[cfg(target_os = "macos")]
const MACOS_PROC_PIDPATH_MAX_SIZE: usize = 4 * 1024;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
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
    pub lexical_data_dir: String,
    pub semantic_data_dir: String,
    pub scip_artifacts_root: String,
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

pub fn attached_native_embedding_state_paths(
    cache_root: &Path,
    owner_state_file: &Path,
    launch: &EmbeddingLaunchMetadata,
) -> Result<Vec<PathBuf>> {
    let mut candidates = vec![cache_root.join(crate::config::SIDECAR_STATE_FILE_V3)];
    let agent_root = cache_root.join("sidecars");
    match std::fs::read_dir(&agent_root) {
        Ok(entries) => {
            for entry in entries {
                let entry =
                    entry.with_context(|| format!("read {} entry", agent_root.display()))?;
                if entry
                    .file_type()
                    .with_context(|| format!("inspect {}", entry.path().display()))?
                    .is_dir()
                {
                    candidates.push(entry.path().join(crate::config::SIDECAR_STATE_FILE_V3));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", agent_root.display()));
        }
    }
    let mut attachments = Vec::new();
    for path in candidates {
        if codestory_workspace::same_workspace_path(&path, owner_state_file) {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", path.display()));
            }
        };
        let state = serde_json::from_str::<SidecarStateFile>(&raw)
            .with_context(|| format!("parse {}", path.display()))?;
        if state.owner == "codestory"
            && state.embedding_launch_ownership == EmbeddingLaunchOwnership::Attached
            && state.embedding_launch.as_ref() == Some(launch)
        {
            attachments.push(path);
        }
    }
    Ok(attachments)
}

pub fn sidecar_up_with_runtime_preserving_launch(
    runtime: &SidecarRuntimeConfig,
) -> Result<SidecarStateFile> {
    let (embedding_launch, embedding_launch_ownership) =
        reusable_embedding_launch_from_state(runtime).map_or(
            (None, EmbeddingLaunchOwnership::Owner),
            |(launch, ownership)| (Some(launch), ownership),
        );
    sidecar_up_with_runtime_and_launch_metadata_and_ownership(
        runtime,
        embedding_launch,
        embedding_launch_ownership,
    )
}

#[cfg(test)]
pub(crate) fn sidecar_up_with_runtime_and_launch_metadata(
    runtime: &SidecarRuntimeConfig,
    embedding_launch: Option<EmbeddingLaunchMetadata>,
) -> Result<SidecarStateFile> {
    sidecar_up_with_runtime_and_launch_metadata_and_ownership(
        runtime,
        embedding_launch,
        EmbeddingLaunchOwnership::Owner,
    )
}

pub(crate) fn sidecar_up_with_runtime_and_launch_metadata_and_ownership(
    runtime: &SidecarRuntimeConfig,
    embedding_launch: Option<EmbeddingLaunchMetadata>,
    embedding_launch_ownership: EmbeddingLaunchOwnership,
) -> Result<SidecarStateFile> {
    runtime.ensure_ports_allocated()?;
    let layout = &runtime.layout;
    layout.ensure_data_dirs()?;
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let ownership = runtime.ownership();
    let embedding_endpoint_fingerprint = runtime.embedding_endpoint_fingerprint()?;
    let state = SidecarStateFile {
        project_identity: runtime.project_identity.clone(),
        owner: "codestory".into(),
        profile: runtime.profile.as_str().into(),
        namespace: runtime.namespace.clone(),
        run_id: runtime.run_id.clone(),
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
        lexical_data_dir: layout.lexical_data_dir.display().to_string(),
        semantic_data_dir: layout.semantic_data_dir.display().to_string(),
        scip_artifacts_root: layout.scip_artifacts_root.display().to_string(),
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
        && state.profile == runtime.profile.as_str()
        && state.run_id.as_deref() == runtime.run_id.as_deref()
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

pub fn sidecar_down_for_runtime(runtime: &SidecarRuntimeConfig) -> Result<()> {
    sidecar_down_for_runtime_inner(runtime)
}

pub fn sidecar_down_after_failed_bootstrap_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> Result<()> {
    sidecar_down_for_runtime_inner(runtime)
}

fn sidecar_down_for_runtime_inner(runtime: &SidecarRuntimeConfig) -> Result<()> {
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
        stop_native_embedding_process_for_state(&state)?;
        std::fs::remove_file(&layout.state_file).context("remove versioned sidecar state")?;
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
    let final_start_probe = probe_process_start_identity(pid);
    match &final_start_probe {
        ProcessStartProbe::Running {
            start_identity: actual,
        } if start_identity_before != *actual => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: format!(
                    "native embedding process start identity changed during snapshot: before={start_identity_before}, after={actual}"
                ),
            };
        }
        ProcessStartProbe::Running { .. }
            if process_owner_state(&final_start_probe, Some(expected_start_identity))
                == ProcessOwnerState::Matching => {}
        ProcessStartProbe::Running {
            start_identity: actual,
        } => {
            return NativeEmbeddingLaunchIdentityStatus::Mismatched {
                pid,
                reason: format!(
                    "live process start identity does not match recorded native embedding launch: expected {expected_start_identity}, got {actual}"
                ),
            };
        }
        ProcessStartProbe::NotRunning => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: "live native embedding process start identity is unavailable".to_string(),
            };
        }
        ProcessStartProbe::Unknown { reason } => {
            return NativeEmbeddingLaunchIdentityStatus::Unverified {
                pid: Some(pid),
                reason: format!("query live native embedding process start identity: {reason}"),
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
fn native_embedding_process_snapshot(pid: u32) -> Result<Option<NativeEmbeddingProcessSnapshot>> {
    #[derive(Deserialize)]
    struct WindowsProcessInfo {
        #[serde(rename = "ExecutablePath")]
        executable_path: Option<String>,
        #[serde(rename = "CommandLine")]
        command_line: Option<String>,
    }

    let Some(started_at_epoch_ms) = process_started_at_epoch_ms(pid)? else {
        return Ok(None);
    };
    let script = format!(
        "$p=Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}'; if ($null -eq $p) {{ exit 2 }}; [pscustomobject]@{{ExecutablePath=$p.ExecutablePath;CommandLine=$p.CommandLine}} | ConvertTo-Json -Compress"
    );
    let mut command = Command::new("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let output = bounded_process_command_output(&mut command)
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
        started_at_epoch_ms: Some(started_at_epoch_ms),
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
    let started_at_epoch_ms = process_started_at_epoch_ms(pid).ok().flatten();
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
    let mut command = Command::new("ps");
    command.args(["-p", &pid.to_string(), "-o", "state=", "-o", "command="]);
    let output = bounded_process_command_output(&mut command)
        .with_context(|| format!("query native embedding pid {pid}"))?;
    match classify_ps_probe_output(&output)? {
        PsProbeOutputStatus::Success => {}
        PsProbeOutputStatus::ProcessMissing => return Ok(None),
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
        snapshot.started_at_epoch_ms = process_started_at_epoch_ms(pid).ok().flatten();
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
        SidecarRuntimeConfig::for_project_profile(Some(project_root), profile),
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
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
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
    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    let current_input = compute_sidecar_input_fingerprint_for_runtime(
        storage,
        storage_path,
        project_root,
        project_id,
        &embedding_backend,
        embedding_dim,
        runtime,
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
