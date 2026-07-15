use crate::config::{EmbeddingServerLaunchMode, SidecarRuntimeConfig, user_cache_root};
use crate::health::{InfrastructureHealth, probe_infrastructure_health};
use crate::sidecar::{
    EmbeddingLaunchOwnership, SidecarStateFile,
    sidecar_up_with_runtime_and_launch_metadata_and_ownership,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
#[cfg(windows)]
use std::io;
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const NATIVE_EMBEDDING_PORT_BIND_FAILED_REASON: &str = "native_embedding_port_bind_failed";
pub const NATIVE_EMBEDDING_DARWIN_EXEC_GATE_PROTOCOL: &str = "darwin_exec_gate_v1";
const NATIVE_EMBEDDING_DARWIN_EXEC_GATE_TOKEN: &str = "codestory-native-launch-v1";
#[cfg(target_os = "macos")]
const NATIVE_EMBEDDING_DARWIN_EXEC_GATE_SCRIPT: &str = "IFS= read -r gate || exit 125; [ \"$gate\" = codestory-native-launch-v1 ] || exit 126; exec \"$@\"";
const MANAGED_LLAMA_EXTRACTED_MARKER: &str = ".codestory-extracted";
#[cfg(windows)]
const WINDOWS_DETACHED_PROCESS: u32 = 0x00000008;
#[cfg(windows)]
const WINDOWS_CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
#[cfg(windows)]
const WINDOWS_CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
#[cfg(windows)]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x08000000;
#[cfg(windows)]
const WINDOWS_HANDLE_FLAG_INHERIT: u32 = 0x00000001;
#[cfg(windows)]
const WINDOWS_STD_INPUT_HANDLE: u32 = -10_i32 as u32;
#[cfg(windows)]
const WINDOWS_STD_OUTPUT_HANDLE: u32 = -11_i32 as u32;
#[cfg(windows)]
const WINDOWS_STD_ERROR_HANDLE: u32 = -12_i32 as u32;
#[cfg(windows)]
const NATIVE_EMBEDDING_WINDOWS_BASE_CREATION_FLAGS: u32 =
    WINDOWS_DETACHED_PROCESS | WINDOWS_CREATE_NEW_PROCESS_GROUP | WINDOWS_CREATE_NO_WINDOW;
#[cfg(windows)]
const NATIVE_EMBEDDING_WINDOWS_CREATION_FLAGS: u32 =
    NATIVE_EMBEDDING_WINDOWS_BASE_CREATION_FLAGS | WINDOWS_CREATE_BREAKAWAY_FROM_JOB;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(std_handle: u32) -> *mut std::ffi::c_void;
    fn GetHandleInformation(handle: *mut std::ffi::c_void, flags: *mut u32) -> i32;
    fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn setsid() -> std::ffi::c_int;
}

#[cfg(windows)]
struct WindowsStandardHandleInheritanceGuard {
    handles: Vec<*mut std::ffi::c_void>,
}

#[cfg(windows)]
impl WindowsStandardHandleInheritanceGuard {
    fn new() -> io::Result<Self> {
        let handles = [
            windows_standard_handle(WINDOWS_STD_INPUT_HANDLE),
            windows_standard_handle(WINDOWS_STD_OUTPUT_HANDLE),
            windows_standard_handle(WINDOWS_STD_ERROR_HANDLE),
        ];
        Self::from_handles(handles.into_iter().flatten())
    }

    fn from_handles(handles: impl IntoIterator<Item = *mut std::ffi::c_void>) -> io::Result<Self> {
        let mut guard = Self {
            handles: Vec::new(),
        };
        for handle in handles {
            let flags = windows_handle_information(handle)?;
            if flags & WINDOWS_HANDLE_FLAG_INHERIT == 0 {
                continue;
            }
            windows_set_handle_inheritance(handle, false)?;
            guard.handles.push(handle);
        }
        Ok(guard)
    }
}

#[cfg(windows)]
impl Drop for WindowsStandardHandleInheritanceGuard {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            let _ = windows_set_handle_inheritance(handle, true);
        }
    }
}

#[cfg(windows)]
fn windows_standard_handle(kind: u32) -> Option<*mut std::ffi::c_void> {
    // SAFETY: GetStdHandle has no pointer preconditions.
    let handle = unsafe { GetStdHandle(kind) };
    (!handle.is_null() && handle as isize != -1).then_some(handle)
}

#[cfg(windows)]
fn windows_handle_information(handle: *mut std::ffi::c_void) -> io::Result<u32> {
    let mut flags = 0_u32;
    // SAFETY: `flags` is writable and `handle` was returned by Windows or supplied by a test.
    if unsafe { GetHandleInformation(handle, &mut flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(flags)
}

#[cfg(windows)]
fn windows_set_handle_inheritance(
    handle: *mut std::ffi::c_void,
    inheritable: bool,
) -> io::Result<()> {
    let flags = if inheritable {
        WINDOWS_HANDLE_FLAG_INHERIT
    } else {
        0
    };
    // SAFETY: `handle` is valid for the duration of the call and the mask changes only inheritance.
    if unsafe { SetHandleInformation(handle, WINDOWS_HANDLE_FLAG_INHERIT, flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EmbedModelInventory {
    pub model_dir: Option<String>,
    pub required_gguf: String,
    pub required_gguf_present: bool,
    pub candidate_dirs: Vec<String>,
}

/// Machine-cache locations published while prewarming managed retrieval assets.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagedAssetPrewarmReport {
    pub cache_root: String,
    pub model_dir: Option<String>,
    pub native_backend: Option<String>,
    pub native_executable: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BootstrapReport {
    pub state: SidecarStateFile,
    pub infrastructure: InfrastructureHealth,
}

#[derive(Debug, Clone)]
pub struct BootstrapSidecarsOptions {
    pub wait_timeout: Duration,
    pub allow_native_embedding_spawn: bool,
    /// A broker-verified native launch owned by this workspace. This permits a
    /// later operation/profile to attach to the same process without treating
    /// a reachable machine-global endpoint as ownerless.
    pub reusable_native_embedding_launch: Option<crate::health::EmbeddingLaunchMetadata>,
}

/// Structured ownership evidence carried when a freshly spawned native process survives a
/// pre-state cleanup failure. The CLI broker uses this to quarantine the exact launch instead of
/// dropping its machine lock and allowing a duplicate process to start.
#[derive(Debug, Error)]
#[error("native embedding startup cleanup failed; exact launch must remain quarantined: {detail}")]
pub struct NativeEmbeddingStartupCleanupFailure {
    launch: crate::health::EmbeddingLaunchMetadata,
    detail: String,
}

impl NativeEmbeddingStartupCleanupFailure {
    pub fn new(launch: crate::health::EmbeddingLaunchMetadata, error: &anyhow::Error) -> Self {
        Self {
            launch,
            detail: format!("{error:#}"),
        }
    }

    pub fn launch(&self) -> &crate::health::EmbeddingLaunchMetadata {
        &self.launch
    }
}

pub fn native_embedding_startup_cleanup_failure(
    error: &anyhow::Error,
) -> Option<&NativeEmbeddingStartupCleanupFailure> {
    error.downcast_ref::<NativeEmbeddingStartupCleanupFailure>()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeEmbeddingServerLaunch {
    executable: PathBuf,
    model_path: PathBuf,
    model_sha256: Option<String>,
    args: Vec<String>,
    log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeEmbeddingSpawn {
    pid: u32,
    spawned_at_epoch_ms: i64,
    newly_spawned: bool,
}

#[derive(Debug, Clone)]
struct NativeLlamaCandidate {
    path: PathBuf,
    backend: crate::config::LlamaSidecarBackend,
}

#[derive(Debug, Deserialize)]
struct NativeLlamaInstallManifest {
    artifact: String,
    #[serde(default)]
    artifact_bytes: Option<u64>,
    artifact_sha256: String,
    executable_rel_path: String,
    executable_sha256: String,
}

pub fn bootstrap_sidecars_with_runtime_progress_and_native_launch_observer(
    runtime: &SidecarRuntimeConfig,
    options: BootstrapSidecarsOptions,
    progress: impl FnMut(&'static str),
    observe_new_native_launch: impl FnMut(&crate::health::EmbeddingLaunchMetadata) -> Result<()>,
) -> Result<BootstrapReport> {
    let mut progress = progress;
    let mut observe_new_native_launch = observe_new_native_launch;
    let port_lease_heartbeat = runtime.start_port_lease_heartbeat()?;
    let BootstrapSidecarsOptions {
        wait_timeout,
        allow_native_embedding_spawn,
        reusable_native_embedding_launch,
    } = options;
    let layout = runtime.layout.clone();
    layout.ensure_data_dirs()?;
    let launch_mode = crate::config::embedding_server_launch_mode_for_runtime(runtime)?;
    let native_embedding = (launch_mode == EmbeddingServerLaunchMode::NativeSpawned)
        .then(|| {
            native_embedding_server_launch_for_bootstrap(
                runtime,
                reusable_native_embedding_launch.as_ref(),
            )
        })
        .transpose()?;

    let native_embedding_spawn = if let Some(launch) = native_embedding.as_ref() {
        match with_bootstrap_progress(&mut progress, "model/bootstrap", || {
            spawn_native_embedding_server(
                launch,
                runtime,
                allow_native_embedding_spawn,
                reusable_native_embedding_launch.as_ref(),
                &mut observe_new_native_launch,
            )
        }) {
            Ok(spawn) => spawn,
            Err(error) => {
                cleanup_pre_state_startup_for_runtime(None, None).with_context(|| {
                    format!("rollback sidecar startup after native embedding spawn failed: {error}")
                })?;
                return Err(error);
            }
        }
    } else {
        None
    };

    let mut embedding_launch = native_embedding
        .as_ref()
        .map(|launch| embedding_launch_metadata(launch, runtime, native_embedding_spawn));
    if let (Some(selected), Some(reused)) = (
        embedding_launch.as_mut(),
        reusable_native_embedding_launch.as_ref(),
    ) && selected.pid == reused.pid
    {
        // The original current-launch log remains the proof source for the
        // reused process. A profile-specific log path contains only the later
        // attachment message and cannot prove startup/offload.
        selected.log_path.clone_from(&reused.log_path);
    }
    let embedding_launch_ownership =
        if native_embedding_spawn.is_some_and(|spawn| !spawn.newly_spawned) {
            EmbeddingLaunchOwnership::Attached
        } else {
            EmbeddingLaunchOwnership::Owner
        };
    let state = match sidecar_up_with_runtime_and_launch_metadata_and_ownership(
        runtime,
        embedding_launch.clone(),
        embedding_launch_ownership,
    ) {
        Ok(state) => state,
        Err(error) => {
            if let Err(cleanup_error) = cleanup_pre_state_startup_for_runtime(
                embedding_launch.as_ref(),
                native_embedding_spawn,
            ) {
                let error = error.context(format!(
                    "write retrieval-sidecars.json; pre-state startup cleanup failed: {cleanup_error}"
                ));
                if native_embedding_spawn.is_some_and(|spawn| spawn.newly_spawned)
                    && let Some(launch) = embedding_launch
                {
                    return Err(error.context(NativeEmbeddingStartupCleanupFailure::new(
                        launch,
                        &cleanup_error,
                    )));
                }
                return Err(error);
            }
            return Err(error);
        }
    };

    if !wait_timeout.is_zero() {
        with_bootstrap_progress(&mut progress, "model/bootstrap", || {
            wait_for_infrastructure(
                runtime,
                wait_timeout,
                newly_spawned_native_launch(embedding_launch.as_ref(), native_embedding_spawn),
            )
        })?;
    }

    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let infrastructure = crate::health::probe_infrastructure_health_with_embedding_device(
        runtime,
        &embedding_device,
    );
    port_lease_heartbeat.finish()?;

    Ok(BootstrapReport {
        state,
        infrastructure,
    })
}

fn with_bootstrap_progress<T>(
    progress: &mut impl FnMut(&'static str),
    phase: &'static str,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    progress(phase);
    action()
}

fn native_embedding_server_launch(
    runtime: &SidecarRuntimeConfig,
) -> Result<NativeEmbeddingServerLaunch> {
    ensure_native_launch_backend_supported(runtime)?;
    ensure_selected_managed_native_llama_server()?;
    let executable = native_llama_server_path()?;
    let model_path = embed_model_dir()?.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF);
    if !model_path.is_file() {
        bail!(
            "native llama.cpp embedding model not found: {}; run `node scripts/setup-retrieval-env.mjs --fetch-embed-model` or set CODESTORY_EMBED_MODEL_DIR",
            model_path.display()
        );
    }
    Ok(native_embedding_server_launch_from_paths(
        executable, model_path, runtime,
    ))
}

fn native_embedding_server_launch_for_bootstrap(
    runtime: &SidecarRuntimeConfig,
    reusable_launch: Option<&crate::health::EmbeddingLaunchMetadata>,
) -> Result<NativeEmbeddingServerLaunch> {
    if let Some(reusable_launch) = reusable_launch {
        return native_embedding_server_launch_from_verified_metadata(runtime, reusable_launch)
            .context("restore verified native embedding launch contract for reuse");
    }
    native_embedding_server_launch(runtime)
}

fn native_embedding_server_launch_from_verified_metadata(
    runtime: &SidecarRuntimeConfig,
    metadata: &crate::health::EmbeddingLaunchMetadata,
) -> Result<NativeEmbeddingServerLaunch> {
    let executable = metadata
        .executable_path
        .as_deref()
        .map(PathBuf::from)
        .context("verified native embedding launch is missing executable_path")?;
    let model_path = metadata
        .model_path
        .as_deref()
        .map(PathBuf::from)
        .context("verified native embedding launch is missing model_path")?;
    let mut launch = native_embedding_server_launch_from_paths(executable, model_path, runtime);
    launch.model_sha256.clone_from(&metadata.model_sha256);
    if !native_embedding_launch_matches_runtime_contract(runtime, metadata, &launch) {
        bail!("verified native embedding launch does not match the requested runtime contract");
    }
    Ok(launch)
}

#[doc(hidden)]
pub fn native_embedding_launch_matches_runtime_for_reuse(
    runtime: &SidecarRuntimeConfig,
    metadata: &crate::health::EmbeddingLaunchMetadata,
) -> Result<bool> {
    if crate::config::embedding_server_launch_mode_for_runtime(runtime)?
        != EmbeddingServerLaunchMode::NativeSpawned
    {
        return Ok(false);
    }
    let Some(executable) = metadata.executable_path.as_deref().map(PathBuf::from) else {
        return Ok(false);
    };
    let Some(model_path) = metadata.model_path.as_deref().map(PathBuf::from) else {
        return Ok(false);
    };
    let mut launch = native_embedding_server_launch_from_paths(executable, model_path, runtime);
    if metadata.model_sha256.is_none() {
        launch.model_sha256 = None;
    }
    Ok(native_embedding_launch_matches_runtime_contract(
        runtime, metadata, &launch,
    ))
}

#[doc(hidden)]
pub fn native_embedding_launch_contract_from_paths(
    executable: PathBuf,
    model_path: PathBuf,
    runtime: &SidecarRuntimeConfig,
) -> crate::health::EmbeddingLaunchMetadata {
    let launch = native_embedding_server_launch_from_paths(executable, model_path, runtime);
    embedding_launch_metadata(&launch, runtime, None)
}

fn native_embedding_launch_matches_runtime_contract(
    runtime: &SidecarRuntimeConfig,
    metadata: &crate::health::EmbeddingLaunchMetadata,
    launch: &NativeEmbeddingServerLaunch,
) -> bool {
    metadata.provider == "llamacpp"
        && metadata.launch_mode == EmbeddingServerLaunchMode::NativeSpawned.as_str()
        && metadata.endpoint == runtime.embedding.endpoint
        && metadata.launch_args == launch.args
        && metadata.launch_fingerprint_sha256.as_deref()
            == Some(native_embedding_launch_fingerprint(launch).as_str())
        && metadata.executable_path.as_deref() == Some(launch.executable.to_string_lossy().as_ref())
        && metadata.model_path.as_deref() == Some(launch.model_path.to_string_lossy().as_ref())
        && metadata.model_sha256 == launch.model_sha256
        && metadata.requested_device
            == crate::embeddings::embedding_accelerator_request().and_then(|request| request.device)
}

pub fn expected_native_embedding_launch_metadata(
    runtime: &SidecarRuntimeConfig,
) -> Result<Option<crate::health::EmbeddingLaunchMetadata>> {
    if crate::config::embedding_server_launch_mode_for_runtime(runtime)?
        != EmbeddingServerLaunchMode::NativeSpawned
    {
        return Ok(None);
    }
    let launch = native_embedding_server_launch(runtime)?;
    Ok(Some(embedding_launch_metadata(&launch, runtime, None)))
}

fn native_embedding_server_launch_from_paths(
    executable: PathBuf,
    model_path: PathBuf,
    runtime: &SidecarRuntimeConfig,
) -> NativeEmbeddingServerLaunch {
    let mut args = native_embedding_launch_args(&model_path, runtime);
    if let Some(request) = crate::embeddings::embedding_accelerator_request()
        && selected_native_llama_backend().is_none()
    {
        args.push("--n-gpu-layers".to_string());
        args.push(request.n_gpu_layers.clone());
        if let Some(device) = request.device {
            args.push("--device".to_string());
            args.push(device);
        }
    }
    NativeEmbeddingServerLaunch {
        executable,
        model_sha256: sha256_file(&model_path).ok(),
        model_path,
        args,
        log_path: crate::embeddings::native_embedding_log_path(runtime),
    }
}

fn native_embedding_launch_args(model_path: &Path, runtime: &SidecarRuntimeConfig) -> Vec<String> {
    if let Some(backend) = selected_native_llama_backend() {
        let request = crate::embeddings::embedding_accelerator_request();
        let n_gpu_layers = request
            .as_ref()
            .map(|request| request.n_gpu_layers.as_str())
            .unwrap_or("0");
        let device = request
            .as_ref()
            .and_then(|request| request.device.as_deref());
        let model = model_path.display().to_string();
        let port = native_embedding_endpoint_port(runtime).to_string();
        let mut args = Vec::new();
        let mut iter = backend.launch_args.into_iter().peekable();
        while let Some(arg) = iter.next() {
            if arg == "--device"
                && iter.peek().is_some_and(|next| next == "{device}")
                && device.is_none()
            {
                iter.next();
                continue;
            }
            args.push(
                arg.replace("{model}", &model)
                    .replace("{port}", &port)
                    .replace("{n_gpu_layers}", n_gpu_layers)
                    .replace("{device}", device.unwrap_or_default()),
            );
        }
        return args;
    }
    vec![
        "--embedding".to_string(),
        "--model".to_string(),
        model_path.display().to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        native_embedding_endpoint_port(runtime).to_string(),
    ]
}

fn native_embedding_endpoint_port(runtime: &SidecarRuntimeConfig) -> u16 {
    runtime
        .embedding
        .endpoint
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix("/v1/embeddings"))
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(runtime.embed_http_port)
}

fn embedding_launch_metadata(
    native_launch: &NativeEmbeddingServerLaunch,
    runtime: &SidecarRuntimeConfig,
    spawn: Option<NativeEmbeddingSpawn>,
) -> crate::health::EmbeddingLaunchMetadata {
    crate::health::EmbeddingLaunchMetadata {
        provider: "llamacpp".to_string(),
        launch_mode: EmbeddingServerLaunchMode::NativeSpawned
            .as_str()
            .to_string(),
        endpoint: runtime.embedding.endpoint.clone(),
        pid: spawn.map(|spawn| spawn.pid),
        spawned_at_epoch_ms: spawn.map(|spawn| spawn.spawned_at_epoch_ms),
        process_start_identity: spawn.and_then(|spawn| {
            crate::native_embedding_process_start_identity(spawn.pid)
                .ok()
                .flatten()
        }),
        spawn_protocol: None,
        launch_args: native_launch.args.clone(),
        launch_fingerprint_sha256: Some(native_embedding_launch_fingerprint(native_launch)),
        executable_source: Some(native_llama_executable_source(&native_launch.executable)),
        executable_path: Some(native_launch.executable.display().to_string()),
        model_path: Some(native_launch.model_path.display().to_string()),
        model_sha256: native_launch.model_sha256.clone(),
        log_path: Some(native_launch.log_path.display().to_string()),
        requested_device: crate::embeddings::embedding_accelerator_request()
            .and_then(|request| request.device),
    }
}

fn native_embedding_launch_fingerprint(native_launch: &NativeEmbeddingServerLaunch) -> String {
    let mut hasher = Sha256::new();
    hasher.update(native_launch.executable.display().to_string().as_bytes());
    for arg in &native_launch.args {
        hasher.update([0]);
        hasher.update(arg.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn native_llama_executable_source(path: &Path) -> String {
    if std::env::var("CODESTORY_EMBED_NATIVE_LLAMA_SERVER").is_ok() {
        return "env:CODESTORY_EMBED_NATIVE_LLAMA_SERVER".to_string();
    }
    if let Some(backend) = selected_native_llama_backend() {
        for backend in matching_native_llama_backends(&backend.provider) {
            let rel_path = native_llama_backend_rel_path(&backend);
            if path == user_cache_root().join(&rel_path) {
                return "managed_cache".to_string();
            }
        }
    }
    "resolved_path".to_string()
}

fn native_llama_server_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CODESTORY_EMBED_NATIVE_LLAMA_SERVER") {
        return validate_explicit_native_llama_server(PathBuf::from(path));
    }
    native_llama_server_path_from_candidates(native_llama_server_candidates()?)
}

fn validate_explicit_native_llama_server(path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!(
            "CODESTORY_EMBED_NATIVE_LLAMA_SERVER must be an absolute path to llama-server; ambient PATH lookup is not allowed"
        );
    }
    if !path.is_file() {
        bail!(
            "CODESTORY_EMBED_NATIVE_LLAMA_SERVER does not point to a file: {}; install managed llama.cpp assets or set the absolute executable path",
            path.display()
        );
    }
    Ok(path)
}

fn native_llama_server_path_from_candidates(
    candidates: Vec<NativeLlamaCandidate>,
) -> Result<PathBuf> {
    let install_hint = candidates
        .first()
        .map(|candidate| candidate.path.display().to_string())
        .context("managed native llama-server candidate list is empty")?;
    let mut invalid_managed_candidate = None;
    for candidate in candidates {
        if !candidate.path.is_file() {
            continue;
        }
        if let Err(error) =
            validate_managed_native_llama_server(&candidate.path, &candidate.backend)
        {
            invalid_managed_candidate = Some(error.to_string());
            continue;
        }
        return Ok(candidate.path);
    }
    let suffix = invalid_managed_candidate
        .map(|error| format!(" Last managed candidate was rejected: {error}."))
        .unwrap_or_default();
    Err(anyhow::anyhow!(
        "native llama-server not found; set CODESTORY_EMBED_NATIVE_LLAMA_SERVER to an absolute path or install managed llama.cpp assets under {}; ambient PATH lookup is not allowed{suffix}",
        install_hint
    ))
}

fn native_llama_server_candidates() -> Result<Vec<NativeLlamaCandidate>> {
    let backend = selected_native_llama_backend().context(
        "no managed native llama-server backend matches this platform and device policy",
    )?;
    let mut candidates = Vec::new();
    for backend in matching_native_llama_backends(&backend.provider) {
        let rel_path = native_llama_backend_rel_path(&backend);
        candidates.push(NativeLlamaCandidate {
            path: user_cache_root().join(&rel_path),
            backend: backend.clone(),
        });
    }
    Ok(candidates)
}

fn selected_native_llama_backend() -> Option<crate::config::LlamaSidecarBackend> {
    let provider = crate::embeddings::embedding_accelerator_request()
        .map(|request| request.provider)
        .unwrap_or_else(|| "cpu".to_string());
    matching_native_llama_backends(&provider).into_iter().next()
}

fn matching_native_llama_backends(provider: &str) -> Vec<crate::config::LlamaSidecarBackend> {
    crate::config::llama_sidecar_backends(provider)
}

fn ensure_native_launch_backend_supported(runtime: &SidecarRuntimeConfig) -> Result<()> {
    if crate::config::embedding_server_launch_mode_for_runtime(runtime)?
        != EmbeddingServerLaunchMode::NativeSpawned
    {
        return Ok(());
    }
    let Some(request) = crate::embeddings::embedding_accelerator_request() else {
        return Ok(());
    };
    if selected_native_llama_backend().is_some() {
        return Ok(());
    }
    let host = crate::config::embedding_host_platform();
    anyhow::bail!(
        "CODESTORY_EMBED_SERVER_LAUNCH=native_spawned is unsupported for provider={} on {}/{}; choose a provider with a managed native backend or configure a trusted external endpoint",
        request.provider,
        host.os,
        host.arch
    )
}

fn native_llama_backend_rel_path(backend: &crate::config::LlamaSidecarBackend) -> PathBuf {
    Path::new(&backend.managed_cache_rel_dir).join(&backend.executable_rel_path)
}

fn ensure_selected_managed_native_llama_server() -> Result<()> {
    if std::env::var("CODESTORY_EMBED_NATIVE_LLAMA_SERVER").is_ok() {
        return Ok(());
    }
    let backend = selected_native_llama_backend().context(
        "no managed native llama-server backend matches this platform and device policy",
    )?;
    if native_llama_server_path_from_candidates(native_llama_server_candidates()?).is_ok() {
        return Ok(());
    }
    ensure_managed_native_llama_server(&backend)?;
    Ok(())
}

fn ensure_managed_native_llama_server(backend: &crate::config::LlamaSidecarBackend) -> Result<()> {
    let executable = user_cache_root().join(native_llama_backend_rel_path(backend));
    let cache_root = user_cache_root();
    let _asset_lock = crate::managed_assets::ManagedAssetLock::acquire(&cache_root)?;
    if executable.is_file() && validate_managed_native_llama_server(&executable, backend).is_ok() {
        return Ok(());
    }
    if let Some(install_dir) = executable.parent()
        && fs::symlink_metadata(install_dir).is_ok()
    {
        crate::managed_assets::quarantine_path(install_dir, "invalid")?;
    }
    let archive = cache_root
        .join("managed-embeddings/blobs/sha256")
        .join(&backend.sha256)
        .join(&backend.artifact);
    crate::managed_assets::ensure_cached_asset_locked(
        &archive,
        std::slice::from_ref(&backend.url),
        backend.artifact_bytes,
        &backend.sha256,
    )?;
    let temp_root = managed_llama_temp_root()?;
    let install_result = install_managed_native_llama_server_from_archive(
        backend,
        &archive,
        &temp_root.join("extract"),
        &executable,
    );
    let cleanup_result = fs::remove_dir_all(&temp_root);
    if let Err(error) = cleanup_result
        && install_result.is_ok()
    {
        return Err(error).with_context(|| format!("remove {}", temp_root.display()));
    }
    install_result
}

/// Publish the requested managed retrieval assets through the shared asset boundary.
pub fn prewarm_managed_assets(
    include_model: bool,
    include_native_backend: bool,
    backend_id: Option<&str>,
) -> Result<ManagedAssetPrewarmReport> {
    if !include_model && !include_native_backend {
        bail!("select --model, --native-backend, or both");
    }
    if backend_id.is_some() && !include_native_backend {
        bail!("--llama-backend requires --native-backend");
    }

    let cache_root = user_cache_root();
    let model_dir = include_model
        .then(|| crate::managed_assets::ensure_managed_embedding_model(&cache_root))
        .transpose()?
        .map(|path| path.display().to_string());

    let backend = if include_native_backend {
        let backend = match backend_id {
            Some(id) => crate::config::llama_sidecar_backend_by_id(id)
                .with_context(|| format!("unknown managed llama-server backend {id}"))?,
            None => selected_native_llama_backend().context(
                "no managed native llama-server backend is available for this host and accelerator",
            )?,
        };
        ensure_managed_native_llama_server(&backend)?;
        Some(backend)
    } else {
        None
    };
    let native_executable = backend.as_ref().map(|backend| {
        cache_root
            .join(native_llama_backend_rel_path(backend))
            .display()
            .to_string()
    });

    Ok(ManagedAssetPrewarmReport {
        cache_root: cache_root.display().to_string(),
        model_dir,
        native_backend: backend.map(|backend| backend.id),
        native_executable,
    })
}

fn managed_llama_temp_root() -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = user_cache_root()
        .join("downloads")
        .join(format!("llama-server-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    Ok(root)
}

fn install_managed_native_llama_server_from_archive(
    backend: &crate::config::LlamaSidecarBackend,
    archive: &Path,
    extract_root: &Path,
    executable: &Path,
) -> Result<()> {
    verify_sha256(archive, &backend.sha256)
        .with_context(|| format!("verify {}", archive.display()))?;
    fs::create_dir_all(extract_root)
        .with_context(|| format!("create {}", extract_root.display()))?;
    let member_path = safe_archive_member_path(&backend.executable_archive_path)?;
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(extract_root)
        .output()
        .with_context(|| format!("run tar for {}", archive.display()))?;
    if !output.status.success() {
        bail!(
            "tar failed extracting {}: {}{}",
            archive.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let extracted = extract_root.join(&member_path);
    validate_extracted_executable(&extracted, backend)?;
    let target_dir = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server executable has no parent"))?;
    let source_dir = extracted
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server archive member has no parent"))?;
    let target_parent = target_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server install dir has no parent"))?;
    fs::create_dir_all(target_parent)
        .with_context(|| format!("create {}", target_parent.display()))?;
    let staging_dir = target_dir.with_extension("download");
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)
            .with_context(|| format!("remove {}", staging_dir.display()))?;
    }
    copy_dir_contents(source_dir, &staging_dir).with_context(|| {
        format!(
            "copy extracted llama-server payload {} to {}",
            source_dir.display(),
            staging_dir.display()
        )
    })?;
    let executable_rel_path = safe_archive_member_path(&backend.executable_rel_path)?;
    let staged_executable = staging_dir.join(executable_rel_path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&staged_executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&staged_executable, permissions)?;
    }
    let executable_sha = sha256_file(&staged_executable)?;
    if !executable_sha.eq_ignore_ascii_case(&backend.executable_sha256) {
        bail!(
            "managed llama-server executable checksum mismatch for {}: expected {}, got {}",
            staged_executable.display(),
            backend.executable_sha256,
            executable_sha
        );
    }
    fs::write(staging_dir.join(MANAGED_LLAMA_EXTRACTED_MARKER), b"1")
        .with_context(|| format!("write extraction marker {}", staging_dir.display()))?;
    write_managed_native_llama_install_manifest(backend, &staged_executable, &executable_sha)?;
    if target_dir.exists() {
        fs::remove_dir_all(target_dir)
            .with_context(|| format!("remove {}", target_dir.display()))?;
    }
    fs::rename(&staging_dir, target_dir).with_context(|| {
        format!(
            "move downloaded llama-server payload {} to {}",
            staging_dir.display(),
            target_dir.display()
        )
    })?;
    validate_managed_native_llama_server(executable, backend)
}

fn safe_archive_member_path(member: &str) -> Result<PathBuf> {
    if member.trim().is_empty() || member.contains('\\') {
        bail!("managed llama-server archive path is not portable: {member}");
    }
    let path = Path::new(member);
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("managed llama-server archive path must be relative and contained: {member}");
            }
        }
    }
    if safe.as_os_str().is_empty() {
        bail!("managed llama-server archive path is empty");
    }
    Ok(safe)
}

fn copy_dir_contents(source: &Path, target: &Path) -> Result<()> {
    let source_root =
        fs::canonicalize(source).with_context(|| format!("canonicalize {}", source.display()))?;
    copy_dir_contents_with_root(source, target, &source_root)
}

fn copy_dir_contents_with_root(source: &Path, target: &Path, source_root: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("create {}", target.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", source.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", entry.path().display()))?;
        let target_path = target.join(entry.file_name());
        if file_type.is_symlink() {
            copy_archive_symlinked_file(&entry.path(), &target_path, source_root)?;
        } else if file_type.is_dir() {
            copy_dir_contents_with_root(&entry.path(), &target_path, source_root)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target_path).with_context(|| {
                format!(
                    "copy managed llama-server archive member {} to {}",
                    entry.path().display(),
                    target_path.display()
                )
            })?;
        } else {
            bail!(
                "managed llama-server archive member is not a regular file or directory: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn copy_archive_symlinked_file(source: &Path, target: &Path, source_root: &Path) -> Result<()> {
    let link_target =
        fs::read_link(source).with_context(|| format!("read symlink {}", source.display()))?;
    copy_archive_symlinked_file_target(source, target, source_root, &link_target)
}

fn copy_archive_symlinked_file_target(
    source: &Path,
    target: &Path,
    source_root: &Path,
    link_target: &Path,
) -> Result<()> {
    if link_target.is_absolute() {
        bail!(
            "managed llama-server archive symlink must be relative: {} -> {}",
            source.display(),
            link_target.display()
        );
    }
    let source_parent = source
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server archive symlink has no parent"))?;
    let resolved = source_parent.join(link_target);
    let resolved = fs::canonicalize(&resolved).with_context(|| {
        format!(
            "resolve managed llama-server archive symlink {} -> {}",
            source.display(),
            link_target.display()
        )
    })?;
    if !resolved.starts_with(source_root) {
        bail!(
            "managed llama-server archive symlink escapes payload: {} -> {}",
            source.display(),
            resolved.display()
        );
    }
    let metadata = fs::metadata(&resolved)
        .with_context(|| format!("metadata symlink target {}", resolved.display()))?;
    if !metadata.is_file() {
        bail!(
            "managed llama-server archive symlink target is not a regular file: {} -> {}",
            source.display(),
            resolved.display()
        );
    }
    fs::copy(&resolved, target).with_context(|| {
        format!(
            "copy managed llama-server archive symlink target {} to {}",
            resolved.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn validate_extracted_executable(
    extracted: &Path,
    backend: &crate::config::LlamaSidecarBackend,
) -> Result<()> {
    let metadata = fs::symlink_metadata(extracted)
        .with_context(|| format!("metadata {}", extracted.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "managed llama-server archive member is not a regular file: {}",
            extracted.display()
        );
    }
    let executable_sha = sha256_file(extracted)?;
    if !executable_sha.eq_ignore_ascii_case(&backend.executable_sha256) {
        bail!(
            "managed llama-server executable checksum mismatch for {}: expected {}, got {}",
            extracted.display(),
            backend.executable_sha256,
            executable_sha
        );
    }
    Ok(())
}

fn write_managed_native_llama_install_manifest(
    backend: &crate::config::LlamaSidecarBackend,
    executable: &Path,
    executable_sha: &str,
) -> Result<()> {
    let manifest_path = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server executable has no parent"))?
        .join("install-manifest.json");
    let manifest = serde_json::json!({
        "backend": backend.id,
        "artifact": backend.artifact,
        "artifact_bytes": backend.artifact_bytes,
        "artifact_sha256": backend.sha256,
        "executable_rel_path": backend.executable_rel_path,
        "executable_sha256": executable_sha,
        "source_url": backend.url,
    });
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize managed llama-server manifest"),
    )
    .with_context(|| format!("write {}", manifest_path.display()))?;
    Ok(())
}

fn validate_managed_native_llama_server(
    executable: &Path,
    backend: &crate::config::LlamaSidecarBackend,
) -> Result<()> {
    let install_dir = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server path has no parent"))?
        .to_path_buf();
    let manifest_path = install_dir.join("install-manifest.json");
    let manifest: NativeLlamaInstallManifest = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parse {}", manifest_path.display()))?;
    if manifest.artifact != backend.artifact {
        bail!(
            "managed llama-server artifact mismatch for {}: expected {}, got {}",
            executable.display(),
            backend.artifact,
            manifest.artifact
        );
    }
    if let Some(artifact_bytes) = manifest.artifact_bytes
        && artifact_bytes != backend.artifact_bytes
    {
        bail!(
            "managed llama-server artifact size mismatch for {}: expected {}, got {}",
            executable.display(),
            backend.artifact_bytes,
            artifact_bytes
        );
    }
    if !manifest
        .artifact_sha256
        .eq_ignore_ascii_case(&backend.sha256)
    {
        bail!(
            "managed llama-server artifact checksum mismatch for {}: expected {}, got {}",
            executable.display(),
            backend.sha256,
            manifest.artifact_sha256
        );
    }
    if manifest.executable_rel_path != backend.executable_rel_path {
        bail!(
            "managed llama-server executable path mismatch for {}: expected {}, got {}",
            executable.display(),
            backend.executable_rel_path,
            manifest.executable_rel_path
        );
    }
    if !manifest
        .executable_sha256
        .eq_ignore_ascii_case(&backend.executable_sha256)
    {
        bail!(
            "managed llama-server executable manifest checksum mismatch for {}: expected {}, got {}",
            executable.display(),
            backend.executable_sha256,
            manifest.executable_sha256
        );
    }
    let actual_executable_sha = sha256_file(executable)?;
    if !actual_executable_sha.eq_ignore_ascii_case(&backend.executable_sha256) {
        bail!(
            "managed llama-server executable checksum mismatch for {}: expected {}, got {}",
            executable.display(),
            backend.executable_sha256,
            actual_executable_sha
        );
    }
    let extraction_marker = install_dir.join(MANAGED_LLAMA_EXTRACTED_MARKER);
    if !extraction_marker.is_file() {
        bail!(
            "managed llama-server install is incomplete for {}; missing {}",
            executable.display(),
            extraction_marker.display()
        );
    }
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn spawn_native_embedding_server(
    launch: &NativeEmbeddingServerLaunch,
    runtime: &SidecarRuntimeConfig,
    allow_spawn: bool,
    reusable_launch: Option<&crate::health::EmbeddingLaunchMetadata>,
    observe_new_native_launch: &mut impl FnMut(&crate::health::EmbeddingLaunchMetadata) -> Result<()>,
) -> Result<Option<NativeEmbeddingSpawn>> {
    let probe = crate::embeddings::probe_product_embedding_runtime_for_runtime(runtime);
    spawn_native_embedding_server_with_probe_and_observer(
        launch,
        runtime,
        allow_spawn,
        reusable_launch,
        probe,
        observe_new_native_launch,
    )
}

fn spawn_native_embedding_server_with_probe_and_observer(
    launch: &NativeEmbeddingServerLaunch,
    runtime: &SidecarRuntimeConfig,
    allow_spawn: bool,
    reusable_launch: Option<&crate::health::EmbeddingLaunchMetadata>,
    probe: crate::embeddings::EmbeddingRuntimeProbe,
    observe_new_native_launch: &mut impl FnMut(&crate::health::EmbeddingLaunchMetadata) -> Result<()>,
) -> Result<Option<NativeEmbeddingSpawn>> {
    if let Some(parent) = launch.log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create native llama.cpp log dir {}", parent.display()))?;
    }
    if native_embedding_server_reusable(&probe) {
        let mut log = open_native_embedding_log(&launch.log_path)?;
        let reusable = if let Some(metadata) = reusable_launch {
            reusable_native_embedding_spawn_from_metadata(runtime, launch, metadata)?
        } else {
            reusable_native_embedding_spawn_from_state(runtime, launch)?
        };
        if let Some(spawn) = reusable {
            writeln!(
                log,
                "reusing existing native llama.cpp embedding server pid={}: {}",
                spawn.pid, probe.detail
            )
            .ok();
            return Ok(Some(spawn));
        }
        writeln!(
            log,
            "refusing ownerless native llama.cpp embedding server reuse: {}",
            probe.detail
        )
        .ok();
        bail!(
            "native llama.cpp embedding endpoint is reachable but no matching sidecar launch metadata with a pid was found; cannot safely transfer the native embedding broker lock"
        );
    }
    if !allow_spawn {
        let mut log = open_native_embedding_log(&launch.log_path)?;
        writeln!(
            log,
            "refusing native llama.cpp embedding server spawn under reuse-only broker lease after probe failed: {}",
            probe.detail
        )
        .ok();
        bail!(
            "native llama.cpp embedding endpoint is unreachable and the broker lease is reuse-only; refusing to start another native embedding server"
        );
    }
    if let Err(error) = crate::embeddings::prepare_native_embedding_log_for_launch(&launch.log_path)
    {
        eprintln!(
            "CodeStory native embedding log rotation warning: path={} error={error:#}",
            launch.log_path.display()
        );
    }
    let mut log = open_native_embedding_log(&launch.log_path)?;
    writeln!(
        log,
        "starting native llama.cpp embedding server: after probe failed ({}) {} {}",
        probe.detail,
        launch.executable.display(),
        launch.args.join(" ")
    )
    .ok();
    #[cfg(windows)]
    writeln!(
        log,
        "native llama.cpp embedding server Windows creation_flags=0x{NATIVE_EMBEDDING_WINDOWS_CREATION_FLAGS:08x}"
    )
    .ok();
    #[cfg(target_os = "macos")]
    writeln!(
        log,
        "native llama.cpp embedding server Darwin session detachment=setsid"
    )
    .ok();
    let mut pending_launch = embedding_launch_metadata(launch, runtime, None);
    pending_launch.spawned_at_epoch_ms = Some(now_epoch_ms());
    pending_launch.spawn_protocol = native_embedding_spawn_protocol().map(str::to_string);
    observe_new_native_launch(&pending_launch)
        .context("publish pending native embedding ownership before spawn")?;
    match spawn_native_embedding_server_once(launch, &log) {
        Ok(mut process) => {
            let pid = process.pid();
            let spawn = NativeEmbeddingSpawn {
                pid,
                spawned_at_epoch_ms: now_epoch_ms(),
                newly_spawned: true,
            };
            let mut selected_launch = embedding_launch_metadata(launch, runtime, Some(spawn));
            selected_launch.spawn_protocol = native_embedding_spawn_protocol().map(str::to_string);
            if selected_launch.process_start_identity.is_none() {
                let error = anyhow::anyhow!(
                    "native embedding process start identity is unavailable immediately after spawn"
                );
                if let Err(cleanup_error) = process.abort() {
                    let error = error.context(format!(
                        "capture exact native embedding process start identity; cleanup failed: {cleanup_error}"
                    ));
                    return Err(error.context(NativeEmbeddingStartupCleanupFailure::new(
                        selected_launch,
                        &cleanup_error,
                    )));
                }
                return Err(error);
            }
            if let Err(error) = observe_new_native_launch(&selected_launch) {
                if let Err(cleanup_error) = process.abort() {
                    let error = error.context(format!(
                        "publish exact native embedding ownership after spawn; cleanup failed: {cleanup_error}"
                    ));
                    return Err(error.context(NativeEmbeddingStartupCleanupFailure::new(
                        selected_launch,
                        &cleanup_error,
                    )));
                }
                return Err(error).context("publish exact native embedding ownership after spawn");
            }
            if let Err(error) = process.release_gate() {
                let cleanup_error = process.abort().err();
                let error = error.context("release native embedding Darwin exec gate");
                if let Some(cleanup_error) = cleanup_error {
                    return Err(error.context(NativeEmbeddingStartupCleanupFailure::new(
                        selected_launch,
                        &cleanup_error,
                    )));
                }
                return Err(error);
            }
            if let Err(error) = wait_for_spawned_native_embedding_identity(&selected_launch) {
                if let Err(cleanup_error) = process.abort() {
                    let error = error.context(format!(
                        "verify native embedding identity after exec gate; cleanup failed: {cleanup_error}"
                    ));
                    return Err(error.context(NativeEmbeddingStartupCleanupFailure::new(
                        selected_launch,
                        &cleanup_error,
                    )));
                }
                return Err(error);
            }
            Ok(Some(spawn))
        }
        Err(error) if native_embedding_breakaway_denied(&error) => Err(error).context(
            "native_embedding_breakaway_denied: host job object blocked CREATE_BREAKAWAY_FROM_JOB; native embedding cannot survive repair exit",
        ),
        Err(error) => Err(error).with_context(|| {
            format!(
                "spawn native llama.cpp server {}{}",
                launch.executable.display(),
                native_embedding_spawn_detail()
            )
        }),
    }
}

fn open_native_embedding_log(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open native llama.cpp log {}", path.display()))
}

struct SpawnedNativeEmbeddingProcess {
    child: Child,
    gate: Option<ChildStdin>,
}

impl SpawnedNativeEmbeddingProcess {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn release_gate(&mut self) -> Result<()> {
        let Some(mut gate) = self.gate.take() else {
            return Ok(());
        };
        writeln!(gate, "{NATIVE_EMBEDDING_DARWIN_EXEC_GATE_TOKEN}")
            .context("write native embedding Darwin exec gate token")?;
        gate.flush()
            .context("flush native embedding Darwin exec gate token")?;
        Ok(())
    }

    fn abort(&mut self) -> Result<()> {
        self.gate.take();
        match self.child.kill() {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
            Err(error) => return Err(error).context("kill owned native embedding spawn"),
        }
        self.child
            .wait()
            .context("reap owned native embedding spawn")?;
        Ok(())
    }
}

fn spawn_native_embedding_server_once(
    launch: &NativeEmbeddingServerLaunch,
    log: &File,
) -> std::io::Result<SpawnedNativeEmbeddingProcess> {
    let stdout = log.try_clone()?;
    let stderr = log.try_clone()?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(NATIVE_EMBEDDING_DARWIN_EXEC_GATE_SCRIPT)
            .arg("codestory-native-gate")
            .arg(&launch.executable)
            .args(&launch.args)
            .stdin(Stdio::piped());
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(&launch.executable);
    #[cfg(not(target_os = "macos"))]
    command.args(&launch.args).stdin(Stdio::null());
    command
        .current_dir(launch.executable.parent().unwrap_or_else(|| Path::new(".")))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    configure_native_embedding_command(&mut command);
    #[cfg(windows)]
    let _standard_handle_guard = WindowsStandardHandleInheritanceGuard::new()?;
    let child = command.spawn()?;
    #[cfg(target_os = "macos")]
    let mut child = child;
    #[cfg(target_os = "macos")]
    let gate = child.stdin.take();
    #[cfg(not(target_os = "macos"))]
    let gate = None;
    Ok(SpawnedNativeEmbeddingProcess { child, gate })
}

#[cfg(target_os = "macos")]
fn native_embedding_spawn_protocol() -> Option<&'static str> {
    Some(NATIVE_EMBEDDING_DARWIN_EXEC_GATE_PROTOCOL)
}

#[cfg(not(target_os = "macos"))]
fn native_embedding_spawn_protocol() -> Option<&'static str> {
    None
}

fn wait_for_spawned_native_embedding_identity(
    launch: &crate::health::EmbeddingLaunchMetadata,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match crate::sidecar::native_embedding_launch_identity_status(launch) {
            crate::sidecar::NativeEmbeddingLaunchIdentityStatus::Matched { .. } => return Ok(()),
            crate::sidecar::NativeEmbeddingLaunchIdentityStatus::NotRunning { pid } => {
                bail!("native embedding pid {pid} exited before exact identity verification")
            }
            crate::sidecar::NativeEmbeddingLaunchIdentityStatus::Mismatched { reason, .. }
            | crate::sidecar::NativeEmbeddingLaunchIdentityStatus::Unverified { reason, .. }
                if Instant::now() >= deadline =>
            {
                bail!("native embedding identity did not converge after spawn: {reason}")
            }
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn native_embedding_server_reusable(probe: &crate::embeddings::EmbeddingRuntimeProbe) -> bool {
    probe.reachable
}

fn reusable_native_embedding_spawn_from_state(
    runtime: &SidecarRuntimeConfig,
    launch: &NativeEmbeddingServerLaunch,
) -> Result<Option<NativeEmbeddingSpawn>> {
    reusable_native_embedding_spawn_from_state_with_identity(
        runtime,
        launch,
        crate::sidecar::ensure_native_embedding_launch_identity,
    )
}

fn reusable_native_embedding_spawn_from_state_with_identity(
    runtime: &SidecarRuntimeConfig,
    launch: &NativeEmbeddingServerLaunch,
    validate_launch: impl FnMut(&crate::health::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<NativeEmbeddingSpawn>> {
    let state_file = &runtime.layout.state_file;
    if !state_file.exists() {
        return Ok(None);
    }
    let contents =
        fs::read_to_string(state_file).with_context(|| format!("read {}", state_file.display()))?;
    let state: SidecarStateFile = serde_json::from_str(&contents)
        .with_context(|| format!("parse {}", state_file.display()))?;
    if state.owner != "codestory"
        || state.namespace != runtime.namespace
        || state.profile != runtime.profile.as_str()
        || state.run_id.as_deref() != runtime.run_id.as_deref()
    {
        return Ok(None);
    }
    let Some(metadata) = state.embedding_launch.as_ref() else {
        return Ok(None);
    };
    reusable_native_embedding_spawn_from_metadata_with_identity(
        runtime,
        launch,
        metadata,
        validate_launch,
    )
}

fn reusable_native_embedding_spawn_from_metadata(
    runtime: &SidecarRuntimeConfig,
    launch: &NativeEmbeddingServerLaunch,
    metadata: &crate::health::EmbeddingLaunchMetadata,
) -> Result<Option<NativeEmbeddingSpawn>> {
    reusable_native_embedding_spawn_from_metadata_with_identity(
        runtime,
        launch,
        metadata,
        crate::sidecar::ensure_native_embedding_launch_identity,
    )
}

fn reusable_native_embedding_spawn_from_metadata_with_identity(
    runtime: &SidecarRuntimeConfig,
    launch: &NativeEmbeddingServerLaunch,
    metadata: &crate::health::EmbeddingLaunchMetadata,
    mut validate_launch: impl FnMut(&crate::health::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<NativeEmbeddingSpawn>> {
    let launch_fingerprint = native_embedding_launch_fingerprint(launch);
    if metadata.launch_mode != EmbeddingServerLaunchMode::NativeSpawned.as_str()
        || metadata.endpoint != runtime.embedding.endpoint
        || metadata.launch_fingerprint_sha256.as_deref() != Some(launch_fingerprint.as_str())
        || metadata
            .model_sha256
            .as_ref()
            .is_some_and(|digest| launch.model_sha256.as_ref() != Some(digest))
    {
        return Ok(None);
    }
    let Some(pid) = metadata.pid else {
        return Ok(None);
    };
    let validated_pid = validate_launch(metadata)
        .with_context(|| format!("validate reusable native embedding pid {pid}"))?;
    if validated_pid != pid {
        bail!(
            "validated reusable native embedding pid mismatch: expected {pid}, got {validated_pid}"
        );
    }
    Ok(Some(NativeEmbeddingSpawn {
        pid,
        spawned_at_epoch_ms: metadata.spawned_at_epoch_ms.unwrap_or_else(now_epoch_ms),
        newly_spawned: false,
    }))
}

fn cleanup_native_embedding_after_state_write_error(
    launch: Option<&crate::health::EmbeddingLaunchMetadata>,
    spawn: Option<NativeEmbeddingSpawn>,
    stop: impl FnOnce(&crate::health::EmbeddingLaunchMetadata) -> Result<()>,
) -> Result<()> {
    if !spawn.is_some_and(|spawn| spawn.newly_spawned) {
        return Ok(());
    }
    let Some(launch) = launch else {
        return Ok(());
    };
    stop(launch)
}

fn cleanup_pre_state_startup_for_runtime(
    launch: Option<&crate::health::EmbeddingLaunchMetadata>,
    spawn: Option<NativeEmbeddingSpawn>,
) -> Result<()> {
    cleanup_native_embedding_after_state_write_error(
        launch,
        spawn,
        crate::sidecar::stop_native_embedding_process_for_launch,
    )
}

fn configure_native_embedding_command(_command: &mut Command) {
    #[cfg(windows)]
    _command.creation_flags(NATIVE_EMBEDDING_WINDOWS_CREATION_FLAGS);
    #[cfg(target_os = "macos")]
    // SAFETY: this closure runs in the child after fork and calls only the async-signal-safe
    // `setsid(2)` before exec. A fresh child is not a process-group leader, so `setsid` gives
    // the native server its own session and process group without inserting a wrapper process.
    unsafe {
        _command.pre_exec(|| {
            if setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

fn native_embedding_spawn_detail() -> &'static str {
    #[cfg(windows)]
    {
        " with detached Windows creation flags including breakaway-from-job"
    }
    #[cfg(target_os = "macos")]
    {
        " in a detached Darwin session"
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        ""
    }
}

fn native_embedding_breakaway_denied(error: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(5)
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        false
    }
}

fn embed_model_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CODESTORY_EMBED_MODEL_DIR") {
        let path = PathBuf::from(path);
        if embed_model_dir_ready(&path) {
            return Ok(path);
        }
        anyhow::bail!(
            "CODESTORY_EMBED_MODEL_DIR does not contain {}",
            crate::embeddings::BGE_BASE_EN_V1_5_GGUF
        );
    }
    crate::managed_assets::ensure_managed_embedding_model(&user_cache_root())
        .context("prepare managed embedding model")
}

pub fn embed_model_inventory() -> EmbedModelInventory {
    let candidates = embed_model_candidates();
    let model_dir = candidates
        .iter()
        .find(|candidate| embed_model_dir_ready(candidate))
        .or_else(|| candidates.first())
        .map(|path| path.display().to_string());
    let required_gguf_present = model_dir.as_ref().is_some_and(|path| {
        let path = Path::new(path);
        if path == crate::managed_assets::managed_embedding_model_dir(&user_cache_root()) {
            crate::managed_assets::managed_embedding_model_is_published(&user_cache_root())
        } else {
            embed_model_dir_ready(path)
        }
    });
    EmbedModelInventory {
        model_dir,
        required_gguf: crate::embeddings::BGE_BASE_EN_V1_5_GGUF.to_string(),
        required_gguf_present,
        candidate_dirs: candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    }
}

fn embed_model_candidates() -> Vec<PathBuf> {
    if let Ok(path) = std::env::var("CODESTORY_EMBED_MODEL_DIR") {
        return vec![PathBuf::from(path)];
    }
    vec![crate::managed_assets::managed_embedding_model_dir(
        &user_cache_root(),
    )]
}

fn embed_model_dir_ready(path: &Path) -> bool {
    path.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
        .is_file()
}

fn newly_spawned_native_launch(
    launch: Option<&crate::health::EmbeddingLaunchMetadata>,
    spawn: Option<NativeEmbeddingSpawn>,
) -> Option<&crate::health::EmbeddingLaunchMetadata> {
    spawn
        .is_some_and(|spawn| spawn.newly_spawned)
        .then_some(launch)
        .flatten()
}

fn wait_for_infrastructure(
    runtime: &SidecarRuntimeConfig,
    timeout: Duration,
    newly_spawned_native: Option<&crate::health::EmbeddingLaunchMetadata>,
) -> Result<InfrastructureHealth> {
    let started = Instant::now();
    let poll = Duration::from_millis(500);
    let mut last = probe_infrastructure_health(runtime);
    loop {
        let native_log = newly_spawned_native
            .and_then(|launch| launch.log_path.as_deref())
            .map(Path::new);
        if native_log.is_some_and(native_embedding_log_reports_bind_failure) {
            bail!(
                "{NATIVE_EMBEDDING_PORT_BIND_FAILED_REASON}: native llama.cpp could not bind {}",
                runtime.embedding.endpoint
            );
        }

        if infrastructure_ready(&last)
            && newly_spawned_native_readiness_confirmed(newly_spawned_native)?
        {
            return Ok(last);
        }
        if started.elapsed() >= timeout {
            break;
        }
        thread::sleep(poll.min(timeout.saturating_sub(started.elapsed())));
        last = probe_infrastructure_health(runtime);
    }
    if infrastructure_ready(&last) && newly_spawned_native.is_some() {
        bail!(
            "native_embedding_identity_unsettled: healthy embedding endpoint {} was not proven to belong to the newly spawned native launch",
            runtime.embedding.endpoint
        );
    }
    Ok(last)
}

fn infrastructure_ready(health: &InfrastructureHealth) -> bool {
    health.embed_reachable
}

fn newly_spawned_native_readiness_confirmed(
    launch: Option<&crate::health::EmbeddingLaunchMetadata>,
) -> Result<bool> {
    let Some(launch) = launch else {
        return Ok(true);
    };
    let Some(log_path) = launch.log_path.as_deref().map(Path::new) else {
        return Ok(false);
    };
    if !native_embedding_log_reports_listener(log_path, &launch.endpoint) {
        return Ok(false);
    }
    crate::sidecar::ensure_native_embedding_launch_identity(launch)
        .context("validate newly spawned native embedding readiness identity")?;
    Ok(true)
}

fn native_embedding_log_reports_bind_failure(path: &Path) -> bool {
    current_native_embedding_log_tail(path).is_some_and(|current_launch| {
        current_launch.contains("address already in use")
            || current_launch.contains("only one usage of each socket address")
            || current_launch.contains("failed to bind")
            || current_launch.contains("bind() failed")
            || current_launch.contains("error while attempting to bind")
    })
}

fn native_embedding_log_reports_listener(path: &Path, endpoint: &str) -> bool {
    let Some(listener_url) = endpoint.strip_suffix("/v1/embeddings") else {
        return false;
    };
    let expected = format!("listening on {}", listener_url.to_ascii_lowercase());
    current_native_embedding_log_tail(path)
        .is_some_and(|current_launch| current_launch.contains(&expected))
}

fn current_native_embedding_log_tail(path: &Path) -> Option<String> {
    const MAX_LOG_TAIL_BYTES: u64 = 64 * 1024;
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(MAX_LOG_TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return None;
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return None;
    }
    let tail = String::from_utf8_lossy(&tail).to_ascii_lowercase();
    let current_launch_start = tail.rfind("starting native llama.cpp embedding server:")?;
    Some(tail[current_launch_start..].to_string())
}
