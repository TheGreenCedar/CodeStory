use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerScope {
    pub(crate) install_id: String,
    pub(crate) project_id: String,
    pub(crate) canonical_root_hash: String,
    pub(crate) workspace_root: String,
    pub(crate) profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_id: Option<String>,
    pub(crate) operation_kind: String,
    pub(crate) schema_version: u32,
    pub(crate) cli_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReadinessBrokerSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) install_id: String,
    pub(crate) project_id: String,
    pub(crate) canonical_root_hash: String,
    pub(crate) workspace_root: String,
    pub(crate) cli_version: String,
    pub(crate) updated_at_epoch_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot_path: Option<String>,
    pub(crate) persistence_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) persistence_error: Option<String>,
    #[serde(default)]
    pub(crate) operations: Vec<BrokerOperationSnapshot>,
    #[serde(default)]
    pub(crate) resources: BTreeMap<String, BrokerResourceSnapshot>,
    pub(crate) reconciliation: BrokerReconciliationSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) gpu_proof: Option<BrokerGpuProofSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerOperationSnapshot {
    pub(crate) operation_id: String,
    pub(crate) operation_kind: String,
    pub(crate) status: String,
    pub(crate) project_id: String,
    pub(crate) workspace_root: String,
    pub(crate) profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compose_project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_at_epoch_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) updated_at_epoch_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) degraded_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerResourceSnapshot {
    pub(crate) resource: String,
    pub(crate) scope: String,
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_workspace_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_at_epoch_ms: Option<i64>,
    pub(crate) lock_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) queued_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerReconciliationSnapshot {
    pub(crate) status: String,
    pub(crate) cleanup_performed: bool,
    #[serde(default)]
    pub(crate) stale_status_paths_removed: Vec<String>,
    #[serde(default)]
    pub(crate) stale_lock_paths_removed: Vec<String>,
    #[serde(default)]
    pub(crate) abandoned_repairs: Vec<BrokerOperationSnapshot>,
    #[serde(default)]
    pub(crate) local_refresh_cleanups: Vec<BrokerOperationSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_repair: Option<BrokerOperationSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unresolved_orphan_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerGpuProofInput {
    pub(crate) embedding_device_policy: Option<String>,
    pub(crate) embedding_device_state: Option<String>,
    pub(crate) embedding_device_observation_source: Option<String>,
    pub(crate) embedding_detected_provider: Option<String>,
    pub(crate) embedding_detected_gpu: Option<String>,
    pub(crate) embedding_accelerator_requested: Option<bool>,
    pub(crate) embedding_accelerator_request_provider: Option<String>,
    pub(crate) embedding_accelerator_request_device: Option<String>,
    pub(crate) embedding_cpu_allowed: Option<bool>,
    pub(crate) embed_smoke_ok: Option<bool>,
    pub(crate) embed_smoke_ms: Option<u64>,
    pub(crate) degraded_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerGpuProofSnapshot {
    pub(crate) requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) requested_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) requested_device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observed_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observation_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detected_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detected_gpu: Option<String>,
    pub(crate) cpu_allowed: bool,
    pub(crate) proof_status: String,
    pub(crate) meaningful_accelerator_work_proven: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) embed_smoke_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) embed_smoke_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) degraded_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct BrokerSnapshotInput {
    pub(crate) project_root: PathBuf,
    pub(crate) cache_root: PathBuf,
    pub(crate) agent_run_id: Option<String>,
    pub(crate) cli_version: String,
    pub(crate) gpu_proof: Option<BrokerGpuProofInput>,
    pub(crate) reconciliation: Option<BrokerReconciliationSnapshot>,
}
