//! CodeStory-owned boundary around the packaged llama.cpp runtime.

mod admission;

pub use admission::{
    EMBEDDING_BULK_QUEUE_CAPACITY, EMBEDDING_QUERY_QUEUE_CAPACITY, EmbeddingActiveRequestSnapshot,
    EmbeddingAdmissionSnapshot, EmbeddingCapacityPressure, EmbeddingCapacityReason,
    EmbeddingOwnerState, EmbeddingRequestClass, EmbeddingRequestContext,
};

use admission::EmbeddingAdmissionTracker;
use crossbeam_channel::{Receiver, Sender, TryRecvError, after, bounded, select_biased, unbounded};
use fs4::fs_std::FileExt;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{LlamaAttentionType, LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use llama_cpp_2::llama_backend::load_backends_from_path;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::{
    LlamaBackendDevice, LlamaBackendDeviceType, LogOptions, list_llama_ggml_backend_devices,
    send_logs_to_tracing,
};
use llama_cpp_sys_2 as llama_sys;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::ffi::{CStr, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

include!(concat!(env!("OUT_DIR"), "/embedded_model.rs"));
include!(concat!(env!("OUT_DIR"), "/model_contract.rs"));
include!(concat!(env!("OUT_DIR"), "/embedding_server_contract.rs"));

const ENGINE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static QUALIFICATION_NATIVE_STALL: AtomicBool = AtomicBool::new(false);
static QUALIFICATION_QUERY_HOLD: AtomicBool = AtomicBool::new(false);
static QUALIFICATION_BULK_HOLD: AtomicBool = AtomicBool::new(false);
const EXECUTION_OBSERVATION_SOURCE: &str = "ggml_eval_callback";

#[doc(hidden)]
pub fn set_embedding_qualification_native_stall(stalled: bool) {
    if embedding_qualification_gate_open() {
        QUALIFICATION_NATIVE_STALL.store(stalled, Ordering::Release);
    }
}

#[doc(hidden)]
pub fn set_embedding_qualification_class_hold(request_class: EmbeddingRequestClass, held: bool) {
    if !embedding_qualification_gate_open() {
        return;
    }
    match request_class {
        EmbeddingRequestClass::Query => QUALIFICATION_QUERY_HOLD.store(held, Ordering::Release),
        EmbeddingRequestClass::Bulk => QUALIFICATION_BULK_HOLD.store(held, Ordering::Release),
    }
}

fn embedding_qualification_gate_open() -> bool {
    std::env::var_os("CODESTORY_EMBED_QUALIFICATION_DIR").is_some_and(|value| !value.is_empty())
        && std::env::var_os("CODESTORY_EMBED_QUALIFICATION_NONCE")
            .is_some_and(|value| !value.is_empty())
}

fn embedding_qualification_request_held(request_class: EmbeddingRequestClass) -> bool {
    QUALIFICATION_NATIVE_STALL.load(Ordering::Acquire)
        || match request_class {
            EmbeddingRequestClass::Query => QUALIFICATION_QUERY_HOLD.load(Ordering::Acquire),
            EmbeddingRequestClass::Bulk => QUALIFICATION_BULK_HOLD.load(Ordering::Acquire),
        }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn load_packaged_backend_modules() -> Result<(), EngineError> {
    let executable = std::env::current_exe().map_err(|error| {
        EngineError::Llama(format!(
            "could not resolve the executable containing native backend modules: {error}"
        ))
    })?;
    let directory = executable.parent().ok_or_else(|| {
        EngineError::Llama(format!(
            "native backend module directory is unavailable for {}",
            executable.display()
        ))
    })?;
    load_backends_from_path(directory);
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn load_packaged_backend_modules() -> Result<(), EngineError> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("the CodeStory executable was built without its embedded embedding model")]
    ModelNotEmbedded,
    #[error("embedding model cache error: {0}")]
    ModelCache(String),
    #[error("requested model `{requested}` does not match compiled model `{compiled}`")]
    ModelRequestMismatch { requested: String, compiled: String },
    #[error("native engine configuration is invalid: {reason}")]
    InvalidConfiguration { reason: &'static str },
    #[error(
        "requested backend `{requested}` is not compiled into this binary; compiled backends: {compiled}"
    )]
    BackendNotCompiled { requested: String, compiled: String },
    #[error("requested backend `{requested}` is not available in the loaded native runtime")]
    BackendUnavailable { requested: String },
    #[error("software adapter `{0}` was rejected by the native backend request")]
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
    #[error(
        "embedding {class} queue is full (depth={depth} capacity={capacity}); retry when {retry_condition}",
        class = pressure.request_class.as_str(),
        depth = pressure.depth,
        capacity = pressure.capacity,
        retry_condition = pressure.retry_condition
    )]
    AdmissionFull { pressure: EmbeddingCapacityPressure },
    #[error("embedding request was cancelled before its result committed")]
    Cancelled,
}

impl EngineError {
    /// Stable machine-readable reason for fail-closed native boundary errors.
    pub const fn reason_code(&self) -> &'static str {
        match self {
            Self::ModelNotEmbedded => "native_model_not_embedded",
            Self::ModelCache(_) => "native_model_cache_error",
            Self::ModelRequestMismatch { .. } => "native_model_request_mismatch",
            Self::InvalidConfiguration { .. } => "native_engine_config_invalid",
            Self::BackendNotCompiled { .. } => "native_backend_not_compiled",
            Self::BackendUnavailable { .. } => "native_backend_unavailable",
            Self::SoftwareAdapter(_) => "native_software_adapter_rejected",
            Self::AcceleratorExecutionUnverified(_) => "native_accelerator_execution_unverified",
            Self::Llama(_) => "native_engine_initialization_failed",
            Self::EmptyInput => "native_embedding_input_empty",
            Self::InputTooLong { .. } => "native_embedding_input_too_long",
            Self::WorkerUnavailable(_) => "native_engine_worker_unavailable",
            Self::Dimension { .. } => "native_embedding_dimension_mismatch",
            Self::AdmissionFull { .. } => "embedding_admission_full",
            Self::Cancelled => "embedding_request_cancelled",
        }
    }

    pub fn capacity_pressure(&self) -> Option<&EmbeddingCapacityPressure> {
        match self {
            Self::AdmissionFull { pressure } => Some(pressure),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDeviceClass {
    Cpu,
    Accelerator,
    Unknown,
}

impl NativeDeviceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Accelerator => "accelerator",
            Self::Unknown => "unknown",
        }
    }
}

/// Build-time ABI and backend capabilities compiled into this native boundary.
///
/// This is descriptive evidence. Product policy chooses one explicit request
/// in `codestory-retrieval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledEngineCapabilities {
    pub schema_version: u32,
    pub target_triple: &'static str,
    pub target_os: &'static str,
    pub target_arch: &'static str,
    pub linkage: &'static str,
    pub backend_loading: &'static str,
    pub backends: &'static [&'static str],
    pub build_identity: &'static str,
}

pub const fn compiled_engine_capabilities() -> CompiledEngineCapabilities {
    CompiledEngineCapabilities {
        schema_version: NATIVE_ENGINE_BUILD_CONTRACT_SCHEMA_VERSION,
        target_triple: NATIVE_ENGINE_TARGET_TRIPLE,
        target_os: NATIVE_ENGINE_TARGET_OS,
        target_arch: NATIVE_ENGINE_TARGET_ARCH,
        linkage: NATIVE_ENGINE_LINKAGE,
        backend_loading: NATIVE_ENGINE_BACKEND_LOADING,
        backends: NATIVE_ENGINE_COMPILED_BACKENDS,
        build_identity: GGML_BUILD_IDENTITY,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBackendRequest {
    pub backend: String,
    pub device_class: NativeDeviceClass,
    pub reject_software_adapters: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEmbeddingPooling {
    Mean,
    Cls,
    Last,
    Rank,
}

/// Raw compatibility facts enforced by the compiled model boundary.
///
/// Product policy such as normalization, prefixes, batching, and fallback is
/// intentionally absent. Retrieval owns those choices and supplies a request
/// that this descriptor can accept or reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledModelCompatibility {
    model_id: &'static str,
    model_sha256: &'static str,
    dimension: usize,
    pooling: NativeEmbeddingPooling,
}

impl CompiledModelCompatibility {
    pub const fn model_id(self) -> &'static str {
        self.model_id
    }

    pub const fn model_sha256(self) -> &'static str {
        self.model_sha256
    }

    pub const fn dimension(self) -> usize {
        self.dimension
    }

    pub const fn pooling(self) -> NativeEmbeddingPooling {
        self.pooling
    }

    pub fn accepts(
        self,
        model_id: &str,
        model_sha256: &str,
        dimension: usize,
        pooling: NativeEmbeddingPooling,
    ) -> bool {
        self.model_id == model_id
            && self.model_sha256 == model_sha256
            && self.dimension == dimension
            && self.pooling as u8 == pooling as u8
    }
}

pub const COMPILED_MODEL_COMPATIBILITY: CompiledModelCompatibility = CompiledModelCompatibility {
    model_id: MODEL_FILE_NAME,
    model_sha256: MODEL_SHA256,
    dimension: EMBEDDING_DIMENSION,
    pooling: NativeEmbeddingPooling::Cls,
};

impl NativeEmbeddingPooling {
    fn llama_pooling_type(self) -> LlamaPoolingType {
        match self {
            Self::Mean => LlamaPoolingType::Mean,
            Self::Cls => LlamaPoolingType::Cls,
            Self::Last => LlamaPoolingType::Last,
            Self::Rank => LlamaPoolingType::Rank,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEmbeddingRequest {
    pub model_id: String,
    pub model_sha256: String,
    pub dimension: usize,
    pub pooling: NativeEmbeddingPooling,
    pub context_tokens: u32,
    pub max_input_tokens: usize,
    pub batch_tokens: u32,
    pub max_batch_sequences: u32,
    pub smoke_input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingEngineConfig {
    pub backend: NativeBackendRequest,
    pub embedding: NativeEmbeddingRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendCapability {
    pub backend: String,
    pub adapter_name: String,
    pub adapter_description: String,
    pub device_class: NativeDeviceClass,
    pub software_adapter: bool,
}

#[derive(Debug, Clone, Default)]
struct EncodeObservation {
    encode_count: u64,
    execution_device_names: Vec<String>,
    execution_backend_names: Vec<String>,
    observed_layer_count: u32,
    resident_tensor_count: u64,
    resident_tensor_bytes: u64,
    execution_node_count: u64,
}

#[derive(Debug, Default)]
struct EvalTelemetryState {
    encode_count: u64,
    target_device_name: String,
    execution_device_names: BTreeSet<String>,
    execution_backend_names: BTreeSet<String>,
    observed_tensors: HashSet<usize>,
    execution_tensors: HashSet<usize>,
    observed_layers: BTreeSet<u32>,
    resident_tensors: HashSet<usize>,
    resident_tensor_bytes: u64,
    execution_node_count: u64,
}

#[derive(Debug, Default)]
struct EvalTelemetry {
    state: Mutex<EvalTelemetryState>,
}

impl EvalTelemetry {
    fn new(target_device_name: String) -> Self {
        Self {
            state: Mutex::new(EvalTelemetryState {
                target_device_name,
                ..EvalTelemetryState::default()
            }),
        }
    }

    fn begin_encode(&self) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| {
            EngineError::Llama("execution telemetry mutex was poisoned".to_string())
        })?;
        state.execution_device_names.clear();
        state.execution_backend_names.clear();
        state.observed_tensors.clear();
        state.execution_tensors.clear();
        state.observed_layers.clear();
        state.resident_tensors.clear();
        state.resident_tensor_bytes = 0;
        state.execution_node_count = 0;
        Ok(())
    }

    fn complete_encode(&self) -> Result<EncodeObservation, EngineError> {
        let mut state = self.state.lock().map_err(|_| {
            EngineError::Llama("execution telemetry mutex was poisoned".to_string())
        })?;
        state.encode_count = state.encode_count.saturating_add(1);
        Ok(observation_from_state(&state))
    }

    fn observation(&self) -> Result<EncodeObservation, EngineError> {
        let state = self.state.lock().map_err(|_| {
            EngineError::Llama("execution telemetry mutex was poisoned".to_string())
        })?;
        Ok(observation_from_state(&state))
    }
}

fn observation_from_state(state: &EvalTelemetryState) -> EncodeObservation {
    EncodeObservation {
        encode_count: state.encode_count,
        execution_device_names: state.execution_device_names.iter().cloned().collect(),
        execution_backend_names: state.execution_backend_names.iter().cloned().collect(),
        observed_layer_count: state.observed_layers.len() as u32,
        resident_tensor_count: state.resident_tensors.len() as u64,
        resident_tensor_bytes: state.resident_tensor_bytes,
        execution_node_count: state.execution_node_count,
    }
}

unsafe extern "C" fn observe_eval_tensor(
    tensor: *mut llama_sys::ggml_tensor,
    ask: bool,
    user_data: *mut c_void,
) -> bool {
    // The callback's return value has two meanings in llama.cpp: during the
    // ask phase, true requests that the scheduler compute and synchronize up
    // to this tensor; during the observation phase, true lets execution
    // continue. Placement metadata is available during ask, so telemetry never
    // needs tensor data or a per-node synchronization.
    if !ask {
        return true;
    }
    if tensor.is_null() || user_data.is_null() {
        return false;
    }
    // The callback pointer is installed from a boxed EvalTelemetry before the
    // context is created. The box outlives and is dropped after the context.
    let telemetry = unsafe { &*(user_data.cast::<EvalTelemetry>()) };
    let Ok(mut state) = telemetry.state.lock() else {
        return false;
    };
    unsafe { observe_scheduled_tensor(tensor, &mut state) };
    false
}

unsafe fn tensor_device_identity(tensor: *mut llama_sys::ggml_tensor) -> Option<(String, String)> {
    let mut current = tensor;
    let buffer = (0..8).find_map(|_| {
        if current.is_null() {
            return None;
        }
        let value = unsafe { (*current).buffer };
        if !value.is_null() {
            return Some(value);
        }
        current = unsafe { (*current).view_src };
        None
    })?;
    let buffer_type = unsafe { llama_sys::ggml_backend_buffer_get_type(buffer) };
    if buffer_type.is_null() {
        return None;
    }
    let device = unsafe { llama_sys::ggml_backend_buft_get_device(buffer_type) };
    if device.is_null() {
        return None;
    }
    let device_name = unsafe { c_string_lossy(llama_sys::ggml_backend_dev_name(device)) }?;
    let registration = unsafe { llama_sys::ggml_backend_dev_backend_reg(device) };
    if registration.is_null() {
        return None;
    }
    let backend_name = unsafe { c_string_lossy(llama_sys::ggml_backend_reg_name(registration)) }?;
    Some((device_name, backend_name))
}

unsafe fn c_string_lossy(pointer: *const std::ffi::c_char) -> Option<String> {
    (!pointer.is_null()).then(|| {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    })
}

unsafe fn observe_scheduled_tensor(
    root: *mut llama_sys::ggml_tensor,
    state: &mut EvalTelemetryState,
) {
    let mut pending = vec![(root, true)];
    while let Some((tensor, scheduled)) = pending.pop() {
        if tensor.is_null() {
            continue;
        }
        let tensor_id = tensor as usize;
        let first_observation = state.observed_tensors.insert(tensor_id);
        if !first_observation && !scheduled {
            continue;
        }
        let raw = unsafe { &*tensor };
        if let Some((device_name, backend_name)) = unsafe { tensor_device_identity(tensor) } {
            let tensor_bytes = if first_observation {
                (unsafe { llama_sys::ggml_nbytes(tensor) }) as u64
            } else {
                0
            };
            record_tensor_observation(
                state,
                tensor_id,
                &raw.name,
                &device_name,
                &backend_name,
                tensor_bytes,
                scheduled,
                first_observation,
            );
        }
        if first_observation {
            pending.extend(
                raw.src
                    .iter()
                    .copied()
                    .filter(|source| !source.is_null())
                    .map(|source| (source, false)),
            );
            if !raw.view_src.is_null() {
                pending.push((raw.view_src, false));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_tensor_observation(
    state: &mut EvalTelemetryState,
    tensor_id: usize,
    name: &[std::ffi::c_char; 64],
    device_name: &str,
    backend_name: &str,
    tensor_bytes: u64,
    scheduled: bool,
    first_observation: bool,
) {
    if device_name != state.target_device_name {
        return;
    }
    if scheduled && state.execution_tensors.insert(tensor_id) {
        state.execution_device_names.insert(device_name.to_string());
        state
            .execution_backend_names
            .insert(backend_name.to_string());
        state.execution_node_count = state.execution_node_count.saturating_add(1);
    }
    if !first_observation {
        return;
    }
    if let Some(layer) = layer_index_from_tensor_name(name) {
        state.observed_layers.insert(layer);
    }
    if state.resident_tensors.insert(tensor_id) {
        state.resident_tensor_bytes = state.resident_tensor_bytes.saturating_add(tensor_bytes);
    }
}

fn layer_index_from_tensor_name(name: &[std::ffi::c_char; 64]) -> Option<u32> {
    let bytes = name
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .map(|byte| byte as u8)
        .collect::<Vec<_>>();
    let start = bytes
        .windows(4)
        .position(|candidate| candidate == b"blk.")?
        + 4;
    let end = bytes[start..]
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .map_or(bytes.len(), |offset| start + offset);
    if end == start {
        return None;
    }
    std::str::from_utf8(&bytes[start..end]).ok()?.parse().ok()
}

fn install_eval_callback(
    params: &mut LlamaContextParams,
    telemetry: &EvalTelemetry,
) -> Result<(), EngineError> {
    if std::mem::size_of::<LlamaContextParams>()
        != std::mem::size_of::<llama_sys::llama_context_params>()
        || std::mem::align_of::<LlamaContextParams>()
            != std::mem::align_of::<llama_sys::llama_context_params>()
    {
        return Err(EngineError::Llama(
            "pinned llama-cpp context parameter layout changed".to_string(),
        ));
    }
    // llama-cpp-2 0.1.151 wraps exactly one llama_context_params value. It does
    // not expose cb_eval yet, so this exact-version boundary installs the C API
    // callback after checking the wrapper and raw layouts remain identical.
    let raw = std::ptr::from_mut(params).cast::<llama_sys::llama_context_params>();
    unsafe {
        (*raw).cb_eval = Some(observe_eval_tensor);
        (*raw).cb_eval_user_data = std::ptr::from_ref(telemetry).cast_mut().cast();
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct EngineIdentity {
    pub model_digest: &'static str,
    pub ggml_build_identity: &'static str,
    pub backend: String,
    pub adapter_name: String,
    pub adapter_description: String,
    pub selected_device_class: NativeDeviceClass,
    pub runtime_capabilities: Vec<RuntimeBackendCapability>,
    pub embedded_model: bool,
    pub materialized_path: PathBuf,
    pub materialized_reused: bool,
    pub initialization_duration: Duration,
    pub smoke_duration: Duration,
    pub adapter_memory_total: usize,
    pub adapter_memory_free_before_load: usize,
    pub adapter_memory_free_after_load: usize,
    pub execution_device_names: Vec<String>,
    pub execution_backend_names: Vec<String>,
    pub execution_observation_source: &'static str,
    pub encode_count: u64,
    pub execution_node_count: u64,
    pub resident_accelerator_tensor_count: u64,
    pub resident_accelerator_tensor_bytes: u64,
    pub model_layer_count: u32,
    pub offloaded_layer_count: u32,
    pub accelerator_execution_verified: bool,
}

fn apply_live_observation(identity: &mut EngineIdentity, observation: EncodeObservation) {
    identity.execution_device_names = observation.execution_device_names;
    identity.execution_backend_names = observation.execution_backend_names;
    identity.encode_count = observation.encode_count;
    identity.execution_node_count = observation.execution_node_count;
    if identity.selected_device_class == NativeDeviceClass::Accelerator {
        identity.offloaded_layer_count = observation.observed_layer_count;
        identity.resident_accelerator_tensor_count = observation.resident_tensor_count;
        identity.resident_accelerator_tensor_bytes = observation.resident_tensor_bytes;
        identity.accelerator_execution_verified = identity.encode_count > 0
            && identity.execution_node_count > 0
            && !identity.execution_device_names.is_empty()
            && identity.offloaded_layer_count == identity.model_layer_count
            && identity.resident_accelerator_tensor_count > 0
            && identity.resident_accelerator_tensor_bytes > 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineResidency {
    Resident,
    Sleeping,
}

impl EngineResidency {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resident => "resident",
            Self::Sleeping => "sleeping",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineLifecycleSnapshot {
    pub identity: EngineIdentity,
    pub residency: EngineResidency,
    pub load_generation: u64,
    pub model_load_count: u64,
    pub worker_alive: bool,
    pub load_error: Option<String>,
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

pub struct EmbeddingResidencyLease {
    shared: Arc<EngineShared>,
    snapshot: EngineLifecycleSnapshot,
}

pub struct EmbeddingRequestHandle {
    context: EmbeddingRequestContext,
    result: Receiver<Result<EmbeddingRequestCompletion, EngineError>>,
}

pub type EmbeddingRequestResult = Result<Vec<Vec<f32>>, EngineError>;

#[derive(Debug)]
pub struct EmbeddingRequestCompletion {
    pub vectors: Vec<Vec<f32>>,
    pub completion_sequence: u64,
}

impl std::fmt::Debug for EmbeddingRequestHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmbeddingRequestHandle")
            .field("request_id", &self.context.request_id())
            .field("scope_id", &self.context.scope_id())
            .finish_non_exhaustive()
    }
}

impl EmbeddingRequestHandle {
    pub fn context(&self) -> &EmbeddingRequestContext {
        &self.context
    }

    pub fn cancel(&self) -> bool {
        self.context.cancel()
    }

    pub fn recv(self) -> Result<Vec<Vec<f32>>, EngineError> {
        self.recv_with_completion()
            .map(|completion| completion.vectors)
    }

    pub fn recv_with_completion(self) -> Result<EmbeddingRequestCompletion, EngineError> {
        self.result
            .recv()
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?
    }

    pub fn try_recv(&self) -> Result<Option<EmbeddingRequestResult>, EngineError> {
        self.try_recv_with_completion()
            .map(|result| result.map(|result| result.map(|completion| completion.vectors)))
    }

    pub fn try_recv_with_completion(
        &self,
    ) -> Result<Option<Result<EmbeddingRequestCompletion, EngineError>>, EngineError> {
        match self.result.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(EngineError::WorkerUnavailable(
                "embedding response channel disconnected".into(),
            )),
        }
    }
}

impl std::fmt::Debug for EmbeddingResidencyLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmbeddingResidencyLease")
            .field("load_generation", &self.snapshot.load_generation)
            .finish_non_exhaustive()
    }
}

impl EmbeddingResidencyLease {
    pub fn snapshot(&self) -> &EngineLifecycleSnapshot {
        &self.snapshot
    }
}

impl Drop for EmbeddingResidencyLease {
    fn drop(&mut self) {
        self.shared.admission.lease_released();
        let _ = self.shared.control_sender.send(Control::ReleaseLease);
    }
}

impl std::fmt::Debug for EmbeddingEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmbeddingEngine")
            .field("snapshot", &self.snapshot().ok())
            .finish_non_exhaustive()
    }
}

struct EngineShared {
    query_queue: Arc<CancellableRequestQueue>,
    bulk_queue: Arc<CancellableRequestQueue>,
    control_sender: Sender<Control>,
    admission: Arc<EmbeddingAdmissionTracker>,
    lifecycle: Arc<Mutex<Option<EngineLifecycleSnapshot>>>,
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
    context: EmbeddingRequestContext,
    inputs: Vec<String>,
    response: Sender<Result<EmbeddingRequestCompletion, EngineError>>,
}

struct CancellableRequestQueue {
    capacity: usize,
    requests: Mutex<VecDeque<EmbeddingRequest>>,
    signal_sender: Sender<()>,
    signal_receiver: Receiver<()>,
}

impl CancellableRequestQueue {
    fn new(capacity: usize) -> Arc<Self> {
        let (signal_sender, signal_receiver) = bounded(1);
        Arc::new(Self {
            capacity,
            requests: Mutex::new(VecDeque::with_capacity(capacity)),
            signal_sender,
            signal_receiver,
        })
    }

    fn try_push(&self, request: EmbeddingRequest) -> Result<(), EmbeddingRequest> {
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::prune_cancelled(&mut requests);
        if requests.len() >= self.capacity {
            return Err(request);
        }
        requests.push_back(request);
        let _ = self.signal_sender.try_send(());
        Ok(())
    }

    fn try_pop(&self) -> Option<EmbeddingRequest> {
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::prune_cancelled(&mut requests);
        let request = requests.pop_front();
        if !requests.is_empty() {
            let _ = self.signal_sender.try_send(());
        }
        request
    }

    fn len(&self) -> usize {
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::prune_cancelled(&mut requests);
        requests.len()
    }

    fn signal(&self) -> &Receiver<()> {
        &self.signal_receiver
    }

    fn prune_cancelled(requests: &mut VecDeque<EmbeddingRequest>) {
        let mut retained = VecDeque::with_capacity(requests.capacity());
        while let Some(request) = requests.pop_front() {
            if request.context.is_cancelled() {
                let _ = request.response.send(Err(EngineError::Cancelled));
            } else {
                retained.push_back(request);
            }
        }
        *requests = retained;
    }
}

enum Control {
    Shutdown,
    EnsureResident {
        response: Sender<Result<EngineLifecycleSnapshot, EngineError>>,
    },
    AcquireLease {
        response: Sender<Result<EngineLifecycleSnapshot, EngineError>>,
    },
    ReleaseLease,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestPriority {
    Query,
    Bulk,
}

impl EmbeddingEngine {
    pub fn initialize(
        cache_root: &Path,
        config: EmbeddingEngineConfig,
    ) -> Result<Self, EngineError> {
        validate_engine_config(&config)?;
        let query_queue = CancellableRequestQueue::new(EMBEDDING_QUERY_QUEUE_CAPACITY);
        let bulk_queue = CancellableRequestQueue::new(EMBEDDING_BULK_QUEUE_CAPACITY);
        let (control_sender, control_receiver) = unbounded();
        let (startup_sender, startup_receiver) = bounded(1);
        let lifecycle = Arc::new(Mutex::new(None));
        let admission = Arc::new(EmbeddingAdmissionTracker::default());
        let worker_lifecycle = lifecycle.clone();
        let worker_admission = Arc::clone(&admission);
        let worker_query_queue = Arc::clone(&query_queue);
        let worker_bulk_queue = Arc::clone(&bulk_queue);
        let cache_root = cache_root.to_path_buf();
        let worker = thread::Builder::new()
            .name("codestory-embedding-engine".into())
            .spawn(move || {
                if let Err(error) = run_engine_owner(
                    &cache_root,
                    &config,
                    &startup_sender,
                    &worker_query_queue,
                    &worker_bulk_queue,
                    &control_receiver,
                    &worker_lifecycle,
                    &worker_admission,
                ) {
                    let _ = startup_sender.try_send(Err(error));
                }
            })
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?;
        match startup_receiver.recv() {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(error) => {
                let _ = worker.join();
                return Err(EngineError::WorkerUnavailable(error.to_string()));
            }
        }
        Ok(Self {
            shared: Arc::new(EngineShared {
                query_queue,
                bulk_queue,
                control_sender,
                admission,
                lifecycle,
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    pub fn snapshot(&self) -> Result<EngineLifecycleSnapshot, EngineError> {
        let mut snapshot = self
            .shared
            .lifecycle
            .lock()
            .map_err(|_| EngineError::WorkerUnavailable("lifecycle mutex was poisoned".into()))?
            .clone()
            .ok_or_else(|| {
                EngineError::WorkerUnavailable("lifecycle snapshot is unavailable".into())
            })?;
        snapshot.worker_alive = self
            .shared
            .worker
            .lock()
            .map_err(|_| EngineError::WorkerUnavailable("worker mutex was poisoned".into()))?
            .as_ref()
            .is_some_and(|worker| !worker.is_finished());
        Ok(snapshot)
    }

    pub fn ensure_resident(&self) -> Result<EngineLifecycleSnapshot, EngineError> {
        let (response, result) = bounded(1);
        self.shared
            .control_sender
            .send(Control::EnsureResident { response })
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?;
        result
            .recv()
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?
    }

    pub fn acquire_residency_lease(&self) -> Result<EmbeddingResidencyLease, EngineError> {
        let (response, result) = bounded(1);
        self.shared
            .control_sender
            .send(Control::AcquireLease { response })
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))?;
        let snapshot = result
            .recv()
            .map_err(|error| EngineError::WorkerUnavailable(error.to_string()))??;
        self.shared.admission.lease_acquired();
        Ok(EmbeddingResidencyLease {
            shared: self.shared.clone(),
            snapshot,
        })
    }

    pub fn embed_query_prepared(&self, input: String) -> Result<Vec<f32>, EngineError> {
        let context = EmbeddingRequestContext::new("local-query", "local-process", 0);
        let mut vectors = self.submit_query_prepared(context, input)?.recv()?;
        vectors
            .pop()
            .ok_or_else(|| EngineError::Llama("embedding worker returned no query vector".into()))
    }

    pub fn embed_documents_prepared(
        &self,
        inputs: &[String],
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        let context = EmbeddingRequestContext::new("local-bulk", "local-process", 0);
        self.submit_documents_prepared(context, inputs.to_vec())?
            .recv()
    }

    pub fn embed_prepared(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EngineError> {
        self.embed_documents_prepared(inputs)
    }

    pub fn submit_query_prepared(
        &self,
        context: EmbeddingRequestContext,
        input: String,
    ) -> Result<EmbeddingRequestHandle, EngineError> {
        self.submit(context, vec![input], RequestPriority::Query)
    }

    pub fn submit_documents_prepared(
        &self,
        context: EmbeddingRequestContext,
        inputs: Vec<String>,
    ) -> Result<EmbeddingRequestHandle, EngineError> {
        self.submit(context, inputs, RequestPriority::Bulk)
    }

    pub fn admission_snapshot(&self) -> EmbeddingAdmissionSnapshot {
        self.shared
            .admission
            .snapshot(self.shared.query_queue.len(), self.shared.bulk_queue.len())
    }

    pub fn begin_draining_if_idle(&self) -> bool {
        let snapshot = self.admission_snapshot();
        if snapshot.query_depth != 0
            || snapshot.bulk_depth != 0
            || snapshot.active_request_count != 0
            || snapshot.lease_count != 0
        {
            return false;
        }
        self.shared
            .admission
            .set_owner_state(EmbeddingOwnerState::Draining);
        true
    }

    fn submit(
        &self,
        context: EmbeddingRequestContext,
        inputs: Vec<String>,
        priority: RequestPriority,
    ) -> Result<EmbeddingRequestHandle, EngineError> {
        if inputs.is_empty() {
            let (response, result) = bounded(1);
            let _ = response.send(Ok(EmbeddingRequestCompletion {
                vectors: Vec::new(),
                completion_sequence: 0,
            }));
            return Ok(EmbeddingRequestHandle { context, result });
        }
        let (response, result) = bounded(1);
        let request = EmbeddingRequest {
            context: context.clone(),
            inputs,
            response,
        };
        let queue = match priority {
            RequestPriority::Query => &self.shared.query_queue,
            RequestPriority::Bulk => &self.shared.bulk_queue,
        };
        match queue.try_push(request) {
            Ok(()) => Ok(EmbeddingRequestHandle { context, result }),
            Err(_) => {
                let request_class = match priority {
                    RequestPriority::Query => EmbeddingRequestClass::Query,
                    RequestPriority::Bulk => EmbeddingRequestClass::Bulk,
                };
                Err(EngineError::AdmissionFull {
                    pressure: self.shared.admission.pressure(
                        EmbeddingCapacityReason::QueueFull,
                        request_class,
                        self.shared.query_queue.len(),
                        self.shared.bulk_queue.len(),
                        context.retry_after_ms(),
                        "a queued request completes, is cancelled, or the server instance changes",
                    ),
                })
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_engine_owner(
    cache_root: &Path,
    config: &EmbeddingEngineConfig,
    startup: &Sender<Result<EngineLifecycleSnapshot, EngineError>>,
    query_queue: &Arc<CancellableRequestQueue>,
    bulk_queue: &Arc<CancellableRequestQueue>,
    control_receiver: &Receiver<Control>,
    lifecycle: &Arc<Mutex<Option<EngineLifecycleSnapshot>>>,
    admission: &Arc<EmbeddingAdmissionTracker>,
) -> Result<(), EngineError> {
    let mut wake = WakeReason::Startup;
    let mut load_generation = 0;
    let mut last_snapshot: Option<EngineLifecycleSnapshot> = None;
    loop {
        let result = run_resident_generation(
            cache_root,
            config,
            wake,
            load_generation + 1,
            startup,
            query_queue,
            bulk_queue,
            control_receiver,
            lifecycle,
            admission,
        );
        trim_unloaded_engine_working_set();
        match result {
            ResidentRunResult::Sleeping(mut snapshot) => {
                load_generation = snapshot.load_generation;
                snapshot.residency = EngineResidency::Sleeping;
                admission.set_owner_state(EmbeddingOwnerState::Sleeping);
                publish_lifecycle(lifecycle, snapshot.clone())?;
                last_snapshot = Some(snapshot);
            }
            ResidentRunResult::Shutdown(mut snapshot) => {
                snapshot.worker_alive = false;
                admission.set_owner_state(EmbeddingOwnerState::Draining);
                publish_lifecycle(lifecycle, snapshot)?;
                return Ok(());
            }
            ResidentRunResult::LoadFailed { wake, error } => {
                if let Some(snapshot) = last_snapshot.as_mut() {
                    snapshot.residency = EngineResidency::Sleeping;
                    snapshot.load_error = Some(error.to_string());
                    admission.set_owner_state(EmbeddingOwnerState::Sleeping);
                    publish_lifecycle(lifecycle, snapshot.clone())?;
                }
                if fail_wake(wake, startup, error) {
                    return Ok(());
                }
            }
        }

        let Some(next_wake) = wait_for_wake(query_queue, bulk_queue, control_receiver, admission)
        else {
            if let Some(mut snapshot) = last_snapshot {
                snapshot.worker_alive = false;
                publish_lifecycle(lifecycle, snapshot)?;
            }
            return Ok(());
        };
        wake = next_wake;
    }
}

#[cfg(target_os = "windows")]
fn trim_unloaded_engine_working_set() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};

    // SAFETY: GetCurrentProcess returns a valid pseudo-handle for the calling process. Passing
    // usize::MAX for both limits asks Windows to evict unused pages after the llama objects drop.
    let _ = unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
}

#[cfg(not(target_os = "windows"))]
fn trim_unloaded_engine_working_set() {}

enum WakeReason {
    Startup,
    Query(EmbeddingRequest),
    Bulk(EmbeddingRequest),
    EnsureResident(Sender<Result<EngineLifecycleSnapshot, EngineError>>),
    AcquireLease(Sender<Result<EngineLifecycleSnapshot, EngineError>>),
}

enum ResidentRunResult {
    Sleeping(EngineLifecycleSnapshot),
    Shutdown(EngineLifecycleSnapshot),
    LoadFailed {
        wake: WakeReason,
        error: EngineError,
    },
}

#[derive(Debug)]
struct ResidencyTracker {
    idle_timeout: Duration,
    last_activity: Instant,
    leases: usize,
}

impl ResidencyTracker {
    fn new(idle_timeout: Duration, now: Instant) -> Self {
        Self {
            idle_timeout,
            last_activity: now,
            leases: 0,
        }
    }

    fn complete_activity(&mut self, now: Instant) {
        self.last_activity = now;
    }

    fn acquire_lease(&mut self) {
        self.leases += 1;
    }

    fn release_lease(&mut self, now: Instant) {
        self.leases = self.leases.saturating_sub(1);
        self.last_activity = now;
    }

    fn remaining(&self, now: Instant) -> Duration {
        if self.leases > 0 {
            return self.idle_timeout;
        }
        self.idle_timeout
            .saturating_sub(now.saturating_duration_since(self.last_activity))
    }

    fn should_sleep(&self, now: Instant) -> bool {
        self.leases == 0 && now.saturating_duration_since(self.last_activity) >= self.idle_timeout
    }
}

#[allow(clippy::too_many_arguments)]
fn run_resident_generation(
    cache_root: &Path,
    config: &EmbeddingEngineConfig,
    wake: WakeReason,
    load_generation: u64,
    startup: &Sender<Result<EngineLifecycleSnapshot, EngineError>>,
    query_queue: &Arc<CancellableRequestQueue>,
    bulk_queue: &Arc<CancellableRequestQueue>,
    control_receiver: &Receiver<Control>,
    lifecycle: &Arc<Mutex<Option<EngineLifecycleSnapshot>>>,
    admission: &Arc<EmbeddingAdmissionTracker>,
) -> ResidentRunResult {
    let mut pending_wake = Some(wake);
    let result = (|| -> Result<ResidentRunResult, EngineError> {
        let started = Instant::now();
        admission.set_owner_state(EmbeddingOwnerState::Waking);
        let materialized = materialize_embedded_model(cache_root)?;
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        load_packaged_backend_modules()?;
        let backend = LlamaBackend::init().map_err(llama_error)?;
        let devices = list_llama_ggml_backend_devices();
        let runtime_capabilities = runtime_backend_capabilities(&devices);
        let device = select_device(&devices, &config.backend)?;
        let free_before = device.memory_free;

        let mut model_params = LlamaModelParams::default().with_use_mmap(true);
        if config.backend.device_class == NativeDeviceClass::Accelerator {
            model_params = model_params
                .with_devices(&[device.index])
                .map_err(llama_error)?
                .with_n_gpu_layers(u32::MAX);
        } else {
            model_params = model_params.with_n_gpu_layers(0);
        }
        let model = LlamaModel::load_from_file(&backend, &materialized.path, &model_params)
            .map_err(llama_error)?;
        if model.n_embd() as usize != config.embedding.dimension {
            return Err(EngineError::Dimension {
                expected: config.embedding.dimension,
                actual: model.n_embd() as usize,
            });
        }
        let model_layer_count = model.n_layer();

        let telemetry = Box::new(EvalTelemetry::new(device.name.clone()));
        let mut context_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(config.embedding.context_tokens))
            .with_n_batch(config.embedding.batch_tokens)
            .with_n_ubatch(config.embedding.batch_tokens)
            .with_n_seq_max(config.embedding.max_batch_sequences)
            .with_attention_type(LlamaAttentionType::NonCausal)
            .with_pooling_type(config.embedding.pooling.llama_pooling_type())
            .with_embeddings(true);
        install_eval_callback(&mut context_params, &telemetry)?;
        let mut context = model
            .new_context(&backend, context_params)
            .map_err(llama_error)?;
        let free_after = list_llama_ggml_backend_devices()
            .into_iter()
            .find(|candidate| candidate.index == device.index)
            .map_or(device.memory_free, |candidate| candidate.memory_free);
        let smoke_started = Instant::now();
        let smoke_context = EmbeddingRequestContext::new("engine-startup-smoke", "engine-owner", 0);
        let _ = smoke_context.activate();
        let smoke = embed_inputs(
            &model,
            &mut context,
            &telemetry,
            std::slice::from_ref(&config.embedding.smoke_input),
            RequestPriority::Query,
            query_queue,
            &config.embedding,
            admission,
            &smoke_context,
        )?;
        let _ = smoke_context.commit();
        if smoke
            .first()
            .is_none_or(|vector| vector.len() != config.embedding.dimension)
        {
            return Err(EngineError::Dimension {
                expected: config.embedding.dimension,
                actual: smoke.first().map_or(0, Vec::len),
            });
        }
        let observation = telemetry.observation()?;
        let accelerator_execution_verified = config.backend.device_class
            == NativeDeviceClass::Accelerator
            && observation.encode_count > 0
            && observation.execution_node_count > 0
            && !observation.execution_device_names.is_empty()
            && observation.observed_layer_count == model_layer_count
            && observation.resident_tensor_count > 0
            && observation.resident_tensor_bytes > 0;
        if config.backend.device_class == NativeDeviceClass::Accelerator
            && !accelerator_execution_verified
        {
            return Err(EngineError::AcceleratorExecutionUnverified(format!(
                "{} (devices={:?} layers={}/{} tensors={} bytes={} nodes={} encodes={})",
                device.description,
                observation.execution_device_names,
                observation.observed_layer_count,
                model_layer_count,
                observation.resident_tensor_count,
                observation.resident_tensor_bytes,
                observation.execution_node_count,
                observation.encode_count,
            )));
        }
        let accelerated = config.backend.device_class == NativeDeviceClass::Accelerator;
        let offloaded_layer_count = if accelerated {
            observation.observed_layer_count
        } else {
            0
        };
        let resident_accelerator_tensor_count = if accelerated {
            observation.resident_tensor_count
        } else {
            0
        };
        let resident_accelerator_tensor_bytes = if accelerated {
            observation.resident_tensor_bytes
        } else {
            0
        };
        let identity = EngineIdentity {
            model_digest: MODEL_SHA256,
            ggml_build_identity: GGML_BUILD_IDENTITY,
            backend: device.backend.clone(),
            adapter_name: device.name.clone(),
            adapter_description: device.description.clone(),
            selected_device_class: config.backend.device_class,
            runtime_capabilities,
            embedded_model: EMBEDDED_MODEL_COMPILED,
            materialized_path: materialized.path,
            materialized_reused: materialized.reused,
            initialization_duration: started.elapsed(),
            smoke_duration: smoke_started.elapsed(),
            adapter_memory_total: device.memory_total,
            adapter_memory_free_before_load: free_before,
            adapter_memory_free_after_load: free_after,
            execution_device_names: observation.execution_device_names,
            execution_backend_names: observation.execution_backend_names,
            execution_observation_source: EXECUTION_OBSERVATION_SOURCE,
            encode_count: observation.encode_count,
            execution_node_count: observation.execution_node_count,
            resident_accelerator_tensor_count,
            resident_accelerator_tensor_bytes,
            model_layer_count,
            offloaded_layer_count,
            accelerator_execution_verified,
        };
        let snapshot = EngineLifecycleSnapshot {
            identity,
            residency: EngineResidency::Resident,
            load_generation,
            model_load_count: load_generation,
            worker_alive: true,
            load_error: None,
        };
        admission.set_owner_state(EmbeddingOwnerState::Ready);
        publish_lifecycle(lifecycle, snapshot.clone())?;

        let channels = ResidentChannels {
            startup,
            query: query_queue,
            bulk: bulk_queue,
            control: control_receiver,
        };
        let mut live_snapshot = snapshot.clone();
        Ok(serve_resident_generation(
            pending_wake
                .take()
                .expect("resident generation must have one wake reason"),
            &snapshot,
            &channels,
            ENGINE_IDLE_TIMEOUT,
            |request, priority| {
                handle_request(
                    request,
                    priority,
                    &model,
                    &mut context,
                    &telemetry,
                    query_queue,
                    &config.embedding,
                    admission,
                );
                if let Ok(observation) = telemetry.observation() {
                    apply_live_observation(&mut live_snapshot.identity, observation);
                    let _ = publish_lifecycle(lifecycle, live_snapshot.clone());
                }
            },
        ))
    })();

    match result {
        Ok(result) => result,
        Err(error) => ResidentRunResult::LoadFailed {
            wake: pending_wake
                .take()
                .expect("load failure must retain its wake reason"),
            error,
        },
    }
}

struct ResidentChannels<'a> {
    startup: &'a Sender<Result<EngineLifecycleSnapshot, EngineError>>,
    query: &'a CancellableRequestQueue,
    bulk: &'a CancellableRequestQueue,
    control: &'a Receiver<Control>,
}

fn serve_resident_generation(
    wake: WakeReason,
    snapshot: &EngineLifecycleSnapshot,
    channels: &ResidentChannels<'_>,
    idle_timeout: Duration,
    mut handle: impl FnMut(EmbeddingRequest, RequestPriority),
) -> ResidentRunResult {
    let mut tracker = ResidencyTracker::new(idle_timeout, Instant::now());
    match wake {
        WakeReason::Startup => {
            let _ = channels.startup.send(Ok(snapshot.clone()));
        }
        WakeReason::Query(request) => {
            handle(request, RequestPriority::Query);
            tracker.complete_activity(Instant::now());
        }
        WakeReason::Bulk(request) => {
            handle(request, RequestPriority::Bulk);
            tracker.complete_activity(Instant::now());
        }
        WakeReason::EnsureResident(response) => {
            tracker.complete_activity(Instant::now());
            let _ = response.send(Ok(snapshot.clone()));
        }
        WakeReason::AcquireLease(response) => {
            grant_residency_lease(response, snapshot, &mut tracker);
        }
    }

    loop {
        let idle = after(tracker.remaining(Instant::now()));
        select_biased! {
            recv(channels.control) -> control => match control {
                Ok(Control::Shutdown) | Err(_) => {
                    return ResidentRunResult::Shutdown(snapshot.clone());
                }
                Ok(Control::EnsureResident { response }) => {
                    tracker.complete_activity(Instant::now());
                    let _ = response.send(Ok(snapshot.clone()));
                }
                Ok(Control::AcquireLease { response }) => {
                    grant_residency_lease(response, snapshot, &mut tracker);
                }
                Ok(Control::ReleaseLease) => tracker.release_lease(Instant::now()),
            },
            recv(channels.query.signal()) -> signal => match signal {
                Ok(()) => {
                    let Some(request) = channels.query.try_pop() else {
                        continue;
                    };
                    handle(request, RequestPriority::Query);
                    tracker.complete_activity(Instant::now());
                }
                Err(_) => return ResidentRunResult::Shutdown(snapshot.clone()),
            },
            recv(channels.bulk.signal()) -> signal => match signal {
                Ok(()) => {
                    let Some(request) = channels.bulk.try_pop() else {
                        continue;
                    };
                    handle(request, RequestPriority::Bulk);
                    tracker.complete_activity(Instant::now());
                }
                Err(_) => return ResidentRunResult::Shutdown(snapshot.clone()),
            },
            recv(idle) -> _ => {
                if tracker.should_sleep(Instant::now()) {
                    return ResidentRunResult::Sleeping(snapshot.clone());
                }
            },
        }
    }
}

fn grant_residency_lease(
    response: Sender<Result<EngineLifecycleSnapshot, EngineError>>,
    snapshot: &EngineLifecycleSnapshot,
    tracker: &mut ResidencyTracker,
) {
    if response.send(Ok(snapshot.clone())).is_ok() {
        tracker.acquire_lease();
    }
}

fn fail_wake(
    wake: WakeReason,
    startup: &Sender<Result<EngineLifecycleSnapshot, EngineError>>,
    error: EngineError,
) -> bool {
    match wake {
        WakeReason::Startup => {
            let _ = startup.send(Err(error));
            true
        }
        WakeReason::Query(request) | WakeReason::Bulk(request) => {
            let _ = request.response.send(Err(error));
            false
        }
        WakeReason::EnsureResident(response) | WakeReason::AcquireLease(response) => {
            let _ = response.send(Err(error));
            false
        }
    }
}

fn wait_for_wake(
    query_queue: &CancellableRequestQueue,
    bulk_queue: &CancellableRequestQueue,
    control_receiver: &Receiver<Control>,
    admission: &EmbeddingAdmissionTracker,
) -> Option<WakeReason> {
    loop {
        select_biased! {
            recv(control_receiver) -> control => match control {
                Ok(Control::Shutdown) | Err(_) => return None,
                Ok(Control::EnsureResident { response }) => {
                    return Some(WakeReason::EnsureResident(response));
                }
                Ok(Control::AcquireLease { response }) => {
                    return Some(WakeReason::AcquireLease(response));
                }
                Ok(Control::ReleaseLease) => {}
            },
            recv(query_queue.signal()) -> signal => {
                if signal.is_ok() {
                    let Some(request) = query_queue.try_pop() else {
                        continue;
                    };
                    if request.context.is_cancelled() {
                        admission.cancelled();
                        let _ = request.response.send(Err(EngineError::Cancelled));
                        continue;
                    }
                    return Some(WakeReason::Query(request));
                }
                return None;
            },
            recv(bulk_queue.signal()) -> signal => {
                if signal.is_ok() {
                    let Some(request) = bulk_queue.try_pop() else {
                        continue;
                    };
                    if request.context.is_cancelled() {
                        admission.cancelled();
                        let _ = request.response.send(Err(EngineError::Cancelled));
                        continue;
                    }
                    return Some(WakeReason::Bulk(request));
                }
                return None;
            },
        }
    }
}

fn publish_lifecycle(
    lifecycle: &Arc<Mutex<Option<EngineLifecycleSnapshot>>>,
    snapshot: EngineLifecycleSnapshot,
) -> Result<(), EngineError> {
    *lifecycle
        .lock()
        .map_err(|_| EngineError::WorkerUnavailable("lifecycle mutex was poisoned".into()))? =
        Some(snapshot);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_request(
    request: EmbeddingRequest,
    priority: RequestPriority,
    model: &LlamaModel,
    context: &mut LlamaContext<'_>,
    telemetry: &EvalTelemetry,
    query_queue: &CancellableRequestQueue,
    config: &NativeEmbeddingRequest,
    admission: &EmbeddingAdmissionTracker,
) {
    let request_class = match priority {
        RequestPriority::Query => EmbeddingRequestClass::Query,
        RequestPriority::Bulk => EmbeddingRequestClass::Bulk,
    };
    if !admission.begin(&request.context, request_class) {
        let _ = request.response.send(Err(EngineError::Cancelled));
        return;
    }
    while embedding_qualification_request_held(request_class) {
        if request.context.is_cancelled() {
            admission.finish(&request.context, false, true);
            let _ = request.response.send(Err(EngineError::Cancelled));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let result = embed_inputs(
        model,
        context,
        telemetry,
        &request.inputs,
        priority,
        query_queue,
        config,
        admission,
        &request.context,
    );
    let cancelled = request.context.is_cancelled();
    let result = if cancelled || !request.context.commit() {
        Err(EngineError::Cancelled)
    } else {
        result
    };
    let completion_sequence = admission.finish(&request.context, result.is_ok(), cancelled);
    let _ = request
        .response
        .send(result.map(|vectors| EmbeddingRequestCompletion {
            vectors,
            completion_sequence,
        }));
}

#[allow(clippy::too_many_arguments)]
fn embed_inputs(
    model: &LlamaModel,
    context: &mut LlamaContext<'_>,
    telemetry: &EvalTelemetry,
    inputs: &[String],
    priority: RequestPriority,
    query_queue: &CancellableRequestQueue,
    config: &NativeEmbeddingRequest,
    admission: &EmbeddingAdmissionTracker,
    request_context: &EmbeddingRequestContext,
) -> Result<Vec<Vec<f32>>, EngineError> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    if inputs.iter().any(|input| input.trim().is_empty()) {
        return Err(EngineError::EmptyInput);
    }
    let tokenized = inputs
        .iter()
        .map(|input| tokenize(model, input, config.max_input_tokens))
        .collect::<Result<Vec<_>, _>>()?;
    let completed_tokens = tokenized.iter().map(Vec::len).sum::<usize>();
    let mut output = Vec::with_capacity(inputs.len());
    let mut offset = 0;
    while offset < tokenized.len() {
        if request_context.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        if priority == RequestPriority::Bulk {
            while let Some(query) = query_queue.try_pop() {
                handle_request(
                    query,
                    RequestPriority::Query,
                    model,
                    context,
                    telemetry,
                    query_queue,
                    config,
                    admission,
                );
            }
        }
        let end = batch_end(
            &tokenized,
            offset,
            config.max_batch_sequences as usize,
            config.batch_tokens as usize,
        );
        embed_token_batch(
            context,
            telemetry,
            &tokenized[offset..end],
            &mut output,
            config.dimension,
        )?;
        admission.progress();
        offset = end;
    }
    request_context.record_completed_tokens(completed_tokens);
    Ok(output)
}

fn batch_end(
    tokenized: &[Vec<llama_cpp_2::token::LlamaToken>],
    offset: usize,
    max_batch_sequences: usize,
    batch_tokens: usize,
) -> usize {
    let mut end = offset;
    let mut tokens = 0;
    while end < tokenized.len() && end - offset < max_batch_sequences {
        let next = tokenized[end].len();
        if end > offset && tokens + next > batch_tokens {
            break;
        }
        tokens += next;
        end += 1;
    }
    end
}

fn embed_token_batch(
    context: &mut LlamaContext<'_>,
    telemetry: &EvalTelemetry,
    sequences: &[Vec<llama_cpp_2::token::LlamaToken>],
    output: &mut Vec<Vec<f32>>,
    expected_dimension: usize,
) -> Result<(), EngineError> {
    let total_tokens = sequences.iter().map(Vec::len).sum();
    let mut batch = LlamaBatch::new(total_tokens, sequences.len() as i32);
    for (sequence_id, tokens) in sequences.iter().enumerate() {
        batch
            .add_sequence(tokens, sequence_id as i32, false)
            .map_err(llama_error)?;
    }
    context.clear_kv_cache();
    telemetry.begin_encode()?;
    context.encode(&mut batch).map_err(llama_error)?;
    telemetry.complete_encode()?;
    for sequence_id in 0..sequences.len() {
        let vector = context
            .embeddings_seq_ith(sequence_id as i32)
            .map_err(llama_error)?
            .to_vec();
        if vector.len() != expected_dimension {
            return Err(EngineError::Dimension {
                expected: expected_dimension,
                actual: vector.len(),
            });
        }
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

fn validate_engine_config(config: &EmbeddingEngineConfig) -> Result<(), EngineError> {
    if config.embedding.model_id != MODEL_FILE_NAME || config.embedding.model_sha256 != MODEL_SHA256
    {
        return Err(EngineError::ModelRequestMismatch {
            requested: format!(
                "{}@{}",
                config.embedding.model_id, config.embedding.model_sha256
            ),
            compiled: format!("{MODEL_FILE_NAME}@{MODEL_SHA256}"),
        });
    }
    if !COMPILED_MODEL_COMPATIBILITY.accepts(
        &config.embedding.model_id,
        &config.embedding.model_sha256,
        config.embedding.dimension,
        config.embedding.pooling,
    ) {
        return Err(EngineError::InvalidConfiguration {
            reason: "compiled_model_compatibility_mismatch",
        });
    }
    if config.embedding.context_tokens == 0 {
        return Err(EngineError::InvalidConfiguration {
            reason: "context_tokens_zero",
        });
    }
    if config.embedding.max_input_tokens == 0 {
        return Err(EngineError::InvalidConfiguration {
            reason: "max_input_tokens_zero",
        });
    }
    if config.embedding.batch_tokens == 0 {
        return Err(EngineError::InvalidConfiguration {
            reason: "batch_tokens_zero",
        });
    }
    if config.embedding.max_batch_sequences == 0
        || config.embedding.max_batch_sequences > i32::MAX as u32
    {
        return Err(EngineError::InvalidConfiguration {
            reason: "max_batch_sequences_out_of_range",
        });
    }
    if config.embedding.max_input_tokens > config.embedding.batch_tokens as usize {
        return Err(EngineError::InvalidConfiguration {
            reason: "max_input_tokens_exceed_batch_tokens",
        });
    }
    if config.embedding.batch_tokens > config.embedding.context_tokens {
        return Err(EngineError::InvalidConfiguration {
            reason: "batch_tokens_exceed_context_tokens",
        });
    }
    if config.embedding.smoke_input.trim().is_empty() {
        return Err(EngineError::InvalidConfiguration {
            reason: "smoke_input_empty",
        });
    }

    let requested = normalize_backend_name(&config.backend.backend);
    let class_matches_backend = match config.backend.device_class {
        NativeDeviceClass::Cpu => requested == "cpu",
        NativeDeviceClass::Accelerator => requested != "cpu",
        NativeDeviceClass::Unknown => false,
    };
    if !class_matches_backend {
        return Err(EngineError::InvalidConfiguration {
            reason: "backend_device_class_mismatch",
        });
    }
    let compiled = compiled_engine_capabilities();
    if !compiled
        .backends
        .iter()
        .any(|backend| *backend == requested)
    {
        return Err(EngineError::BackendNotCompiled {
            requested,
            compiled: compiled.backends.join(","),
        });
    }
    Ok(())
}

fn select_device(
    devices: &[LlamaBackendDevice],
    request: &NativeBackendRequest,
) -> Result<LlamaBackendDevice, EngineError> {
    let requested = normalize_backend_name(&request.backend);
    let matching = devices
        .iter()
        .filter(|device| {
            backend_matches_requested(&device.backend, &requested)
                && device_class(device) == request.device_class
        })
        .collect::<Vec<_>>();
    if let Some(device) = matching
        .iter()
        .copied()
        .find(|device| !request.reject_software_adapters || !is_software_adapter(device))
    {
        return Ok(device.clone());
    }
    if request.reject_software_adapters && !matching.is_empty() {
        return Err(EngineError::SoftwareAdapter(
            matching
                .iter()
                .map(|device| format!("{} ({})", device.name, device.description))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    Err(EngineError::BackendUnavailable { requested })
}

fn runtime_backend_capabilities(devices: &[LlamaBackendDevice]) -> Vec<RuntimeBackendCapability> {
    devices
        .iter()
        .map(|device| RuntimeBackendCapability {
            backend: normalize_backend_name(&device.backend),
            adapter_name: device.name.clone(),
            adapter_description: device.description.clone(),
            device_class: device_class(device),
            software_adapter: is_software_adapter(device),
        })
        .collect()
}

fn device_class(device: &LlamaBackendDevice) -> NativeDeviceClass {
    match device.device_type {
        LlamaBackendDeviceType::Cpu => NativeDeviceClass::Cpu,
        LlamaBackendDeviceType::Accelerator
        | LlamaBackendDeviceType::Gpu
        | LlamaBackendDeviceType::IntegratedGpu => NativeDeviceClass::Accelerator,
        LlamaBackendDeviceType::Unknown => NativeDeviceClass::Unknown,
    }
}

fn backend_matches_requested(actual: &str, requested: &str) -> bool {
    let actual = actual.trim().to_ascii_lowercase();
    match requested {
        "metal" => actual == "metal" || actual == "mtl",
        "vulkan" => actual == "vulkan" || actual.starts_with("vulkan"),
        _ => actual == requested,
    }
}

fn normalize_backend_name(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "mtl" => "metal".to_string(),
        value if value.starts_with("vulkan") => "vulkan".to_string(),
        value => value.to_string(),
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
    max_input_tokens: usize,
) -> Result<Vec<llama_cpp_2::token::LlamaToken>, EngineError> {
    let tokens = model
        .str_to_token(input, AddBos::Always)
        .map_err(llama_error)?;
    if tokens.len() > max_input_tokens {
        return Err(EngineError::InputTooLong {
            actual: tokens.len(),
            maximum: max_input_tokens,
        });
    }
    Ok(tokens)
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
    fn cancelled_queue_entries_release_capacity_without_reordering_live_work() {
        let queue = CancellableRequestQueue::new(2);
        let request = |id: &str| {
            let context = EmbeddingRequestContext::new(id, "scope", 0);
            let (response, result) = bounded(1);
            (
                context.clone(),
                EmbeddingRequest {
                    context,
                    inputs: vec![id.into()],
                    response,
                },
                result,
            )
        };
        let (first_context, first, first_result) = request("first");
        let (_, second, _) = request("second");
        let (_, third, _) = request("third");
        assert!(queue.try_push(first).is_ok(), "first queue slot");
        assert!(queue.try_push(second).is_ok(), "second queue slot");
        assert_eq!(queue.len(), 2);

        assert!(first_context.cancel());
        assert_eq!(queue.len(), 1);
        assert!(matches!(
            first_result.try_recv(),
            Ok(Err(EngineError::Cancelled))
        ));
        assert!(
            queue.try_push(third).is_ok(),
            "cancelled slot is immediately reusable"
        );
        assert_eq!(queue.len(), 2);
        assert_eq!(
            queue
                .try_pop()
                .expect("second remains first")
                .context
                .request_id(),
            "second"
        );
        assert_eq!(
            queue
                .try_pop()
                .expect("third remains second")
                .context
                .request_id(),
            "third"
        );
    }

    #[test]
    fn compiled_model_descriptor_and_native_build_contract_are_inspectable() {
        let compatibility = COMPILED_MODEL_COMPATIBILITY;
        assert_eq!(compatibility.model_id(), MODEL_FILE_NAME);
        assert_eq!(compatibility.model_sha256(), MODEL_SHA256);
        assert_eq!(compatibility.dimension(), EMBEDDING_DIMENSION);
        assert_eq!(compatibility.pooling(), NativeEmbeddingPooling::Cls);
        assert!(compatibility.accepts(
            MODEL_FILE_NAME,
            MODEL_SHA256,
            EMBEDDING_DIMENSION,
            NativeEmbeddingPooling::Cls
        ));
        assert!(!compatibility.accepts(
            MODEL_FILE_NAME,
            MODEL_SHA256,
            EMBEDDING_DIMENSION,
            NativeEmbeddingPooling::Mean
        ));
        assert_eq!(MODEL_PRODUCER_NAME, env!("CARGO_PKG_NAME"));
        assert_eq!(MODEL_PRODUCER_VERSION, env!("CARGO_PKG_VERSION"));
        assert!(PRODUCT_EMBEDDING_RUNTIME_ID.contains(&format!(
            "producer-{MODEL_PRODUCER_NAME}@{MODEL_PRODUCER_VERSION}"
        )));
        assert_eq!(MODEL_LICENSE_SPDX_ID, "MIT");
        assert!(MODEL_LICENSE_SOURCE_URL.starts_with("https://"));

        let capabilities = compiled_engine_capabilities();
        assert_eq!(capabilities.schema_version, 2);
        assert!(capabilities.backends.contains(&"cpu"));
        assert!(matches!(capabilities.linkage, "static" | "dynamic"));
        for fragment in [
            format!("target={}", capabilities.target_triple),
            format!("os={}", capabilities.target_os),
            format!("arch={}", capabilities.target_arch),
            format!("linkage={}", capabilities.linkage),
            format!("backend_loading={}", capabilities.backend_loading),
            format!("backends={}", capabilities.backends.join(",")),
            format!("model_sha256={MODEL_SHA256}"),
            format!("embedding_contract_sha256={NATIVE_ENGINE_EMBEDDING_CONTRACT_SHA256}"),
        ] {
            assert!(
                capabilities.build_identity.contains(&fragment),
                "{fragment}"
            );
        }
        assert!(
            capabilities
                .build_identity
                .starts_with("codestory-native-engine-v1|")
        );
        assert!(capabilities.build_identity.ends_with("|end"));
    }

    #[test]
    fn hostile_model_backend_and_class_requests_fail_with_stable_reasons() {
        let mut config = valid_config();
        config.embedding.model_sha256 = "0".repeat(64);
        let error = validate_engine_config(&config).expect_err("model drift must fail");
        assert_eq!(error.reason_code(), "native_model_request_mismatch");

        let mut config = valid_config();
        config.backend.backend = "cuda".into();
        config.backend.device_class = NativeDeviceClass::Accelerator;
        let error = validate_engine_config(&config).expect_err("uncompiled backend must fail");
        assert_eq!(error.reason_code(), "native_backend_not_compiled");

        let mut config = valid_config();
        config.backend.device_class = NativeDeviceClass::Accelerator;
        let error = validate_engine_config(&config).expect_err("CPU class drift must fail");
        assert_eq!(error.reason_code(), "native_engine_config_invalid");

        let mut config = valid_config();
        config.embedding.pooling = NativeEmbeddingPooling::Mean;
        let error = validate_engine_config(&config).expect_err("pooling drift must fail");
        assert_eq!(error.reason_code(), "native_engine_config_invalid");
    }

    #[test]
    fn eval_telemetry_counter_advances_only_on_completed_encodes() {
        let telemetry = EvalTelemetry::new("test-device".into());
        telemetry.begin_encode().expect("begin first encode");
        {
            let mut state = telemetry.state.lock().expect("telemetry state");
            state.execution_device_names.insert("test-device".into());
            state.execution_backend_names.insert("test-backend".into());
            state.execution_node_count = 3;
        }
        let first = telemetry.complete_encode().expect("complete first encode");
        assert_eq!(first.encode_count, 1);
        telemetry.begin_encode().expect("begin second encode");
        assert_eq!(
            telemetry
                .observation()
                .expect("pending observation")
                .encode_count,
            1
        );
        let second = telemetry.complete_encode().expect("complete second encode");
        assert_eq!(second.encode_count, 2);
    }

    #[test]
    fn pinned_context_params_accept_the_eval_callback_boundary() {
        let telemetry = EvalTelemetry::new("test-device".into());
        let mut params = LlamaContextParams::default();
        install_eval_callback(&mut params, &telemetry).expect("install callback");
        let raw = std::ptr::from_ref(&params).cast::<llama_sys::llama_context_params>();
        assert!(unsafe { (*raw).cb_eval.is_some() });
        assert_eq!(
            unsafe { (*raw).cb_eval_user_data },
            std::ptr::from_ref(&telemetry).cast_mut().cast()
        );
    }

    #[test]
    fn eval_callback_observes_ask_without_requesting_per_node_synchronization() {
        // In the pinned llama.cpp callback ABI, false during ask batches the
        // node with the rest of its backend split. True during observation
        // allows graph execution to continue.
        let telemetry = EvalTelemetry::new("test-device".into());
        // Bindgen represents ggml_tensor as plain C scalars, arrays, and
        // pointers. An all-zero tensor is a valid metadata-only callback probe
        // with no buffer, sources, or view.
        let mut tensor = unsafe { std::mem::zeroed::<llama_sys::ggml_tensor>() };
        let tensor_pointer = std::ptr::from_mut(&mut tensor);
        let user_data = std::ptr::from_ref(&telemetry).cast_mut().cast();

        assert!(!unsafe { observe_eval_tensor(tensor_pointer, true, user_data) });
        assert!(
            telemetry
                .state
                .lock()
                .expect("telemetry state")
                .observed_tensors
                .contains(&(tensor_pointer as usize))
        );
        assert!(unsafe { observe_eval_tensor(tensor_pointer, false, user_data) });
    }

    #[test]
    fn mixed_device_ancestors_only_count_target_resident_layers() {
        fn name(value: &[u8]) -> [std::ffi::c_char; 64] {
            let mut name = [0 as std::ffi::c_char; 64];
            for (target, source) in name.iter_mut().zip(value) {
                *target = *source as std::ffi::c_char;
            }
            name
        }

        let mut state = EvalTelemetryState {
            target_device_name: "Apple GPU".into(),
            ..EvalTelemetryState::default()
        };
        record_tensor_observation(
            &mut state,
            1,
            &name(b"blk.0.attn_q"),
            "CPU",
            "CPU",
            1_024,
            true,
            true,
        );
        record_tensor_observation(
            &mut state,
            2,
            &name(b"blk.1.attn_q"),
            "Apple GPU",
            "Metal",
            2_048,
            true,
            true,
        );
        record_tensor_observation(
            &mut state,
            3,
            &name(b"blk.2.ffn_up.weight"),
            "CPU",
            "CPU",
            4_096,
            false,
            true,
        );

        assert_eq!(state.observed_layers, BTreeSet::from([1]));
        assert_eq!(state.execution_node_count, 1);
        assert_eq!(
            state.execution_device_names,
            BTreeSet::from(["Apple GPU".into()])
        );
        assert_eq!(
            state.execution_backend_names,
            BTreeSet::from(["Metal".into()])
        );
        assert_eq!(state.resident_tensors, HashSet::from([2]));
        assert_eq!(state.resident_tensor_bytes, 2_048);
    }

    #[test]
    fn layer_observation_uses_backend_tensor_names() {
        let mut name = [0 as std::ffi::c_char; 64];
        for (target, source) in name.iter_mut().zip(b"blk.17.attn_q.weight") {
            *target = *source as std::ffi::c_char;
        }
        assert_eq!(layer_index_from_tensor_name(&name), Some(17));
        let unnamed = [0 as std::ffi::c_char; 64];
        assert_eq!(layer_index_from_tensor_name(&unnamed), None);
    }

    #[test]
    fn runtime_selection_honors_the_explicit_request_without_fallback() {
        let devices = vec![
            test_device("CPU", "cpu", LlamaBackendDeviceType::Cpu),
            test_device("Apple GPU", "Metal", LlamaBackendDeviceType::Gpu),
        ];
        let metal = NativeBackendRequest {
            backend: "metal".into(),
            device_class: NativeDeviceClass::Accelerator,
            reject_software_adapters: true,
        };
        assert_eq!(
            select_device(&devices, &metal)
                .expect("explicit Metal request")
                .name,
            "Apple GPU"
        );

        let vulkan = NativeBackendRequest {
            backend: "vulkan".into(),
            device_class: NativeDeviceClass::Accelerator,
            reject_software_adapters: true,
        };
        let error = select_device(&devices, &vulkan).expect_err("must not fall back to CPU");
        assert_eq!(error.reason_code(), "native_backend_unavailable");

        let unknown = vec![test_device(
            "Unclassified Metal device",
            "Metal",
            LlamaBackendDeviceType::Unknown,
        )];
        let error = select_device(&unknown, &metal).expect_err("unknown device must fail closed");
        assert_eq!(error.reason_code(), "native_backend_unavailable");
    }

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

    fn valid_config() -> EmbeddingEngineConfig {
        EmbeddingEngineConfig {
            backend: NativeBackendRequest {
                backend: "cpu".into(),
                device_class: NativeDeviceClass::Cpu,
                reject_software_adapters: true,
            },
            embedding: NativeEmbeddingRequest {
                model_id: MODEL_FILE_NAME.into(),
                model_sha256: MODEL_SHA256.into(),
                dimension: EMBEDDING_DIMENSION,
                pooling: NativeEmbeddingPooling::Cls,
                context_tokens: 4096,
                max_input_tokens: 512,
                batch_tokens: 1024,
                max_batch_sequences: 6,
                smoke_input: "native boundary smoke".into(),
            },
        }
    }

    fn test_device(
        name: &str,
        backend: &str,
        device_type: LlamaBackendDeviceType,
    ) -> LlamaBackendDevice {
        LlamaBackendDevice {
            index: 0,
            name: name.into(),
            description: name.into(),
            backend: backend.into(),
            memory_total: 1,
            memory_free: 1,
            device_type,
        }
    }

    #[test]
    fn residency_tracker_sleeps_only_after_the_idle_window() {
        let started = Instant::now();
        let timeout = Duration::from_secs(60);
        let tracker = ResidencyTracker::new(timeout, started);

        assert_eq!(tracker.remaining(started), timeout);
        assert!(!tracker.should_sleep(started + timeout - Duration::from_millis(1)));
        assert!(tracker.should_sleep(started + timeout));
    }

    #[test]
    fn residency_lease_pins_the_load_and_release_starts_a_fresh_window() {
        let started = Instant::now();
        let timeout = Duration::from_secs(60);
        let mut tracker = ResidencyTracker::new(timeout, started);
        tracker.acquire_lease();

        assert!(!tracker.should_sleep(started + timeout * 2));

        let released = started + timeout * 2;
        tracker.release_lease(released);
        assert!(!tracker.should_sleep(released + timeout - Duration::from_millis(1)));
        assert!(tracker.should_sleep(released + timeout));
    }

    #[test]
    fn abandoned_lease_handoff_does_not_pin_residency() {
        let started = Instant::now();
        let timeout = Duration::from_secs(60);
        let mut tracker = ResidencyTracker::new(timeout, started);
        let snapshot = test_lifecycle_snapshot();
        let (sender, receiver) = bounded(1);
        drop(receiver);

        grant_residency_lease(sender, &snapshot, &mut tracker);

        assert!(tracker.should_sleep(started + timeout));
    }

    #[test]
    fn owner_loop_honors_injected_timeout_and_all_live_leases() {
        let (startup_sender, _startup_receiver) = bounded(1);
        let query_queue = CancellableRequestQueue::new(1);
        let bulk_queue = CancellableRequestQueue::new(1);
        let (control_sender, control_receiver) = unbounded();
        let (first_lease_sender, first_lease_receiver) = bounded(1);
        let (done_sender, done_receiver) = bounded(1);
        let snapshot = test_lifecycle_snapshot();

        let worker = thread::spawn(move || {
            let channels = ResidentChannels {
                startup: &startup_sender,
                query: &query_queue,
                bulk: &bulk_queue,
                control: &control_receiver,
            };
            let result = serve_resident_generation(
                WakeReason::AcquireLease(first_lease_sender),
                &snapshot,
                &channels,
                Duration::from_millis(20),
                |_, _| panic!("lease-only owner test received an embedding request"),
            );
            let _ = done_sender.send(result);
        });

        first_lease_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("first lease handoff")
            .expect("first lease grant");
        let (second_lease_sender, second_lease_receiver) = bounded(1);
        control_sender
            .send(Control::AcquireLease {
                response: second_lease_sender,
            })
            .expect("queue second lease");
        second_lease_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("second lease handoff")
            .expect("second lease grant");

        assert!(
            done_receiver
                .recv_timeout(Duration::from_millis(60))
                .is_err()
        );
        control_sender
            .send(Control::ReleaseLease)
            .expect("release first lease");
        assert!(
            done_receiver
                .recv_timeout(Duration::from_millis(60))
                .is_err()
        );
        control_sender
            .send(Control::ReleaseLease)
            .expect("release final lease");
        assert!(matches!(
            done_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("owner should sleep after the injected timeout"),
            ResidentRunResult::Sleeping(_)
        ));
        worker.join().expect("owner test worker");
    }

    fn test_lifecycle_snapshot() -> EngineLifecycleSnapshot {
        EngineLifecycleSnapshot {
            identity: EngineIdentity {
                model_digest: MODEL_SHA256,
                ggml_build_identity: GGML_BUILD_IDENTITY,
                backend: "test".into(),
                adapter_name: "test".into(),
                adapter_description: "test".into(),
                selected_device_class: NativeDeviceClass::Cpu,
                runtime_capabilities: Vec::new(),
                embedded_model: true,
                materialized_path: PathBuf::from("test.gguf"),
                materialized_reused: true,
                initialization_duration: Duration::ZERO,
                smoke_duration: Duration::ZERO,
                adapter_memory_total: 0,
                adapter_memory_free_before_load: 0,
                adapter_memory_free_after_load: 0,
                execution_device_names: Vec::new(),
                execution_backend_names: Vec::new(),
                execution_observation_source: EXECUTION_OBSERVATION_SOURCE,
                encode_count: 0,
                execution_node_count: 0,
                resident_accelerator_tensor_count: 0,
                resident_accelerator_tensor_bytes: 0,
                model_layer_count: 13,
                offloaded_layer_count: 0,
                accelerator_execution_verified: false,
            },
            residency: EngineResidency::Resident,
            load_generation: 1,
            model_load_count: 1,
            worker_alive: true,
            load_error: None,
        }
    }
}
