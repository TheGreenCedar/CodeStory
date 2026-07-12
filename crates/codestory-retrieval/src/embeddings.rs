//! Query embeddings for Qdrant plus diagnostic document embedding helpers.
//!
//! Product Qdrant indexing copies stored local semantic-document vectors. The live sidecar still
//! uses **BAAI/bge-base-en-v1.5** (768-dim) via llama.cpp `/v1/embeddings` for query vectors and
//! semantic smoke checks.

use crate::outbound_http::read_bytes;
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// bge-base-en-v1.5 vector width (must match Qdrant collection and llama.cpp model).
pub const RETRIEVAL_EMBEDDING_DIM: usize = 768;

/// GGUF filename under `CODESTORY_EMBED_MODEL_DIR` (see docker/retrieval-compose.yml).
pub const BGE_BASE_EN_V1_5_GGUF: &str = "bge-base-en-v1.5.Q8_0.gguf";

pub const BGE_QUERY_PREFIX_DEFAULT: &str =
    "Represent this sentence for searching relevant passages: ";
pub const PRODUCT_EMBEDDING_RUNTIME_ID: &str = "llamacpp:bge-base-en-v1.5";

const EMBEDDING_BACKEND_ENV: &str = "CODESTORY_EMBED_BACKEND";
const ALLOW_REMOTE_EMBEDDINGS_ENV: &str = "CODESTORY_ALLOW_REMOTE_EMBEDDINGS";
const DEVICE_POLICY_ENV: &str = "CODESTORY_EMBED_DEVICE_POLICY";
const DEVICE_STATE_ENV: &str = "CODESTORY_EMBED_DEVICE_STATE";
const DEVICE_PROVIDER_ENV: &str = "CODESTORY_EMBED_DEVICE_PROVIDER";
const DEVICE_NAME_ENV: &str = "CODESTORY_EMBED_DEVICE_NAME";
const DISABLE_HOST_GPU_DETECT_ENV: &str = "CODESTORY_EMBED_DISABLE_HOST_GPU_DETECT";
const LLAMACPP_DEVICE_ENV: &str = "CODESTORY_EMBED_LLAMACPP_DEVICE";
const LLAMACPP_N_GPU_LAYERS_ENV: &str = "CODESTORY_EMBED_LLAMACPP_N_GPU_LAYERS";
const ALLOW_CPU_ENV: &str = "CODESTORY_EMBED_ALLOW_CPU";
const NATIVE_LLAMA_LOG_START_MARKER: &str = "starting native llama.cpp embedding server:";
const NATIVE_LLAMA_LOG_READ_TAIL_BYTES: u64 = 512 * 1024;
const NATIVE_LLAMA_PREVIOUS_LOG_TAIL_BYTES: u64 = 256 * 1024;
const RUNTIME_EMBED_DEVICE_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(10);
const RUNTIME_EMBED_DEVICE_OBSERVATION_POLL: Duration = Duration::from_millis(250);
const ACCELERATOR_SMOKE_TIMEOUT: Duration = Duration::from_secs(5);

const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct LlamaCppEmbeddingClient {
    config: crate::config::EmbeddingRuntimeConfig,
}

impl LlamaCppEmbeddingClient {
    pub fn new(config: &crate::config::EmbeddingRuntimeConfig) -> Result<Self> {
        if let Some(error) = config.configuration_error.as_deref() {
            bail!(error.to_string());
        }
        ensure_llamacpp_url_allowed_with_policy(&config.endpoint, config.allow_remote)?;
        Ok(Self {
            config: config.clone(),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_query_with_timeout(text, HTTP_TIMEOUT)
    }

    pub fn embed_query_with_timeout(&self, text: &str, timeout: Duration) -> Result<Vec<f32>> {
        let prefix = self.config.query_prefix.as_deref().unwrap_or_else(|| {
            if self.is_llamacpp() {
                BGE_QUERY_PREFIX_DEFAULT
            } else {
                ""
            }
        });
        self.embed_prepared_with_timeout(&format!("{prefix}{text}"), timeout)
    }

    pub fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let prefix = self.config.document_prefix.as_deref().unwrap_or_default();
        let prepared = texts
            .iter()
            .map(|text| format!("{prefix}{text}"))
            .collect::<Vec<_>>();
        if prepared.iter().any(|text| text.trim().is_empty()) {
            bail!("cannot embed empty text");
        }
        if self.is_llamacpp() {
            self.embed_llamacpp_batched(&prepared)
        } else {
            Ok(prepared
                .iter()
                .map(|text| hash_projection_embed(text, RETRIEVAL_EMBEDDING_DIM))
                .collect())
        }
    }

    pub fn embed_prepared_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.iter().any(|text| text.trim().is_empty()) {
            bail!("cannot embed empty text");
        }
        if self.is_llamacpp() {
            self.embed_llamacpp_batched(texts)
        } else {
            Ok(texts
                .iter()
                .map(|text| hash_projection_embed(text, RETRIEVAL_EMBEDDING_DIM))
                .collect())
        }
    }

    pub fn probe(&self) -> EmbeddingRuntimeProbe {
        let started = Instant::now();
        let result = self
            .embed_query("codestory health probe")
            .map(|embedding| vec![embedding]);
        let elapsed_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
        match result {
            Ok(vectors) => EmbeddingRuntimeProbe {
                reachable: true,
                detail: format!(
                    "{} embeddings reachable dim={}",
                    self.backend_label(),
                    vectors.first().map(|vector| vector.len()).unwrap_or(0)
                ),
                elapsed_ms,
            },
            Err(error) => EmbeddingRuntimeProbe {
                reachable: false,
                detail: format!("{} embeddings unavailable: {error}", self.backend_label()),
                elapsed_ms,
            },
        }
    }

    pub fn backend_label(&self) -> &'static str {
        if self.is_llamacpp() {
            "llamacpp"
        } else {
            "hash"
        }
    }

    fn is_llamacpp(&self) -> bool {
        matches!(
            self.config.backend.trim().to_ascii_lowercase().as_str(),
            "" | "auto" | "llamacpp" | "llama_cpp" | "llama.cpp" | "llama-cpp" | "gguf"
        )
    }

    fn embed_prepared_with_timeout(&self, prepared: &str, timeout: Duration) -> Result<Vec<f32>> {
        if prepared.trim().is_empty() {
            bail!("cannot embed empty text");
        }
        if self.is_llamacpp() {
            llamacpp_embed_with_timeout(&[prepared.to_string()], &self.config, timeout)?
                .pop()
                .ok_or_else(|| anyhow!("llama.cpp returned no embedding vector"))
        } else {
            Ok(hash_projection_embed(prepared, RETRIEVAL_EMBEDDING_DIM))
        }
    }

    fn embed_llamacpp_batched(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.len() <= self.config.batch_size {
            return llamacpp_embed_with_timeout(texts, &self.config, HTTP_TIMEOUT);
        }
        let batches = texts
            .chunks(self.config.batch_size)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        let mut output = Vec::with_capacity(texts.len());
        for (wave_index, wave) in batches.chunks(self.config.request_count).enumerate() {
            let mut wave_results = thread::scope(|scope| {
                let mut handles = Vec::with_capacity(wave.len());
                for (index, batch) in wave.iter().cloned().enumerate() {
                    let config = self.config.clone();
                    handles.push(scope.spawn(move || {
                        llamacpp_embed_with_timeout(&batch, &config, HTTP_TIMEOUT)
                            .map(|vectors| (index, vectors))
                    }));
                }
                let mut joined = Vec::with_capacity(handles.len());
                for handle in handles {
                    joined.push(
                        handle
                            .join()
                            .map_err(|_| anyhow!("llama.cpp embedding worker panicked"))??,
                    );
                }
                Ok::<_, anyhow::Error>(joined)
            })
            .with_context(|| format!("embed llama.cpp request wave {wave_index}"))?;
            wave_results.sort_by_key(|(index, _)| *index);
            for (_, vectors) in wave_results {
                output.extend(vectors);
            }
        }
        Ok(output)
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeProbe {
    pub reachable: bool,
    pub detail: String,
    pub elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingAcceleratorSmoke {
    pub elapsed_ms: u64,
    pub device: EmbeddingDeviceReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingDeviceReadiness {
    pub requested_policy: &'static str,
    pub observed_state: &'static str,
    pub observation_source: &'static str,
    pub detected_provider: Option<String>,
    pub detected_gpu: Option<String>,
    pub accelerator_requested: bool,
    pub accelerator_request_provider: Option<String>,
    pub accelerator_request_device: Option<String>,
    pub cpu_allowed: bool,
    pub full_retrieval_allowed: bool,
    pub degraded_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingAcceleratorRequest {
    pub provider: String,
    pub device: Option<String>,
    pub n_gpu_layers: String,
}

#[derive(Debug, Deserialize)]
struct NativeEmbeddingDeviceStateFile {
    embedding_launch: Option<NativeEmbeddingDeviceLaunch>,
}

#[derive(Debug, Deserialize)]
struct NativeEmbeddingDeviceLaunch {
    executable_path: Option<String>,
    requested_device: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ExactEmbeddingRuntimeState {
    namespace: String,
    embed_url: String,
    embedding_accelerator_request_provider: Option<String>,
    embedding_accelerator_request_device: Option<String>,
    embedding_launch: Option<crate::health::EmbeddingLaunchMetadata>,
    embedding_container_identity: Option<String>,
}

/// Stable id stored on retrieval manifest rows (backend + model family).
pub fn embedding_runtime_id() -> String {
    embedding_runtime_id_for_runtime(&crate::config::SidecarRuntimeConfig::local())
}

pub fn embedding_runtime_id_for_runtime(runtime: &crate::config::SidecarRuntimeConfig) -> String {
    if runtime
        .embedding
        .backend
        .trim()
        .eq_ignore_ascii_case("llamacpp")
    {
        PRODUCT_EMBEDDING_RUNTIME_ID.into()
    } else {
        "hash-projection:768".into()
    }
}

pub fn manifest_embedding_backend_is_product(backend: Option<&str>) -> bool {
    backend == Some(PRODUCT_EMBEDDING_RUNTIME_ID)
}

pub fn ensure_product_embedding_backend() -> Result<()> {
    ensure_product_embedding_backend_with_device(embedding_device_readiness())
}

pub fn ensure_product_embedding_backend_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Result<()> {
    if !runtime
        .embedding
        .backend
        .trim()
        .eq_ignore_ascii_case("llamacpp")
    {
        bail!("llama.cpp embedding sidecar is mandatory");
    }
    let deadline = Instant::now() + RUNTIME_EMBED_DEVICE_OBSERVATION_TIMEOUT;
    let mut device = embedding_device_readiness_for_runtime(runtime);
    loop {
        if device.full_retrieval_allowed {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            bail!("{}", device.degraded_reason.expect("device policy reason"));
        }
        thread::sleep((deadline - now).min(RUNTIME_EMBED_DEVICE_OBSERVATION_POLL));
        device = embedding_device_readiness_for_runtime(runtime);
    }
}

fn ensure_product_embedding_backend_with_device(device: EmbeddingDeviceReadiness) -> Result<()> {
    ensure_product_embedding_backend_static()?;
    if !device.full_retrieval_allowed {
        bail!("{}", device.degraded_reason.expect("device policy reason"));
    }
    Ok(())
}

fn ensure_product_embedding_backend_static() -> Result<()> {
    if !llamacpp_backend_selected() {
        bail!(
            "llama.cpp embedding sidecar is mandatory; set CODESTORY_EMBED_BACKEND=llamacpp and CODESTORY_EMBED_LLAMACPP_URL"
        );
    }
    Ok(())
}

pub fn embedding_device_readiness() -> EmbeddingDeviceReadiness {
    embedding_device_readiness_with_observed_state(None, None)
}

pub fn embedding_device_readiness_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> EmbeddingDeviceReadiness {
    embedding_device_readiness_with_observed_state(
        observe_sidecar_embedding_device_state(runtime),
        Some(runtime),
    )
}

fn embedding_device_readiness_with_observed_state(
    sidecar_observed_state: Option<EmbeddingDeviceObservation>,
    runtime: Option<&crate::config::SidecarRuntimeConfig>,
) -> EmbeddingDeviceReadiness {
    let cpu_allowed = runtime
        .map(|runtime| {
            runtime
                .embedding
                .device_policy
                .eq_ignore_ascii_case("allow_cpu")
        })
        .unwrap_or_else(explicit_cpu_allowed);
    let detection = host_embedding_device_detection();
    let accelerator_request = (!cpu_allowed).then(default_embedding_accelerator_request);
    let accelerator_requested = accelerator_request.is_some();
    let observation = sidecar_observed_state
        .unwrap_or_else(|| observed_embedding_device_state(cpu_allowed, accelerator_requested));
    let observed_state = observation.state;
    let accelerated = observed_state == "accelerated";
    let full_retrieval_allowed = accelerated || cpu_allowed;
    let requested_policy = if cpu_allowed {
        "cpu_allowed"
    } else {
        "accelerator_required"
    };
    let degraded_reason = (!full_retrieval_allowed).then(|| {
        let request_note = if accelerator_requested {
            " host accelerator was detected/requested, but the embedding sidecar did not prove accelerator execution;"
        } else {
            ""
        };
        format!("embedding_device_unverified: requested_policy={requested_policy} observed_device={observed_state} observation_source={};{request_note} set {ALLOW_CPU_ENV}=1 or {DEVICE_POLICY_ENV}=allow_cpu for intentional CPU-backed retrieval", observation.source)
    });

    EmbeddingDeviceReadiness {
        requested_policy,
        observed_state,
        observation_source: observation.source,
        detected_provider: detection.as_ref().map(|gpu| gpu.provider.clone()),
        detected_gpu: detection.map(|gpu| gpu.name),
        accelerator_requested,
        accelerator_request_provider: accelerator_request
            .as_ref()
            .map(|request| request.provider.clone()),
        accelerator_request_device: accelerator_request
            .as_ref()
            .and_then(|request| request.device.clone()),
        cpu_allowed,
        full_retrieval_allowed,
        degraded_reason,
    }
}

fn observe_sidecar_embedding_device_state(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<EmbeddingDeviceObservation> {
    let (text, source) = if crate::config::embedding_server_launch_mode_for_runtime(runtime)
        .ok()
        .is_some_and(|mode| mode == crate::config::EmbeddingServerLaunchMode::NativeSpawned)
    {
        return observe_native_embedding_device_state(runtime);
    } else {
        (read_container_embedding_log(runtime)?, "sidecar_log")
    };
    match observed_embedding_device_state_from_text(&text) {
        "unknown" => None,
        state => Some(EmbeddingDeviceObservation { state, source }),
    }
}

fn read_container_embedding_log(runtime: &crate::config::SidecarRuntimeConfig) -> Option<String> {
    let output = Command::new("docker")
        .args([
            "logs",
            "--tail",
            "200",
            &format!("{}-embed", runtime.compose_project),
        ])
        .output()
        .ok()?;
    output.status.success().then(|| {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn observe_native_embedding_device_state(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<EmbeddingDeviceObservation> {
    let text = read_native_embedding_log_tail(
        &native_embedding_log_path(runtime),
        NATIVE_LLAMA_LOG_READ_TAIL_BYTES,
    )
    .ok();
    if let Some(text) = text.as_deref() {
        match observed_embedding_device_state_from_text(native_embedding_log_current_launch(text)) {
            "unknown" => {}
            state => {
                return Some(EmbeddingDeviceObservation {
                    state,
                    source: "native_log",
                });
            }
        }
    }
    observe_native_embedding_device_state_from_device_list(runtime)
}

fn observe_native_embedding_device_state_from_device_list(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<EmbeddingDeviceObservation> {
    let request = embedding_accelerator_request()?;
    let state: NativeEmbeddingDeviceStateFile =
        serde_json::from_slice(&std::fs::read(&runtime.layout.state_file).ok()?).ok()?;
    let launch = state.embedding_launch?;
    let executable = launch.executable_path?;
    let output = Command::new(executable)
        .arg("--list-devices")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if native_device_list_proves_accelerator(&text, &request, launch.requested_device.as_deref()) {
        Some(EmbeddingDeviceObservation {
            state: "accelerated",
            source: "native_device_list",
        })
    } else {
        None
    }
}

fn native_device_list_proves_accelerator(
    text: &str,
    request: &EmbeddingAcceleratorRequest,
    requested_device: Option<&str>,
) -> bool {
    let normalized = text.to_ascii_lowercase();
    if let Some(device) = requested_device.or(request.device.as_deref()) {
        let device = device.to_ascii_lowercase();
        return text.lines().any(|line| {
            let line = line.trim_start().to_ascii_lowercase();
            line.starts_with(&format!("{device}:")) || line.contains(&device)
        });
    }
    if request.provider.is_empty() {
        return false;
    }
    normalized.contains(&request.provider.to_ascii_lowercase())
}

pub(crate) fn native_embedding_log_path(runtime: &crate::config::SidecarRuntimeConfig) -> PathBuf {
    runtime
        .layout
        .state_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("llama-server-native.log")
}

pub(crate) fn prepare_native_embedding_log_for_launch(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let previous = read_native_embedding_log_tail_bytes(path, NATIVE_LLAMA_PREVIOUS_LOG_TAIL_BYTES)
        .with_context(|| format!("read bounded native embedding log tail {}", path.display()))?;
    let previous_path = path.with_file_name("llama-server-native.previous.log");
    codestory_workspace::atomic_file::write_bytes_atomic(
        &previous_path,
        "native-embedding-log",
        &previous,
    )
    .with_context(|| {
        format!(
            "write bounded previous native embedding log {}",
            previous_path.display()
        )
    })?;
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("truncate native embedding log {}", path.display()))?;
    Ok(())
}

fn read_native_embedding_log_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    Ok(
        String::from_utf8_lossy(&read_native_embedding_log_tail_bytes(path, max_bytes)?)
            .into_owned(),
    )
}

fn read_native_embedding_log_tail_bytes(path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max_bytes)))?;
    let remaining = len.min(max_bytes);
    let mut bytes = Vec::with_capacity(remaining as usize);
    file.take(remaining).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn native_embedding_log_current_launch(text: &str) -> &str {
    text.rfind(NATIVE_LLAMA_LOG_START_MARKER)
        .map(|offset| &text[offset..])
        .unwrap_or(text)
}

pub fn ensure_embedding_accelerator_smoke_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Result<Option<EmbeddingAcceleratorSmoke>> {
    let before = embedding_device_readiness_for_runtime(runtime);
    if before.cpu_allowed {
        return Ok(None);
    }
    let launch_mode = crate::config::embedding_server_launch_mode_for_runtime(runtime)?;
    if launch_mode == crate::config::EmbeddingServerLaunchMode::ExternalEndpoint {
        bail!(
            "gpu_unverified: external embedding endpoints do not expose CodeStory-owned runtime log offload proof"
        );
    }
    let state = exact_embedding_runtime_state(runtime)?;
    let request = embedding_accelerator_request_from_runtime_state(&state)?;
    let native_launch_before = match launch_mode {
        crate::config::EmbeddingServerLaunchMode::NativeSpawned => {
            let launch = state.embedding_launch.clone().ok_or_else(|| {
                anyhow!("gpu_unverified: native embedding runtime state has no launch identity")
            })?;
            crate::sidecar::ensure_native_embedding_launch_identity(&launch)
                .context("gpu_unverified: validate native embedding launch before smoke")?;
            Some(launch)
        }
        crate::config::EmbeddingServerLaunchMode::DockerComposeEmbed => None,
        crate::config::EmbeddingServerLaunchMode::ExternalEndpoint => unreachable!(),
    };
    let container_identity_before =
        if launch_mode == crate::config::EmbeddingServerLaunchMode::DockerComposeEmbed {
            Some(ensure_persisted_running_embedding_container_identity_from_state(runtime, &state)?)
        } else {
            None
        };
    let probe = probe_product_embedding_runtime_with_timeout(runtime, ACCELERATOR_SMOKE_TIMEOUT);
    let text = if launch_mode == crate::config::EmbeddingServerLaunchMode::NativeSpawned {
        let after_state = exact_embedding_runtime_state(runtime)?;
        if after_state != state {
            bail!("gpu_unverified: persisted embedding runtime identity changed during smoke");
        }
        let after = after_state.embedding_launch.ok_or_else(|| {
            anyhow!("gpu_unverified: native embedding launch identity disappeared during smoke")
        })?;
        if native_launch_before.as_ref() != Some(&after) {
            bail!("gpu_unverified: native embedding launch identity changed during smoke");
        }
        crate::sidecar::ensure_native_embedding_launch_identity(&after)
            .context("gpu_unverified: validate native embedding launch after smoke")?;
        read_native_embedding_log_tail(
            &native_embedding_log_path(runtime),
            NATIVE_LLAMA_LOG_READ_TAIL_BYTES,
        )
        .ok()
        .map(|text| native_embedding_log_current_launch(&text).to_string())
    } else {
        let after_state = exact_embedding_runtime_state(runtime)?;
        if after_state != state {
            bail!("gpu_unverified: persisted embedding runtime identity changed during smoke");
        }
        let after = ensure_persisted_running_embedding_container_identity_from_state(
            runtime,
            &after_state,
        )?;
        if container_identity_before.as_deref() != Some(after.as_str()) {
            bail!("gpu_unverified: embedding container identity changed during accelerator smoke");
        }
        read_container_embedding_log(runtime)
    };
    let log_proven = text
        .as_deref()
        .is_some_and(|text| runtime_log_proves_requested_accelerator(text, &request));
    let mut device = embedding_device_readiness_for_runtime(runtime);
    if log_proven {
        device.observed_state = "accelerated";
        device.observation_source =
            if launch_mode == crate::config::EmbeddingServerLaunchMode::NativeSpawned {
                "native_log"
            } else {
                "sidecar_log"
            };
        device.full_retrieval_allowed = true;
        device.degraded_reason = None;
    }
    device.accelerator_requested = true;
    device.accelerator_request_provider = Some(request.provider.clone());
    device.accelerator_request_device = request.device.clone();
    evaluate_embedding_accelerator_smoke(probe, device, log_proven)
}

fn probe_product_embedding_runtime_with_timeout(
    runtime: &crate::config::SidecarRuntimeConfig,
    timeout: Duration,
) -> EmbeddingRuntimeProbe {
    let started = Instant::now();
    let result = LlamaCppEmbeddingClient::new(&runtime.embedding)
        .and_then(|client| client.embed_query_with_timeout("codestory accelerator smoke", timeout));
    let elapsed_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
    match result {
        Ok(vector) => EmbeddingRuntimeProbe {
            reachable: true,
            detail: format!("llamacpp embeddings reachable dim={}", vector.len()),
            elapsed_ms,
        },
        Err(error) => EmbeddingRuntimeProbe {
            reachable: false,
            detail: format!("llamacpp embeddings unavailable: {error}"),
            elapsed_ms,
        },
    }
}

pub(crate) fn running_embedding_container_identity(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Result<String> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.Id}}|{{.State.StartedAt}}|{{.State.Running}}",
            &format!("{}-embed", runtime.compose_project),
        ])
        .output()
        .context("gpu_unverified: inspect embedding container identity")?;
    if !output.status.success() {
        bail!("gpu_unverified: embedding container identity is unavailable");
    }
    validate_running_embedding_container_identity(&String::from_utf8_lossy(&output.stdout))
}

pub(crate) fn ensure_persisted_running_embedding_container_identity(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Result<String> {
    let state = exact_embedding_runtime_state(runtime)?;
    ensure_persisted_running_embedding_container_identity_from_state(runtime, &state)
}

fn ensure_persisted_running_embedding_container_identity_from_state(
    runtime: &crate::config::SidecarRuntimeConfig,
    state: &ExactEmbeddingRuntimeState,
) -> Result<String> {
    let identity = running_embedding_container_identity(runtime)?;
    validate_persisted_running_embedding_container_identity(
        state.embedding_container_identity.as_deref(),
        &identity,
    )?;
    Ok(identity)
}

fn validate_persisted_running_embedding_container_identity(
    persisted: Option<&str>,
    running: &str,
) -> Result<()> {
    if persisted != Some(running) {
        bail!(
            "gpu_unverified: running embedding container does not match persisted runtime identity"
        );
    }
    Ok(())
}

fn validate_running_embedding_container_identity(output: &str) -> Result<String> {
    let identity = output.trim().to_string();
    if identity.is_empty() || !identity.ends_with("|true") {
        bail!("gpu_unverified: embedding container is not running");
    }
    Ok(identity)
}

fn exact_embedding_runtime_state(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Result<ExactEmbeddingRuntimeState> {
    let state: ExactEmbeddingRuntimeState =
        serde_json::from_slice(&std::fs::read(&runtime.layout.state_file).with_context(|| {
            format!(
                "gpu_unverified: read exact embedding runtime state {}",
                runtime.layout.state_file.display()
            )
        })?)
        .context("gpu_unverified: parse exact embedding runtime state")?;
    if state.namespace != runtime.namespace || state.embed_url != runtime.embedding.endpoint {
        bail!(
            "gpu_unverified: embedding runtime state does not match selected namespace and endpoint"
        );
    }
    Ok(state)
}

fn embedding_accelerator_request_from_runtime_state(
    state: &ExactEmbeddingRuntimeState,
) -> Result<EmbeddingAcceleratorRequest> {
    let provider = state
        .embedding_accelerator_request_provider
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("gpu_unverified: embedding runtime state has no requested provider")
        })?;
    let device = state
        .embedding_accelerator_request_device
        .clone()
        .or_else(|| {
            state
                .embedding_launch
                .as_ref()
                .and_then(|launch| launch.requested_device.clone())
        });
    Ok(EmbeddingAcceleratorRequest {
        provider,
        device,
        n_gpu_layers: String::new(),
    })
}

fn evaluate_embedding_accelerator_smoke(
    probe: EmbeddingRuntimeProbe,
    device: EmbeddingDeviceReadiness,
    log_proven: bool,
) -> Result<Option<EmbeddingAcceleratorSmoke>> {
    if device.cpu_allowed {
        return Ok(None);
    }
    let elapsed_ms = probe.elapsed_ms.ok_or_else(|| {
        anyhow!("gpu_unverified: embedding smoke produced no bounded timing evidence")
    })?;
    if !probe.reachable || !log_proven || device.observed_state != "accelerated" {
        bail!(
            "gpu_unverified: embedding smoke failed before long semantic indexing: reachable={} elapsed_ms={} runtime_log_offload={} requested_provider={} requested_device={}",
            probe.reachable,
            elapsed_ms,
            log_proven,
            device
                .accelerator_request_provider
                .as_deref()
                .unwrap_or("none"),
            device
                .accelerator_request_device
                .as_deref()
                .unwrap_or("none")
        );
    }
    Ok(Some(EmbeddingAcceleratorSmoke { elapsed_ms, device }))
}

fn runtime_log_proves_requested_accelerator(
    text: &str,
    request: &EmbeddingAcceleratorRequest,
) -> bool {
    let text = text.to_ascii_lowercase();
    let positive_offload = text
        .lines()
        .any(|line| line_reports_gpu_offload(line) == Some(true));
    let provider = request.provider.to_ascii_lowercase();
    let provider_seen = text.contains(&provider)
        || crate::config::selected_llama_sidecar_backend(&request.provider).is_some_and(
            |backend| {
                backend
                    .log_markers
                    .iter()
                    .any(|marker| text.contains(&marker.to_ascii_lowercase()))
            },
        );
    let device_seen = request
        .device
        .as_deref()
        .is_none_or(|device| text.contains(&device.to_ascii_lowercase()));
    positive_offload && provider_seen && device_seen
}

pub fn embedding_accelerator_request() -> Option<EmbeddingAcceleratorRequest> {
    if explicit_cpu_allowed() {
        return None;
    }
    Some(default_embedding_accelerator_request())
}

fn default_embedding_accelerator_request() -> EmbeddingAcceleratorRequest {
    let host = crate::config::embedding_host_platform();
    let provider = default_embedding_accelerator_provider(&host);
    EmbeddingAcceleratorRequest {
        provider: provider.clone(),
        device: env_trimmed(LLAMACPP_DEVICE_ENV)
            .or_else(|| (provider == "vulkan").then(|| "Vulkan0".to_string())),
        n_gpu_layers: env_trimmed(LLAMACPP_N_GPU_LAYERS_ENV).unwrap_or_else(|| "99".to_string()),
    }
}

fn default_embedding_accelerator_provider(host: &crate::config::EmbeddingHostPlatform) -> String {
    if host.os == "macos" && host.arch == "aarch64" {
        return "metal".to_string();
    }
    if host.os == "linux"
        && let Some(provider) = env_trimmed(DEVICE_PROVIDER_ENV)
            .map(|value| normalize_accelerator_request_provider(&value))
            .filter(|provider| {
                ["cuda", "hip", "vulkan", "sycl", "openvino"].contains(&provider.as_str())
            })
    {
        return provider;
    }
    "vulkan".to_string()
}

fn explicit_cpu_allowed() -> bool {
    env_truthy(ALLOW_CPU_ENV)
        || std::env::var(DEVICE_POLICY_ENV)
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "allow_cpu" | "cpu_allowed" | "cpu"
                )
            })
            .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmbeddingDeviceObservation {
    state: &'static str,
    source: &'static str,
}

fn observed_embedding_device_state(
    cpu_allowed: bool,
    accelerator_requested: bool,
) -> EmbeddingDeviceObservation {
    if let Some(state) = std::env::var(DEVICE_STATE_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "accelerated" | "gpu" | "vulkan" | "cuda" | "hip" | "metal" | "sycl" | "openvino" => {
                Some("accelerated")
            }
            "cpu" => Some("cpu"),
            "unknown" => Some("unknown"),
            _ => None,
        })
    {
        return EmbeddingDeviceObservation {
            state,
            source: "manual_env",
        };
    }
    if cpu_allowed {
        return EmbeddingDeviceObservation {
            state: "cpu",
            source: "cpu_policy",
        };
    }
    if accelerator_requested {
        return EmbeddingDeviceObservation {
            state: "unknown",
            source: "accelerator_request_unobserved",
        };
    }
    EmbeddingDeviceObservation {
        state: "unknown",
        source: "sidecar_unobserved",
    }
}

fn observed_embedding_device_state_from_text(text: &str) -> &'static str {
    let mut saw_cpu = false;
    let mut saw_accelerated = false;
    let mut saw_metal = false;
    let text = text.to_ascii_lowercase();
    let log_markers = selected_backend_log_markers();
    let metal_requested = log_markers.iter().any(|marker| marker == "metal");
    for line in text.lines() {
        if line_reports_gpu_offload(line) == Some(true) {
            saw_accelerated = true;
        }
        if line.contains("using device")
            && ["vulkan", "cuda", "hip", "metal", "sycl", "openvino"]
                .iter()
                .any(|needle| line.contains(needle))
        {
            saw_accelerated = true;
        }
        if metal_requested && line_reports_metal_evidence(line) {
            saw_metal = true;
        }
        saw_cpu |= line_reports_gpu_offload(line) == Some(false)
            || line.contains("n_gpu_layers = 0")
            || line.contains("using cpu")
            || line.contains("no gpu")
            || line.contains("no vulkan device");
    }
    if saw_accelerated {
        "accelerated"
    } else if saw_cpu {
        "cpu"
    } else if saw_metal
        || (!log_markers.is_empty()
            && log_markers
                .iter()
                .all(|marker| text.contains(&marker.to_ascii_lowercase())))
    {
        "accelerated"
    } else {
        "unknown"
    }
}

fn line_reports_metal_evidence(line: &str) -> bool {
    line.contains("ggml_metal_init")
        || line.contains("metal backend")
        || line.trim_start().starts_with("mtl0")
        || line.trim_start().starts_with("mtl :")
}

fn selected_backend_log_markers() -> Vec<String> {
    embedding_accelerator_request()
        .and_then(|request| crate::config::selected_llama_sidecar_backend(&request.provider))
        .map(|backend| backend.log_markers)
        .unwrap_or_default()
}

fn line_reports_gpu_offload(line: &str) -> Option<bool> {
    let keyword = if let Some(offset) = line.find("offloaded ") {
        offset + "offloaded ".len()
    } else {
        let offset = line.find("offloading ")?;
        offset + "offloading ".len()
    };
    let digits = line[keyword..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse::<u32>().ok().map(|count| count > 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostGpuDetection {
    provider: String,
    name: String,
}

fn host_embedding_device_detection() -> Option<HostGpuDetection> {
    if env_truthy(DISABLE_HOST_GPU_DETECT_ENV) {
        return None;
    }
    let env_provider = env_trimmed(DEVICE_PROVIDER_ENV).map(|value| normalize_gpu_provider(&value));
    let env_name = env_trimmed(DEVICE_NAME_ENV);
    if let Some(provider) = env_provider.filter(|provider| provider == "amd") {
        return Some(HostGpuDetection {
            provider,
            name: env_name.unwrap_or_else(|| "AMD GPU".to_string()),
        });
    }
    detect_windows_amd_gpu()
}

fn detect_windows_amd_gpu() -> Option<HostGpuDetection> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            windows_video_controller_probe_script(),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    detect_amd_gpu_from_windows_video_controller(&String::from_utf8_lossy(&output.stdout))
}

fn windows_video_controller_probe_script() -> &'static str {
    r#"$job = Start-Job -ScriptBlock { Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) $($_.AdapterCompatibility)" } }; if (Wait-Job $job -Timeout 10) { Receive-Job $job; Remove-Job $job -Force } else { Stop-Job $job; Remove-Job $job -Force; exit 124 }"#
}

fn detect_amd_gpu_from_windows_video_controller(output: &str) -> Option<HostGpuDetection> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| {
            let normalized = line.to_ascii_lowercase();
            normalized.contains("amd")
                || normalized.contains("radeon")
                || normalized.contains("advanced micro devices")
        })
        .map(|name| HostGpuDetection {
            provider: "amd".to_string(),
            name: name.to_string(),
        })
}

fn normalize_gpu_provider(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.contains("amd")
        || normalized.contains("radeon")
        || normalized.contains("advanced micro devices")
    {
        "amd".to_string()
    } else {
        normalized
    }
}

fn normalize_accelerator_request_provider(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.contains("nvidia") || normalized.contains("cuda") {
        "cuda".to_string()
    } else if normalized.contains("hip") || normalized.contains("rocm") {
        "hip".to_string()
    } else if normalized.contains("sycl") {
        "sycl".to_string()
    } else if normalized.contains("openvino") {
        "openvino".to_string()
    } else if normalized.contains("vulkan") {
        "vulkan".to_string()
    } else {
        normalized
    }
}

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub fn embed_query(text: &str) -> Result<Vec<f32>> {
    LlamaCppEmbeddingClient::new(&crate::config::SidecarRuntimeConfig::local().embedding)?
        .embed_query(text)
}

pub fn embed_query_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
    text: &str,
) -> Result<Vec<f32>> {
    LlamaCppEmbeddingClient::new(&runtime.embedding)?.embed_query(text)
}

/// Active embedding backend label for ops/status (`hash`, `llamacpp`).
pub fn embedding_backend_label() -> &'static str {
    if llamacpp_backend_selected() {
        "llamacpp"
    } else {
        "hash"
    }
}

pub fn embedding_backend_label_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> &'static str {
    if runtime
        .embedding
        .backend
        .trim()
        .eq_ignore_ascii_case("llamacpp")
    {
        "llamacpp"
    } else {
        "hash"
    }
}

#[cfg(test)]
pub fn embed_documents(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    LlamaCppEmbeddingClient::new(&crate::config::SidecarRuntimeConfig::local().embedding)?
        .embed_documents(texts)
}

pub fn embed_documents_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    LlamaCppEmbeddingClient::new(&runtime.embedding)?.embed_documents(texts)
}

fn llamacpp_backend_selected() -> bool {
    match std::env::var(EMBEDDING_BACKEND_ENV) {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "llamacpp" || normalized == "llama_cpp"
        }
        Err(_) => true,
    }
}

pub fn probe_product_embedding_runtime() -> EmbeddingRuntimeProbe {
    probe_product_embedding_runtime_for_runtime(&crate::config::SidecarRuntimeConfig::local())
}

pub fn probe_product_embedding_runtime_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> EmbeddingRuntimeProbe {
    LlamaCppEmbeddingClient::new(&runtime.embedding)
        .map(|client| client.probe())
        .unwrap_or_else(|error| EmbeddingRuntimeProbe {
            reachable: false,
            detail: format!("llama.cpp embeddings unavailable: {error}"),
            elapsed_ms: None,
        })
}

#[cfg(test)]
fn ensure_llamacpp_url_allowed(url: &str) -> Result<()> {
    ensure_llamacpp_url_allowed_with_policy(url, allow_remote_embeddings())
}

fn ensure_llamacpp_url_allowed_with_policy(url: &str, allow_remote: bool) -> Result<()> {
    if !allow_remote && !is_loopback_embedding_url(url) {
        bail!(
            "remote embedding URL is disabled; use a loopback URL or set {ALLOW_REMOTE_EMBEDDINGS_ENV}=1"
        );
    }
    Ok(())
}

#[cfg(test)]
fn allow_remote_embeddings() -> bool {
    std::env::var(ALLOW_REMOTE_EMBEDDINGS_ENV)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

fn is_loopback_embedding_url(url: &str) -> bool {
    let Some(host) = http_url_host(url) else {
        return false;
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "[::1]"
    )
}

fn http_url_host(url: &str) -> Option<&str> {
    let rest = url.trim().strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        return Some(&host_port[..=end]);
    }
    host_port.split(':').next().filter(|host| !host.is_empty())
}

fn llamacpp_embed_with_timeout(
    texts: &[String],
    config: &crate::config::EmbeddingRuntimeConfig,
    timeout: Duration,
) -> Result<Vec<Vec<f32>>> {
    let model = config
        .model_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.profile);
    let body = serde_json::json!({
        "input": texts,
        "model": model,
    });
    let payload = serde_json::to_string(&body).context("serialize embeddings request")?;
    let display_endpoint = crate::config::redacted_embedding_endpoint(&config.endpoint);
    let agent = ureq::builder().redirects(0).build();
    let response = read_bytes(
        agent
            .post(&config.endpoint)
            .timeout(timeout)
            .set("Content-Type", "application/json")
            .send_string(&payload),
    )
    .map_err(|error| {
        anyhow!(
            "llama.cpp embeddings request to {display_endpoint} failed: {}",
            redact_embedding_error(&config.endpoint, &error.to_string())
        )
    })?;
    let status = response.status;
    if !(200..300).contains(&status) {
        bail!("llama.cpp embeddings endpoint {display_endpoint} returned HTTP {status}");
    }
    let response_body = std::str::from_utf8(&response.body)
        .with_context(|| format!("decode UTF-8 JSON from llama.cpp endpoint {display_endpoint}"))?;
    parse_openai_embeddings(response_body, texts.len(), expected_embedding_dim(config))
        .with_context(|| format!("parse response from llama.cpp endpoint {display_endpoint}"))
}

#[derive(Deserialize)]
struct OpenAiEmbeddingsResponse {
    data: Vec<OpenAiEmbeddingRow>,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingRow {
    index: Option<usize>,
    embedding: Vec<f32>,
}

fn parse_openai_embeddings(
    body: &str,
    expected_count: usize,
    expected_dim: Option<usize>,
) -> Result<Vec<Vec<f32>>> {
    let parsed: OpenAiEmbeddingsResponse =
        serde_json::from_str(body).context("parse llama.cpp embeddings json")?;
    if parsed.data.len() != expected_count {
        bail!(
            "llama.cpp embeddings response returned {} vectors for {expected_count} inputs",
            parsed.data.len()
        );
    }
    let mut indexed = Vec::with_capacity(parsed.data.len());
    let mut seen = std::collections::BTreeSet::new();
    for row in parsed.data {
        let index = row
            .index
            .ok_or_else(|| anyhow!("llama.cpp embeddings response row missing `index`"))?;
        if index >= expected_count {
            bail!("llama.cpp embeddings response index {index} is outside 0..{expected_count}");
        }
        if !seen.insert(index) {
            bail!("llama.cpp embeddings response duplicated index {index}");
        }
        if expected_dim.is_some_and(|expected| row.embedding.len() != expected) {
            let expected_dim = expected_dim.expect("checked above");
            bail!(
                "llama.cpp embedding dim {} != expected {}; check the configured model profile and GGUF",
                row.embedding.len(),
                expected_dim
            );
        }
        indexed.push((index, row.embedding));
    }
    indexed.sort_by_key(|(index, _)| *index);
    Ok(indexed.into_iter().map(|(_, vector)| vector).collect())
}

fn expected_embedding_dim(config: &crate::config::EmbeddingRuntimeConfig) -> Option<usize> {
    config
        .expected_dim
        .or(config.truncate_dim)
        .or_else(|| named_embedding_profile_dim(&config.profile))
}

fn named_embedding_profile_dim(profile: &str) -> Option<usize> {
    match profile.trim().to_ascii_lowercase().as_str() {
        "minilm" | "minilm-l6-v2" | "all-minilm-l6-v2" | "bge-small" | "bge-small-en-v1.5" => {
            Some(384)
        }
        "bge-base"
        | "bge-base-en-v1.5"
        | "baai/bge-base-en-v1.5"
        | "embeddinggemma"
        | "embeddinggemma-300m"
        | "gemma"
        | "gemma-embedding-300m"
        | "google/embeddinggemma-300m"
        | "nomic"
        | "nomic-v1.5"
        | "nomic-embed-text-v1.5"
        | "nomic-v2"
        | "nomic-embed-text-v2"
        | "nomic-embed-text-v2-moe" => Some(768),
        "qwen" | "qwen3" | "qwen3-embedding-0.6b" | "qwen/qwen3-embedding-0.6b" => Some(1024),
        _ => None,
    }
}

fn redact_embedding_error(endpoint: &str, error: &str) -> String {
    let display = crate::config::redacted_embedding_endpoint(endpoint);
    let mut redacted = error.replace(endpoint, &display);
    let rest = endpoint
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(endpoint);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if let Some((userinfo, host)) = authority.rsplit_once('@') {
        redacted = redacted
            .replace(authority, host)
            .replace(userinfo, "[redacted]");
        for secret in userinfo.split(':') {
            if !secret.is_empty() {
                redacted = redacted.replace(secret, "[redacted]");
            }
        }
    }
    if let Some(query) = endpoint.split_once('?').map(|(_, value)| value) {
        let query = query.split('#').next().unwrap_or_default();
        if !query.is_empty() {
            redacted = redacted.replace(query, "[redacted]");
        }
        for value in query
            .split('&')
            .filter_map(|pair| pair.split_once('=').map(|(_, value)| value))
            .filter(|value| !value.is_empty())
        {
            redacted = redacted.replace(value, "[redacted]");
        }
    }
    if let Some(fragment) = endpoint.split_once('#').map(|(_, value)| value)
        && !fragment.is_empty()
    {
        redacted = redacted.replace(fragment, "[redacted]");
    }
    redacted
}

/// Same algorithm as `codestory_runtime::search::engine::embed_text_with_hash_projection`.
pub fn hash_projection_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dim];
    for token in text.split_whitespace() {
        let norm = token.trim().to_ascii_lowercase();
        if norm.is_empty() {
            continue;
        }
        let mut hasher = DefaultHasher::new();
        norm.hash(&mut hasher);
        let hash = hasher.finish();
        let index = (hash as usize) % dim;
        let sign = if ((hash >> 8) & 1) == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
        let index2 = ((hash >> 17) as usize) % dim;
        vector[index2] += 0.5 * sign;
    }
    l2_normalize(&mut vector);
    vector
}

fn l2_normalize(vector: &mut [f32]) {
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if norm <= f64::EPSILON {
        return;
    }
    let scale = (1.0 / norm) as f32;
    for value in vector.iter_mut() {
        *value *= scale;
    }
}

/// Stable 8-d hash vector used only for diagnostic downgraded vectors.
pub fn label_to_vector(label: &str) -> Vec<f32> {
    let digest = Sha256::digest(label.as_bytes());
    (0..8).map(|index| digest[index] as f32 / 255.0).collect()
}

pub fn qdrant_vector_dim() -> usize {
    RETRIEVAL_EMBEDDING_DIM
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SidecarLayout, SidecarProfile, SidecarRuntimeConfig};
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::net::TcpListener;

    fn llama_client_config(endpoint: String) -> crate::config::EmbeddingRuntimeConfig {
        crate::config::EmbeddingRuntimeConfig {
            configuration_error: None,
            backend: "llamacpp".into(),
            endpoint,
            endpoint_origin: crate::config::EmbeddingEndpointOrigin::TrustedProjectConfig,
            profile: "custom".into(),
            model_id: Some("retained-model-id".into()),
            pooling: None,
            query_prefix: None,
            document_prefix: None,
            layer_norm: None,
            truncate_dim: None,
            expected_dim: Some(3),
            batch_size: 128,
            request_count: 1,
            allow_remote: false,
            device_policy: "allow_cpu".into(),
            server_launch: Some("external_endpoint".into()),
        }
    }

    fn one_shot_embedding_server(
        status: &str,
        headers: &str,
        body: String,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind embedding server");
        let url = format!(
            "http://{}/v1/embeddings",
            listener.local_addr().expect("embedding server address")
        );
        let status = status.to_string();
        let headers = headers.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept embedding request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).expect("read embedding request");
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
                let header = String::from_utf8_lossy(&request[..header_end]);
                let content_length = header
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
            let response = format!(
                "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write embedding response");
            String::from_utf8(request).expect("embedding request utf8")
        });
        (url, handle)
    }

    #[test]
    fn openai_embedding_rows_are_sorted_by_index() {
        let body = serde_json::json!({"data": [
            {"index": 1, "embedding": [0.0, 1.0, 0.0]},
            {"index": 0, "embedding": [1.0, 0.0, 0.0]}
        ]})
        .to_string();

        let vectors = parse_openai_embeddings(&body, 2, Some(3)).expect("parse rows");

        assert_eq!(vectors[0], vec![1.0, 0.0, 0.0]);
        assert_eq!(vectors[1], vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn openai_embedding_rows_reject_invalid_index_sets_and_counts() {
        for (body, expected_count, expected) in [
            (
                serde_json::json!({"data": [
                    {"index": 0, "embedding": [1.0]},
                    {"index": 0, "embedding": [2.0]}
                ]})
                .to_string(),
                2,
                "duplicated index 0",
            ),
            (
                serde_json::json!({"data": [{"embedding": [1.0]}]}).to_string(),
                1,
                "missing `index`",
            ),
            (
                serde_json::json!({"data": [{"index": 2, "embedding": [1.0]}]}).to_string(),
                1,
                "outside 0..1",
            ),
            (
                serde_json::json!({"data": [{"index": 0, "embedding": [1.0]}]}).to_string(),
                2,
                "1 vectors for 2 inputs",
            ),
            (
                serde_json::json!({"data": [
                    {"index": 0, "embedding": [1.0]},
                    {"index": 1, "embedding": [2.0]}
                ]})
                .to_string(),
                1,
                "2 vectors for 1 inputs",
            ),
        ] {
            let error = parse_openai_embeddings(&body, expected_count, None)
                .expect_err("invalid response must fail");
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn named_embedding_profiles_supply_dimensions_with_explicit_precedence() {
        let mut config = llama_client_config("http://127.0.0.1:8080/v1/embeddings".into());
        config.expected_dim = None;
        config.profile = "qwen3-embedding-0.6b".into();
        assert_eq!(expected_embedding_dim(&config), Some(1024));
        config.truncate_dim = Some(256);
        assert_eq!(expected_embedding_dim(&config), Some(256));
        config.expected_dim = Some(128);
        assert_eq!(expected_embedding_dim(&config), Some(128));
    }

    #[test]
    fn production_llamacpp_client_uses_retained_model_and_sorted_rows() -> Result<()> {
        let response = serde_json::json!({"data": [
            {"index": 1, "embedding": [0.0, 1.0, 0.0]},
            {"index": 0, "embedding": [1.0, 0.0, 0.0]}
        ]})
        .to_string();
        let (url, request) = one_shot_embedding_server("200 OK", "", response);
        let client = LlamaCppEmbeddingClient::new(&llama_client_config(url))?;

        let vectors = client.embed_prepared_texts(&["alpha".into(), "beta".into()])?;
        let request = request.join().expect("embedding server thread");

        assert_eq!(vectors[0], vec![1.0, 0.0, 0.0]);
        assert_eq!(vectors[1], vec![0.0, 1.0, 0.0]);
        assert!(request.contains("retained-model-id"), "{request}");
        Ok(())
    }

    #[test]
    fn query_embedding_honors_request_deadline_timeout() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!(
            "http://{}/v1/embeddings",
            listener.local_addr().expect("embedding server address")
        );
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept embedding request");
            thread::sleep(Duration::from_millis(200));
            drop(stream);
        });
        let client = LlamaCppEmbeddingClient::new(&llama_client_config(url))?;
        let started = Instant::now();

        client
            .embed_query_with_timeout("deadline", Duration::from_millis(20))
            .expect_err("request deadline should stop a stalled embedding response");

        assert!(started.elapsed() < Duration::from_millis(150));
        server.join().expect("embedding server thread");
        Ok(())
    }

    #[test]
    fn production_llamacpp_client_rejects_redirects_and_redacts_endpoint_secrets() -> Result<()> {
        let (url, request) = one_shot_embedding_server(
            "302 Found",
            "Location: http://127.0.0.1:9/redirected\r\n",
            "redirect".into(),
        );
        let secret_url = format!(
            "{}?token=query-secret#fragment-secret",
            url.replacen("http://", "http://username-secret:password-secret@", 1)
        );
        let error = LlamaCppEmbeddingClient::new(&llama_client_config(secret_url))?
            .embed_prepared_texts(&["alpha".into()])
            .expect_err("redirect must not be followed");
        request.join().expect("embedding server thread");

        let rendered = format!("{error:#}");
        assert!(rendered.contains("302"), "{rendered}");
        for secret in [
            "username-secret",
            "password-secret",
            "query-secret",
            "fragment-secret",
        ] {
            assert!(!rendered.contains(secret), "{rendered}");
        }
        Ok(())
    }

    #[test]
    fn production_llamacpp_transport_errors_redact_endpoint_secrets() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        drop(listener);
        let endpoint = format!(
            "http://username-secret:password-secret@{address}/v1/embeddings?token=query-secret#fragment-secret"
        );
        let error = LlamaCppEmbeddingClient::new(&llama_client_config(endpoint))?
            .embed_prepared_texts(&["alpha".into()])
            .expect_err("closed endpoint must fail");

        let rendered = format!("{error:#}");
        for secret in [
            "username-secret",
            "password-secret",
            "query-secret",
            "fragment-secret",
        ] {
            assert!(!rendered.contains(secret), "{rendered}");
        }
        Ok(())
    }

    #[test]
    fn production_llamacpp_invalid_json_errors_redact_endpoint_secrets() -> Result<()> {
        let (url, request) = one_shot_embedding_server("200 OK", "", "invalid".into());
        let endpoint = format!(
            "{}?token=query-secret#fragment-secret",
            url.replacen("http://", "http://username-secret:password-secret@", 1)
        );
        let error = LlamaCppEmbeddingClient::new(&llama_client_config(endpoint))?
            .embed_prepared_texts(&["alpha".into()])
            .expect_err("invalid JSON must fail");
        request.join().expect("embedding server thread");

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("parse llama.cpp embeddings json"),
            "{rendered}"
        );
        for secret in [
            "username-secret",
            "password-secret",
            "query-secret",
            "fragment-secret",
        ] {
            assert!(!rendered.contains(secret), "{rendered}");
        }
        Ok(())
    }

    #[test]
    fn production_llamacpp_client_accepts_json_over_ten_mib() -> Result<()> {
        let response = serde_json::json!({
            "padding": "x".repeat(10 * 1024 * 1024 + 1),
            "data": [{"index": 0, "embedding": [1.0, 0.0, 0.0]}]
        })
        .to_string();
        let (url, request) = one_shot_embedding_server("200 OK", "", response);

        let vectors = LlamaCppEmbeddingClient::new(&llama_client_config(url))?
            .embed_prepared_texts(&["alpha".into()])?;
        request.join().expect("embedding server thread");

        assert_eq!(vectors, vec![vec![1.0, 0.0, 0.0]]);
        Ok(())
    }

    #[test]
    fn hash_projection_dim_matches_retrieval_embedding_dim() {
        let vector = hash_projection_embed("extension_service handler", RETRIEVAL_EMBEDDING_DIM);
        assert_eq!(vector.len(), RETRIEVAL_EMBEDDING_DIM);
        let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01 || vector.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn embed_documents_preserves_count_for_hash_projection() {
        let _lock = crate::test_support::env_lock();
        let _guard = EnvGuard::set(EMBEDDING_BACKEND_ENV, "hash");
        let docs = vec!["alpha".to_string(), "beta".to_string()];

        let vectors = embed_documents(&docs).expect("embed docs");

        assert_eq!(vectors.len(), docs.len());
        assert!(
            vectors
                .iter()
                .all(|vector| vector.len() == RETRIEVAL_EMBEDDING_DIM)
        );
    }

    #[test]
    fn label_to_vector_smoke_dim_is_eight() {
        assert_eq!(label_to_vector("handler").len(), 8);
    }

    #[test]
    fn default_qdrant_semantic_vectors_are_768() {
        let _lock = crate::test_support::env_lock();
        let _guard = EnvGuard::remove("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS");
        let _guard2 = EnvGuard::remove(EMBEDDING_BACKEND_ENV);
        assert_eq!(embedding_runtime_id(), PRODUCT_EMBEDDING_RUNTIME_ID);
        assert_eq!(qdrant_vector_dim(), RETRIEVAL_EMBEDDING_DIM);
    }

    #[test]
    fn embedding_runtime_id_llamacpp_when_backend_set() {
        let _lock = crate::test_support::env_lock();
        let _guard = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _guard2 = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        assert_eq!(embedding_runtime_id(), "llamacpp:bge-base-en-v1.5");
    }

    #[test]
    fn explicit_onnx_backend_is_not_product_sidecar_runtime() {
        let _lock = crate::test_support::env_lock();
        let _guard = EnvGuard::set(EMBEDDING_BACKEND_ENV, "onnx");
        let _guard2 = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");

        assert_eq!(embedding_runtime_id(), "hash-projection:768");
        assert!(!manifest_embedding_backend_is_product(Some(
            embedding_runtime_id().as_str()
        )));
        let error = ensure_product_embedding_backend()
            .expect_err("explicit ONNX should not satisfy product sidecar indexing");
        assert!(
            error
                .to_string()
                .contains("llama.cpp embedding sidecar is mandatory"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn default_embedding_device_policy_blocks_unknown_device() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "windows/x86_64");
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let readiness = embedding_device_readiness();

        assert_eq!(readiness.requested_policy, "accelerator_required");
        assert_eq!(readiness.observed_state, "unknown");
        assert_eq!(
            readiness.observation_source,
            "accelerator_request_unobserved"
        );
        assert_eq!(readiness.detected_provider, None);
        assert_eq!(readiness.detected_gpu, None);
        assert!(readiness.accelerator_requested);
        assert_eq!(
            readiness.accelerator_request_provider.as_deref(),
            Some("vulkan")
        );
        assert_eq!(
            readiness.accelerator_request_device.as_deref(),
            Some("Vulkan0")
        );
        assert!(!readiness.full_retrieval_allowed);
        assert!(
            readiness
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("embedding_device_unverified")
        );
    }

    #[test]
    fn explicit_cpu_opt_in_allows_cpu_backed_retrieval() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::set(ALLOW_CPU_ENV, "1");
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::set(DEVICE_STATE_ENV, "cpu");
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let readiness = embedding_device_readiness();

        assert_eq!(readiness.requested_policy, "cpu_allowed");
        assert_eq!(readiness.observed_state, "cpu");
        assert_eq!(readiness.observation_source, "manual_env");
        assert!(readiness.cpu_allowed);
        assert!(!readiness.accelerator_requested);
        assert!(embedding_accelerator_request().is_none());
        assert!(readiness.full_retrieval_allowed);
        assert!(readiness.degraded_reason.is_none());
    }

    #[test]
    fn windows_video_controller_parser_detects_amd_gpu() {
        let detection = detect_amd_gpu_from_windows_video_controller(
            "Intel(R) UHD Graphics\r\nAMD Radeon RX 7800 XT\r\n",
        )
        .expect("amd gpu");

        assert_eq!(detection.provider, "amd");
        assert_eq!(detection.name, "AMD Radeon RX 7800 XT");
    }

    #[test]
    fn windows_video_controller_parser_skips_virtual_monitor_before_amd() {
        let detection = detect_amd_gpu_from_windows_video_controller(
            "Meta Virtual Monitor Meta Platforms\r\nAMD Radeon RX 7900 XT Advanced Micro Devices, Inc.\r\n",
        )
        .expect("amd gpu");

        assert_eq!(detection.provider, "amd");
        assert!(detection.name.contains("AMD Radeon RX 7900 XT"));
    }

    #[test]
    fn windows_video_controller_probe_script_has_timeout() {
        let script = windows_video_controller_probe_script();

        assert!(script.contains("Wait-Job $job -Timeout 10"));
        assert!(script.contains("AdapterCompatibility"));
        assert!(script.contains("Stop-Job $job"));
        assert!(script.contains("exit 124"));
    }

    #[test]
    fn default_policy_requests_vulkan_without_observing_acceleration() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "windows/x86_64");
        let _provider = EnvGuard::remove(DEVICE_PROVIDER_ENV);
        let _name = EnvGuard::remove(DEVICE_NAME_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");
        let _llama_device = EnvGuard::remove(LLAMACPP_DEVICE_ENV);
        let _ngl = EnvGuard::remove(LLAMACPP_N_GPU_LAYERS_ENV);

        let readiness = embedding_device_readiness();
        let request = embedding_accelerator_request().expect("accelerator request");

        assert_eq!(readiness.requested_policy, "accelerator_required");
        assert_eq!(readiness.observed_state, "unknown");
        assert_eq!(
            readiness.observation_source,
            "accelerator_request_unobserved"
        );
        assert_eq!(readiness.detected_provider, None);
        assert_eq!(readiness.detected_gpu, None);
        assert!(readiness.accelerator_requested);
        assert_eq!(
            readiness.accelerator_request_provider.as_deref(),
            Some("vulkan")
        );
        assert_eq!(
            readiness.accelerator_request_device.as_deref(),
            Some("Vulkan0")
        );
        assert!(!readiness.cpu_allowed);
        assert!(!readiness.full_retrieval_allowed);
        assert!(
            readiness
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar did not prove accelerator execution")
        );
        assert_eq!(request.provider, "vulkan");
        assert_eq!(request.device.as_deref(), Some("Vulkan0"));

        assert_eq!(request.n_gpu_layers, "99");
    }

    #[test]
    fn simulated_macos_arm64_request_reports_metal_without_vulkan_device() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _llama_device = EnvGuard::remove(LLAMACPP_DEVICE_ENV);
        let _ngl = EnvGuard::remove(LLAMACPP_N_GPU_LAYERS_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let readiness = embedding_device_readiness();
        let request = embedding_accelerator_request().expect("accelerator request");

        assert_eq!(request.provider, "metal");
        assert_eq!(request.device, None);
        assert_eq!(request.n_gpu_layers, "99");
        assert_eq!(
            readiness.accelerator_request_provider.as_deref(),
            Some("metal")
        );
        assert_eq!(readiness.accelerator_request_device, None);
    }

    #[test]
    fn selected_backend_log_markers_count_as_accelerator_evidence() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _llama_device = EnvGuard::remove(LLAMACPP_DEVICE_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");

        assert_eq!(
            observed_embedding_device_state_from_text(
                "ggml_metal_init: metal backend ready\nload_tensors: offloaded layers\n"
            ),
            "accelerated"
        );
        assert_eq!(
            observed_embedding_device_state_from_text("ggml_metal_init: metal backend ready\n"),
            "accelerated"
        );
        assert_eq!(
            observed_embedding_device_state_from_text(
                "MTL0 : Apple M4 Pro\nMTL : EMBED_LIBRARY = 1\n"
            ),
            "accelerated"
        );
        assert_eq!(
            observed_embedding_device_state_from_text(
                "ggml_metal_init: metal backend ready\nload_tensors: offloaded 0/33 layers\n"
            ),
            "cpu"
        );
    }

    #[test]
    fn linux_provider_override_reports_selected_manifest_provider() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _llama_device = EnvGuard::remove(LLAMACPP_DEVICE_ENV);
        let _ngl = EnvGuard::remove(LLAMACPP_N_GPU_LAYERS_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "linux/x86_64");
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "NVIDIA Corporation");
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let readiness = embedding_device_readiness();
        let request = embedding_accelerator_request().expect("accelerator request");

        assert_eq!(request.provider, "cuda");
        assert_eq!(request.device, None);
        assert_eq!(
            readiness.accelerator_request_provider.as_deref(),
            Some("cuda")
        );
        assert_eq!(readiness.accelerator_request_device, None);
    }

    #[test]
    fn linux_default_request_uses_vulkan_cell_and_device() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _llama_device = EnvGuard::remove(LLAMACPP_DEVICE_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "linux/aarch64");
        let _provider = EnvGuard::remove(DEVICE_PROVIDER_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let request = embedding_accelerator_request().expect("accelerator request");

        assert_eq!(request.provider, "vulkan");
        assert_eq!(request.device.as_deref(), Some("Vulkan0"));
        assert_eq!(
            crate::config::selected_llama_sidecar_backend(&request.provider)
                .expect("linux arm64 vulkan cell")
                .id,
            "linux-aarch64-vulkan"
        );
    }

    #[test]
    fn linux_backend_log_markers_count_as_accelerator_evidence() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _llama_device = EnvGuard::remove(LLAMACPP_DEVICE_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "linux/x86_64");

        for (provider, marker) in [
            ("cuda", "cuda backend ready"),
            ("hip", "hip backend ready"),
            ("vulkan", "vulkan backend ready"),
            ("sycl", "sycl backend ready"),
            ("openvino", "openvino backend ready"),
        ] {
            let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, provider);
            assert_eq!(
                observed_embedding_device_state_from_text(&format!(
                    "{marker}\nload_tensors: offloaded layers\n"
                )),
                "accelerated",
                "{provider} markers should prove acceleration"
            );
            assert_eq!(
                observed_embedding_device_state_from_text(&format!("{marker}\n")),
                "unknown",
                "{provider} without offload marker is inconclusive"
            );
        }
    }

    #[test]
    fn accelerator_smoke_requires_requested_device_offload_and_timing() {
        let request = EmbeddingAcceleratorRequest {
            provider: "vulkan".into(),
            device: Some("Vulkan0".into()),
            n_gpu_layers: "99".into(),
        };
        assert!(runtime_log_proves_requested_accelerator(
            "vulkan backend ready\nusing device Vulkan0\noffloaded 13/13 layers to GPU\n",
            &request,
        ));
        assert!(!runtime_log_proves_requested_accelerator(
            "vulkan backend ready\nusing device Vulkan1\noffloaded 13/13 layers to GPU\n",
            &request,
        ));
        assert!(!runtime_log_proves_requested_accelerator(
            "vulkan backend ready\nusing device Vulkan0\noffloaded 0/13 layers to GPU\n",
            &request,
        ));

        let device = EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "accelerated",
            observation_source: "native_log",
            detected_provider: Some("amd".into()),
            detected_gpu: Some("AMD GPU".into()),
            accelerator_requested: true,
            accelerator_request_provider: Some("vulkan".into()),
            accelerator_request_device: Some("Vulkan0".into()),
            cpu_allowed: false,
            full_retrieval_allowed: true,
            degraded_reason: None,
        };
        let error = evaluate_embedding_accelerator_smoke(
            EmbeddingRuntimeProbe {
                reachable: true,
                detail: "reachable".into(),
                elapsed_ms: None,
            },
            device,
            true,
        )
        .expect_err("missing timing must not verify accelerator work");
        assert!(error.to_string().contains("gpu_unverified"));
    }

    #[test]
    fn container_identity_requires_running_state() {
        assert_eq!(
            validate_running_embedding_container_identity("abc|2026-07-12T00:00:00Z|true\n")
                .expect("running identity"),
            "abc|2026-07-12T00:00:00Z|true"
        );
        assert!(
            validate_running_embedding_container_identity("abc|2026-07-12T00:00:00Z|false")
                .expect_err("stopped container must fail")
                .to_string()
                .contains("not running")
        );
    }

    #[test]
    fn running_container_identity_must_match_persisted_runtime_identity() {
        let persisted = "container-a|2026-07-12T00:00:00Z|true";
        validate_persisted_running_embedding_container_identity(Some(persisted), persisted)
            .expect("exact persisted running identity");
        assert!(
            validate_persisted_running_embedding_container_identity(
                Some(persisted),
                "container-b|2026-07-12T00:01:00Z|true",
            )
            .expect_err("replacement container must fail persisted identity proof")
            .to_string()
            .contains("does not match persisted")
        );
        assert!(
            validate_persisted_running_embedding_container_identity(None, persisted)
                .expect_err("missing persisted identity must fail")
                .to_string()
                .contains("does not match persisted")
        );
    }

    #[test]
    fn accelerator_smoke_fails_closed_before_rebuild_when_log_is_unproven() {
        let device = EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "accelerated",
            observation_source: "native_device_list",
            detected_provider: Some("amd".into()),
            detected_gpu: Some("AMD GPU".into()),
            accelerator_requested: true,
            accelerator_request_provider: Some("vulkan".into()),
            accelerator_request_device: Some("Vulkan0".into()),
            cpu_allowed: false,
            full_retrieval_allowed: true,
            degraded_reason: None,
        };
        let error = evaluate_embedding_accelerator_smoke(
            EmbeddingRuntimeProbe {
                reachable: true,
                detail: "reachable".into(),
                elapsed_ms: Some(12),
            },
            device,
            false,
        )
        .expect_err("inventory plus a reachable endpoint is not GPU proof");
        let message = error.to_string();
        assert!(message.contains("gpu_unverified"));
        assert!(message.contains("runtime_log_offload=false"));
    }

    #[test]
    fn accelerator_smoke_request_is_bound_to_selected_runtime_state() {
        let root = tempfile::tempdir().expect("runtime state dir");
        let mut runtime = SidecarRuntimeConfig::local();
        runtime.namespace = "agent-proof".into();
        runtime.layout.state_file = root.path().join("retrieval-sidecars.json");
        runtime.embedding.endpoint = "http://127.0.0.1:39001/v1/embeddings".into();
        std::fs::write(
            &runtime.layout.state_file,
            serde_json::json!({
                "namespace": runtime.namespace,
                "embed_url": runtime.embedding.endpoint,
                "embedding_accelerator_request_provider": "vulkan",
                "embedding_accelerator_request_device": "Vulkan0"
            })
            .to_string(),
        )
        .expect("write runtime state");

        let state = exact_embedding_runtime_state(&runtime).expect("matching runtime state");
        let request =
            embedding_accelerator_request_from_runtime_state(&state).expect("persisted request");
        assert_eq!(request.provider, "vulkan");
        assert_eq!(request.device.as_deref(), Some("Vulkan0"));

        let mut missing_provider = state;
        missing_provider.embedding_accelerator_request_provider = None;
        assert!(
            embedding_accelerator_request_from_runtime_state(&missing_provider)
                .expect_err("launch provider is not an accelerator request")
                .to_string()
                .contains("no requested provider")
        );

        runtime.namespace = "other-agent".into();
        let error = exact_embedding_runtime_state(&runtime)
            .expect_err("cross-namespace state must fail closed");
        assert!(error.to_string().contains("gpu_unverified"));
    }

    #[test]
    fn accelerator_smoke_reports_external_runtime_proof_boundary() {
        let mut runtime = SidecarRuntimeConfig::local();
        runtime.embedding.server_launch = Some("external_endpoint".into());
        runtime.embedding.endpoint = "http://127.0.0.1:39002/v1/embeddings".into();
        runtime.embedding.endpoint_origin =
            crate::config::EmbeddingEndpointOrigin::TrustedProjectConfig;
        runtime.embedding.device_policy = "accelerator_required".into();

        let error = ensure_embedding_accelerator_smoke_for_runtime(&runtime)
            .expect_err("external endpoint has no CodeStory-owned runtime log proof");
        let message = error.to_string();
        assert!(message.contains("gpu_unverified"));
        assert!(message.contains("external embedding endpoints"));
        assert!(!message.contains("docker"));
    }

    #[test]
    fn sidecar_log_observed_acceleration_allows_default_vulkan_request() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "windows/x86_64");
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7900 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let observed = observed_embedding_device_state_from_text(
            "llama_model_load: offloaded 33/33 layers to GPU\n",
        );
        let readiness = embedding_device_readiness_with_observed_state(
            Some(EmbeddingDeviceObservation {
                state: observed,
                source: "sidecar_log",
            }),
            None,
        );

        assert_eq!(observed, "accelerated");
        assert_eq!(readiness.requested_policy, "accelerator_required");
        assert_eq!(readiness.observed_state, "accelerated");
        assert_eq!(readiness.observation_source, "sidecar_log");
        assert_eq!(readiness.detected_provider.as_deref(), Some("amd"));
        assert_eq!(
            readiness.detected_gpu.as_deref(),
            Some("AMD Radeon RX 7900 XT")
        );
        assert!(readiness.accelerator_requested);
        assert_eq!(
            readiness.accelerator_request_provider.as_deref(),
            Some("vulkan")
        );
        assert_eq!(
            readiness.accelerator_request_device.as_deref(),
            Some("Vulkan0")
        );
        assert!(readiness.full_retrieval_allowed);
        assert!(readiness.degraded_reason.is_none());
    }

    #[test]
    fn inconclusive_sidecar_log_keeps_default_vulkan_request_unknown() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7900 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let observed = observed_embedding_device_state_from_text("server listening on 0.0.0.0");
        let readiness = embedding_device_readiness_with_observed_state(None, None);

        assert_eq!(observed, "unknown");
        assert_eq!(readiness.requested_policy, "accelerator_required");
        assert_eq!(readiness.observed_state, "unknown");
        assert_eq!(
            readiness.observation_source,
            "accelerator_request_unobserved"
        );
        assert_eq!(readiness.detected_provider.as_deref(), Some("amd"));
        assert!(readiness.accelerator_requested);
        assert!(!readiness.full_retrieval_allowed);
    }

    #[test]
    fn native_log_observed_acceleration_uses_native_source() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7900 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let readiness = embedding_device_readiness_with_observed_state(
            Some(EmbeddingDeviceObservation {
                state: "accelerated",
                source: "native_log",
            }),
            None,
        );

        assert_eq!(readiness.observed_state, "accelerated");
        assert_eq!(readiness.observation_source, "native_log");
        assert!(readiness.full_retrieval_allowed);
    }

    #[test]
    fn native_device_list_proves_requested_vulkan_device() {
        let request = EmbeddingAcceleratorRequest {
            provider: "vulkan".to_string(),
            device: Some("Vulkan0".to_string()),
            n_gpu_layers: "99".to_string(),
        };
        let devices = "Available devices:\n  Vulkan0: AMD Radeon RX 7900 XT (20464 MiB)\n";

        assert!(native_device_list_proves_accelerator(
            devices,
            &request,
            Some("Vulkan0")
        ));
        assert!(!native_device_list_proves_accelerator(
            "Available devices:\n  CUDA0: NVIDIA GPU\n",
            &request,
            Some("Vulkan0")
        ));
    }

    #[test]
    fn native_device_list_observation_allows_accelerator_required_readiness() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7900 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let readiness = embedding_device_readiness_with_observed_state(
            Some(EmbeddingDeviceObservation {
                state: "accelerated",
                source: "native_device_list",
            }),
            None,
        );

        assert_eq!(readiness.observed_state, "accelerated");
        assert_eq!(readiness.observation_source, "native_device_list");
        assert!(readiness.full_retrieval_allowed);
        assert!(readiness.degraded_reason.is_none());
    }

    #[test]
    fn native_log_current_launch_ignores_stale_acceleration() {
        let text = concat!(
            "starting native llama.cpp embedding server: old --device Vulkan0\n",
            "using device Vulkan0\n",
            "offloaded 33/33 layers to GPU\n",
            "starting native llama.cpp embedding server: current --device Vulkan0\n",
            "server listening on 0.0.0.0\n",
            "n_gpu_layers = 0\n",
        );

        let current = native_embedding_log_current_launch(text);

        assert!(current.contains("current --device Vulkan0"));
        assert!(!current.contains("old --device Vulkan0"));
        assert_eq!(observed_embedding_device_state_from_text(current), "cpu");
    }

    #[test]
    fn native_log_current_launch_accepts_spawn_log_prefix() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _llama_device = EnvGuard::remove(LLAMACPP_DEVICE_ENV);
        let _platform = EnvGuard::set("CODESTORY_TEST_HOST_PLATFORM", "macos/aarch64");
        let text = concat!(
            "starting native llama.cpp embedding server: old --device Vulkan0\n",
            "using device Vulkan0\n",
            "offloaded 33/33 layers to GPU\n",
            "starting native llama.cpp embedding server: after probe failed (unreachable) /cache/llama-server --port 18080\n",
            "MTL0 : Apple M4 Pro\n",
            "MTL : EMBED_LIBRARY = 1\n",
        );

        let current = native_embedding_log_current_launch(text);

        assert!(current.contains("after probe failed"));
        assert!(!current.contains("old --device Vulkan0"));
        assert_eq!(
            observed_embedding_device_state_from_text(current),
            "accelerated"
        );
    }

    #[test]
    fn native_log_rotation_keeps_exact_bounded_raw_tail() {
        let dir = tempfile::tempdir().expect("log dir");
        let path = dir.path().join("llama-server-native.log");
        let mut bytes = vec![b'x'; NATIVE_LLAMA_PREVIOUS_LOG_TAIL_BYTES as usize + 37];
        let tail_start = bytes.len() - NATIVE_LLAMA_PREVIOUS_LOG_TAIL_BYTES as usize;
        bytes[tail_start] = 0xff;
        std::fs::write(&path, &bytes).expect("write oversized log");

        prepare_native_embedding_log_for_launch(&path).expect("rotate log");

        assert_eq!(std::fs::metadata(&path).expect("current metadata").len(), 0);
        let previous = std::fs::read(dir.path().join("llama-server-native.previous.log"))
            .expect("previous tail");
        assert_eq!(
            previous.len(),
            NATIVE_LLAMA_PREVIOUS_LOG_TAIL_BYTES as usize
        );
        assert_eq!(previous, bytes[tail_start..]);
    }

    #[test]
    fn bounded_native_log_read_ignores_stale_acceleration_outside_tail() {
        let dir = tempfile::tempdir().expect("log dir");
        let path = dir.path().join("llama-server-native.log");
        let mut bytes =
            b"starting native llama.cpp embedding server: old\noffloaded 33/33 layers to GPU\n"
                .to_vec();
        bytes.extend(std::iter::repeat_n(
            b'x',
            NATIVE_LLAMA_LOG_READ_TAIL_BYTES as usize,
        ));
        bytes.extend_from_slice(NATIVE_LLAMA_LOG_START_MARKER.as_bytes());
        bytes.extend_from_slice(b" current\nn_gpu_layers = 0\n");
        std::fs::write(&path, bytes).expect("write multi-launch log");

        let text = read_native_embedding_log_tail(&path, NATIVE_LLAMA_LOG_READ_TAIL_BYTES)
            .expect("bounded tail");
        let current = native_embedding_log_current_launch(&text);

        assert!(current.contains("current"));
        assert!(!current.contains("offloaded 33/33"));
        assert_eq!(observed_embedding_device_state_from_text(current), "cpu");
    }

    #[test]
    fn explicit_unknown_device_still_fails_closed_on_amd_host() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::set(DEVICE_STATE_ENV, "unknown");
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7800 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let readiness = embedding_device_readiness();

        assert_eq!(readiness.requested_policy, "accelerator_required");
        assert_eq!(readiness.observed_state, "unknown");
        assert_eq!(readiness.observation_source, "manual_env");
        assert!(!readiness.full_retrieval_allowed);
    }

    #[test]
    fn explicit_accelerated_device_allows_retrieval_with_manual_observation_source() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::set(DEVICE_STATE_ENV, "vulkan");
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let readiness = embedding_device_readiness();

        assert_eq!(readiness.requested_policy, "accelerator_required");
        assert_eq!(readiness.observed_state, "accelerated");
        assert_eq!(readiness.observation_source, "manual_env");
        assert!(readiness.full_retrieval_allowed);
        assert!(readiness.degraded_reason.is_none());
    }

    #[test]
    fn cpu_policy_without_device_state_reports_cpu_policy_source() {
        let _lock = crate::test_support::env_lock();
        let _allow_cpu = EnvGuard::set(ALLOW_CPU_ENV, "1");
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let readiness = embedding_device_readiness();

        assert_eq!(readiness.requested_policy, "cpu_allowed");
        assert_eq!(readiness.observed_state, "cpu");
        assert_eq!(readiness.observation_source, "cpu_policy");
        assert!(readiness.full_retrieval_allowed);
        assert!(readiness.degraded_reason.is_none());
    }

    #[test]
    fn product_embedding_backend_requires_device_policy() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _real = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);

        let error = ensure_product_embedding_backend()
            .expect_err("unknown device should not satisfy product readiness");

        assert!(
            error.to_string().contains("embedding_device_unverified"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn product_embedding_backend_uses_runtime_native_log_observation() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _real = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        let _launch = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "native_spawned");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");
        let root = tempfile::TempDir::new().expect("temp dir");
        let runtime = SidecarRuntimeConfig {
            project_identity: None,
            layout: SidecarLayout {
                qdrant_http_port: 16333,
                qdrant_grpc_port: 16334,
                lexical_data_dir: root.path().join("lexical"),
                qdrant_data_dir: root.path().join("qdrant"),
                scip_artifacts_root: root.path().join("scip"),
                state_file: root.path().join("state").join("retrieval-sidecars.json"),
            },
            profile: SidecarProfile::Agent,
            run_id: Some("shared-agent".into()),
            namespace: "agent-shared-agent".into(),
            compose_project: "codestory-agent-shared-agent".into(),
            embed_http_port: 18080,
            cleanup_command: "codestory-cli retrieval down".into(),
            labels: BTreeMap::new(),
            ..SidecarRuntimeConfig::local()
        };
        std::fs::create_dir_all(runtime.layout.state_file.parent().expect("state parent"))
            .expect("create state dir");
        std::fs::write(
            native_embedding_log_path(&runtime),
            "llama_model_load: offloaded 33/33 layers to GPU\n",
        )
        .expect("write native log");

        let generic_error = ensure_product_embedding_backend()
            .expect_err("generic check should not see the runtime native log");
        assert!(
            generic_error
                .to_string()
                .contains("embedding_device_unverified")
        );

        ensure_product_embedding_backend_for_runtime(&runtime)
            .expect("runtime native log should satisfy accelerator policy");
    }

    #[test]
    fn product_embedding_backend_for_runtime_waits_for_native_log_observation() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _real = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        let _launch = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "native_spawned");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");
        let root = tempfile::TempDir::new().expect("temp dir");
        let runtime = SidecarRuntimeConfig {
            project_identity: None,
            layout: SidecarLayout {
                qdrant_http_port: 16333,
                qdrant_grpc_port: 16334,
                lexical_data_dir: root.path().join("lexical"),
                qdrant_data_dir: root.path().join("qdrant"),
                scip_artifacts_root: root.path().join("scip"),
                state_file: root.path().join("state").join("retrieval-sidecars.json"),
            },
            profile: SidecarProfile::Agent,
            run_id: Some("shared-agent".into()),
            namespace: "agent-shared-agent".into(),
            compose_project: "codestory-agent-shared-agent".into(),
            embed_http_port: 18080,
            cleanup_command: "codestory-cli retrieval down".into(),
            labels: BTreeMap::new(),
            ..SidecarRuntimeConfig::local()
        };
        std::fs::create_dir_all(runtime.layout.state_file.parent().expect("state parent"))
            .expect("create state dir");
        let log_path = native_embedding_log_path(&runtime);
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            std::fs::write(
                log_path,
                "starting native llama.cpp embedding server: test --device Vulkan0\nload_tensors: offloaded 13/13 layers to GPU\n",
            )
            .expect("write native log");
        });

        ensure_product_embedding_backend_for_runtime(&runtime)
            .expect("runtime helper should wait for native log acceleration");
        writer.join().expect("native log writer");
    }

    #[test]
    fn product_embedding_backend_allows_explicit_cpu_policy() {
        let _lock = crate::test_support::env_lock();
        let _backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _real = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::set(DEVICE_POLICY_ENV, "allow_cpu");
        let _device = EnvGuard::set(DEVICE_STATE_ENV, "cpu");

        ensure_product_embedding_backend().expect("explicit CPU policy should be accepted");
    }

    #[test]
    fn loopback_embedding_urls_are_allowed() {
        assert!(is_loopback_embedding_url(
            "http://127.0.0.1:8080/v1/embeddings"
        ));
        assert!(is_loopback_embedding_url(
            "http://localhost:8080/v1/embeddings"
        ));
        assert!(is_loopback_embedding_url("http://[::1]:8080/v1/embeddings"));
    }

    #[test]
    fn remote_embedding_urls_are_not_loopback() {
        assert!(!is_loopback_embedding_url(
            "https://example.com/v1/embeddings"
        ));
        assert!(!is_loopback_embedding_url(
            "http://192.168.1.10:8080/v1/embeddings"
        ));
        assert!(!is_loopback_embedding_url(
            "http://localhost.example.com:8080/v1/embeddings"
        ));
    }

    #[test]
    fn remote_embedding_url_requires_explicit_opt_in() {
        let _lock = crate::test_support::env_lock();
        let _guard = EnvGuard::remove(ALLOW_REMOTE_EMBEDDINGS_ENV);
        let error = ensure_llamacpp_url_allowed("http://192.168.1.10:8080/v1/embeddings")
            .expect_err("remote URL should be rejected by default");
        assert!(error.to_string().contains(ALLOW_REMOTE_EMBEDDINGS_ENV));

        let _allow = EnvGuard::set(ALLOW_REMOTE_EMBEDDINGS_ENV, "1");
        ensure_llamacpp_url_allowed("http://192.168.1.10:8080/v1/embeddings")
            .expect("remote URL should be allowed when explicitly opted in");
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: test-only single-threaded env mutation.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            // SAFETY: test-only env mutation guarded by crate::test_support::env_lock().
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: test-only single-threaded env mutation.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
