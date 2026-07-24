use crate::embedding_contract::RETRIEVAL_EMBEDDING_DIM;
use codestory_llama_sys::{
    EMBEDDING_BULK_QUEUE_CAPACITY, EMBEDDING_QUERY_QUEUE_CAPACITY, EmbeddingRequestClass,
    EmbeddingRequestContext,
};
use std::time::Duration;

mod admission;
mod client;
mod exchange;
mod protocol;
mod qualification_control;
mod qualification_worker;
mod scheduler;
mod server;
mod transport;

pub use client::{
    PerUserEmbeddingClient, PerUserEmbeddingResidencyLease, install_embedding_client_transport,
};
pub use qualification_control::{
    EmbeddingQualificationAttemptResult, EmbeddingQualificationOperationResult,
    EmbeddingQualificationParameters, EmbeddingQualificationRequest, EmbeddingQualificationResult,
    run_per_user_embedding_qualification,
};
pub use qualification_worker::{
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, EmbeddingQualificationWorkerError,
    EmbeddingQualificationWorkerOutput, EmbeddingQualificationWorkerProtocolExchange,
    EmbeddingQualificationWorkerQueueOperation, EmbeddingQualificationWorkerRequest,
};
pub use server::{
    EmbeddingServerBudgets, PerUserEmbeddingServerConfig, run_per_user_embedding_server,
};
pub use transport::{
    AwakeMonotonicClock, EmbeddingClientBudgets, EmbeddingClientTransport, EmbeddingConnectIntent,
    EmbeddingConnectOutcome, EmbeddingExecutableIdentity, EmbeddingServerBindOutcome,
    EmbeddingServerListener, EmbeddingServerStream, EmbeddingServerTransport,
    EmbeddingSpawnAttempt, EmbeddingTransportFailure, EmbeddingTransportIdentity,
};

use exchange::{
    configure_exchange_timeout, decode_vectors, duration_ms, elapsed_since, embedding_scope_id,
    encode_vectors, exchange, hello, is_server_loss, is_sha256, positive_duration_ms, read_frame,
    request, response_result, validate_engine_identity, validate_engine_server_identity,
    validate_lease_server_identity, validate_same_server, validate_server_snapshot, vectors_result,
    write_frame,
};
#[cfg(test)]
use exchange::{exchange_raw_os_error, map_bounded_exchange_error};

#[cfg(test)]
use admission::{ServerRequestAdmission, ServerRequestAdmissionDepths};
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
    ServerQualificationEvent, ServerQualificationEventClock, read_server_qualification_command,
    server_qualification_control_from_values,
};
#[cfg(test)]
use scheduler::{ActiveServerRequest, spawn_server_watchdog};
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

#[cfg(test)]
mod tests;
