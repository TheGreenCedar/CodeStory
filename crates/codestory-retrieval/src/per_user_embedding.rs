use crate::config::SidecarRuntimeConfig;
use crate::embedding_contract::{
    CODERANK_DOCUMENT_PREFIX, CODERANK_QUERY_PREFIX, EMBEDDING_ELEMENT_TYPE, EMBEDDING_MODEL_ID,
    EMBEDDING_MODEL_SHA256, EMBEDDING_NORMALIZATION, EMBEDDING_POOLING,
    EMBEDDING_VECTOR_SCHEMA_VERSION, RETRIEVAL_EMBEDDING_DIM, native_engine_config,
    normalize_and_validate_vectors,
};
use anyhow::{Context, Result, anyhow, bail};
use codestory_llama_sys::{
    EMBEDDING_BULK_QUEUE_CAPACITY, EMBEDDING_QUERY_QUEUE_CAPACITY, EmbeddingAdmissionSnapshot,
    EmbeddingCapacityPressure, EmbeddingCapacityReason, EmbeddingEngine, EmbeddingEngineConfig,
    EmbeddingOwnerState, EmbeddingRequestClass, EmbeddingRequestContext, EngineError,
    EngineLifecycleSnapshot, NativeDeviceClass,
};
use codestory_workspace::{
    WorkspacePathIdentity, workspace_file_identity, workspace_path_identity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

pub const PER_USER_EMBEDDING_BOOTSTRAP_VERSION: u32 = 1;
pub const PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION: u32 = 1;
pub const PER_USER_EMBEDDING_PROTOCOL_V1: &str = "codestory.per-user-embedding/v1";
pub const PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
pub const PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS: u64 = 60_000;
pub const PER_USER_EMBEDDING_PROTOCOL_SHA256: &str =
    codestory_llama_sys::PER_USER_EMBEDDING_PROTOCOL_SHA256;
pub const PER_USER_EMBEDDING_CONSTANT_SET_SHA256: &str =
    codestory_llama_sys::PER_USER_EMBEDDING_CONSTANT_SET_SHA256;
pub const PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256: &str =
    codestory_llama_sys::PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256;
pub const PER_USER_EMBEDDING_CONSTANT_SET_FROZEN: bool =
    codestory_llama_sys::PER_USER_EMBEDDING_CONSTANT_SET_FROZEN;
pub const PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT: usize = 2_048;
pub const PER_USER_EMBEDDING_MAX_INPUT_BYTES: usize = 1024 * 1024;
pub const PER_USER_EMBEDDING_MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
pub const PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
pub static PER_USER_EMBEDDING_SERVER_PROOF_MARKER: &[u8] =
    codestory_llama_sys::EMBEDDING_SERVER_PROOF_MARKER;

const SERVER_ACCEPT_POLL: Duration = Duration::from_millis(25);
const CONNECTION_POLL: Duration = Duration::from_millis(25);
const SERVER_CONTROL_CONNECTION_RESERVE: usize = 8;
const SERVER_REJECTION_CONNECTION_RESERVE: usize = 8;
const SERVER_CONNECTION_HANDLER_CAPACITY: usize = EMBEDDING_QUERY_QUEUE_CAPACITY
    + EMBEDDING_BULK_QUEUE_CAPACITY
    + SERVER_CONTROL_CONNECTION_RESERVE;
const SERVER_TOTAL_CONNECTION_HANDLER_CAPACITY: usize =
    SERVER_CONNECTION_HANDLER_CAPACITY + SERVER_REJECTION_CONNECTION_RESERVE;
const SERVER_QUALIFICATION_MAX_COMMAND_BYTES: u64 = 16 * 1024;
const SERVER_QUALIFICATION_MAX_EVENT_BYTES: u64 = 4 * 1024 * 1024;
const SERVER_QUALIFICATION_MAX_EVENT_RECORDS: u64 = 2_048;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingTransportIdentity {
    pub endpoint_namespace_id: String,
    pub lifetime_authority_id: String,
    pub listener_id: String,
    pub peer_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_process_start_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingConnectIntent {
    Activate,
    Observe,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct EmbeddingTransportFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct EmbeddingSpawnAttempt {
    generation: u64,
    state: Arc<Mutex<EmbeddingSpawnAttemptState>>,
}

#[derive(Debug)]
enum EmbeddingSpawnAttemptState {
    Pending,
    Succeeded,
    Failed(EmbeddingTransportFailure),
}

impl EmbeddingSpawnAttempt {
    pub fn new(generation: u64) -> Self {
        debug_assert_ne!(generation, 0);
        Self {
            generation,
            state: Arc::new(Mutex::new(EmbeddingSpawnAttemptState::Pending)),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn record_failure(&self, failure: EmbeddingTransportFailure) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, EmbeddingSpawnAttemptState::Pending) {
            *state = EmbeddingSpawnAttemptState::Failed(failure);
        }
    }

    pub fn record_success(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            EmbeddingSpawnAttemptState::Succeeded;
    }

    pub fn failure(&self) -> Option<EmbeddingTransportFailure> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            EmbeddingSpawnAttemptState::Failed(failure) => Some(failure.clone()),
            EmbeddingSpawnAttemptState::Pending | EmbeddingSpawnAttemptState::Succeeded => None,
        }
    }
}

pub trait EmbeddingServerStream: Read + Write + Send {
    fn transport_identity(&self) -> &EmbeddingTransportIdentity;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    /// Returns false once the authenticated peer process has exited. This is
    /// deliberately process-liveness only; it never inspects project state.
    fn peer_is_alive(&self) -> io::Result<bool>;
    /// Returns the authenticated peer process exit code when the platform can
    /// prove that the retained process identity has exited. A live peer and a
    /// platform without exit-code support both return `None`.
    fn peer_exit_code(&self) -> io::Result<Option<u32>> {
        Ok(None)
    }
    /// Completes transport-specific delivery of the final response before the
    /// server tears the connection down. Transports whose close preserves
    /// unread bytes need no additional work.
    fn finish_response_delivery(&self) -> io::Result<()> {
        Ok(())
    }
    fn shutdown(&self) -> io::Result<()>;
}

pub enum EmbeddingConnectOutcome {
    Connected(Box<dyn EmbeddingServerStream>),
    NoOwner,
    OwnerUnresponsive(EmbeddingTransportFailure),
}

impl fmt::Debug for EmbeddingConnectOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected(stream) => formatter
                .debug_tuple("Connected")
                .field(stream.transport_identity())
                .finish(),
            Self::NoOwner => formatter.write_str("NoOwner"),
            Self::OwnerUnresponsive(error) => formatter
                .debug_tuple("OwnerUnresponsive")
                .field(error)
                .finish(),
        }
    }
}

pub trait AwakeMonotonicClock: Send + Sync {
    fn now_ns(&self) -> u64;
    fn sleep(&self, duration: Duration);
    fn snapshot(&self) -> EmbeddingServerClockSnapshot;
}

pub trait EmbeddingClientTransport: Send + Sync {
    fn connect(
        &self,
        intent: EmbeddingConnectIntent,
        budget: Duration,
        spawn_attempt: Option<&EmbeddingSpawnAttempt>,
    ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure>;
    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure>;
    fn clock(&self) -> Arc<dyn AwakeMonotonicClock>;
    fn executable_identity(&self) -> EmbeddingExecutableIdentity;
    fn budgets(&self) -> EmbeddingClientBudgets;
}

pub trait EmbeddingServerListener: Send + Sync {
    fn accept(
        &self,
        timeout: Duration,
    ) -> std::result::Result<Option<Box<dyn EmbeddingServerStream>>, EmbeddingTransportFailure>;
    fn identity(&self) -> &EmbeddingTransportIdentity;
    fn close(&self) -> std::result::Result<(), EmbeddingTransportFailure>;
}

pub enum EmbeddingServerBindOutcome {
    Bound(Box<dyn EmbeddingServerListener>),
    AlreadyOwned,
}

impl fmt::Debug for EmbeddingServerBindOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bound(listener) => formatter
                .debug_tuple("Bound")
                .field(listener.identity())
                .finish(),
            Self::AlreadyOwned => formatter.write_str("AlreadyOwned"),
        }
    }
}

pub trait EmbeddingServerTransport: Send + Sync {
    fn bind(&self) -> std::result::Result<EmbeddingServerBindOutcome, EmbeddingTransportFailure>;
    fn clock(&self) -> Arc<dyn AwakeMonotonicClock>;
    fn fail_stop(&self, reason_code: &str);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingExecutableIdentity {
    pub pid: u32,
    pub process_start_id: String,
    pub executable_sha256: String,
    pub executable_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingClientBudgets {
    pub connect: Duration,
    pub spawn: Duration,
    pub retry_after: Duration,
    pub query_request: Duration,
    pub bulk_request: Duration,
}

impl EmbeddingClientBudgets {
    /// Values generated from the checked-in constant set. Its draft section
    /// is used only while package qualification remains fail-closed.
    pub const fn current() -> Self {
        Self {
            connect: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_CONNECT_TIMEOUT_MS,
            ),
            spawn: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_SPAWN_CONVERGENCE_TIMEOUT_MS,
            ),
            retry_after: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_RETRY_AFTER_MS,
            ),
            query_request: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_QUERY_REQUEST_DEADLINE_MS,
            ),
            bulk_request: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingServerBudgets {
    pub idle_timeout: Duration,
    pub native_no_progress: Duration,
    pub watchdog_poll: Duration,
}

impl EmbeddingServerBudgets {
    /// Values generated from the checked-in constant set. Its draft section
    /// is used only while package qualification remains fail-closed.
    pub const fn current() -> Self {
        Self {
            idle_timeout: Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS),
            native_no_progress: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_HARD_NATIVE_NO_PROGRESS_MS,
            ),
            watchdog_poll: Duration::from_millis(
                codestory_llama_sys::PER_USER_EMBEDDING_WATCHDOG_CADENCE_MS,
            ),
        }
    }
}

pub struct PerUserEmbeddingServerConfig {
    pub transport: Arc<dyn EmbeddingServerTransport>,
    pub engine_cache_root: PathBuf,
    pub executable: EmbeddingExecutableIdentity,
    pub allow_cpu: bool,
    pub budgets: EmbeddingServerBudgets,
    pub protocol: EmbeddingServerProtocolSnapshot,
}

impl fmt::Debug for PerUserEmbeddingServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingServerConfig")
            .field("engine_cache_root", &self.engine_cache_root)
            .field("executable", &self.executable)
            .field("allow_cpu", &self.allow_cpu)
            .field("budgets", &self.budgets)
            .field("protocol", &self.protocol)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingCompatibility {
    pub protocol_schema_version: u32,
    pub product_runtime_id: String,
    pub model_id: String,
    pub model_sha256: String,
    pub tokenizer_sha256: String,
    pub config_sha256: String,
    pub query_prefix: String,
    pub document_prefix: String,
    pub pooling: String,
    pub normalization: String,
    pub dimension: u32,
    pub element_type: String,
    pub vector_schema_version: u32,
    pub ggml_build_identity: String,
    pub target_triple: String,
    pub policy: String,
}

impl EmbeddingCompatibility {
    pub fn current(allow_cpu: bool) -> Self {
        let capabilities = codestory_llama_sys::compiled_engine_capabilities();
        Self {
            protocol_schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            product_runtime_id: crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into(),
            model_id: EMBEDDING_MODEL_ID.into(),
            model_sha256: EMBEDDING_MODEL_SHA256.into(),
            tokenizer_sha256: codestory_llama_sys::MODEL_TOKENIZER_SHA256.into(),
            config_sha256: codestory_llama_sys::MODEL_CONFIG_SHA256.into(),
            query_prefix: CODERANK_QUERY_PREFIX.into(),
            document_prefix: CODERANK_DOCUMENT_PREFIX.into(),
            pooling: EMBEDDING_POOLING.into(),
            normalization: EMBEDDING_NORMALIZATION.into(),
            dimension: RETRIEVAL_EMBEDDING_DIM as u32,
            element_type: EMBEDDING_ELEMENT_TYPE.into(),
            vector_schema_version: EMBEDDING_VECTOR_SCHEMA_VERSION,
            ggml_build_identity: codestory_llama_sys::GGML_BUILD_IDENTITY.into(),
            target_triple: capabilities.target_triple.into(),
            policy: if allow_cpu {
                "cpu_explicit"
            } else {
                "accelerated"
            }
            .into(),
        }
    }

    pub fn digest(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self).context("serialize embedding compatibility")?;
        Ok(hex_sha256(&bytes))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerClockSnapshot {
    pub domain: String,
    pub api: String,
    pub boot_id: String,
    pub resolution_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerProtocolSnapshot {
    pub bootstrap_version: u32,
    pub schema_version: u32,
    pub protocol_sha256: String,
    pub constant_set_sha256: String,
    pub measurement_protocol_sha256: String,
}

impl EmbeddingServerProtocolSnapshot {
    pub fn current() -> Self {
        Self {
            bootstrap_version: PER_USER_EMBEDDING_BOOTSTRAP_VERSION,
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            protocol_sha256: PER_USER_EMBEDDING_PROTOCOL_SHA256.into(),
            constant_set_sha256: PER_USER_EMBEDDING_CONSTANT_SET_SHA256.into(),
            measurement_protocol_sha256: PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerAuthoritySnapshot {
    pub endpoint_namespace_id: String,
    pub lifetime_authority_id: String,
    pub listener_id: String,
    pub peer_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerProcessSnapshot {
    pub server_instance_id: String,
    pub pid: u32,
    pub process_start_id: String,
    pub executable_sha256: String,
    pub executable_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerActiveRequestSnapshot {
    pub request_id: String,
    pub scope_id: String,
    pub class: String,
    pub phase: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerSchedulerSnapshot {
    pub query_capacity: u64,
    pub query_depth: u64,
    pub bulk_capacity: u64,
    pub bulk_depth: u64,
    pub connection_count: u64,
    pub active_request_count: u64,
    pub lease_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_request: Option<EmbeddingServerActiveRequestSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerEngineSnapshot {
    pub engine_owner_id: String,
    pub native_worker_id: String,
    pub load_generation: u64,
    pub model_load_count: u64,
    pub successful_encode_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerFailureSnapshot {
    pub code: String,
    pub retry_class: String,
    pub retry_after_ms: u64,
    pub retry_condition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWatchdogClock {
    pub domain: String,
    pub api: String,
    pub boot_id: String,
    pub observed_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationWatchdogMarker {
    pub schema_version: u32,
    pub nonce_sha256: String,
    pub server_instance_id: String,
    pub pid: u32,
    pub process_start_id: String,
    pub executable_sha256: String,
    pub executable_version: String,
    pub reason: String,
    pub clock: EmbeddingQualificationWatchdogClock,
    pub progress_sequence: u64,
    pub last_progress_ns: u64,
    pub hard_native_no_progress_ms: u64,
    pub watchdog_cadence_ms: u64,
}

pub fn embedding_qualification_watchdog_marker_filename(
    nonce_sha256: &str,
    server_instance_id: &str,
) -> Result<String> {
    if nonce_sha256.len() != 64
        || !nonce_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || server_instance_id.is_empty()
        || server_instance_id.len() > 128
        || !server_instance_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("embedding_qualification_watchdog_marker_identity_invalid");
    }
    Ok(format!(
        "{nonce_sha256}.{server_instance_id}.watchdog-fail-stop.json"
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingServerSnapshot {
    pub schema_version: u32,
    pub event_sequence: u64,
    pub lifecycle: String,
    pub clock: EmbeddingServerClockSnapshot,
    pub protocol: EmbeddingServerProtocolSnapshot,
    pub authority: EmbeddingServerAuthoritySnapshot,
    pub process: EmbeddingServerProcessSnapshot,
    pub scheduler: EmbeddingServerSchedulerSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EmbeddingServerEngineSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<EmbeddingServerFailureSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingEngineIdentity {
    pub server_instance_id: String,
    pub load_generation: u64,
    pub model_load_count: u64,
    pub residency: String,
    pub worker_alive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
    pub model_digest: String,
    pub ggml_build_identity: String,
    pub backend: String,
    pub adapter_name: String,
    pub adapter_description: String,
    pub policy: String,
    pub embedded_model: bool,
    pub materialized_model_sha256: String,
    pub materialized_reused: bool,
    pub initialization_ms: u64,
    pub smoke_ms: u64,
    pub adapter_memory_total: u64,
    pub adapter_memory_used_by_load: u64,
    pub execution_device_names: Vec<String>,
    pub execution_backend_names: Vec<String>,
    pub execution_observation_source: String,
    pub encode_count: u64,
    pub execution_node_count: u64,
    pub resident_accelerator_tensor_count: u64,
    pub resident_accelerator_tensor_bytes: u64,
    pub model_layer_count: u32,
    pub offloaded_layer_count: u32,
    pub accelerator_execution_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingEngineLeaseIdentity {
    pub lease_token: String,
    pub server_instance_id: String,
    pub load_generation: u64,
    pub compatibility_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProtocolRequest {
    pub protocol: String,
    pub schema_version: u32,
    pub request_id: String,
    pub compatibility: EmbeddingCompatibility,
    pub operation: EmbeddingOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbeddingOperation {
    Hello {
        intent: String,
        client_pid: u32,
        client_process_start_id: String,
        client_executable_sha256: String,
        client_executable_version: String,
    },
    Snapshot,
    EnsureResident {
        scope_id: String,
        deadline_ms: u64,
        retry_after_ms: u64,
    },
    AcquireLease {
        scope_id: String,
        deadline_ms: u64,
        retry_after_ms: u64,
    },
    ReleaseLease {
        lease_token: String,
    },
    EmbedQuery {
        scope_id: String,
        deadline_ms: u64,
        retry_after_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cancel_token: Option<String>,
        input: String,
    },
    EmbedDocuments {
        scope_id: String,
        deadline_ms: u64,
        retry_after_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cancel_token: Option<String>,
        inputs: Vec<String>,
    },
    Cancel {
        target_request_id: String,
        cancel_token: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProtocolResponse {
    pub protocol: String,
    pub schema_version: u32,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<EmbeddingResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EmbeddingProtocolError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbeddingResult {
    Hello {
        compatibility_sha256: String,
        snapshot: Box<EmbeddingServerSnapshot>,
    },
    Snapshot {
        snapshot: Box<EmbeddingServerSnapshot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease: Option<EmbeddingEngineLeaseIdentity>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<Box<EmbeddingEngineIdentity>>,
    },
    Identity {
        identity: Box<EmbeddingEngineIdentity>,
    },
    Lease {
        lease: EmbeddingEngineLeaseIdentity,
        identity: Box<EmbeddingEngineIdentity>,
    },
    Vectors {
        rows: u32,
        columns: u32,
        encoding: String,
        identity: Box<EmbeddingEngineIdentity>,
    },
    Released,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingProtocolError {
    pub code: String,
    pub message: String,
    pub retry_class: String,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<EmbeddingCapacityPressureWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingCapacityPressureWire {
    pub reason: String,
    pub queue_class: String,
    pub capacity: u64,
    pub depth: u64,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    pub owner_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_request_class: Option<String>,
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct PerUserEmbeddingError {
    pub code: String,
    pub message: String,
    pub retry_class: String,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    pub capacity: Option<EmbeddingCapacityPressureWire>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingRetryStateWire {
    pub code: String,
    pub message: String,
    pub retry_class: String,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    pub capacity: Option<EmbeddingCapacityPressureWire>,
}

pub fn embedding_retry_state(error: &anyhow::Error) -> Option<EmbeddingRetryStateWire> {
    error
        .downcast_ref::<PerUserEmbeddingError>()
        .map(|error| EmbeddingRetryStateWire {
            code: error.code.clone(),
            message: error.message.clone(),
            retry_class: error.retry_class.clone(),
            retry_after_ms: error.retry_after_ms,
            retry_condition: error.retry_condition.clone(),
            capacity: error.capacity.clone(),
        })
}

pub fn embedding_capacity_pressure(error: &anyhow::Error) -> Option<EmbeddingCapacityPressureWire> {
    embedding_retry_state(error).and_then(|retry| retry.capacity)
}

static CLIENT_TRANSPORT: OnceLock<Arc<dyn EmbeddingClientTransport>> = OnceLock::new();

pub fn install_embedding_client_transport(
    transport: Arc<dyn EmbeddingClientTransport>,
) -> Result<()> {
    CLIENT_TRANSPORT
        .set(transport)
        .map_err(|_| anyhow!("embedding_client_transport_already_installed"))
}

#[derive(Clone)]
pub struct PerUserEmbeddingClient {
    transport: Arc<dyn EmbeddingClientTransport>,
    compatibility: EmbeddingCompatibility,
    scope_id: String,
}

struct EmbeddingCallControl<'a> {
    operation_timeout: Duration,
    outer_deadline: Option<Instant>,
    operation_deadline: OnceLock<Instant>,
    cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl<'a> EmbeddingCallControl<'a> {
    fn new(
        operation_timeout: Duration,
        outer_timeout: Option<Duration>,
        cancelled: &'a (dyn Fn() -> bool + Sync),
    ) -> Result<Self> {
        if operation_timeout.is_zero() || outer_timeout.is_some_and(|timeout| timeout.is_zero()) {
            bail!("embedding_server_deadline_invalid");
        }
        let outer_deadline = outer_timeout
            .map(|timeout| {
                Instant::now()
                    .checked_add(timeout)
                    .ok_or_else(|| anyhow!("embedding_server_deadline_invalid"))
            })
            .transpose()?;
        let control = Self {
            operation_timeout,
            outer_deadline,
            operation_deadline: OnceLock::new(),
            cancelled,
        };
        control.check()?;
        Ok(control)
    }

    fn arm(&self) -> Result<()> {
        if self.operation_deadline.get().is_none() {
            let deadline = Instant::now()
                .checked_add(self.operation_timeout)
                .ok_or_else(|| anyhow!("embedding_server_deadline_invalid"))?;
            let _ = self.operation_deadline.set(deadline);
        }
        self.check()
    }

    fn active_deadline(&self) -> Option<Instant> {
        match (self.outer_deadline, self.operation_deadline.get().copied()) {
            (Some(outer), Some(operation)) => Some(outer.min(operation)),
            (Some(outer), None) => Some(outer),
            (None, Some(operation)) => Some(operation),
            (None, None) => None,
        }
    }

    fn triggered(&self) -> bool {
        (self.cancelled)()
            || self
                .active_deadline()
                .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn check(&self) -> Result<()> {
        if (self.cancelled)() {
            return Err(PerUserEmbeddingError {
                code: "embedding_cancelled".into(),
                message: "the caller cancelled the embedding request".into(),
                retry_class: "none".into(),
                retry_after_ms: 0,
                retry_condition: "the caller starts a new request".into(),
                capacity: None,
            }
            .into());
        }
        if self
            .active_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(PerUserEmbeddingError {
                code: "embedding_deadline_exceeded".into(),
                message: "the caller deadline elapsed during the embedding request".into(),
                retry_class: "after_delay".into(),
                retry_after_ms: 0,
                retry_condition: "the caller starts a new request with a fresh deadline".into(),
                capacity: None,
            }
            .into());
        }
        Ok(())
    }

    fn remaining(&self, maximum: Duration) -> Result<Duration> {
        self.check()?;
        let remaining = self.active_deadline().map_or(maximum, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(maximum)
        });
        if remaining.is_zero() {
            self.check()?;
            bail!("embedding_server_deadline_invalid");
        }
        Ok(remaining)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationRequest {
    pub schema_version: u32,
    pub nonce_sha256: String,
    pub scenario: String,
    pub parameters: EmbeddingQualificationParameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationParameters {
    pub query_count: u32,
    pub bulk_count: u32,
    pub documents_per_bulk: u32,
    pub input_bytes: u32,
    pub hold_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationOperationResult {
    pub correlation_id: String,
    pub class: String,
    pub submitted_ns: u64,
    pub completed_ns: u64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<EmbeddingQualificationAttemptResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationAttemptResult {
    pub ordinal: u32,
    pub request_id: String,
    pub server_instance_id: String,
    pub submitted_ns: u64,
    pub completed_ns: u64,
    pub outcome: String,
}

type EmbeddingQualificationAttemptExchange = (
    (EmbeddingResult, Vec<u8>),
    Vec<EmbeddingQualificationAttemptResult>,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationResult {
    pub schema_version: u32,
    pub scenario: String,
    pub started_ns: u64,
    pub finished_ns: u64,
    pub operations: Vec<EmbeddingQualificationOperationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_snapshot: Option<EmbeddingServerSnapshot>,
}

struct ValidatedEmbeddingConnection {
    stream: Box<dyn EmbeddingServerStream>,
    snapshot: EmbeddingServerSnapshot,
}

pub fn run_per_user_embedding_qualification(
    runtime: &SidecarRuntimeConfig,
    request: EmbeddingQualificationRequest,
) -> Result<EmbeddingQualificationResult> {
    validate_qualification_gate(&request)?;
    validate_qualification_request(&request)?;
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let clock = Arc::clone(&client.transport.clock());
    let started_ns = clock.now_ns();
    let initial_snapshot = client.observe()?;
    let input = "q".repeat(request.parameters.input_bytes.max(1) as usize);
    let documents = (0..request.parameters.documents_per_bulk.max(1))
        .map(|index| format!("{index}:{input}"))
        .collect::<Vec<_>>();
    let mut work = Vec::new();
    match request.scenario.as_str() {
        "query" | "replay" => {
            for _ in 0..request.parameters.query_count.max(1) {
                work.push(("query", input.clone(), Vec::new()));
            }
        }
        "bulk" => {
            for _ in 0..request.parameters.bulk_count.max(1) {
                work.push(("bulk", String::new(), documents.clone()));
            }
        }
        "mixed" => {
            for _ in 0..request.parameters.bulk_count {
                work.push(("bulk", String::new(), documents.clone()));
            }
            for _ in 0..request.parameters.query_count {
                work.push(("query", input.clone(), Vec::new()));
            }
        }
        "lease" => work.push(("lease", String::new(), Vec::new())),
        "observe" => work.push(("observe", String::new(), Vec::new())),
        "incompatible" => work.push(("incompatible", String::new(), Vec::new())),
        _ => bail!("embedding_qualification_scenario_unknown"),
    }
    let mut workers = Vec::with_capacity(work.len());
    for (class, query, bulk) in work {
        let client = client.clone();
        let clock = Arc::clone(&clock);
        let hold = Duration::from_millis(request.parameters.hold_ms);
        workers.push(
            thread::Builder::new()
                .name(format!("codestory-embedding-qualification-{class}"))
                .spawn(move || qualification_operation(client, clock, class, query, bulk, hold))
                .context("spawn embedding qualification operation")?,
        );
    }
    let mut operations = Vec::with_capacity(workers.len());
    for worker in workers {
        operations.push(
            worker
                .join()
                .map_err(|_| anyhow!("embedding_qualification_operation_panicked"))?,
        );
    }
    let final_snapshot = client.observe()?;
    Ok(EmbeddingQualificationResult {
        schema_version: 1,
        scenario: request.scenario,
        started_ns,
        finished_ns: clock.now_ns(),
        operations,
        initial_snapshot,
        final_snapshot,
    })
}

fn qualification_operation(
    mut client: PerUserEmbeddingClient,
    clock: Arc<dyn AwakeMonotonicClock>,
    class: &str,
    query: String,
    bulk: Vec<String>,
    hold: Duration,
) -> EmbeddingQualificationOperationResult {
    let correlation_id = Uuid::new_v4().to_string();
    let submitted_ns = clock.now_ns();
    let result = match class {
        "query" => client
            .embed_query_with_qualification_attempts(&query)
            .map(|(_, attempts)| (None, attempts)),
        "bulk" => client
            .embed_documents_with_qualification_attempts(&bulk)
            .map(|(_, attempts)| (None, attempts)),
        "lease" => client.acquire_residency_lease().and_then(|mut lease| {
            if !hold.is_zero() {
                clock.sleep(hold);
            }
            let identity = lease.revalidate()?;
            lease.release()?;
            Ok((Some(identity), Vec::new()))
        }),
        "observe" => client.observe().map(|_| (None, Vec::new())),
        "incompatible" => {
            client.compatibility.config_sha256 = "qualification-incompatible".into();
            client
                .ensure_resident()
                .map(|identity| (Some(identity), Vec::new()))
        }
        _ => unreachable!("qualification scenarios are validated before dispatch"),
    };
    let completed_ns = clock.now_ns();
    match result {
        Ok((identity, attempts)) => EmbeddingQualificationOperationResult {
            correlation_id,
            class: class.into(),
            submitted_ns,
            completed_ns,
            status: "ok".into(),
            error_code: None,
            server_instance_id: identity
                .as_ref()
                .map(|identity| identity.server_instance_id.clone()),
            load_generation: identity.as_ref().map(|identity| identity.load_generation),
            attempts,
        },
        Err(error) => EmbeddingQualificationOperationResult {
            correlation_id,
            class: class.into(),
            submitted_ns,
            completed_ns,
            status: "failed".into(),
            error_code: Some(qualification_error_code(&error)),
            server_instance_id: None,
            load_generation: None,
            attempts: Vec::new(),
        },
    }
}

fn validate_qualification_gate(request: &EmbeddingQualificationRequest) -> Result<()> {
    let directory = std::env::var_os("CODESTORY_EMBED_QUALIFICATION_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("embedding_qualification_gate_closed"))?;
    let nonce = std::env::var("CODESTORY_EMBED_QUALIFICATION_NONCE")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("embedding_qualification_gate_closed"))?;
    if !PathBuf::from(directory).is_dir() || request.nonce_sha256 != hex_sha256(nonce.as_bytes()) {
        bail!("embedding_qualification_gate_closed");
    }
    Ok(())
}

fn validate_qualification_request(request: &EmbeddingQualificationRequest) -> Result<()> {
    if request.schema_version != 1
        || request.parameters.query_count > 128
        || request.parameters.bulk_count > 128
        || request.parameters.documents_per_bulk > PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT as u32
        || request.parameters.input_bytes == 0
        || request.parameters.input_bytes as usize > PER_USER_EMBEDDING_MAX_INPUT_BYTES
        || request.parameters.hold_ms > 600_000
    {
        bail!("embedding_qualification_request_invalid");
    }
    Ok(())
}

fn qualification_error_code(error: &anyhow::Error) -> String {
    error
        .chain()
        .find_map(|cause| {
            cause
                .downcast_ref::<PerUserEmbeddingError>()
                .map(|error| error.code.clone())
        })
        .unwrap_or_else(|| {
            error
                .to_string()
                .split(':')
                .next()
                .unwrap_or("embedding_qualification_failed")
                .into()
        })
}

impl fmt::Debug for PerUserEmbeddingClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingClient")
            .field("scope_id", &self.scope_id)
            .field("compatibility", &self.compatibility)
            .finish_non_exhaustive()
    }
}

impl PerUserEmbeddingClient {
    pub fn for_runtime(runtime: &SidecarRuntimeConfig) -> Result<Self> {
        let transport = CLIENT_TRANSPORT
            .get()
            .cloned()
            .ok_or_else(|| anyhow!("embedding_server_transport_unavailable"))?;
        Ok(Self {
            transport,
            compatibility: EmbeddingCompatibility::current(runtime.embedding.allow_cpu),
            scope_id: embedding_scope_id(runtime),
        })
    }

    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_query_with_control(text, None, &|| false)
    }

    pub fn embed_query_with_control(
        &self,
        text: &str,
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<f32>> {
        self.embed_query_with_control_and_attempts(text, maximum_timeout, cancelled)
            .map(|(vector, _)| vector)
    }

    fn embed_query_with_qualification_attempts(
        &self,
        text: &str,
    ) -> Result<(Vec<f32>, Vec<EmbeddingQualificationAttemptResult>)> {
        self.embed_query_with_control_and_attempts(text, None, &|| false)
    }

    fn embed_query_with_control_and_attempts(
        &self,
        text: &str,
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(Vec<f32>, Vec<EmbeddingQualificationAttemptResult>)> {
        validate_raw_inputs(std::slice::from_ref(&text.to_string()))?;
        let budgets = self.transport.budgets();
        let (result, attempts) = self.call_pure_with_replay_controlled_and_attempts(
            budgets.query_request,
            maximum_timeout,
            cancelled,
            |deadline_ms, token| EmbeddingOperation::EmbedQuery {
                scope_id: self.scope_id.clone(),
                deadline_ms,
                retry_after_ms: duration_ms(budgets.retry_after),
                cancel_token: Some(token),
                input: text.to_string(),
            },
        )?;
        let (rows, columns, identity, payload) = vectors_result(result)?;
        if rows != 1 {
            bail!("embedding_vector_row_count_mismatch: expected=1 observed={rows}");
        }
        let mut vectors = decode_vectors(rows, columns, &payload)?;
        validate_engine_identity(&identity, &self.compatibility)?;
        let vector = normalize_and_validate_vectors(std::mem::take(&mut vectors))?
            .pop()
            .ok_or_else(|| anyhow!("embedding_vector_missing"))?;
        Ok((vector, attempts))
    }

    pub fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_documents_with_control(texts, None, &|| false)
    }

    pub fn embed_documents_with_control(
        &self,
        texts: &[String],
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<Vec<f32>>> {
        self.embed_documents_with_control_and_attempts(texts, maximum_timeout, cancelled)
            .map(|(vectors, _)| vectors)
    }

    fn embed_documents_with_qualification_attempts(
        &self,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, Vec<EmbeddingQualificationAttemptResult>)> {
        self.embed_documents_with_control_and_attempts(texts, None, &|| false)
    }

    fn embed_documents_with_control_and_attempts(
        &self,
        texts: &[String],
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(Vec<Vec<f32>>, Vec<EmbeddingQualificationAttemptResult>)> {
        if texts.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        validate_raw_inputs(texts)?;
        let budgets = self.transport.budgets();
        let (result, attempts) = self.call_pure_with_replay_controlled_and_attempts(
            budgets.bulk_request,
            maximum_timeout,
            cancelled,
            |deadline_ms, token| EmbeddingOperation::EmbedDocuments {
                scope_id: self.scope_id.clone(),
                deadline_ms,
                retry_after_ms: duration_ms(budgets.retry_after),
                cancel_token: Some(token),
                inputs: texts.to_vec(),
            },
        )?;
        let (rows, columns, identity, payload) = vectors_result(result)?;
        if rows as usize != texts.len() {
            bail!(
                "embedding_vector_row_count_mismatch: expected={} observed={rows}",
                texts.len()
            );
        }
        validate_engine_identity(&identity, &self.compatibility)?;
        Ok((
            normalize_and_validate_vectors(decode_vectors(rows, columns, &payload)?)?,
            attempts,
        ))
    }

    pub fn ensure_resident(&self) -> Result<EmbeddingEngineIdentity> {
        let budgets = self.transport.budgets();
        let mut connection = self.connect(EmbeddingConnectIntent::Activate, true)?;
        configure_exchange_timeout(&*connection.stream, budgets.bulk_request)?;
        let request_id = Uuid::new_v4().to_string();
        let operation = EmbeddingOperation::EnsureResident {
            scope_id: self.scope_id.clone(),
            deadline_ms: duration_ms(budgets.bulk_request),
            retry_after_ms: duration_ms(budgets.retry_after),
        };
        let (response, _) = exchange(
            &mut *connection.stream,
            request(&request_id, self.compatibility.clone(), operation),
        )?;
        let EmbeddingResult::Identity { identity } = response_result(response)? else {
            bail!("embedding_server_protocol_mismatch: expected identity");
        };
        validate_engine_identity(&identity, &self.compatibility)?;
        validate_engine_server_identity(&identity, &connection.snapshot)?;
        Ok(*identity)
    }

    pub fn acquire_residency_lease(&self) -> Result<PerUserEmbeddingResidencyLease> {
        let budgets = self.transport.budgets();
        let mut connection = self.connect(EmbeddingConnectIntent::Activate, true)?;
        configure_exchange_timeout(&*connection.stream, budgets.bulk_request)?;
        let request_id = Uuid::new_v4().to_string();
        let operation = EmbeddingOperation::AcquireLease {
            scope_id: self.scope_id.clone(),
            deadline_ms: duration_ms(budgets.bulk_request),
            retry_after_ms: duration_ms(budgets.retry_after),
        };
        let (response, _) = exchange(
            &mut *connection.stream,
            request(&request_id, self.compatibility.clone(), operation),
        )?;
        let EmbeddingResult::Lease { lease, identity } = response_result(response)? else {
            bail!("embedding_server_protocol_mismatch: expected lease");
        };
        validate_engine_identity(&identity, &self.compatibility)?;
        validate_engine_server_identity(&identity, &connection.snapshot)?;
        validate_lease_server_identity(&lease, &identity, &connection.snapshot)?;
        Ok(PerUserEmbeddingResidencyLease {
            stream: Some(connection.stream),
            compatibility: self.compatibility.clone(),
            lease,
            identity: *identity,
            server: connection.snapshot,
            budgets,
        })
    }

    pub fn observe(&self) -> Result<Option<EmbeddingServerSnapshot>> {
        Ok(self
            .observe_with_identity()?
            .map(|(snapshot, _identity)| snapshot))
    }

    pub(crate) fn observe_with_identity(
        &self,
    ) -> Result<Option<(EmbeddingServerSnapshot, Option<EmbeddingEngineIdentity>)>> {
        let mut connection = match self.connect(EmbeddingConnectIntent::Observe, false) {
            Ok(connected) => connected,
            Err(error) if error.to_string().contains("embedding_server_absent") => return Ok(None),
            Err(error) => return Err(error),
        };
        configure_exchange_timeout(&*connection.stream, self.transport.budgets().connect)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut *connection.stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::Snapshot,
            ),
        )?;
        let EmbeddingResult::Snapshot {
            snapshot, identity, ..
        } = response_result(response)?
        else {
            bail!("embedding_server_protocol_mismatch: expected snapshot");
        };
        validate_server_snapshot(
            &snapshot,
            connection.stream.transport_identity(),
            &self.transport.executable_identity(),
        )?;
        validate_same_server(&snapshot, &connection.snapshot)?;
        if let Some(identity) = identity.as_deref() {
            validate_engine_identity(identity, &self.compatibility)?;
            validate_engine_server_identity(identity, &snapshot)?;
        }
        Ok(Some((*snapshot, identity.map(|identity| *identity))))
    }

    fn call_pure_with_replay_controlled_and_attempts<B>(
        &self,
        operation_timeout: Duration,
        outer_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
        operation: B,
    ) -> Result<EmbeddingQualificationAttemptExchange>
    where
        B: Fn(u64, String) -> EmbeddingOperation,
    {
        let control = EmbeddingCallControl::new(operation_timeout, outer_timeout, cancelled)?;
        let clock = self.transport.clock();
        let mut replayed = false;
        let mut recover_after_inflight_loss = false;
        let mut attempts = Vec::with_capacity(2);
        loop {
            control.check()?;
            let mut connection = match self.connect_with_control(
                EmbeddingConnectIntent::Activate,
                true,
                Some(&control),
                recover_after_inflight_loss,
            ) {
                Ok(connection) => connection,
                Err(error) if !replayed && is_server_loss(&error) => {
                    control.check()?;
                    replayed = true;
                    continue;
                }
                Err(error) => return Err(error),
            };
            control.arm()?;
            let request_id = Uuid::new_v4().to_string();
            let cancel_token = Uuid::new_v4().to_string();
            let remaining = control.remaining(operation_timeout)?;
            let request_operation =
                operation(positive_duration_ms(remaining), cancel_token.clone());
            configure_exchange_timeout(&*connection.stream, remaining)?;
            let server_instance_id = connection.snapshot.process.server_instance_id.clone();
            let submitted_ns = clock.now_ns();
            let completed = AtomicBool::new(false);
            let exchange_result = thread::scope(|scope| {
                scope.spawn(|| {
                    self.watch_controlled_cancellation(
                        &control,
                        &completed,
                        &request_id,
                        &cancel_token,
                    );
                });
                let result = exchange(
                    &mut *connection.stream,
                    request(&request_id, self.compatibility.clone(), request_operation),
                );
                completed.store(true, Ordering::Release);
                result
            });
            let call = (|| {
                control.check()?;
                let (response, payload) = exchange_result?;
                let result = response_result(response)?;
                if let EmbeddingResult::Vectors { identity, .. } = &result {
                    validate_engine_server_identity(identity, &connection.snapshot)?;
                }
                Ok::<_, anyhow::Error>((result, payload))
            })();
            let completed_ns = clock.now_ns();
            let outcome = match &call {
                Ok(_) => "completed",
                Err(error) if is_server_loss(error) => "server_loss",
                Err(_) => "failed",
            };
            attempts.push(EmbeddingQualificationAttemptResult {
                ordinal: attempts.len() as u32 + 1,
                request_id,
                server_instance_id,
                submitted_ns,
                completed_ns,
                outcome: outcome.into(),
            });
            match call {
                Ok(result) => return Ok((result, attempts)),
                Err(error) if !replayed && is_server_loss(&error) => {
                    control.check()?;
                    replayed = true;
                    recover_after_inflight_loss = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn watch_controlled_cancellation(
        &self,
        control: &EmbeddingCallControl<'_>,
        completed: &AtomicBool,
        request_id: &str,
        cancel_token: &str,
    ) {
        while !completed.load(Ordering::Acquire) && !control.triggered() {
            thread::sleep(CONNECTION_POLL);
        }
        if !completed.load(Ordering::Acquire) {
            // The server has the same finite request deadline, so cancellation
            // is best effort rather than a retry loop. Retrying every poll
            // after a full handler admission would turn timed-out callers into
            // an unbounded control-connection storm.
            let _ = self.send_cancel(request_id, cancel_token);
        }
    }

    fn send_cancel(&self, target_request_id: &str, cancel_token: &str) -> Result<bool> {
        let mut connection = self.connect(EmbeddingConnectIntent::Activate, false)?;
        configure_exchange_timeout(&*connection.stream, self.transport.budgets().connect)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut *connection.stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::Cancel {
                    target_request_id: target_request_id.into(),
                    cancel_token: cancel_token.into(),
                },
            ),
        )?;
        match response_result(response)? {
            EmbeddingResult::Cancelled => Ok(true),
            EmbeddingResult::Released => Ok(false),
            _ => bail!("embedding_server_protocol_mismatch: expected cancellation result"),
        }
    }

    fn connect(
        &self,
        intent: EmbeddingConnectIntent,
        may_spawn: bool,
    ) -> Result<ValidatedEmbeddingConnection> {
        self.connect_with_control(intent, may_spawn, None, false)
    }

    fn connect_with_control(
        &self,
        intent: EmbeddingConnectIntent,
        may_spawn: bool,
        control: Option<&EmbeddingCallControl<'_>>,
        recover_after_server_loss: bool,
    ) -> Result<ValidatedEmbeddingConnection> {
        let budgets = self.transport.budgets();
        let mut spawned_at_ns = None;
        let mut owner_recovery_started_at_ns = None;
        let mut spawn_attempt = None;
        let wait_for_convergence = |started_at_ns| -> Result<()> {
            if let Some(control) = control {
                control.check()?;
            }
            let elapsed = elapsed_since(self.transport.clock().as_ref(), started_at_ns);
            let remaining = budgets.spawn.saturating_sub(elapsed);
            if remaining.is_zero() {
                bail!("embedding_server_start_timeout");
            }
            let remaining = control
                .map(|control| control.remaining(remaining))
                .transpose()?
                .unwrap_or(remaining);
            self.transport
                .clock()
                .sleep(budgets.retry_after.min(remaining));
            Ok(())
        };
        loop {
            if let Some(control) = control {
                control.check()?;
            }
            let connect_budget = control
                .map(|control| control.remaining(budgets.connect))
                .transpose()?
                .unwrap_or(budgets.connect);
            match self
                .transport
                .connect(intent, connect_budget, spawn_attempt.as_ref())
                .map_err(anyhow::Error::new)?
            {
                EmbeddingConnectOutcome::Connected(mut stream) => {
                    configure_exchange_timeout(&*stream, connect_budget)?;
                    let transport_identity = stream.transport_identity().clone();
                    let executable = self.transport.executable_identity();
                    let snapshot = match hello(
                        &mut *stream,
                        intent,
                        self.compatibility.clone(),
                        &transport_identity,
                        &executable,
                    ) {
                        Ok(snapshot) => snapshot,
                        Err(error) if recover_after_server_loss && is_server_loss(&error) => {
                            let recovery_started_at_ns = owner_recovery_started_at_ns
                                .get_or_insert_with(|| self.transport.clock().now_ns());
                            wait_for_convergence(*recovery_started_at_ns)?;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    if let Some(control) = control {
                        control.check()?;
                    }
                    return Ok(ValidatedEmbeddingConnection { stream, snapshot });
                }
                EmbeddingConnectOutcome::NoOwner if may_spawn && spawned_at_ns.is_none() => {
                    spawn_attempt = Some(
                        self.transport
                            .spawn_exact_current_exe()
                            .map_err(anyhow::Error::new)?,
                    );
                    spawned_at_ns = Some(self.transport.clock().now_ns());
                }
                EmbeddingConnectOutcome::NoOwner if !may_spawn => {
                    bail!("embedding_server_absent");
                }
                EmbeddingConnectOutcome::NoOwner => {
                    let spawned_at_ns =
                        spawned_at_ns.expect("an activating retry follows an exact-exe spawn");
                    wait_for_convergence(spawned_at_ns)?;
                }
                EmbeddingConnectOutcome::OwnerUnresponsive(error) => {
                    if let Some(spawned_at_ns) = spawned_at_ns {
                        wait_for_convergence(spawned_at_ns)?;
                        continue;
                    }
                    if recover_after_server_loss {
                        let recovery_started_at_ns = owner_recovery_started_at_ns
                            .get_or_insert_with(|| self.transport.clock().now_ns());
                        wait_for_convergence(*recovery_started_at_ns)?;
                        continue;
                    }
                    return Err(PerUserEmbeddingError {
                        code: "embedding_server_owner_unresponsive".into(),
                        message: error.message,
                        retry_class: "after_server_change".into(),
                        retry_after_ms: duration_ms(budgets.retry_after),
                        retry_condition: "the lifetime authority changes".into(),
                        capacity: None,
                    }
                    .into());
                }
            }
        }
    }
}

pub struct PerUserEmbeddingResidencyLease {
    stream: Option<Box<dyn EmbeddingServerStream>>,
    compatibility: EmbeddingCompatibility,
    lease: EmbeddingEngineLeaseIdentity,
    identity: EmbeddingEngineIdentity,
    server: EmbeddingServerSnapshot,
    budgets: EmbeddingClientBudgets,
}

impl fmt::Debug for PerUserEmbeddingResidencyLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingResidencyLease")
            .field("lease", &self.lease)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl PerUserEmbeddingResidencyLease {
    pub fn identity(&self) -> &EmbeddingEngineIdentity {
        &self.identity
    }

    pub fn lease_identity(&self) -> &EmbeddingEngineLeaseIdentity {
        &self.lease
    }

    pub fn revalidate(&mut self) -> Result<EmbeddingEngineIdentity> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("embedding_publication_lease_released"))?;
        configure_exchange_timeout(&**stream, self.budgets.bulk_request)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut **stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::Snapshot,
            ),
        )?;
        let EmbeddingResult::Snapshot {
            snapshot,
            lease: Some(lease),
            identity: Some(identity),
        } = response_result(response)?
        else {
            bail!("embedding_server_protocol_mismatch: expected lease revalidation");
        };
        if lease != self.lease || identity.server_instance_id != self.identity.server_instance_id {
            bail!("embedding_publication_lease_changed");
        }
        validate_server_snapshot(
            &snapshot,
            stream.transport_identity(),
            &EmbeddingExecutableIdentity {
                pid: self.server.process.pid,
                process_start_id: self.server.process.process_start_id.clone(),
                executable_sha256: self.server.process.executable_sha256.clone(),
                executable_version: self.server.process.executable_version.clone(),
            },
        )?;
        validate_same_server(&snapshot, &self.server)?;
        validate_lease_server_identity(&lease, &identity, &snapshot)?;
        validate_engine_identity(&identity, &self.compatibility)?;
        self.identity = *identity;
        Ok(self.identity.clone())
    }

    pub fn release(mut self) -> Result<()> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> Result<()> {
        let Some(mut stream) = self.stream.take() else {
            return Ok(());
        };
        configure_exchange_timeout(&*stream, self.budgets.connect)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut *stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::ReleaseLease {
                    lease_token: self.lease.lease_token.clone(),
                },
            ),
        )?;
        let EmbeddingResult::Released = response_result(response)? else {
            bail!("embedding_server_protocol_mismatch: expected lease release");
        };
        Ok(())
    }
}

impl Drop for PerUserEmbeddingResidencyLease {
    fn drop(&mut self) {
        let _ = self.release_inner();
    }
}

#[derive(Debug, Clone)]
struct ActiveServerRequest {
    request_id: String,
    scope_id: String,
    request_class: EmbeddingRequestClass,
    phase: String,
    started_ns: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ServerRequestAdmissionDepths {
    query: usize,
    bulk: usize,
}

impl ServerRequestAdmissionDepths {
    fn depth(self, request_class: EmbeddingRequestClass) -> usize {
        match request_class {
            EmbeddingRequestClass::Query => self.query,
            EmbeddingRequestClass::Bulk => self.bulk,
        }
    }

    fn capacity(request_class: EmbeddingRequestClass) -> usize {
        match request_class {
            EmbeddingRequestClass::Query => EMBEDDING_QUERY_QUEUE_CAPACITY,
            EmbeddingRequestClass::Bulk => EMBEDDING_BULK_QUEUE_CAPACITY,
        }
    }

    fn increment(&mut self, request_class: EmbeddingRequestClass) {
        match request_class {
            EmbeddingRequestClass::Query => self.query += 1,
            EmbeddingRequestClass::Bulk => self.bulk += 1,
        }
    }

    fn decrement(&mut self, request_class: EmbeddingRequestClass) {
        match request_class {
            EmbeddingRequestClass::Query => self.query = self.query.saturating_sub(1),
            EmbeddingRequestClass::Bulk => self.bulk = self.bulk.saturating_sub(1),
        }
    }
}

#[derive(Debug, Default)]
struct ServerRequestAdmission {
    depths: Mutex<ServerRequestAdmissionDepths>,
}

impl ServerRequestAdmission {
    fn try_acquire(
        self: &Arc<Self>,
        request_class: EmbeddingRequestClass,
        active_execution: bool,
    ) -> std::result::Result<ServerRequestAdmissionPermit, ServerRequestAdmissionDepths> {
        let mut depths = self
            .depths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let in_flight_capacity = ServerRequestAdmissionDepths::capacity(request_class)
            .saturating_add(usize::from(active_execution));
        if depths.depth(request_class) >= in_flight_capacity {
            return Err(*depths);
        }
        depths.increment(request_class);
        Ok(ServerRequestAdmissionPermit {
            inner: Arc::new(ServerRequestAdmissionPermitInner {
                admission: Arc::clone(self),
                request_class,
                released: AtomicBool::new(false),
            }),
        })
    }

    fn snapshot(&self) -> ServerRequestAdmissionDepths {
        *self
            .depths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn release(&self, request_class: EmbeddingRequestClass) {
        self.depths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .decrement(request_class);
    }
}

#[derive(Debug, Clone)]
struct ServerRequestAdmissionPermit {
    inner: Arc<ServerRequestAdmissionPermitInner>,
}

impl ServerRequestAdmissionPermit {
    fn release(&self) {
        if !self.inner.released.swap(true, Ordering::AcqRel) {
            self.inner.admission.release(self.inner.request_class);
        }
    }
}

impl Drop for ServerRequestAdmissionPermit {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug)]
struct ServerRequestAdmissionPermitInner {
    admission: Arc<ServerRequestAdmission>,
    request_class: EmbeddingRequestClass,
    released: AtomicBool,
}

#[derive(Debug, Clone)]
struct ServerCancellation {
    context: EmbeddingRequestContext,
    admission: ServerRequestAdmissionPermit,
    auth: Option<ServerCancellationAuth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerCancellationAuth {
    token: String,
    client_pid: u32,
    client_process_start_id: String,
}

#[derive(Debug)]
struct PinnedQualificationDirectory {
    path: PathBuf,
    identity: NativeFileIdentity,
    #[cfg(unix)]
    handle: File,
}

type NativeFileIdentity = WorkspacePathIdentity;

#[derive(Debug)]
struct ServerQualificationEventLog {
    path: PathBuf,
    file: File,
    identity: NativeFileIdentity,
    bytes: u64,
    records: u64,
    last_sequence: u64,
}

#[derive(Debug)]
struct ServerQualificationCommandFile {
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct ServerQualificationControl {
    directory: PinnedQualificationDirectory,
    events: Mutex<ServerQualificationEventLog>,
    nonce: String,
    nonce_sha256: String,
    last_sequence: AtomicU64,
    processed_command_sha256: Mutex<Option<String>>,
    force_incompatible: AtomicBool,
    freeze_owner: AtomicBool,
}

impl ServerQualificationControl {
    fn command_was_processed(&self, command_sha256: &str) -> bool {
        self.processed_command_sha256
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_deref()
            == Some(command_sha256)
    }

    fn mark_command_processed(&self, command_sha256: String) {
        *self
            .processed_command_sha256
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(command_sha256);
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerQualificationCommand {
    schema_version: u32,
    sequence: u64,
    nonce_sha256: String,
    action: String,
    parameters: ServerQualificationCommandParameters,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerQualificationCommandParameters {
    #[serde(default)]
    class: Option<String>,
}

#[derive(Debug, Serialize)]
struct ServerQualificationEventClock {
    domain: String,
    api: String,
    boot_id: String,
    observed_ns: u64,
}

#[derive(Debug, Serialize)]
struct ServerQualificationEvent {
    schema_version: u32,
    sequence: u64,
    action: String,
    status: String,
    server_event_sequence: u64,
    clock: ServerQualificationEventClock,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ExistingServerQualificationEvent {
    schema_version: u32,
    sequence: u64,
}

struct PerUserEmbeddingServerState {
    clock: Arc<dyn AwakeMonotonicClock>,
    engine_cache_root: PathBuf,
    engine_config: EmbeddingEngineConfig,
    engine: Mutex<Option<EmbeddingEngine>>,
    process: EmbeddingServerProcessSnapshot,
    protocol: EmbeddingServerProtocolSnapshot,
    authority: EmbeddingServerAuthoritySnapshot,
    connections: AtomicUsize,
    pre_request_connections: AtomicUsize,
    admission_gate: Mutex<()>,
    request_admission: Arc<ServerRequestAdmission>,
    active: Mutex<std::collections::BTreeMap<String, ActiveServerRequest>>,
    cancellations: Mutex<std::collections::BTreeMap<String, ServerCancellation>>,
    draining: AtomicBool,
    stopped: AtomicBool,
    last_work_ended_ns: AtomicU64,
    event_sequence: AtomicU64,
    last_failure: Mutex<Option<EmbeddingServerFailureSnapshot>>,
    qualification: Option<ServerQualificationControl>,
}

impl fmt::Debug for PerUserEmbeddingServerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingServerState")
            .field("process", &self.process)
            .field("protocol", &self.protocol)
            .field("authority", &self.authority)
            .field("connections", &self.connections.load(Ordering::Acquire))
            .field("draining", &self.draining.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl PerUserEmbeddingServerState {
    fn engine(&self) -> Result<EmbeddingEngine> {
        let _admission = self
            .admission_gate
            .lock()
            .map_err(|_| anyhow!("embedding_server_admission_gate_poisoned"))?;
        if self.draining.load(Ordering::Acquire) {
            bail!("embedding_server_draining");
        }
        let mut slot = self
            .engine
            .lock()
            .map_err(|_| anyhow!("embedding_server_engine_state_poisoned"))?;
        if slot.is_none() {
            *slot = Some(
                EmbeddingEngine::initialize(&self.engine_cache_root, self.engine_config.clone())
                    .map_err(engine_error)?,
            );
            self.bump_event();
        }
        Ok(slot
            .as_ref()
            .expect("embedding engine initialized above")
            .clone())
    }

    fn initialized_engine(&self) -> Option<EmbeddingEngine> {
        self.engine.lock().ok().and_then(|engine| engine.clone())
    }

    fn try_initialized_engine(&self) -> Option<EmbeddingEngine> {
        self.engine
            .try_lock()
            .ok()
            .and_then(|engine| engine.clone())
    }

    fn try_admit_request(
        &self,
        request_class: EmbeddingRequestClass,
        retry_after_ms: u64,
    ) -> std::result::Result<ServerRequestAdmissionPermit, Box<EmbeddingProtocolError>> {
        let active_execution = self
            .try_initialized_engine()
            .and_then(|engine| engine.admission_snapshot().active_request)
            .is_some_and(|active| active.request_class == request_class);
        self.request_admission
            .try_acquire(request_class, active_execution)
            .map_err(|depths| {
                let active = self
                    .active
                    .lock()
                    .ok()
                    .and_then(|active| active.values().next().cloned());
                let owner_state = self
                    .try_initialized_engine()
                    .map(|engine| engine.admission_snapshot().owner_state)
                    .unwrap_or(EmbeddingOwnerState::Waking);
                let pressure = EmbeddingCapacityPressureWire {
                    reason: EmbeddingCapacityReason::QueueFull.as_str().into(),
                    queue_class: request_class.as_str().into(),
                    capacity: ServerRequestAdmissionDepths::capacity(request_class) as u64,
                    depth: depths
                        .depth(request_class)
                        .saturating_sub(usize::from(active_execution))
                        as u64,
                    retry_after_ms,
                    retry_condition: "an admitted request completes or is cancelled".into(),
                    owner_state: owner_state.as_str().into(),
                    active_scope_id: active.as_ref().map(|active| active.scope_id.clone()),
                    active_request_id: active.as_ref().map(|active| active.request_id.clone()),
                    active_request_class: active
                        .as_ref()
                        .map(|active| active.request_class.as_str().into()),
                };
                Box::new(EmbeddingProtocolError {
                    code: "embedding_capacity".into(),
                    message: format!("{} request admission is full", request_class.as_str()),
                    retry_class: "after_capacity_change".into(),
                    retry_after_ms,
                    retry_condition: pressure.retry_condition.clone(),
                    capacity: Some(pressure),
                })
            })
    }

    fn connection_capacity_error(
        &self,
        reason: &str,
        capacity: usize,
        depth: usize,
    ) -> EmbeddingProtocolError {
        let active = self
            .active
            .lock()
            .ok()
            .and_then(|active| active.values().next().cloned());
        let owner_state = self
            .try_initialized_engine()
            .map(|engine| engine.admission_snapshot().owner_state)
            .unwrap_or(EmbeddingOwnerState::Waking);
        let pressure = EmbeddingCapacityPressureWire {
            reason: reason.into(),
            queue_class: "connection".into(),
            capacity: capacity as u64,
            depth: depth as u64,
            retry_after_ms: duration_ms(EmbeddingClientBudgets::current().retry_after),
            retry_condition: "an authenticated connection handler completes".into(),
            owner_state: owner_state.as_str().into(),
            active_scope_id: active.as_ref().map(|active| active.scope_id.clone()),
            active_request_id: active.as_ref().map(|active| active.request_id.clone()),
            active_request_class: active
                .as_ref()
                .map(|active| active.request_class.as_str().into()),
        };
        EmbeddingProtocolError {
            code: "embedding_capacity".into(),
            message: "embedding connection admission is full".into(),
            retry_class: "after_capacity_change".into(),
            retry_after_ms: pressure.retry_after_ms,
            retry_condition: pressure.retry_condition.clone(),
            capacity: Some(pressure),
        }
    }

    fn try_begin_connection(self: &Arc<Self>) -> Option<ServerConnectionGuard> {
        self.connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |connections| {
                (connections < SERVER_CONNECTION_HANDLER_CAPACITY).then_some(connections + 1)
            })
            .ok()?;
        self.bump_event();
        Some(ServerConnectionGuard {
            state: Arc::clone(self),
        })
    }

    fn try_begin_rejection_connection(self: &Arc<Self>) -> Option<ServerConnectionGuard> {
        self.connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |connections| {
                (connections < SERVER_TOTAL_CONNECTION_HANDLER_CAPACITY).then_some(connections + 1)
            })
            .ok()?;
        self.bump_event();
        Some(ServerConnectionGuard {
            state: Arc::clone(self),
        })
    }

    fn try_begin_pre_request(self: &Arc<Self>) -> Option<ServerPreRequestGuard> {
        self.pre_request_connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |connections| {
                (connections < SERVER_CONTROL_CONNECTION_RESERVE).then_some(connections + 1)
            })
            .ok()?;
        Some(ServerPreRequestGuard {
            state: Arc::clone(self),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_request(
        self: &Arc<Self>,
        connection_id: &str,
        request_id: &str,
        scope_id: &str,
        request_class: EmbeddingRequestClass,
        phase: &str,
        context: EmbeddingRequestContext,
        admission: ServerRequestAdmissionPermit,
        cancellation_auth: Option<ServerCancellationAuth>,
    ) -> Result<ServerRequestGuard> {
        let _admission = self
            .admission_gate
            .lock()
            .map_err(|_| anyhow!("embedding_server_admission_gate_poisoned"))?;
        if self.draining.load(Ordering::Acquire) {
            bail!("embedding_server_draining");
        }
        let key = request_key(connection_id, request_id);
        let mut active = self
            .active
            .lock()
            .map_err(|_| anyhow!("embedding_server_active_state_poisoned"))?;
        let mut cancellations = self
            .cancellations
            .lock()
            .map_err(|_| anyhow!("embedding_server_cancellation_state_poisoned"))?;
        if active.contains_key(&key) {
            bail!("embedding_server_duplicate_request_id");
        }
        active.insert(
            key.clone(),
            ActiveServerRequest {
                request_id: request_id.into(),
                scope_id: scope_id.into(),
                request_class,
                phase: phase.into(),
                started_ns: self.clock.now_ns(),
            },
        );
        cancellations.insert(
            key.clone(),
            ServerCancellation {
                context,
                admission: admission.clone(),
                auth: cancellation_auth,
            },
        );
        drop(cancellations);
        drop(active);
        self.bump_event();
        Ok(ServerRequestGuard {
            state: Arc::clone(self),
            key: Some(key),
            _admission: admission,
        })
    }

    fn cancel(
        &self,
        request_id: &str,
        cancel_token: &str,
        client_pid: u32,
        client_process_start_id: &str,
    ) -> bool {
        self.cancellations.lock().ok().is_some_and(|requests| {
            let suffix = format!(":{request_id}");
            let mut matches = requests
                .iter()
                .filter(|(key, cancellation)| {
                    key.ends_with(&suffix)
                        && cancellation.auth.as_ref().is_some_and(|auth| {
                            auth.token == cancel_token
                                && auth.client_pid == client_pid
                                && auth.client_process_start_id == client_process_start_id
                        })
                })
                .map(|(_, context)| context);
            let first = matches.next();
            if matches.next().is_some() {
                return false;
            }
            first.is_some_and(|cancellation| {
                let cancelled = cancellation.context.cancel();
                if cancelled {
                    cancellation.admission.release();
                }
                cancelled
            })
        })
    }

    fn update_request_phase(&self, key: &str, phase: &str) {
        if let Ok(mut active) = self.active.lock()
            && let Some(request) = active.get_mut(key)
            && request.phase != phase
        {
            request.phase = phase.into();
            self.bump_event();
        }
    }

    fn finish_request(&self, key: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(key);
        }
        if let Ok(mut cancellations) = self.cancellations.lock() {
            cancellations.remove(key);
        }
        self.restart_idle_window();
        self.bump_event();
    }

    fn restart_idle_window(&self) {
        self.last_work_ended_ns
            .store(self.clock.now_ns(), Ordering::Release);
    }

    fn true_idle(&self) -> bool {
        if self.active.lock().map_or(true, |active| !active.is_empty()) {
            return false;
        }
        if self.request_admission.snapshot() != ServerRequestAdmissionDepths::default() {
            return false;
        }
        self.initialized_engine().is_none_or(|engine| {
            let admission = engine.admission_snapshot();
            admission.query_depth == 0
                && admission.bulk_depth == 0
                && admission.active_request_count == 0
                && admission.lease_count == 0
        })
    }

    fn begin_draining_if_idle(&self) -> bool {
        let Ok(_admission) = self.admission_gate.lock() else {
            return false;
        };
        if self
            .draining
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return self.true_idle();
        }
        if !self.true_idle() {
            self.draining.store(false, Ordering::Release);
            return false;
        }
        if let Some(engine) = self.initialized_engine()
            && !engine.begin_draining_if_idle()
        {
            self.draining.store(false, Ordering::Release);
            return false;
        }
        self.bump_event();
        true
    }

    fn record_failure(&self, failure: EmbeddingServerFailureSnapshot) {
        if let Ok(mut last_failure) = self.last_failure.lock() {
            *last_failure = Some(failure);
        }
        self.bump_event();
    }

    fn bump_event(&self) {
        self.event_sequence.fetch_add(1, Ordering::AcqRel);
    }

    fn snapshot(&self) -> EmbeddingServerSnapshot {
        // Status and every Hello must remain bounded while another request is
        // performing the cold native load under the engine mutex.
        let engine = self.try_initialized_engine();
        let lifecycle = engine.as_ref().and_then(|engine| engine.snapshot().ok());
        let admission = engine.as_ref().map(EmbeddingEngine::admission_snapshot);
        let front_admission = self.request_admission.snapshot();
        let active = self.active.lock().ok().and_then(|active| {
            admission
                .as_ref()
                .and_then(|admission| admission.active_request.as_ref())
                .and_then(|native| {
                    active
                        .values()
                        .find(|candidate| {
                            candidate.request_id == native.request_id
                                && candidate.scope_id == native.scope_id
                                && candidate.request_class == native.request_class
                        })
                        .cloned()
                })
                .or_else(|| {
                    admission
                        .is_none()
                        .then(|| active.values().next().cloned())
                        .flatten()
                })
        });
        let scheduler = match admission.as_ref() {
            Some(admission) => scheduler_snapshot(
                admission,
                self.connections.load(Ordering::Acquire),
                active.as_ref(),
                self.clock.as_ref(),
            ),
            None => EmbeddingServerSchedulerSnapshot {
                query_capacity: EMBEDDING_QUERY_QUEUE_CAPACITY as u64,
                query_depth: front_admission.query as u64,
                bulk_capacity: EMBEDDING_BULK_QUEUE_CAPACITY as u64,
                bulk_depth: front_admission.bulk as u64,
                connection_count: self.connections.load(Ordering::Acquire) as u64,
                active_request_count: self.active.lock().map_or(0, |active| active.len() as u64),
                lease_count: 0,
                active_request: active
                    .as_ref()
                    .map(|active| active_request_snapshot(active, self.clock.as_ref())),
            },
        };
        let engine_snapshot = lifecycle
            .as_ref()
            .map(|lifecycle| EmbeddingServerEngineSnapshot {
                engine_owner_id: format!("{}:engine-owner", self.process.server_instance_id),
                native_worker_id: format!(
                    "{}:native-worker:{}",
                    self.process.server_instance_id, lifecycle.load_generation
                ),
                load_generation: lifecycle.load_generation,
                model_load_count: lifecycle.model_load_count,
                successful_encode_count: lifecycle.identity.encode_count,
            });
        let lifecycle_name = if self.draining.load(Ordering::Acquire) {
            "draining"
        } else {
            lifecycle
                .as_ref()
                .map_or("listening", |lifecycle| lifecycle.residency.as_str())
        };
        EmbeddingServerSnapshot {
            schema_version: PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
            event_sequence: self.event_sequence.load(Ordering::Acquire),
            lifecycle: lifecycle_name.into(),
            clock: self.clock.snapshot(),
            protocol: self.protocol.clone(),
            authority: self.authority.clone(),
            process: self.process.clone(),
            scheduler,
            engine: engine_snapshot,
            failure: self
                .last_failure
                .lock()
                .ok()
                .and_then(|failure| failure.clone()),
        }
    }

    fn shutdown_engine(&self) {
        match self.engine.lock() {
            Ok(mut engine) => {
                engine.take();
            }
            Err(poisoned) => {
                poisoned.into_inner().take();
            }
        }
    }
}

struct ServerLeaseActivity<L> {
    state: Arc<PerUserEmbeddingServerState>,
    lease: Option<L>,
}

impl<L> ServerLeaseActivity<L> {
    fn new(state: &Arc<PerUserEmbeddingServerState>, lease: L) -> Self {
        Self {
            state: Arc::clone(state),
            lease: Some(lease),
        }
    }

    fn lease(&self) -> &L {
        self.lease
            .as_ref()
            .expect("server lease activity remains live until drop")
    }
}

impl<L> Drop for ServerLeaseActivity<L> {
    fn drop(&mut self) {
        // Reset the idle clock before the native lease count becomes zero, so
        // the accept loop can never observe true idle with the old timestamp.
        self.state.restart_idle_window();
        self.lease.take();
        self.state.bump_event();
    }
}

struct ServerRequestGuard {
    state: Arc<PerUserEmbeddingServerState>,
    key: Option<String>,
    _admission: ServerRequestAdmissionPermit,
}

impl Drop for ServerRequestGuard {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.state.finish_request(&key);
        }
    }
}

impl ServerRequestGuard {
    fn update_phase(&self, phase: &str) {
        if let Some(key) = self.key.as_deref() {
            self.state.update_request_phase(key, phase);
        }
    }
}

struct ServerConnectionGuard {
    state: Arc<PerUserEmbeddingServerState>,
}

impl Drop for ServerConnectionGuard {
    fn drop(&mut self) {
        self.state.connections.fetch_sub(1, Ordering::AcqRel);
        self.state.bump_event();
    }
}

struct ServerPreRequestGuard {
    state: Arc<PerUserEmbeddingServerState>,
}

impl Drop for ServerPreRequestGuard {
    fn drop(&mut self) {
        self.state
            .pre_request_connections
            .fetch_sub(1, Ordering::AcqRel);
    }
}

fn server_qualification_control_from_env() -> Result<Option<ServerQualificationControl>> {
    server_qualification_control_from_values(
        std::env::var_os("CODESTORY_EMBED_QUALIFICATION_DIR"),
        std::env::var("CODESTORY_EMBED_QUALIFICATION_NONCE").ok(),
    )
}

fn server_qualification_control_from_values(
    directory: Option<std::ffi::OsString>,
    nonce: Option<String>,
) -> Result<Option<ServerQualificationControl>> {
    match (directory, nonce) {
        (None, None) => Ok(None),
        (Some(directory), Some(nonce))
            if !directory.is_empty()
                && !nonce.is_empty()
                && nonce.len() <= 128
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) =>
        {
            let directory = PinnedQualificationDirectory::open(&PathBuf::from(directory))?;
            let events = ServerQualificationEventLog::open(&directory, &nonce)?;
            let last_sequence = events.last_sequence;
            Ok(Some(ServerQualificationControl {
                directory,
                events: Mutex::new(events),
                nonce_sha256: hex_sha256(nonce.as_bytes()),
                nonce,
                last_sequence: AtomicU64::new(last_sequence),
                processed_command_sha256: Mutex::new(None),
                force_incompatible: AtomicBool::new(false),
                freeze_owner: AtomicBool::new(false),
            }))
        }
        _ => bail!("embedding_qualification_gate_incomplete"),
    }
}

impl PinnedQualificationDirectory {
    fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            bail!("embedding_qualification_directory_not_absolute");
        }
        let source = fs::symlink_metadata(path).with_context(|| {
            format!(
                "inspect embedding qualification directory {}",
                path.display()
            )
        })?;
        validate_private_qualification_directory_metadata(&source)?;
        let canonical = fs::canonicalize(path).with_context(|| {
            format!(
                "canonicalize embedding qualification directory {}",
                path.display()
            )
        })?;
        #[cfg(not(windows))]
        if canonical != path {
            bail!("embedding_qualification_directory_untrusted");
        }
        let metadata = fs::symlink_metadata(&canonical)
            .context("reinspect canonical embedding qualification directory")?;
        validate_private_qualification_directory_metadata(&metadata)?;
        let identity = native_path_identity(&canonical)?;
        #[cfg(windows)]
        {
            let caller = fs::symlink_metadata(path)
                .context("reinspect caller embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&caller)?;
            if native_path_identity(path)? != identity {
                bail!("embedding_qualification_directory_untrusted");
            }
        }
        #[cfg(unix)]
        let handle = {
            use std::os::unix::fs::OpenOptionsExt;
            let handle = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&canonical)
                .context("pin embedding qualification directory")?;
            let opened = handle
                .metadata()
                .context("inspect pinned embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&opened)?;
            if native_file_identity(&handle)? != identity {
                bail!("embedding_qualification_directory_replaced");
            }
            handle
        };
        let pinned = Self {
            path: canonical,
            identity,
            #[cfg(unix)]
            handle,
        };
        pinned.revalidate()?;
        Ok(pinned)
    }

    fn revalidate(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.path)
            .context("revalidate embedding qualification directory")?;
        validate_private_qualification_directory_metadata(&metadata)?;
        if native_path_identity(&self.path)? != self.identity {
            bail!("embedding_qualification_directory_replaced");
        }
        #[cfg(unix)]
        {
            let opened = self
                .handle
                .metadata()
                .context("revalidate pinned embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&opened)?;
            if native_file_identity(&self.handle)? != self.identity {
                bail!("embedding_qualification_directory_replaced");
            }
        }
        Ok(())
    }

    fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }
}

impl ServerQualificationEventLog {
    fn open(directory: &PinnedQualificationDirectory, nonce: &str) -> Result<Self> {
        directory.revalidate()?;
        let path = directory.join(format!("{nonce}.events.jsonl"));
        let mut create = OpenOptions::new();
        create.read(true).append(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            create.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = match create.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&path)
                    .context("inspect existing embedding qualification event log")?;
                validate_private_qualification_file_metadata(
                    &metadata,
                    SERVER_QUALIFICATION_MAX_EVENT_BYTES,
                )?;
                let mut existing = OpenOptions::new();
                existing.read(true).append(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    existing.custom_flags(libc::O_NOFOLLOW);
                }
                existing
                    .open(&path)
                    .context("open existing embedding qualification event log")?
            }
            Err(error) => {
                return Err(error).context("create private embedding qualification event log");
            }
        };
        directory.revalidate()?;
        let metadata = file
            .metadata()
            .context("inspect opened embedding qualification event log")?;
        validate_private_qualification_file_metadata(
            &metadata,
            SERVER_QUALIFICATION_MAX_EVENT_BYTES,
        )?;
        let identity = native_file_identity(&file)?;
        let path_metadata =
            fs::symlink_metadata(&path).context("reinspect embedding qualification event log")?;
        validate_private_qualification_file_metadata(
            &path_metadata,
            SERVER_QUALIFICATION_MAX_EVENT_BYTES,
        )?;
        if native_path_identity(&path)? != identity {
            bail!("embedding_qualification_event_log_replaced");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.try_clone()
            .context("clone embedding qualification event log")?
            .take(SERVER_QUALIFICATION_MAX_EVENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("read existing embedding qualification event log")?;
        if bytes.len() as u64 > SERVER_QUALIFICATION_MAX_EVENT_BYTES
            || (!bytes.is_empty() && !bytes.ends_with(b"\n"))
        {
            bail!("embedding_qualification_event_log_untrusted");
        }
        let mut records = 0_u64;
        let mut last_sequence = 0_u64;
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let event = serde_json::from_slice::<ExistingServerQualificationEvent>(line)
                .context("parse existing embedding qualification event")?;
            if event.schema_version != 1 {
                bail!("embedding_qualification_event_log_untrusted");
            }
            records = records.saturating_add(1);
            last_sequence = last_sequence.max(event.sequence);
        }
        if records > SERVER_QUALIFICATION_MAX_EVENT_RECORDS {
            bail!("embedding_qualification_event_log_record_limit");
        }
        Ok(Self {
            path,
            file,
            identity,
            bytes: bytes.len() as u64,
            records,
            last_sequence,
        })
    }

    fn record(
        &mut self,
        directory: &PinnedQualificationDirectory,
        event: &ServerQualificationEvent,
    ) -> Result<()> {
        directory.revalidate()?;
        let path_metadata = fs::symlink_metadata(&self.path)
            .context("revalidate embedding qualification event log path")?;
        validate_private_qualification_file_metadata(
            &path_metadata,
            SERVER_QUALIFICATION_MAX_EVENT_BYTES,
        )?;
        let opened = self
            .file
            .metadata()
            .context("revalidate opened embedding qualification event log")?;
        if native_path_identity(&self.path)? != self.identity
            || native_file_identity(&self.file)? != self.identity
            || opened.len() != self.bytes
        {
            bail!("embedding_qualification_event_log_replaced");
        }
        let mut encoded =
            serde_json::to_vec(event).context("encode embedding qualification event")?;
        encoded.push(b'\n');
        if self.records >= SERVER_QUALIFICATION_MAX_EVENT_RECORDS
            || self.bytes.saturating_add(encoded.len() as u64)
                > SERVER_QUALIFICATION_MAX_EVENT_BYTES
        {
            bail!("embedding_qualification_event_log_limit");
        }
        self.file
            .write_all(&encoded)
            .context("append embedding qualification event")?;
        self.file
            .flush()
            .context("flush embedding qualification event")?;
        self.file
            .sync_all()
            .context("sync embedding qualification event")?;
        let next_bytes = self.bytes + encoded.len() as u64;
        directory.revalidate()?;
        let path_metadata = fs::symlink_metadata(&self.path)
            .context("reinspect embedding qualification event log after append")?;
        let opened = self
            .file
            .metadata()
            .context("reinspect opened embedding qualification event log after append")?;
        if native_path_identity(&self.path)? != self.identity
            || native_file_identity(&self.file)? != self.identity
            || path_metadata.len() != next_bytes
            || opened.len() != next_bytes
        {
            bail!("embedding_qualification_event_log_replaced");
        }
        self.bytes = next_bytes;
        self.records += 1;
        self.last_sequence = self.last_sequence.max(event.sequence);
        Ok(())
    }
}

fn validate_private_qualification_directory_metadata(metadata: &fs::Metadata) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("embedding_qualification_directory_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_qualification_directory_untrusted");
        }
    }
    Ok(())
}

fn validate_private_qualification_file_metadata(
    metadata: &fs::Metadata,
    maximum_bytes: u64,
) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum_bytes {
        bail!("embedding_qualification_file_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            bail!("embedding_qualification_file_untrusted");
        }
    }
    Ok(())
}

fn native_path_identity(path: &Path) -> Result<NativeFileIdentity> {
    workspace_path_identity(path)
        .context("embedding qualification filesystem path identity is unavailable")
}

fn native_file_identity(file: &File) -> Result<NativeFileIdentity> {
    workspace_file_identity(file)
        .context("embedding qualification filesystem file identity is unavailable")
}

fn read_server_qualification_command(
    control: &ServerQualificationControl,
) -> Result<Option<ServerQualificationCommandFile>> {
    control.directory.revalidate()?;
    let path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    let path_metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).context("inspect embedding qualification command");
        }
    };
    validate_private_qualification_file_metadata(
        &path_metadata,
        SERVER_QUALIFICATION_MAX_COMMAND_BYTES,
    )?;
    let identity = native_path_identity(&path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(&path)
        .context("open embedding qualification command")?;
    let opened = file
        .metadata()
        .context("inspect opened embedding qualification command")?;
    validate_private_qualification_file_metadata(&opened, SERVER_QUALIFICATION_MAX_COMMAND_BYTES)?;
    if native_file_identity(&file)? != identity {
        bail!("embedding_qualification_command_replaced");
    }
    control.directory.revalidate()?;
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.take(SERVER_QUALIFICATION_MAX_COMMAND_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read embedding qualification command")?;
    if bytes.len() as u64 > SERVER_QUALIFICATION_MAX_COMMAND_BYTES {
        bail!("embedding_qualification_command_limit");
    }
    let path_metadata =
        fs::symlink_metadata(&path).context("reinspect embedding qualification command")?;
    validate_private_qualification_file_metadata(
        &path_metadata,
        SERVER_QUALIFICATION_MAX_COMMAND_BYTES,
    )?;
    if native_path_identity(&path)? != identity {
        bail!("embedding_qualification_command_replaced");
    }
    control.directory.revalidate()?;
    Ok(Some(ServerQualificationCommandFile { bytes }))
}

fn poll_server_qualification_command(
    state: &Arc<PerUserEmbeddingServerState>,
    transport: &dyn EmbeddingServerTransport,
) -> Result<()> {
    let Some(control) = state.qualification.as_ref() else {
        return Ok(());
    };
    let Some(command_file) = read_server_qualification_command(control)? else {
        return Ok(());
    };
    let command_sha256 = hex_sha256(&command_file.bytes);
    if control.command_was_processed(&command_sha256) {
        return Ok(());
    }
    let parsed = serde_json::from_slice::<ServerQualificationCommand>(&command_file.bytes);
    if parsed.as_ref().is_ok_and(|command| {
        command.schema_version == 1
            && command.nonce_sha256 == control.nonce_sha256
            && command.sequence <= control.last_sequence.load(Ordering::Acquire)
    }) {
        control.mark_command_processed(command_sha256);
        return Ok(());
    }
    let (sequence, action) = parsed
        .as_ref()
        .map(|command| (command.sequence, command.action.clone()))
        .unwrap_or_else(|_| (0, "invalid".into()));
    let mut status = "completed";
    let mut details = None;
    let mut snapshot = None;
    let mut crash = false;
    match parsed {
        Ok(command)
            if command.schema_version == 1
                && command.nonce_sha256 == control.nonce_sha256
                && command.sequence > control.last_sequence.load(Ordering::Acquire) =>
        {
            let result = match command.action.as_str() {
                "crash_server" => {
                    crash = true;
                    status = "accepted";
                    Ok(())
                }
                "stall_native" => {
                    codestory_llama_sys::set_embedding_qualification_native_stall(true);
                    Ok(())
                }
                "release_native" => {
                    codestory_llama_sys::set_embedding_qualification_native_stall(false);
                    Ok(())
                }
                "hold_class" => qualification_hold_class(command.parameters.class.as_deref(), true),
                "release_class" => {
                    qualification_hold_class(command.parameters.class.as_deref(), false)
                }
                "force_incompatible" => {
                    control.force_incompatible.store(true, Ordering::Release);
                    Ok(())
                }
                "clear_incompatible" => {
                    control.force_incompatible.store(false, Ordering::Release);
                    Ok(())
                }
                "snapshot" => {
                    let current = state.snapshot();
                    details = Some(std::collections::BTreeMap::from([
                        (
                            "idle_epoch_ns".into(),
                            state.last_work_ended_ns.load(Ordering::Acquire).to_string(),
                        ),
                        ("true_idle".into(), state.true_idle().to_string()),
                        ("clock_domain".into(), current.clock.domain.clone()),
                        ("clock_boot_id".into(), current.clock.boot_id.clone()),
                        (
                            "server_instance_id".into(),
                            current.process.server_instance_id.clone(),
                        ),
                    ]));
                    snapshot = Some(current);
                    Ok(())
                }
                "freeze_owner" => {
                    control.freeze_owner.store(true, Ordering::Release);
                    Ok(())
                }
                "release_owner" => {
                    control.freeze_owner.store(false, Ordering::Release);
                    Ok(())
                }
                _ => bail!("embedding_qualification_action_unknown"),
            };
            if let Err(error) = result {
                status = "failed";
                details = Some(opaque_qualification_details(&error));
            }
            control
                .last_sequence
                .store(command.sequence, Ordering::Release);
        }
        Ok(_) => {
            status = "failed";
            details = Some(qualification_detail(
                "code",
                "embedding_qualification_command_rejected",
            ));
        }
        Err(_) => {
            status = "failed";
            details = Some(qualification_detail(
                "code",
                "embedding_qualification_command_invalid",
            ));
        }
    }
    write_server_qualification_event(
        control,
        state,
        ServerQualificationEvent {
            schema_version: 1,
            sequence,
            action,
            status: status.into(),
            server_event_sequence: state.event_sequence.load(Ordering::Acquire),
            clock: {
                let clock = state.clock.snapshot();
                ServerQualificationEventClock {
                    domain: clock.domain,
                    api: clock.api,
                    boot_id: clock.boot_id,
                    observed_ns: state.clock.now_ns(),
                }
            },
            snapshot,
            details,
        },
    )?;
    control.mark_command_processed(command_sha256);
    if crash {
        transport.fail_stop("embedding_qualification_crash");
        state.draining.store(true, Ordering::Release);
    }
    Ok(())
}

fn qualification_hold_class(class: Option<&str>, hold: bool) -> Result<()> {
    match class {
        Some("query") => {
            codestory_llama_sys::set_embedding_qualification_class_hold(
                EmbeddingRequestClass::Query,
                hold,
            );
            Ok(())
        }
        Some("bulk") => {
            codestory_llama_sys::set_embedding_qualification_class_hold(
                EmbeddingRequestClass::Bulk,
                hold,
            );
            Ok(())
        }
        _ => bail!("embedding_qualification_class_invalid"),
    }
}

fn write_server_qualification_event(
    control: &ServerQualificationControl,
    _state: &PerUserEmbeddingServerState,
    event: ServerQualificationEvent,
) -> Result<()> {
    control
        .events
        .lock()
        .map_err(|_| anyhow!("embedding_qualification_event_log_poisoned"))?
        .record(&control.directory, &event)
}

fn opaque_qualification_details(
    error: &anyhow::Error,
) -> std::collections::BTreeMap<String, String> {
    qualification_detail(
        "code",
        error
            .to_string()
            .split(':')
            .next()
            .unwrap_or("embedding_qualification_failed"),
    )
}

fn qualification_detail(key: &str, value: &str) -> std::collections::BTreeMap<String, String> {
    [(key.into(), value.into())].into_iter().collect()
}

pub fn run_per_user_embedding_server(config: PerUserEmbeddingServerConfig) -> Result<()> {
    validate_server_config(&config)?;
    let listener = match config.transport.bind().map_err(anyhow::Error::new)? {
        EmbeddingServerBindOutcome::Bound(listener) => {
            Arc::<dyn EmbeddingServerListener>::from(listener)
        }
        EmbeddingServerBindOutcome::AlreadyOwned => return Ok(()),
    };
    let authority = listener.identity().clone();
    if !authority.peer_verified {
        bail!("embedding_server_listener_peer_proof_missing");
    }
    let clock = config.transport.clock();
    let server_instance_id = Uuid::new_v4().to_string();
    let state = Arc::new(PerUserEmbeddingServerState {
        clock: Arc::clone(&clock),
        engine_cache_root: config.engine_cache_root,
        engine_config: native_engine_config(config.allow_cpu)?,
        engine: Mutex::new(None),
        process: EmbeddingServerProcessSnapshot {
            server_instance_id,
            pid: config.executable.pid,
            process_start_id: config.executable.process_start_id,
            executable_sha256: config.executable.executable_sha256,
            executable_version: config.executable.executable_version,
        },
        protocol: config.protocol,
        authority: EmbeddingServerAuthoritySnapshot {
            endpoint_namespace_id: authority.endpoint_namespace_id,
            lifetime_authority_id: authority.lifetime_authority_id,
            listener_id: authority.listener_id,
            peer_verified: authority.peer_verified,
        },
        connections: AtomicUsize::new(0),
        pre_request_connections: AtomicUsize::new(0),
        admission_gate: Mutex::new(()),
        request_admission: Arc::new(ServerRequestAdmission::default()),
        active: Mutex::new(std::collections::BTreeMap::new()),
        cancellations: Mutex::new(std::collections::BTreeMap::new()),
        draining: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        last_work_ended_ns: AtomicU64::new(clock.now_ns()),
        event_sequence: AtomicU64::new(1),
        last_failure: Mutex::new(None),
        qualification: server_qualification_control_from_env()?,
    });

    let watchdog = spawn_server_watchdog(
        Arc::clone(&state),
        Arc::clone(&config.transport),
        config.budgets,
    )?;
    let mut connections = Vec::new();
    let serve_result = (|| -> Result<()> {
        loop {
            poll_server_qualification_command(&state, config.transport.as_ref())?;
            if state
                .qualification
                .as_ref()
                .is_some_and(|control| control.freeze_owner.load(Ordering::Acquire))
            {
                clock.sleep(SERVER_ACCEPT_POLL);
                continue;
            }
            if state.draining.load(Ordering::Acquire) {
                break;
            }
            if state.true_idle()
                && elapsed_since(
                    clock.as_ref(),
                    state.last_work_ended_ns.load(Ordering::Acquire),
                ) >= config.budgets.idle_timeout
                && state.begin_draining_if_idle()
            {
                break;
            }
            match listener.accept(SERVER_ACCEPT_POLL) {
                Ok(Some(stream)) => {
                    if let Some(connection_guard) = state.try_begin_connection() {
                        let state_for_connection = Arc::clone(&state);
                        connections.push(
                            thread::Builder::new()
                                .name("codestory-embedding-connection".into())
                                .spawn(move || {
                                    let _guard = connection_guard;
                                    if let Err(error) =
                                        serve_embedding_connection(state_for_connection, stream)
                                    {
                                        tracing::debug!(
                                            error = %error,
                                            "embedding connection closed"
                                        );
                                    }
                                })
                                .context("spawn embedding connection handler")?,
                        );
                    } else if let Some(rejection_guard) = state.try_begin_rejection_connection() {
                        let state_for_rejection = Arc::clone(&state);
                        connections.push(
                            thread::Builder::new()
                                .name("codestory-embedding-capacity-rejection".into())
                                .spawn(move || {
                                    let _guard = rejection_guard;
                                    if let Err(error) =
                                        serve_embedding_connection_at_handler_capacity(
                                            state_for_rejection,
                                            stream,
                                        )
                                    {
                                        tracing::debug!(
                                            error = %error,
                                            "embedding capacity rejection closed"
                                        );
                                    }
                                })
                                .context("spawn embedding capacity rejection handler")?,
                        );
                    } else {
                        // Total live handlers remain hard bounded even when
                        // hostile partial handshakes occupy the rejection
                        // reserve.
                        let _ = stream.shutdown();
                    }
                }
                Ok(None) => {}
                Err(_error) if state.draining.load(Ordering::Acquire) => break,
                Err(error) => return Err(anyhow::Error::new(error)),
            }
            reap_finished_connection_handlers(&mut connections);
        }
        Ok(())
    })();

    state.draining.store(true, Ordering::Release);
    let _ = listener.close();
    let state_for_cleanup = Arc::clone(&state);
    let cleanup = thread::Builder::new()
        .name("codestory-embedding-cleanup".into())
        .spawn(move || {
            state_for_cleanup.shutdown_engine();
            state_for_cleanup.stopped.store(true, Ordering::Release);
        })
        .context("spawn embedding server cleanup")?;
    let _ = watchdog.join();
    if cleanup.is_finished() {
        let _ = cleanup.join();
    }
    serve_result
}

fn reap_finished_connection_handlers(connections: &mut Vec<thread::JoinHandle<()>>) {
    connections.retain(|connection| !connection.is_finished());
}

fn validate_server_config(config: &PerUserEmbeddingServerConfig) -> Result<()> {
    if config.budgets.idle_timeout
        != Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
    {
        bail!("embedding_server_idle_timeout_contract_mismatch");
    }
    if config.budgets.native_no_progress.is_zero()
        || config.budgets.watchdog_poll.is_zero()
        || config.protocol.bootstrap_version != PER_USER_EMBEDDING_BOOTSTRAP_VERSION
        || config.protocol.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || config.protocol.protocol_sha256 != PER_USER_EMBEDDING_PROTOCOL_SHA256
        || config.protocol.constant_set_sha256 != PER_USER_EMBEDDING_CONSTANT_SET_SHA256
        || config.protocol.measurement_protocol_sha256
            != PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256
    {
        bail!("embedding_server_constant_contract_mismatch");
    }
    for value in [
        config.executable.process_start_id.as_str(),
        config.executable.executable_sha256.as_str(),
        config.executable.executable_version.as_str(),
    ] {
        if value.trim().is_empty() {
            bail!("embedding_server_process_identity_incomplete");
        }
    }
    Ok(())
}

fn spawn_server_watchdog(
    state: Arc<PerUserEmbeddingServerState>,
    transport: Arc<dyn EmbeddingServerTransport>,
    budgets: EmbeddingServerBudgets,
) -> Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("codestory-embedding-watchdog".into())
        .spawn(move || {
            let started_ns = state.clock.now_ns();
            let mut query_progress = WatchdogClassProgress::new(started_ns);
            let mut bulk_progress = WatchdogClassProgress::new(started_ns);
            let mut draining_progress = WatchdogClassProgress::new(started_ns);
            while !state.stopped.load(Ordering::Acquire) {
                state.clock.sleep(budgets.watchdog_poll);
                if state.stopped.load(Ordering::Acquire) {
                    return;
                }
                let draining = state.draining.load(Ordering::Acquire);
                let active_classes = state.active.lock().map_or_else(
                    |_| ActiveRequestClasses::default(),
                    |active| ActiveRequestClasses {
                        query: active
                            .values()
                            .any(|request| request.request_class == EmbeddingRequestClass::Query),
                        bulk: active
                            .values()
                            .any(|request| request.request_class == EmbeddingRequestClass::Bulk),
                    },
                );
                let progress = state
                    .try_initialized_engine()
                    .map(|engine| {
                        let admission = engine.admission_snapshot();
                        WatchdogProgressSnapshot {
                            overall: admission.progress_sequence,
                            query: admission.query_progress_sequence,
                            bulk: admission.bulk_progress_sequence,
                        }
                    })
                    .unwrap_or_default();
                let stalled = query_progress
                    .observe(
                        active_classes.query,
                        progress.query,
                        state.clock.as_ref(),
                        budgets.native_no_progress,
                    )
                    .or_else(|| {
                        bulk_progress.observe(
                            active_classes.bulk,
                            progress.bulk,
                            state.clock.as_ref(),
                            budgets.native_no_progress,
                        )
                    })
                    .or_else(|| {
                        draining_progress.observe(
                            draining && !active_classes.query && !active_classes.bulk,
                            progress.overall,
                            state.clock.as_ref(),
                            budgets.native_no_progress,
                        )
                    });
                if let Some(stalled) = stalled {
                    state.record_failure(EmbeddingServerFailureSnapshot {
                        code: "embedding_engine_stalled".into(),
                        retry_class: "same_rpc_once".into(),
                        retry_after_ms: 0,
                        retry_condition: "the server instance changes".into(),
                    });
                    if let Some(control) = state.qualification.as_ref()
                        && let Err(error) = publish_watchdog_fail_stop_marker(
                            control,
                            &state,
                            budgets,
                            stalled.sequence,
                            stalled.last_progress_ns,
                        )
                    {
                        tracing::error!(
                            error = %error,
                            "failed to publish embedding qualification watchdog marker"
                        );
                    }
                    transport.fail_stop("embedding_engine_stalled");
                    state.draining.store(true, Ordering::Release);
                    state.stopped.store(true, Ordering::Release);
                    return;
                }
            }
        })
        .context("spawn embedding server watchdog")
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ActiveRequestClasses {
    query: bool,
    bulk: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct WatchdogProgressSnapshot {
    overall: u64,
    query: u64,
    bulk: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WatchdogStall {
    sequence: u64,
    last_progress_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WatchdogClassProgress {
    sequence: u64,
    last_progress_ns: u64,
    was_active: bool,
}

impl WatchdogClassProgress {
    fn new(now_ns: u64) -> Self {
        Self {
            sequence: 0,
            last_progress_ns: now_ns,
            was_active: false,
        }
    }

    fn observe(
        &mut self,
        active: bool,
        sequence: u64,
        clock: &dyn AwakeMonotonicClock,
        timeout: Duration,
    ) -> Option<WatchdogStall> {
        if !active {
            self.was_active = false;
            self.sequence = sequence;
            self.last_progress_ns = clock.now_ns();
            return None;
        }
        if !self.was_active || sequence != self.sequence {
            self.was_active = true;
            self.sequence = sequence;
            self.last_progress_ns = clock.now_ns();
            return None;
        }
        (elapsed_since(clock, self.last_progress_ns) >= timeout).then_some(WatchdogStall {
            sequence,
            last_progress_ns: self.last_progress_ns,
        })
    }
}

fn publish_watchdog_fail_stop_marker(
    control: &ServerQualificationControl,
    state: &PerUserEmbeddingServerState,
    budgets: EmbeddingServerBudgets,
    progress_sequence: u64,
    last_progress_ns: u64,
) -> Result<()> {
    control.directory.revalidate()?;
    let filename = embedding_qualification_watchdog_marker_filename(
        &control.nonce_sha256,
        &state.process.server_instance_id,
    )?;
    let destination = control.directory.join(&filename);
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => bail!("embedding_qualification_watchdog_marker_exists"),
        Err(error) => return Err(error).context("inspect watchdog marker destination"),
    }
    let clock = state.clock.snapshot();
    let marker = EmbeddingQualificationWatchdogMarker {
        schema_version: 1,
        nonce_sha256: control.nonce_sha256.clone(),
        server_instance_id: state.process.server_instance_id.clone(),
        pid: state.process.pid,
        process_start_id: state.process.process_start_id.clone(),
        executable_sha256: state.process.executable_sha256.clone(),
        executable_version: state.process.executable_version.clone(),
        reason: "embedding_engine_stalled".into(),
        clock: EmbeddingQualificationWatchdogClock {
            domain: clock.domain,
            api: clock.api,
            boot_id: clock.boot_id,
            observed_ns: state.clock.now_ns(),
        },
        progress_sequence,
        last_progress_ns,
        hard_native_no_progress_ms: duration_ms(budgets.native_no_progress),
        watchdog_cadence_ms: duration_ms(budgets.watchdog_poll),
    };
    let mut encoded = serde_json::to_vec(&marker).context("encode watchdog fail-stop marker")?;
    encoded.push(b'\n');
    let temporary = control.directory.join(format!(
        ".{filename}.{}.{}.tmp",
        std::process::id(),
        Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&temporary)
        .context("create watchdog fail-stop marker temp file")?;
    let publish = (|| -> Result<()> {
        file.write_all(&encoded)
            .context("write watchdog fail-stop marker")?;
        file.flush().context("flush watchdog fail-stop marker")?;
        file.sync_all().context("sync watchdog fail-stop marker")?;
        drop(file);
        control.directory.revalidate()?;
        fs::rename(&temporary, &destination).context("publish watchdog fail-stop marker")?;
        sync_qualification_directory(&control.directory.path)?;
        let metadata = fs::symlink_metadata(&destination)
            .context("inspect published watchdog fail-stop marker")?;
        validate_private_qualification_file_metadata(&metadata, 64 * 1024)?;
        if metadata.len() != encoded.len() as u64 {
            bail!("embedding_qualification_watchdog_marker_truncated");
        }
        Ok(())
    })();
    if publish.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    publish
}

#[cfg(unix)]
fn sync_qualification_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .context("sync embedding qualification directory")
}

#[cfg(not(unix))]
fn sync_qualification_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn serve_embedding_connection(
    state: Arc<PerUserEmbeddingServerState>,
    mut stream: Box<dyn EmbeddingServerStream>,
) -> Result<()> {
    let result = serve_embedding_connection_inner(state, &mut *stream, false);
    finish_embedding_response_delivery(&*stream, result)
}

fn serve_embedding_connection_at_handler_capacity(
    state: Arc<PerUserEmbeddingServerState>,
    mut stream: Box<dyn EmbeddingServerStream>,
) -> Result<()> {
    let result = serve_embedding_connection_inner(state, &mut *stream, true);
    finish_embedding_response_delivery(&*stream, result)
}

fn finish_embedding_response_delivery(
    stream: &dyn EmbeddingServerStream,
    result: Result<()>,
) -> Result<()> {
    result?;
    // Do not inherit the wire deadline here: an authenticated same-user client
    // may choose any positive value and could otherwise retain every bounded
    // handler after receiving its response. The frozen query request deadline
    // is the smallest server-owned product budget and physically covers a
    // response larger than the Windows pipe buffer.
    stream
        .set_read_timeout(Some(EmbeddingClientBudgets::current().query_request))
        .context("bound embedding final response delivery")?;
    stream
        .finish_response_delivery()
        .context("finish embedding final response delivery")
}

fn serve_embedding_connection_inner(
    state: Arc<PerUserEmbeddingServerState>,
    stream: &mut dyn EmbeddingServerStream,
    handler_capacity_limited: bool,
) -> Result<()> {
    let pre_request_guard = (!handler_capacity_limited)
        .then(|| state.try_begin_pre_request())
        .flatten();
    if !stream.transport_identity().peer_verified {
        bail!("embedding_server_peer_unverified");
    }
    let transport_peer_pid = stream
        .transport_identity()
        .peer_pid
        .filter(|pid| *pid != 0)
        .ok_or_else(|| anyhow!("embedding_server_peer_process_identity_missing"))?;
    let transport_peer_process_start_id = stream
        .transport_identity()
        .peer_process_start_id
        .as_deref()
        .filter(|start_id| !start_id.is_empty())
        .ok_or_else(|| anyhow!("embedding_server_peer_process_identity_missing"))?
        .to_owned();
    stream
        .set_read_timeout(Some(EmbeddingClientBudgets::current().connect))
        .context("bound embedding server handshake read")?;
    stream
        .set_write_timeout(Some(EmbeddingClientBudgets::current().connect))
        .context("bound embedding server handshake write")?;
    let connection_id = Uuid::new_v4().to_string();
    let (hello_request, hello_payload): (EmbeddingProtocolRequest, Vec<u8>) = read_frame(stream)?;
    if !hello_payload.is_empty() {
        bail!("embedding_server_protocol_hello_required");
    }
    validate_protocol_request(&hello_request)?;
    let EmbeddingOperation::Hello {
        intent,
        client_pid,
        client_process_start_id,
        client_executable_sha256,
        client_executable_version,
    } = &hello_request.operation
    else {
        bail!("embedding_server_protocol_hello_required");
    };
    if *client_pid != transport_peer_pid
        || client_process_start_id != &transport_peer_process_start_id
    {
        bail!("embedding_server_peer_identity_mismatch");
    }
    if !is_sha256(client_executable_sha256) || client_executable_version.trim().is_empty() {
        bail!("embedding_server_peer_executable_identity_invalid");
    }
    let peer_executable_mismatch = client_executable_sha256 != &state.process.executable_sha256
        || client_executable_version != &state.process.executable_version;
    if !matches!(intent.as_str(), "activate" | "observe") {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &hello_request.request_id,
                protocol_error(
                    "embedding_server_intent_invalid",
                    "hello intent must be activate or observe",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if handler_capacity_limited || pre_request_guard.is_none() {
        let (reason, capacity, depth) = if handler_capacity_limited {
            (
                "connection_handler_full",
                SERVER_CONNECTION_HANDLER_CAPACITY,
                state.connections.load(Ordering::Acquire),
            )
        } else {
            (
                "pre_request_full",
                SERVER_CONTROL_CONNECTION_RESERVE,
                SERVER_CONTROL_CONNECTION_RESERVE,
            )
        };
        write_protocol_response(
            &mut *stream,
            failure_response(
                &hello_request.request_id,
                state.connection_capacity_error(reason, capacity, depth),
            ),
            &[],
        )?;
        return Ok(());
    }
    let observe_only = intent == "observe";
    let expected = EmbeddingCompatibility::current(
        state.engine_config.backend.device_class == NativeDeviceClass::Cpu,
    );
    let compatible = hello_request.compatibility == expected
        && !peer_executable_mismatch
        && !state
            .qualification
            .as_ref()
            .is_some_and(|control| control.force_incompatible.load(Ordering::Acquire));
    if !compatible {
        // Observe is a read-only contract. An incompatible status/doctor
        // process may report the live owner, but it must never transfer
        // authority or make that owner drain.
        let idle = !observe_only && state.begin_draining_if_idle();
        let error = EmbeddingProtocolError {
            code: if idle {
                "embedding_server_draining"
            } else {
                "embedding_server_incompatible_active_owner"
            }
            .into(),
            message: "the live per-user embedding server has an incompatible engine contract"
                .into(),
            retry_class: "after_owner_idle".into(),
            retry_after_ms: 0,
            retry_condition: "the incompatible server exits while fully idle".into(),
            capacity: None,
        };
        write_protocol_response(
            &mut *stream,
            failure_response(&hello_request.request_id, error),
            &[],
        )?;
        return Ok(());
    }
    write_protocol_response(
        &mut *stream,
        success_response(
            &hello_request.request_id,
            EmbeddingResult::Hello {
                compatibility_sha256: expected.digest()?,
                snapshot: Box::new(state.snapshot()),
            },
        ),
        &[],
    )?;

    let (request, payload): (EmbeddingProtocolRequest, Vec<u8>) = read_frame(&mut *stream)?;
    if let Err(error) = validate_protocol_request(&request) {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_protocol_mismatch",
                    &format!("embedding request protocol was rejected: {error}"),
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if !payload.is_empty() {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_request_payload_forbidden",
                    "request payload bytes are not accepted",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if request.compatibility != expected {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_compatibility_changed",
                    "request compatibility changed after hello",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if observe_only && !matches!(request.operation, EmbeddingOperation::Snapshot) {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_observe_operation_forbidden",
                    "observe connections may only request a snapshot",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    drop(pre_request_guard);
    match request.operation.clone() {
        EmbeddingOperation::Snapshot => {
            let identity = state
                .try_initialized_engine()
                .and_then(|engine| engine.snapshot().ok())
                .and_then(|snapshot| {
                    engine_identity(&state.process.server_instance_id, &snapshot).ok()
                });
            write_protocol_response(
                &mut *stream,
                success_response(
                    &request.request_id,
                    EmbeddingResult::Snapshot {
                        snapshot: Box::new(state.snapshot()),
                        lease: None,
                        identity: identity.map(Box::new),
                    },
                ),
                &[],
            )?;
        }
        EmbeddingOperation::EnsureResident {
            scope_id,
            deadline_ms,
            retry_after_ms,
        } => {
            if deadline_ms == 0 {
                return write_deadline_invalid(&mut *stream, &request.request_id);
            }
            configure_server_operation_timeout(&*stream, deadline_ms)?;
            let started_ns = state.clock.now_ns();
            let context =
                EmbeddingRequestContext::new(&request.request_id, &scope_id, retry_after_ms);
            let admission =
                match state.try_admit_request(EmbeddingRequestClass::Bulk, retry_after_ms) {
                    Ok(admission) => admission,
                    Err(error) => {
                        return write_protocol_response(
                            &mut *stream,
                            failure_response(&request.request_id, *error),
                            &[],
                        );
                    }
                };
            let guard = state.begin_request(
                &connection_id,
                &request.request_id,
                &scope_id,
                EmbeddingRequestClass::Bulk,
                "ensure_resident",
                context,
                admission,
                None,
            )?;
            guard.update_phase("native_execution");
            let result = state
                .engine()
                .and_then(|engine| engine.ensure_resident().map_err(engine_error))
                .and_then(|snapshot| engine_identity(&state.process.server_instance_id, &snapshot));
            if elapsed_since(state.clock.as_ref(), started_ns) >= Duration::from_millis(deadline_ms)
            {
                return write_deadline_exceeded(
                    &mut *stream,
                    &request.request_id,
                    retry_after_ms,
                    EmbeddingRequestClass::Bulk,
                    state.initialized_engine().as_ref(),
                );
            }
            guard.update_phase("response");
            match result {
                Ok(identity) => write_protocol_response(
                    &mut *stream,
                    success_response(
                        &request.request_id,
                        EmbeddingResult::Identity {
                            identity: Box::new(identity),
                        },
                    ),
                    &[],
                )?,
                Err(error) => write_anyhow_failure(&mut *stream, &request.request_id, error)?,
            }
        }
        EmbeddingOperation::AcquireLease {
            scope_id,
            deadline_ms,
            retry_after_ms,
        } => {
            if deadline_ms == 0 {
                return write_deadline_invalid(&mut *stream, &request.request_id);
            }
            configure_server_operation_timeout(&*stream, deadline_ms)?;
            serve_lease_connection(
                &state,
                &connection_id,
                &mut *stream,
                request,
                scope_id,
                deadline_ms,
                retry_after_ms,
                expected,
            )?;
        }
        EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms,
            retry_after_ms,
            cancel_token,
            input,
        } => {
            if let Err(error) = validate_raw_inputs(std::slice::from_ref(&input)) {
                return write_protocol_response(
                    &mut *stream,
                    failure_response(
                        &request.request_id,
                        protocol_error(
                            "embedding_server_input_invalid",
                            &format!("embedding query input was rejected: {error}"),
                        ),
                    ),
                    &[],
                );
            }
            serve_embedding_request(
                &state,
                &connection_id,
                &mut *stream,
                &request.request_id,
                scope_id,
                EmbeddingRequestClass::Query,
                deadline_ms,
                retry_after_ms,
                cancel_token,
                transport_peer_pid,
                &transport_peer_process_start_id,
                vec![format!("{CODERANK_QUERY_PREFIX}{input}")],
            )?;
        }
        EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms,
            retry_after_ms,
            cancel_token,
            inputs,
        } => {
            if let Err(error) = validate_raw_inputs(&inputs) {
                return write_protocol_response(
                    &mut *stream,
                    failure_response(
                        &request.request_id,
                        protocol_error(
                            "embedding_server_input_invalid",
                            &format!("embedding document inputs were rejected: {error}"),
                        ),
                    ),
                    &[],
                );
            }
            let inputs = inputs
                .into_iter()
                .map(|input| format!("{CODERANK_DOCUMENT_PREFIX}{input}"))
                .collect();
            serve_embedding_request(
                &state,
                &connection_id,
                &mut *stream,
                &request.request_id,
                scope_id,
                EmbeddingRequestClass::Bulk,
                deadline_ms,
                retry_after_ms,
                cancel_token,
                transport_peer_pid,
                &transport_peer_process_start_id,
                inputs,
            )?;
        }
        EmbeddingOperation::Cancel {
            target_request_id,
            cancel_token,
        } => {
            if !valid_cancel_token(&cancel_token) {
                return write_protocol_response(
                    &mut *stream,
                    failure_response(
                        &request.request_id,
                        protocol_error(
                            "embedding_server_cancel_token_invalid",
                            "embedding cancellation requires an unguessable token",
                        ),
                    ),
                    &[],
                );
            }
            let cancelled = state.cancel(
                &target_request_id,
                &cancel_token,
                transport_peer_pid,
                &transport_peer_process_start_id,
            );
            write_protocol_response(
                &mut *stream,
                success_response(
                    &request.request_id,
                    if cancelled {
                        EmbeddingResult::Cancelled
                    } else {
                        EmbeddingResult::Released
                    },
                ),
                &[],
            )?;
        }
        EmbeddingOperation::Hello { .. } | EmbeddingOperation::ReleaseLease { .. } => {
            write_protocol_response(
                &mut *stream,
                failure_response(
                    &request.request_id,
                    protocol_error(
                        "embedding_server_operation_invalid",
                        "operation is invalid outside its connection state",
                    ),
                ),
                &[],
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn serve_embedding_request(
    state: &Arc<PerUserEmbeddingServerState>,
    connection_id: &str,
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
    scope_id: String,
    request_class: EmbeddingRequestClass,
    deadline_ms: u64,
    retry_after_ms: u64,
    cancel_token: Option<String>,
    client_pid: u32,
    client_process_start_id: &str,
    inputs: Vec<String>,
) -> Result<()> {
    if deadline_ms == 0 {
        return write_protocol_response(
            stream,
            failure_response(
                request_id,
                protocol_error(
                    "embedding_server_deadline_invalid",
                    "embedding request deadline must be finite and positive",
                ),
            ),
            &[],
        );
    }
    let deadline = ServerRequestDeadline::start(state.clock.as_ref(), deadline_ms);
    let context = EmbeddingRequestContext::new(request_id, &scope_id, retry_after_ms);
    let cancellation_auth = match cancel_token {
        Some(token) if valid_cancel_token(&token) => Some(ServerCancellationAuth {
            token,
            client_pid,
            client_process_start_id: client_process_start_id.into(),
        }),
        Some(_) => {
            return write_protocol_response(
                stream,
                failure_response(
                    request_id,
                    protocol_error(
                        "embedding_server_cancel_token_invalid",
                        "embedding cancellation requires an unguessable token",
                    ),
                ),
                &[],
            );
        }
        None => None,
    };
    configure_server_operation_timeout(stream, deadline_ms)?;
    let admission = state.try_admit_request(request_class, retry_after_ms);
    if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
        return write_deadline_exceeded(stream, request_id, retry_after_ms, request_class, None);
    }
    let admission = match admission {
        Ok(admission) => admission,
        Err(error) => {
            return write_protocol_response(stream, failure_response(request_id, *error), &[]);
        }
    };
    let guard = state.begin_request(
        connection_id,
        request_id,
        &scope_id,
        request_class,
        "queued",
        context.clone(),
        admission,
        cancellation_auth,
    );
    if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
        return write_deadline_exceeded(stream, request_id, retry_after_ms, request_class, None);
    }
    let guard = guard?;
    let engine = state.engine();
    if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
        return write_deadline_exceeded(
            stream,
            request_id,
            retry_after_ms,
            request_class,
            engine.as_ref().ok(),
        );
    }
    let engine = match engine {
        Ok(engine) => engine,
        Err(error) => return write_anyhow_failure(stream, request_id, error),
    };
    if context.is_cancelled() || cancel_if_peer_dead(stream, &context)? {
        return Ok(());
    }
    let handle = match request_class {
        EmbeddingRequestClass::Query => {
            engine.submit_query_prepared(context.clone(), inputs[0].clone())
        }
        EmbeddingRequestClass::Bulk => engine.submit_documents_prepared(context.clone(), inputs),
    };
    let handle = match handle {
        Ok(handle) => handle,
        Err(error) => return write_engine_failure(stream, request_id, error),
    };
    loop {
        guard.update_phase(context.phase());
        if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
            let _ = handle.cancel();
            return write_deadline_exceeded(
                stream,
                request_id,
                retry_after_ms,
                request_class,
                Some(&engine),
            );
        }
        if cancel_if_peer_dead(stream, &context)? {
            let _ = handle.cancel();
            return Ok(());
        }
        match handle.try_recv_with_completion() {
            Ok(Some(Ok(completion))) => {
                let native_completion_sequence = completion.completion_sequence;
                let vectors = normalize_and_validate_vectors(completion.vectors)?;
                let payload = encode_vectors(&vectors)?;
                let snapshot = engine.snapshot().map_err(engine_error)?;
                let identity = engine_identity(&state.process.server_instance_id, &snapshot)?;
                if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
                    let _ = handle.cancel();
                    return write_deadline_exceeded(
                        stream,
                        request_id,
                        retry_after_ms,
                        request_class,
                        Some(&engine),
                    );
                }
                record_qualification_completed_tokens(
                    state,
                    request_id,
                    context.completed_tokens(),
                    native_completion_sequence,
                )?;
                guard.update_phase("response");
                return write_protocol_response(
                    stream,
                    success_response(
                        request_id,
                        EmbeddingResult::Vectors {
                            rows: vectors.len() as u32,
                            columns: RETRIEVAL_EMBEDDING_DIM as u32,
                            encoding: "f32_le".into(),
                            identity: Box::new(identity),
                        },
                    ),
                    &payload,
                );
            }
            Ok(Some(Err(error))) => return write_engine_failure(stream, request_id, error),
            Ok(None) => {}
            Err(error) => return write_engine_failure(stream, request_id, error),
        }
        state.clock.sleep(CONNECTION_POLL);
    }
}

#[derive(Debug, Clone, Copy)]
struct ServerRequestDeadline {
    started_ns: u64,
    timeout: Duration,
}

impl ServerRequestDeadline {
    fn start(clock: &dyn AwakeMonotonicClock, deadline_ms: u64) -> Self {
        Self {
            started_ns: clock.now_ns(),
            timeout: Duration::from_millis(deadline_ms),
        }
    }

    fn cancel_if_elapsed(
        self,
        clock: &dyn AwakeMonotonicClock,
        context: &EmbeddingRequestContext,
    ) -> bool {
        if elapsed_since(clock, self.started_ns) < self.timeout {
            return false;
        }
        context.cancel();
        true
    }
}

fn record_qualification_completed_tokens(
    state: &PerUserEmbeddingServerState,
    request_id: &str,
    completed_tokens: u64,
    native_completion_sequence: u64,
) -> Result<()> {
    let Some(control) = state.qualification.as_ref() else {
        return Ok(());
    };
    if completed_tokens == 0 {
        bail!("embedding_qualification_completed_token_count_missing");
    }
    if native_completion_sequence == 0 {
        bail!("embedding_qualification_native_completion_sequence_missing");
    }
    let clock = state.clock.snapshot();
    write_server_qualification_event(
        control,
        state,
        ServerQualificationEvent {
            schema_version: 1,
            sequence: 0,
            action: "completed_tokens".into(),
            status: "completed".into(),
            server_event_sequence: state.event_sequence.load(Ordering::Acquire),
            clock: ServerQualificationEventClock {
                domain: clock.domain,
                api: clock.api,
                boot_id: clock.boot_id,
                observed_ns: state.clock.now_ns(),
            },
            snapshot: None,
            details: Some(
                [
                    ("request_id".into(), request_id.into()),
                    ("completed_tokens".into(), completed_tokens.to_string()),
                    (
                        "native_completion_sequence".into(),
                        native_completion_sequence.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        },
    )
}

fn cancel_if_peer_dead(
    stream: &dyn EmbeddingServerStream,
    context: &EmbeddingRequestContext,
) -> Result<bool> {
    if stream
        .peer_is_alive()
        .context("probe embedding client liveness")?
    {
        return Ok(false);
    }
    context.cancel();
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn serve_lease_connection(
    state: &Arc<PerUserEmbeddingServerState>,
    connection_id: &str,
    stream: &mut dyn EmbeddingServerStream,
    request: EmbeddingProtocolRequest,
    scope_id: String,
    deadline_ms: u64,
    retry_after_ms: u64,
    compatibility: EmbeddingCompatibility,
) -> Result<()> {
    let started_ns = state.clock.now_ns();
    let context = EmbeddingRequestContext::new(&request.request_id, &scope_id, retry_after_ms);
    let admission = match state.try_admit_request(EmbeddingRequestClass::Bulk, retry_after_ms) {
        Ok(admission) => admission,
        Err(error) => {
            return write_protocol_response(
                stream,
                failure_response(&request.request_id, *error),
                &[],
            );
        }
    };
    let guard = state.begin_request(
        connection_id,
        &request.request_id,
        &scope_id,
        EmbeddingRequestClass::Bulk,
        "acquire_lease",
        context,
        admission,
        None,
    )?;
    let engine = match state.engine() {
        Ok(engine) => engine,
        Err(error) => return write_anyhow_failure(stream, &request.request_id, error),
    };
    let native_lease = match engine.acquire_residency_lease() {
        Ok(lease) => ServerLeaseActivity::new(state, lease),
        Err(error) => return write_engine_failure(stream, &request.request_id, error),
    };
    if elapsed_since(state.clock.as_ref(), started_ns) >= Duration::from_millis(deadline_ms) {
        drop(native_lease);
        return write_deadline_exceeded(
            stream,
            &request.request_id,
            retry_after_ms,
            EmbeddingRequestClass::Bulk,
            Some(&engine),
        );
    }
    if !stream
        .peer_is_alive()
        .context("probe lease client liveness")?
    {
        drop(native_lease);
        return Ok(());
    }
    guard.update_phase("response");
    let identity = engine_identity(
        &state.process.server_instance_id,
        native_lease.lease().snapshot(),
    )?;
    let lease = EmbeddingEngineLeaseIdentity {
        lease_token: Uuid::new_v4().to_string(),
        server_instance_id: state.process.server_instance_id.clone(),
        load_generation: identity.load_generation,
        compatibility_sha256: compatibility.digest()?,
    };
    write_protocol_response(
        stream,
        success_response(
            &request.request_id,
            EmbeddingResult::Lease {
                lease: lease.clone(),
                identity: Box::new(identity.clone()),
            },
        ),
        &[],
    )?;
    drop(guard);
    stream
        .set_read_timeout(Some(CONNECTION_POLL))
        .context("bound held embedding lease liveness poll")?;
    stream
        .set_write_timeout(Some(EmbeddingClientBudgets::current().connect))
        .context("bound held embedding lease response")?;
    let mut frame_reader = IncrementalProtocolFrameReader::default();

    loop {
        let (next, payload): (EmbeddingProtocolRequest, Vec<u8>) =
            match frame_reader.poll(stream)? {
                ProtocolFramePoll::Pending => {
                    if !stream
                        .peer_is_alive()
                        .context("probe held lease client liveness")?
                    {
                        return Ok(());
                    }
                    continue;
                }
                ProtocolFramePoll::Closed => return Ok(()),
                ProtocolFramePoll::Ready(next) => next,
            };
        if !payload.is_empty() {
            return Ok(());
        }
        validate_protocol_request(&next)?;
        if next.compatibility != compatibility {
            return Ok(());
        }
        match next.operation {
            EmbeddingOperation::Snapshot => {
                let current = engine.snapshot().map_err(engine_error)?;
                let current_identity =
                    engine_identity(&state.process.server_instance_id, &current)?;
                if current_identity.server_instance_id != lease.server_instance_id
                    || current_identity.load_generation != lease.load_generation
                {
                    write_protocol_response(
                        stream,
                        failure_response(
                            &next.request_id,
                            protocol_error(
                                "embedding_publication_lease_changed",
                                "embedding lease load identity changed before publication",
                            ),
                        ),
                        &[],
                    )?;
                    return Ok(());
                }
                write_protocol_response(
                    stream,
                    success_response(
                        &next.request_id,
                        EmbeddingResult::Snapshot {
                            snapshot: Box::new(state.snapshot()),
                            lease: Some(lease.clone()),
                            identity: Some(Box::new(current_identity)),
                        },
                    ),
                    &[],
                )?;
            }
            EmbeddingOperation::ReleaseLease { lease_token }
                if lease_token == lease.lease_token =>
            {
                drop(native_lease);
                write_protocol_response(
                    stream,
                    success_response(&next.request_id, EmbeddingResult::Released),
                    &[],
                )?;
                return Ok(());
            }
            _ => {
                write_protocol_response(
                    stream,
                    failure_response(
                        &next.request_id,
                        protocol_error(
                            "embedding_publication_lease_operation_invalid",
                            "only snapshot or release is valid on a lease connection",
                        ),
                    ),
                    &[],
                )?;
                return Ok(());
            }
        }
    }
}

#[derive(Default)]
struct IncrementalProtocolFrameReader {
    bytes: Vec<u8>,
}

enum ProtocolFramePoll<T> {
    Pending,
    Closed,
    Ready((T, Vec<u8>)),
}

impl IncrementalProtocolFrameReader {
    fn poll<T: for<'de> Deserialize<'de>>(
        &mut self,
        stream: &mut dyn EmbeddingServerStream,
    ) -> Result<ProtocolFramePoll<T>> {
        if let Some(frame) = self.decode_ready()? {
            return Ok(ProtocolFramePoll::Ready(frame));
        }
        let mut chunk = [0_u8; 8 * 1024];
        match stream.read(&mut chunk) {
            Ok(0) if self.bytes.is_empty() => return Ok(ProtocolFramePoll::Closed),
            Ok(0) => bail!("embedding_server_frame_truncated"),
            Ok(read) => self.bytes.extend_from_slice(&chunk[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(ProtocolFramePoll::Pending);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::NotConnected
                        | io::ErrorKind::UnexpectedEof
                ) =>
            {
                return Ok(ProtocolFramePoll::Closed);
            }
            Err(error) => return Err(error).context("read held embedding lease frame"),
        }
        Ok(match self.decode_ready()? {
            Some(frame) => ProtocolFramePoll::Ready(frame),
            None => ProtocolFramePoll::Pending,
        })
    }

    fn decode_ready<T: for<'de> Deserialize<'de>>(&mut self) -> Result<Option<(T, Vec<u8>)>> {
        if self.bytes.len() < 8 {
            return Ok(None);
        }
        let control_len =
            u32::from_be_bytes(self.bytes[0..4].try_into().expect("four-byte frame length"))
                as usize;
        let payload_len =
            u32::from_be_bytes(self.bytes[4..8].try_into().expect("four-byte frame length"))
                as usize;
        if control_len == 0
            || control_len > PER_USER_EMBEDDING_MAX_METADATA_BYTES
            || payload_len > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
        {
            bail!("embedding_server_frame_too_large");
        }
        let frame_len = 8_usize
            .checked_add(control_len)
            .and_then(|length| length.checked_add(payload_len))
            .ok_or_else(|| anyhow!("embedding_server_frame_length_overflow"))?;
        if self.bytes.len() < frame_len {
            return Ok(None);
        }
        let control = serde_json::from_slice(&self.bytes[8..8 + control_len])
            .context("decode held embedding lease control frame")?;
        let payload = self.bytes[8 + control_len..frame_len].to_vec();
        self.bytes.drain(..frame_len);
        Ok(Some((control, payload)))
    }
}

fn scheduler_snapshot(
    admission: &EmbeddingAdmissionSnapshot,
    connections: usize,
    active: Option<&ActiveServerRequest>,
    clock: &dyn AwakeMonotonicClock,
) -> EmbeddingServerSchedulerSnapshot {
    EmbeddingServerSchedulerSnapshot {
        query_capacity: admission.query_capacity as u64,
        query_depth: admission.query_depth as u64,
        bulk_capacity: admission.bulk_capacity as u64,
        bulk_depth: admission.bulk_depth as u64,
        connection_count: connections as u64,
        active_request_count: admission.active_request_count as u64,
        lease_count: admission.lease_count as u64,
        active_request: active.map(|active| active_request_snapshot(active, clock)),
    }
}

fn active_request_snapshot(
    active: &ActiveServerRequest,
    clock: &dyn AwakeMonotonicClock,
) -> EmbeddingServerActiveRequestSnapshot {
    EmbeddingServerActiveRequestSnapshot {
        request_id: active.request_id.clone(),
        scope_id: active.scope_id.clone(),
        class: active.request_class.as_str().into(),
        phase: active.phase.clone(),
        elapsed_ms: duration_ms(elapsed_since(clock, active.started_ns)),
    }
}

fn engine_identity(
    server_instance_id: &str,
    snapshot: &EngineLifecycleSnapshot,
) -> Result<EmbeddingEngineIdentity> {
    let identity = &snapshot.identity;
    let policy = match identity.selected_device_class {
        NativeDeviceClass::Cpu => "cpu_explicit",
        NativeDeviceClass::Accelerator => "accelerated",
        NativeDeviceClass::Unknown => bail!("embedding_backend_device_class_unknown"),
    };
    Ok(EmbeddingEngineIdentity {
        server_instance_id: server_instance_id.into(),
        load_generation: snapshot.load_generation,
        model_load_count: snapshot.model_load_count,
        residency: snapshot.residency.as_str().into(),
        worker_alive: snapshot.worker_alive,
        load_error: snapshot.load_error.clone(),
        model_digest: identity.model_digest.into(),
        ggml_build_identity: identity.ggml_build_identity.into(),
        backend: identity.backend.clone(),
        adapter_name: identity.adapter_name.clone(),
        adapter_description: identity.adapter_description.clone(),
        policy: policy.into(),
        embedded_model: identity.embedded_model,
        materialized_model_sha256: identity.model_digest.into(),
        materialized_reused: identity.materialized_reused,
        initialization_ms: duration_ms(identity.initialization_duration),
        smoke_ms: duration_ms(identity.smoke_duration),
        adapter_memory_total: identity.adapter_memory_total as u64,
        adapter_memory_used_by_load: identity
            .adapter_memory_free_before_load
            .saturating_sub(identity.adapter_memory_free_after_load)
            as u64,
        execution_device_names: identity.execution_device_names.clone(),
        execution_backend_names: identity.execution_backend_names.clone(),
        execution_observation_source: identity.execution_observation_source.into(),
        encode_count: identity.encode_count,
        execution_node_count: identity.execution_node_count,
        resident_accelerator_tensor_count: identity.resident_accelerator_tensor_count,
        resident_accelerator_tensor_bytes: identity.resident_accelerator_tensor_bytes,
        model_layer_count: identity.model_layer_count,
        offloaded_layer_count: identity.offloaded_layer_count,
        accelerator_execution_verified: identity.accelerator_execution_verified,
    })
}

fn validate_protocol_request(request: &EmbeddingProtocolRequest) -> Result<()> {
    if request.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || request.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || request.request_id.trim().is_empty()
        || request.request_id.len() > 256
    {
        bail!("embedding_server_protocol_mismatch");
    }
    Ok(())
}

fn validate_raw_inputs(inputs: &[String]) -> Result<()> {
    if inputs.is_empty()
        || inputs.len() > PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT
        || inputs.iter().any(|input| input.trim().is_empty())
    {
        bail!("embedding_server_input_shape_invalid");
    }
    let bytes = inputs
        .iter()
        .try_fold(0_usize, |total, input| total.checked_add(input.len()))
        .ok_or_else(|| anyhow!("embedding_server_input_length_overflow"))?;
    if bytes > PER_USER_EMBEDDING_MAX_INPUT_BYTES {
        bail!("embedding_server_input_too_large");
    }
    Ok(())
}

fn capacity_wire(
    snapshot: &EmbeddingAdmissionSnapshot,
    reason: EmbeddingCapacityReason,
    request_class: EmbeddingRequestClass,
    retry_after_ms: u64,
    retry_condition: &str,
) -> EmbeddingCapacityPressureWire {
    let (capacity, depth) = match request_class {
        EmbeddingRequestClass::Query => (snapshot.query_capacity, snapshot.query_depth),
        EmbeddingRequestClass::Bulk => (snapshot.bulk_capacity, snapshot.bulk_depth),
    };
    EmbeddingCapacityPressureWire {
        reason: reason.as_str().into(),
        queue_class: request_class.as_str().into(),
        capacity: capacity as u64,
        depth: depth as u64,
        retry_after_ms,
        retry_condition: retry_condition.into(),
        owner_state: snapshot.owner_state.as_str().into(),
        active_scope_id: snapshot
            .active_request
            .as_ref()
            .map(|active| active.scope_id.clone()),
        active_request_id: snapshot
            .active_request
            .as_ref()
            .map(|active| active.request_id.clone()),
        active_request_class: snapshot
            .active_request
            .as_ref()
            .map(|active| active.request_class.as_str().into()),
    }
}

fn success_response(request_id: &str, result: EmbeddingResult) -> EmbeddingProtocolResponse {
    EmbeddingProtocolResponse {
        protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
        schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
        request_id: request_id.into(),
        result: Some(result),
        error: None,
    }
}

fn failure_response(request_id: &str, error: EmbeddingProtocolError) -> EmbeddingProtocolResponse {
    EmbeddingProtocolResponse {
        protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
        schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
        request_id: request_id.into(),
        result: None,
        error: Some(error),
    }
}

fn protocol_error(code: &str, message: &str) -> EmbeddingProtocolError {
    EmbeddingProtocolError {
        code: code.into(),
        message: message.into(),
        retry_class: "terminal".into(),
        retry_after_ms: 0,
        retry_condition: "the request or compatible executable changes".into(),
        capacity: None,
    }
}

fn configure_server_operation_timeout(
    stream: &dyn EmbeddingServerStream,
    deadline_ms: u64,
) -> Result<()> {
    let wire_timeout = Duration::from_millis(deadline_ms);
    if wire_timeout.is_zero() {
        bail!("embedding_server_deadline_invalid");
    }
    // The wire deadline can shorten an exchange, but it cannot lengthen a
    // response write. In particular, Windows PIPE_NOWAIT writes must retry
    // zero progress while the kernel buffer is full. A peer-selected timeout
    // there would let a non-reading same-user client retain every bounded
    // connection handler. The smallest frozen request budget is already
    // qualified for responses larger than the Windows pipe buffer.
    let timeout = wire_timeout.min(EmbeddingClientBudgets::current().query_request);
    stream
        .set_read_timeout(Some(timeout))
        .context("bound embedding server request read")?;
    stream
        .set_write_timeout(Some(timeout))
        .context("bound embedding server response write")
}

fn write_deadline_invalid(stream: &mut dyn EmbeddingServerStream, request_id: &str) -> Result<()> {
    write_protocol_response(
        stream,
        failure_response(
            request_id,
            protocol_error(
                "embedding_server_deadline_invalid",
                "embedding request deadline must be finite and positive",
            ),
        ),
        &[],
    )
}

fn write_deadline_exceeded(
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
    retry_after_ms: u64,
    request_class: EmbeddingRequestClass,
    engine: Option<&EmbeddingEngine>,
) -> Result<()> {
    let capacity = engine.map(|engine| {
        capacity_wire(
            &engine.admission_snapshot(),
            EmbeddingCapacityReason::DeadlineElapsed,
            request_class,
            retry_after_ms,
            "the active request completes or the server instance changes",
        )
    });
    write_protocol_response(
        stream,
        failure_response(
            request_id,
            EmbeddingProtocolError {
                code: "embedding_deadline_exceeded".into(),
                message: "embedding request exceeded its server-owned soft deadline".into(),
                retry_class: "after_delay".into(),
                retry_after_ms,
                retry_condition: "the active request completes or the server instance changes"
                    .into(),
                capacity,
            },
        ),
        &[],
    )
}

fn write_protocol_response(
    stream: &mut dyn EmbeddingServerStream,
    response: EmbeddingProtocolResponse,
    payload: &[u8],
) -> Result<()> {
    write_frame(stream, &response, payload)
}

fn write_engine_failure(
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
    error: EngineError,
) -> Result<()> {
    let protocol_error = match error.capacity_pressure() {
        Some(pressure) => EmbeddingProtocolError {
            code: "embedding_capacity".into(),
            message: error.to_string(),
            retry_class: "after_capacity_change".into(),
            retry_after_ms: pressure.retry_after_ms,
            retry_condition: pressure.retry_condition.clone(),
            capacity: Some(EmbeddingCapacityPressureWire::from(pressure)),
        },
        None => EmbeddingProtocolError {
            code: error.reason_code().into(),
            message: error.to_string(),
            retry_class: if matches!(error, EngineError::Cancelled) {
                "none"
            } else {
                "after_server_change"
            }
            .into(),
            retry_after_ms: 0,
            retry_condition: "the server instance or engine evidence changes".into(),
            capacity: None,
        },
    };
    write_protocol_response(stream, failure_response(request_id, protocol_error), &[])
}

fn write_anyhow_failure(
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
    error: anyhow::Error,
) -> Result<()> {
    if let Some(engine) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<EngineError>())
    {
        let code = engine.reason_code();
        return write_protocol_response(
            stream,
            failure_response(
                request_id,
                EmbeddingProtocolError {
                    code: code.into(),
                    message: error.to_string(),
                    retry_class: "after_server_change".into(),
                    retry_after_ms: 0,
                    retry_condition: "the server instance or engine evidence changes".into(),
                    capacity: engine
                        .capacity_pressure()
                        .map(EmbeddingCapacityPressureWire::from),
                },
            ),
            &[],
        );
    }
    write_protocol_response(
        stream,
        failure_response(
            request_id,
            EmbeddingProtocolError {
                code: "embedding_server_internal_error".into(),
                message: error.to_string(),
                retry_class: "terminal".into(),
                retry_after_ms: 0,
                retry_condition: "the request or server implementation changes".into(),
                capacity: None,
            },
        ),
        &[],
    )
}

impl From<&EmbeddingCapacityPressure> for EmbeddingCapacityPressureWire {
    fn from(pressure: &EmbeddingCapacityPressure) -> Self {
        Self {
            reason: pressure.reason.as_str().into(),
            queue_class: pressure.request_class.as_str().into(),
            capacity: pressure.capacity as u64,
            depth: pressure.depth as u64,
            retry_after_ms: pressure.retry_after_ms,
            retry_condition: pressure.retry_condition.clone(),
            owner_state: pressure.owner_state.as_str().into(),
            active_scope_id: pressure.active_scope_id.clone(),
            active_request_id: pressure.active_request_id.clone(),
            active_request_class: pressure
                .active_request_class
                .map(|class| class.as_str().into()),
        }
    }
}

fn request_key(connection_id: &str, request_id: &str) -> String {
    format!("{connection_id}:{request_id}")
}

fn valid_cancel_token(token: &str) -> bool {
    Uuid::parse_str(token).is_ok()
}

fn engine_error(error: EngineError) -> anyhow::Error {
    anyhow::Error::new(error)
}

fn request(
    request_id: &str,
    compatibility: EmbeddingCompatibility,
    operation: EmbeddingOperation,
) -> EmbeddingProtocolRequest {
    EmbeddingProtocolRequest {
        protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
        schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
        request_id: request_id.into(),
        compatibility,
        operation,
    }
}

fn hello(
    stream: &mut dyn EmbeddingServerStream,
    intent: EmbeddingConnectIntent,
    compatibility: EmbeddingCompatibility,
    transport_identity: &EmbeddingTransportIdentity,
    executable: &EmbeddingExecutableIdentity,
) -> Result<EmbeddingServerSnapshot> {
    let request_id = Uuid::new_v4().to_string();
    let intent = match intent {
        EmbeddingConnectIntent::Activate => "activate",
        EmbeddingConnectIntent::Observe => "observe",
    };
    let (response, _) = exchange(
        stream,
        request(
            &request_id,
            compatibility.clone(),
            EmbeddingOperation::Hello {
                intent: intent.into(),
                client_pid: executable.pid,
                client_process_start_id: executable.process_start_id.clone(),
                client_executable_sha256: executable.executable_sha256.clone(),
                client_executable_version: executable.executable_version.clone(),
            },
        ),
    )?;
    let EmbeddingResult::Hello {
        compatibility_sha256,
        snapshot,
    } = response_result(response)?
    else {
        bail!("embedding_server_protocol_mismatch: expected hello");
    };
    if compatibility_sha256 != compatibility.digest()? {
        bail!("embedding_server_incompatible_active_owner");
    }
    validate_server_snapshot(&snapshot, transport_identity, executable)?;
    Ok(*snapshot)
}

fn exchange(
    stream: &mut dyn EmbeddingServerStream,
    request: EmbeddingProtocolRequest,
) -> Result<(EmbeddingProtocolResponse, Vec<u8>)> {
    let request_id = request.request_id.clone();
    write_frame(stream, &request, &[])
        .map_err(|error| map_bounded_exchange_error(error, stream))?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(stream).map_err(|error| map_bounded_exchange_error(error, stream))?;
    if response.request_id != request_id {
        bail!("embedding_server_response_request_id_mismatch");
    }
    if response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
    {
        bail!("embedding_server_protocol_mismatch");
    }
    Ok((response, payload))
}

fn map_bounded_exchange_error(
    error: anyhow::Error,
    stream: &dyn EmbeddingServerStream,
) -> anyhow::Error {
    let io_kind = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<io::Error>().map(io::Error::kind));
    if matches!(
        io_kind,
        Some(io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
    ) {
        return error.context(PerUserEmbeddingError {
            code: "embedding_server_owner_unresponsive".into(),
            message: "the embedding server did not complete a bounded exchange".into(),
            retry_class: "after_server_change".into(),
            retry_after_ms: duration_ms(EmbeddingClientBudgets::current().retry_after),
            retry_condition: "the lifetime authority or server instance changes".into(),
            capacity: None,
        });
    }
    if matches!(
        io_kind,
        Some(
            io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::NotConnected
                | io::ErrorKind::UnexpectedEof
        )
    ) {
        let raw_os_error = exchange_raw_os_error(&error);
        let identity = stream.transport_identity();
        let (peer_state, peer_exit_code) = match stream.peer_exit_code() {
            Ok(Some(exit_code)) => ("exited".to_string(), Some(exit_code)),
            Ok(None) => match stream.peer_is_alive() {
                Ok(true) => ("running".to_string(), None),
                Ok(false) => match stream.peer_exit_code() {
                    Ok(exit_code) => ("exited".to_string(), exit_code),
                    Err(probe_error) => (
                        format!("exited (exit-code probe failed: {probe_error})"),
                        None,
                    ),
                },
                Err(probe_error) => (format!("unknown ({probe_error})"), None),
            },
            Err(probe_error) => (format!("unknown ({probe_error})"), None),
        };
        let source_chain = format!("{error:#}");
        let message = format!(
            "the authenticated embedding server connection was lost; raw_os_error={}; \
             peer_pid={}; peer_process_start_id={}; peer_state={peer_state}; \
             peer_exit_code={}; source={source_chain}",
            raw_os_error.map_or_else(|| "none".into(), |code| code.to_string()),
            identity
                .peer_pid
                .map_or_else(|| "unknown".into(), |pid| pid.to_string()),
            identity
                .peer_process_start_id
                .as_deref()
                .unwrap_or("unknown"),
            peer_exit_code.map_or_else(|| "none".into(), |code| code.to_string()),
        );
        return error.context(PerUserEmbeddingError {
            code: "embedding_server_connection_lost".into(),
            message,
            retry_class: "same_rpc_once".into(),
            retry_after_ms: 0,
            retry_condition: "the server instance changes".into(),
            capacity: None,
        });
    }
    error
}

fn exchange_raw_os_error(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .and_then(nested_io_raw_os_error)
    })
}

fn nested_io_raw_os_error(error: &io::Error) -> Option<i32> {
    error.raw_os_error().or_else(|| {
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .and_then(nested_io_raw_os_error)
    })
}

fn response_result(response: EmbeddingProtocolResponse) -> Result<EmbeddingResult> {
    if response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
    {
        bail!("embedding_server_protocol_mismatch");
    }
    if let Some(result) = response.result {
        return Ok(result);
    }
    let error = response
        .error
        .ok_or_else(|| anyhow!("embedding server returned neither result nor error"))?;
    Err(PerUserEmbeddingError {
        code: error.code,
        message: error.message,
        retry_class: error.retry_class,
        retry_after_ms: error.retry_after_ms,
        retry_condition: error.retry_condition,
        capacity: error.capacity,
    }
    .into())
}

fn vectors_result(
    result: (EmbeddingResult, Vec<u8>),
) -> Result<(u32, u32, EmbeddingEngineIdentity, Vec<u8>)> {
    let (
        EmbeddingResult::Vectors {
            rows,
            columns,
            encoding,
            identity,
        },
        payload,
    ) = result
    else {
        bail!("embedding_server_protocol_mismatch: expected vectors");
    };
    if encoding != "f32_le" {
        bail!("embedding_vector_encoding_mismatch");
    }
    Ok((rows, columns, *identity, payload))
}

fn write_frame<T: Serialize>(
    stream: &mut dyn EmbeddingServerStream,
    control: &T,
    payload: &[u8],
) -> Result<()> {
    let control = serde_json::to_vec(control).context("serialize embedding protocol frame")?;
    if control.len() > PER_USER_EMBEDDING_MAX_METADATA_BYTES
        || payload.len() > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
    {
        bail!("embedding_server_frame_too_large");
    }
    stream
        .write_all(&(control.len() as u32).to_be_bytes())
        .context("write embedding control length")?;
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .context("write embedding payload length")?;
    stream
        .write_all(&control)
        .context("write embedding control frame")?;
    stream
        .write_all(payload)
        .context("write embedding payload frame")?;
    stream.flush().context("flush embedding protocol frame")
}

fn read_frame<T: for<'de> Deserialize<'de>>(
    stream: &mut dyn EmbeddingServerStream,
) -> Result<(T, Vec<u8>)> {
    let mut control_len = [0_u8; 4];
    let mut payload_len = [0_u8; 4];
    stream
        .read_exact(&mut control_len)
        .context("read embedding control length")?;
    stream
        .read_exact(&mut payload_len)
        .context("read embedding payload length")?;
    let control_len = u32::from_be_bytes(control_len) as usize;
    let payload_len = u32::from_be_bytes(payload_len) as usize;
    if control_len == 0
        || control_len > PER_USER_EMBEDDING_MAX_METADATA_BYTES
        || payload_len > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
    {
        bail!("embedding_server_frame_too_large");
    }
    let mut control = vec![0_u8; control_len];
    let mut payload = vec![0_u8; payload_len];
    stream
        .read_exact(&mut control)
        .context("read embedding control frame")?;
    stream
        .read_exact(&mut payload)
        .context("read embedding payload frame")?;
    let control =
        serde_json::from_slice(&control).context("decode embedding protocol control frame")?;
    Ok((control, payload))
}

fn encode_vectors(vectors: &[Vec<f32>]) -> Result<Vec<u8>> {
    if vectors.len() > PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT
        || vectors
            .iter()
            .any(|vector| vector.len() != RETRIEVAL_EMBEDDING_DIM)
    {
        bail!("embedding_vector_shape_invalid");
    }
    let bytes = vectors
        .len()
        .checked_mul(RETRIEVAL_EMBEDDING_DIM)
        .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| anyhow!("embedding_vector_payload_overflow"))?;
    if bytes > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES {
        bail!("embedding_vector_payload_too_large");
    }
    let mut payload = Vec::with_capacity(bytes);
    for vector in vectors {
        for value in vector {
            payload.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(payload)
}

fn decode_vectors(rows: u32, columns: u32, payload: &[u8]) -> Result<Vec<Vec<f32>>> {
    if columns as usize != RETRIEVAL_EMBEDDING_DIM {
        bail!(
            "embedding_vector_dimension_mismatch: expected={} observed={columns}",
            RETRIEVAL_EMBEDDING_DIM
        );
    }
    let expected = (rows as usize)
        .checked_mul(columns as usize)
        .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| anyhow!("embedding_vector_payload_overflow"))?;
    if payload.len() != expected {
        bail!(
            "embedding_vector_payload_length_mismatch: expected={expected} observed={}",
            payload.len()
        );
    }
    let mut vectors = Vec::with_capacity(rows as usize);
    for row in payload.chunks_exact(columns as usize * std::mem::size_of::<f32>()) {
        let vector = row
            .chunks_exact(std::mem::size_of::<f32>())
            .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte f32 chunk")))
            .collect();
        vectors.push(vector);
    }
    Ok(vectors)
}

fn validate_engine_identity(
    identity: &EmbeddingEngineIdentity,
    compatibility: &EmbeddingCompatibility,
) -> Result<()> {
    if !identity.worker_alive || identity.load_error.is_some() {
        bail!("embedding_server_engine_unavailable");
    }
    if identity.model_digest != compatibility.model_sha256
        || identity.ggml_build_identity != compatibility.ggml_build_identity
        || identity.policy != compatibility.policy
        || identity.materialized_model_sha256 != compatibility.model_sha256
        || identity.load_generation == 0
    {
        bail!("embedding_server_engine_identity_mismatch");
    }
    Ok(())
}

fn validate_engine_server_identity(
    identity: &EmbeddingEngineIdentity,
    server: &EmbeddingServerSnapshot,
) -> Result<()> {
    if identity.server_instance_id != server.process.server_instance_id {
        bail!("embedding_server_instance_changed");
    }
    Ok(())
}

fn validate_lease_server_identity(
    lease: &EmbeddingEngineLeaseIdentity,
    identity: &EmbeddingEngineIdentity,
    server: &EmbeddingServerSnapshot,
) -> Result<()> {
    if lease.server_instance_id != server.process.server_instance_id
        || lease.server_instance_id != identity.server_instance_id
        || lease.load_generation != identity.load_generation
        || lease.lease_token.trim().is_empty()
    {
        bail!("embedding_publication_lease_changed");
    }
    Ok(())
}

fn validate_same_server(
    observed: &EmbeddingServerSnapshot,
    accepted: &EmbeddingServerSnapshot,
) -> Result<()> {
    if observed.process != accepted.process
        || observed.protocol != accepted.protocol
        || observed.authority != accepted.authority
    {
        bail!("embedding_server_instance_changed");
    }
    Ok(())
}

fn validate_server_snapshot(
    snapshot: &EmbeddingServerSnapshot,
    transport: &EmbeddingTransportIdentity,
    executable: &EmbeddingExecutableIdentity,
) -> Result<()> {
    if snapshot.schema_version != PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION
        || snapshot.protocol != EmbeddingServerProtocolSnapshot::current()
        || snapshot.process.server_instance_id.trim().is_empty()
        || snapshot.process.pid == 0
        || snapshot.process.process_start_id.trim().is_empty()
        || snapshot.process.executable_sha256.trim().is_empty()
        || snapshot.process.executable_version.trim().is_empty()
    {
        bail!("embedding_server_snapshot_contract_mismatch");
    }
    if !transport.peer_verified
        || !snapshot.authority.peer_verified
        || snapshot.authority.endpoint_namespace_id != transport.endpoint_namespace_id
        || snapshot.authority.lifetime_authority_id != transport.lifetime_authority_id
        || snapshot.authority.listener_id != transport.listener_id
        || transport.peer_pid != Some(snapshot.process.pid)
        || transport.peer_process_start_id.as_deref()
            != Some(snapshot.process.process_start_id.as_str())
    {
        bail!("embedding_server_peer_identity_mismatch");
    }
    if snapshot.process.executable_sha256 != executable.executable_sha256
        || snapshot.process.executable_version != executable.executable_version
    {
        bail!("embedding_server_executable_identity_mismatch");
    }
    Ok(())
}

fn configure_exchange_timeout(stream: &dyn EmbeddingServerStream, timeout: Duration) -> Result<()> {
    if timeout.is_zero() {
        bail!("embedding_server_timeout_invalid");
    }
    stream
        .set_read_timeout(Some(timeout))
        .map_err(exchange_timeout_configuration_error)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(exchange_timeout_configuration_error)?;
    Ok(())
}

fn exchange_timeout_configuration_error(error: io::Error) -> anyhow::Error {
    PerUserEmbeddingError {
        code: "embedding_server_owner_unresponsive".into(),
        message: format!("could not bound the embedding server exchange: {error}"),
        retry_class: "after_server_change".into(),
        retry_after_ms: duration_ms(EmbeddingClientBudgets::current().retry_after),
        retry_condition: "the lifetime authority or server instance changes".into(),
        capacity: None,
    }
    .into()
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn embedding_scope_id(runtime: &SidecarRuntimeConfig) -> String {
    let scope_seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    hex_sha256(scope_seed.as_bytes())
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn positive_duration_ms(duration: Duration) -> u64 {
    duration_ms(duration).max(1)
}

fn elapsed_since(clock: &dyn AwakeMonotonicClock, started_ns: u64) -> Duration {
    Duration::from_nanos(clock.now_ns().saturating_sub(started_ns))
}

fn is_server_loss(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<PerUserEmbeddingError>()
        .is_some_and(|error| {
            matches!(
                error.code.as_str(),
                "embedding_server_owner_unresponsive" | "embedding_server_connection_lost"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn project_runtime_identity_selects_a_distinct_embedding_scope() {
        let first_root = tempfile::tempdir().expect("first project root");
        let second_root = tempfile::tempdir().expect("second project root");
        let first = SidecarRuntimeConfig::for_project_profile(
            Some(first_root.path()),
            crate::config::SidecarProfile::Local,
        );
        let second = SidecarRuntimeConfig::for_project_profile(
            Some(second_root.path()),
            crate::config::SidecarProfile::Local,
        );

        assert_ne!(first.project_identity, second.project_identity);
        assert_ne!(embedding_scope_id(&first), embedding_scope_id(&second));
        assert_eq!(
            embedding_scope_id(&first),
            embedding_scope_id(&first.clone())
        );
    }

    #[derive(Debug)]
    struct TestClock {
        now: AtomicU64,
    }

    impl TestClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                now: AtomicU64::new(1),
            })
        }
    }

    impl AwakeMonotonicClock for TestClock {
        fn now_ns(&self) -> u64 {
            self.now.load(Ordering::Acquire)
        }

        fn sleep(&self, duration: Duration) {
            self.now.fetch_add(
                duration.as_nanos().max(1).min(u128::from(u64::MAX)) as u64,
                Ordering::AcqRel,
            );
        }

        fn snapshot(&self) -> EmbeddingServerClockSnapshot {
            EmbeddingServerClockSnapshot {
                domain: "awake_monotonic".into(),
                api: "test_clock".into(),
                boot_id: "test-boot".into(),
                resolution_ns: 1,
            }
        }
    }

    struct MemoryStream {
        identity: EmbeddingTransportIdentity,
        input: Cursor<Vec<u8>>,
        output: Arc<Mutex<Vec<u8>>>,
        finished_deliveries: Arc<AtomicUsize>,
        read_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
        write_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
        alive: bool,
        exit_codes: Mutex<Vec<Option<u32>>>,
    }

    struct MemoryStreamFixture {
        stream: MemoryStream,
        output: Arc<Mutex<Vec<u8>>>,
        finished_deliveries: Arc<AtomicUsize>,
        read_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
        write_timeouts: Arc<Mutex<Vec<Option<Duration>>>>,
    }

    impl MemoryStream {
        fn new(input: Vec<u8>, alive: bool) -> (Self, Arc<Mutex<Vec<u8>>>) {
            let fixture = Self::with_delivery_tracking(input, alive);
            (fixture.stream, fixture.output)
        }

        fn with_delivery_tracking(input: Vec<u8>, alive: bool) -> MemoryStreamFixture {
            let output = Arc::new(Mutex::new(Vec::new()));
            let finished_deliveries = Arc::new(AtomicUsize::new(0));
            let read_timeouts = Arc::new(Mutex::new(Vec::new()));
            let write_timeouts = Arc::new(Mutex::new(Vec::new()));
            MemoryStreamFixture {
                stream: Self {
                    identity: test_transport_identity(),
                    input: Cursor::new(input),
                    output: Arc::clone(&output),
                    finished_deliveries: Arc::clone(&finished_deliveries),
                    read_timeouts: Arc::clone(&read_timeouts),
                    write_timeouts: Arc::clone(&write_timeouts),
                    alive,
                    exit_codes: Mutex::new(vec![None]),
                },
                output,
                finished_deliveries,
                read_timeouts,
                write_timeouts,
            }
        }
    }

    impl Read for MemoryStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for MemoryStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.output
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl EmbeddingServerStream for MemoryStream {
        fn transport_identity(&self) -> &EmbeddingTransportIdentity {
            &self.identity
        }

        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.read_timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(timeout);
            Ok(())
        }

        fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.write_timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(timeout);
            Ok(())
        }

        fn peer_is_alive(&self) -> io::Result<bool> {
            Ok(self.alive)
        }

        fn peer_exit_code(&self) -> io::Result<Option<u32>> {
            let mut exit_codes = self
                .exit_codes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if exit_codes.len() > 1 {
                return Ok(exit_codes.remove(0));
            }
            Ok(exit_codes.first().copied().flatten())
        }

        fn finish_response_delivery(&self) -> io::Result<()> {
            self.finished_deliveries.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        fn shutdown(&self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone)]
    enum ScriptOutcome {
        Success,
        Loss,
        HelloLoss,
        Capacity,
        TimedBulk {
            hello_delay: Duration,
            exchange_delay: Duration,
            lose_exchange: bool,
        },
        Blocking {
            request_started: Arc<AtomicBool>,
            cancelled: Arc<AtomicBool>,
        },
    }

    struct ScriptStream {
        identity: EmbeddingTransportIdentity,
        writes: Vec<u8>,
        reads: Cursor<Vec<u8>>,
        outcome: ScriptOutcome,
        compatibility: EmbeddingCompatibility,
        read_gate: Option<Arc<AtomicBool>>,
    }

    impl ScriptStream {
        fn new(outcome: ScriptOutcome, compatibility: EmbeddingCompatibility) -> Self {
            Self {
                identity: test_transport_identity(),
                writes: Vec::new(),
                reads: Cursor::new(Vec::new()),
                outcome,
                compatibility,
                read_gate: None,
            }
        }

        fn prepare_response(&mut self) -> io::Result<()> {
            let request: EmbeddingProtocolRequest =
                decode_test_frame(&self.writes).map_err(io::Error::other)?;
            self.writes.clear();
            let (response, payload) = match request.operation {
                EmbeddingOperation::Hello { .. } => {
                    if matches!(self.outcome, ScriptOutcome::HelloLoss) {
                        self.reads = Cursor::new(Vec::new());
                        return Ok(());
                    }
                    if let ScriptOutcome::TimedBulk { hello_delay, .. } = self.outcome {
                        thread::sleep(hello_delay);
                    }
                    (
                        success_response(
                            &request.request_id,
                            EmbeddingResult::Hello {
                                compatibility_sha256: self
                                    .compatibility
                                    .digest()
                                    .map_err(io::Error::other)?,
                                snapshot: Box::new(test_snapshot()),
                            },
                        ),
                        Vec::new(),
                    )
                }
                EmbeddingOperation::EmbedQuery { .. } => match self.outcome.clone() {
                    ScriptOutcome::Loss => {
                        self.reads = Cursor::new(Vec::new());
                        return Ok(());
                    }
                    ScriptOutcome::Capacity => (
                        failure_response(
                            &request.request_id,
                            EmbeddingProtocolError {
                                code: "embedding_capacity".into(),
                                message: "query queue is full".into(),
                                retry_class: "after_capacity_change".into(),
                                retry_after_ms: 10,
                                retry_condition: "a live request completes".into(),
                                capacity: Some(test_capacity()),
                            },
                        ),
                        Vec::new(),
                    ),
                    ScriptOutcome::Success => {
                        let mut vector = vec![0.0_f32; RETRIEVAL_EMBEDDING_DIM];
                        vector[0] = 1.0;
                        (
                            success_response(
                                &request.request_id,
                                EmbeddingResult::Vectors {
                                    rows: 1,
                                    columns: RETRIEVAL_EMBEDDING_DIM as u32,
                                    encoding: "f32_le".into(),
                                    identity: Box::new(test_engine_identity()),
                                },
                            ),
                            encode_vectors(&[vector]).map_err(io::Error::other)?,
                        )
                    }
                    ScriptOutcome::Blocking {
                        request_started,
                        cancelled,
                    } => {
                        request_started.store(true, Ordering::Release);
                        self.read_gate = Some(cancelled);
                        (
                            failure_response(
                                &request.request_id,
                                EmbeddingProtocolError {
                                    code: "embedding_cancelled".into(),
                                    message: "the active request was cancelled".into(),
                                    retry_class: "none".into(),
                                    retry_after_ms: 0,
                                    retry_condition: "the caller starts a new request".into(),
                                    capacity: None,
                                },
                            ),
                            Vec::new(),
                        )
                    }
                    ScriptOutcome::HelloLoss => {
                        return Err(io::Error::other("query reached hello-loss stream"));
                    }
                    ScriptOutcome::TimedBulk { .. } => {
                        return Err(io::Error::other("query reached timed bulk stream"));
                    }
                },
                EmbeddingOperation::EmbedDocuments { inputs, .. } => {
                    let ScriptOutcome::TimedBulk {
                        exchange_delay,
                        lose_exchange,
                        ..
                    } = self.outcome
                    else {
                        return Err(io::Error::other("documents reached non-bulk stream"));
                    };
                    thread::sleep(exchange_delay);
                    if lose_exchange {
                        self.reads = Cursor::new(Vec::new());
                        return Ok(());
                    }
                    let vectors = (0..inputs.len())
                        .map(|_| {
                            let mut vector = vec![0.0_f32; RETRIEVAL_EMBEDDING_DIM];
                            vector[0] = 1.0;
                            vector
                        })
                        .collect::<Vec<_>>();
                    (
                        success_response(
                            &request.request_id,
                            EmbeddingResult::Vectors {
                                rows: inputs.len() as u32,
                                columns: RETRIEVAL_EMBEDDING_DIM as u32,
                                encoding: "f32_le".into(),
                                identity: Box::new(test_engine_identity()),
                            },
                        ),
                        encode_vectors(&vectors).map_err(io::Error::other)?,
                    )
                }
                EmbeddingOperation::Cancel { .. } => {
                    let ScriptOutcome::Blocking { cancelled, .. } = self.outcome.clone() else {
                        return Err(io::Error::other("unexpected cancellation request"));
                    };
                    cancelled.store(true, Ordering::Release);
                    (
                        success_response(&request.request_id, EmbeddingResult::Cancelled),
                        Vec::new(),
                    )
                }
                EmbeddingOperation::Snapshot => (
                    success_response(
                        &request.request_id,
                        EmbeddingResult::Snapshot {
                            snapshot: Box::new(test_snapshot()),
                            lease: None,
                            identity: None,
                        },
                    ),
                    Vec::new(),
                ),
                _ => (
                    failure_response(
                        &request.request_id,
                        protocol_error("test_operation_unsupported", "unsupported test operation"),
                    ),
                    Vec::new(),
                ),
            };
            self.reads = Cursor::new(encode_test_frame(&response, &payload));
            Ok(())
        }
    }

    impl Read for ScriptStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            while self
                .read_gate
                .as_ref()
                .is_some_and(|gate| !gate.load(Ordering::Acquire))
            {
                thread::sleep(Duration::from_millis(1));
            }
            self.reads.read(buffer)
        }
    }

    impl Write for ScriptStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.prepare_response()
        }
    }

    impl EmbeddingServerStream for ScriptStream {
        fn transport_identity(&self) -> &EmbeddingTransportIdentity {
            &self.identity
        }

        fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }

        fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }

        fn peer_is_alive(&self) -> io::Result<bool> {
            Ok(true)
        }

        fn shutdown(&self) -> io::Result<()> {
            Ok(())
        }
    }

    struct StallingHelloStream {
        identity: EmbeddingTransportIdentity,
        read_timeout: Mutex<Option<Duration>>,
        observed_read_timeout: Arc<Mutex<Option<Duration>>>,
    }

    impl Read for StallingHelloStream {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            let timeout = self
                .read_timeout
                .lock()
                .expect("stalling Hello read timeout")
                .expect("Hello exchange must configure a read timeout");
            thread::sleep(timeout.saturating_add(Duration::from_millis(5)));
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "scripted initial Hello stall",
            ))
        }
    }

    impl Write for StallingHelloStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl EmbeddingServerStream for StallingHelloStream {
        fn transport_identity(&self) -> &EmbeddingTransportIdentity {
            &self.identity
        }

        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            *self
                .read_timeout
                .lock()
                .expect("stalling Hello read timeout") = timeout;
            *self
                .observed_read_timeout
                .lock()
                .expect("observed Hello read timeout") = timeout;
            Ok(())
        }

        fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }

        fn peer_is_alive(&self) -> io::Result<bool> {
            Ok(true)
        }

        fn shutdown(&self) -> io::Result<()> {
            Ok(())
        }
    }

    struct ClientTestTransport {
        clock: Arc<TestClock>,
        connect_count: AtomicUsize,
        spawn_count: AtomicUsize,
        loss_count: usize,
        capacity: bool,
        compatibility: EmbeddingCompatibility,
    }

    impl ClientTestTransport {
        fn new(loss_count: usize, capacity: bool) -> Arc<Self> {
            Arc::new(Self {
                clock: TestClock::new(),
                connect_count: AtomicUsize::new(0),
                spawn_count: AtomicUsize::new(0),
                loss_count,
                capacity,
                compatibility: EmbeddingCompatibility::current(true),
            })
        }
    }

    impl EmbeddingClientTransport for ClientTestTransport {
        fn connect(
            &self,
            _intent: EmbeddingConnectIntent,
            _budget: Duration,
            _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
        ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
            let attempt = self.connect_count.fetch_add(1, Ordering::AcqRel) + 1;
            let outcome = if self.capacity {
                ScriptOutcome::Capacity
            } else if attempt <= self.loss_count {
                ScriptOutcome::Loss
            } else {
                ScriptOutcome::Success
            };
            Ok(EmbeddingConnectOutcome::Connected(Box::new(
                ScriptStream::new(outcome, self.compatibility.clone()),
            )))
        }

        fn spawn_exact_current_exe(
            &self,
        ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
            let generation = self.spawn_count.fetch_add(1, Ordering::AcqRel) as u64 + 1;
            Ok(EmbeddingSpawnAttempt::new(generation))
        }

        fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
            self.clock.clone()
        }

        fn executable_identity(&self) -> EmbeddingExecutableIdentity {
            test_executable()
        }

        fn budgets(&self) -> EmbeddingClientBudgets {
            EmbeddingClientBudgets::current()
        }
    }

    struct ControlledCancelTestTransport {
        clock: Arc<TestClock>,
        connect_count: AtomicUsize,
        request_started: Arc<AtomicBool>,
        server_cancelled: Arc<AtomicBool>,
        compatibility: EmbeddingCompatibility,
    }

    impl ControlledCancelTestTransport {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                clock: TestClock::new(),
                connect_count: AtomicUsize::new(0),
                request_started: Arc::new(AtomicBool::new(false)),
                server_cancelled: Arc::new(AtomicBool::new(false)),
                compatibility: EmbeddingCompatibility::current(true),
            })
        }
    }

    impl EmbeddingClientTransport for ControlledCancelTestTransport {
        fn connect(
            &self,
            _intent: EmbeddingConnectIntent,
            _budget: Duration,
            _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
        ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
            self.connect_count.fetch_add(1, Ordering::AcqRel);
            Ok(EmbeddingConnectOutcome::Connected(Box::new(
                ScriptStream::new(
                    ScriptOutcome::Blocking {
                        request_started: Arc::clone(&self.request_started),
                        cancelled: Arc::clone(&self.server_cancelled),
                    },
                    self.compatibility.clone(),
                ),
            )))
        }

        fn spawn_exact_current_exe(
            &self,
        ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
            Ok(EmbeddingSpawnAttempt::new(1))
        }

        fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
            self.clock.clone()
        }

        fn executable_identity(&self) -> EmbeddingExecutableIdentity {
            test_executable()
        }

        fn budgets(&self) -> EmbeddingClientBudgets {
            EmbeddingClientBudgets {
                connect: Duration::from_millis(100),
                spawn: Duration::from_millis(100),
                retry_after: Duration::from_millis(1),
                query_request: Duration::from_secs(1),
                bulk_request: Duration::from_secs(1),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum BootstrapConnectOutcome {
        Connected,
        Loss,
        HelloLoss,
        NoOwner,
        OwnerUnresponsive,
    }

    struct BootstrapTestTransport {
        clock: Arc<TestClock>,
        connect_count: AtomicUsize,
        spawn_count: AtomicUsize,
        outcomes: Mutex<std::collections::VecDeque<BootstrapConnectOutcome>>,
        fallback: BootstrapConnectOutcome,
        budgets: EmbeddingClientBudgets,
        compatibility: EmbeddingCompatibility,
    }

    struct DeadlineBudgetTransport {
        clock: Arc<TestClock>,
        connect_count: AtomicUsize,
        spawn_count: AtomicUsize,
        compatibility: EmbeddingCompatibility,
    }

    struct ExplicitDeadlineTransport {
        clock: Arc<TestClock>,
        connect_count: AtomicUsize,
        observed_connect_budget: Mutex<Option<Duration>>,
        observed_read_timeout: Arc<Mutex<Option<Duration>>>,
    }

    impl ExplicitDeadlineTransport {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                clock: TestClock::new(),
                connect_count: AtomicUsize::new(0),
                observed_connect_budget: Mutex::new(None),
                observed_read_timeout: Arc::new(Mutex::new(None)),
            })
        }
    }

    impl EmbeddingClientTransport for ExplicitDeadlineTransport {
        fn connect(
            &self,
            _intent: EmbeddingConnectIntent,
            budget: Duration,
            _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
        ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
            self.connect_count.fetch_add(1, Ordering::AcqRel);
            *self
                .observed_connect_budget
                .lock()
                .expect("observed connect budget") = Some(budget);
            Ok(EmbeddingConnectOutcome::Connected(Box::new(
                StallingHelloStream {
                    identity: test_transport_identity(),
                    read_timeout: Mutex::new(None),
                    observed_read_timeout: Arc::clone(&self.observed_read_timeout),
                },
            )))
        }

        fn spawn_exact_current_exe(
            &self,
        ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
            panic!("an explicit deadline must expire before spawning")
        }

        fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
            self.clock.clone()
        }

        fn executable_identity(&self) -> EmbeddingExecutableIdentity {
            test_executable()
        }

        fn budgets(&self) -> EmbeddingClientBudgets {
            EmbeddingClientBudgets {
                connect: Duration::from_millis(500),
                spawn: Duration::from_millis(500),
                retry_after: Duration::from_millis(10),
                query_request: Duration::from_millis(500),
                bulk_request: Duration::from_millis(500),
            }
        }
    }

    impl DeadlineBudgetTransport {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                clock: TestClock::new(),
                connect_count: AtomicUsize::new(0),
                spawn_count: AtomicUsize::new(0),
                compatibility: EmbeddingCompatibility::current(true),
            })
        }
    }

    impl EmbeddingClientTransport for DeadlineBudgetTransport {
        fn connect(
            &self,
            _intent: EmbeddingConnectIntent,
            _budget: Duration,
            _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
        ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
            let attempt = self.connect_count.fetch_add(1, Ordering::AcqRel) + 1;
            Ok(match attempt {
                1 => EmbeddingConnectOutcome::Connected(Box::new(ScriptStream::new(
                    ScriptOutcome::TimedBulk {
                        hello_delay: Duration::from_millis(200),
                        exchange_delay: Duration::from_millis(100),
                        lose_exchange: true,
                    },
                    self.compatibility.clone(),
                ))),
                2 => EmbeddingConnectOutcome::NoOwner,
                3 => {
                    thread::sleep(Duration::from_millis(75));
                    EmbeddingConnectOutcome::OwnerUnresponsive(EmbeddingTransportFailure {
                        code: "embedding_server_owner_unresponsive".into(),
                        message: "the fail-stopped owner is releasing authority".into(),
                    })
                }
                _ => EmbeddingConnectOutcome::Connected(Box::new(ScriptStream::new(
                    ScriptOutcome::TimedBulk {
                        hello_delay: Duration::ZERO,
                        exchange_delay: Duration::from_millis(100),
                        lose_exchange: false,
                    },
                    self.compatibility.clone(),
                ))),
            })
        }

        fn spawn_exact_current_exe(
            &self,
        ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
            let generation = self.spawn_count.fetch_add(1, Ordering::AcqRel) as u64 + 1;
            Ok(EmbeddingSpawnAttempt::new(generation))
        }

        fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
            self.clock.clone()
        }

        fn executable_identity(&self) -> EmbeddingExecutableIdentity {
            test_executable()
        }

        fn budgets(&self) -> EmbeddingClientBudgets {
            EmbeddingClientBudgets {
                connect: Duration::from_millis(10),
                spawn: Duration::from_millis(100),
                retry_after: Duration::from_millis(1),
                query_request: Duration::from_millis(400),
                bulk_request: Duration::from_millis(400),
            }
        }
    }

    impl BootstrapTestTransport {
        fn new(
            outcomes: impl IntoIterator<Item = BootstrapConnectOutcome>,
            fallback: BootstrapConnectOutcome,
            spawn: Duration,
        ) -> Arc<Self> {
            Arc::new(Self {
                clock: TestClock::new(),
                connect_count: AtomicUsize::new(0),
                spawn_count: AtomicUsize::new(0),
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                fallback,
                budgets: EmbeddingClientBudgets {
                    connect: Duration::from_millis(1),
                    spawn,
                    retry_after: Duration::from_millis(1),
                    query_request: Duration::from_secs(1),
                    bulk_request: Duration::from_secs(1),
                },
                compatibility: EmbeddingCompatibility::current(true),
            })
        }
    }

    impl EmbeddingClientTransport for BootstrapTestTransport {
        fn connect(
            &self,
            _intent: EmbeddingConnectIntent,
            _budget: Duration,
            _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
        ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
            self.connect_count.fetch_add(1, Ordering::AcqRel);
            let outcome = self
                .outcomes
                .lock()
                .expect("bootstrap outcome script")
                .pop_front()
                .unwrap_or(self.fallback);
            Ok(match outcome {
                BootstrapConnectOutcome::Connected => EmbeddingConnectOutcome::Connected(Box::new(
                    ScriptStream::new(ScriptOutcome::Success, self.compatibility.clone()),
                )),
                BootstrapConnectOutcome::Loss => EmbeddingConnectOutcome::Connected(Box::new(
                    ScriptStream::new(ScriptOutcome::Loss, self.compatibility.clone()),
                )),
                BootstrapConnectOutcome::HelloLoss => EmbeddingConnectOutcome::Connected(Box::new(
                    ScriptStream::new(ScriptOutcome::HelloLoss, self.compatibility.clone()),
                )),
                BootstrapConnectOutcome::NoOwner => EmbeddingConnectOutcome::NoOwner,
                BootstrapConnectOutcome::OwnerUnresponsive => {
                    EmbeddingConnectOutcome::OwnerUnresponsive(EmbeddingTransportFailure {
                        code: "embedding_server_owner_unresponsive".into(),
                        message: "the lifetime authority exists without a live endpoint".into(),
                    })
                }
            })
        }

        fn spawn_exact_current_exe(
            &self,
        ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
            let generation = self.spawn_count.fetch_add(1, Ordering::AcqRel) as u64 + 1;
            Ok(EmbeddingSpawnAttempt::new(generation))
        }

        fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
            self.clock.clone()
        }

        fn executable_identity(&self) -> EmbeddingExecutableIdentity {
            test_executable()
        }

        fn budgets(&self) -> EmbeddingClientBudgets {
            self.budgets
        }
    }

    struct WatchdogTransport {
        clock: Arc<TestClock>,
        fail_stops: AtomicUsize,
    }

    impl EmbeddingServerTransport for WatchdogTransport {
        fn bind(
            &self,
        ) -> std::result::Result<EmbeddingServerBindOutcome, EmbeddingTransportFailure> {
            Err(EmbeddingTransportFailure {
                code: "test".into(),
                message: "not used".into(),
            })
        }

        fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
            self.clock.clone()
        }

        fn fail_stop(&self, _reason_code: &str) {
            self.fail_stops.fetch_add(1, Ordering::AcqRel);
        }
    }

    struct PollingStream {
        inner: MemoryStream,
        pending_reads: usize,
    }

    impl Read for PollingStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.pending_reads != 0 {
                self.pending_reads -= 1;
                return Err(io::Error::new(io::ErrorKind::TimedOut, "poll"));
            }
            self.inner.read(buffer)
        }
    }

    impl Write for PollingStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.inner.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl EmbeddingServerStream for PollingStream {
        fn transport_identity(&self) -> &EmbeddingTransportIdentity {
            self.inner.transport_identity()
        }

        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.inner.set_read_timeout(timeout)
        }

        fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.inner.set_write_timeout(timeout)
        }

        fn peer_is_alive(&self) -> io::Result<bool> {
            Ok(true)
        }

        fn shutdown(&self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pure_rpc_replays_once_and_only_once_on_typed_loss() {
        let transport = ClientTestTransport::new(1, false);
        let client = test_client(transport.clone());
        let (vector, attempts) = client
            .embed_query_with_qualification_attempts("x")
            .expect("one replay succeeds");
        assert_eq!(vector.len(), RETRIEVAL_EMBEDDING_DIM);
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 2);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].ordinal, 1);
        assert_eq!(attempts[0].outcome, "server_loss");
        assert_eq!(attempts[1].ordinal, 2);
        assert_eq!(attempts[1].outcome, "completed");
        assert_ne!(attempts[0].request_id, attempts[1].request_id);

        let transport = ClientTestTransport::new(usize::MAX, false);
        let client = test_client(transport.clone());
        let error = client
            .embed_query("x")
            .expect_err("second loss is terminal");
        assert!(is_server_loss(&error));
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 2);
    }

    #[test]
    fn pure_rpc_replay_waits_for_a_fail_stopped_owner_to_release_authority() {
        let transport = BootstrapTestTransport::new(
            [
                BootstrapConnectOutcome::Loss,
                BootstrapConnectOutcome::OwnerUnresponsive,
                BootstrapConnectOutcome::NoOwner,
                BootstrapConnectOutcome::OwnerUnresponsive,
                BootstrapConnectOutcome::Connected,
            ],
            BootstrapConnectOutcome::Connected,
            Duration::from_millis(5),
        );
        let client = PerUserEmbeddingClient {
            transport: transport.clone(),
            compatibility: EmbeddingCompatibility::current(true),
            scope_id: "test-scope".into(),
        };

        let (_, attempts) = client
            .embed_query_with_qualification_attempts("x")
            .expect("one replay converges after the fail-stopped owner releases authority");

        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].outcome, "server_loss");
        assert_eq!(attempts[1].outcome, "completed");
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 5);
    }

    #[test]
    fn pure_rpc_replay_converges_after_recovery_hello_loss() {
        let transport = BootstrapTestTransport::new(
            [
                BootstrapConnectOutcome::Loss,
                BootstrapConnectOutcome::HelloLoss,
                BootstrapConnectOutcome::OwnerUnresponsive,
                BootstrapConnectOutcome::NoOwner,
                BootstrapConnectOutcome::OwnerUnresponsive,
                BootstrapConnectOutcome::Connected,
            ],
            BootstrapConnectOutcome::Connected,
            Duration::from_millis(6),
        );
        let client = PerUserEmbeddingClient {
            transport: transport.clone(),
            compatibility: EmbeddingCompatibility::current(true),
            scope_id: "test-scope".into(),
        };

        let (_, attempts) = client
            .embed_query_with_qualification_attempts("x")
            .expect("recovery hello loss converges before the replay RPC is sent");

        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].outcome, "server_loss");
        assert_eq!(attempts[1].outcome, "completed");
        assert_ne!(attempts[0].request_id, attempts[1].request_id);
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 6);
    }

    #[test]
    fn bulk_replay_budget_preserves_the_full_replay_window_after_initial_bootstrap() {
        // The frozen bulk deadline is the sum of the stalled-native window,
        // replacement convergence, and replay-success budget. Keep those
        // phases comfortably inside the 400 ms test deadline, then add 200 ms
        // of initial Hello work that the frozen formula does not account for.
        // The old accounting takes about 475 ms and the repaired accounting
        // about 275 ms, leaving wide real-time margins around the same 400 ms
        // deadline despite ordinary scheduler jitter.
        let transport = DeadlineBudgetTransport::new();
        let client = test_client(transport.clone());
        let result = client.embed_documents_with_qualification_attempts(&["x".into()]);
        let observed = result
            .as_ref()
            .err()
            .and_then(embedding_retry_state)
            .map(|retry| retry.code);

        assert!(
            result.is_ok(),
            "a contract-sized recovery must retain its full replay window; observed_code={observed:?}, result={result:?}"
        );
        let (_, attempts) = result.expect("successful replay");
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].outcome, "server_loss");
        assert_eq!(attempts[1].outcome, "completed");
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn explicit_caller_deadline_bounds_initial_hello() {
        let transport = ExplicitDeadlineTransport::new();
        let client = test_client(transport.clone());
        let started = Instant::now();
        let error = client
            .embed_query_with_control("x", Some(Duration::from_millis(50)), &|| false)
            .expect_err("the explicit caller deadline must bound initial Hello");
        let elapsed = started.elapsed();
        let retry = embedding_retry_state(&error).expect("typed caller deadline");
        let connect_budget = transport
            .observed_connect_budget
            .lock()
            .expect("observed connect budget")
            .expect("connect budget");
        let read_timeout = transport
            .observed_read_timeout
            .lock()
            .expect("observed Hello read timeout")
            .expect("Hello read timeout");

        assert_eq!(retry.code, "embedding_deadline_exceeded");
        assert!(connect_budget <= Duration::from_millis(50));
        assert!(read_timeout <= Duration::from_millis(50));
        assert!(
            elapsed < Duration::from_millis(250),
            "explicit deadline took {elapsed:?}, approaching the 500 ms connect budget"
        );
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn pure_rpc_does_not_wait_for_a_preexisting_frozen_owner() {
        let transport = BootstrapTestTransport::new(
            [BootstrapConnectOutcome::OwnerUnresponsive],
            BootstrapConnectOutcome::OwnerUnresponsive,
            Duration::from_millis(5),
        );
        let client = PerUserEmbeddingClient {
            transport: transport.clone(),
            compatibility: EmbeddingCompatibility::current(true),
            scope_id: "test-scope".into(),
        };

        let error = client
            .embed_query("x")
            .expect_err("a pre-existing frozen owner remains a typed failure");
        let typed = error
            .downcast_ref::<PerUserEmbeddingError>()
            .expect("typed frozen-owner state");

        assert_eq!(typed.code, "embedding_server_owner_unresponsive");
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 2);
    }

    #[test]
    fn caller_cancellation_interrupts_active_rpc_over_authenticated_control_connection() {
        let transport = ControlledCancelTestTransport::new();
        let client = test_client(transport.clone());
        let caller_cancelled = AtomicBool::new(false);

        let error = thread::scope(|scope| {
            let request = scope.spawn(|| {
                client.embed_query_with_control("x", Some(Duration::from_secs(1)), &|| {
                    caller_cancelled.load(Ordering::Acquire)
                })
            });
            while !transport.request_started.load(Ordering::Acquire) {
                thread::yield_now();
            }
            caller_cancelled.store(true, Ordering::Release);
            request
                .join()
                .expect("controlled request thread")
                .expect_err("caller cancellation must win")
        });

        let retry = embedding_retry_state(&error).expect("typed cancellation");
        assert_eq!(retry.code, "embedding_cancelled");
        assert_eq!(retry.retry_class, "none");
        assert!(transport.server_cancelled.load(Ordering::Acquire));
        assert!(
            transport.connect_count.load(Ordering::Acquire) >= 2,
            "the watcher must use a separate authenticated control connection"
        );
    }

    #[test]
    fn cancellation_wins_before_connection_loss_can_replay() {
        let transport = ClientTestTransport::new(usize::MAX, false);
        let client = test_client(transport.clone());
        let error = client
            .embed_query_with_control("x", Some(Duration::from_secs(1)), &|| {
                transport.connect_count.load(Ordering::Acquire) > 0
            })
            .expect_err("cancellation after connect must suppress pure replay");

        assert_eq!(
            embedding_retry_state(&error)
                .expect("typed cancellation")
                .code,
            "embedding_cancelled"
        );
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn typed_capacity_does_not_spawn_or_replay() {
        let transport = ClientTestTransport::new(0, true);
        let client = test_client(transport.clone());
        let error = client.embed_query("x").expect_err("capacity is surfaced");
        let pressure = embedding_capacity_pressure(&error).expect("typed pressure");
        assert_eq!(pressure.reason, "queue_full");
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn post_spawn_authority_without_endpoint_converges_within_spawn_budget() {
        let transport = BootstrapTestTransport::new(
            [
                BootstrapConnectOutcome::NoOwner,
                BootstrapConnectOutcome::OwnerUnresponsive,
                BootstrapConnectOutcome::Connected,
            ],
            BootstrapConnectOutcome::Connected,
            Duration::from_millis(5),
        );
        let client = PerUserEmbeddingClient {
            transport: transport.clone(),
            compatibility: EmbeddingCompatibility::current(true),
            scope_id: "test-scope".into(),
        };

        client
            .connect(EmbeddingConnectIntent::Activate, true)
            .expect("a spawned owner may hold authority before publishing its endpoint");

        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 3);
    }

    #[test]
    fn preexisting_frozen_authority_remains_typed_and_does_not_spawn() {
        let transport = BootstrapTestTransport::new(
            [BootstrapConnectOutcome::OwnerUnresponsive],
            BootstrapConnectOutcome::OwnerUnresponsive,
            Duration::from_millis(5),
        );
        let client = PerUserEmbeddingClient {
            transport: transport.clone(),
            compatibility: EmbeddingCompatibility::current(true),
            scope_id: "test-scope".into(),
        };

        let error = match client.connect(EmbeddingConnectIntent::Activate, true) {
            Ok(_) => panic!("an owner present before spawn is terminal"),
            Err(error) => error,
        };
        let typed = error
            .downcast_ref::<PerUserEmbeddingError>()
            .expect("typed owner state");

        assert_eq!(typed.code, "embedding_server_owner_unresponsive");
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn post_spawn_owner_convergence_is_hard_bounded() {
        let transport = BootstrapTestTransport::new(
            [BootstrapConnectOutcome::NoOwner],
            BootstrapConnectOutcome::OwnerUnresponsive,
            Duration::from_millis(2),
        );
        let client = PerUserEmbeddingClient {
            transport: transport.clone(),
            compatibility: EmbeddingCompatibility::current(true),
            scope_id: "test-scope".into(),
        };

        let error = match client.connect(EmbeddingConnectIntent::Activate, true) {
            Ok(_) => panic!("a spawned owner must publish within the convergence budget"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("embedding_server_start_timeout"));
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 4);
    }

    #[test]
    fn response_correlation_and_protocol_hashes_are_enforced() {
        let response = success_response("wrong", EmbeddingResult::Released);
        let (mut stream, _) = MemoryStream::new(encode_test_frame(&response, &[]), true);
        let error = exchange(
            &mut stream,
            request(
                "expected",
                EmbeddingCompatibility::current(true),
                EmbeddingOperation::Snapshot,
            ),
        )
        .expect_err("wrong response id");
        assert!(
            error
                .to_string()
                .contains("embedding_server_response_request_id_mismatch")
        );

        validate_server_snapshot(
            &test_snapshot(),
            &test_transport_identity(),
            &test_executable(),
        )
        .expect("same exact executable digest is compatible");

        let mut snapshot = test_snapshot();
        snapshot.protocol.protocol_sha256 = "wrong".into();
        assert!(
            validate_server_snapshot(&snapshot, &test_transport_identity(), &test_executable(),)
                .is_err()
        );

        let mut snapshot = test_snapshot();
        snapshot.process.executable_sha256 = "b".repeat(64);
        let error =
            validate_server_snapshot(&snapshot, &test_transport_identity(), &test_executable())
                .expect_err("snapshot executable digest mismatch");
        assert!(
            error
                .to_string()
                .contains("embedding_server_executable_identity_mismatch")
        );
    }

    #[test]
    fn checked_in_protocol_hash_flows_into_the_build_marker() {
        let expected = hex_sha256(include_bytes!(
            "../../../docs/testing/per-user-embedding-server-protocol.json"
        ));
        assert_eq!(PER_USER_EMBEDDING_PROTOCOL_SHA256, expected);

        let marker =
            std::str::from_utf8(PER_USER_EMBEDDING_SERVER_PROOF_MARKER).expect("UTF-8 marker");
        assert!(
            marker.contains(&format!("protocol_sha256={expected}|")),
            "build marker did not bind the checked-in protocol hash: {marker}"
        );
    }

    #[test]
    fn transport_identity_contains_no_peer_image_hash() {
        let identity =
            serde_json::to_value(test_transport_identity()).expect("serialize transport identity");
        assert!(identity.get("peer_executable_sha256").is_none());
        assert_eq!(identity["peer_pid"], 42);
        assert_eq!(identity["peer_process_start_id"], "server-start");
    }

    #[test]
    fn hello_process_start_claim_must_match_authenticated_transport() {
        let mut operation = test_hello_operation("observe");
        let EmbeddingOperation::Hello {
            client_process_start_id,
            ..
        } = &mut operation
        else {
            unreachable!("test helper always builds hello");
        };
        *client_process_start_id = "stale-start".into();
        let hello = request(
            "stale-client",
            EmbeddingCompatibility::current(true),
            operation,
        );
        let fixture = MemoryStream::with_delivery_tracking(encode_test_frame(&hello, &[]), true);
        let error = serve_embedding_connection(test_server_state(), Box::new(fixture.stream))
            .expect_err("stale client identity must fail");
        assert!(
            error
                .to_string()
                .contains("embedding_server_peer_identity_mismatch")
        );
        assert_eq!(
            fixture.finished_deliveries.load(Ordering::Acquire),
            0,
            "an uncorrelated protocol failure must not pretend response delivery completed"
        );
    }

    #[test]
    fn bounded_frames_reject_oversized_lengths_before_allocation() {
        let mut bytes = Vec::new();
        bytes
            .extend_from_slice(&((PER_USER_EMBEDDING_MAX_METADATA_BYTES + 1) as u32).to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        let (mut stream, _) = MemoryStream::new(bytes, true);
        let error = read_frame::<serde_json::Value>(&mut stream).expect_err("oversized frame");
        assert!(
            error
                .to_string()
                .contains("embedding_server_frame_too_large")
        );
    }

    #[test]
    fn server_response_write_timeout_cannot_exceed_the_frozen_query_budget() {
        let fixture = MemoryStream::with_delivery_tracking(Vec::new(), true);

        configure_server_operation_timeout(&fixture.stream, 24 * 60 * 60 * 1_000)
            .expect("configure peer-selected exchange deadline");

        let expected = Some(EmbeddingClientBudgets::current().query_request);
        assert_eq!(
            fixture
                .read_timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last()
                .copied()
                .flatten(),
            expected,
            "a peer-selected deadline must not retain a server read beyond the frozen cap"
        );
        assert_eq!(
            fixture
                .write_timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last()
                .copied()
                .flatten(),
            expected,
            "a non-reading peer must not retain a response writer beyond the frozen cap"
        );
    }

    #[test]
    fn bounded_exchange_maps_normalized_disconnect_and_retains_peer_evidence() {
        let raw_error = io::Error::from_raw_os_error(233);
        let normalized = io::Error::new(io::ErrorKind::NotConnected, raw_error);
        let source = anyhow::Error::new(normalized).context("read embedding control length");
        let (stream, _) = MemoryStream::new(Vec::new(), true);

        let error = map_bounded_exchange_error(source, &stream);
        let retry = embedding_retry_state(&error).expect("typed connection loss");

        assert_eq!(retry.code, "embedding_server_connection_lost");
        assert_eq!(retry.retry_class, "same_rpc_once");
        assert!(retry.message.contains("raw_os_error=233"));
        assert!(retry.message.contains("peer_pid=42"));
        assert!(retry.message.contains("peer_process_start_id=server-start"));
        assert!(retry.message.contains("peer_state=running"));
        assert!(retry.message.contains("peer_exit_code=none"));
        assert!(
            retry
                .message
                .contains("source=read embedding control length")
        );
        assert_eq!(exchange_raw_os_error(&error), Some(233));
    }

    #[test]
    fn bounded_exchange_does_not_type_unrelated_io_errors() {
        let source = anyhow::Error::new(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unrelated denial",
        ))
        .context("read embedding control length");
        let (stream, _) = MemoryStream::new(Vec::new(), false);

        let error = map_bounded_exchange_error(source, &stream);

        assert!(embedding_retry_state(&error).is_none());
        assert_eq!(error.to_string(), "read embedding control length");
    }

    #[test]
    fn bounded_exchange_reprobes_exit_code_after_liveness_observes_exit() {
        let raw_error = io::Error::from_raw_os_error(233);
        let normalized = io::Error::new(io::ErrorKind::NotConnected, raw_error);
        let source = anyhow::Error::new(normalized).context("read embedding control length");
        let (mut stream, _) = MemoryStream::new(Vec::new(), false);
        stream.exit_codes = Mutex::new(vec![None, Some(0xc000_0005)]);

        let error = map_bounded_exchange_error(source, &stream);
        let retry = embedding_retry_state(&error).expect("typed connection loss");

        assert!(retry.message.contains("peer_state=exited"));
        assert!(retry.message.contains("peer_exit_code=3221225477"));
    }

    #[test]
    fn held_lease_reader_survives_repeated_timeouts_then_decodes() {
        let frame = encode_test_frame(
            &request(
                "lease-snapshot",
                EmbeddingCompatibility::current(true),
                EmbeddingOperation::Snapshot,
            ),
            &[],
        );
        let (inner, _) = MemoryStream::new(frame, true);
        let mut stream = PollingStream {
            inner,
            pending_reads: 4,
        };
        let mut reader = IncrementalProtocolFrameReader::default();
        for _ in 0..4 {
            assert!(matches!(
                reader
                    .poll::<EmbeddingProtocolRequest>(&mut stream)
                    .expect("bounded poll"),
                ProtocolFramePoll::Pending
            ));
        }
        assert!(matches!(
            reader
                .poll::<EmbeddingProtocolRequest>(&mut stream)
                .expect("eventual frame"),
            ProtocolFramePoll::Ready((
                EmbeddingProtocolRequest {
                    operation: EmbeddingOperation::Snapshot,
                    ..
                },
                _
            ))
        ));
    }

    #[test]
    fn lease_and_server_identity_drift_fail_closed() {
        let snapshot = test_snapshot();
        let identity = test_engine_identity();
        let mut lease = EmbeddingEngineLeaseIdentity {
            lease_token: "lease".into(),
            server_instance_id: snapshot.process.server_instance_id.clone(),
            load_generation: identity.load_generation,
            compatibility_sha256: "compat".into(),
        };
        assert!(validate_lease_server_identity(&lease, &identity, &snapshot).is_ok());
        lease.load_generation += 1;
        assert!(validate_lease_server_identity(&lease, &identity, &snapshot).is_err());
        let mut changed = snapshot.clone();
        changed.process.server_instance_id = "other".into();
        assert!(validate_same_server(&changed, &snapshot).is_err());
    }

    #[test]
    fn lease_end_restarts_the_full_true_idle_window_before_native_release() {
        struct LeaseDropProbe {
            state: Arc<PerUserEmbeddingServerState>,
            observed_idle_start: Arc<AtomicU64>,
        }

        impl Drop for LeaseDropProbe {
            fn drop(&mut self) {
                self.observed_idle_start.store(
                    self.state.last_work_ended_ns.load(Ordering::Acquire),
                    Ordering::Release,
                );
            }
        }

        let state = test_server_state();
        let observed_idle_start = Arc::new(AtomicU64::new(0));
        let lease = ServerLeaseActivity::new(
            &state,
            LeaseDropProbe {
                state: Arc::clone(&state),
                observed_idle_start: Arc::clone(&observed_idle_start),
            },
        );
        state.clock.sleep(Duration::from_secs(75));

        drop(lease);

        let idle_start = state.last_work_ended_ns.load(Ordering::Acquire);
        assert_eq!(idle_start, state.clock.now_ns());
        assert_eq!(
            observed_idle_start.load(Ordering::Acquire),
            idle_start,
            "the idle clock must reset before the wrapped native lease is released"
        );
        state.clock.sleep(Duration::from_millis(
            PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS - 1,
        ));
        assert!(
            elapsed_since(state.clock.as_ref(), idle_start)
                < Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
        );
        state.clock.sleep(Duration::from_millis(1));
        assert_eq!(
            elapsed_since(state.clock.as_ref(), idle_start),
            Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
        );
    }

    #[test]
    fn request_deadline_covers_pre_engine_work_and_cancels_abandoned_context() {
        let clock = TestClock::new();
        let context = EmbeddingRequestContext::new("deadline", "scope", 0);
        let deadline = ServerRequestDeadline::start(clock.as_ref(), 10);

        clock.sleep(Duration::from_millis(9));
        assert!(!deadline.cancel_if_elapsed(clock.as_ref(), &context));
        assert!(!context.is_cancelled());

        // This elapsed time represents admission plus cold engine
        // initialization before a native request handle exists.
        clock.sleep(Duration::from_millis(1));
        assert!(deadline.cancel_if_elapsed(clock.as_ref(), &context));
        assert!(context.is_cancelled());
    }

    #[test]
    fn idle_admission_closes_before_a_new_request_can_enter() {
        let state = test_server_state();
        assert!(state.begin_draining_if_idle());
        let context = EmbeddingRequestContext::new("late", "scope", 0);
        let admission = state
            .try_admit_request(EmbeddingRequestClass::Query, 0)
            .expect("front admission remains independently bounded");
        assert!(
            state
                .begin_request(
                    "connection",
                    "late",
                    "scope",
                    EmbeddingRequestClass::Query,
                    "queued",
                    context,
                    admission,
                    None,
                )
                .is_err()
        );
        assert!(state.engine.lock().expect("engine state").is_none());
    }

    #[test]
    fn dead_authenticated_peer_cancels_queued_context() {
        let (stream, _) = MemoryStream::new(Vec::new(), false);
        let context = EmbeddingRequestContext::new("dead", "scope", 0);
        assert!(cancel_if_peer_dead(&stream, &context).expect("liveness probe"));
        assert!(context.is_cancelled());
    }

    #[test]
    fn observe_intent_rejects_activation_without_initializing_or_resetting_idle() {
        let compatibility = EmbeddingCompatibility::current(true);
        let hello = request(
            "hello",
            compatibility.clone(),
            test_hello_operation("observe"),
        );
        let activate = request(
            "activate",
            compatibility,
            EmbeddingOperation::EnsureResident {
                scope_id: "scope".into(),
                deadline_ms: 100,
                retry_after_ms: 1,
            },
        );
        let mut input = encode_test_frame(&hello, &[]);
        input.extend_from_slice(&encode_test_frame(&activate, &[]));
        let fixture = MemoryStream::with_delivery_tracking(input, true);
        let state = test_server_state();
        let idle_before = state.last_work_ended_ns.load(Ordering::Acquire);
        serve_embedding_connection(Arc::clone(&state), Box::new(fixture.stream))
            .expect("observe rejection is correlated");
        assert_eq!(
            fixture.finished_deliveries.load(Ordering::Acquire),
            1,
            "a correlated final response must finish transport delivery before teardown"
        );
        assert_eq!(
            fixture
                .read_timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last()
                .copied()
                .flatten(),
            Some(EmbeddingClientBudgets::current().query_request),
            "final delivery must replace the peer-selected timeout with the server-owned cap"
        );
        assert!(state.engine.lock().expect("engine state").is_none());
        assert_eq!(
            state.last_work_ended_ns.load(Ordering::Acquire),
            idle_before
        );
        let bytes = fixture
            .output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (mut output_stream, _) = MemoryStream::new(bytes, true);
        let _: (EmbeddingProtocolResponse, Vec<u8>) =
            read_frame(&mut output_stream).expect("hello response");
        let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
            read_frame(&mut output_stream).expect("observe rejection");
        assert_eq!(
            response.error.expect("terminal error").code,
            "embedding_server_observe_operation_forbidden"
        );
    }

    #[test]
    fn incompatible_observe_reports_without_draining_or_resetting_idle() {
        let mut compatibility = EmbeddingCompatibility::current(true);
        compatibility.config_sha256 = "incompatible-observer".into();
        let hello = request("hello", compatibility, test_hello_operation("observe"));
        let (stream, output) = MemoryStream::new(encode_test_frame(&hello, &[]), true);
        let state = test_server_state();
        let idle_before = state.last_work_ended_ns.load(Ordering::Acquire);
        let event_before = state.event_sequence.load(Ordering::Acquire);

        serve_embedding_connection(Arc::clone(&state), Box::new(stream))
            .expect("incompatible observation is correlated");

        assert!(!state.draining.load(Ordering::Acquire));
        assert!(state.engine.lock().expect("engine state").is_none());
        assert_eq!(
            state.last_work_ended_ns.load(Ordering::Acquire),
            idle_before
        );
        assert_eq!(state.event_sequence.load(Ordering::Acquire), event_before);
        let bytes = output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (mut output_stream, _) = MemoryStream::new(bytes, true);
        let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
            read_frame(&mut output_stream).expect("incompatible response");
        assert!(response.result.is_none());
        let error = response.error.expect("terminal incompatibility");
        assert_eq!(error.code, "embedding_server_incompatible_active_owner");
    }

    #[test]
    fn same_user_hello_executable_mismatch_uses_typed_upgrade_handshake() {
        let observed = test_server_state();
        let observe_error = serve_mismatched_peer_hello(&observed, "observe");
        assert_eq!(
            observe_error.code,
            "embedding_server_incompatible_active_owner"
        );
        assert!(!observed.draining.load(Ordering::Acquire));

        let active = test_server_state();
        active.active.lock().expect("active state").insert(
            "existing:request".into(),
            ActiveServerRequest {
                request_id: "request".into(),
                scope_id: "scope".into(),
                request_class: EmbeddingRequestClass::Query,
                phase: "native_execution".into(),
                started_ns: active.clock.now_ns(),
            },
        );
        let active_error = serve_mismatched_peer_hello(&active, "activate");
        assert_eq!(
            active_error.code,
            "embedding_server_incompatible_active_owner"
        );
        assert!(!active.draining.load(Ordering::Acquire));

        let idle = test_server_state();
        let idle_error = serve_mismatched_peer_hello(&idle, "activate");
        assert_eq!(idle_error.code, "embedding_server_draining");
        assert!(idle.draining.load(Ordering::Acquire));
    }

    #[test]
    fn cold_initialization_admission_is_bounded_per_class_and_cancel_reclaims_capacity() {
        let state = test_server_state();
        let _cold_initialization = state.engine.lock().expect("hold cold engine state");
        let mut query_guards = Vec::new();
        let mut bulk_guards = Vec::new();

        for index in 0..EMBEDDING_QUERY_QUEUE_CAPACITY {
            let parsed_request = state
                .try_begin_pre_request()
                .expect("bounded request parser slot");
            drop(parsed_request);
            query_guards.push(begin_test_request(
                &state,
                EmbeddingRequestClass::Query,
                &format!("query-{index}"),
            ));
        }
        for index in 0..EMBEDDING_BULK_QUEUE_CAPACITY {
            let parsed_request = state
                .try_begin_pre_request()
                .expect("bounded request parser slot");
            drop(parsed_request);
            bulk_guards.push(begin_test_request(
                &state,
                EmbeddingRequestClass::Bulk,
                &format!("bulk-{index}"),
            ));
        }

        let query_error = state
            .try_admit_request(EmbeddingRequestClass::Query, 17)
            .expect_err("the 65th cold query must receive typed capacity");
        let bulk_error = state
            .try_admit_request(EmbeddingRequestClass::Bulk, 19)
            .expect_err("the 65th cold bulk request must receive typed capacity");
        for (error, class, retry_after_ms) in [(query_error, "query", 17), (bulk_error, "bulk", 19)]
        {
            assert_eq!(error.code, "embedding_capacity");
            let pressure = error.capacity.expect("typed capacity details");
            assert_eq!(pressure.reason, "queue_full");
            assert_eq!(pressure.queue_class, class);
            assert_eq!(pressure.capacity, 64);
            assert_eq!(pressure.depth, 64);
            assert_eq!(pressure.owner_state, "waking");
            assert_eq!(pressure.retry_after_ms, retry_after_ms);
        }
        assert_eq!(
            state.request_admission.snapshot(),
            ServerRequestAdmissionDepths {
                query: EMBEDDING_QUERY_QUEUE_CAPACITY,
                bulk: EMBEDDING_BULK_QUEUE_CAPACITY,
            }
        );
        assert_eq!(
            state.active.lock().expect("active state").len(),
            EMBEDDING_QUERY_QUEUE_CAPACITY + EMBEDDING_BULK_QUEUE_CAPACITY
        );

        assert!(!state.cancel(
            "query-0",
            "00000000-0000-0000-0000-000000000000",
            test_executable().pid,
            &test_executable().process_start_id,
        ));
        assert!(!state.cancel(
            "query-0",
            &test_cancel_token(),
            test_executable().pid + 1,
            &test_executable().process_start_id,
        ));
        assert!(state.cancel(
            "query-0",
            &test_cancel_token(),
            test_executable().pid,
            &test_executable().process_start_id,
        ));
        assert_eq!(
            state.request_admission.snapshot().query,
            EMBEDDING_QUERY_QUEUE_CAPACITY - 1
        );
        let replacement = state
            .try_admit_request(EmbeddingRequestClass::Query, 23)
            .expect("cancellation immediately reclaims the class permit");
        drop(replacement);
        drop(query_guards.remove(0));
        assert_eq!(
            state.active.lock().expect("active state").len(),
            EMBEDDING_QUERY_QUEUE_CAPACITY + EMBEDDING_BULK_QUEUE_CAPACITY - 1
        );

        drop(query_guards);
        drop(bulk_guards);
        assert_eq!(
            state.request_admission.snapshot(),
            ServerRequestAdmissionDepths::default()
        );
        assert!(state.active.lock().expect("active state").is_empty());
    }

    #[test]
    fn front_admission_reserves_the_documented_queue_behind_one_active_request() {
        let admission = Arc::new(ServerRequestAdmission::default());
        let permits = (0..=EMBEDDING_BULK_QUEUE_CAPACITY)
            .map(|_| {
                admission
                    .try_acquire(EmbeddingRequestClass::Bulk, true)
                    .expect("one active request plus the full queue remains bounded")
            })
            .collect::<Vec<_>>();
        assert_eq!(admission.snapshot().bulk, EMBEDDING_BULK_QUEUE_CAPACITY + 1);
        assert!(
            admission
                .try_acquire(EmbeddingRequestClass::Bulk, true)
                .is_err(),
            "the request after the active slot and full queue must be rejected"
        );
        drop(permits);
        assert_eq!(
            admission.snapshot(),
            ServerRequestAdmissionDepths::default()
        );
    }

    #[test]
    fn hostile_idle_connections_are_bounded_and_product_rejection_is_correlated() {
        let state = test_server_state();
        let idle_before = state.last_work_ended_ns.load(Ordering::Acquire);
        let mut permits = (0..SERVER_CONNECTION_HANDLER_CAPACITY)
            .map(|_| {
                state
                    .try_begin_connection()
                    .expect("connection permit within hard bound")
            })
            .collect::<Vec<_>>();
        assert!(state.try_begin_connection().is_none());
        let idle_permits = (0..SERVER_CONTROL_CONNECTION_RESERVE)
            .map(|_| {
                state
                    .try_begin_pre_request()
                    .expect("idle handshake within the pre-request bound")
            })
            .collect::<Vec<_>>();
        assert!(
            state.try_begin_pre_request().is_none(),
            "at most eight connections may remain between Hello and a classified request"
        );
        assert!(
            state.true_idle(),
            "idle handshakes must not extend the native owner's true-idle lifetime"
        );
        assert_eq!(
            state.last_work_ended_ns.load(Ordering::Acquire),
            idle_before
        );

        let product_hello = request(
            "product-pre-request-capacity",
            EmbeddingCompatibility::current(true),
            test_hello_operation("activate"),
        );
        let (stream, output) = MemoryStream::new(encode_test_frame(&product_hello, &[]), true);
        serve_embedding_connection(Arc::clone(&state), Box::new(stream))
            .expect("pre-request rejection is correlated");
        let bytes = output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (mut output_stream, _) = MemoryStream::new(bytes, true);
        let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
            read_frame(&mut output_stream).expect("typed pre-request rejection");
        let pressure = response
            .error
            .and_then(|error| error.capacity)
            .expect("pre-request pressure");
        assert_eq!(pressure.reason, "pre_request_full");
        assert_eq!(pressure.capacity, SERVER_CONTROL_CONNECTION_RESERVE as u64);

        let rejection_guard = state
            .try_begin_rejection_connection()
            .expect("dedicated rejection reserve remains available");
        let hello = request(
            "product-hello",
            EmbeddingCompatibility::current(true),
            test_hello_operation("activate"),
        );
        let (stream, output) = MemoryStream::new(encode_test_frame(&hello, &[]), true);
        serve_embedding_connection_at_handler_capacity(Arc::clone(&state), Box::new(stream))
            .expect("hard-cap rejection is correlated");
        let bytes = output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (mut output_stream, _) = MemoryStream::new(bytes, true);
        let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
            read_frame(&mut output_stream).expect("typed product rejection");
        let error = response.error.expect("capacity response");
        assert_eq!(error.code, "embedding_capacity");
        let pressure = error.capacity.expect("connection pressure");
        assert_eq!(pressure.reason, "connection_handler_full");
        assert_eq!(pressure.queue_class, "connection");
        assert_eq!(pressure.capacity, SERVER_CONNECTION_HANDLER_CAPACITY as u64);
        assert!(pressure.depth >= pressure.capacity);

        assert_eq!(
            state.connections.load(Ordering::Acquire),
            SERVER_CONNECTION_HANDLER_CAPACITY + 1
        );
        drop(rejection_guard);
        drop(idle_permits);
        drop(permits.pop());
        let replacement = state
            .try_begin_connection()
            .expect("dropped handler permit is immediately reusable");
        drop(replacement);
        drop(permits);
        assert_eq!(state.connections.load(Ordering::Acquire), 0);
    }

    #[test]
    fn finished_connection_handlers_are_reaped_under_high_churn() {
        let mut retained = Vec::new();
        for _ in 0..256 {
            retained.push(thread::spawn(|| {}));
            while retained
                .last()
                .is_some_and(|connection| !connection.is_finished())
            {
                thread::yield_now();
            }
            reap_finished_connection_handlers(&mut retained);
            assert!(
                retained.is_empty(),
                "completed JoinHandles must not accumulate between accepts"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn qualification_gate_rejects_broad_or_linked_filesystem_surfaces() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temporary = tempfile::tempdir().expect("temporary qualification root");
        let directory = temporary.path().join("qualification");
        fs::create_dir(&directory).expect("qualification directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
            .expect("set broad directory mode");
        let canonical = fs::canonicalize(&directory).expect("canonical qualification directory");
        let broad_error = server_qualification_control_from_values(
            Some(canonical.clone().into_os_string()),
            Some("test-nonce".into()),
        )
        .expect_err("group- or world-accessible qualification directories are rejected");
        assert!(
            broad_error
                .to_string()
                .contains("embedding_qualification_directory_untrusted")
        );

        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("restore private directory mode");
        let linked_directory = temporary.path().join("linked-qualification");
        symlink(&canonical, &linked_directory).expect("link qualification directory");
        let linked_error = server_qualification_control_from_values(
            Some(linked_directory.into_os_string()),
            Some("test-nonce".into()),
        )
        .expect_err("linked qualification directories are rejected");
        assert!(
            linked_error
                .to_string()
                .contains("embedding_qualification_directory_untrusted")
        );

        let event_target = temporary.path().join("event-target");
        fs::write(&event_target, b"").expect("event target");
        fs::set_permissions(&event_target, fs::Permissions::from_mode(0o600))
            .expect("private event target");
        symlink(&event_target, canonical.join("test-nonce.events.jsonl")).expect("link event log");
        let event_error = server_qualification_control_from_values(
            Some(canonical.into_os_string()),
            Some("test-nonce".into()),
        )
        .expect_err("linked event logs are rejected");
        assert!(
            event_error
                .to_string()
                .contains("embedding_qualification_file_untrusted")
        );
    }

    #[cfg(unix)]
    #[test]
    fn qualification_gate_bounds_and_pins_commands_and_events() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let (temporary, control) = test_qualification_control();
        let command_path = control
            .directory
            .join(format!("{}.command.json", control.nonce));
        let command_target = temporary.path().join("command-target");
        fs::write(&command_target, b"{}").expect("command target");
        fs::set_permissions(&command_target, fs::Permissions::from_mode(0o600))
            .expect("private command target");
        symlink(&command_target, &command_path).expect("link command");
        assert!(
            read_server_qualification_command(&control)
                .expect_err("linked commands are rejected")
                .to_string()
                .contains("embedding_qualification_file_untrusted")
        );

        fs::remove_file(&command_path).expect("remove command link");
        fs::write(
            &command_path,
            vec![b'x'; SERVER_QUALIFICATION_MAX_COMMAND_BYTES as usize + 1],
        )
        .expect("oversized command");
        fs::set_permissions(&command_path, fs::Permissions::from_mode(0o600))
            .expect("private oversized command");
        assert!(
            read_server_qualification_command(&control)
                .expect_err("oversized commands are rejected")
                .to_string()
                .contains("embedding_qualification_file_untrusted")
        );

        fs::write(&command_path, b"{}").expect("bounded command");
        fs::set_permissions(&command_path, fs::Permissions::from_mode(0o600))
            .expect("private bounded command");
        let command = read_server_qualification_command(&control)
            .expect("read bounded command")
            .expect("command exists");
        let command_sha256 = hex_sha256(&command.bytes);
        control.mark_command_processed(command_sha256.clone());
        assert!(control.command_was_processed(&command_sha256));
        assert!(
            command_path.exists(),
            "the server leaves qualification command cleanup to its writer"
        );

        fs::remove_file(&command_path).expect("remove read command");
        fs::write(&command_path, b"{\"replacement\":true}").expect("replacement command");
        fs::set_permissions(&command_path, fs::Permissions::from_mode(0o600))
            .expect("private replacement command");
        let replacement = read_server_qualification_command(&control)
            .expect("read replacement command")
            .expect("replacement command exists");
        let replacement_sha256 = hex_sha256(&replacement.bytes);
        assert!(
            !control.command_was_processed(&replacement_sha256),
            "replacement content is never mistaken for the processed command"
        );
        assert!(
            command_path.exists(),
            "replacement command remains untouched"
        );

        let mut events = control.events.lock().expect("event log");
        events.records = SERVER_QUALIFICATION_MAX_EVENT_RECORDS;
        assert!(
            events
                .record(&control.directory, &test_qualification_event())
                .expect_err("event record count is bounded")
                .to_string()
                .contains("embedding_qualification_event_log_limit")
        );
        events.records = 0;
        events
            .file
            .set_len(SERVER_QUALIFICATION_MAX_EVENT_BYTES)
            .expect("expand event log to byte limit");
        events.bytes = SERVER_QUALIFICATION_MAX_EVENT_BYTES;
        assert!(
            events
                .record(&control.directory, &test_qualification_event())
                .expect_err("event bytes are bounded")
                .to_string()
                .contains("embedding_qualification_event_log_limit")
        );
        events.file.set_len(0).expect("reset event log");
        events.bytes = 0;
        let moved_event_path = events.path.with_extension("moved");
        fs::rename(&events.path, &moved_event_path).expect("move pinned event log");
        fs::write(&events.path, b"").expect("replacement event log");
        fs::set_permissions(&events.path, fs::Permissions::from_mode(0o600))
            .expect("private replacement event log");
        assert!(
            events
                .record(&control.directory, &test_qualification_event())
                .expect_err("replacement event logs are rejected")
                .to_string()
                .contains("embedding_qualification_event_log_replaced")
        );
        drop(events);

        let original_directory = control.directory.path.clone();
        let moved_directory = temporary.path().join("moved-qualification");
        fs::rename(&original_directory, &moved_directory).expect("move pinned directory");
        fs::create_dir(&original_directory).expect("replacement directory");
        fs::set_permissions(&original_directory, fs::Permissions::from_mode(0o700))
            .expect("private replacement directory");
        assert!(
            control
                .directory
                .revalidate()
                .expect_err("replacement directories are rejected")
                .to_string()
                .contains("embedding_qualification_directory_replaced")
        );
    }

    #[cfg(windows)]
    #[test]
    fn qualification_event_log_rejects_a_replaced_windows_path() {
        let (_temporary, control) = test_qualification_control();
        let mut events = control.events.lock().expect("event log");
        let moved_event_path = events.path.with_extension("moved");
        fs::rename(&events.path, &moved_event_path).expect("move pinned event log");
        fs::write(&events.path, b"").expect("replacement event log");

        assert!(
            events
                .record(&control.directory, &test_qualification_event())
                .expect_err("replacement event logs are rejected")
                .to_string()
                .contains("embedding_qualification_event_log_replaced")
        );
    }

    #[cfg(windows)]
    #[test]
    fn qualification_gate_accepts_native_identical_windows_path_spellings() {
        let temporary = tempfile::tempdir().expect("temporary qualification root");
        let directory = temporary.path().join("qualification");
        fs::create_dir(&directory).expect("qualification directory");
        let canonical = fs::canonicalize(&directory).expect("canonical qualification directory");
        assert_ne!(
            directory, canonical,
            "Windows canonicalization should expose the verbatim spelling mismatch"
        );
        assert_eq!(
            native_path_identity(&directory).expect("caller directory identity"),
            native_path_identity(&canonical).expect("canonical directory identity")
        );

        let control = server_qualification_control_from_values(
            Some(directory.into_os_string()),
            Some("test-nonce".into()),
        )
        .expect("native-identical Windows spellings are trusted")
        .expect("qualification control is enabled");

        assert_eq!(control.directory.path, canonical);
        control
            .directory
            .revalidate()
            .expect("canonical directory remains pinned");
    }

    #[cfg(unix)]
    #[test]
    fn qualification_restart_restores_the_last_durable_command_sequence() {
        let (_temporary, control) = test_qualification_control();
        let directory = control.directory.path.clone();
        let mut event = test_qualification_event();
        event.sequence = 7;
        control
            .events
            .lock()
            .expect("event log")
            .record(&control.directory, &event)
            .expect("durable qualification event");
        drop(control);

        let restarted = server_qualification_control_from_values(
            Some(directory.into_os_string()),
            Some("test-nonce".into()),
        )
        .expect("reopen qualification control")
        .expect("qualification control remains enabled");
        assert_eq!(restarted.last_sequence.load(Ordering::Acquire), 7);
    }

    #[test]
    fn watchdog_progress_isolated_by_request_class() {
        let clock = TestClock::new();
        let timeout = Duration::from_millis(5);
        let mut query = WatchdogClassProgress::new(clock.now_ns());
        let mut bulk = WatchdogClassProgress::new(clock.now_ns());

        clock.sleep(Duration::from_millis(3));
        assert!(query.observe(true, 1, clock.as_ref(), timeout).is_none());
        assert!(bulk.observe(true, 0, clock.as_ref(), timeout).is_none());
        clock.sleep(Duration::from_millis(6));
        assert!(query.observe(true, 2, clock.as_ref(), timeout).is_none());
        let stalled = bulk
            .observe(true, 0, clock.as_ref(), timeout)
            .expect("query progress must not mask a stalled bulk class");

        assert_eq!(stalled.sequence, 0);
        assert_eq!(stalled.last_progress_ns, 3_000_001);
    }

    #[test]
    fn watchdog_class_activation_starts_a_fresh_deadline() {
        let clock = TestClock::new();
        let timeout = Duration::from_millis(5);
        let mut progress = WatchdogClassProgress::new(clock.now_ns());

        clock.sleep(Duration::from_millis(20));
        assert!(
            progress
                .observe(false, 0, clock.as_ref(), timeout)
                .is_none()
        );
        clock.sleep(Duration::from_millis(20));
        assert!(
            progress.observe(true, 0, clock.as_ref(), timeout).is_none(),
            "inactive time must not be charged to a newly active class"
        );
        clock.sleep(timeout);
        assert!(progress.observe(true, 0, clock.as_ref(), timeout).is_some());
    }

    #[test]
    fn inactive_watchdog_class_never_trips() {
        let clock = TestClock::new();
        let timeout = Duration::from_millis(1);
        let mut progress = WatchdogClassProgress::new(clock.now_ns());

        for sequence in 0..4 {
            clock.sleep(Duration::from_millis(10));
            assert!(
                progress
                    .observe(false, sequence, clock.as_ref(), timeout)
                    .is_none()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn watchdog_marker_is_private_durable_and_never_reuses_stale_evidence() {
        use std::os::unix::fs::MetadataExt;

        let (temporary, control) = test_qualification_control();
        let marker_path = control.directory.join(
            embedding_qualification_watchdog_marker_filename(
                &control.nonce_sha256,
                &test_snapshot().process.server_instance_id,
            )
            .expect("marker filename"),
        );
        let state = test_server_state();
        publish_watchdog_fail_stop_marker(
            &control,
            &state,
            EmbeddingServerBudgets {
                idle_timeout: Duration::from_secs(60),
                native_no_progress: Duration::from_millis(4),
                watchdog_poll: Duration::from_millis(1),
            },
            7,
            1,
        )
        .expect("publish marker");
        let metadata = fs::symlink_metadata(&marker_path).expect("marker metadata");
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        assert_eq!(metadata.mode() & 0o077, 0);
        let marker: EmbeddingQualificationWatchdogMarker =
            serde_json::from_slice(&fs::read(&marker_path).expect("read marker"))
                .expect("parse marker");
        assert_eq!(marker.reason, "embedding_engine_stalled");
        assert_eq!(marker.nonce_sha256, control.nonce_sha256);
        assert_eq!(marker.progress_sequence, 7);
        assert!(
            publish_watchdog_fail_stop_marker(
                &control,
                &state,
                EmbeddingServerBudgets {
                    idle_timeout: Duration::from_secs(60),
                    native_no_progress: Duration::from_millis(4),
                    watchdog_poll: Duration::from_millis(1),
                },
                8,
                1,
            )
            .expect_err("stale marker is rejected")
            .to_string()
            .contains("embedding_qualification_watchdog_marker_exists")
        );
        drop(temporary);
    }

    #[test]
    fn shutdown_with_stuck_initialization_keeps_watchdog_fail_stop_armed() {
        let state = test_server_state();
        state.active.lock().expect("active state").insert(
            "connection:request".into(),
            ActiveServerRequest {
                request_id: "request".into(),
                scope_id: "scope".into(),
                request_class: EmbeddingRequestClass::Bulk,
                phase: "native_execution".into(),
                started_ns: state.clock.now_ns(),
            },
        );
        state.draining.store(true, Ordering::Release);
        let transport = Arc::new(WatchdogTransport {
            clock: TestClock::new(),
            fail_stops: AtomicUsize::new(0),
        });
        let _engine_lock = state.engine.lock().expect("simulate stuck initializer");
        let watchdog = spawn_server_watchdog(
            Arc::clone(&state),
            transport.clone(),
            EmbeddingServerBudgets {
                idle_timeout: Duration::from_secs(60),
                native_no_progress: Duration::from_millis(2),
                watchdog_poll: Duration::from_millis(1),
            },
        )
        .expect("watchdog");
        watchdog.join().expect("watchdog completion");
        assert_eq!(transport.fail_stops.load(Ordering::Acquire), 1);
        assert!(state.stopped.load(Ordering::Acquire));
    }

    #[test]
    fn background_engine_cleanup_marks_normal_shutdown_complete() {
        let state = test_server_state();
        state.draining.store(true, Ordering::Release);
        let state_for_cleanup = Arc::clone(&state);
        let cleanup = thread::spawn(move || {
            state_for_cleanup.shutdown_engine();
            state_for_cleanup.stopped.store(true, Ordering::Release);
        });

        cleanup.join().expect("cleanup completion");

        assert!(state.stopped.load(Ordering::Acquire));
    }

    fn test_client<T>(transport: Arc<T>) -> PerUserEmbeddingClient
    where
        T: EmbeddingClientTransport + 'static,
    {
        PerUserEmbeddingClient {
            transport,
            compatibility: EmbeddingCompatibility::current(true),
            scope_id: "test-scope".into(),
        }
    }

    fn test_server_state() -> Arc<PerUserEmbeddingServerState> {
        let clock = TestClock::new();
        Arc::new(PerUserEmbeddingServerState {
            clock,
            engine_cache_root: PathBuf::from("test-cache"),
            engine_config: native_engine_config(true).expect("CPU engine config"),
            engine: Mutex::new(None),
            process: test_snapshot().process,
            protocol: EmbeddingServerProtocolSnapshot::current(),
            authority: test_snapshot().authority,
            connections: AtomicUsize::new(0),
            pre_request_connections: AtomicUsize::new(0),
            admission_gate: Mutex::new(()),
            request_admission: Arc::new(ServerRequestAdmission::default()),
            active: Mutex::new(std::collections::BTreeMap::new()),
            cancellations: Mutex::new(std::collections::BTreeMap::new()),
            draining: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            last_work_ended_ns: AtomicU64::new(1),
            event_sequence: AtomicU64::new(1),
            last_failure: Mutex::new(None),
            qualification: None,
        })
    }

    #[cfg(any(unix, windows))]
    fn test_qualification_control() -> (tempfile::TempDir, ServerQualificationControl) {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temporary qualification root");
        let directory = temporary.path().join("qualification");
        fs::create_dir(&directory).expect("qualification directory");
        #[cfg(unix)]
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("private qualification directory");
        let canonical = fs::canonicalize(&directory).expect("canonical qualification directory");
        let control = server_qualification_control_from_values(
            Some(canonical.into_os_string()),
            Some("test-nonce".into()),
        )
        .expect("valid qualification control")
        .expect("qualification control is enabled");
        (temporary, control)
    }

    #[cfg(any(unix, windows))]
    fn test_qualification_event() -> ServerQualificationEvent {
        ServerQualificationEvent {
            schema_version: 1,
            sequence: 1,
            action: "snapshot".into(),
            status: "completed".into(),
            server_event_sequence: 1,
            clock: ServerQualificationEventClock {
                domain: "awake_monotonic".into(),
                api: "test".into(),
                boot_id: "test-boot".into(),
                observed_ns: 1,
            },
            snapshot: None,
            details: None,
        }
    }

    fn begin_test_request(
        state: &Arc<PerUserEmbeddingServerState>,
        request_class: EmbeddingRequestClass,
        request_id: &str,
    ) -> ServerRequestGuard {
        let admission = state
            .try_admit_request(request_class, 11)
            .expect("request is within the class bound");
        state
            .begin_request(
                &format!("connection-{request_id}"),
                request_id,
                &format!("scope-{request_id}"),
                request_class,
                "queued",
                EmbeddingRequestContext::new(request_id, format!("scope-{request_id}"), 11),
                admission,
                Some(ServerCancellationAuth {
                    token: test_cancel_token(),
                    client_pid: test_executable().pid,
                    client_process_start_id: test_executable().process_start_id,
                }),
            )
            .expect("admitted request enters bounded active state")
    }

    fn test_cancel_token() -> String {
        "b9236f3d-c1f4-4af0-8c73-6d6574c40c5e".into()
    }

    fn serve_mismatched_peer_hello(
        state: &Arc<PerUserEmbeddingServerState>,
        intent: &str,
    ) -> EmbeddingProtocolError {
        let mut operation = test_hello_operation(intent);
        let EmbeddingOperation::Hello {
            client_executable_sha256,
            ..
        } = &mut operation
        else {
            unreachable!("test helper always builds hello");
        };
        *client_executable_sha256 = "b".repeat(64);
        let hello = request(
            "upgrade-hello",
            EmbeddingCompatibility::current(true),
            operation,
        );
        let (stream, output) = MemoryStream::new(encode_test_frame(&hello, &[]), true);
        serve_embedding_connection(Arc::clone(state), Box::new(stream))
            .expect("upgrade incompatibility is correlated");
        let bytes = output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (mut output_stream, _) = MemoryStream::new(bytes, true);
        let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
            read_frame(&mut output_stream).expect("upgrade response");
        response.error.expect("typed upgrade incompatibility")
    }

    fn test_executable() -> EmbeddingExecutableIdentity {
        EmbeddingExecutableIdentity {
            pid: 42,
            process_start_id: "server-start".into(),
            executable_sha256: "a".repeat(64),
            executable_version: "0.16.0".into(),
        }
    }

    fn test_hello_operation(intent: &str) -> EmbeddingOperation {
        EmbeddingOperation::Hello {
            intent: intent.into(),
            client_pid: 42,
            client_process_start_id: "server-start".into(),
            client_executable_sha256: "a".repeat(64),
            client_executable_version: "0.16.0".into(),
        }
    }

    fn test_transport_identity() -> EmbeddingTransportIdentity {
        EmbeddingTransportIdentity {
            endpoint_namespace_id: "endpoint".into(),
            lifetime_authority_id: "authority".into(),
            listener_id: "listener".into(),
            peer_verified: true,
            peer_pid: Some(42),
            peer_process_start_id: Some("server-start".into()),
        }
    }

    fn test_snapshot() -> EmbeddingServerSnapshot {
        EmbeddingServerSnapshot {
            schema_version: PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
            event_sequence: 1,
            lifecycle: "listening".into(),
            clock: TestClock::new().snapshot(),
            protocol: EmbeddingServerProtocolSnapshot::current(),
            authority: EmbeddingServerAuthoritySnapshot {
                endpoint_namespace_id: "endpoint".into(),
                lifetime_authority_id: "authority".into(),
                listener_id: "listener".into(),
                peer_verified: true,
            },
            process: EmbeddingServerProcessSnapshot {
                server_instance_id: "server".into(),
                pid: 42,
                process_start_id: "server-start".into(),
                executable_sha256: "a".repeat(64),
                executable_version: "0.16.0".into(),
            },
            scheduler: EmbeddingServerSchedulerSnapshot {
                query_capacity: 64,
                query_depth: 0,
                bulk_capacity: 64,
                bulk_depth: 0,
                connection_count: 1,
                active_request_count: 0,
                lease_count: 0,
                active_request: None,
            },
            engine: None,
            failure: None,
        }
    }

    fn test_engine_identity() -> EmbeddingEngineIdentity {
        EmbeddingEngineIdentity {
            server_instance_id: "server".into(),
            load_generation: 1,
            model_load_count: 1,
            residency: "resident".into(),
            worker_alive: true,
            load_error: None,
            model_digest: EMBEDDING_MODEL_SHA256.into(),
            ggml_build_identity: codestory_llama_sys::GGML_BUILD_IDENTITY.into(),
            backend: "CPU".into(),
            adapter_name: "CPU".into(),
            adapter_description: "test".into(),
            policy: "cpu_explicit".into(),
            embedded_model: true,
            materialized_model_sha256: EMBEDDING_MODEL_SHA256.into(),
            materialized_reused: true,
            initialization_ms: 1,
            smoke_ms: 1,
            adapter_memory_total: 1024,
            adapter_memory_used_by_load: 512,
            execution_device_names: Vec::new(),
            execution_backend_names: Vec::new(),
            execution_observation_source: "ggml_eval_callback".into(),
            encode_count: 1,
            execution_node_count: 0,
            resident_accelerator_tensor_count: 0,
            resident_accelerator_tensor_bytes: 0,
            model_layer_count: 13,
            offloaded_layer_count: 0,
            accelerator_execution_verified: false,
        }
    }

    fn test_capacity() -> EmbeddingCapacityPressureWire {
        EmbeddingCapacityPressureWire {
            reason: "queue_full".into(),
            queue_class: "query".into(),
            capacity: 64,
            depth: 64,
            retry_after_ms: 10,
            retry_condition: "a live request completes".into(),
            owner_state: "ready".into(),
            active_scope_id: None,
            active_request_id: None,
            active_request_class: None,
        }
    }

    fn encode_test_frame<T: Serialize>(value: &T, payload: &[u8]) -> Vec<u8> {
        let control = serde_json::to_vec(value).expect("test frame JSON");
        let mut frame = Vec::with_capacity(8 + control.len() + payload.len());
        frame.extend_from_slice(&(control.len() as u32).to_be_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&control);
        frame.extend_from_slice(payload);
        frame
    }

    fn decode_test_frame<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
        if bytes.len() < 8 {
            bail!("test frame is incomplete");
        }
        let control_len =
            u32::from_be_bytes(bytes[0..4].try_into().expect("four-byte frame length")) as usize;
        serde_json::from_slice(&bytes[8..8 + control_len]).context("decode test frame")
    }
}
