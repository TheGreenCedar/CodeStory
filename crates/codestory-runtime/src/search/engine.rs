use crate::symbol_query::RetrievalFileRole;
#[cfg(test)]
use crate::symbol_query::query_mentions_non_primary_source;
use anyhow::{Context, Result, anyhow, bail};
use codestory_contracts::graph::NodeId;
use fs4::fs_std::FileExt;
use ndarray::Array2;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32String};
use ort::{
    ep, inputs,
    session::{Session, builder::GraphOptimizationLevel},
    value::TensorRef,
};
use parking_lot::Mutex;
use rayon::prelude::*;
use serde_json::{Value as JsonValue, json};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::schema::{FAST, INDEXED, STORED, Schema, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

pub const EMBEDDING_DIM: usize = 384;
const SEARCH_WRITER_HEAP_BYTES: usize = 20_000_000;
pub const EMBEDDING_MODEL_ID_ENV: &str = "CODESTORY_EMBED_MODEL_ID";
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
pub const ONNX_MODEL_PATH_ENV: &str = "CODESTORY_EMBED_ONNX_MODEL";
pub const ONNX_TOKENIZER_PATH_ENV: &str = "CODESTORY_EMBED_ONNX_TOKENIZER";
pub const ONNX_PROVIDER_ENV: &str = "CODESTORY_EMBED_ONNX_PROVIDER";
pub const ONNX_THREADS_ENV: &str = "CODESTORY_EMBED_ONNX_THREADS";
pub const ONNX_BATCH_TOKENS_ENV: &str = "CODESTORY_EMBED_ONNX_BATCH_TOKENS";
pub const LLAMACPP_EMBEDDINGS_URL_ENV: &str = "CODESTORY_EMBED_LLAMACPP_URL";
pub const LLAMACPP_REQUEST_COUNT_ENV: &str = "CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT";
pub const STORED_VECTOR_ENCODING_ENV: &str = "CODESTORY_STORED_VECTOR_ENCODING";
pub const SYMBOL_FULL_TEXT_INDEX_ENV: &str = "CODESTORY_SYMBOL_FULL_TEXT_INDEX";
const DEFAULT_LLAMACPP_EMBEDDINGS_URL: &str = "http://127.0.0.1:8080/v1/embeddings";
const DEFAULT_LLAMACPP_REQUEST_COUNT: usize = 1;
const MAX_LLAMACPP_REQUEST_COUNT: usize = 16;
const DEFAULT_ONNX_BATCH_TOKENS: usize = 32_768;
#[cfg(test)]
const SEMANTIC_QUANTIZED_RESCORE_MULTIPLIER: usize = 4;

type HttpHeaders = Vec<(String, String)>;
type RawHttpResponse = (u16, HttpHeaders, Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SymbolCandidateRank {
    exact_display: u8,
    exact_terminal: u8,
    exact_leading: u8,
    fuzzy_score: u32,
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeAvailability {
    pub available: bool,
    pub model_id: Option<String>,
    pub fallback_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProfileContract {
    pub profile: String,
    pub backend: String,
    pub model_id: String,
    pub cache_key: String,
    pub dimension: Option<u32>,
}

fn env_usize(key: &str, min: usize, max: usize) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|value| value.clamp(min, max))
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

fn symbol_full_text_index_enabled_from_env() -> bool {
    env_bool_override(SYMBOL_FULL_TEXT_INDEX_ENV).unwrap_or(true)
}

fn embedding_parallel_chunk_size(text_count: usize, worker_count: usize) -> usize {
    let workers = worker_count.max(1).min(text_count.max(1));
    text_count.max(1).div_ceil(workers).max(1)
}

fn llamacpp_request_count_from_env() -> usize {
    let Ok(raw) = std::env::var(LLAMACPP_REQUEST_COUNT_ENV) else {
        return DEFAULT_LLAMACPP_REQUEST_COUNT;
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "auto" | "available_parallelism") {
        return std::thread::available_parallelism()
            .map(|workers| workers.get())
            .unwrap_or(DEFAULT_LLAMACPP_REQUEST_COUNT)
            .clamp(1, MAX_LLAMACPP_REQUEST_COUNT);
    }
    normalized
        .parse::<usize>()
        .map(|value| value.clamp(1, MAX_LLAMACPP_REQUEST_COUNT))
        .unwrap_or(DEFAULT_LLAMACPP_REQUEST_COUNT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingBackendSelection {
    Onnx,
    LlamaCpp,
    HashProjection,
}

impl EmbeddingBackendSelection {
    fn from_env() -> Result<Self> {
        let runtime_mode_raw = std::env::var(EMBEDDING_RUNTIME_MODE_ENV).ok();
        let backend_raw = std::env::var(EMBEDDING_BACKEND_ENV).ok();
        let runtime_mode = runtime_mode_raw
            .as_deref()
            .unwrap_or("llamacpp")
            .trim()
            .to_ascii_lowercase();
        if runtime_mode == "hash" || runtime_mode == "hash_projection" {
            return Ok(Self::HashProjection);
        }

        let backend = backend_raw
            .as_deref()
            .unwrap_or(&runtime_mode)
            .trim()
            .to_ascii_lowercase();
        if backend_is_auto_or_default(&backend)
            && runtime_mode_raw
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            && backend_raw
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            && std::env::var(LLAMACPP_EMBEDDINGS_URL_ENV)
                .ok()
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
        {
            return Ok(Self::LlamaCpp);
        }
        match backend.as_str() {
            "" | "auto" | "onnx" | "ort" | "onnxruntime" | "onnx-runtime" => Ok(Self::Onnx),
            "llamacpp" | "llama.cpp" | "llama-cpp" | "gguf" => Ok(Self::LlamaCpp),
            "hash" | "hash_projection" => Ok(Self::HashProjection),
            other => Err(anyhow!(
                "unsupported embedding backend `{other}` (set {EMBEDDING_BACKEND_ENV}=onnx, llamacpp, or hash)"
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

fn backend_is_auto_or_default(value: &str) -> bool {
    matches!(
        value,
        "" | "auto" | "onnx" | "ort" | "onnxruntime" | "onnx-runtime"
    )
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
            .unwrap_or_else(|_| "bge-base-en-v1.5".to_string())
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
            "custom" => Self {
                name: "custom".to_string(),
                model_id: "custom-local".to_string(),
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
                    "unsupported embedding profile `{other}` (set {EMBEDDING_PROFILE_ENV}=minilm, bge-small-en-v1.5, bge-base-en-v1.5, qwen3-embedding-0.6b, embeddinggemma-300m, nomic-embed-text-v1.5, nomic-embed-text-v2-moe, or custom)"
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
            return unavailable_embedding_runtime(None, error);
        }
    };
    let backend = match EmbeddingBackendSelection::from_env() {
        Ok(backend) => backend,
        Err(error) => {
            return unavailable_embedding_runtime(Some(profile.model_id.clone()), error);
        }
    };
    let model_id = profile.cache_model_id(backend);

    if backend == EmbeddingBackendSelection::HashProjection {
        return available_embedding_runtime(model_id);
    }

    if let Err(error) = ensure_embedding_backend_available(backend) {
        return unavailable_embedding_runtime(Some(model_id), error);
    }

    available_embedding_runtime(model_id)
}

fn available_embedding_runtime(model_id: String) -> EmbeddingRuntimeAvailability {
    EmbeddingRuntimeAvailability {
        available: true,
        model_id: Some(model_id),
        fallback_message: None,
    }
}

fn unavailable_embedding_runtime(
    model_id: Option<String>,
    error: impl std::fmt::Display,
) -> EmbeddingRuntimeAvailability {
    EmbeddingRuntimeAvailability {
        available: false,
        model_id,
        fallback_message: Some(error.to_string()),
    }
}

fn ensure_embedding_backend_available(backend: EmbeddingBackendSelection) -> Result<()> {
    match backend {
        EmbeddingBackendSelection::Onnx => OnnxEmbeddingRuntime::probe_from_env().map(|_| ()),
        EmbeddingBackendSelection::LlamaCpp => {
            LlamaCppEndpoint::from_env().and_then(|endpoint| endpoint.ensure_reachable())
        }
        EmbeddingBackendSelection::HashProjection => Ok(()),
    }
}

pub fn embedding_profile_contract_from_env() -> Result<EmbeddingProfileContract> {
    let profile = EmbeddingProfile::from_env()?;
    let backend = EmbeddingBackendSelection::from_env()?;
    let cache_key = profile.cache_model_id(backend);
    Ok(EmbeddingProfileContract {
        profile: profile.name.clone(),
        backend: backend.as_str().to_string(),
        model_id: profile.model_id.clone(),
        cache_key,
        dimension: profile
            .expected_dim
            .map(|value| value.min(u32::MAX as usize) as u32),
    })
}

pub fn probe_onnx_runtime_paths(model_path: &Path, tokenizer_path: &Path) -> Result<()> {
    OnnxEmbeddingRuntime::from_paths(OnnxModelPaths {
        model_path: model_path.to_path_buf(),
        tokenizer_path: tokenizer_path.to_path_buf(),
    })
    .map(|_| ())
}

#[derive(Debug, Clone)]
pub struct LlmSearchDoc {
    pub node_id: NodeId,
    pub file_role: RetrievalFileRole,
    pub doc_text: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeProbe {
    pub model_path: PathBuf,
    pub model_id: String,
}

#[derive(Debug, Clone)]
struct OnnxModelPaths {
    model_path: PathBuf,
    tokenizer_path: PathBuf,
}

impl OnnxModelPaths {
    fn from_env() -> Result<Self> {
        let model_path = required_path_env(ONNX_MODEL_PATH_ENV)?;
        let tokenizer_path = required_path_env(ONNX_TOKENIZER_PATH_ENV)?;
        Ok(Self {
            model_path,
            tokenizer_path,
        })
    }
}

fn required_path_env(key: &str) -> Result<PathBuf> {
    let raw = std::env::var(key).with_context(|| {
        format!(
            "{key} is not set; ONNX is diagnostic-only, so set {ONNX_MODEL_PATH_ENV} and {ONNX_TOKENIZER_PATH_ENV} explicitly or use product llama.cpp sidecar retrieval"
        )
    })?;
    let path = PathBuf::from(raw.trim());
    if raw.trim().is_empty() {
        bail!("{key} is empty");
    }
    if !path.exists() {
        bail!("{key} points at a missing path: {}", path.display());
    }
    Ok(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnnxProvider {
    Auto,
    DirectMl,
    Cpu,
}

impl OnnxProvider {
    fn from_env() -> Result<Self> {
        let raw = std::env::var(ONNX_PROVIDER_ENV)
            .unwrap_or_else(|_| "auto".to_string())
            .trim()
            .to_ascii_lowercase();
        match raw.as_str() {
            "" | "auto" => Ok(Self::Auto),
            "directml" | "dml" | "gpu" => Ok(Self::DirectMl),
            "cpu" => Ok(Self::Cpu),
            other => Err(anyhow!(
                "unsupported {ONNX_PROVIDER_ENV} value `{other}` (expected auto, directml, or cpu)"
            )),
        }
    }
}

#[derive(Debug)]
struct OnnxEmbeddingRuntime {
    tokenizer: tokenizers::Tokenizer,
    session: Mutex<Session>,
}

struct OnnxEncodedRow {
    ids: Vec<i64>,
    attention: Vec<i64>,
    token_types: Vec<i64>,
}

struct OnnxEncodedBatch {
    input_ids: Array2<i64>,
    attention_mask: Array2<i64>,
    token_type_ids: Array2<i64>,
    attention_values: Option<Vec<i64>>,
    sequence_len: usize,
}

impl OnnxEmbeddingRuntime {
    fn from_env() -> Result<(PathBuf, Self)> {
        let paths = OnnxModelPaths::from_env()?;
        Self::from_paths(paths)
    }

    fn probe_from_env() -> Result<PathBuf> {
        Self::from_env().map(|(path, _runtime)| path)
    }

    fn from_paths(paths: OnnxModelPaths) -> Result<(PathBuf, Self)> {
        let tokenizer =
            tokenizers::Tokenizer::from_file(&paths.tokenizer_path).map_err(|error| {
                anyhow!(
                    "failed to load ONNX tokenizer {}: {error}",
                    paths.tokenizer_path.display()
                )
            })?;
        let session = build_onnx_session(&paths.model_path)?;
        Ok((
            paths.model_path,
            Self {
                tokenizer,
                session: Mutex::new(session),
            },
        ))
    }

    fn embed_texts(&self, texts: &[String], profile: &EmbeddingProfile) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let rows = self.encode_rows(texts, profile.max_tokens)?;
        let max_batch_tokens =
            env_usize(ONNX_BATCH_TOKENS_ENV, 1, 262_144).unwrap_or(DEFAULT_ONNX_BATCH_TOKENS);
        let mut embeddings = Vec::with_capacity(texts.len());
        let mut start = 0;
        while start < rows.len() {
            let end = onnx_batch_end_for_token_budget(&rows, start, max_batch_tokens);
            let batch = shape_onnx_batch(&rows[start..end], profile.pooling)?;
            embeddings.extend(self.run_encoded_batch(batch, profile)?);
            start = end;
        }
        Ok(embeddings)
    }

    fn run_encoded_batch(
        &self,
        batch: OnnxEncodedBatch,
        profile: &EmbeddingProfile,
    ) -> Result<Vec<Vec<f32>>> {
        let input_ids = TensorRef::from_array_view(batch.input_ids.view())
            .context("failed to create ONNX input_ids tensor")?;
        let attention_mask = TensorRef::from_array_view(batch.attention_mask.view())
            .context("failed to create ONNX attention_mask tensor")?;
        let token_type_ids = TensorRef::from_array_view(batch.token_type_ids.view())
            .context("failed to create ONNX token_type_ids tensor")?;

        let mut session = self.session.lock();
        let outputs = session
            .run(inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
                "token_type_ids" => token_type_ids,
            ])
            .context("failed to run ONNX embedding model")?;
        let output = outputs
            .get("sentence_embedding")
            .or_else(|| outputs.get("embeddings"))
            .or_else(|| outputs.get("last_hidden_state"))
            .unwrap_or_else(|| &outputs[0]);
        let (shape, data) = output
            .try_extract_tensor::<f32>()
            .context("ONNX embedding output was not a float tensor")?;

        let attention = batch.attention_values.as_deref();
        extract_onnx_embeddings(
            data,
            shape,
            attention,
            batch.input_ids.nrows(),
            batch.sequence_len,
            profile.pooling,
        )
    }

    fn encode_rows(&self, texts: &[String], max_tokens: usize) -> Result<Vec<OnnxEncodedRow>> {
        let pad_id = self.tokenizer.token_to_id("[PAD]").unwrap_or(0) as i64;
        let sep_id = self
            .tokenizer
            .token_to_id("[SEP]")
            .map(|value| value as i64);
        let encodings = self
            .tokenizer
            .encode_batch_fast(texts.iter().map(String::as_str).collect::<Vec<_>>(), true)
            .map_err(|error| {
                anyhow!("failed to tokenize input batch for ONNX embedding model: {error}")
            })?;
        let mut rows = Vec::with_capacity(texts.len());

        for encoding in encodings {
            let mut ids = encoding
                .get_ids()
                .iter()
                .map(|value| *value as i64)
                .collect::<Vec<_>>();
            let mut attention = encoding
                .get_attention_mask()
                .iter()
                .map(|value| *value as i64)
                .collect::<Vec<_>>();
            let mut token_types = encoding
                .get_type_ids()
                .iter()
                .map(|value| *value as i64)
                .collect::<Vec<_>>();

            if ids.is_empty() {
                ids.push(pad_id);
                attention.push(0);
                token_types.push(0);
            }
            if token_types.len() < ids.len() {
                token_types.resize(ids.len(), 0);
            }
            if attention.len() < ids.len() {
                attention.resize(ids.len(), 1);
            }
            if ids.len() > max_tokens {
                ids.truncate(max_tokens);
                attention.truncate(max_tokens);
                token_types.truncate(max_tokens);
                if let (Some(sep_id), Some(last)) = (sep_id, ids.last_mut()) {
                    *last = sep_id;
                }
            }

            rows.push(OnnxEncodedRow {
                ids,
                attention,
                token_types,
            });
        }

        Ok(rows)
    }
}

fn onnx_batch_end_for_token_budget(
    rows: &[OnnxEncodedRow],
    start: usize,
    max_batch_tokens: usize,
) -> usize {
    let mut end = start;
    let mut sequence_len = 1_usize;
    while end < rows.len() {
        let next_sequence_len = sequence_len.max(rows[end].ids.len());
        let next_row_count = end + 1 - start;
        if end > start && next_sequence_len * next_row_count > max_batch_tokens {
            break;
        }
        sequence_len = next_sequence_len;
        end += 1;
    }
    end.max(start + 1)
}

fn shape_onnx_batch(
    rows: &[OnnxEncodedRow],
    pooling: EmbeddingPooling,
) -> Result<OnnxEncodedBatch> {
    let pad_id = 0_i64;
    let row_count = rows.len();
    let sequence_len = rows
        .iter()
        .map(|row| row.ids.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let mut input_values = vec![pad_id; row_count * sequence_len];
    let mut attention_values = vec![0_i64; row_count * sequence_len];
    let mut token_type_values = vec![0_i64; row_count * sequence_len];

    for (row_index, row) in rows.iter().enumerate() {
        let offset = row_index * sequence_len;
        input_values[offset..offset + row.ids.len()].copy_from_slice(&row.ids);
        attention_values[offset..offset + row.attention.len()].copy_from_slice(&row.attention);
        token_type_values[offset..offset + row.token_types.len()].copy_from_slice(&row.token_types);
    }

    let needs_attention_values = !matches!(pooling, EmbeddingPooling::Cls);
    let pooling_attention_values = needs_attention_values.then(|| attention_values.clone());

    Ok(OnnxEncodedBatch {
        input_ids: Array2::from_shape_vec((row_count, sequence_len), input_values)
            .context("failed to shape ONNX input_ids")?,
        attention_mask: Array2::from_shape_vec((row_count, sequence_len), attention_values)
            .context("failed to shape ONNX attention_mask")?,
        token_type_ids: Array2::from_shape_vec((row_count, sequence_len), token_type_values)
            .context("failed to shape ONNX token_type_ids")?,
        attention_values: pooling_attention_values,
        sequence_len,
    })
}

fn build_onnx_session(model_path: &Path) -> Result<Session> {
    match OnnxProvider::from_env()? {
        OnnxProvider::Auto => {
            if cfg!(target_os = "windows")
                && let Ok(session) =
                    build_onnx_session_for_provider(model_path, OnnxProvider::DirectMl)
            {
                return Ok(session);
            }
            build_onnx_session_for_provider(model_path, OnnxProvider::Cpu)
        }
        OnnxProvider::DirectMl => {
            build_onnx_session_for_provider(model_path, OnnxProvider::DirectMl)
        }
        OnnxProvider::Cpu => build_onnx_session_for_provider(model_path, OnnxProvider::Cpu),
    }
}

fn build_onnx_session_for_provider(model_path: &Path, provider: OnnxProvider) -> Result<Session> {
    let mut builder = Session::builder()
        .context("failed to initialize ONNX Runtime session builder")?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort::Error::<()>::from)
        .context("failed to configure ONNX graph optimizations")?;
    if let Some(threads) = env_usize(ONNX_THREADS_ENV, 1, 64) {
        builder = builder
            .with_intra_threads(threads)
            .map_err(ort::Error::<()>::from)
            .context("failed to configure ONNX intra-op threads")?;
    }
    builder = match provider {
        OnnxProvider::DirectMl => builder
            .with_execution_providers([ep::DirectML::default().build().error_on_failure()])
            .map_err(ort::Error::<()>::from)
            .context("failed to configure ONNX DirectML execution provider")?,
        OnnxProvider::Cpu => builder
            .with_execution_providers([ep::CPU::default().build()])
            .map_err(ort::Error::<()>::from)
            .context("failed to configure ONNX CPU execution provider")?,
        OnnxProvider::Auto => unreachable!("auto provider is resolved before session creation"),
    };
    builder.commit_from_file(model_path).with_context(|| {
        format!(
            "failed to load ONNX embedding model {}",
            model_path.display()
        )
    })
}

fn extract_onnx_embeddings(
    data: &[f32],
    shape: &[i64],
    attention: Option<&[i64]>,
    expected_batch: usize,
    expected_sequence_len: usize,
    pooling: EmbeddingPooling,
) -> Result<Vec<Vec<f32>>> {
    match shape.len() {
        2 => collect_onnx_pooled_embeddings(data, shape, expected_batch),
        3 => pool_onnx_hidden_state(
            data,
            shape,
            attention,
            expected_batch,
            expected_sequence_len,
            pooling,
        ),
        _ => bail!("ONNX embedding output must be rank 2 or rank 3, got shape {shape:?}"),
    }
}

fn collect_onnx_pooled_embeddings(
    data: &[f32],
    shape: &[i64],
    expected_batch: usize,
) -> Result<Vec<Vec<f32>>> {
    let batch = usize::try_from(shape[0]).context("invalid ONNX output batch dimension")?;
    let hidden_dim = usize::try_from(shape[1]).context("invalid ONNX output hidden dimension")?;
    if batch != expected_batch {
        bail!("ONNX embedding output batch mismatch: expected {expected_batch}, got {batch}");
    }
    if data.len() != batch * hidden_dim {
        bail!(
            "ONNX embedding output data length mismatch: shape={shape:?}, values={}",
            data.len()
        );
    }
    Ok(data
        .chunks_exact(hidden_dim)
        .map(<[f32]>::to_vec)
        .collect::<Vec<_>>())
}

fn pool_onnx_hidden_state(
    data: &[f32],
    shape: &[i64],
    attention: Option<&[i64]>,
    expected_batch: usize,
    expected_sequence_len: usize,
    pooling: EmbeddingPooling,
) -> Result<Vec<Vec<f32>>> {
    if shape.len() != 3 {
        bail!("ONNX embedding output must be rank 3, got shape {shape:?}");
    }
    let batch = usize::try_from(shape[0]).context("invalid ONNX output batch dimension")?;
    let sequence_len =
        usize::try_from(shape[1]).context("invalid ONNX output sequence dimension")?;
    let hidden_dim = usize::try_from(shape[2]).context("invalid ONNX output hidden dimension")?;
    if batch != expected_batch {
        bail!("ONNX embedding output batch mismatch: expected {expected_batch}, got {batch}");
    }
    if sequence_len != expected_sequence_len {
        bail!(
            "ONNX embedding output sequence mismatch: expected {expected_sequence_len}, got {sequence_len}"
        );
    }
    if data.len() != batch * sequence_len * hidden_dim {
        bail!(
            "ONNX embedding output data length mismatch: shape={shape:?}, values={}",
            data.len()
        );
    }

    let mut embeddings = Vec::with_capacity(batch);
    for row in 0..batch {
        let vector = match pooling {
            EmbeddingPooling::Cls => {
                let start = row * sequence_len * hidden_dim;
                data[start..start + hidden_dim].to_vec()
            }
            EmbeddingPooling::LastToken => {
                let attention =
                    attention.ok_or_else(|| anyhow!("ONNX last-token pooling needs attention"))?;
                let token_index = last_attention_index(attention, row, sequence_len).unwrap_or(0);
                let start = ((row * sequence_len) + token_index) * hidden_dim;
                data[start..start + hidden_dim].to_vec()
            }
            EmbeddingPooling::Mean => {
                let attention =
                    attention.ok_or_else(|| anyhow!("ONNX mean pooling needs attention"))?;
                let mut vector = vec![0.0_f32; hidden_dim];
                let mut count = 0.0_f32;
                for token_index in 0..sequence_len {
                    if attention[row * sequence_len + token_index] == 0 {
                        continue;
                    }
                    let start = ((row * sequence_len) + token_index) * hidden_dim;
                    for (target, value) in vector
                        .iter_mut()
                        .zip(data[start..start + hidden_dim].iter().copied())
                    {
                        *target += value;
                    }
                    count += 1.0;
                }
                if count > 0.0 {
                    for value in &mut vector {
                        *value /= count;
                    }
                }
                vector
            }
        };
        embeddings.push(vector);
    }
    Ok(embeddings)
}

fn last_attention_index(attention: &[i64], row: usize, sequence_len: usize) -> Option<usize> {
    let offset = row * sequence_len;
    (0..sequence_len)
        .rev()
        .find(|token_index| attention[offset + *token_index] != 0)
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

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredVectorEncoding {
    Float32,
    Int8,
    Uint8,
    Binary,
    Ubinary,
}

#[cfg(test)]
impl StoredVectorEncoding {
    fn from_env() -> Result<Self> {
        let raw = std::env::var(STORED_VECTOR_ENCODING_ENV).unwrap_or_default();
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" | "float32" | "none" => Ok(Self::Float32),
            "int8" => Ok(Self::Int8),
            "uint8" => Ok(Self::Uint8),
            "binary" => Ok(Self::Binary),
            "ubinary" => Ok(Self::Ubinary),
            other => Err(anyhow!(
                "unsupported stored vector encoding `{other}` (set {STORED_VECTOR_ENCODING_ENV}=float32, int8, uint8, binary, or ubinary)"
            )),
        }
    }

    fn quantize(self, values: &[f32]) -> Option<QuantizedEmbedding> {
        match self {
            Self::Float32 => None,
            Self::Int8 => Some(QuantizedEmbedding::Int8(
                values
                    .iter()
                    .map(|value| (value * 127.0).round().clamp(-127.0, 127.0) as i8)
                    .collect(),
            )),
            Self::Uint8 => Some(QuantizedEmbedding::Uint8(
                values
                    .iter()
                    .map(|value| ((value + 1.0) * 127.5).round().clamp(0.0, 255.0) as u8)
                    .collect(),
            )),
            Self::Binary => Some(QuantizedEmbedding::Binary(pack_sign_bits(values))),
            Self::Ubinary => Some(QuantizedEmbedding::Ubinary(pack_sign_bits(values))),
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
enum QuantizedEmbedding {
    Int8(Vec<i8>),
    Uint8(Vec<u8>),
    Binary(PackedSignBits),
    Ubinary(PackedSignBits),
}

#[cfg(test)]
impl QuantizedEmbedding {
    fn approximate_cosine(&self, query_embedding: &[f32]) -> f32 {
        match self {
            Self::Int8(values) => {
                if values.len() != query_embedding.len() || values.is_empty() {
                    return 0.0;
                }
                query_embedding
                    .iter()
                    .zip(values)
                    .map(|(query, doc)| query * (*doc as f32 / 127.0))
                    .sum()
            }
            Self::Uint8(values) => {
                if values.len() != query_embedding.len() || values.is_empty() {
                    return 0.0;
                }
                query_embedding
                    .iter()
                    .zip(values)
                    .map(|(query, doc)| query * ((*doc as f32 / 127.5) - 1.0))
                    .sum()
            }
            Self::Binary(bits) => signed_binary_cosine(query_embedding, bits),
            Self::Ubinary(bits) => unsigned_binary_cosine(query_embedding, bits),
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct PackedSignBits {
    bytes: Vec<u8>,
    len: usize,
    positives: usize,
}

#[cfg(test)]
fn pack_sign_bits(values: &[f32]) -> PackedSignBits {
    let mut bytes = vec![0_u8; values.len().div_ceil(8)];
    let mut positives = 0;
    for (index, value) in values.iter().enumerate() {
        if *value >= 0.0 {
            bytes[index / 8] |= 1 << (index % 8);
            positives += 1;
        }
    }
    PackedSignBits {
        bytes,
        len: values.len(),
        positives,
    }
}

#[cfg(test)]
fn sign_bit(bits: &PackedSignBits, index: usize) -> bool {
    let Some(byte) = bits.bytes.get(index / 8) else {
        return false;
    };
    (byte & (1 << (index % 8))) != 0
}

#[cfg(test)]
fn signed_binary_cosine(query_embedding: &[f32], bits: &PackedSignBits) -> f32 {
    if query_embedding.len() != bits.len || bits.len == 0 {
        return 0.0;
    }
    let mut score = 0_i32;
    for (index, query) in query_embedding.iter().enumerate() {
        let same_sign = (*query >= 0.0) == sign_bit(bits, index);
        score += if same_sign { 1 } else { -1 };
    }
    score as f32 / bits.len as f32
}

#[cfg(test)]
fn unsigned_binary_cosine(query_embedding: &[f32], bits: &PackedSignBits) -> f32 {
    if query_embedding.len() != bits.len || bits.len == 0 {
        return 0.0;
    }
    let mut query_positives = 0_usize;
    let mut intersection = 0_usize;
    for (index, query) in query_embedding.iter().enumerate() {
        if *query >= 0.0 {
            query_positives += 1;
            if sign_bit(bits, index) {
                intersection += 1;
            }
        }
    }
    if query_positives == 0 || bits.positives == 0 {
        return 0.0;
    }
    intersection as f32 / ((query_positives * bits.positives) as f32).sqrt()
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            max_results: 20,
            lexical_weight: 0.0,
            semantic_weight: 1.0,
            graph_weight: 0.0,
            lexical_limit: 0,
            semantic_limit: 20,
        }
    }
}

impl HybridSearchConfig {
    /// Lexical-first hybrid policy for packet subqueries and exact-anchor paths.
    pub fn lexical_first() -> Self {
        Self {
            max_results: 20,
            lexical_weight: 1.0,
            semantic_weight: 0.0,
            graph_weight: 0.0,
            lexical_limit: 200,
            semantic_limit: 20,
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

    fn ensure_reachable(&self) -> Result<()> {
        let mut addrs = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .with_context(|| format!("failed to resolve llama.cpp endpoint {}", self.url()))?;
        let addr = addrs
            .next()
            .ok_or_else(|| anyhow!("failed to resolve llama.cpp endpoint {}", self.url()))?;
        TcpStream::connect_timeout(&addr, Duration::from_millis(750))
            .with_context(|| format!("failed to connect to llama.cpp endpoint {}", self.url()))?;
        Ok(())
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

fn split_http_response(response: &[u8]) -> Result<RawHttpResponse> {
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

        if backend == EmbeddingBackendSelection::Onnx {
            let model_path = OnnxEmbeddingRuntime::probe_from_env()?;
            return Ok(EmbeddingRuntimeProbe {
                model_path,
                model_id,
            });
        }

        if backend == EmbeddingBackendSelection::LlamaCpp {
            let endpoint = LlamaCppEndpoint::from_env()?;
            endpoint.ensure_reachable()?;
            return Ok(EmbeddingRuntimeProbe {
                model_path: PathBuf::from(endpoint.url()),
                model_id,
            });
        }

        unreachable!("all embedding backends are handled above")
    }

    pub fn from_env() -> Result<Self> {
        let profile = EmbeddingProfile::from_env()?;
        let backend = EmbeddingBackendSelection::from_env()?;
        let model_id = profile.cache_model_id(backend);

        match backend {
            EmbeddingBackendSelection::HashProjection => Ok(Self {
                model_path: PathBuf::from("hash-projection"),
                model_id,
                profile,
                backend: EmbeddingBackend::HashProjection,
            }),
            EmbeddingBackendSelection::Onnx => {
                let (model_path, runtime) = OnnxEmbeddingRuntime::from_env()?;
                Ok(Self {
                    model_path,
                    model_id,
                    profile,
                    backend: EmbeddingBackend::Onnx(Arc::new(runtime)),
                })
            }
            EmbeddingBackendSelection::LlamaCpp => {
                let endpoint = LlamaCppEndpoint::from_env()?;
                endpoint.ensure_reachable()?;
                Ok(Self {
                    model_path: PathBuf::from(endpoint.url()),
                    model_id,
                    profile,
                    backend: EmbeddingBackend::LlamaCpp(Arc::new(LlamaCppEmbeddingRuntime {
                        endpoint,
                        request_count: llamacpp_request_count_from_env(),
                    })),
                })
            }
        }
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

    pub fn embed_text_refs(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
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
            EmbeddingBackend::Onnx(runtime) => runtime.embed_texts(texts, &self.profile),
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
            model_path: PathBuf::from("hash-projection"),
            model_id: "test-model".to_string(),
            profile,
            backend: EmbeddingBackend::HashProjection,
        }
    }
}

pub struct SearchEngine {
    symbols: Vec<(Utf32String, NodeId)>,
    index: Index,
    reader: IndexReader,
    llm_docs: HashMap<NodeId, LlmSearchDoc>,
    #[cfg(test)]
    quantized_llm_docs: HashMap<NodeId, QuantizedEmbedding>,
    embedding_runtime: Option<EmbeddingRuntime>,
    #[cfg(test)]
    stored_vector_encoding: StoredVectorEncoding,
    full_text_index_enabled: bool,
    #[cfg(test)]
    query_embedding_cache: HashMap<String, Vec<f32>>,
    _persisted_index_guard: Option<PersistedSearchIndexGuard>,
}

struct PersistedSearchIndexGuard {
    file: File,
    path: PathBuf,
    mode: PersistedSearchIndexLockMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistedSearchIndexLockMode {
    Shared,
    Exclusive,
}

impl PersistedSearchIndexGuard {
    fn acquire_shared(search_dir: &Path) -> Result<Self> {
        Self::acquire_with_mode(search_dir, PersistedSearchIndexLockMode::Shared)
    }

    fn acquire_exclusive(search_dir: &Path) -> Result<Self> {
        Self::acquire_with_mode(search_dir, PersistedSearchIndexLockMode::Exclusive)
    }

    fn acquire_with_mode(search_dir: &Path, mode: PersistedSearchIndexLockMode) -> Result<Self> {
        let lock_path = persisted_search_index_lock_path(search_dir);
        if let Some(parent) = lock_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create search index lock parent {}",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open search index lock {}", lock_path.display()))?;
        match mode {
            PersistedSearchIndexLockMode::Shared => {
                FileExt::lock_shared(&file).with_context(|| {
                    format!(
                        "Failed to take shared search index lock {}",
                        search_dir.display()
                    )
                })?
            }
            PersistedSearchIndexLockMode::Exclusive => FileExt::lock_exclusive(&file)
                .with_context(|| {
                    format!(
                        "Failed to take exclusive search index lock {}",
                        search_dir.display()
                    )
                })?,
        }
        Ok(Self {
            file,
            path: lock_path,
            mode,
        })
    }

    #[cfg(test)]
    fn try_acquire_shared(search_dir: &Path) -> Result<Self> {
        Self::try_acquire_with_mode(search_dir, PersistedSearchIndexLockMode::Shared)
    }

    #[cfg(test)]
    fn try_acquire_exclusive(search_dir: &Path) -> Result<Self> {
        Self::try_acquire_with_mode(search_dir, PersistedSearchIndexLockMode::Exclusive)
    }

    #[cfg(test)]
    fn try_acquire_with_mode(
        search_dir: &Path,
        mode: PersistedSearchIndexLockMode,
    ) -> Result<Self> {
        let lock_path = persisted_search_index_lock_path(search_dir);
        if let Some(parent) = lock_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create search index lock parent {}",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open search index lock {}", lock_path.display()))?;
        let acquired = match mode {
            PersistedSearchIndexLockMode::Shared => {
                FileExt::try_lock_shared(&file).with_context(|| {
                    format!(
                        "Failed to take shared search index lock {}",
                        search_dir.display()
                    )
                })?
            }
            PersistedSearchIndexLockMode::Exclusive => FileExt::try_lock_exclusive(&file)
                .with_context(|| {
                    format!(
                        "Failed to take exclusive search index lock {}",
                        search_dir.display()
                    )
                })?,
        };
        if !acquired {
            bail!(
                "Search index {} is already locked in {:?} mode",
                search_dir.display(),
                mode
            );
        }
        Ok(Self {
            file,
            path: lock_path,
            mode,
        })
    }

    fn is_exclusive(&self) -> bool {
        self.mode == PersistedSearchIndexLockMode::Exclusive
    }
}

impl Drop for PersistedSearchIndexGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                "Failed to unlock persisted search index lock: {error}"
            );
        }
    }
}

fn persisted_search_index_lock_path(search_dir: &Path) -> PathBuf {
    let mut path = search_dir.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

impl SearchEngine {
    fn build_schema() -> Schema {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        schema_builder.add_i64_field("node_id", INDEXED | STORED | FAST);
        schema_builder.build()
    }

    fn new_with_index(
        index: Index,
        persisted_index_guard: Option<PersistedSearchIndexGuard>,
    ) -> Result<Self> {
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(Self {
            symbols: Vec::new(),
            index,
            reader,
            llm_docs: HashMap::new(),
            #[cfg(test)]
            quantized_llm_docs: HashMap::new(),
            embedding_runtime: None,
            #[cfg(test)]
            stored_vector_encoding: StoredVectorEncoding::from_env()?,
            full_text_index_enabled: symbol_full_text_index_enabled_from_env(),
            #[cfg(test)]
            query_embedding_cache: HashMap::new(),
            _persisted_index_guard: persisted_index_guard,
        })
    }

    #[cfg(test)]
    pub(crate) fn symbols(&self) -> &[(Utf32String, NodeId)] {
        &self.symbols
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn warm_query_embeddings(&mut self, queries: &[String]) -> Result<()> {
        for query in queries {
            let trimmed = query.trim();
            if trimmed.is_empty() {
                continue;
            }
            let _ = self.embed_query_cached(trimmed)?;
        }
        Ok(())
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn clear_query_embedding_cache(&mut self) {
        self.query_embedding_cache.clear();
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn cached_query_embedding(&self, query: &str) -> Option<Vec<f32>> {
        self.query_embedding_cache
            .get(&query.trim().to_ascii_lowercase())
            .cloned()
    }

    #[cfg(test)]
    pub(crate) fn hybrid_search_state(&self) -> HybridSearchState {
        HybridSearchState::from_engine(self)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn ensure_query_embedding(&mut self, query: &str) -> Result<Vec<f32>> {
        self.embed_query_cached(query)
    }

    #[cfg(test)]
    fn embed_query_cached(&mut self, query: &str) -> Result<Vec<f32>> {
        let key = query.trim().to_ascii_lowercase();
        if let Some(cached) = self.query_embedding_cache.get(&key) {
            return Ok(cached.clone());
        }
        let runtime = self
            .embedding_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("embedding runtime is not configured"))?;
        let embedding = runtime.embed_query(query)?;
        self.query_embedding_cache.insert(key, embedding.clone());
        Ok(embedding)
    }

    pub fn new(storage_path: Option<&Path>) -> Result<Self> {
        let schema = Self::build_schema();
        let index = if let Some(path) = storage_path {
            let guard = PersistedSearchIndexGuard::acquire_exclusive(path)?;
            return Self::new_persisted_with_guard(path, guard);
        } else {
            Index::create_in_ram(schema)
        };
        Self::new_with_index(index, None)
    }

    #[allow(dead_code)]
    pub fn open_existing(path: &Path) -> Result<Self> {
        let guard = PersistedSearchIndexGuard::acquire_shared(path)?;
        Self::open_persisted_with_guard(path, guard)
    }

    pub(crate) fn open_existing_or_recreate(path: &Path) -> Result<(Self, Option<anyhow::Error>)> {
        let shared_guard = PersistedSearchIndexGuard::acquire_shared(path)?;
        match Self::open_persisted_with_guard(path, shared_guard) {
            Ok(engine) => Ok((engine, None)),
            Err(open_error) => {
                let guard = PersistedSearchIndexGuard::acquire_exclusive(path)?;
                let engine = Self::new_persisted_with_guard(path, guard)?;
                Ok((engine, Some(open_error)))
            }
        }
    }

    pub(crate) fn recreate_persisted_from_existing(
        path: &Path,
        mut existing: Self,
    ) -> Result<Self> {
        let guard = existing._persisted_index_guard.take();
        drop(existing);
        match guard.filter(|guard| guard.is_exclusive()) {
            Some(guard) => Self::new_persisted_with_guard(path, guard),
            None => {
                let guard = PersistedSearchIndexGuard::acquire_exclusive(path)?;
                Self::new_persisted_with_guard(path, guard)
            }
        }
    }

    fn new_persisted_with_guard(path: &Path, guard: PersistedSearchIndexGuard) -> Result<Self> {
        if !guard.is_exclusive() {
            bail!(
                "Recreating persisted search index {} requires an exclusive lock",
                path.display()
            );
        }
        recreate_search_storage_dir(path)?;
        let schema = Self::build_schema();
        let index = Index::create_in_dir(path, schema)
            .with_context(|| format!("Failed to create tantivy index at {}", path.display()))?;
        Self::new_with_index(index, Some(guard))
    }

    fn open_persisted_with_guard(path: &Path, guard: PersistedSearchIndexGuard) -> Result<Self> {
        let index = Index::open_in_dir(path)
            .with_context(|| format!("Failed to open tantivy index at {}", path.display()))?;
        Self::new_with_index(index, Some(guard))
            .with_context(|| format!("Failed to initialize tantivy reader at {}", path.display()))
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

    #[cfg(test)]
    pub fn embedding_model_id(&self) -> Option<&str> {
        self.embedding_runtime
            .as_ref()
            .map(EmbeddingRuntime::model_id)
    }

    #[cfg(test)]
    pub fn embedding_runtime_configured(&self) -> bool {
        self.embedding_runtime.is_some()
    }

    pub fn full_text_doc_count(&self) -> usize {
        if !self.full_text_index_enabled {
            return self.symbols.len();
        }
        self.reader.searcher().num_docs() as usize
    }

    #[cfg(test)]
    pub fn semantic_index_ready(&self) -> bool {
        self.embedding_runtime.is_some() && !self.llm_docs.is_empty()
    }

    #[cfg(test)]
    pub fn semantic_doc_count(&self) -> u32 {
        self.llm_docs.len().min(u32::MAX as usize) as u32
    }

    #[cfg(test)]
    pub(crate) fn snapshot_content_version(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for value in [
            self.symbols.len() as u64,
            self.llm_docs.len() as u64,
            self.quantized_llm_docs.len() as u64,
            u64::from(self.semantic_doc_count()),
            self.full_text_doc_count() as u64,
            self.stored_vector_encoding as u8 as u64,
        ] {
            hash ^= value;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    pub fn embed_text_refs(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let runtime = self
            .embedding_runtime
            .as_ref()
            .ok_or_else(|| anyhow!("embedding runtime is not configured"))?;
        runtime.embed_text_refs(texts)
    }

    pub fn index_llm_symbol_docs(&mut self, docs: Vec<LlmSearchDoc>) {
        self.llm_docs.clear();
        #[cfg(test)]
        self.quantized_llm_docs.clear();
        for doc in docs {
            self.insert_llm_symbol_doc(doc);
        }
    }

    pub fn clear_llm_symbol_docs(&mut self) {
        self.llm_docs.clear();
        #[cfg(test)]
        self.quantized_llm_docs.clear();
    }

    pub fn extend_llm_symbol_docs<I>(&mut self, docs: I)
    where
        I: IntoIterator<Item = LlmSearchDoc>,
    {
        for doc in docs {
            self.insert_llm_symbol_doc(doc);
        }
    }

    fn insert_llm_symbol_doc(&mut self, doc: LlmSearchDoc) {
        let node_id = doc.node_id;
        #[cfg(test)]
        {
            if let Some(quantized) = self.stored_vector_encoding.quantize(&doc.embedding) {
                self.quantized_llm_docs.insert(node_id, quantized);
            } else {
                self.quantized_llm_docs.remove(&node_id);
            }
        }
        self.llm_docs.insert(node_id, doc);
    }

    #[cfg(test)]
    fn semantic_scores(
        &self,
        query_embedding: &[f32],
        semantic_limit: usize,
    ) -> Vec<(NodeId, f32)> {
        self.hybrid_search_state()
            .semantic_scores_with_role_preference(query_embedding, semantic_limit, false)
    }

    #[cfg(test)]
    fn semantic_scores_for_query(
        &self,
        query: &str,
        query_embedding: &[f32],
        semantic_limit: usize,
    ) -> Vec<(NodeId, f32)> {
        self.hybrid_search_state()
            .semantic_scores_for_query(query, query_embedding, semantic_limit)
    }

    pub fn index_nodes(&mut self, nodes: Vec<(NodeId, String)>) -> Result<()> {
        if !self.full_text_index_enabled {
            self.symbols.extend(
                nodes
                    .into_iter()
                    .map(|(id, name)| (Utf32String::from(name.as_str()), id)),
            );
            return Ok(());
        }

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
    pub fn search_symbol(&self, query: &str) -> Vec<NodeId> {
        if query.is_empty() {
            return Vec::new();
        }
        self.search_symbol_with_scores(query)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    pub fn search_symbol_with_scores(&self, query: &str) -> Vec<(NodeId, f32)> {
        search_symbols_with_scores(&self.symbols, query)
    }

    #[cfg(test)]
    pub fn search_hybrid_with_scores(
        &mut self,
        query: &str,
        graph_boosts: &HashMap<NodeId, f32>,
        config: HybridSearchConfig,
    ) -> Result<Vec<HybridSearchHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let semantic_weight = config.semantic_weight.clamp(0.0, 1.0);
        let query_embedding = if semantic_weight > f32::EPSILON {
            if !self.semantic_index_ready() {
                return Err(anyhow!(
                    "semantic retrieval is required but embedding runtime or semantic index is unavailable"
                ));
            }
            Some(self.embed_query_cached(query)?)
        } else {
            None
        };
        self.hybrid_search_state()
            .search_hybrid_with_query_embedding(
                query,
                query_embedding.as_deref(),
                graph_boosts,
                config,
            )
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
        self.quantized_llm_docs
            .retain(|id, _| !remove_ids.contains(&id.0));

        if !self.full_text_index_enabled {
            return Ok(());
        }

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
        if !self.full_text_index_enabled {
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

        let top_docs = searcher.search(&query, &TopDocs::with_limit(20).order_by_score())?;

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

pub(crate) fn search_symbols_with_scores(
    symbols: &[(Utf32String, NodeId)],
    query: &str,
) -> Vec<(NodeId, f32)> {
    if query.is_empty() {
        return Vec::new();
    }
    if !crate::agent::nucleo_policy::nucleo_full_scan_enabled() {
        return Vec::new();
    }

    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    const SYMBOL_SCAN_CHUNK: usize = 256;
    let mut matches = if symbols.len() >= SYMBOL_SCAN_CHUNK {
        symbols
            .par_chunks(SYMBOL_SCAN_CHUNK)
            .flat_map(|chunk| {
                let mut matcher = Matcher::new(NucleoConfig::DEFAULT);
                chunk
                    .iter()
                    .filter_map(|(name, id)| {
                        pattern
                            .score(name.slice(..), &mut matcher)
                            .map(|score| (*id, score, symbol_candidate_rank(query, name, score)))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    } else {
        let mut matcher = Matcher::new(NucleoConfig::DEFAULT);
        symbols
            .iter()
            .filter_map(|(name, id)| {
                pattern
                    .score(name.slice(..), &mut matcher)
                    .map(|score| (*id, score, symbol_candidate_rank(query, name, score)))
            })
            .collect::<Vec<_>>()
    };

    matches.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| right.1.cmp(&left.1)));

    let mut seen = HashSet::new();
    matches
        .into_iter()
        .map(|(id, score, _)| (id, score as f32))
        .filter(|(id, _)| seen.insert(*id))
        .take(200)
        .collect()
}

fn symbol_candidate_rank(query: &str, name: &Utf32String, score: u32) -> SymbolCandidateRank {
    let query = query.trim().to_ascii_lowercase();
    let display = name.to_string();
    let display_lower = display.to_ascii_lowercase();
    let terminal_lower = display
        .rsplit([':', '.', '/', '\\'])
        .next()
        .unwrap_or(display.as_str())
        .to_ascii_lowercase();
    let leading_lower = display
        .split("::")
        .next()
        .unwrap_or(display.as_str())
        .to_ascii_lowercase();

    SymbolCandidateRank {
        exact_display: u8::from(display_lower == query),
        exact_terminal: u8::from(terminal_lower == query),
        exact_leading: u8::from(leading_lower == query),
        fuzzy_score: score,
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

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct HybridSearchState {
    symbols: Arc<Vec<(Utf32String, NodeId)>>,
    llm_docs: Arc<HashMap<NodeId, LlmSearchDoc>>,
    quantized_llm_docs: Arc<HashMap<NodeId, QuantizedEmbedding>>,
    stored_vector_encoding: StoredVectorEncoding,
}

#[cfg(test)]
impl HybridSearchState {
    pub fn from_engine(engine: &SearchEngine) -> Self {
        Self {
            symbols: Arc::new(engine.symbols().to_vec()),
            llm_docs: Arc::new(engine.llm_docs.clone()),
            quantized_llm_docs: Arc::new(engine.quantized_llm_docs.clone()),
            stored_vector_encoding: engine.stored_vector_encoding,
        }
    }

    pub fn symbols(&self) -> &[(Utf32String, NodeId)] {
        self.symbols.as_slice()
    }

    pub fn semantic_index_ready(&self) -> bool {
        !self.llm_docs.is_empty()
    }

    pub fn semantic_scores_for_query(
        &self,
        query: &str,
        query_embedding: &[f32],
        semantic_limit: usize,
    ) -> Vec<(NodeId, f32)> {
        self.semantic_scores_with_role_preference(
            query_embedding,
            semantic_limit,
            !query_mentions_non_primary_source(query),
        )
    }

    pub fn semantic_scores_with_role_preference(
        &self,
        query_embedding: &[f32],
        semantic_limit: usize,
        prefer_primary_sources: bool,
    ) -> Vec<(NodeId, f32)> {
        if semantic_limit == 0 {
            return Vec::new();
        }

        let mut scored = if self.stored_vector_encoding == StoredVectorEncoding::Float32 {
            self.llm_docs
                .par_iter()
                .map(|(_, doc)| {
                    let cosine = cosine_similarity(query_embedding, &doc.embedding);
                    (doc.node_id, semantic_score_from_cosine(cosine))
                })
                .collect::<Vec<_>>()
        } else {
            let mut approximate = self
                .llm_docs
                .values()
                .map(|doc| {
                    let cosine = self
                        .quantized_llm_docs
                        .get(&doc.node_id)
                        .map(|embedding| embedding.approximate_cosine(query_embedding))
                        .unwrap_or_else(|| cosine_similarity(query_embedding, &doc.embedding));
                    (doc.node_id, cosine)
                })
                .collect::<Vec<_>>();
            let rescore_limit = semantic_limit
                .saturating_mul(SEMANTIC_QUANTIZED_RESCORE_MULTIPLIER)
                .max(semantic_limit)
                .min(self.llm_docs.len());
            self.truncate_semantic_scores(&mut approximate, rescore_limit, prefer_primary_sources);
            approximate
                .into_iter()
                .take(rescore_limit)
                .filter_map(|(node_id, _)| {
                    self.llm_docs.get(&node_id).map(|doc| {
                        let cosine = cosine_similarity(query_embedding, &doc.embedding);
                        (node_id, semantic_score_from_cosine(cosine))
                    })
                })
                .collect::<Vec<_>>()
        };

        self.truncate_semantic_scores(&mut scored, semantic_limit, prefer_primary_sources);
        scored
    }

    pub fn truncate_semantic_scores(
        &self,
        scored: &mut Vec<(NodeId, f32)>,
        limit: usize,
        prefer_primary_sources: bool,
    ) {
        if !prefer_primary_sources {
            truncate_node_scores(scored, limit);
            return;
        }
        if limit == 0 {
            scored.clear();
            return;
        }
        if scored.len() > limit {
            let pivot = limit - 1;
            scored.select_nth_unstable_by(pivot, |left, right| {
                self.compare_semantic_scores_for_primary_query(left, right)
            });
            scored.truncate(limit);
        }
        scored.sort_by(|left, right| self.compare_semantic_scores_for_primary_query(left, right));
    }

    pub fn compare_semantic_scores_for_primary_query(
        &self,
        left: &(NodeId, f32),
        right: &(NodeId, f32),
    ) -> std::cmp::Ordering {
        let left_non_primary = self
            .llm_docs
            .get(&left.0)
            .map(|doc| doc.file_role.is_non_primary())
            .unwrap_or(false);
        let right_non_primary = self
            .llm_docs
            .get(&right.0)
            .map(|doc| doc.file_role.is_non_primary())
            .unwrap_or(false);

        left_non_primary
            .cmp(&right_non_primary)
            .then_with(|| compare_node_scores_desc(left, right))
    }

    pub fn search_hybrid_with_query_embedding(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        graph_boosts: &HashMap<NodeId, f32>,
        config: HybridSearchConfig,
    ) -> Result<Vec<HybridSearchHit>> {
        let semantic_weight = config.semantic_weight.clamp(0.0, 1.0);
        let semantic_enabled = semantic_weight > f32::EPSILON;

        let negative_terms = explicit_negative_query_terms(query);

        let lexical_matches = search_symbols_with_scores(self.symbols.as_slice(), query);
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

        let semantic_map = if semantic_enabled {
            if !self.semantic_index_ready() {
                return Err(anyhow!(
                    "semantic retrieval is required but embedding runtime or semantic index is unavailable"
                ));
            }
            let query_embedding = query_embedding.ok_or_else(|| {
                anyhow!("semantic retrieval is required but query embedding is unavailable")
            })?;
            let semantic_scored =
                self.semantic_scores_for_query(query, query_embedding, config.semantic_limit);
            semantic_scored
                .iter()
                .take(config.semantic_limit)
                .copied()
                .collect::<HashMap<_, _>>()
        } else {
            HashMap::new()
        };

        let mut candidate_ids = HashSet::new();
        candidate_ids.extend(lexical_map.keys().copied());
        candidate_ids.extend(semantic_map.keys().copied());
        candidate_ids.extend(graph_boosts.keys().copied());

        let lexical_weight = config.lexical_weight.clamp(0.0, 1.0);
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
                let total_score = if self.node_matches_negative_terms(node_id, &negative_terms) {
                    total_score * 0.72
                } else {
                    total_score
                };

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

    pub fn node_matches_negative_terms(&self, node_id: NodeId, negative_terms: &[String]) -> bool {
        if negative_terms.is_empty() {
            return false;
        }

        let mut candidate_text = String::new();
        if let Some(doc) = self.llm_docs.get(&node_id) {
            candidate_text.push_str(&doc.doc_text);
        }
        if let Some((name, _)) = self.symbols.iter().find(|(_, id)| *id == node_id) {
            candidate_text.push(' ');
            candidate_text.push_str(&name.slice(..).to_string());
        }

        text_matches_negative_terms(&candidate_text, negative_terms)
    }
}

#[cfg(test)]
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
fn semantic_score_from_cosine(cosine: f32) -> f32 {
    ((cosine + 1.0) * 0.5).clamp(0.0, 1.0)
}

#[cfg(test)]
fn explicit_negative_query_terms(query: &str) -> Vec<String> {
    let tokens = normalized_alnum_terms(query);
    let mut terms = Vec::new();
    let mut seen = HashSet::new();

    for index in 0..tokens.len() {
        let starts_two_token_phrase = (tokens[index] == "rather"
            && tokens.get(index + 1).is_some_and(|t| t == "than"))
            || (tokens[index] == "instead" && tokens.get(index + 1).is_some_and(|t| t == "of"));
        let start = if tokens[index] == "not" {
            Some(index + 1)
        } else if starts_two_token_phrase {
            Some(index + 2)
        } else {
            None
        };

        let Some(start) = start else {
            continue;
        };
        let mut examined = 0;
        for token in tokens.iter().skip(start) {
            if is_negative_clause_boundary(token) {
                break;
            }
            if is_negative_term_stopword(token) {
                continue;
            }
            examined += 1;
            if is_salient_negative_term(token) && seen.insert(token.clone()) {
                terms.push(token.clone());
            }
            if examined >= 5 {
                break;
            }
        }
    }

    terms
}

#[cfg(test)]
fn text_matches_negative_terms(text: &str, negative_terms: &[String]) -> bool {
    if negative_terms.is_empty() {
        return false;
    }
    let terms = normalized_alnum_terms(text)
        .into_iter()
        .collect::<HashSet<_>>();
    !terms.is_empty() && negative_terms.iter().all(|term| terms.contains(term))
}

#[cfg(test)]
fn normalized_alnum_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            terms.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}

#[cfg(test)]
fn is_negative_clause_boundary(term: &str) -> bool {
    matches!(
        term,
        "but" | "while" | "whereas" | "although" | "though" | "however" | "except"
    )
}

#[cfg(test)]
fn is_negative_term_stopword(term: &str) -> bool {
    matches!(
        term,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "be"
            | "by"
            | "confused"
            | "different"
            | "distinguish"
            | "from"
            | "for"
            | "group"
            | "in"
            | "into"
            | "is"
            | "method"
            | "methods"
            | "not"
            | "of"
            | "on"
            | "or"
            | "outrank"
            | "project"
            | "rather"
            | "replace"
            | "return"
            | "should"
            | "source"
            | "than"
            | "the"
            | "to"
            | "with"
    )
}

#[cfg(test)]
fn is_salient_negative_term(term: &str) -> bool {
    term.len() >= 7 || term.chars().any(|ch| ch.is_ascii_digit())
}

#[cfg(test)]
fn compare_node_scores_desc(left: &(NodeId, f32), right: &(NodeId, f32)) -> std::cmp::Ordering {
    right
        .1
        .total_cmp(&left.1)
        .then_with(|| left.0.cmp(&right.0))
}

#[cfg(test)]
fn truncate_node_scores(scored: &mut Vec<(NodeId, f32)>, limit: usize) {
    if limit == 0 {
        scored.clear();
        return;
    }
    if scored.len() > limit {
        let pivot = limit - 1;
        scored.select_nth_unstable_by(pivot, compare_node_scores_desc);
        scored.truncate(limit);
    }
    scored.sort_by(compare_node_scores_desc);
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

    fn test_axis_embedding(axis: usize) -> Vec<f32> {
        let mut embedding = vec![0.0; EMBEDDING_DIM];
        embedding[axis] = 1.0;
        embedding
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
                if let Some(value) = self.previous.as_deref() {
                    std::env::set_var(self.key, value);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn embedding_profile_defaults_to_bge_base() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::remove(EMBEDDING_PROFILE_ENV);

        let profile = EmbeddingProfile::from_env()?;

        assert_eq!(profile.name, "bge-base-en-v1.5");
        assert_eq!(profile.model_id, "BAAI/bge-base-en-v1.5-local");
        assert_eq!(profile.expected_dim, Some(768));
        Ok(())
    }

    #[test]
    fn mandatory_sidecar_defaults_to_llamacpp_backend_when_backend_is_unset() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _mode = EnvGuard::remove(EMBEDDING_RUNTIME_MODE_ENV);
        let _backend = EnvGuard::remove(EMBEDDING_BACKEND_ENV);
        let _url = EnvGuard::remove(LLAMACPP_EMBEDDINGS_URL_ENV);
        let _real_embeddings = EnvGuard::remove("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS");

        assert_eq!(
            EmbeddingBackendSelection::from_env()?,
            EmbeddingBackendSelection::LlamaCpp
        );

        let _explicit_backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "onnx");
        assert_eq!(
            EmbeddingBackendSelection::from_env()?,
            EmbeddingBackendSelection::Onnx
        );
        Ok(())
    }

    #[test]
    fn onnx_runtime_availability_rejects_corrupt_assets() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempdir()?;
        let model_path = temp.path().join("model.onnx");
        let tokenizer_path = temp.path().join("tokenizer.json");
        std::fs::write(&model_path, b"not an onnx model")?;
        std::fs::write(&tokenizer_path, b"not a tokenizer")?;

        let _mode = EnvGuard::remove(EMBEDDING_RUNTIME_MODE_ENV);
        let _backend = EnvGuard::set(EMBEDDING_BACKEND_ENV, "onnx");
        let _model = EnvGuard::set(ONNX_MODEL_PATH_ENV, &model_path.to_string_lossy());
        let _tokenizer = EnvGuard::set(ONNX_TOKENIZER_PATH_ENV, &tokenizer_path.to_string_lossy());
        let _provider = EnvGuard::set(ONNX_PROVIDER_ENV, "cpu");

        let availability = embedding_runtime_availability_from_env();

        assert!(!availability.available);
        assert!(
            availability
                .fallback_message
                .as_deref()
                .is_some_and(|message| message.contains("failed to load ONNX tokenizer")),
            "availability should report a real ONNX asset load failure: {availability:?}"
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

    #[test]
    fn onnx_hidden_state_pooling_respects_attention_mask() -> Result<()> {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, //
            10.0, 20.0, 30.0, 40.0, //
        ];
        let attention = vec![1, 0, 1, 1];

        let cls = pool_onnx_hidden_state(&data, &[2, 2, 2], None, 2, 2, EmbeddingPooling::Cls)?;
        assert_eq!(cls, vec![vec![1.0, 2.0], vec![10.0, 20.0]]);

        let mean = pool_onnx_hidden_state(
            &data,
            &[2, 2, 2],
            Some(&attention),
            2,
            2,
            EmbeddingPooling::Mean,
        )?;
        assert_eq!(mean, vec![vec![1.0, 2.0], vec![20.0, 30.0]]);

        let last = pool_onnx_hidden_state(
            &data,
            &[2, 2, 2],
            Some(&attention),
            2,
            2,
            EmbeddingPooling::LastToken,
        )?;
        assert_eq!(last, vec![vec![1.0, 2.0], vec![30.0, 40.0]]);
        Ok(())
    }

    #[test]
    fn onnx_rank2_output_is_treated_as_pooled_embeddings() -> Result<()> {
        let data = vec![1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0];

        let embeddings =
            extract_onnx_embeddings(&data, &[2, 4], None, 2, 1, EmbeddingPooling::Cls)?;

        assert_eq!(
            embeddings,
            vec![vec![1.0, 2.0, 3.0, 4.0], vec![10.0, 20.0, 30.0, 40.0]]
        );
        Ok(())
    }

    #[test]
    fn onnx_batch_token_budget_limits_padded_tensor_size() {
        let rows = vec![
            OnnxEncodedRow {
                ids: vec![1; 4],
                attention: vec![1; 4],
                token_types: vec![0; 4],
            },
            OnnxEncodedRow {
                ids: vec![1; 6],
                attention: vec![1; 6],
                token_types: vec![0; 6],
            },
            OnnxEncodedRow {
                ids: vec![1; 6],
                attention: vec![1; 6],
                token_types: vec![0; 6],
            },
        ];

        assert_eq!(onnx_batch_end_for_token_budget(&rows, 0, 12), 2);
        assert_eq!(onnx_batch_end_for_token_budget(&rows, 2, 1), 3);

        let cls_batch = shape_onnx_batch(&rows[..1], EmbeddingPooling::Cls).unwrap();
        assert!(cls_batch.attention_values.is_none());
        let mean_batch = shape_onnx_batch(&rows[..1], EmbeddingPooling::Mean).unwrap();
        assert!(mean_batch.attention_values.is_some());
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
            name: "custom".to_string(),
            model_id: "custom-local".to_string(),
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
            "custom-local|backend=llamacpp|pool=Mean|query_prefix=|document_prefix=doc: |layer_norm=false|truncate_dim=None|expected_dim=Some(3)"
        );
        Ok(())
    }

    #[test]
    #[ignore]
    fn embedding_identity_probe_from_env() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let query = std::env::var("CODESTORY_EMBED_IDENTITY_PROBE_QUERY").unwrap_or_else(|_| {
            "Where is the retrieval sidecar embedding contract enforced?".into()
        });
        let docs = std::env::var("CODESTORY_EMBED_IDENTITY_PROBE_DOCS_JSON")
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .filter(|docs| !docs.is_empty())
            .unwrap_or_else(|| {
                vec![
                    "llama.cpp embedding sidecar is mandatory for product retrieval_mode full."
                        .into(),
                    "Managed ONNX assets are diagnostic until fresh quality evidence exists."
                        .into(),
                    "Hash projection is deterministic but not semantic product readiness.".into(),
                    "Packet and search readiness require full sidecar retrieval evidence.".into(),
                ]
            });

        let load_started = std::time::Instant::now();
        let runtime = match EmbeddingRuntime::from_env() {
            Ok(runtime) => runtime,
            Err(error) => {
                println!(
                    "CODESTORY_EMBEDDING_IDENTITY_PROBE_JSON={}",
                    serde_json::to_string(&serde_json::json!({
                        "ok": false,
                        "failure_text": error.to_string(),
                        "backend_env": std::env::var(EMBEDDING_BACKEND_ENV).ok(),
                        "profile_env": std::env::var(EMBEDDING_PROFILE_ENV).ok(),
                        "model_env": std::env::var(ONNX_MODEL_PATH_ENV).ok(),
                        "tokenizer_env": std::env::var(ONNX_TOKENIZER_PATH_ENV).ok(),
                        "url_env": std::env::var(LLAMACPP_EMBEDDINGS_URL_ENV).ok(),
                    }))?
                );
                return Ok(());
            }
        };
        let model_load_ms = load_started.elapsed().as_secs_f64() * 1000.0;

        let query_started = std::time::Instant::now();
        let query_embedding = runtime.embed_query(&query)?;
        let query_embedding_ms = query_started.elapsed().as_secs_f64() * 1000.0;

        let batch_started = std::time::Instant::now();
        let document_embeddings = runtime.embed_texts(&docs)?;
        let batch_document_embedding_ms = batch_started.elapsed().as_secs_f64() * 1000.0;

        let mut scored = document_embeddings
            .iter()
            .enumerate()
            .map(|(index, embedding)| (index, dot_product(&query_embedding, embedding)))
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });

        let model_path = runtime.model_path();
        let cache_bytes = file_len(model_path)
            .into_iter()
            .chain(
                std::env::var(ONNX_TOKENIZER_PATH_ENV)
                    .ok()
                    .and_then(|path| file_len(std::path::Path::new(&path))),
            )
            .sum::<u64>();
        let cache_bytes = if cache_bytes == 0 {
            None
        } else {
            Some(cache_bytes)
        };

        println!(
            "CODESTORY_EMBEDDING_IDENTITY_PROBE_JSON={}",
            serde_json::to_string(&serde_json::json!({
                "ok": true,
                "model_id": runtime.model_id(),
                "model_path": model_path.to_string_lossy(),
                "model_load_ms": model_load_ms,
                "query_embedding_ms": query_embedding_ms,
                "batch_document_embedding_ms": batch_document_embedding_ms,
                "cache_bytes": cache_bytes,
                "vector_dimension": query_embedding.len(),
                "finite_vector_check": all_finite(&query_embedding) && document_embeddings.iter().all(|vector| all_finite(vector)),
                "l2_normalized_vector_check": l2_normalized(&query_embedding) && document_embeddings.iter().all(|vector| l2_normalized(vector)),
                "document_count": docs.len(),
                "top_k": scored.iter().take(3).map(|(index, score)| serde_json::json!({
                    "index": index,
                    "score": score,
                    "text": docs.get(*index).cloned().unwrap_or_default(),
                })).collect::<Vec<_>>(),
            }))?
        );
        Ok(())
    }

    fn file_len(path: &std::path::Path) -> Option<u64> {
        std::fs::metadata(path).ok().map(|metadata| metadata.len())
    }

    fn dot_product(left: &[f32], right: &[f32]) -> f32 {
        left.iter()
            .zip(right.iter())
            .map(|(left, right)| left * right)
            .sum()
    }

    fn all_finite(vector: &[f32]) -> bool {
        !vector.is_empty() && vector.iter().all(|value| value.is_finite())
    }

    fn l2_normalized(vector: &[f32]) -> bool {
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        (norm - 1.0).abs() <= 0.05
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
    fn exact_symbol_search_prioritizes_exact_display_name_candidates() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        engine.index_nodes(vec![
            (NodeId(1), "StorageAccess::~StorageAccess".to_string()),
            (NodeId(2), "StorageAccess::getFileContent".to_string()),
            (NodeId(3), "ComponentFactory::getStorageAccess".to_string()),
            (NodeId(4), "StorageAccess".to_string()),
        ])?;

        let results = engine.search_symbol("StorageAccess");

        assert_eq!(results.first(), Some(&NodeId(4)));
        Ok(())
    }

    #[test]
    fn symbol_full_text_index_can_be_disabled_for_projection_only_search() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::set(SYMBOL_FULL_TEXT_INDEX_ENV, "false");
        let mut engine = SearchEngine::new(None)?;

        engine.index_nodes(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
        ])?;

        assert_eq!(engine.full_text_doc_count(), 2);
        assert_eq!(engine.search_symbol("Beta"), vec![NodeId(2)]);
        assert!(engine.search_full_text("betasymbol")?.is_empty());

        engine.remove_nodes(&[NodeId(2)])?;
        assert_eq!(engine.full_text_doc_count(), 1);
        assert!(engine.search_symbol("Beta").is_empty());
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
    fn persisted_search_index_lock_is_sibling_and_held_by_engine() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("codestory.search");
        let lock_path = persisted_search_index_lock_path(&search_dir);

        let mut engine = SearchEngine::new(Some(search_dir.as_path()))?;
        assert!(search_dir.exists());
        assert!(lock_path.exists());
        assert!(
            PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
            "new persisted engine should hold an exclusive search-index lock"
        );
        assert!(
            PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path()).is_err(),
            "exclusive search-index lock should block readers while rebuilding"
        );

        engine.index_nodes(vec![(NodeId(1), "Locked Symbol".to_string())])?;
        drop(engine);

        let _guard = PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path())?;
        assert!(
            lock_path.exists(),
            "recreating the search dir must not delete its sibling lock"
        );
        Ok(())
    }

    #[test]
    fn persisted_search_index_rebuild_reuses_existing_lock() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("search");

        let mut engine = SearchEngine::new(Some(search_dir.as_path()))?;
        engine.index_nodes(vec![(NodeId(1), "Before Rebuild".to_string())])?;
        drop(engine);

        let existing = SearchEngine::open_existing(search_dir.as_path())?;
        {
            let _second_reader =
                PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path())?;
            assert!(
                PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
                "open_existing should hold a shared search-index lock"
            );
        }
        assert!(
            PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
            "open_existing should keep writers out while the reader is alive"
        );

        let mut rebuilt =
            SearchEngine::recreate_persisted_from_existing(search_dir.as_path(), existing)?;
        assert!(
            PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
            "rebuild should keep an exclusive search-index lock while recreating the index"
        );
        assert!(
            PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path()).is_err(),
            "exclusive rebuild lock should block readers"
        );
        rebuilt.index_nodes(vec![(NodeId(2), "After Rebuild".to_string())])?;
        assert_eq!(rebuilt.search_full_text("after")?, vec![NodeId(2)]);
        drop(rebuilt);

        let _guard = PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path())?;
        Ok(())
    }

    #[test]
    fn persisted_search_index_open_failure_rebuild_keeps_lock() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("missing.search");

        let (mut engine, open_error) =
            SearchEngine::open_existing_or_recreate(search_dir.as_path())?;
        assert!(open_error.is_some());
        assert!(
            PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
            "open-failure fallback should keep an exclusive search-index lock"
        );
        assert!(
            PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path()).is_err(),
            "open-failure fallback should block readers until rebuilt"
        );

        engine.index_nodes(vec![(NodeId(1), "Recovered Symbol".to_string())])?;
        assert_eq!(engine.search_full_text("recovered")?, vec![NodeId(1)]);
        drop(engine);

        let _guard = PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path())?;
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
                file_role: RetrievalFileRole::Source,
                doc_text: "alpha symbol".to_string(),
                embedding: embed_text_with_hash_projection("alpha symbol", EMBEDDING_DIM),
            },
            LlmSearchDoc {
                node_id: NodeId(2),
                file_role: RetrievalFileRole::Source,
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
                file_role: RetrievalFileRole::Source,
                doc_text: "handles authorization policy and permission checks".to_string(),
                embedding: embed_text_with_hash_projection(
                    "handles authorization policy and permission checks",
                    EMBEDDING_DIM,
                ),
            },
            LlmSearchDoc {
                node_id: NodeId(11),
                file_role: RetrievalFileRole::Source,
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

    #[test]
    fn test_explicit_negative_query_terms_keep_salient_distractors_only() {
        assert_eq!(
            explicit_negative_query_terms(
                "choose the compilation database source group, not the Codeblocks project source group"
            ),
            vec!["codeblocks"]
        );
        assert_eq!(
            explicit_negative_query_terms(
                "choose SourceGroupCxxEmpty rather than the compilation database source group"
            ),
            vec!["compilation", "database"]
        );
    }

    #[test]
    fn test_negative_terms_match_candidate_text_only_when_all_terms_are_present() {
        let negative_terms = explicit_negative_query_terms(
            "choose the Codeblocks source group when project targets produce commands, not compile_commands.json",
        );

        assert!(text_matches_negative_terms(
            "SourceGroupCxxCdb uses compile_commands json compilation database files",
            &negative_terms
        ));
        assert!(!text_matches_negative_terms(
            "SourceGroupCxxCodeblocks reads project targets",
            &negative_terms
        ));
    }

    #[test]
    fn test_hybrid_search_quantized_prefilter_rescores_full_precision() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::set(STORED_VECTOR_ENCODING_ENV, "int8");

        let mut engine = SearchEngine::new(None)?;
        assert_eq!(engine.stored_vector_encoding, StoredVectorEncoding::Int8);
        engine.index_nodes(vec![
            (NodeId(20), "auth_policy".to_string()),
            (NodeId(21), "theme_tokens".to_string()),
        ])?;
        engine.set_embedding_runtime(EmbeddingRuntime::test_runtime());
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(20),
                file_role: RetrievalFileRole::Source,
                doc_text: "authorization policy permission validation".to_string(),
                embedding: embed_text_with_hash_projection(
                    "authorization policy permission validation",
                    EMBEDDING_DIM,
                ),
            },
            LlmSearchDoc {
                node_id: NodeId(21),
                file_role: RetrievalFileRole::Source,
                doc_text: "visual theme color token spacing".to_string(),
                embedding: embed_text_with_hash_projection(
                    "visual theme color token spacing",
                    EMBEDDING_DIM,
                ),
            },
        ]);

        assert_eq!(engine.quantized_llm_docs.len(), 2);
        let hits = engine.search_hybrid_with_scores(
            "permission validation",
            &HashMap::new(),
            HybridSearchConfig {
                max_results: 5,
                lexical_weight: 0.0,
                semantic_weight: 1.0,
                graph_weight: 0.0,
                lexical_limit: 1,
                semantic_limit: 5,
            },
        )?;

        assert!(!hits.is_empty());
        assert_eq!(hits[0].node_id, NodeId(20));
        Ok(())
    }

    #[test]
    fn test_semantic_scores_prefer_primary_docs_for_production_queries() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(30),
                file_role: RetrievalFileRole::Test,
                doc_text: "test fixture with exact authorization policy wording".to_string(),
                embedding: test_axis_embedding(0),
            },
            LlmSearchDoc {
                node_id: NodeId(31),
                file_role: RetrievalFileRole::Source,
                doc_text: "production authorization handler".to_string(),
                embedding: test_axis_embedding(1),
            },
        ]);

        let scored =
            engine.semantic_scores_for_query("authorization policy", &test_axis_embedding(0), 1);

        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, NodeId(31));
        assert!(
            scored[0].1 < 0.99,
            "primary candidate should be retained even when a test doc has a stronger raw vector score"
        );
        Ok(())
    }

    #[test]
    fn test_semantic_scores_keep_requested_non_primary_docs() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(40),
                file_role: RetrievalFileRole::Test,
                doc_text: "test fixture with exact authorization policy wording".to_string(),
                embedding: test_axis_embedding(0),
            },
            LlmSearchDoc {
                node_id: NodeId(41),
                file_role: RetrievalFileRole::Source,
                doc_text: "production authorization handler".to_string(),
                embedding: test_axis_embedding(1),
            },
        ]);

        let scored = engine.semantic_scores_for_query(
            "authorization policy test",
            &test_axis_embedding(0),
            1,
        );

        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, NodeId(40));
        Ok(())
    }

    #[test]
    fn test_quantized_semantic_prefilter_prefers_primary_docs() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::set(STORED_VECTOR_ENCODING_ENV, "int8");

        let mut engine = SearchEngine::new(None)?;
        engine.index_llm_symbol_docs(vec![
            LlmSearchDoc {
                node_id: NodeId(50),
                file_role: RetrievalFileRole::Docs,
                doc_text: "documentation with exact authorization policy wording".to_string(),
                embedding: test_axis_embedding(0),
            },
            LlmSearchDoc {
                node_id: NodeId(51),
                file_role: RetrievalFileRole::Source,
                doc_text: "production authorization handler".to_string(),
                embedding: test_axis_embedding(1),
            },
        ]);

        let scored =
            engine.semantic_scores_for_query("authorization policy", &test_axis_embedding(0), 1);

        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, NodeId(51));
        Ok(())
    }

    #[test]
    fn test_float32_semantic_scores_return_bounded_top_candidates() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::set(STORED_VECTOR_ENCODING_ENV, "float32");

        fn axis_embedding(axis: usize) -> Vec<f32> {
            let mut embedding = vec![0.0; EMBEDDING_DIM];
            embedding[axis] = 1.0;
            embedding
        }

        let mut engine = SearchEngine::new(None)?;
        let docs = (0..8)
            .map(|axis| LlmSearchDoc {
                node_id: NodeId(20 + axis as i64),
                file_role: RetrievalFileRole::Source,
                doc_text: format!("doc {axis}"),
                embedding: axis_embedding(axis),
            })
            .collect();
        engine.index_llm_symbol_docs(docs);

        let scored = engine.semantic_scores(&axis_embedding(0), 1);

        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, NodeId(20));
        assert!(engine.semantic_scores(&axis_embedding(0), 0).is_empty());
        Ok(())
    }

    #[test]
    fn test_quantized_semantic_scores_return_bounded_top_candidates() -> Result<()> {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = EnvGuard::set(STORED_VECTOR_ENCODING_ENV, "int8");

        fn axis_embedding(axis: usize) -> Vec<f32> {
            let mut embedding = vec![0.0; EMBEDDING_DIM];
            embedding[axis] = 1.0;
            embedding
        }

        let mut engine = SearchEngine::new(None)?;
        let docs = (0..8)
            .map(|axis| LlmSearchDoc {
                node_id: NodeId(20 + axis as i64),
                file_role: RetrievalFileRole::Source,
                doc_text: format!("doc {axis}"),
                embedding: axis_embedding(axis),
            })
            .collect();
        engine.index_llm_symbol_docs(docs);

        let scored = engine.semantic_scores(&axis_embedding(0), 1);

        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, NodeId(20));
        Ok(())
    }
}
