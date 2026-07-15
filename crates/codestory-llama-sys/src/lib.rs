//! CodeStory-owned boundary around the statically linked llama.cpp runtime.

use crossbeam_channel::{Receiver, Sender, bounded, select_biased, unbounded};
use fs4::fs_std::FileExt;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{LlamaAttentionType, LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::{
    LlamaBackendDevice, LlamaBackendDeviceType, LogOptions, list_llama_ggml_backend_devices,
    send_logs_to_tracing,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

include!(concat!(env!("OUT_DIR"), "/embedded_model.rs"));
include!(concat!(env!("OUT_DIR"), "/model_contract.rs"));

const EMBEDDING_DIM: usize = 768;
const MODEL_CONTEXT_TOKENS: usize = 512;
const LOGICAL_BATCH_TOKENS: usize = 1024;
const MAX_BATCH_SEQUENCES: usize = 6;
const REQUEST_QUEUE_CAPACITY: usize = 64;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("the CodeStory executable was built without its embedded embedding model")]
    ModelNotEmbedded,
    #[error("embedding model cache error: {0}")]
    ModelCache(String),
    #[error(
        "no supported {expected} accelerator was found; set CODESTORY_EMBED_ALLOW_CPU=1 only for intentional CPU execution"
    )]
    AcceleratorUnavailable { expected: &'static str },
    #[error("software adapter `{0}` is not allowed for production embeddings")]
    SoftwareAdapter(String),
    #[error("the loaded model did not prove execution on `{0}`")]
    AcceleratorExecutionUnverified(String),
    #[error("llama.cpp initialization failed: {0}")]
    Llama(String),
    #[error("embedding input is empty")]
    EmptyInput,
    #[error("embedding input contains {actual} tokens; maximum is {maximum}")]
    InputTooLong { actual: usize, maximum: usize },
    #[error("embedding engine worker unavailable: {0}")]
    WorkerUnavailable(String),
    #[error("llama.cpp returned {actual} dimensions; expected {expected}")]
    Dimension { expected: usize, actual: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPolicy {
    Accelerated,
    CpuExplicit,
}

impl ExecutionPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accelerated => "accelerated",
            Self::CpuExplicit => "cpu_explicit",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineIdentity {
    pub model_digest: &'static str,
    pub ggml_build_identity: &'static str,
    pub backend: String,
    pub adapter_name: String,
    pub adapter_description: String,
    pub policy: ExecutionPolicy,
    pub embedded_model: bool,
    pub materialized_path: PathBuf,
    pub materialized_reused: bool,
    pub initialization_duration: Duration,
    pub smoke_duration: Duration,
    pub adapter_memory_total: usize,
    pub adapter_memory_free_before_load: usize,
    pub adapter_memory_free_after_load: usize,
    pub execution_device_names: Vec<String>,
    pub model_layer_count: u32,
    pub offloaded_layer_count: u32,
    pub accelerator_execution_verified: bool,
}

#[derive(Debug, Clone)]
pub struct MaterializedModel {
    pub path: PathBuf,
    pub reused: bool,
}

#[derive(Clone)]
pub struct EmbeddingEngine {
    shared: Arc<EngineShared>,
}

impl std::fmt::Debug for EmbeddingEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmbeddingEngine")
            .field("identity", &self.shared.identity)
            .finish_non_exhaustive()
    }
}

struct EngineShared {
    query_sender: Sender<EmbeddingRequest>,
    bulk_sender: Sender<EmbeddingRequest>,
    control_sender: Sender<Control>,
    identity: EngineIdentity,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for EngineShared {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl EngineShared {
    fn stop_worker(&self) {
        let _ = self.control_sender.send(Control::Shutdown);
        let worker = match self.worker.lock() {
            Ok(mut worker) => worker.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

struct EmbeddingRequest {
    inputs: Vec<String>,
    response: Sender<Result<Vec<Vec<f32>>, EngineError>>,
}

enum Control {
    Shutdown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestPriority {
    Query,
    Bulk,
}

impl EmbeddingEngine {
    pub fn initialize(cache_root: &Path, allow_cpu: bool) -> Result<Self, EngineError> {
        let (query_sender, query_receiver) = bounded(REQUEST_QUEUE_CAPACITY);
        let (bulk_sender, bulk_receiver) = bounded(REQUEST_QUEUE_CAPACITY);
        let (control_sender, control_receiver) = unbounded();
        let (startup_sender, startup_receiver) = bounded(1);
        let cache_root = cache_root.to_path_buf();
        let worker = thread::Builder::new()
            .name("codestory-embedding-engine".into())
            .spawn(move || {
                if let Err(error) = run_engine_owner(
                    &cache_root,
                    allow_cpu,
                    &startup_sender,
                    &query_receiver,
                    &bulk_receiver,
                    &control_receiver,
                ) {
                    let _ = startup_sender.try_send(Err(error));
                }
            })
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?;
        let identity = match startup_receiver.recv() {
            Ok(Ok(identity)) => identity,
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(error) => {
                let _ = worker.join();
                return Err(EngineError::WorkerUnavailable(error.to_string()));
            }
        };
        Ok(Self {
            shared: Arc::new(EngineShared {
                query_sender,
                bulk_sender,
                control_sender,
                identity,
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    pub fn identity(&self) -> &EngineIdentity {
        &self.shared.identity
    }

    pub fn embed_query_prepared(&self, input: String) -> Result<Vec<f32>, EngineError> {
        let mut vectors = self.request(vec![input], RequestPriority::Query)?;
        vectors
            .pop()
            .ok_or_else(|| EngineError::Llama("embedding worker returned no query vector".into()))
    }

    pub fn embed_documents_prepared(
        &self,
        inputs: &[String],
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        self.request(inputs.to_vec(), RequestPriority::Bulk)
    }

    pub fn embed_prepared(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EngineError> {
        self.embed_documents_prepared(inputs)
    }

    fn request(
        &self,
        inputs: Vec<String>,
        priority: RequestPriority,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let (response, result) = bounded(1);
        let request = EmbeddingRequest { inputs, response };
        let sender = match priority {
            RequestPriority::Query => &self.shared.query_sender,
            RequestPriority::Bulk => &self.shared.bulk_sender,
        };
        sender
            .send(request)
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?;
        result
            .recv()
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?
    }
}

fn run_engine_owner(
    cache_root: &Path,
    allow_cpu: bool,
    startup: &Sender<Result<EngineIdentity, EngineError>>,
    query_receiver: &Receiver<EmbeddingRequest>,
    bulk_receiver: &Receiver<EmbeddingRequest>,
    control_receiver: &Receiver<Control>,
) -> Result<(), EngineError> {
    let started = Instant::now();
    let materialized = materialize_embedded_model(cache_root)?;
    send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    let backend = LlamaBackend::init().map_err(llama_error)?;
    let devices = list_llama_ggml_backend_devices();
    let (device, policy) = select_device(&devices, allow_cpu)?;
    let free_before = device.memory_free;

    let mut model_params = LlamaModelParams::default().with_use_mmap(true);
    if policy == ExecutionPolicy::Accelerated {
        model_params = model_params
            .with_devices(&[device.index])
            .map_err(llama_error)?
            .with_n_gpu_layers(u32::MAX);
    } else {
        model_params = model_params.with_n_gpu_layers(0);
    }
    let model = LlamaModel::load_from_file(&backend, &materialized.path, &model_params)
        .map_err(llama_error)?;
    if model.n_embd() as usize != EMBEDDING_DIM {
        return Err(EngineError::Dimension {
            expected: EMBEDDING_DIM,
            actual: model.n_embd() as usize,
        });
    }
    let model_layer_count = model.n_layer() + 1;

    let context_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(4096))
        .with_n_batch(LOGICAL_BATCH_TOKENS as u32)
        .with_n_ubatch(LOGICAL_BATCH_TOKENS as u32)
        .with_n_seq_max(MAX_BATCH_SEQUENCES as u32)
        .with_attention_type(LlamaAttentionType::NonCausal)
        .with_pooling_type(LlamaPoolingType::Cls)
        .with_embeddings(true);
    let mut context = model
        .new_context(&backend, context_params)
        .map_err(llama_error)?;
    let free_after = list_llama_ggml_backend_devices()
        .into_iter()
        .find(|candidate| candidate.index == device.index)
        .map_or(device.memory_free, |candidate| candidate.memory_free);
    // llama.cpp receives one explicit physical device and n_gpu_layers=u32::MAX.
    // A successful load with a real allocation on that device proves the full
    // layer request was realized; llama.cpp does not silently lower this value.
    let accelerator_execution_verified =
        policy == ExecutionPolicy::Accelerated && free_before > free_after && free_after > 0;
    if policy == ExecutionPolicy::Accelerated && !accelerator_execution_verified {
        return Err(EngineError::AcceleratorExecutionUnverified(format!(
            "{} ({})",
            device.name, device.description
        )));
    }
    let offloaded_layer_count = if accelerator_execution_verified {
        model_layer_count
    } else {
        0
    };

    let smoke_started = Instant::now();
    let smoke = embed_inputs(
        &model,
        &mut context,
        &[
            "Represent this query for searching relevant code: codestory embedding smoke"
                .to_string(),
        ],
        RequestPriority::Query,
        query_receiver,
    )?;
    if smoke
        .first()
        .is_none_or(|vector| vector.len() != EMBEDDING_DIM)
    {
        return Err(EngineError::Dimension {
            expected: EMBEDDING_DIM,
            actual: smoke.first().map_or(0, Vec::len),
        });
    }
    let identity = EngineIdentity {
        model_digest: MODEL_SHA256,
        ggml_build_identity: GGML_BUILD_IDENTITY,
        backend: device.backend.clone(),
        adapter_name: device.name.clone(),
        adapter_description: device.description.clone(),
        policy,
        embedded_model: EMBEDDED_MODEL_COMPILED,
        materialized_path: materialized.path,
        materialized_reused: materialized.reused,
        initialization_duration: started.elapsed(),
        smoke_duration: smoke_started.elapsed(),
        adapter_memory_total: device.memory_total,
        adapter_memory_free_before_load: free_before,
        adapter_memory_free_after_load: free_after,
        execution_device_names: if accelerator_execution_verified {
            vec![device.name.clone()]
        } else {
            Vec::new()
        },
        model_layer_count,
        offloaded_layer_count,
        accelerator_execution_verified,
    };
    startup
        .send(Ok(identity))
        .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?;

    loop {
        select_biased! {
            recv(control_receiver) -> _ => break,
            recv(query_receiver) -> request => match request {
                Ok(request) => handle_request(
                    request,
                    RequestPriority::Query,
                    &model,
                    &mut context,
                    query_receiver,
                ),
                Err(_) => break,
            },
            recv(bulk_receiver) -> request => match request {
                Ok(request) => handle_request(
                    request,
                    RequestPriority::Bulk,
                    &model,
                    &mut context,
                    query_receiver,
                ),
                Err(_) => break,
            },
        }
    }
    Ok(())
}

fn handle_request(
    request: EmbeddingRequest,
    priority: RequestPriority,
    model: &LlamaModel,
    context: &mut LlamaContext<'_>,
    query_receiver: &Receiver<EmbeddingRequest>,
) {
    let result = embed_inputs(model, context, &request.inputs, priority, query_receiver);
    let _ = request.response.send(result);
}

fn embed_inputs(
    model: &LlamaModel,
    context: &mut LlamaContext<'_>,
    inputs: &[String],
    priority: RequestPriority,
    query_receiver: &Receiver<EmbeddingRequest>,
) -> Result<Vec<Vec<f32>>, EngineError> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    if inputs.iter().any(|input| input.trim().is_empty()) {
        return Err(EngineError::EmptyInput);
    }
    let tokenized = inputs
        .iter()
        .map(|input| tokenize(model, input))
        .collect::<Result<Vec<_>, _>>()?;
    let mut output = Vec::with_capacity(inputs.len());
    let mut offset = 0;
    while offset < tokenized.len() {
        if priority == RequestPriority::Bulk {
            while let Ok(query) = query_receiver.try_recv() {
                handle_request(
                    query,
                    RequestPriority::Query,
                    model,
                    context,
                    query_receiver,
                );
            }
        }
        let end = batch_end(&tokenized, offset);
        embed_token_batch(context, &tokenized[offset..end], &mut output)?;
        offset = end;
    }
    Ok(output)
}

fn batch_end(tokenized: &[Vec<llama_cpp_2::token::LlamaToken>], offset: usize) -> usize {
    let mut end = offset;
    let mut tokens = 0;
    while end < tokenized.len() && end - offset < MAX_BATCH_SEQUENCES {
        let next = tokenized[end].len();
        if end > offset && tokens + next > LOGICAL_BATCH_TOKENS {
            break;
        }
        tokens += next;
        end += 1;
    }
    end
}

fn embed_token_batch(
    context: &mut LlamaContext<'_>,
    sequences: &[Vec<llama_cpp_2::token::LlamaToken>],
    output: &mut Vec<Vec<f32>>,
) -> Result<(), EngineError> {
    let total_tokens = sequences.iter().map(Vec::len).sum();
    let mut batch = LlamaBatch::new(total_tokens, sequences.len() as i32);
    for (sequence_id, tokens) in sequences.iter().enumerate() {
        batch
            .add_sequence(tokens, sequence_id as i32, false)
            .map_err(llama_error)?;
    }
    context.clear_kv_cache();
    context.encode(&mut batch).map_err(llama_error)?;
    for sequence_id in 0..sequences.len() {
        let mut vector = context
            .embeddings_seq_ith(sequence_id as i32)
            .map_err(llama_error)?
            .to_vec();
        if vector.len() != EMBEDDING_DIM {
            return Err(EngineError::Dimension {
                expected: EMBEDDING_DIM,
                actual: vector.len(),
            });
        }
        l2_normalize(&mut vector);
        output.push(vector);
    }
    Ok(())
}

pub fn materialize_embedded_model(cache_root: &Path) -> Result<MaterializedModel, EngineError> {
    if !EMBEDDED_MODEL_COMPILED || EMBEDDED_MODEL_BYTES.is_empty() {
        return Err(EngineError::ModelNotEmbedded);
    }
    if EMBEDDED_MODEL_BYTES.len() as u64 != MODEL_SIZE {
        return Err(EngineError::ModelCache(format!(
            "embedded size {} does not match {MODEL_SIZE}",
            EMBEDDED_MODEL_BYTES.len()
        )));
    }

    let directory = cache_root
        .join("embedded-models")
        .join("sha256")
        .join(MODEL_SHA256);
    fs::create_dir_all(&directory).map_err(cache_error)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".materialize.lock"))
        .map_err(cache_error)?;
    FileExt::lock_exclusive(&lock).map_err(cache_error)?;
    let result = materialize_embedded_model_locked(&directory);
    let unlock = FileExt::unlock(&lock).map_err(cache_error);
    match (result, unlock) {
        (Ok(reused), Ok(())) => Ok(MaterializedModel {
            path: directory.join(MODEL_FILE_NAME),
            reused,
        }),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn materialize_embedded_model_locked(directory: &Path) -> Result<bool, EngineError> {
    let path = directory.join(MODEL_FILE_NAME);
    if verified_model_file(&path)? {
        return Ok(true);
    }
    if fs::symlink_metadata(&path).is_ok() {
        fs::remove_file(&path).map_err(cache_error)?;
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = directory.join(format!(
        ".{MODEL_FILE_NAME}.{}.{}.partial",
        std::process::id(),
        nonce + u128::from(sequence)
    ));
    let write_result = (|| -> Result<(), EngineError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(cache_error)?;
        file.write_all(EMBEDDED_MODEL_BYTES).map_err(cache_error)?;
        file.sync_all().map_err(cache_error)?;
        if sha256_file(&temp)? != MODEL_SHA256 {
            return Err(EngineError::ModelCache(
                "materialized model digest did not match embedded digest".into(),
            ));
        }
        match fs::rename(&temp, &path) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                ) && verified_model_file(&path)? =>
            {
                fs::remove_file(&temp).map_err(cache_error)?;
            }
            Err(error) => return Err(cache_error(error)),
        }
        sync_directory(directory)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result?;
    Ok(false)
}

fn select_device(
    devices: &[LlamaBackendDevice],
    allow_cpu: bool,
) -> Result<(LlamaBackendDevice, ExecutionPolicy), EngineError> {
    if allow_cpu {
        let device = devices
            .iter()
            .find(|device| device.device_type == LlamaBackendDeviceType::Cpu)
            .cloned()
            .ok_or_else(|| EngineError::Llama("llama.cpp exposed no CPU device".into()))?;
        return Ok((device, ExecutionPolicy::CpuExplicit));
    }

    let expected = expected_accelerator_backend();
    let matching = devices
        .iter()
        .filter(|device| {
            backend_matches_expected(&device.backend, expected)
                && matches!(
                    device.device_type,
                    LlamaBackendDeviceType::Gpu
                        | LlamaBackendDeviceType::IntegratedGpu
                        | LlamaBackendDeviceType::Accelerator
                )
        })
        .collect::<Vec<_>>();
    if let Some(device) = matching
        .iter()
        .copied()
        .find(|device| !is_software_adapter(device))
    {
        return Ok((device.clone(), ExecutionPolicy::Accelerated));
    }
    if !matching.is_empty() {
        return Err(EngineError::SoftwareAdapter(
            matching
                .iter()
                .map(|device| format!("{} ({})", device.name, device.description))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    Err(EngineError::AcceleratorUnavailable { expected })
}

fn backend_matches_expected(actual: &str, expected: &str) -> bool {
    let actual = actual.trim().to_ascii_lowercase();
    match expected {
        "Metal" => actual == "metal" || actual == "mtl",
        "Vulkan" => actual == "vulkan" || actual.starts_with("vulkan"),
        _ => actual.eq_ignore_ascii_case(expected),
    }
}

fn expected_accelerator_backend() -> &'static str {
    if cfg!(target_os = "macos") {
        "Metal"
    } else {
        "Vulkan"
    }
}

fn is_software_adapter(device: &LlamaBackendDevice) -> bool {
    let value =
        format!("{} {} {}", device.backend, device.name, device.description).to_ascii_lowercase();
    [
        "llvmpipe",
        "lavapipe",
        "swiftshader",
        "warp",
        "software rasterizer",
        "microsoft basic render driver",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn tokenize(
    model: &LlamaModel,
    input: &str,
) -> Result<Vec<llama_cpp_2::token::LlamaToken>, EngineError> {
    let tokens = model
        .str_to_token(input, AddBos::Always)
        .map_err(llama_error)?;
    if tokens.len() > MODEL_CONTEXT_TOKENS {
        return Err(EngineError::InputTooLong {
            actual: tokens.len(),
            maximum: MODEL_CONTEXT_TOKENS,
        });
    }
    Ok(tokens)
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
    for value in vector {
        *value *= scale;
    }
}

fn verified_model_file(path: &Path) -> Result<bool, EngineError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(cache_error(error)),
    };
    if !metadata.file_type().is_file() || metadata.len() != MODEL_SIZE {
        return Ok(false);
    }
    Ok(sha256_file(path)? == MODEL_SHA256)
}

fn sha256_file(path: &Path) -> Result<String, EngineError> {
    let mut reader = BufReader::new(File::open(path).map_err(cache_error)?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(cache_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), EngineError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(cache_error)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), EngineError> {
    Ok(())
}

fn llama_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::Llama(error.to_string())
}

fn cache_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::ModelCache(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_adapters_are_rejected_by_name_or_description() {
        for marker in [
            "llvmpipe",
            "lavapipe",
            "SwiftShader",
            "WARP",
            "Software Rasterizer",
        ] {
            let device = LlamaBackendDevice {
                index: 0,
                name: marker.into(),
                description: marker.into(),
                backend: "Vulkan".into(),
                memory_total: 1,
                memory_free: 1,
                device_type: LlamaBackendDeviceType::Gpu,
            };
            assert!(is_software_adapter(&device));
        }
    }
}
