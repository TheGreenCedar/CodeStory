use crate::config::SidecarRuntimeConfig;
use crate::embedding_contract::RETRIEVAL_EMBEDDING_DIM;
#[cfg(test)]
use crate::embedding_contract::{
    CODERANK_DOCUMENT_PREFIX, CODERANK_QUERY_PREFIX, EMBEDDING_MODEL_SHA256, native_engine_config,
    normalize_and_validate_vectors,
};
use anyhow::{Context, Result, anyhow, bail};
use codestory_llama_sys::{
    EMBEDDING_BULK_QUEUE_CAPACITY, EMBEDDING_QUERY_QUEUE_CAPACITY, EmbeddingRequestClass,
    EmbeddingRequestContext,
};
#[cfg(test)]
use codestory_llama_sys::{
    EmbeddingAdmissionSnapshot, EmbeddingCapacityPressure, EmbeddingCapacityReason,
    EmbeddingEngine, EmbeddingEngineConfig, EmbeddingOwnerState, EngineError,
    EngineLifecycleSnapshot, NativeDeviceClass,
};
use serde::{Deserialize, Serialize};
use std::fmt;
#[cfg(test)]
use std::fs;
use std::io::{self, Read, Write};
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::thread;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use thiserror::Error;
use uuid::Uuid;

mod admission;
mod client;
mod protocol;
mod qualification_control;
mod scheduler;
mod server;

pub use client::{
    PerUserEmbeddingClient, PerUserEmbeddingResidencyLease, install_embedding_client_transport,
};
pub use qualification_control::{
    EmbeddingQualificationAttemptResult, EmbeddingQualificationOperationResult,
    EmbeddingQualificationParameters, EmbeddingQualificationRequest, EmbeddingQualificationResult,
    run_per_user_embedding_qualification,
};
pub use server::{
    EmbeddingServerBudgets, PerUserEmbeddingServerConfig, run_per_user_embedding_server,
};

#[cfg(test)]
use admission::{
    ServerRequestAdmission, ServerRequestAdmissionDepths, ServerRequestAdmissionPermit,
};
use protocol::hex_sha256;
pub use protocol::{
    EmbeddingCapacityPressureWire, EmbeddingCompatibility, EmbeddingEngineIdentity,
    EmbeddingEngineLeaseIdentity, EmbeddingOperation, EmbeddingProtocolError,
    EmbeddingProtocolRequest, EmbeddingProtocolResponse, EmbeddingQualificationWatchdogClock,
    EmbeddingQualificationWatchdogMarker, EmbeddingResult, EmbeddingRetryStateWire,
    EmbeddingServerActiveRequestSnapshot, EmbeddingServerAuthoritySnapshot,
    EmbeddingServerClockSnapshot, EmbeddingServerEngineSnapshot, EmbeddingServerFailureSnapshot,
    EmbeddingServerProcessSnapshot, EmbeddingServerProtocolSnapshot,
    EmbeddingServerSchedulerSnapshot, EmbeddingServerSnapshot,
    PER_USER_EMBEDDING_BOOTSTRAP_VERSION, PER_USER_EMBEDDING_CONSTANT_SET_FROZEN,
    PER_USER_EMBEDDING_CONSTANT_SET_SHA256, PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT,
    PER_USER_EMBEDDING_MAX_INPUT_BYTES, PER_USER_EMBEDDING_MAX_METADATA_BYTES,
    PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES, PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256,
    PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION, PER_USER_EMBEDDING_PROTOCOL_SHA256,
    PER_USER_EMBEDDING_PROTOCOL_V1, PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
    PER_USER_EMBEDDING_SERVER_PROOF_MARKER, PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
    PerUserEmbeddingError, embedding_capacity_pressure,
    embedding_qualification_watchdog_marker_filename, embedding_retry_state,
};
use qualification_control::ServerQualificationControl;
#[cfg(test)]
use qualification_control::{
    ServerQualificationEvent, ServerQualificationEventClock, poll_server_qualification_command,
    read_server_qualification_command, server_qualification_control_from_env,
    server_qualification_control_from_values, write_server_qualification_event,
};
#[cfg(test)]
use scheduler::{
    ActiveServerRequest, active_request_snapshot, scheduler_snapshot, spawn_server_watchdog,
};
#[cfg(test)]
use scheduler::{WatchdogClassProgress, publish_watchdog_fail_stop_marker};
use server::PerUserEmbeddingServerState;
#[cfg(test)]
use server::{
    IncrementalProtocolFrameReader, ProtocolFramePoll, ServerCancellationAuth, ServerLeaseActivity,
    ServerRequestDeadline, ServerRequestGuard, ServerRequestRegistration, cancel_if_peer_dead,
    configure_server_operation_timeout, failure_response, protocol_error,
    reap_finished_connection_handlers, serve_embedding_connection,
    serve_embedding_connection_at_handler_capacity, success_response,
};

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
                .begin_request(ServerRequestRegistration {
                    connection_id: "connection",
                    request_id: "late",
                    scope_id: "scope",
                    request_class: EmbeddingRequestClass::Query,
                    phase: "queued",
                    context,
                    admission,
                    cancellation_auth: None,
                })
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
        let connection_id = format!("connection-{request_id}");
        let scope_id = format!("scope-{request_id}");
        state
            .begin_request(ServerRequestRegistration {
                connection_id: &connection_id,
                request_id,
                scope_id: &scope_id,
                request_class,
                phase: "queued",
                context: EmbeddingRequestContext::new(request_id, &scope_id, 11),
                admission,
                cancellation_auth: Some(ServerCancellationAuth {
                    token: test_cancel_token(),
                    client_pid: test_executable().pid,
                    client_process_start_id: test_executable().process_start_id,
                }),
            })
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
