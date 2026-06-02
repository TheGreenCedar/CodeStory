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
use std::thread;
use std::time::Duration;

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

const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_LLAMACPP_BATCH_SIZE: usize = 128;
const DEFAULT_LLAMACPP_REQUEST_COUNT: usize = 6;

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeProbe {
    pub reachable: bool,
    pub detail: String,
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
