//! Query embeddings for Qdrant plus diagnostic document embedding helpers.
//!
//! Product Qdrant indexing copies stored local semantic-document vectors. The live sidecar still
//! uses **BAAI/bge-base-en-v1.5** (768-dim) via llama.cpp `/v1/embeddings` for query vectors and
//! semantic smoke checks.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
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

const LLAMACPP_URL_ENV: &str = "CODESTORY_EMBED_LLAMACPP_URL";
const DEFAULT_LLAMACPP_URL: &str = "http://127.0.0.1:8080/v1/embeddings";
const EMBEDDING_BACKEND_ENV: &str = "CODESTORY_EMBED_BACKEND";
const QUERY_PREFIX_ENV: &str = "CODESTORY_EMBED_QUERY_PREFIX";
const DOCUMENT_PREFIX_ENV: &str = "CODESTORY_EMBED_DOCUMENT_PREFIX";
const LLAMACPP_BATCH_SIZE_ENV: &str = "CODESTORY_EMBED_LLAMACPP_BATCH_SIZE";
const LLAMACPP_REQUEST_COUNT_ENV: &str = "CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT";
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
const RUNTIME_EMBED_DEVICE_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(10);
const RUNTIME_EMBED_DEVICE_OBSERVATION_POLL: Duration = Duration::from_millis(250);

const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_LLAMACPP_BATCH_SIZE: usize = 128;
const DEFAULT_LLAMACPP_REQUEST_COUNT: usize = 6;

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeProbe {
    pub reachable: bool,
    pub detail: String,
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
    pub device: String,
    pub n_gpu_layers: String,
}

/// Stable id stored on retrieval manifest rows (backend + model family).
pub fn embedding_runtime_id() -> String {
    if llamacpp_backend_selected() {
        PRODUCT_EMBEDDING_RUNTIME_ID.into()
    } else if super::config::qdrant_semantic_vectors_enabled() {
        "hash-projection:768".into()
    } else {
        "hash-label:8".into()
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
    ensure_product_embedding_backend_static()?;
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
    if !super::config::qdrant_semantic_vectors_enabled() {
        bail!("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS=0 is unsupported for product sidecar indexing");
    }
    if !llamacpp_backend_selected() {
        bail!(
            "llama.cpp embedding sidecar is mandatory; set CODESTORY_EMBED_BACKEND=llamacpp and CODESTORY_EMBED_LLAMACPP_URL"
        );
    }
    Ok(())
}

pub fn embedding_device_readiness() -> EmbeddingDeviceReadiness {
    embedding_device_readiness_with_observed_state(None)
}

pub fn embedding_device_readiness_for_runtime(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> EmbeddingDeviceReadiness {
    embedding_device_readiness_with_observed_state(observe_sidecar_embedding_device_state(runtime))
}

fn embedding_device_readiness_with_observed_state(
    sidecar_observed_state: Option<EmbeddingDeviceObservation>,
) -> EmbeddingDeviceReadiness {
    let cpu_allowed = explicit_cpu_allowed();
    let detection = host_embedding_device_detection();
    let accelerator_request = if cpu_allowed {
        None
    } else {
        embedding_accelerator_request_for_detection(detection.as_ref())
    };
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
        accelerator_request_provider: accelerator_request.as_ref().map(|_| "vulkan".to_string()),
        accelerator_request_device: accelerator_request
            .as_ref()
            .map(|request| request.device.clone()),
        cpu_allowed,
        full_retrieval_allowed,
        degraded_reason,
    }
}

fn observe_sidecar_embedding_device_state(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<EmbeddingDeviceObservation> {
    if crate::config::embedding_server_launch_mode()
        .ok()
        .is_some_and(|mode| mode == crate::config::EmbeddingServerLaunchMode::NativeSpawned)
    {
        return observe_native_embedding_device_state(runtime);
    }
    let output = Command::new("docker")
        .args([
            "logs",
            "--tail",
            "200",
            &format!("{}-embed", runtime.compose_project),
        ])
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
    match observed_embedding_device_state_from_text(&text) {
        "unknown" => None,
        state => Some(EmbeddingDeviceObservation {
            state,
            source: "sidecar_log",
        }),
    }
}

fn observe_native_embedding_device_state(
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<EmbeddingDeviceObservation> {
    let text = std::fs::read_to_string(native_embedding_log_path(runtime)).ok()?;
    match observed_embedding_device_state_from_text(native_embedding_log_current_launch(&text)) {
        "unknown" => None,
        state => Some(EmbeddingDeviceObservation {
            state,
            source: "native_log",
        }),
    }
}

pub(crate) fn native_embedding_log_path(runtime: &crate::config::SidecarRuntimeConfig) -> PathBuf {
    runtime
        .layout
        .state_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("llama-server-native.log")
}

fn native_embedding_log_current_launch(text: &str) -> &str {
    text.rfind(NATIVE_LLAMA_LOG_START_MARKER)
        .map(|offset| &text[offset..])
        .unwrap_or(text)
}

pub fn embedding_accelerator_request() -> Option<EmbeddingAcceleratorRequest> {
    if explicit_cpu_allowed() {
        return None;
    }
    let detection = host_embedding_device_detection();
    embedding_accelerator_request_for_detection(detection.as_ref())
}

fn embedding_accelerator_request_for_detection(
    detection: Option<&HostGpuDetection>,
) -> Option<EmbeddingAcceleratorRequest> {
    let detection = detection?;
    (detection.provider == "amd").then(|| EmbeddingAcceleratorRequest {
        device: env_trimmed(LLAMACPP_DEVICE_ENV).unwrap_or_else(|| "Vulkan0".to_string()),
        n_gpu_layers: env_trimmed(LLAMACPP_N_GPU_LAYERS_ENV).unwrap_or_else(|| "99".to_string()),
    })
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
            "accelerated" | "gpu" | "vulkan" | "cuda" | "metal" => Some("accelerated"),
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
    for line in text.lines().map(|line| line.to_ascii_lowercase()) {
        if line_reports_gpu_offload(&line) == Some(true)
            || (line.contains("using device")
                && ["vulkan", "cuda", "metal"]
                    .iter()
                    .any(|needle| line.contains(needle)))
        {
            return "accelerated";
        }
        saw_cpu |= line_reports_gpu_offload(&line) == Some(false)
            || line.contains("n_gpu_layers = 0")
            || line.contains("using cpu")
            || line.contains("no gpu")
            || line.contains("no vulkan device");
    }
    if saw_cpu { "cpu" } else { "unknown" }
}

fn line_reports_gpu_offload(line: &str) -> Option<bool> {
    let keyword = if let Some(offset) = line.find("offloaded ") {
        offset + "offloaded ".len()
    } else if let Some(offset) = line.find("offloading ") {
        offset + "offloading ".len()
    } else {
        return None;
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
    r#"$job = Start-Job -ScriptBlock { Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) $($_.AdapterCompatibility)" } }; if (Wait-Job $job -Timeout 2) { Receive-Job $job; Remove-Job $job -Force } else { Stop-Job $job; Remove-Job $job -Force; exit 124 }"#
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
    let prefix = query_prefix();
    embed_prepared(&format!("{prefix}{text}"))
}

/// Active embedding backend label for ops/status (`hash`, `llamacpp`).
pub fn embedding_backend_label() -> &'static str {
    if llamacpp_backend_selected() {
        "llamacpp"
    } else {
        "hash"
    }
}

pub fn embed_documents(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let prefix = std::env::var(DOCUMENT_PREFIX_ENV).unwrap_or_default();
    let prepared = texts
        .iter()
        .map(|text| format!("{prefix}{text}"))
        .collect::<Vec<_>>();
    if prepared.iter().any(|text| text.trim().is_empty()) {
        bail!("cannot embed empty text");
    }
    if llamacpp_backend_selected() {
        llamacpp_embed_batched(&prepared)
    } else {
        Ok(prepared
            .iter()
            .map(|text| hash_projection_embed(text, RETRIEVAL_EMBEDDING_DIM))
            .collect())
    }
}

fn query_prefix() -> String {
    if let Ok(value) = std::env::var(QUERY_PREFIX_ENV)
        && (!value.is_empty() || !llamacpp_backend_selected())
    {
        return value;
    }
    if llamacpp_backend_selected() {
        return BGE_QUERY_PREFIX_DEFAULT.to_string();
    }
    String::new()
}

fn embed_prepared(prepared: &str) -> Result<Vec<f32>> {
    if prepared.trim().is_empty() {
        bail!("cannot embed empty text");
    }
    if llamacpp_backend_selected() {
        llamacpp_embed(&[prepared.to_string()])?
            .pop()
            .ok_or_else(|| anyhow!("llama.cpp returned no embedding vector"))
    } else {
        Ok(hash_projection_embed(prepared, RETRIEVAL_EMBEDDING_DIM))
    }
}

fn llamacpp_backend_selected() -> bool {
    match std::env::var(EMBEDDING_BACKEND_ENV) {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "llamacpp" || normalized == "llama_cpp"
        }
        Err(_) => {
            super::config::qdrant_semantic_vectors_enabled()
                || std::env::var(LLAMACPP_URL_ENV).is_ok()
        }
    }
}

fn llamacpp_embed(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let url = llamacpp_url()?;
    llamacpp_embed_with_timeout(texts, &url, HTTP_TIMEOUT)
}

fn llamacpp_embed_batched(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let batch_size = env_usize(
        LLAMACPP_BATCH_SIZE_ENV,
        DEFAULT_LLAMACPP_BATCH_SIZE,
        1,
        1024,
    );
    let request_count = env_usize(
        LLAMACPP_REQUEST_COUNT_ENV,
        DEFAULT_LLAMACPP_REQUEST_COUNT,
        1,
        16,
    );
    if texts.len() <= batch_size {
        return llamacpp_embed(texts);
    }

    let url = llamacpp_url()?;
    let batches = texts
        .chunks(batch_size)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();
    let mut output = Vec::with_capacity(texts.len());
    for (wave_index, wave) in batches.chunks(request_count).enumerate() {
        let mut wave_results = thread::scope(|scope| {
            let mut handles = Vec::with_capacity(wave.len());
            for (index, batch) in wave.iter().cloned().enumerate() {
                let url = url.clone();
                handles.push(scope.spawn(move || {
                    llamacpp_embed_with_timeout(&batch, &url, HTTP_TIMEOUT)
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

fn env_usize(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(min, max))
        .unwrap_or(default)
}

pub fn probe_product_embedding_runtime() -> EmbeddingRuntimeProbe {
    let result = llamacpp_url().and_then(|url| {
        llamacpp_embed_with_timeout(
            &["codestory health probe".to_string()],
            &url,
            HEALTH_TIMEOUT,
        )
    });
    match result {
        Ok(vectors) => EmbeddingRuntimeProbe {
            reachable: true,
            detail: format!(
                "llama.cpp embeddings reachable dim={}",
                vectors.first().map(|vector| vector.len()).unwrap_or(0)
            ),
        },
        Err(error) => EmbeddingRuntimeProbe {
            reachable: false,
            detail: format!("llama.cpp embeddings unavailable: {error}"),
        },
    }
}

fn llamacpp_url() -> Result<String> {
    let url = std::env::var(LLAMACPP_URL_ENV).unwrap_or_else(|_| DEFAULT_LLAMACPP_URL.to_string());
    ensure_llamacpp_url_allowed(&url)?;
    Ok(url)
}

fn ensure_llamacpp_url_allowed(url: &str) -> Result<()> {
    if !allow_remote_embeddings() && !is_loopback_embedding_url(url) {
        bail!(
            "remote embedding URL is disabled; use a loopback URL or set {ALLOW_REMOTE_EMBEDDINGS_ENV}=1"
        );
    }
    Ok(())
}

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
    url: &str,
    timeout: Duration,
) -> Result<Vec<Vec<f32>>> {
    let body = serde_json::json!({
        "input": texts,
        "model": "bge-base-en-v1.5",
    });
    let payload = serde_json::to_string(&body).context("serialize embeddings request")?;
    let response = ureq::post(url)
        .timeout(timeout)
        .set("Content-Type", "application/json")
        .send_string(&payload)
        .map_err(|error| anyhow!("llama.cpp embeddings request failed: {error}"))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        bail!("llama.cpp embeddings http {status}");
    }
    let response_body = response.into_string().unwrap_or_default();
    parse_openai_embeddings(&response_body, true)
}

#[derive(Deserialize)]
struct OpenAiEmbeddingsResponse {
    data: Vec<OpenAiEmbeddingRow>,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingRow {
    embedding: Vec<f32>,
}

fn parse_openai_embeddings(body: &str, require_llamacpp_dim: bool) -> Result<Vec<Vec<f32>>> {
    let parsed: OpenAiEmbeddingsResponse =
        serde_json::from_str(body).context("parse llama.cpp embeddings json")?;
    if parsed.data.is_empty() {
        bail!("llama.cpp embeddings response had no data rows");
    }
    let mut vectors = Vec::with_capacity(parsed.data.len());
    for row in parsed.data {
        if require_llamacpp_dim && row.embedding.len() != RETRIEVAL_EMBEDDING_DIM {
            bail!(
                "llama.cpp embedding dim {} != expected {} (bge-base-en-v1.5); check model GGUF and CODESTORY_EMBED_BACKEND",
                row.embedding.len(),
                RETRIEVAL_EMBEDDING_DIM
            );
        }
        vectors.push(row.embedding);
    }
    Ok(vectors)
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
    if super::config::qdrant_semantic_vectors_enabled() {
        RETRIEVAL_EMBEDDING_DIM
    } else {
        8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SidecarLayout, SidecarProfile, SidecarRuntimeConfig};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn hash_projection_dim_matches_retrieval_embedding_dim() {
        let vector = hash_projection_embed("extension_service handler", RETRIEVAL_EMBEDDING_DIM);
        assert_eq!(vector.len(), RETRIEVAL_EMBEDDING_DIM);
        let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01 || vector.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn embed_documents_preserves_count_for_hash_projection() {
        let _lock = ENV_LOCK.lock().expect("env lock");
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
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::remove("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS");
        let _guard2 = EnvGuard::remove(EMBEDDING_BACKEND_ENV);
        assert_eq!(embedding_runtime_id(), PRODUCT_EMBEDDING_RUNTIME_ID);
        assert_eq!(qdrant_vector_dim(), RETRIEVAL_EMBEDDING_DIM);
    }

    #[test]
    fn embedding_runtime_id_llamacpp_when_backend_set() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _guard2 = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        assert_eq!(embedding_runtime_id(), "llamacpp:bge-base-en-v1.5");
    }

    #[test]
    fn explicit_onnx_backend_is_not_product_sidecar_runtime() {
        let _lock = ENV_LOCK.lock().expect("env lock");
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
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");

        let readiness = embedding_device_readiness();

        assert_eq!(readiness.requested_policy, "accelerator_required");
        assert_eq!(readiness.observed_state, "unknown");
        assert_eq!(readiness.observation_source, "sidecar_unobserved");
        assert_eq!(readiness.detected_provider, None);
        assert_eq!(readiness.detected_gpu, None);
        assert!(!readiness.accelerator_requested);
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
        let _lock = ENV_LOCK.lock().expect("env lock");
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

        assert!(script.contains("Wait-Job $job -Timeout 2"));
        assert!(script.contains("AdapterCompatibility"));
        assert!(script.contains("Stop-Job $job"));
        assert!(script.contains("exit 124"));
    }

    #[test]
    fn amd_provider_env_detects_and_requests_vulkan_without_observing_acceleration() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7800 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);
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
        assert_eq!(readiness.detected_provider.as_deref(), Some("amd"));
        assert_eq!(
            readiness.detected_gpu.as_deref(),
            Some("AMD Radeon RX 7800 XT")
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
        assert!(!readiness.cpu_allowed);
        assert!(!readiness.full_retrieval_allowed);
        assert!(
            readiness
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar did not prove accelerator execution")
        );
        assert_eq!(request.device, "Vulkan0");
        assert_eq!(request.n_gpu_layers, "99");
    }

    #[test]
    fn sidecar_log_observed_acceleration_allows_amd_vulkan_request() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7900 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let observed = observed_embedding_device_state_from_text(
            "llama_model_load: offloaded 33/33 layers to GPU\n",
        );
        let readiness =
            embedding_device_readiness_with_observed_state(Some(EmbeddingDeviceObservation {
                state: observed,
                source: "sidecar_log",
            }));

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
    fn inconclusive_sidecar_log_keeps_amd_vulkan_request_unknown() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7900 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let observed = observed_embedding_device_state_from_text("server listening on 0.0.0.0");
        let readiness = embedding_device_readiness_with_observed_state(None);

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
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _provider = EnvGuard::set(DEVICE_PROVIDER_ENV, "amd");
        let _name = EnvGuard::set(DEVICE_NAME_ENV, "AMD Radeon RX 7900 XT");
        let _host_detect = EnvGuard::remove(DISABLE_HOST_GPU_DETECT_ENV);

        let readiness =
            embedding_device_readiness_with_observed_state(Some(EmbeddingDeviceObservation {
                state: "accelerated",
                source: "native_log",
            }));

        assert_eq!(readiness.observed_state, "accelerated");
        assert_eq!(readiness.observation_source, "native_log");
        assert!(readiness.full_retrieval_allowed);
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
    fn explicit_unknown_device_still_fails_closed_on_amd_host() {
        let _lock = ENV_LOCK.lock().expect("env lock");
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
        let _lock = ENV_LOCK.lock().expect("env lock");
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
        let _lock = ENV_LOCK.lock().expect("env lock");
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
        let _lock = ENV_LOCK.lock().expect("env lock");
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
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _real = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        let _launch = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "native_spawned");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");
        let root = tempfile::TempDir::new().expect("temp dir");
        let runtime = SidecarRuntimeConfig {
            layout: SidecarLayout {
                zoekt_http_port: 16070,
                qdrant_http_port: 16333,
                qdrant_grpc_port: 16334,
                zoekt_data_dir: root.path().join("zoekt"),
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
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "llamacpp");
        let _real = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        let _launch = EnvGuard::set("CODESTORY_EMBED_SERVER_LAUNCH", "native_spawned");
        let _allow_cpu = EnvGuard::remove(ALLOW_CPU_ENV);
        let _policy = EnvGuard::remove(DEVICE_POLICY_ENV);
        let _device = EnvGuard::remove(DEVICE_STATE_ENV);
        let _host_detect = EnvGuard::set(DISABLE_HOST_GPU_DETECT_ENV, "1");
        let root = tempfile::TempDir::new().expect("temp dir");
        let runtime = SidecarRuntimeConfig {
            layout: SidecarLayout {
                zoekt_http_port: 16070,
                qdrant_http_port: 16333,
                qdrant_grpc_port: 16334,
                zoekt_data_dir: root.path().join("zoekt"),
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
        let _lock = ENV_LOCK.lock().expect("env lock");
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
        let _lock = ENV_LOCK.lock().expect("env lock");
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
            // SAFETY: test-only env mutation guarded by ENV_LOCK.
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
