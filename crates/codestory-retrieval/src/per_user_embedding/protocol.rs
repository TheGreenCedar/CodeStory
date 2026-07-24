//! Stable per-user embedding wire contracts and serialized snapshots.

use crate::embedding_contract::{
    CODERANK_DOCUMENT_PREFIX, CODERANK_QUERY_PREFIX, EMBEDDING_ELEMENT_TYPE, EMBEDDING_MODEL_ID,
    EMBEDDING_MODEL_SHA256, EMBEDDING_NORMALIZATION, EMBEDDING_POOLING,
    EMBEDDING_VECTOR_SCHEMA_VERSION, RETRIEVAL_EMBEDDING_DIM,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

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

pub(super) fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
