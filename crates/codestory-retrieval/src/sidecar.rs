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
use crate::index::{compute_sidecar_input_fingerprint_for_runtime, sidecar_project_id_for_root};
use anyhow::{Context, Result, bail};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext,
};
use codestory_store::Store;
use codestory_workspace::{RefreshInputs, WorkspaceManifest};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::fs;
use std::path::Path;
use std::process::Command;
#[cfg(not(windows))]
use std::time::{Duration, Instant};

const NATIVE_EMBEDDING_PROCESS_START_TOLERANCE_MS: i64 = 5 * 60 * 1000;
const LEGACY_ZOEKT_CLEANUP_ENTRY_LIMIT: usize = 4_096;
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
    pub project_identity: Option<codestory_workspace::ProjectIdentityV2>,
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
    #[serde(default = "default_sidecar_image_pins")]
    pub sidecar_images: SidecarImagePins,
    #[serde(alias = "zoekt_data_dir")]
    pub lexical_data_dir: String,
    pub qdrant_data_dir: String,
    pub scip_artifacts_root: String,
    #[serde(default)]
    pub compose_file: Option<String>,
    #[serde(default)]
    pub cleanup_command: String,
    pub started_at_epoch_ms: i64,
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
    let embedding_launch = reusable_embedding_launch_from_state(runtime);
    sidecar_up_with_runtime_and_launch_metadata(runtime, compose_file, embedding_launch)
}

pub(crate) fn sidecar_up_with_runtime_and_launch_metadata(
    runtime: &SidecarRuntimeConfig,
    compose_file: Option<&Path>,
    embedding_launch: Option<EmbeddingLaunchMetadata>,
) -> Result<SidecarStateFile> {
    runtime.ensure_ports_allocated()?;
    let layout = &runtime.layout;
    cleanup_owned_legacy_zoekt(layout)?;
    layout.ensure_data_dirs()?;
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let state = SidecarStateFile {
        project_identity: runtime.project_identity.clone(),
        owner: "codestory".into(),
        profile: runtime.profile.as_str().into(),
        namespace: runtime.namespace.clone(),
        compose_project: runtime.compose_project.clone(),
        run_id: runtime.run_id.clone(),
        qdrant_http_port: layout.qdrant_http_port,
        qdrant_grpc_port: layout.qdrant_grpc_port,
        embed_http_port: runtime.embed_http_port,
        embed_url: SidecarLayout::embed_base_url(runtime.embed_http_port),
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
        sidecar_images: default_sidecar_image_pins(),
        lexical_data_dir: layout.lexical_data_dir.display().to_string(),
        qdrant_data_dir: layout.qdrant_data_dir.display().to_string(),
        scip_artifacts_root: layout.scip_artifacts_root.display().to_string(),
        compose_file: compose_file.map(|path| path.display().to_string()),
        cleanup_command: runtime.cleanup_command.clone(),
        started_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
    };
    let json = serde_json::to_vec_pretty(&state).context("serialize sidecar state")?;
    codestory_workspace::atomic_file::write_bytes_atomic(
        &layout.state_file,
        "retrieval-sidecars",
        &json,
    )
    .context("write retrieval-sidecars.json")?;
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

fn cleanup_owned_legacy_zoekt(layout: &SidecarLayout) -> Result<()> {
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
    let mut remaining = LEGACY_ZOEKT_CLEANUP_ENTRY_LIMIT;
    if !remove_tree_bounded(&legacy_root, &mut remaining)? {
        eprintln!(
            "CodeStory legacy Zoekt cleanup reached its {}-entry limit; remaining owned data will be retried",
            LEGACY_ZOEKT_CLEANUP_ENTRY_LIMIT
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
            .with_context(|| format!("remove legacy Zoekt artifact {}", path.display()))?;
        return Ok(true);
    }
    let mut complete = true;
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("read legacy Zoekt directory {}", path.display()))?
    {
        if !remove_tree_bounded(&entry?.path(), remaining)? {
            complete = false;
            break;
        }
    }
    if complete {
        std::fs::remove_dir(path)
            .with_context(|| format!("remove empty legacy Zoekt directory {}", path.display()))?;
    }
    Ok(complete)
}

/// Returns true when a sidecar state file matches the runtime identity used for reuse/handoff.
pub fn sidecar_state_matches_runtime(
    state: &SidecarStateFile,
    runtime: &SidecarRuntimeConfig,
) -> bool {
    state.owner == "codestory"
        && state.namespace == runtime.namespace
        && state.compose_project == runtime.compose_project
        && state.profile == runtime.profile.as_str()
        && state.run_id.as_deref() == runtime.run_id.as_deref()
        && state.embed_http_port == runtime.embed_http_port
        && state.embed_url == SidecarLayout::embed_base_url(runtime.embed_http_port)
        && state
            .project_identity
            .as_ref()
            .is_none_or(|state_identity| {
                runtime
                    .project_identity
                    .as_ref()
                    .is_some_and(|runtime_identity| {
                        state_identity.workspace_id == runtime_identity.workspace_id
                    })
            })
}

fn reusable_embedding_launch_from_state(
    runtime: &SidecarRuntimeConfig,
) -> Option<EmbeddingLaunchMetadata> {
    let state = read_sidecar_state(&runtime.layout.state_file)?;
    if !sidecar_state_matches_runtime(&state, runtime) {
        return None;
    }
    state.embedding_launch
}

pub fn sidecar_down() -> Result<()> {
    sidecar_down_for_runtime(&SidecarRuntimeConfig::local())
}

pub fn sidecar_down_for_project(project_root: &Path, profile: SidecarProfile) -> Result<()> {
    sidecar_down_for_runtime(&sidecar_runtime_for_project(project_root, profile))
}

pub fn sidecar_down_for_runtime(runtime: &SidecarRuntimeConfig) -> Result<()> {
    let layout = &runtime.layout;
    if layout.state_file.exists() {
        if let Some(state) = std::fs::read_to_string(&layout.state_file)
            .ok()
            .and_then(|contents| serde_json::from_str::<SidecarStateFile>(&contents).ok())
            && state.owner == "codestory"
            && state.namespace == runtime.namespace
        {
            if runtime.profile == SidecarProfile::Agent {
                crate::compose::docker_compose_down_for_state(&state)?;
            }
            stop_native_embedding_process_for_state(&state)?;
        }
        std::fs::remove_file(&layout.state_file).context("remove retrieval-sidecars.json")?;
    }
    Ok(())
}

fn stop_native_embedding_process_for_state(state: &SidecarStateFile) -> Result<()> {
    let Some(launch) = state.embedding_launch.as_ref() else {
        return Ok(());
    };
    stop_native_embedding_process_for_launch(launch)
}

pub(crate) fn stop_native_embedding_process_for_launch(
    launch: &EmbeddingLaunchMetadata,
) -> Result<()> {
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
    native_embedding_process_match_status(launch, &snapshot, pid)
}

fn stop_native_embedding_process(pid: u32, launch: &EmbeddingLaunchMetadata) -> Result<()> {
    if pid == 0 {
        bail!("identity_unverified: native embedding pid is zero");
    }
    if pid == std::process::id() {
        bail!("identity_unverified: native embedding pid {pid} is the current CodeStory process");
    }
    let Some(snapshot) = native_embedding_process_snapshot(pid)? else {
        return Ok(());
    };
    ensure_native_embedding_process_matches(launch, &snapshot)
        .with_context(|| format!("identity_unverified: native embedding pid {pid}"))?;
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
    started_at_epoch_ms: Option<i64>,
}

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

    let Some(command_line) = snapshot.command_line.as_deref() else {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: "live native embedding process has no command line for launch-arg validation"
                .to_string(),
        };
    };
    if launch.launch_args.is_empty() {
        return NativeEmbeddingLaunchIdentityStatus::Unverified {
            pid: Some(pid),
            reason: "recorded native embedding launch is missing launch_args".to_string(),
        };
    }
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
    if let (Some(expected), Some(actual)) =
        (launch.spawned_at_epoch_ms, snapshot.started_at_epoch_ms)
        && expected.abs_diff(actual) > NATIVE_EMBEDDING_PROCESS_START_TOLERANCE_MS as u64
    {
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
    normalized_identity_path(left) == normalized_identity_path(right)
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
        #[serde(rename = "StartedAtEpochMs")]
        started_at_epoch_ms: Option<i64>,
    }

    let script = format!(
        "$p=Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}'; if ($null -eq $p) {{ exit 2 }}; $started=[int64](([Management.ManagementDateTimeConverter]::ToDateTime($p.CreationDate).ToUniversalTime() - [datetime]'1970-01-01').TotalMilliseconds); [pscustomobject]@{{ExecutablePath=$p.ExecutablePath;CommandLine=$p.CommandLine;StartedAtEpochMs=$started}} | ConvertTo-Json -Compress"
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
        started_at_epoch_ms: info.started_at_epoch_ms,
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
        snapshot.started_at_epoch_ms = native_embedding_process_started_at_epoch_ms(pid);
    }
    Ok(snapshot)
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
        started_at_epoch_ms: None,
    })
}

#[cfg(not(windows))]
fn native_embedding_process_started_at_epoch_ms(pid: u32) -> Option<i64> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "etimes="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let elapsed_secs = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<i64>()
        .ok()?;
    Some(chrono::Utc::now().timestamp_millis() - elapsed_secs.saturating_mul(1000))
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
    if runtime.embedding.endpoint_origin != crate::config::EmbeddingEndpointOrigin::ManagedSidecar {
        let probe = crate::embeddings::probe_product_embedding_runtime_for_runtime(&runtime);
        tracing::debug!(
            reachable = probe.reachable,
            detail = %probe.detail,
            "Probed explicit embedding endpoint for retained project runtime"
        );
    }
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(&runtime);
    let project_id = sidecar_project_id_for_root(project_root);
    let manifest = if let Some(path) = storage_path.filter(|path| path.exists()) {
        let storage = Store::open(path).context("open storage for manifest")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
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
            return Ok(attach_status_ownership(
                enrich_status_with_semantic_doc_stats(
                    attach_repair_hint(
                        attach_manifest_contract(
                            unavailable_status_report_with_embedding_device(
                                format!("sidecar_manifest_stale: {reason}"),
                                Some(manifest.clone()),
                                &embedding_device,
                            ),
                            project_root,
                        ),
                        project_root,
                        Some(&runtime),
                    ),
                    &storage,
                ),
                &runtime,
            ));
        }
        if let Some(manifest) = manifest.as_ref()
            && let Some(reason) =
                manifest_unavailable_reason_for_runtime(&project_id, &storage, manifest, &runtime)
        {
            return Ok(attach_status_ownership(
                enrich_status_with_semantic_doc_stats(
                    attach_repair_hint(
                        attach_manifest_contract(
                            unavailable_status_report_with_embedding_device(
                                reason,
                                Some(manifest.clone()),
                                &embedding_device,
                            ),
                            project_root,
                        ),
                        project_root,
                        Some(&runtime),
                    ),
                    &storage,
                ),
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
        return Ok(attach_status_ownership(
            enrich_status_with_semantic_doc_stats(
                attach_repair_hint(
                    attach_manifest_contract(report, project_root),
                    project_root,
                    Some(&runtime),
                ),
                &storage,
            ),
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

fn attach_status_ownership(
    mut report: RetrievalStatusReport,
    runtime: &SidecarRuntimeConfig,
) -> RetrievalStatusReport {
    report.ownership = Some(runtime.ownership());
    report.query_embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    if let Some(state) = read_sidecar_state(&runtime.layout.state_file) {
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

pub(crate) fn validate_strict_sidecar_readiness(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
) -> Result<()> {
    let project_id = sidecar_project_id_for_root(project_root);
    let Some(manifest) = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load retrieval manifest for strict readiness")?
    else {
        return Ok(());
    };
    if let Some(reason) = strict_readiness_unavailable_reason(
        project_root,
        storage_path,
        storage,
        &project_id,
        &manifest,
    )? {
        anyhow::bail!("sidecar_manifest_stale: {reason}");
    }
    Ok(())
}

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
    use tempfile::TempDir;

    fn test_runtime(root: &TempDir) -> SidecarRuntimeConfig {
        SidecarRuntimeConfig {
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
        }
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
        runtime.project_identity = Some(codestory_workspace::project_identity_v2(root.path()));
        let launch = EmbeddingLaunchMetadata {
            provider: "llamacpp".to_string(),
            launch_mode: "native_spawned".to_string(),
            endpoint: "http://127.0.0.1:18080/v1/embeddings".to_string(),
            pid: Some(1234),
            spawned_at_epoch_ms: Some(123),
            launch_args: vec!["--port".to_string(), "18080".to_string()],
            launch_fingerprint_sha256: Some("fingerprint".to_string()),
            executable_source: Some("managed_cache".to_string()),
            executable_path: Some("C:/cache/llama-server".to_string()),
            model_path: Some("C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string()),
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
    fn sidecar_state_reads_legacy_zoekt_data_dir_without_reemitting_it() {
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
    fn upgrade_removes_only_state_proven_owned_legacy_zoekt_data() {
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

        cleanup_owned_legacy_zoekt(&runtime.layout).expect("cleanup");

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
        cleanup_owned_legacy_zoekt(&runtime.layout).expect("skip foreign");
        assert!(legacy.exists());
    }

    fn native_embedding_launch_fixture() -> EmbeddingLaunchMetadata {
        EmbeddingLaunchMetadata {
            provider: "llamacpp".to_string(),
            launch_mode: "native_spawned".to_string(),
            endpoint: "http://127.0.0.1:18080/v1/embeddings".to_string(),
            pid: Some(1234),
            spawned_at_epoch_ms: Some(123),
            launch_args: vec![
                "--model".to_string(),
                "C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string(),
                "--port".to_string(),
                "18080".to_string(),
            ],
            launch_fingerprint_sha256: Some("fingerprint".to_string()),
            executable_source: Some("managed_cache".to_string()),
            executable_path: Some("C:/cache/llama-server.exe".to_string()),
            model_path: Some("C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string()),
            requested_device: None,
        }
    }

    #[test]
    fn native_embedding_identity_accepts_matching_process_snapshot() {
        let launch = native_embedding_launch_fixture();
        let snapshot = NativeEmbeddingProcessSnapshot {
            executable_path: Some("C:\\cache\\llama-server.exe".to_string()),
            command_line: Some(
                "\"C:\\cache\\llama-server.exe\" --model C:/cache/bge-base-en-v1.5.Q8_0.gguf --port 18080"
                    .to_string(),
            ),
            started_at_epoch_ms: Some(123),
        };

        ensure_native_embedding_process_matches(&launch, &snapshot).expect("matching snapshot");
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
            executable_path: Some("C:/cache/llama-server.exe".to_string()),
            command_line: Some("\"C:/cache/llama-server.exe\" --port 18080".to_string()),
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
                started_at_epoch_ms: None,
            })
        );
    }

    #[test]
    fn native_embedding_identity_rejects_reused_pid_with_same_command() {
        let launch = native_embedding_launch_fixture();
        let snapshot = NativeEmbeddingProcessSnapshot {
            executable_path: Some("C:/cache/llama-server.exe".to_string()),
            command_line: Some(
                "\"C:/cache/llama-server.exe\" --model C:/cache/bge-base-en-v1.5.Q8_0.gguf --port 18080"
                    .to_string(),
            ),
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
            started_at_epoch_ms: Some(123),
        };
        assert!(matches!(
            native_embedding_process_match_status(&launch, &mismatch, 1234),
            NativeEmbeddingLaunchIdentityStatus::Mismatched { .. }
        ));

        let unverified = NativeEmbeddingProcessSnapshot {
            executable_path: Some("C:/cache/llama-server.exe".to_string()),
            command_line: None,
            started_at_epoch_ms: Some(123),
        };
        assert!(matches!(
            native_embedding_process_match_status(&launch, &unverified, 1234),
            NativeEmbeddingLaunchIdentityStatus::Unverified { .. }
        ));
    }

    #[test]
    fn status_rejects_stale_manifest_before_component_probes() {
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

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar_manifest_stale")
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
        let changed = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("changed sidecar status");
        assert_eq!(changed.retrieval_mode, "unavailable");
        assert!(
            changed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "changed indexed file should make sidecar status fail closed: {changed:?}"
        );

        std::fs::remove_file(&source_path).expect("remove source");
        let removed = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("removed sidecar status");
        assert_eq!(removed.retrieval_mode, "unavailable");
        assert!(
            removed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexed_file_removed_after_sidecar_manifest"),
            "removed indexed file should make sidecar status fail closed: {removed:?}"
        );
    }

    #[test]
    fn lightweight_status_does_not_scan_live_indexable_inventory() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
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
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

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
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_indexable_file_is_added() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
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
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new indexable file should make strict status fail closed: {report:?}"
        );
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_parser_backed_language_file_is_added() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
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
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new registry-backed parser file should make strict status fail closed: {report:?}"
        );
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
