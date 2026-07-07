use crate::config::{
    EmbeddingServerLaunchMode, NATIVE_LLAMA_MANAGED_CACHE_REL_PATH,
    NATIVE_LLAMA_SOURCE_CACHE_REL_PATH, SidecarLayout, SidecarProfile, SidecarRuntimeConfig,
    embedding_server_launch_mode, retrieval_compose_profile, user_cache_root,
};
use crate::health::{InfrastructureHealth, probe_infrastructure_health};
use crate::qdrant_storage::{
    BootstrapStorageScope, DEFAULT_QDRANT_COLLECTION_RETENTION, QdrantStorageRepairReport,
    repair_qdrant_storage,
};
use crate::sidecar::{SidecarStateFile, sidecar_up_with_runtime_and_launch_metadata};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Relative path from repository root to the retrieval compose file.
pub const DEFAULT_COMPOSE_REL_PATH: &str = "docker/retrieval-compose.yml";
const BUNDLED_RETRIEVAL_COMPOSE: &str = include_str!("../../../docker/retrieval-compose.yml");
const DOCKER_ADDRESS_POOL_EXHAUSTED_REASON: &str = "docker_address_pool_exhausted";
const DOCKER_ADDRESS_POOL_EXHAUSTED_NEEDLE: &str =
    "all predefined address pools have been fully subnetted";
const LINUX_VULKAN_RENDER_NODE: &str = "/dev/dri";
const VULKAN_COMPOSE_OVERRIDE: &str =
    "services:\n  embed:\n    devices:\n      - /dev/dri:/dev/dri\n";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct EmbedModelInventory {
    pub model_dir: Option<String>,
    pub required_gguf: String,
    pub required_gguf_present: bool,
    pub candidate_dirs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BootstrapReport {
    pub state: SidecarStateFile,
    pub infrastructure: InfrastructureHealth,
    pub compose_started: bool,
    pub compose_file: Option<PathBuf>,
    pub storage_repair: QdrantStorageRepairReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeEmbeddingServerLaunch {
    executable: PathBuf,
    model_path: PathBuf,
    args: Vec<String>,
    log_path: PathBuf,
}

#[derive(Debug, Clone)]
struct NativeLlamaCandidate {
    path: PathBuf,
    backend: Option<crate::config::LlamaSidecarBackend>,
}

#[derive(Debug, Deserialize)]
struct NativeLlamaInstallManifest {
    artifact: String,
    artifact_sha256: String,
    executable_rel_path: String,
    executable_sha256: String,
}

/// Prepare cache dirs, optionally start Docker Compose, write sidecar state, wait for probes.
pub fn bootstrap_sidecars(
    repo_root: Option<&Path>,
    storage_scope: &BootstrapStorageScope,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
) -> Result<BootstrapReport> {
    bootstrap_sidecars_with_profile(
        repo_root,
        storage_scope,
        compose_file,
        skip_compose,
        wait_timeout,
        SidecarProfile::Local,
    )
}

pub fn bootstrap_sidecars_with_profile(
    repo_root: Option<&Path>,
    storage_scope: &BootstrapStorageScope,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
    profile: SidecarProfile,
) -> Result<BootstrapReport> {
    let runtime = SidecarRuntimeConfig::for_project_profile(repo_root, profile);
    bootstrap_sidecars_with_runtime(
        &runtime,
        repo_root,
        storage_scope,
        compose_file,
        skip_compose,
        wait_timeout,
    )
}

pub fn bootstrap_sidecars_with_runtime(
    runtime: &SidecarRuntimeConfig,
    repo_root: Option<&Path>,
    storage_scope: &BootstrapStorageScope,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
) -> Result<BootstrapReport> {
    bootstrap_sidecars_with_runtime_progress(
        runtime,
        repo_root,
        storage_scope,
        compose_file,
        skip_compose,
        wait_timeout,
        |_| {},
    )
}

pub fn bootstrap_sidecars_with_runtime_progress(
    runtime: &SidecarRuntimeConfig,
    repo_root: Option<&Path>,
    storage_scope: &BootstrapStorageScope,
    compose_file: Option<&Path>,
    skip_compose: bool,
    wait_timeout: Duration,
    mut progress: impl FnMut(&'static str),
) -> Result<BootstrapReport> {
    let layout = runtime.layout.clone();
    runtime.activate_embed_url_default();
    layout.ensure_data_dirs()?;
    let storage_repair =
        repair_qdrant_storage(&layout, storage_scope, DEFAULT_QDRANT_COLLECTION_RETENTION)?;
    let launch_mode = embedding_server_launch_mode()?;
    let native_embedding = (launch_mode == EmbeddingServerLaunchMode::NativeSpawned)
        .then(|| native_embedding_server_launch(repo_root, runtime))
        .transpose()?;

    let resolved_compose = if skip_compose {
        None
    } else {
        Some(resolve_compose_file(repo_root, compose_file)?)
    };

    let compose_started = if let Some(path) = resolved_compose.as_ref() {
        with_bootstrap_progress(&mut progress, "container startup", || {
            docker_compose_up(path, repo_root, runtime, launch_mode, false)
        })?;
        true
    } else {
        false
    };
    if let Some(launch) = native_embedding.as_ref() {
        with_bootstrap_progress(&mut progress, "model/bootstrap", || {
            spawn_native_embedding_server(launch)
        })?;
    }

    let state = sidecar_up_with_runtime_and_launch_metadata(
        runtime,
        resolved_compose.as_deref(),
        native_embedding
            .as_ref()
            .map(|launch| embedding_launch_metadata(launch, runtime, repo_root)),
    )?;

    let infrastructure = if !wait_timeout.is_zero() {
        with_bootstrap_progress(&mut progress, "model/bootstrap", || {
            wait_for_infrastructure(&layout, wait_timeout)
        })?
    } else {
        probe_infrastructure_health(&layout)
    };

    if compose_started
        && !infrastructure_ready(&infrastructure)
        && let Some(path) = resolved_compose.as_ref()
    {
        with_bootstrap_progress(&mut progress, "container refresh", || {
            docker_compose_up(path, repo_root, runtime, launch_mode, true)
        })?;
        if !wait_timeout.is_zero() {
            let _ = with_bootstrap_progress(&mut progress, "model/bootstrap", || {
                wait_for_infrastructure(&layout, wait_timeout)
            })?;
        }
    }

    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let infrastructure = crate::health::probe_infrastructure_health_with_embedding_device(
        &layout,
        &embedding_device,
    );

    Ok(BootstrapReport {
        state,
        infrastructure,
        compose_started,
        compose_file: resolved_compose,
        storage_repair,
    })
}

fn infrastructure_ready(health: &InfrastructureHealth) -> bool {
    health.zoekt_reachable && health.qdrant_reachable && health.embed_reachable
}

fn with_bootstrap_progress<T>(
    progress: &mut impl FnMut(&'static str),
    phase: &'static str,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    progress(phase);
    action()
}

pub fn resolve_compose_file(
    repo_root: Option<&Path>,
    override_path: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = override_path {
        let path = path.to_path_buf();
        if path.is_file() {
            return Ok(path);
        }
        bail!("compose file not found: {}", path.display());
    }
    if let Ok(path) = std::env::var("CODESTORY_RETRIEVAL_COMPOSE_FILE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "CODESTORY_RETRIEVAL_COMPOSE_FILE is set but not a file: {}",
            path.display()
        );
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(root) = repo_root {
        roots.push(root.to_path_buf());
    }
    if let Ok(dir) = std::env::current_dir()
        && !roots.iter().any(|existing| existing == &dir)
    {
        roots.push(dir);
    }
    for root in roots {
        let candidate = root.join(DEFAULT_COMPOSE_REL_PATH);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    write_bundled_compose_file(&user_cache_root())
}

fn write_bundled_compose_file(cache_root: &Path) -> Result<PathBuf> {
    let compose_path = cache_root.join("retrieval-compose.yml");
    std::fs::create_dir_all(cache_root)
        .with_context(|| format!("create CodeStory cache dir {}", cache_root.display()))?;
    if compose_path.is_file()
        && std::fs::read_to_string(&compose_path)
            .map(|contents| contents == BUNDLED_RETRIEVAL_COMPOSE)
            .unwrap_or(false)
    {
        return Ok(compose_path);
    }
    std::fs::write(&compose_path, BUNDLED_RETRIEVAL_COMPOSE)
        .with_context(|| format!("write bundled retrieval compose {}", compose_path.display()))?;
    Ok(compose_path)
}

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn docker_compose_up(
    compose_file: &Path,
    repo_root: Option<&Path>,
    runtime: &SidecarRuntimeConfig,
    launch_mode: EmbeddingServerLaunchMode,
    force_recreate: bool,
) -> Result<()> {
    let layout = &runtime.layout;
    if !docker_available() {
        bail!(
            "docker is not available on PATH. Install Docker Desktop (Windows) or Docker Engine, \
             then re-run bootstrap. Manual Qdrant: docker run -p 6333:6333 -p 6334:6334 \
             -v \"{}:/qdrant/storage\" {}",
            layout.qdrant_data_dir.display(),
            crate::config::QDRANT_IMAGE_PIN
        );
    }

    std::fs::create_dir_all(&layout.qdrant_data_dir).with_context(|| {
        format!(
            "create Qdrant data dir {}",
            layout.qdrant_data_dir.display()
        )
    })?;
    std::fs::create_dir_all(&layout.zoekt_data_dir)
        .with_context(|| format!("create Zoekt data dir {}", layout.zoekt_data_dir.display()))?;

    let workdir = repo_root
        .or_else(|| compose_file.parent().and_then(|p| p.parent()))
        .unwrap_or_else(|| Path::new("."));

    let mut command = docker_compose_command()?;
    let compose_profile = retrieval_compose_profile();
    remove_container_if_present("codestory-zoekt-stub")?;
    let embedding_device = crate::embeddings::embedding_device_readiness();
    let accelerator_request = crate::embeddings::embedding_accelerator_request();
    let vulkan_override = maybe_write_vulkan_compose_override(
        &user_cache_root(),
        Path::new(LINUX_VULKAN_RENDER_NODE),
        accelerator_request
            .as_ref()
            .is_some_and(|request| request.provider == "vulkan"),
    )?;
    command
        .arg("compose")
        .arg("-p")
        .arg(&runtime.compose_project)
        .arg("-f")
        .arg(compose_file);
    if let Some(override_file) = vulkan_override.as_ref() {
        command.arg("-f").arg(override_file);
    }
    command.arg("up").arg("-d");
    if force_recreate {
        command.arg("--force-recreate");
    }
    command
        .current_dir(workdir)
        .env(
            "CODESTORY_QDRANT_DATA_DIR",
            docker_bind_path(&layout.qdrant_data_dir),
        )
        .env(
            "CODESTORY_QDRANT_HTTP_PORT",
            layout.qdrant_http_port.to_string(),
        )
        .env(
            "CODESTORY_QDRANT_GRPC_PORT",
            layout.qdrant_grpc_port.to_string(),
        )
        .env("CODESTORY_ZOEKT_PORT", layout.zoekt_http_port.to_string())
        .env(
            "CODESTORY_ZOEKT_DATA_DIR",
            docker_bind_path(&layout.zoekt_data_dir),
        )
        .env(
            "CODESTORY_EMBED_MODEL_DIR",
            docker_bind_path(&embed_model_dir(repo_root, layout)?),
        )
        .env("CODESTORY_EMBED_PORT", runtime.embed_http_port.to_string())
        .env(
            "CODESTORY_EMBED_LLAMACPP_URL",
            SidecarLayout::embed_base_url(runtime.embed_http_port),
        )
        .env(
            "CODESTORY_EMBED_DEVICE_STATE",
            embedding_device.observed_state,
        )
        .env("CODESTORY_SIDECAR_NAMESPACE", &runtime.namespace)
        .env("CODESTORY_SIDECAR_PROFILE", runtime.profile.as_str())
        .env("CODESTORY_SIDECAR_OWNER", "codestory")
        .env("COMPOSE_PROFILES", compose_profile);
    if let Some(provider) = embedding_device.detected_provider.as_deref() {
        command.env("CODESTORY_EMBED_DEVICE_PROVIDER", provider);
    }
    if let Some(gpu) = embedding_device.detected_gpu.as_deref() {
        command.env("CODESTORY_EMBED_DEVICE_NAME", gpu);
    }
    if let Some(request) = accelerator_request {
        let device = request.device;
        let n_gpu_layers = request.n_gpu_layers;
        command
            .env("CODESTORY_EMBED_LLAMACPP_N_GPU_LAYERS", &n_gpu_layers)
            .env("LLAMA_ARG_N_GPU_LAYERS", n_gpu_layers);
        if let Some(device) = device {
            command
                .env("CODESTORY_EMBED_LLAMACPP_DEVICE", &device)
                .env("LLAMA_ARG_DEVICE", device);
        } else {
            command
                .env_remove("CODESTORY_EMBED_LLAMACPP_DEVICE")
                .env_remove("LLAMA_ARG_DEVICE");
        }
    } else {
        command
            .env_remove("CODESTORY_EMBED_LLAMACPP_DEVICE")
            .env_remove("LLAMA_ARG_DEVICE")
            .env_remove("LLAMA_ARG_N_GPU_LAYERS");
    }
    command.args(docker_compose_services_for_launch_mode(launch_mode));

    let output = command.output().context("spawn docker compose")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{}",
            docker_compose_up_failure_message(output.status.code(), &stdout, &stderr, repo_root)
        );
    }
    Ok(())
}

fn maybe_write_vulkan_compose_override(
    cache_root: &Path,
    host_render_node: &Path,
    accelerator_requested: bool,
) -> Result<Option<PathBuf>> {
    if !accelerator_requested || !host_render_node.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(cache_root)
        .with_context(|| format!("create CodeStory cache dir {}", cache_root.display()))?;
    let override_path = cache_root.join("retrieval-compose-vulkan.override.yml");
    if override_path.is_file()
        && std::fs::read_to_string(&override_path)
            .map(|contents| contents == VULKAN_COMPOSE_OVERRIDE)
            .unwrap_or(false)
    {
        return Ok(Some(override_path));
    }
    std::fs::write(&override_path, VULKAN_COMPOSE_OVERRIDE).with_context(|| {
        format!(
            "write Vulkan retrieval compose override {}",
            override_path.display()
        )
    })?;
    Ok(Some(override_path))
}

fn docker_compose_up_failure_message(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    repo_root: Option<&Path>,
) -> String {
    if docker_address_pool_exhausted(stdout) || docker_address_pool_exhausted(stderr) {
        let project = repo_root
            .map(|path| quoted_project_arg(&path.display().to_string()))
            .unwrap_or_else(|| "<repo>".to_string());
        return format!(
            "docker compose up failed (exit {exit_code:?}): reason={DOCKER_ADDRESS_POOL_EXHAUSTED_REASON}\n\
Docker's predefined address pools are exhausted. Run read-only inventory: \
`codestory-cli sidecar inventory --project {project} --format markdown` \
or `codestory-cli sidecar inventory --project {project} --format json`.\n\
Raw docker compose output:\n{stdout}{stderr}"
        );
    }

    format!("docker compose up failed (exit {exit_code:?}):\n{stdout}{stderr}")
}

fn quoted_project_arg(project: &str) -> String {
    format!("\"{}\"", project.replace('"', "\\\""))
}

fn docker_address_pool_exhausted(details: &str) -> bool {
    details
        .to_ascii_lowercase()
        .contains(DOCKER_ADDRESS_POOL_EXHAUSTED_NEEDLE)
}

pub fn docker_compose_down_for_state(state: &SidecarStateFile) -> Result<()> {
    if state.owner != "codestory" || state.profile != SidecarProfile::Agent.as_str() {
        return Ok(());
    }
    let Some(compose_file) = state.compose_file.as_ref().map(PathBuf::from) else {
        return Ok(());
    };
    if !compose_file.is_file() || !docker_available() {
        return Ok(());
    }
    let output = docker_compose_command()?
        .arg("compose")
        .arg("-p")
        .arg(&state.compose_project)
        .arg("-f")
        .arg(&compose_file)
        .arg("down")
        .arg("--remove-orphans")
        .output()
        .context("spawn docker compose down")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "docker compose down failed for owned sidecar namespace {} (exit {:?}):\n{stdout}{stderr}",
            state.namespace,
            output.status.code()
        );
    }
    Ok(())
}

fn docker_compose_command() -> Result<Command> {
    Ok(Command::new("docker"))
}

fn docker_compose_services_for_launch_mode(
    mode: EmbeddingServerLaunchMode,
) -> &'static [&'static str] {
    match mode {
        EmbeddingServerLaunchMode::DockerComposeEmbed => &[],
        EmbeddingServerLaunchMode::NativeSpawned => &["qdrant", "zoekt"],
    }
}

fn native_embedding_server_launch(
    repo_root: Option<&Path>,
    runtime: &SidecarRuntimeConfig,
) -> Result<NativeEmbeddingServerLaunch> {
    ensure_selected_managed_native_llama_server(repo_root)?;
    let executable = native_llama_server_path(repo_root)?;
    let model_path =
        embed_model_dir(repo_root, &runtime.layout)?.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF);
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
        let port = runtime.embed_http_port.to_string();
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
        runtime.embed_http_port.to_string(),
    ]
}

fn embedding_launch_metadata(
    native_launch: &NativeEmbeddingServerLaunch,
    runtime: &SidecarRuntimeConfig,
    repo_root: Option<&Path>,
) -> crate::health::EmbeddingLaunchMetadata {
    crate::health::EmbeddingLaunchMetadata {
        provider: "llamacpp".to_string(),
        launch_mode: EmbeddingServerLaunchMode::NativeSpawned
            .as_str()
            .to_string(),
        endpoint: SidecarLayout::embed_base_url(runtime.embed_http_port),
        executable_source: Some(native_llama_executable_source(
            &native_launch.executable,
            repo_root,
        )),
        executable_path: Some(native_launch.executable.display().to_string()),
        model_path: Some(native_launch.model_path.display().to_string()),
        requested_device: crate::embeddings::embedding_accelerator_request()
            .and_then(|request| request.device),
    }
}

fn native_llama_executable_source(path: &Path, repo_root: Option<&Path>) -> String {
    if std::env::var("CODESTORY_EMBED_NATIVE_LLAMA_SERVER").is_ok() {
        return "env:CODESTORY_EMBED_NATIVE_LLAMA_SERVER".to_string();
    }
    if let Some(backend) = selected_native_llama_backend() {
        for backend in matching_native_llama_backends(&backend.provider) {
            let rel_path = native_llama_backend_rel_path(&backend);
            if path == user_cache_root().join(&rel_path) {
                return "managed_cache".to_string();
            }
            if let Some(root) = repo_root
                && path == root.join(&rel_path)
            {
                return "repo_managed_cache".to_string();
            }
        }
    }
    if path == user_cache_root().join(NATIVE_LLAMA_MANAGED_CACHE_REL_PATH) {
        return "managed_cache".to_string();
    }
    if let Some(root) = repo_root {
        if path == root.join(NATIVE_LLAMA_SOURCE_CACHE_REL_PATH) {
            return "source_cache".to_string();
        }
        if path == root.join(NATIVE_LLAMA_MANAGED_CACHE_REL_PATH) {
            return "repo_managed_cache".to_string();
        }
    }
    "resolved_path".to_string()
}

fn native_llama_server_path(repo_root: Option<&Path>) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CODESTORY_EMBED_NATIVE_LLAMA_SERVER") {
        return validate_explicit_native_llama_server(PathBuf::from(path));
    }
    native_llama_server_path_from_candidates(native_llama_server_candidates(repo_root))
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
        .iter()
        .find(|candidate| candidate.backend.is_some())
        .map(|candidate| candidate.path.display().to_string())
        .unwrap_or_else(|| {
            user_cache_root()
                .join(NATIVE_LLAMA_MANAGED_CACHE_REL_PATH)
                .display()
                .to_string()
        });
    let mut invalid_managed_candidate = None;
    for candidate in candidates {
        if !candidate.path.is_file() {
            continue;
        }
        if let Some(backend) = &candidate.backend
            && let Err(error) = validate_managed_native_llama_server(&candidate.path, backend)
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

fn native_llama_server_candidates(repo_root: Option<&Path>) -> Vec<NativeLlamaCandidate> {
    if let Some(backend) = selected_native_llama_backend() {
        let mut candidates = Vec::new();
        for backend in matching_native_llama_backends(&backend.provider) {
            let rel_path = native_llama_backend_rel_path(&backend);
            candidates.push(NativeLlamaCandidate {
                path: user_cache_root().join(&rel_path),
                backend: Some(backend.clone()),
            });
            if let Some(root) = repo_root {
                candidates.push(NativeLlamaCandidate {
                    path: root.join(rel_path),
                    backend: Some(backend),
                });
            }
        }
        return candidates;
    }
    let mut candidates = vec![NativeLlamaCandidate {
        path: user_cache_root().join(NATIVE_LLAMA_MANAGED_CACHE_REL_PATH),
        backend: None,
    }];
    if let Some(root) = repo_root {
        candidates.push(NativeLlamaCandidate {
            path: root.join(NATIVE_LLAMA_SOURCE_CACHE_REL_PATH),
            backend: None,
        });
        candidates.push(NativeLlamaCandidate {
            path: root.join(NATIVE_LLAMA_MANAGED_CACHE_REL_PATH),
            backend: None,
        });
    }
    candidates
}

fn selected_native_llama_backend() -> Option<crate::config::LlamaSidecarBackend> {
    crate::embeddings::embedding_accelerator_request()
        .and_then(|request| crate::config::selected_llama_sidecar_backend(&request.provider))
}

fn matching_native_llama_backends(provider: &str) -> Vec<crate::config::LlamaSidecarBackend> {
    crate::config::llama_sidecar_backends(provider)
}

fn native_llama_backend_rel_path(backend: &crate::config::LlamaSidecarBackend) -> PathBuf {
    Path::new(&backend.managed_cache_rel_dir).join(&backend.executable_rel_path)
}

fn ensure_selected_managed_native_llama_server(repo_root: Option<&Path>) -> Result<()> {
    if std::env::var("CODESTORY_EMBED_NATIVE_LLAMA_SERVER").is_ok() {
        return Ok(());
    }
    if let Some(backend) = selected_native_llama_backend() {
        if native_llama_server_path_from_candidates(native_llama_server_candidates(repo_root))
            .is_ok()
        {
            return Ok(());
        }
        ensure_managed_native_llama_server(&backend)?;
    }
    Ok(())
}

fn ensure_managed_native_llama_server(backend: &crate::config::LlamaSidecarBackend) -> Result<()> {
    let executable = user_cache_root().join(native_llama_backend_rel_path(backend));
    if executable.is_file() && validate_managed_native_llama_server(&executable, backend).is_ok() {
        return Ok(());
    }
    let temp_root = managed_llama_temp_root()?;
    let archive = temp_root.join(&backend.artifact);
    let install_result = (|| {
        download_managed_native_llama_server_archive(backend, &archive)?;
        install_managed_native_llama_server_from_archive(backend, &archive, &executable)
    })();
    let cleanup_result = fs::remove_dir_all(&temp_root);
    if let Err(error) = cleanup_result
        && install_result.is_ok()
    {
        return Err(error).with_context(|| format!("remove {}", temp_root.display()));
    }
    install_result
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

fn download_managed_native_llama_server_archive(
    backend: &crate::config::LlamaSidecarBackend,
    archive: &Path,
) -> Result<()> {
    let response = ureq::get(&backend.url)
        .call()
        .with_context(|| format!("download {}", backend.url))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", backend.url))?;
    fs::write(archive, &bytes).with_context(|| format!("write {}", archive.display()))?;
    verify_sha256(archive, &backend.sha256)
        .with_context(|| format!("verify {}", archive.display()))?;
    Ok(())
}

fn install_managed_native_llama_server_from_archive(
    backend: &crate::config::LlamaSidecarBackend,
    archive: &Path,
    executable: &Path,
) -> Result<()> {
    verify_sha256(archive, &backend.sha256)
        .with_context(|| format!("verify {}", archive.display()))?;
    let extract_root = archive
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server archive has no parent"))?
        .join("extract");
    fs::create_dir_all(&extract_root)
        .with_context(|| format!("create {}", extract_root.display()))?;
    let member_path = safe_archive_member_path(&backend.executable_archive_path)?;
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&extract_root)
        .arg(&backend.executable_archive_path)
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
    let parent = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server executable has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let partial = executable.with_extension("download");
    fs::copy(&extracted, &partial).with_context(|| {
        format!(
            "copy extracted llama-server {} to {}",
            extracted.display(),
            partial.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&partial)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&partial, permissions)?;
    }
    let executable_sha = sha256_file(&partial)?;
    if !executable_sha.eq_ignore_ascii_case(&backend.executable_sha256) {
        bail!(
            "managed llama-server executable checksum mismatch for {}: expected {}, got {}",
            partial.display(),
            backend.executable_sha256,
            executable_sha
        );
    }
    fs::rename(&partial, executable).with_context(|| {
        format!(
            "move downloaded llama-server {} to {}",
            partial.display(),
            executable.display()
        )
    })?;
    write_managed_native_llama_install_manifest(backend, executable, &executable_sha)?;
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
    let manifest_path = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed llama-server path has no parent"))?
        .join("install-manifest.json");
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

fn spawn_native_embedding_server(launch: &NativeEmbeddingServerLaunch) -> Result<()> {
    if let Some(parent) = launch.log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create native llama.cpp log dir {}", parent.display()))?;
    }
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&launch.log_path)
        .with_context(|| format!("open native llama.cpp log {}", launch.log_path.display()))?;
    writeln!(
        log,
        "starting native llama.cpp embedding server: {} {}",
        launch.executable.display(),
        launch.args.join(" ")
    )
    .ok();
    let stdout = log
        .try_clone()
        .context("clone native llama.cpp stdout log")?;
    let _child = Command::new(&launch.executable)
        .args(&launch.args)
        .current_dir(launch.executable.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(log))
        .spawn()
        .with_context(|| {
            format!(
                "spawn native llama.cpp server {}",
                launch.executable.display()
            )
        })?;
    Ok(())
}

fn remove_container_if_present(name: &str) -> Result<()> {
    let inspect = Command::new("docker")
        .args(["container", "inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("inspect docker container {name}"))?;
    if !inspect.success() {
        return Ok(());
    }
    let output = Command::new("docker")
        .args(["rm", "-f", name])
        .output()
        .with_context(|| format!("remove stale docker container {name}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "failed to remove stale docker container {name} (exit {:?}):\n{stdout}{stderr}",
            output.status.code()
        );
    }
    Ok(())
}

fn docker_bind_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let without_verbatim = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    };
    without_verbatim.replace('\\', "/")
}

fn embed_model_dir(repo_root: Option<&Path>, layout: &SidecarLayout) -> Result<PathBuf> {
    let inventory = embed_model_inventory(repo_root, layout);
    if let Some(model_dir) = inventory
        .required_gguf_present
        .then_some(inventory.model_dir.as_ref())
        .flatten()
    {
        return Ok(PathBuf::from(model_dir));
    }
    if std::env::var("CODESTORY_EMBED_MODEL_DIR").is_ok() {
        anyhow::bail!(
            "CODESTORY_EMBED_MODEL_DIR does not contain {}; run `node scripts/setup-retrieval-env.mjs --fetch-embed-model` or set CODESTORY_EMBED_MODEL_DIR",
            crate::embeddings::BGE_BASE_EN_V1_5_GGUF
        );
    }
    anyhow::bail!(
        "No llama.cpp embedding model directory contains {}; run `node scripts/setup-retrieval-env.mjs --fetch-embed-model` or set CODESTORY_EMBED_MODEL_DIR",
        crate::embeddings::BGE_BASE_EN_V1_5_GGUF
    )
}

pub fn embed_model_inventory(
    repo_root: Option<&Path>,
    layout: &SidecarLayout,
) -> EmbedModelInventory {
    let candidates = embed_model_candidates(repo_root, layout);
    let model_dir = candidates
        .iter()
        .find(|candidate| embed_model_dir_ready(candidate))
        .or_else(|| candidates.first())
        .map(|path| path.display().to_string());
    let required_gguf_present = model_dir
        .as_ref()
        .is_some_and(|path| embed_model_dir_ready(Path::new(path)));
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

fn embed_model_candidates(repo_root: Option<&Path>, layout: &SidecarLayout) -> Vec<PathBuf> {
    if let Ok(path) = std::env::var("CODESTORY_EMBED_MODEL_DIR") {
        return vec![PathBuf::from(path)];
    }
    let workdir = repo_root
        .or_else(|| Some(Path::new(".")))
        .unwrap_or(Path::new("."));
    let fallback = layout
        .qdrant_data_dir
        .parent()
        .map(|parent| parent.join("embed-models"))
        .unwrap_or_else(|| layout.qdrant_data_dir.join("embed-models"));
    let mut candidates = Vec::new();
    for candidate in [
        workdir.join("target").join("retrieval-models"),
        workdir.join("models").join("gguf").join("bge-base-en-v1.5"),
        user_cache_root().join("embed-models"),
        fallback,
    ] {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

#[cfg(test)]
fn embed_model_dir_from_candidates(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<PathBuf> {
    for candidate in candidates {
        if embed_model_dir_ready(&candidate) {
            return Ok(candidate);
        }
    }
    bail!(
        "No llama.cpp embedding model directory contains {}; run `node scripts/setup-retrieval-env.mjs --fetch-embed-model` or set CODESTORY_EMBED_MODEL_DIR",
        crate::embeddings::BGE_BASE_EN_V1_5_GGUF
    )
}

fn embed_model_dir_ready(path: &Path) -> bool {
    path.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
        .is_file()
}

fn wait_for_infrastructure(
    layout: &SidecarLayout,
    timeout: Duration,
) -> Result<InfrastructureHealth> {
    let started = Instant::now();
    let poll = Duration::from_millis(500);
    let mut last = probe_infrastructure_health(layout);
    while started.elapsed() < timeout {
        if last.zoekt_reachable && last.qdrant_reachable && last.embed_reachable {
            return Ok(last);
        }
        thread::sleep(poll);
        last = probe_infrastructure_health(layout);
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn embed_model_dir_discovers_repo_models_layout() {
        let project = tempdir().expect("project");
        let model_dir = project
            .path()
            .join("models")
            .join("gguf")
            .join("bge-base-en-v1.5");
        std::fs::create_dir_all(&model_dir).expect("model dir");
        std::fs::write(
            model_dir.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF),
            b"model placeholder",
        )
        .expect("model file");
        let layout = SidecarLayout::from_env();

        assert_eq!(
            embed_model_dir(Some(project.path()), &layout).expect("model dir"),
            model_dir
        );
    }

    #[test]
    fn embed_model_dir_uses_first_candidate_with_model() {
        let empty = tempdir().expect("empty");
        let cache = tempdir().expect("cache");
        let model_dir = cache.path().join("embed-models");
        std::fs::create_dir_all(&model_dir).expect("model dir");
        std::fs::write(
            model_dir.join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF),
            b"model placeholder",
        )
        .expect("model file");

        assert_eq!(
            embed_model_dir_from_candidates([empty.path().to_path_buf(), model_dir.clone()])
                .expect("fallback model dir"),
            model_dir
        );
    }

    #[test]
    fn embed_model_dir_fails_before_empty_fallback_container() {
        let empty = tempdir().expect("empty");

        let error = embed_model_dir_from_candidates([empty.path().to_path_buf()])
            .expect_err("missing model must fail before docker compose");

        assert!(
            error
                .to_string()
                .contains(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
        );
    }

    #[test]
    fn docker_bind_path_removes_windows_verbatim_prefix() {
        assert_eq!(
            docker_bind_path(Path::new(r"\\?\C:\Users\alber\codestory\models")),
            "C:/Users/alber/codestory/models"
        );
    }

    #[test]
    fn compose_failure_classifies_docker_address_pool_exhaustion() {
        let project = Path::new("C:/repo/example project");
        let stdout = "compose stdout\n";
        let stderr =
            "failed to create network: all predefined address pools have been fully subnetted";

        let message = docker_compose_up_failure_message(Some(1), stdout, stderr, Some(project));

        assert!(message.contains("reason=docker_address_pool_exhausted"));
        assert!(message.contains(stdout));
        assert!(message.contains(stderr));
        assert!(message.contains(
            "codestory-cli sidecar inventory --project \"C:/repo/example project\" --format markdown"
        ));
        assert!(message.contains(
            "codestory-cli sidecar inventory --project \"C:/repo/example project\" --format json"
        ));
        for forbidden in [" prune", " remove", " down", " delete", " restart"] {
            assert!(
                !message.to_ascii_lowercase().contains(forbidden),
                "guidance must stay non-destructive: {message}"
            );
        }
    }

    #[test]
    fn compose_failure_preserves_generic_stderr_without_reason() {
        let stderr = "compose service failed for another reason";

        let message = docker_compose_up_failure_message(Some(17), "stdout\n", stderr, None);

        assert!(message.contains("docker compose up failed (exit Some(17))"));
        assert!(message.contains("stdout\n"));
        assert!(message.contains(stderr));
        assert!(!message.contains("docker_address_pool_exhausted"));
        assert!(!message.contains("sidecar inventory"));
    }

    #[test]
    fn bundled_compose_file_is_written_to_cache() {
        let cache = tempdir().expect("cache");
        let path = write_bundled_compose_file(cache.path()).expect("write bundled compose");

        assert_eq!(path, cache.path().join("retrieval-compose.yml"));
        let contents = std::fs::read_to_string(path).expect("read bundled compose");
        assert!(contents.contains("name: ${CODESTORY_SIDECAR_NAMESPACE:-codestory-retrieval}"));
        for image in [
            crate::config::QDRANT_IMAGE_PIN,
            crate::config::ZOEKT_WEBSERVER_IMAGE_PIN,
            crate::config::LLAMACPP_SERVER_IMAGE_PIN,
        ] {
            assert!(
                image.contains("@sha256:"),
                "image pin must include digest: {image}"
            );
            assert!(
                contents.contains(image),
                "bundled compose should contain exact image pin {image}"
            );
        }
    }

    #[test]
    fn bundled_compose_keeps_llamacpp_device_env_request_only() {
        assert!(BUNDLED_RETRIEVAL_COMPOSE.contains("- LLAMA_ARG_DEVICE"));
        assert!(BUNDLED_RETRIEVAL_COMPOSE.contains("- LLAMA_ARG_N_GPU_LAYERS"));
        assert!(!BUNDLED_RETRIEVAL_COMPOSE.contains("/dev/dri:/dev/dri"));
        assert!(!BUNDLED_RETRIEVAL_COMPOSE.contains("devices:"));
        assert!(!BUNDLED_RETRIEVAL_COMPOSE.contains("LLAMA_ARG_DEVICE:"));
        assert!(!BUNDLED_RETRIEVAL_COMPOSE.contains(":-none"));
    }

    #[test]
    fn vulkan_compose_override_is_only_written_when_host_render_node_exists() {
        let cache = tempdir().expect("cache");
        let missing = cache.path().join("missing-dri");
        let skipped =
            maybe_write_vulkan_compose_override(cache.path(), &missing, true).expect("skip");
        assert_eq!(skipped, None);

        let disabled =
            maybe_write_vulkan_compose_override(cache.path(), &missing, false).expect("disabled");
        assert_eq!(disabled, None);

        let render_node = cache.path().join("dri");
        std::fs::create_dir(&render_node).expect("render node dir");
        let written =
            maybe_write_vulkan_compose_override(cache.path(), &render_node, true).expect("write");
        let written = written.expect("override path");
        let contents = std::fs::read_to_string(written).expect("override contents");
        assert!(contents.contains("/dev/dri:/dev/dri"));
    }

    #[test]
    fn native_launch_mode_limits_compose_to_qdrant_and_zoekt() {
        assert_eq!(
            docker_compose_services_for_launch_mode(EmbeddingServerLaunchMode::NativeSpawned),
            &["qdrant", "zoekt"]
        );
        assert!(
            docker_compose_services_for_launch_mode(EmbeddingServerLaunchMode::DockerComposeEmbed)
                .is_empty()
        );
    }

    #[test]
    fn bootstrap_progress_is_emitted_before_blocking_work() {
        let phases = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let progress_phases = std::rc::Rc::clone(&phases);
        let action_phases = std::rc::Rc::clone(&phases);

        with_bootstrap_progress(
            &mut |phase| progress_phases.borrow_mut().push(phase),
            "model/bootstrap",
            || {
                assert_eq!(&*action_phases.borrow(), &["model/bootstrap"]);
                Ok(())
            },
        )
        .expect("progress wrapper should return action result");

        assert_eq!(&*phases.borrow(), &["model/bootstrap"]);
    }

    #[test]
    fn native_explicit_executable_requires_absolute_file() {
        let relative = validate_explicit_native_llama_server(PathBuf::from("llama-server.exe"))
            .expect_err("relative executable must fail closed");
        assert!(relative.to_string().contains("absolute path"));

        let temp = tempdir().expect("temp");
        let exe = temp.path().join("llama-server.exe");
        std::fs::write(&exe, b"fake exe").expect("exe");
        assert_eq!(
            validate_explicit_native_llama_server(exe.clone()).expect("absolute file"),
            exe
        );
    }

    #[test]
    fn native_executable_candidates_do_not_fall_back_to_path() {
        let error = native_llama_server_path_from_candidates(vec![NativeLlamaCandidate {
            path: PathBuf::from("llama-server.exe"),
            backend: None,
        }])
        .expect_err("bare executable must not resolve through PATH");

        assert!(
            error
                .to_string()
                .contains("ambient PATH lookup is not allowed")
        );
    }

    #[test]
    fn windows_manifest_candidates_prefer_b9902_and_keep_legacy_b9058() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "windows/x86_64");
        let _device = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_DEVICE");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");

        let candidates = native_llama_server_candidates(None);
        let ids = candidates
            .iter()
            .filter_map(|candidate| {
                candidate
                    .backend
                    .as_ref()
                    .map(|backend| backend.id.as_str())
            })
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                "windows-x86_64-vulkan",
                "windows-x86_64-vulkan-b9058-legacy"
            ]
        );
        assert!(
            candidates[0]
                .path
                .display()
                .to_string()
                .contains("llama-b9902-bin-win-vulkan-x64")
        );
        assert!(
            candidates[1]
                .path
                .display()
                .to_string()
                .contains("llama-b9058-bin-win-vulkan-x64")
        );
    }

    #[test]
    fn legacy_windows_vulkan_cache_requires_matching_install_manifest() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "windows/x86_64");
        let legacy = crate::config::llama_sidecar_backends("vulkan")
            .into_iter()
            .find(|backend| backend.id == "windows-x86_64-vulkan-b9058-legacy")
            .expect("legacy windows vulkan backend");
        let temp = tempdir().expect("temp");
        let exe = temp.path().join("llama-server.exe");
        std::fs::write(&exe, b"legacy exe").expect("exe");

        let missing_manifest =
            native_llama_server_path_from_candidates(vec![NativeLlamaCandidate {
                path: exe.clone(),
                backend: Some(legacy.clone()),
            }])
            .expect_err("legacy cache still needs install manifest");
        assert!(
            missing_manifest
                .to_string()
                .contains("install-manifest.json")
        );

        let exe_sha = sha256_file(&exe).expect("exe sha");
        let manifest = serde_json::json!({
            "backend": legacy.id,
            "artifact": legacy.artifact,
            "artifact_sha256": legacy.sha256,
            "executable_rel_path": legacy.executable_rel_path,
            "executable_sha256": exe_sha,
            "source_url": legacy.url,
        });
        std::fs::write(
            temp.path().join("install-manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest"),
        )
        .expect("manifest write");
        let mut legacy = legacy;
        legacy.executable_sha256 = exe_sha;
        let selected = native_llama_server_path_from_candidates(vec![NativeLlamaCandidate {
            path: exe.clone(),
            backend: Some(legacy),
        }])
        .expect("legacy cache path with manifest");

        assert_eq!(selected, exe);
    }

    #[test]
    fn managed_native_candidate_requires_install_manifest() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let backend =
            crate::config::selected_llama_sidecar_backend("metal").expect("mac metal backend");
        let temp = tempdir().expect("temp");
        let exe = temp.path().join("llama-server");
        std::fs::write(&exe, b"fake exe").expect("exe");

        let error = native_llama_server_path_from_candidates(vec![NativeLlamaCandidate {
            path: exe,
            backend: Some(backend),
        }])
        .expect_err("missing install manifest must fail closed");

        assert!(error.to_string().contains("install-manifest.json"));
    }

    #[test]
    fn managed_native_candidate_accepts_matching_install_manifest() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let mut backend =
            crate::config::selected_llama_sidecar_backend("metal").expect("mac metal backend");
        let temp = tempdir().expect("temp");
        let exe = temp.path().join("llama-server");
        std::fs::write(&exe, b"fake exe").expect("exe");
        let executable_sha = sha256_file(&exe).expect("sha");
        backend.executable_sha256 = executable_sha.clone();
        std::fs::write(
            temp.path().join("install-manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "artifact": backend.artifact,
                "artifact_sha256": backend.sha256,
                "executable_rel_path": backend.executable_rel_path,
                "executable_sha256": executable_sha,
            }))
            .expect("manifest"),
        )
        .expect("manifest write");

        let selected = native_llama_server_path_from_candidates(vec![NativeLlamaCandidate {
            path: exe.clone(),
            backend: Some(backend),
        }])
        .expect("valid managed candidate");

        assert_eq!(selected, exe);
    }

    #[test]
    fn managed_native_candidate_rejects_manifest_blessed_wrong_executable() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let backend =
            crate::config::selected_llama_sidecar_backend("metal").expect("mac metal backend");
        let temp = tempdir().expect("temp");
        let exe = temp.path().join("llama-server");
        std::fs::write(&exe, b"wrong exe").expect("exe");
        let executable_sha = sha256_file(&exe).expect("sha");
        std::fs::write(
            temp.path().join("install-manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "artifact": backend.artifact,
                "artifact_sha256": backend.sha256,
                "executable_rel_path": backend.executable_rel_path,
                "executable_sha256": executable_sha,
            }))
            .expect("manifest"),
        )
        .expect("manifest write");

        let error = native_llama_server_path_from_candidates(vec![NativeLlamaCandidate {
            path: exe,
            backend: Some(backend),
        }])
        .expect_err("cache manifest must not bless arbitrary executable bytes");

        assert!(
            error
                .to_string()
                .contains("executable manifest checksum mismatch")
                || error.to_string().contains("executable checksum mismatch"),
            "{error}"
        );
    }

    #[test]
    fn managed_native_archive_member_path_must_be_contained() {
        assert!(safe_archive_member_path("llama-b9902/llama-server").is_ok());
        assert!(safe_archive_member_path("../llama-server").is_err());
        assert!(safe_archive_member_path("/tmp/llama-server").is_err());
        assert!(safe_archive_member_path("llama-b9902\\llama-server").is_err());
    }

    #[test]
    fn managed_native_install_extracts_archive_and_writes_manifest() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let mut backend =
            crate::config::selected_llama_sidecar_backend("metal").expect("mac metal backend");
        let temp = tempdir().expect("temp");
        let archive_root = temp.path().join("archive-root");
        let payload_dir = archive_root.join("llama-b9902");
        std::fs::create_dir_all(&payload_dir).expect("payload dir");
        std::fs::write(payload_dir.join("llama-server"), b"fake exe").expect("payload exe");
        backend.executable_archive_path = "llama-b9902/llama-server".to_string();
        backend.executable_sha256 =
            sha256_file(&payload_dir.join("llama-server")).expect("executable sha");
        let archive = temp.path().join("llama-test.tar.gz");
        let output = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&archive_root)
            .arg("llama-b9902")
            .output()
            .expect("tar");
        assert!(
            output.status.success(),
            "tar failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        backend.artifact = "llama-test.tar.gz".to_string();
        backend.sha256 = sha256_file(&archive).expect("archive sha");

        let executable = temp
            .path()
            .join("managed-embeddings")
            .join("llama")
            .join("b9902")
            .join("llama-b9902-bin-macos-arm64-metal")
            .join("llama-server");
        install_managed_native_llama_server_from_archive(&backend, &archive, &executable)
            .expect("install");

        assert_eq!(
            std::fs::read(&executable).expect("installed exe"),
            b"fake exe"
        );
        validate_managed_native_llama_server(&executable, &backend)
            .expect("installed manifest validates");
    }

    #[test]
    fn native_launch_args_use_model_port_and_default_vulkan_device() {
        let _lock = crate::test_support::env_lock();
        let _provider = EnvGuard::remove("CODESTORY_EMBED_DEVICE_PROVIDER");
        let _name = EnvGuard::remove("CODESTORY_EMBED_DEVICE_NAME");
        let _device = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_DEVICE");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "windows/x86_64");

        let temp = tempdir().expect("temp");
        let exe = temp.path().join("llama-server.exe");
        let model = temp.path().join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF);
        std::fs::write(&exe, b"fake exe").expect("exe");
        std::fs::write(&model, b"fake model").expect("model");
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Local);

        let launch =
            native_embedding_server_launch_from_paths(exe.clone(), model.clone(), &runtime);
        let model_arg = model.display().to_string();
        let port_arg = runtime.embed_http_port.to_string();

        assert_eq!(launch.executable, exe);
        assert_eq!(launch.args[0], "--embedding");
        assert!(
            launch
                .args
                .windows(2)
                .any(|pair| pair[0] == "--model" && pair[1] == model_arg.as_str())
        );
        assert!(
            launch
                .args
                .windows(2)
                .any(|pair| pair[0] == "--host" && pair[1] == "127.0.0.1")
        );
        assert!(
            launch
                .args
                .windows(2)
                .any(|pair| pair[0] == "--port" && pair[1] == port_arg.as_str())
        );
        assert!(
            launch
                .args
                .windows(2)
                .any(|pair| pair[0] == "--n-gpu-layers" && pair[1] == "99")
        );
        assert!(
            launch
                .args
                .windows(2)
                .any(|pair| pair[0] == "--device" && pair[1] == "Vulkan0")
        );
        let metadata = embedding_launch_metadata(&launch, &runtime, Some(temp.path()));
        assert_eq!(metadata.provider, "llamacpp");
        assert_eq!(metadata.launch_mode, "native_spawned");
        assert_eq!(metadata.executable_path, Some(exe.display().to_string()));
        assert_eq!(metadata.model_path, Some(model.display().to_string()));
        assert_eq!(metadata.requested_device.as_deref(), Some("Vulkan0"));
    }

    #[test]
    fn metal_native_launch_uses_gpu_layers_without_device_arg() {
        let _lock = crate::test_support::env_lock();
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let _device = EnvGuard::remove("CODESTORY_EMBED_LLAMACPP_DEVICE");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");

        let temp = tempdir().expect("temp");
        let exe = temp.path().join("llama-server");
        let model = temp.path().join(crate::embeddings::BGE_BASE_EN_V1_5_GGUF);
        std::fs::write(&exe, b"fake exe").expect("exe");
        std::fs::write(&model, b"fake model").expect("model");
        let runtime = SidecarRuntimeConfig::for_project_profile(None, SidecarProfile::Local);

        let launch =
            native_embedding_server_launch_from_paths(exe.clone(), model.clone(), &runtime);

        assert!(
            launch
                .args
                .windows(2)
                .any(|pair| pair[0] == "--n-gpu-layers" && pair[1] == "99")
        );
        assert!(!launch.args.iter().any(|arg| arg == "--device"));
        let metadata = embedding_launch_metadata(&launch, &runtime, Some(temp.path()));
        assert_eq!(metadata.requested_device, None);
    }

    #[test]
    fn native_launch_missing_model_fails_before_spawn() {
        let temp = tempdir().expect("temp");
        let error = embed_model_dir_from_candidates([temp.path().join("embed-models")])
            .expect_err("missing model should fail closed");

        assert!(
            error
                .to_string()
                .contains(crate::embeddings::BGE_BASE_EN_V1_5_GGUF)
        );
        assert!(error.to_string().contains("fetch-embed-model"));
    }

    #[test]
    fn repo_compose_file_still_wins_over_bundled_fallback() {
        let project = tempdir().expect("project");
        let compose = project.path().join(DEFAULT_COMPOSE_REL_PATH);
        std::fs::create_dir_all(compose.parent().expect("compose parent"))
            .expect("create compose parent");
        std::fs::write(&compose, "services: {}\n").expect("write compose");

        let resolved = resolve_compose_file(Some(project.path()), None).expect("resolve compose");

        assert_eq!(resolved, compose);
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
