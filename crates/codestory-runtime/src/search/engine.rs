use anyhow::{Context, Result, anyhow};
use codestory_contracts::graph::NodeId;
use ndarray::{Array2, ArrayView2, ArrayView3, Axis};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32String};
use ort::session::{
    Session,
    builder::{GraphOptimizationLevel, PrepackedWeights, SessionBuilder},
};
use ort::value::TensorRef;
use rayon::prelude::*;
use serde_json::{Value as JsonValue, json};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::schema::{FAST, INDEXED, STORED, Schema, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

pub const EMBEDDING_DIM: usize = 384;
const SEARCH_WRITER_HEAP_BYTES: usize = 20_000_000;
pub const EMBEDDING_MODEL_ENV: &str = "CODESTORY_EMBED_MODEL_PATH";
pub const EMBEDDING_MODEL_ID_ENV: &str = "CODESTORY_EMBED_MODEL_ID";
pub const EMBEDDING_TOKENIZER_ENV: &str = "CODESTORY_EMBED_TOKENIZER_PATH";
pub const EMBEDDING_MAX_TOKENS_ENV: &str = "CODESTORY_EMBED_MAX_TOKENS";
pub const EMBEDDING_RUNTIME_MODE_ENV: &str = "CODESTORY_EMBED_RUNTIME_MODE";
pub const EMBEDDING_BACKEND_ENV: &str = "CODESTORY_EMBED_BACKEND";
pub const EMBEDDING_PROFILE_ENV: &str = "CODESTORY_EMBED_PROFILE";
pub const EMBEDDING_POOLING_ENV: &str = "CODESTORY_EMBED_POOLING";
pub const EMBEDDING_QUERY_PREFIX_ENV: &str = "CODESTORY_EMBED_QUERY_PREFIX";
pub const EMBEDDING_DOCUMENT_PREFIX_ENV: &str = "CODESTORY_EMBED_DOCUMENT_PREFIX";
pub const EMBEDDING_LAYER_NORM_ENV: &str = "CODESTORY_EMBED_LAYER_NORM";
pub const EMBEDDING_TRUNCATE_DIM_ENV: &str = "CODESTORY_EMBED_TRUNCATE_DIM";
pub const EMBEDDING_EXPECTED_DIM_ENV: &str = "CODESTORY_EMBED_EXPECTED_DIM";
pub const LLAMACPP_EMBEDDINGS_URL_ENV: &str = "CODESTORY_EMBED_LLAMACPP_URL";
pub const LLAMACPP_REQUEST_COUNT_ENV: &str = "CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT";
pub const DEFAULT_BUNDLED_EMBED_MODEL_PATH: &str = "models/bge-small-en-v1.5/onnx/model.onnx";
const EMBEDDING_INTRA_THREADS_ENV: &str = "CODESTORY_EMBED_INTRA_THREADS";
const EMBEDDING_INTER_THREADS_ENV: &str = "CODESTORY_EMBED_INTER_THREADS";
const EMBEDDING_PARALLEL_EXECUTION_ENV: &str = "CODESTORY_EMBED_PARALLEL_EXECUTION";
const EMBEDDING_EXECUTION_PROVIDER_ENV: &str = "CODESTORY_EMBED_EXECUTION_PROVIDER";
const EMBEDDING_SESSION_COUNT_ENV: &str = "CODESTORY_EMBED_SESSION_COUNT";
const DEFAULT_LLAMACPP_EMBEDDINGS_URL: &str = "http://127.0.0.1:8080/v1/embeddings";

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeAvailability {
    pub available: bool,
    pub model_id: Option<String>,
    pub fallback_message: Option<String>,
}

fn resolve_bundled_embed_model_path() -> Option<PathBuf> {
    let relative = PathBuf::from(DEFAULT_BUNDLED_EMBED_MODEL_PATH);
    let mut candidates = Vec::new();

    // Relative-to-cwd check.
    candidates.push(relative.clone());

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(&relative));
        for ancestor in cwd.ancestors().skip(1).take(8) {
            candidates.push(ancestor.join(&relative));
        }
    }

    // Cargo and other launchers can start the process from different cwd values.
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        candidates.push(exe_dir.join(&relative));
        for ancestor in exe_dir.ancestors().skip(1).take(8) {
            candidates.push(ancestor.join(&relative));
        }
    }

    let mut seen = HashSet::new();
    for candidate in candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

fn default_tokenizer_path_for_model(model_path: &Path) -> PathBuf {
    let sibling = model_path.with_file_name("tokenizer.json");
    if sibling.is_file() {
        return sibling;
    }

    if let Some(parent_tokenizer) = model_path
        .parent()
        .and_then(Path::parent)
        .map(|parent| parent.join("tokenizer.json"))
        && parent_tokenizer.is_file()
    {
        return parent_tokenizer;
    }

    sibling
}

fn env_usize(key: &str, min: usize, max: usize) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|value| value.clamp(min, max))
}

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|raw| {
            matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn env_bool_override(key: &str) -> Option<bool> {
    std::env::var(key).ok().and_then(|raw| {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    })
}

fn embedding_session_count_from_env() -> usize {
    env_usize(EMBEDDING_SESSION_COUNT_ENV, 1, 16).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|threads| threads.get().clamp(1, 2))
            .unwrap_or(1)
    })
}

fn embedding_parallel_chunk_size(text_count: usize, worker_count: usize) -> usize {
    let workers = worker_count.max(1).min(text_count.max(1));
    text_count.max(1).div_ceil(workers).max(1)
}

#[cfg(feature = "onnx-cuda")]
fn configure_cuda_embedding_provider(builder: SessionBuilder) -> Result<SessionBuilder> {
    builder
        .with_execution_providers([ort::ep::CUDAExecutionProvider::default().build()])
        .context("failed to configure CUDA ONNX execution provider")
}

#[cfg(not(feature = "onnx-cuda"))]
fn configure_cuda_embedding_provider(builder: SessionBuilder) -> Result<SessionBuilder> {
    tracing::warn!(
        "{EMBEDDING_EXECUTION_PROVIDER_ENV}=cuda requested, but this binary was built without the onnx-cuda feature; using CPU execution provider"
    );
    Ok(builder)
}

#[cfg(feature = "onnx-directml")]
fn configure_directml_embedding_provider(builder: SessionBuilder) -> Result<SessionBuilder> {
    builder
        .with_execution_providers([ort::ep::DirectMLExecutionProvider::default().build()])
        .context("failed to configure DirectML ONNX execution provider")
}

#[cfg(not(feature = "onnx-directml"))]
fn configure_directml_embedding_provider(builder: SessionBuilder) -> Result<SessionBuilder> {
    tracing::warn!(
        "{EMBEDDING_EXECUTION_PROVIDER_ENV}=directml requested, but this binary was built without the onnx-directml feature; using CPU execution provider"
    );
    Ok(builder)
}

fn configure_embedding_execution_provider(builder: SessionBuilder) -> Result<SessionBuilder> {
    let provider = std::env::var(EMBEDDING_EXECUTION_PROVIDER_ENV)
        .unwrap_or_else(|_| "cpu".to_string())
        .trim()
        .to_ascii_lowercase();
    match provider.as_str() {
        "" | "cpu" => Ok(builder),
        "cuda" => configure_cuda_embedding_provider(builder),
        "directml" | "dml" => configure_directml_embedding_provider(builder),
        other => {
            tracing::warn!(
                execution_provider = other,
                "Unknown semantic embedding execution provider; using CPU execution provider"
            );
            Ok(builder)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingBackendSelection {
    Onnx,
    LlamaCpp,
    HashProjection,
}

impl EmbeddingBackendSelection {
    fn from_env() -> Result<Self> {
        let runtime_mode = std::env::var(EMBEDDING_RUNTIME_MODE_ENV)
            .unwrap_or_else(|_| "onnx".to_string())
            .trim()
            .to_ascii_lowercase();
        if runtime_mode == "hash" || runtime_mode == "hash_projection" {
            return Ok(Self::HashProjection);
        }

        let backend = std::env::var(EMBEDDING_BACKEND_ENV)
            .unwrap_or_else(|_| runtime_mode)
            .trim()
            .to_ascii_lowercase();
        match backend.as_str() {
            "" | "auto" | "onnx" => Ok(Self::Onnx),
            "hash" | "hash_projection" => Ok(Self::HashProjection),
            "llamacpp" | "llama.cpp" | "llama-cpp" | "gguf" => Ok(Self::LlamaCpp),
            other => Err(anyhow!(
                "unsupported embedding backend `{other}` (set {EMBEDDING_BACKEND_ENV}=onnx, hash, or llamacpp)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Onnx => "onnx",
            Self::LlamaCpp => "llamacpp",
            Self::HashProjection => "hash",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingPooling {
    Mean,
    Cls,
    LastToken,
}

impl EmbeddingPooling {
    fn from_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mean" | "avg" | "average" => Some(Self::Mean),
            "cls" | "first" => Some(Self::Cls),
            "last" | "last_token" | "last-token" => Some(Self::LastToken),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct EmbeddingProfile {
    name: String,
    model_id: String,
    max_tokens: usize,
    pooling: EmbeddingPooling,
    query_prefix: String,
    document_prefix: String,
    layer_norm: bool,
    truncate_dim: Option<usize>,
    expected_dim: Option<usize>,
}

impl EmbeddingProfile {
    fn from_env() -> Result<Self> {
        let name = std::env::var(EMBEDDING_PROFILE_ENV)
            .unwrap_or_else(|_| "bge-small-en-v1.5".to_string())
            .trim()
            .to_ascii_lowercase();

        let mut profile = match name.as_str() {
            "" | "minilm" | "all-minilm-l6-v2" => Self {
                name: "minilm".to_string(),
                model_id: "sentence-transformers/all-MiniLM-L6-v2-local".to_string(),
                max_tokens: 256,
                pooling: EmbeddingPooling::Mean,
                query_prefix: String::new(),
                document_prefix: String::new(),
                layer_norm: false,
                truncate_dim: None,
                expected_dim: Some(384),
            },
            "bge-small" | "bge-small-en-v1.5" => Self {
                name: "bge-small-en-v1.5".to_string(),
                model_id: "BAAI/bge-small-en-v1.5-local".to_string(),
                max_tokens: 512,
                pooling: EmbeddingPooling::Cls,
                query_prefix: "Represent this sentence for searching relevant passages: "
                    .to_string(),
                document_prefix: String::new(),
                layer_norm: false,
                truncate_dim: None,
                expected_dim: Some(384),
            },
            "bge-base" | "bge-base-en-v1.5" => Self {
                name: "bge-base-en-v1.5".to_string(),
                model_id: "BAAI/bge-base-en-v1.5-local".to_string(),
                max_tokens: 512,
                pooling: EmbeddingPooling::Cls,
                query_prefix: "Represent this sentence for searching relevant passages: "
                    .to_string(),
                document_prefix: String::new(),
                layer_norm: false,
                truncate_dim: None,
                expected_dim: Some(768),
            },
            "qwen" | "qwen3" | "qwen3-embedding-0.6b" => Self {
                name: "qwen3-embedding-0.6b".to_string(),
                model_id: "Qwen/Qwen3-Embedding-0.6B-local".to_string(),
                max_tokens: 32_768,
                pooling: EmbeddingPooling::LastToken,
                query_prefix:
                    "Instruct: Retrieve relevant code symbols and implementation details\nQuery: "
                        .to_string(),
                document_prefix: String::new(),
                layer_norm: false,
                truncate_dim: None,
                expected_dim: Some(1024),
            },
            "embeddinggemma" | "embeddinggemma-300m" | "gemma" | "gemma-embedding-300m" => Self {
                name: "embeddinggemma-300m".to_string(),
                model_id: "google/embeddinggemma-300m-local".to_string(),
                max_tokens: 2048,
                pooling: EmbeddingPooling::Mean,
                query_prefix: "task: search result | query: ".to_string(),
                document_prefix: "title: none | text: ".to_string(),
                layer_norm: false,
                truncate_dim: None,
                expected_dim: Some(768),
            },
            "nomic" | "nomic-v1.5" | "nomic-embed-text-v1.5" => Self {
                name: "nomic-embed-text-v1.5".to_string(),
                model_id: "nomic-ai/nomic-embed-text-v1.5-local".to_string(),
                max_tokens: 8192,
                pooling: EmbeddingPooling::Mean,
                query_prefix: "search_query: ".to_string(),
                document_prefix: "search_document: ".to_string(),
                layer_norm: true,
                truncate_dim: None,
                expected_dim: Some(768),
            },
            "nomic-v2" | "nomic-embed-text-v2" | "nomic-embed-text-v2-moe" => Self {
                name: "nomic-embed-text-v2-moe".to_string(),
                model_id: "nomic-ai/nomic-embed-text-v2-moe-local".to_string(),
                max_tokens: 512,
                pooling: EmbeddingPooling::Mean,
                query_prefix: "search_query: ".to_string(),
                document_prefix: "search_document: ".to_string(),
                layer_norm: true,
                truncate_dim: None,
                expected_dim: Some(768),
            },
            "custom" | "custom-onnx" => Self {
                name: "custom-onnx".to_string(),
                model_id: "custom-onnx-local".to_string(),
                max_tokens: 256,
                pooling: EmbeddingPooling::Mean,
                query_prefix: String::new(),
                document_prefix: String::new(),
                layer_norm: false,
                truncate_dim: None,
                expected_dim: None,
            },
            other => {
                return Err(anyhow!(
                    "unsupported embedding profile `{other}` (set {EMBEDDING_PROFILE_ENV}=minilm, bge-small-en-v1.5, bge-base-en-v1.5, qwen3-embedding-0.6b, embeddinggemma-300m, nomic-embed-text-v1.5, nomic-embed-text-v2-moe, or custom-onnx)"
                ));
            }
        };

        if let Ok(model_id) = std::env::var(EMBEDDING_MODEL_ID_ENV)
            && !model_id.trim().is_empty()
        {
            profile.model_id = model_id;
        }
        if let Some(max_tokens) = env_usize(EMBEDDING_MAX_TOKENS_ENV, 8, 32_768) {
            profile.max_tokens = max_tokens;
        }
        if let Ok(raw) = std::env::var(EMBEDDING_POOLING_ENV) {
            profile.pooling = EmbeddingPooling::from_value(&raw)
                .ok_or_else(|| anyhow!("unsupported {EMBEDDING_POOLING_ENV} value `{raw}`"))?;
        }
        if let Ok(prefix) = std::env::var(EMBEDDING_QUERY_PREFIX_ENV) {
            profile.query_prefix = prefix;
        }
        if let Ok(prefix) = std::env::var(EMBEDDING_DOCUMENT_PREFIX_ENV) {
            profile.document_prefix = prefix;
        }
        if let Some(layer_norm) = env_bool_override(EMBEDDING_LAYER_NORM_ENV) {
            profile.layer_norm = layer_norm;
        }
        if let Some(truncate_dim) = env_usize(EMBEDDING_TRUNCATE_DIM_ENV, 1, 8192) {
            profile.truncate_dim = Some(truncate_dim);
            profile.expected_dim = Some(truncate_dim);
        }
        if let Some(expected_dim) = env_usize(EMBEDDING_EXPECTED_DIM_ENV, 1, 8192) {
            profile.expected_dim = Some(expected_dim);
        }

        Ok(profile)
    }

    fn cache_model_id(&self, backend: EmbeddingBackendSelection) -> String {
        if backend == EmbeddingBackendSelection::HashProjection {
            return self.model_id.clone();
        }

        if self.name == "minilm"
            && backend == EmbeddingBackendSelection::Onnx
            && self.query_prefix.is_empty()
            && self.document_prefix.is_empty()
            && !self.layer_norm
            && self.truncate_dim.is_none()
            && self.expected_dim == Some(384)
        {
            return self.model_id.clone();
        }

        format!(
            "{}|backend={}|pool={:?}|query_prefix={}|document_prefix={}|layer_norm={}|truncate_dim={:?}|expected_dim={:?}",
            self.model_id,
            backend.as_str(),
            self.pooling,
            self.query_prefix,
            self.document_prefix,
            self.layer_norm,
            self.truncate_dim,
            self.expected_dim
        )
    }
}

pub fn embedding_runtime_availability_from_env() -> EmbeddingRuntimeAvailability {
    let profile = match EmbeddingProfile::from_env() {
        Ok(profile) => profile,
        Err(error) => {
            return EmbeddingRuntimeAvailability {
                available: false,
                model_id: None,
                fallback_message: Some(error.to_string()),
            };
        }
    };
    let backend = match EmbeddingBackendSelection::from_env() {
        Ok(backend) => backend,
        Err(error) => {
            return EmbeddingRuntimeAvailability {
                available: false,
                model_id: Some(profile.model_id.clone()),
                fallback_message: Some(error.to_string()),
            };
        }
    };
    let model_id = profile.cache_model_id(backend);

    if backend == EmbeddingBackendSelection::HashProjection {
        return EmbeddingRuntimeAvailability {
            available: true,
            model_id: Some(model_id),
            fallback_message: None,
        };
    }

    if backend == EmbeddingBackendSelection::LlamaCpp {
        if let Err(error) = LlamaCppEndpoint::from_env() {
            return EmbeddingRuntimeAvailability {
                available: false,
                model_id: Some(model_id),
                fallback_message: Some(error.to_string()),
            };
        }
        return EmbeddingRuntimeAvailability {
            available: true,
            model_id: Some(model_id),
            fallback_message: None,
        };
    }

    let model_path = if let Ok(path) = std::env::var(EMBEDDING_MODEL_ENV) {
        PathBuf::from(path)
    } else if let Some(path) = resolve_bundled_embed_model_path() {
        path
    } else {
        return EmbeddingRuntimeAvailability {
            available: false,
            model_id: Some(model_id),
            fallback_message: Some(format!(
                "Missing {EMBEDDING_MODEL_ENV}. Configure a local embedding model artifact path or place a bundled model at {DEFAULT_BUNDLED_EMBED_MODEL_PATH}."
            )),
        };
    };

    if !model_path.is_file() {
        return EmbeddingRuntimeAvailability {
            available: false,
            model_id: Some(model_id),
            fallback_message: Some(format!(
                "embedding model artifact not found at {}",
                model_path.display()
            )),
        };
    }

    let tokenizer_path = std::env::var(EMBEDDING_TOKENIZER_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_tokenizer_path_for_model(&model_path));
    if !tokenizer_path.is_file() {
        return EmbeddingRuntimeAvailability {
            available: false,
            model_id: Some(model_id),
            fallback_message: Some(format!(
                "embedding tokenizer not found at {} (set {EMBEDDING_TOKENIZER_ENV})",
                tokenizer_path.display()
            )),
        };
    }

    EmbeddingRuntimeAvailability {
        available: true,
        model_id: Some(model_id),
        fallback_message: None,
    }
}

#[derive(Debug, Clone)]
pub struct LlmSearchDoc {
    pub node_id: NodeId,
    pub doc_text: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeProbe {
    pub model_path: PathBuf,
    pub model_id: String,
}

#[derive(Debug, Clone)]
pub struct HybridSearchHit {
    pub node_id: NodeId,
    pub lexical_score: f32,
    pub semantic_score: f32,
    pub graph_score: f32,
    pub total_score: f32,
}

#[derive(Debug, Clone)]
pub struct HybridSearchConfig {
    pub max_results: usize,
    pub lexical_weight: f32,
    pub semantic_weight: f32,
    pub graph_weight: f32,
    pub lexical_limit: usize,
    pub semantic_limit: usize,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            max_results: 20,
            lexical_weight: 0.35,
            semantic_weight: 0.55,
            graph_weight: 0.10,
            lexical_limit: 80,
            semantic_limit: 120,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntime {
    model_path: PathBuf,
    model_id: String,
    profile: EmbeddingProfile,
    backend: EmbeddingBackend,
}

#[derive(Debug, Clone)]
enum EmbeddingBackend {
    Onnx(Arc<OnnxEmbeddingRuntime>),
    LlamaCpp(Arc<LlamaCppEmbeddingRuntime>),
    HashProjection,
}

#[derive(Debug)]
struct OnnxEmbeddingRuntime {
    workers: Vec<OnnxEmbeddingWorker>,
}

#[derive(Debug)]
struct OnnxEmbeddingWorker {
    session: Mutex<Session>,
    tokenizer: Mutex<Tokenizer>,
    max_tokens: usize,
    pooling: EmbeddingPooling,
    accepts_token_type_ids: bool,
}

#[derive(Debug, Clone)]
struct LlamaCppEndpoint {
    host: String,
    port: u16,
    path: String,
}

impl LlamaCppEndpoint {
    fn from_env() -> Result<Self> {
        let raw = std::env::var(LLAMACPP_EMBEDDINGS_URL_ENV)
            .unwrap_or_else(|_| DEFAULT_LLAMACPP_EMBEDDINGS_URL.to_string());
        Self::parse(&raw)
    }

    fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        let rest = trimmed
            .strip_prefix("http://")
            .ok_or_else(|| anyhow!("{LLAMACPP_EMBEDDINGS_URL_ENV} must be an http:// URL"))?;
        let (authority, path) = rest
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((rest, "/v1/embeddings".to_string()));
        let (host, port) = if let Some((host, raw_port)) = authority.rsplit_once(':') {
            let port = raw_port
                .parse::<u16>()
                .with_context(|| format!("invalid port in {LLAMACPP_EMBEDDINGS_URL_ENV}"))?;
            (host.to_string(), port)
        } else {
            (authority.to_string(), 80)
        };
        if host.trim().is_empty() {
            return Err(anyhow!("{LLAMACPP_EMBEDDINGS_URL_ENV} must include a host"));
        }
        Ok(Self { host, port, path })
    }

    fn url(&self) -> String {
        format!("http://{}:{}{}", self.host, self.port, self.path)
    }
}

#[derive(Debug)]
struct LlamaCppEmbeddingRuntime {
    endpoint: LlamaCppEndpoint,
    request_count: usize,
}

impl LlamaCppEmbeddingRuntime {
    fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if self.request_count > 1 && texts.len() > 1 {
            let chunk_size = embedding_parallel_chunk_size(texts.len(), self.request_count);
            let chunks = texts
                .par_chunks(chunk_size)
                .map(|chunk| self.embed_texts_serial(chunk))
                .collect::<Result<Vec<_>>>()?;
            return Ok(chunks.into_iter().flatten().collect());
        }
        self.embed_texts_serial(texts)
    }

    fn embed_texts_serial(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let request = json!({
            "input": texts,
            "model": "codestory-local-embedding"
        });
        let response = post_json_to_http_endpoint(&self.endpoint, &request)?;
        parse_openai_embeddings_response(response, texts.len())
    }
}

fn post_json_to_http_endpoint(
    endpoint: &LlamaCppEndpoint,
    request: &JsonValue,
) -> Result<JsonValue> {
    let body = serde_json::to_vec(request).context("failed to serialize llama.cpp request")?;
    let mut stream =
        TcpStream::connect((endpoint.host.as_str(), endpoint.port)).with_context(|| {
            format!(
                "failed to connect to llama.cpp embeddings endpoint {}",
                endpoint.url()
            )
        })?;
    stream.set_read_timeout(Some(Duration::from_secs(300)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        endpoint.path,
        endpoint.host,
        endpoint.port,
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let (status_code, headers, body) = split_http_response(&response)?;
    if !(200..300).contains(&status_code) {
        return Err(anyhow!(
            "llama.cpp embeddings endpoint {} returned HTTP {status_code}: {}",
            endpoint.url(),
            String::from_utf8_lossy(&body)
        ));
    }

    let body = if headers
        .iter()
        .any(|(key, value)| key == "transfer-encoding" && value.contains("chunked"))
    {
        decode_chunked_http_body(&body)?
    } else {
        body
    };

    serde_json::from_slice(&body).with_context(|| {
        format!(
            "failed to parse JSON response from llama.cpp endpoint {}",
            endpoint.url()
        )
    })
}

fn split_http_response(response: &[u8]) -> Result<(u16, Vec<(String, String)>, Vec<u8>)> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response from llama.cpp endpoint"))?;
    let header_text = String::from_utf8_lossy(&response[..header_end]);
    let mut lines = header_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing HTTP status line from llama.cpp endpoint"))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("missing HTTP status code from llama.cpp endpoint"))?
        .parse::<u16>()
        .context("invalid HTTP status code from llama.cpp endpoint")?;
    let headers = lines
        .filter_map(|line| {
            line.split_once(':').map(|(key, value)| {
                (
                    key.trim().to_ascii_lowercase(),
                    value.trim().to_ascii_lowercase(),
                )
            })
        })
        .collect::<Vec<_>>();
    Ok((status_code, headers, response[header_end + 4..].to_vec()))
}

fn decode_chunked_http_body(body: &[u8]) -> Result<Vec<u8>> {
    let mut offset = 0;
    let mut decoded = Vec::new();
    while offset < body.len() {
        let line_end = body[offset..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| anyhow!("invalid chunked response from llama.cpp endpoint"))?
            + offset;
        let size_text = String::from_utf8_lossy(&body[offset..line_end]);
        let size_hex = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16)
            .context("invalid chunk size from llama.cpp endpoint")?;
        offset = line_end + 2;
        if size == 0 {
            break;
        }
        if offset + size > body.len() {
            return Err(anyhow!(
                "truncated chunked response from llama.cpp endpoint"
            ));
        }
        decoded.extend_from_slice(&body[offset..offset + size]);
        offset += size + 2;
    }
    Ok(decoded)
}

fn parse_openai_embeddings_response(
    response: JsonValue,
    expected_count: usize,
) -> Result<Vec<Vec<f32>>> {
    let data = response
        .get("data")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| anyhow!("llama.cpp embeddings response missing `data` array"))?;
    if data.len() != expected_count {
        return Err(anyhow!(
            "llama.cpp embeddings response returned {} vectors for {} inputs",
            data.len(),
            expected_count
        ));
    }

    let mut indexed = Vec::with_capacity(data.len());
    for (fallback_index, item) in data.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(JsonValue::as_u64)
            .map(|value| value as usize)
            .unwrap_or(fallback_index);
        let embedding = item
            .get("embedding")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| anyhow!("llama.cpp embeddings response item missing `embedding`"))?
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .map(|number| number as f32)
                    .ok_or_else(|| anyhow!("llama.cpp embedding contained a non-numeric value"))
            })
            .collect::<Result<Vec<_>>>()?;
        indexed.push((index, embedding));
    }
    indexed.sort_by_key(|(index, _)| *index);
    Ok(indexed
        .into_iter()
        .map(|(_, embedding)| embedding)
        .collect())
}

impl EmbeddingRuntime {
    fn probe_model_artifact<P: AsRef<Path>>(
        path: P,
        model_id: impl Into<String>,
    ) -> Result<EmbeddingRuntimeProbe> {
        let model_path = path.as_ref().to_path_buf();
        if !model_path.is_file() {
            return Err(anyhow!(
                "embedding model artifact not found at {}",
                model_path.display()
            ));
        }
        let tokenizer_path = std::env::var(EMBEDDING_TOKENIZER_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_tokenizer_path_for_model(&model_path));
        if !tokenizer_path.is_file() {
            return Err(anyhow!(
                "embedding tokenizer not found at {} (set {EMBEDDING_TOKENIZER_ENV})",
                tokenizer_path.display()
            ));
        }

        Ok(EmbeddingRuntimeProbe {
            model_path,
            model_id: model_id.into(),
        })
    }

    fn ensure_onnx_initialized() -> Result<()> {
        static INIT: OnceLock<()> = OnceLock::new();
        let _ = INIT.get_or_init(|| {
            // `commit()` returns false when another global environment is already configured.
            let _ = ort::init().with_name("codestory-embeddings").commit();
        });
        Ok(())
    }

    pub fn from_model_artifact<P: AsRef<Path>>(
        path: P,
        model_id: impl Into<String>,
    ) -> Result<Self> {
        let mut profile = EmbeddingProfile::from_env()?;
        profile.model_id = model_id.into();
        Self::from_onnx_model_artifact(path, profile)
    }

    fn from_onnx_model_artifact<P: AsRef<Path>>(
        path: P,
        profile: EmbeddingProfile,
    ) -> Result<Self> {
        Self::ensure_onnx_initialized()?;
        let model_path = path.as_ref().to_path_buf();
        let model_id = profile.cache_model_id(EmbeddingBackendSelection::Onnx);
        let probe = Self::probe_model_artifact(&model_path, model_id)?;
        let tokenizer_path = std::env::var(EMBEDDING_TOKENIZER_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_tokenizer_path_for_model(&model_path));

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|error| anyhow!("failed to load tokenizer: {error}"))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: profile.max_tokens,
                ..Default::default()
            }))
            .map_err(|error| anyhow!("failed to configure tokenizer truncation: {error}"))?;
        tokenizer.with_padding(Some(PaddingParams::default()));

        let session_count = embedding_session_count_from_env();
        let prepacked_weights = PrepackedWeights::new();
        let mut workers = Vec::with_capacity(session_count);
        for _ in 0..session_count {
            let session = build_onnx_embedding_session(&model_path, Some(&prepacked_weights))?;
            let accepts_token_type_ids = session
                .inputs()
                .iter()
                .any(|input| input.name() == "token_type_ids");
            workers.push(OnnxEmbeddingWorker {
                session: Mutex::new(session),
                tokenizer: Mutex::new(tokenizer.clone()),
                max_tokens: profile.max_tokens,
                pooling: profile.pooling,
                accepts_token_type_ids,
            });
        }

        Ok(Self {
            model_path,
            model_id: probe.model_id,
            profile,
            backend: EmbeddingBackend::Onnx(Arc::new(OnnxEmbeddingRuntime { workers })),
        })
    }

    pub fn probe_from_env() -> Result<EmbeddingRuntimeProbe> {
        let profile = EmbeddingProfile::from_env()?;
        let backend = EmbeddingBackendSelection::from_env()?;
        let model_id = profile.cache_model_id(backend);

        if backend == EmbeddingBackendSelection::HashProjection {
            return Ok(EmbeddingRuntimeProbe {
                model_path: PathBuf::from("hash-projection"),
                model_id,
            });
        }

        if backend == EmbeddingBackendSelection::LlamaCpp {
            let endpoint = LlamaCppEndpoint::from_env()?;
            return Ok(EmbeddingRuntimeProbe {
                model_path: PathBuf::from(endpoint.url()),
                model_id,
            });
        }

        if let Ok(path) = std::env::var(EMBEDDING_MODEL_ENV) {
            return Self::probe_model_artifact(path, model_id);
        }

        if let Some(bundled_path) = resolve_bundled_embed_model_path() {
            return Self::probe_model_artifact(bundled_path, model_id);
        }

        Err(anyhow!(
            "Missing {EMBEDDING_MODEL_ENV}. Configure a local embedding model artifact path or place a bundled model at {DEFAULT_BUNDLED_EMBED_MODEL_PATH}."
        ))
    }

    pub fn from_env() -> Result<Self> {
        let profile = EmbeddingProfile::from_env()?;
        let backend = EmbeddingBackendSelection::from_env()?;
        let model_id = profile.cache_model_id(backend);

        match backend {
            EmbeddingBackendSelection::HashProjection => {
                return Ok(Self {
                    model_path: PathBuf::from("hash-projection"),
                    model_id,
                    profile,
                    backend: EmbeddingBackend::HashProjection,
                });
            }
            EmbeddingBackendSelection::LlamaCpp => {
                let endpoint = LlamaCppEndpoint::from_env()?;
                return Ok(Self {
                    model_path: PathBuf::from(endpoint.url()),
                    model_id,
                    profile,
                    backend: EmbeddingBackend::LlamaCpp(Arc::new(LlamaCppEmbeddingRuntime {
                        endpoint,
                        request_count: env_usize(LLAMACPP_REQUEST_COUNT_ENV, 1, 16).unwrap_or(1),
                    })),
                });
            }
            EmbeddingBackendSelection::Onnx => {}
        }

        if let Ok(path) = std::env::var(EMBEDDING_MODEL_ENV) {
            return Self::from_onnx_model_artifact(path, profile);
        }

        if let Some(bundled_path) = resolve_bundled_embed_model_path() {
            return Self::from_onnx_model_artifact(bundled_path, profile);
        }

        Err(anyhow!(
            "Missing {EMBEDDING_MODEL_ENV}. Configure a local embedding model artifact path or place a bundled model at {DEFAULT_BUNDLED_EMBED_MODEL_PATH}."
        ))
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        if query.trim().is_empty() {
            return Err(anyhow!("query cannot be empty for semantic retrieval"));
        }
        let prepared = format!("{}{}", self.profile.query_prefix, query);
        let mut vectors = self.embed_prepared_texts(&[prepared])?;
        vectors
            .pop()
            .ok_or_else(|| anyhow!("embedding runtime returned no query embedding"))
    }

    pub fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prepared = texts
            .iter()
            .map(|text| format!("{}{}", self.profile.document_prefix, text))
            .collect::<Vec<_>>();
        self.embed_prepared_texts(&prepared)
    }

    fn embed_prepared_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut embeddings = match &self.backend {
            EmbeddingBackend::HashProjection => {
                let mut out = Vec::with_capacity(texts.len());
                for text in texts {
                    if text.trim().is_empty() {
                        out.push(vec![
                            0.0;
                            self.profile.expected_dim.unwrap_or(EMBEDDING_DIM)
                        ]);
                    } else {
                        out.push(embed_text_with_hash_projection(
                            text,
                            self.profile.expected_dim.unwrap_or(EMBEDDING_DIM),
                        ));
                    }
                }
                Ok(out)
            }
            EmbeddingBackend::Onnx(runtime) => runtime.embed_texts(texts),
            EmbeddingBackend::LlamaCpp(runtime) => runtime.embed_texts(texts),
        }?;
        postprocess_embeddings(&mut embeddings, &self.profile)?;
        Ok(embeddings)
    }

    #[cfg(test)]
    pub fn test_runtime() -> Self {
        let profile = EmbeddingProfile {
            name: "test".to_string(),
            model_id: "test-model".to_string(),
            max_tokens: 256,
            pooling: EmbeddingPooling::Mean,
            query_prefix: String::new(),
            document_prefix: String::new(),
            layer_norm: false,
            truncate_dim: None,
            expected_dim: Some(EMBEDDING_DIM),
        };
        Self {
            model_path: PathBuf::from("test-model.onnx"),
            model_id: "test-model".to_string(),
            profile,
            backend: EmbeddingBackend::HashProjection,
        }
    }
}

fn build_onnx_embedding_session(
    model_path: &Path,
    prepacked_weights: Option<&PrepackedWeights>,
) -> Result<Session> {
    let mut builder = Session::builder()
        .context("failed to create ONNX session builder")?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .context("failed to configure ONNX graph optimization")?;
    if let Some(threads) = env_usize(EMBEDDING_INTRA_THREADS_ENV, 1, 128) {
        builder = builder
            .with_intra_threads(threads)
            .context("failed to configure ONNX intra-op threads")?;
    }
    if let Some(threads) = env_usize(EMBEDDING_INTER_THREADS_ENV, 1, 128) {
        builder = builder
            .with_inter_threads(threads)
            .context("failed to configure ONNX inter-op threads")?;
    }
    if env_bool(EMBEDDING_PARALLEL_EXECUTION_ENV) {
        builder = builder
            .with_parallel_execution(true)
            .context("failed to configure ONNX parallel execution")?;
    }
    if let Some(prepacked_weights) = prepacked_weights {
        builder = builder
            .with_prepacked_weights(prepacked_weights)
            .context("failed to configure ONNX prepacked weights")?;
    }
    builder = configure_embedding_execution_provider(builder)?;

    builder
        .commit_from_file(model_path)
        .with_context(|| format!("failed to load ONNX model {}", model_path.display()))
}

impl OnnxEmbeddingRuntime {
    fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if self.workers.len() <= 1 || texts.len() <= 1 {
            return self.workers[0].embed_texts(texts);
        }

        let chunk_size = embedding_parallel_chunk_size(texts.len(), self.workers.len());
        let chunks = texts
            .par_chunks(chunk_size)
            .enumerate()
            .map(|(chunk_index, chunk)| {
                let worker = &self.workers[chunk_index % self.workers.len()];
                worker.embed_texts(chunk)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(chunks.into_iter().flatten().collect())
    }
}

impl OnnxEmbeddingWorker {
    fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let tokenizer = self
            .tokenizer
            .lock()
            .map_err(|_| anyhow!("embedding tokenizer lock poisoned"))?;
        let encodings = tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|error| anyhow!("failed to tokenize batch: {error}"))?;
        drop(tokenizer);

        let seq_len = encodings
            .iter()
            .map(|encoding| encoding.get_ids().len())
            .max()
            .unwrap_or(1)
            .clamp(1, self.max_tokens);

        let batch = encodings.len();
        let mut input_ids = Array2::<i64>::zeros((batch, seq_len));
        let mut attention_mask = Array2::<i64>::zeros((batch, seq_len));

        for (row, encoding) in encodings.iter().enumerate() {
            for (col, token_id) in encoding.get_ids().iter().take(seq_len).enumerate() {
                input_ids[[row, col]] = i64::from(*token_id);
            }
            for (col, mask) in encoding
                .get_attention_mask()
                .iter()
                .take(seq_len)
                .enumerate()
            {
                attention_mask[[row, col]] = i64::from(*mask);
            }
        }

        let mut initial_error_text = None;
        let mut embeddings = {
            let mut session = self
                .session
                .lock()
                .map_err(|_| anyhow!("embedding session lock poisoned"))?;
            let input_ids_ref = TensorRef::from_array_view(&input_ids)
                .context("failed to build ONNX input_ids tensor")?;
            let attention_mask_ref = TensorRef::from_array_view(&attention_mask)
                .context("failed to build ONNX attention_mask tensor")?;
            let run_result = if self.accepts_token_type_ids {
                let token_type_ids = Array2::<i64>::zeros((batch, seq_len));
                let token_type_ids_ref = TensorRef::from_array_view(&token_type_ids)
                    .context("failed to build ONNX token_type_ids tensor")?;
                session.run(ort::inputs![
                    "input_ids" => input_ids_ref,
                    "attention_mask" => attention_mask_ref,
                    "token_type_ids" => token_type_ids_ref
                ])
            } else {
                session.run(ort::inputs![
                    "input_ids" => input_ids_ref,
                    "attention_mask" => attention_mask_ref
                ])
            };
            match run_result {
                Ok(outputs) => {
                    extract_onnx_embeddings(outputs, attention_mask.view(), self.pooling)?
                }
                Err(error) => {
                    initial_error_text = Some(error.to_string());
                    Vec::new()
                }
            }
        };

        if let Some(error_text) = initial_error_text {
            let needs_two_input_fallback = self.accepts_token_type_ids
                && error_text.contains("token_type_ids")
                && (error_text.contains("Invalid input name")
                    || error_text.contains("Invalid Feed Input Name")
                    || error_text.contains("Unknown input"));
            if !needs_two_input_fallback {
                return Err(anyhow!("ONNX embedding inference failed: {error_text}"));
            }

            let mut session = self
                .session
                .lock()
                .map_err(|_| anyhow!("embedding session lock poisoned"))?;
            let input_ids_ref = TensorRef::from_array_view(&input_ids)
                .context("failed to rebuild ONNX input_ids tensor")?;
            let attention_mask_ref = TensorRef::from_array_view(&attention_mask)
                .context("failed to rebuild ONNX attention_mask tensor")?;
            let outputs = session
                .run(ort::inputs![
                    "input_ids" => input_ids_ref,
                    "attention_mask" => attention_mask_ref
                ])
                .with_context(|| {
                    format!(
                        "ONNX embedding inference failed while retrying without token_type_ids ({error_text})"
                    )
                })?;
            embeddings = extract_onnx_embeddings(outputs, attention_mask.view(), self.pooling)?;
        }

        Ok(embeddings)
    }
}

fn extract_onnx_embeddings(
    outputs: ort::session::SessionOutputs<'_>,
    attention_mask: ArrayView2<'_, i64>,
    pooling: EmbeddingPooling,
) -> Result<Vec<Vec<f32>>> {
    let (name, output) = outputs
        .iter()
        .next()
        .ok_or_else(|| anyhow!("ONNX embedding model produced no outputs"))?;
    let output_array = output
        .try_extract_array::<f32>()
        .with_context(|| format!("failed to extract ONNX output tensor `{name}`"))?;

    match output_array.ndim() {
        2 => Ok(rows_to_vecs(
            output_array.view().into_dimensionality::<ndarray::Ix2>()?,
        )),
        3 => Ok(pool_last_hidden(
            output_array.view().into_dimensionality::<ndarray::Ix3>()?,
            attention_mask,
            pooling,
        )),
        n => Err(anyhow!(
            "unsupported ONNX embedding output rank {n}; expected 2 or 3"
        )),
    }
}

fn rows_to_vecs(values: ArrayView2<'_, f32>) -> Vec<Vec<f32>> {
    values
        .axis_iter(Axis(0))
        .map(|row| row.iter().copied().collect::<Vec<_>>())
        .collect()
}

fn pool_last_hidden(
    values: ArrayView3<'_, f32>,
    attention_mask: ArrayView2<'_, i64>,
    pooling: EmbeddingPooling,
) -> Vec<Vec<f32>> {
    match pooling {
        EmbeddingPooling::Mean => mean_pool_last_hidden(values, attention_mask),
        EmbeddingPooling::Cls => values
            .axis_iter(Axis(0))
            .map(|token_matrix| {
                token_matrix
                    .index_axis(Axis(0), 0)
                    .iter()
                    .copied()
                    .collect()
            })
            .collect(),
        EmbeddingPooling::LastToken => last_token_pool_last_hidden(values, attention_mask),
    }
}

fn mean_pool_last_hidden(
    values: ArrayView3<'_, f32>,
    attention_mask: ArrayView2<'_, i64>,
) -> Vec<Vec<f32>> {
    let mut pooled = Vec::with_capacity(values.len_of(Axis(0)));
    for (batch_index, token_matrix) in values.axis_iter(Axis(0)).enumerate() {
        let hidden_size = token_matrix.len_of(Axis(1));
        let mut sum = vec![0.0_f32; hidden_size];
        let mut denom = 0.0_f32;

        for (token_index, token_vec) in token_matrix.axis_iter(Axis(0)).enumerate() {
            if attention_mask
                .get((batch_index, token_index))
                .copied()
                .unwrap_or(0)
                <= 0
            {
                continue;
            }
            denom += 1.0;
            for (idx, value) in token_vec.iter().copied().enumerate() {
                sum[idx] += value;
            }
        }

        if denom > 0.0 {
            for value in &mut sum {
                *value /= denom;
            }
        }
        pooled.push(sum);
    }
    pooled
}

fn last_token_pool_last_hidden(
    values: ArrayView3<'_, f32>,
    attention_mask: ArrayView2<'_, i64>,
) -> Vec<Vec<f32>> {
    values
        .axis_iter(Axis(0))
        .enumerate()
        .map(|(batch_index, token_matrix)| {
            let token_count = token_matrix.len_of(Axis(0));
            let last_index = (0..token_count)
                .rev()
                .find(|token_index| {
                    attention_mask
                        .get((batch_index, *token_index))
                        .copied()
                        .unwrap_or(0)
                        > 0
                })
                .unwrap_or(0);
            token_matrix
                .index_axis(Axis(0), last_index)
                .iter()
                .copied()
                .collect()
        })
        .collect()
}

pub struct SearchEngine {
    matcher: Matcher,
    symbols: Vec<(Utf32String, NodeId)>,
    index: Index,
    reader: IndexReader,
    llm_docs: HashMap<NodeId, LlmSearchDoc>,
    embedding_runtime: Option<EmbeddingRuntime>,
}

impl SearchEngine {
    fn build_schema() -> Schema {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        schema_builder.add_i64_field("node_id", INDEXED | STORED | FAST);
        schema_builder.build()
    }

    fn new_with_index(index: Index) -> Result<Self> {
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(Self {
            matcher: Matcher::new(NucleoConfig::DEFAULT),
            symbols: Vec::new(),
            index,
            reader,
            llm_docs: HashMap::new(),
            embedding_runtime: None,
        })
    }

    pub fn new(storage_path: Option<&Path>) -> Result<Self> {
        let schema = Self::build_schema();
        let index = if let Some(path) = storage_path {
            recreate_search_storage_dir(path)?;
            Index::create_in_dir(path, schema.clone())
                .with_context(|| format!("Failed to create tantivy index at {}", path.display()))?
        } else {
            Index::create_in_ram(schema)
        };
        Self::new_with_index(index)
    }

    pub fn open_existing(path: &Path) -> Result<Self> {
        let index = Index::open_in_dir(path)
            .with_context(|| format!("Failed to open tantivy index at {}", path.display()))?;
        Self::new_with_index(index)
    }

    #[cfg(test)]
    pub fn set_embedding_runtime(&mut self, runtime: EmbeddingRuntime) {
        self.embedding_runtime = Some(runtime);
    }

    pub fn set_embedding_runtime_from_env(&mut self) -> Result<()> {
        let runtime = EmbeddingRuntime::from_env()?;
        self.embedding_runtime = Some(runtime);
        Ok(())
    }

    pub fn embedding_model_id(&self) -> Option<&str> {
        self.embedding_runtime
            .as_ref()
            .map(EmbeddingRuntime::model_id)
    }

    pub fn embedding_runtime_configured(&self) -> bool {
        self.embedding_runtime.is_some()
    }

    pub fn full_text_doc_count(&self) -> usize {
        self.reader.searcher().num_docs() as usize
    }

    pub fn semantic_index_ready(&self) -> bool {
        self.embedding_runtime.is_some() && !self.llm_docs.is_empty()
    }

    pub fn semantic_doc_count(&self) -> u32 {
        self.llm_docs.len().min(u32::MAX as usize) as u32
    }

    pub fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let runtime = self
            .embedding_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("embedding runtime is not configured"))?;
        runtime.embed_texts(texts)
    }

    pub fn index_llm_symbol_docs(&mut self, docs: Vec<LlmSearchDoc>) {
        self.llm_docs.clear();
        for doc in docs {
            self.llm_docs.insert(doc.node_id, doc);
        }
    }

    pub fn clear_llm_symbol_docs(&mut self) {
        self.llm_docs.clear();
    }

    pub fn extend_llm_symbol_docs<I>(&mut self, docs: I)
    where
        I: IntoIterator<Item = LlmSearchDoc>,
    {
        for doc in docs {
            self.llm_docs.insert(doc.node_id, doc);
        }
    }

    pub fn index_nodes(&mut self, nodes: Vec<(NodeId, String)>) -> Result<()> {
        let mut index_writer: IndexWriter<TantivyDocument> =
            self.index.writer(SEARCH_WRITER_HEAP_BYTES)?;
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;

        for (id, name) in nodes {
            self.symbols.push((Utf32String::from(name.as_str()), id));
            index_writer.add_document(doc!(
                name_field => name,
                id_field => id.0
            ))?;
        }

        index_writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn load_symbol_projection<I>(&mut self, symbols: I)
    where
        I: IntoIterator<Item = (NodeId, String)>,
    {
        self.symbols.clear();
        self.symbols.extend(
            symbols
                .into_iter()
                .map(|(id, name)| (Utf32String::from(name.as_str()), id)),
        );
    }

    #[cfg(test)]
    pub fn search_symbol(&mut self, query: &str) -> Vec<NodeId> {
        if query.is_empty() {
            return Vec::new();
        }
        self.search_symbol_with_scores(query)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    pub fn search_symbol_with_scores(&mut self, query: &str) -> Vec<(NodeId, f32)> {
        if query.is_empty() {
            return Vec::new();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut matches = Vec::new();

        for (name, id) in &self.symbols {
            if let Some(score) = pattern.score(name.slice(..), &mut self.matcher) {
                matches.push((*id, score));
            }
        }

        matches.sort_by_key(|b| std::cmp::Reverse(b.1));

        let mut seen = HashSet::new();
        matches
            .into_iter()
            .map(|(id, score)| (id, score as f32))
            .filter(|(id, _)| seen.insert(*id))
            .take(200)
            .collect()
    }

    pub fn search_hybrid_with_scores(
        &mut self,
        query: &str,
        graph_boosts: &HashMap<NodeId, f32>,
        config: HybridSearchConfig,
    ) -> Result<Vec<HybridSearchHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        if !self.semantic_index_ready() {
            return Err(anyhow!(
                "semantic retrieval is required but embedding runtime or semantic index is unavailable"
            ));
        }

        let runtime = self
            .embedding_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("embedding runtime is not configured"))?;
        let query_embedding = runtime.embed_query(query)?;

        let lexical_matches = self.search_symbol_with_scores(query);
        let lexical_max = lexical_matches
            .iter()
            .map(|(_, score)| *score)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let lexical_map = lexical_matches
            .into_iter()
            .take(config.lexical_limit)
            .map(|(node_id, score)| (node_id, (score / lexical_max).clamp(0.0, 1.0)))
            .collect::<HashMap<_, _>>();

        let mut semantic_scored = self
            .llm_docs
            .values()
            .map(|doc| {
                let cosine = cosine_similarity(&query_embedding, &doc.embedding);
                (doc.node_id, ((cosine + 1.0) * 0.5).clamp(0.0, 1.0))
            })
            .collect::<Vec<_>>();
        semantic_scored.sort_by(|left, right| right.1.total_cmp(&left.1));

        let semantic_map = semantic_scored
            .into_iter()
            .take(config.semantic_limit)
            .collect::<HashMap<_, _>>();

        let mut candidate_ids = HashSet::new();
        candidate_ids.extend(lexical_map.keys().copied());
        candidate_ids.extend(semantic_map.keys().copied());
        candidate_ids.extend(graph_boosts.keys().copied());

        let lexical_weight = config.lexical_weight.clamp(0.0, 1.0);
        let semantic_weight = config.semantic_weight.clamp(0.0, 1.0);
        let graph_weight = config.graph_weight.clamp(0.0, 1.0);

        let mut hits = candidate_ids
            .into_iter()
            .map(|node_id| {
                let lexical_score = lexical_map.get(&node_id).copied().unwrap_or(0.0);
                let semantic_score = semantic_map.get(&node_id).copied().unwrap_or(0.0);
                let graph_score = graph_boosts
                    .get(&node_id)
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let total_score = lexical_weight * lexical_score
                    + semantic_weight * semantic_score
                    + graph_weight * graph_score;

                HybridSearchHit {
                    node_id,
                    lexical_score,
                    semantic_score,
                    graph_score,
                    total_score,
                }
            })
            .collect::<Vec<_>>();

        hits.sort_by(|left, right| {
            right
                .total_score
                .total_cmp(&left.total_score)
                .then_with(|| left.node_id.0.cmp(&right.node_id.0))
        });
        hits.truncate(config.max_results.max(1));

        Ok(hits)
    }

    #[cfg(test)]
    pub fn remove_nodes(&mut self, nodes: &[NodeId]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        let mut remove_ids = HashSet::new();
        for id in nodes {
            remove_ids.insert(id.0);
        }

        self.symbols.retain(|(_, id)| !remove_ids.contains(&id.0));
        self.llm_docs.retain(|id, _| !remove_ids.contains(&id.0));

        let mut index_writer: IndexWriter<TantivyDocument> =
            self.index.writer(SEARCH_WRITER_HEAP_BYTES)?;
        let schema = self.index.schema();
        let node_field = schema.get_field("node_id")?;
        for id in &remove_ids {
            index_writer.delete_term(tantivy::Term::from_field_i64(node_field, *id));
        }
        index_writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn search_full_text(&self, query_str: &str) -> Result<Vec<NodeId>> {
        if query_str.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;

        let query_parser = QueryParser::for_index(&self.index, vec![name_field]);
        let query = query_parser
            .parse_query(query_str)
            .context("Failed to parse tantivy query")?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(20))?;

        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for (_score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
            if let Some(id_val) = retrieved_doc.get_first(id_field).and_then(|v| v.as_i64()) {
                let id = NodeId(id_val);
                if seen.insert(id) {
                    results.push(id);
                }
            }
        }

        Ok(results)
    }
}

fn recreate_search_storage_dir(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Failed to clear search index dir {}", path.display()))?;
    }
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create search index dir {}", path.display()))?;
    Ok(())
}

fn embed_text_with_hash_projection(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dim];

    for token in text.split_whitespace() {
        let norm = token.trim().to_ascii_lowercase();
        if norm.is_empty() {
            continue;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        norm.hash(&mut hasher);
        let hash = hasher.finish();
        let index = (hash as usize) % dim;
        let sign = if ((hash >> 8) & 1) == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;

        // Add a tiny secondary feature for short-range context.
        let index2 = ((hash >> 17) as usize) % dim;
        vector[index2] += 0.5 * sign;
    }

    l2_normalize(&mut vector);
    vector
}

fn postprocess_embeddings(embeddings: &mut [Vec<f32>], profile: &EmbeddingProfile) -> Result<()> {
    for embedding in embeddings {
        if let Some(truncate_dim) = profile.truncate_dim {
            if embedding.len() < truncate_dim {
                return Err(anyhow!(
                    "embedding from profile `{}` has dimension {}, cannot truncate to {}",
                    profile.name,
                    embedding.len(),
                    truncate_dim
                ));
            }
            embedding.truncate(truncate_dim);
        }
        if let Some(expected_dim) = profile.expected_dim
            && embedding.len() != expected_dim
        {
            return Err(anyhow!(
                "embedding from profile `{}` has dimension {}, expected {}",
                profile.name,
                embedding.len(),
                expected_dim
            ));
        }
        if profile.layer_norm {
            layer_normalize(embedding);
        }
        l2_normalize(embedding);
    }
    Ok(())
}

fn layer_normalize(values: &mut [f32]) {
    if values.is_empty() {
        return;
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let variance = values
        .iter()
        .map(|value| {
            let centered = *value - mean;
            centered * centered
        })
        .sum::<f32>()
        / values.len() as f32;
    let denom = (variance + 1.0e-12).sqrt();
    for value in values {
        *value = (*value - mean) / denom;
    }
}

fn l2_normalize(values: &mut [f32]) {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return;
    }
    for value in values {
        *value /= norm;
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter()
        .zip(b.iter())
        .map(|(left, right)| left * right)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use tempfile::tempdir;

    static ENV_TEST_LOCK: StdMutex<()> = StdMutex::new(());

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
                if let Some(value) = self.previous.as_deref() {
                    std::env::set_var(self.key, value);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn embedding_session_count_env_override_is_clamped() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::set(EMBEDDING_SESSION_COUNT_ENV, "999");
        assert_eq!(embedding_session_count_from_env(), 16);

        let _guard = EnvGuard::set(EMBEDDING_SESSION_COUNT_ENV, "0");
        assert_eq!(embedding_session_count_from_env(), 1);

        let _guard = EnvGuard::set(EMBEDDING_SESSION_COUNT_ENV, "3");
        assert_eq!(embedding_session_count_from_env(), 3);
    }

    #[test]
    fn embedding_session_count_defaults_to_bounded_parallelism() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::remove(EMBEDDING_SESSION_COUNT_ENV);
        let expected = std::thread::available_parallelism()
            .map(|threads| threads.get().clamp(1, 2))
            .unwrap_or(1);

        assert_eq!(embedding_session_count_from_env(), expected);
    }

    #[test]
    fn embedding_profile_defaults_to_bge_small() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::remove(EMBEDDING_PROFILE_ENV);

        let profile = EmbeddingProfile::from_env()?;

        assert_eq!(profile.name, "bge-small-en-v1.5");
        assert_eq!(profile.model_id, "BAAI/bge-small-en-v1.5-local");
        assert_eq!(profile.expected_dim, Some(384));
        assert_eq!(
            DEFAULT_BUNDLED_EMBED_MODEL_PATH,
            "models/bge-small-en-v1.5/onnx/model.onnx"
        );
        Ok(())
    }

    #[test]
    fn embedding_parallel_chunk_size_spreads_batches_across_workers() {
        assert_eq!(embedding_parallel_chunk_size(64, 4), 16);
        assert_eq!(embedding_parallel_chunk_size(65, 4), 17);
        assert_eq!(embedding_parallel_chunk_size(7, 16), 1);
        assert_eq!(embedding_parallel_chunk_size(0, 4), 1);
    }

    fn run_one_fake_embedding_server(
        response_body: &'static str,
    ) -> Result<(String, thread::JoinHandle<String>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fake embedding request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            let mut expected_len = None;
            loop {
                let read = stream
                    .read(&mut buffer)
                    .expect("read fake embedding request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if expected_len.is_none()
                    && let Some(header_end) =
                        request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_len = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            if key.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    expected_len = Some(header_end + 4 + content_len);
                }
                if let Some(expected_len) = expected_len
                    && request.len() >= expected_len
                {
                    break;
                }
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fake embedding response");
            String::from_utf8_lossy(&request).to_string()
        });
        Ok((format!("http://{addr}/v1/embeddings"), handle))
    }

    #[test]
    fn llamacpp_backend_uses_openai_embedding_endpoint() -> Result<()> {
        let response = r#"{"data":[{"index":0,"embedding":[1.0,0.0,0.0]},{"index":1,"embedding":[0.0,2.0,0.0]}]}"#;
        let (url, handle) = run_one_fake_embedding_server(response)?;
        let profile = EmbeddingProfile {
            name: "custom-onnx".to_string(),
            model_id: "custom-onnx-local".to_string(),
            max_tokens: 256,
            pooling: EmbeddingPooling::Mean,
            query_prefix: String::new(),
            document_prefix: "doc: ".to_string(),
            layer_norm: false,
            truncate_dim: None,
            expected_dim: Some(3),
        };
        let runtime = EmbeddingRuntime {
            model_path: PathBuf::from(&url),
            model_id: profile.cache_model_id(EmbeddingBackendSelection::LlamaCpp),
            profile,
            backend: EmbeddingBackend::LlamaCpp(Arc::new(LlamaCppEmbeddingRuntime {
                endpoint: LlamaCppEndpoint::parse(&url)?,
                request_count: 1,
            })),
        };
        let embeddings = runtime.embed_texts(&["alpha".to_string(), "beta".to_string()])?;
        let request = handle.join().expect("fake embedding server should finish");

        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0], vec![1.0, 0.0, 0.0]);
        assert_eq!(embeddings[1], vec![0.0, 1.0, 0.0]);
        assert!(
            request.contains("doc: alpha") && request.contains("doc: beta"),
            "request did not include document prefixes: {request}"
        );
        assert_eq!(
            runtime.model_id(),
            "custom-onnx-local|backend=llamacpp|pool=Mean|query_prefix=|document_prefix=doc: |layer_norm=false|truncate_dim=None|expected_dim=Some(3)"
        );
        Ok(())
    }

    #[test]
    fn llamacpp_endpoint_parse_accepts_openai_embeddings_url() -> Result<()> {
        let endpoint = LlamaCppEndpoint::parse("http://127.0.0.1:8080/v1/embeddings")?;

        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.path, "/v1/embeddings");
        Ok(())
    }

    #[test]
    fn test_search_engine() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        let nodes = vec![
            (NodeId(1), "MyClass".to_string()),
            (NodeId(2), "my_function".to_string()),
            (NodeId(3), "another_function".to_string()),
        ];

        engine.index_nodes(nodes)?;

        let results = engine.search_symbol("MyC");
        assert!(!results.is_empty(), "Should find at least one match");
        assert_eq!(
            results[0],
            NodeId(1),
            "MyClass should be the best match for 'MyC'"
        );

        let results = engine.search_symbol("func");
        assert_eq!(results.len(), 2);

        let results = engine.search_full_text("another")?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], NodeId(3));

        Ok(())
    }

    #[test]
    fn test_remove_nodes() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        engine.index_nodes(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
            (NodeId(3), "GammaSymbol".to_string()),
        ])?;

        let before = engine.search_symbol("Beta");
        assert!(before.contains(&NodeId(2)));
        assert_eq!(engine.search_full_text("betasymbol")?, vec![NodeId(2)]);

        engine.remove_nodes(&[NodeId(2)])?;

        let after = engine.search_symbol("Beta");
        assert!(!after.contains(&NodeId(2)));
        assert!(engine.search_full_text("betasymbol")?.is_empty());

        let remaining = engine.search_symbol("Gamma");
        assert!(remaining.contains(&NodeId(3)));

        Ok(())
    }

    #[test]
    fn test_open_existing_reuses_persisted_index_without_recreating_dir() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("search");
        let marker = search_dir.join("keep.txt");

        let mut engine = SearchEngine::new(Some(search_dir.as_path()))?;
        engine.index_nodes(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
        ])?;
        std::fs::write(&marker, "marker")?;
        drop(engine);

        let mut reopened = SearchEngine::open_existing(search_dir.as_path())?;
        reopened.load_symbol_projection(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
        ]);

        assert!(
            marker.exists(),
            "opening an existing index should not recreate the dir"
        );
        assert_eq!(reopened.search_full_text("betasymbol")?, vec![NodeId(2)]);
        assert_eq!(reopened.search_symbol("Beta"), vec![NodeId(2)]);
        Ok(())
    }

    #[test]
    fn test_hybrid_search_requires_semantic_runtime() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_nodes(vec![(NodeId(1), "AlphaSymbol".to_string())])?;
        let err = engine
            .search_hybrid_with_scores("alpha", &HashMap::new(), HybridSearchConfig::default())
            .expect_err("semantic runtime should be required");
        assert!(
            err.to_string().contains("semantic retrieval is required"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn test_hybrid_search_scores() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_nodes(vec![
            (NodeId(1), "pkg::alpha".to_string()),
            (NodeId(2), "pkg::beta".to_string()),
        ])?;
        engine.set_embedding_runtime(EmbeddingRuntime::test_runtime());
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(1),
                doc_text: "alpha symbol".to_string(),
                embedding: embed_text_with_hash_projection("alpha symbol", EMBEDDING_DIM),
            },
            LlmSearchDoc {
                node_id: NodeId(2),
                doc_text: "beta symbol".to_string(),
                embedding: embed_text_with_hash_projection("beta symbol", EMBEDDING_DIM),
            },
        ]);

        let mut graph_boosts = HashMap::new();
        graph_boosts.insert(NodeId(1), 1.0);

        let hits = engine.search_hybrid_with_scores(
            "alpha",
            &graph_boosts,
            HybridSearchConfig {
                max_results: 5,
                lexical_weight: 0.3,
                semantic_weight: 0.5,
                graph_weight: 0.2,
                lexical_limit: 10,
                semantic_limit: 10,
            },
        )?;

        assert!(!hits.is_empty());
        assert_eq!(hits[0].node_id, NodeId(1));
        Ok(())
    }

    #[test]
    fn test_hybrid_search_semantic_can_win_with_weak_lexical_overlap() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_nodes(vec![
            (NodeId(10), "fn_a".to_string()),
            (NodeId(11), "fn_b".to_string()),
        ])?;
        engine.set_embedding_runtime(EmbeddingRuntime::test_runtime());
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(10),
                doc_text: "handles authorization policy and permission checks".to_string(),
                embedding: embed_text_with_hash_projection(
                    "handles authorization policy and permission checks",
                    EMBEDDING_DIM,
                ),
            },
            LlmSearchDoc {
                node_id: NodeId(11),
                doc_text: "renders UI theme colors and spacing".to_string(),
                embedding: embed_text_with_hash_projection(
                    "renders UI theme colors and spacing",
                    EMBEDDING_DIM,
                ),
            },
        ]);

        let hits = engine.search_hybrid_with_scores(
            "permission validation",
            &HashMap::new(),
            HybridSearchConfig {
                max_results: 5,
                lexical_weight: 0.2,
                semantic_weight: 0.7,
                graph_weight: 0.1,
                lexical_limit: 5,
                semantic_limit: 5,
            },
        )?;

        assert!(!hits.is_empty());
        assert_eq!(hits[0].node_id, NodeId(10));
        Ok(())
    }
}
