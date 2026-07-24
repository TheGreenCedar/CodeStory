use codestory_retrieval::{
    EmbeddingCapacityPressureWire, EmbeddingEngineIdentity, EmbeddingProtocolResponse,
    EmbeddingQualificationParameters, EmbeddingQualificationResult, EmbeddingServerSnapshot,
    EmbeddingTransportIdentity,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerRequest {
    pub(super) schema_version: u32,
    pub(super) nonce_sha256: String,
    pub(super) executable_sha256: String,
    pub(super) project: PathBuf,
    pub(super) operation: String,
    pub(super) parameters: EmbeddingQualificationParameters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) start_gate: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) start_gate_timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerOutput {
    pub(super) schema_version: u32,
    pub(super) pid: u32,
    pub(super) process_start_id: String,
    pub(super) executable_sha256: String,
    pub(super) executable_version: String,
    pub(super) project_identity_sha256: String,
    pub(super) clock: codestory_retrieval::EmbeddingServerClockSnapshot,
    pub(super) started_ns: u64,
    pub(super) finished_ns: u64,
    pub(super) inclusive_clock_api: String,
    pub(super) inclusive_started_ns: u64,
    pub(super) inclusive_finished_ns: u64,
    pub(super) boot_id_started: String,
    pub(super) boot_id_finished: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) result: Option<EmbeddingQualificationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) protocol_exchange: Option<WorkerProtocolExchange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) queue_operations: Option<Vec<WorkerQueueOperation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) engine_identity: Option<EmbeddingEngineIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<WorkerError>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerProtocolExchange {
    pub(super) request_id: String,
    pub(super) submitted_ns: u64,
    pub(super) finished_ns: u64,
    pub(super) transport_identity: EmbeddingTransportIdentity,
    pub(super) hello_snapshot: EmbeddingServerSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) final_snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) response: Option<EmbeddingProtocolResponse>,
    pub(super) response_payload_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) terminal_transport_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerError {
    pub(super) code: String,
    pub(super) message_head: String,
    pub(super) retry_class: String,
    pub(super) retry_after_ms: u64,
    pub(super) retry_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) capacity: Option<EmbeddingCapacityPressureWire>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerQueueOperation {
    pub(super) correlation_id: String,
    pub(super) project_identity_sha256: String,
    pub(super) class: String,
    pub(super) ordinal: u32,
    #[serde(default)]
    pub(super) submission_batch: u32,
    pub(super) submitted_ns: u64,
    pub(super) completed_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) native_completion_sequence: Option<u64>,
    pub(super) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<codestory_retrieval::EmbeddingProtocolError>,
    pub(super) response_payload_bytes: u64,
    pub(super) transport_identity: EmbeddingTransportIdentity,
    pub(super) hello_snapshot: EmbeddingServerSnapshot,
}
