use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{display, local_refresh_status, ready_repair_status};

const BROKER_SCHEMA_VERSION: u32 = 1;
const BROKER_DIR: &str = "readiness-broker";
const BROKER_SNAPSHOT_FILE: &str = "snapshot.json";
const MACHINE_RESOURCE_DIR: &str = "machine";
const MACHINE_LOCK_SCHEMA_VERSION: u32 = 1;
const MACHINE_LOCK_STALE_TTL: Duration = Duration::from_secs(20 * 60);
pub(crate) const NATIVE_EMBEDDING_RESOURCE: &str = "native_embedding_runtime";

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct BrokerMachineResourceLockFile {
    schema_version: u32,
    resource: String,
    scope: BrokerScope,
    pid: u32,
    started_at_epoch_ms: i64,
    token: String,
    operation_id: String,
}

#[derive(Debug)]
pub(crate) enum BrokerMachineResourceLockAttempt {
    Acquired(BrokerMachineResourceLock),
    Busy(BrokerMachineResourceBusy),
}

#[derive(Debug, Clone)]
pub(crate) struct BrokerMachineResourceBusy {
    pub(crate) snapshot: BrokerResourceSnapshot,
}

#[derive(Debug)]
pub(crate) struct BrokerMachineResourceLock {
    path: PathBuf,
    token: String,
}

impl Drop for BrokerMachineResourceLock {
    fn drop(&mut self) {
        let Some(lock) = read_machine_resource_lock_file(&self.path) else {
            return;
        };
        if lock.token == self.token {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn agent_repair_scope(
    project_root: &Path,
    run_id: Option<&str>,
    cli_version: &str,
) -> BrokerScope {
    operation_scope(
        project_root,
        "agent",
        run_id.or(Some(codestory_retrieval::DEFAULT_AGENT_RUN_ID)),
        "agent_repair",
        cli_version,
    )
}

pub(crate) fn operation_scope(
    project_root: &Path,
    profile: &str,
    run_id: Option<&str>,
    operation_kind: &str,
    cli_version: &str,
) -> BrokerScope {
    let install_id = install_id();
    let canonical_root = clean_path_text(project_root);
    let canonical_root_hash = hash_text(&canonical_root);
    let run_id = run_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    BrokerScope {
        install_id,
        project_id: project_id_from_hash(&canonical_root_hash),
        canonical_root_hash,
        workspace_root: display::clean_path_string(&project_root.to_string_lossy()),
        profile: profile.to_string(),
        run_id: run_id.clone(),
        agent_id: (profile == "agent").then_some(run_id).flatten(),
        operation_kind: operation_kind.to_string(),
        schema_version: BROKER_SCHEMA_VERSION,
        cli_version: cli_version.to_string(),
    }
}

pub(crate) fn broker_operation_id(scope: &BrokerScope) -> String {
    let run = scope.run_id.as_deref().unwrap_or("none");
    format!(
        "{}:{}:{}:{}",
        scope.operation_kind, scope.project_id, scope.profile, run
    )
}

pub(crate) fn try_acquire_machine_resource_lock(
    resource: &str,
    scope: &BrokerScope,
) -> Result<BrokerMachineResourceLockAttempt> {
    let path = machine_resource_lock_path(resource);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let started_at_epoch_ms = now_epoch_ms();
    let pid = std::process::id();
    let token = format!("{pid}:{started_at_epoch_ms}");
    let operation_id = broker_operation_id(scope);
    let lock = BrokerMachineResourceLockFile {
        schema_version: MACHINE_LOCK_SCHEMA_VERSION,
        resource: resource.to_string(),
        scope: scope.clone(),
        pid,
        started_at_epoch_ms,
        token: token.clone(),
        operation_id,
    };
    let content = serde_json::to_vec_pretty(&lock)?;

    match create_lock_file(&path, &content) {
        Ok(()) => {
            return Ok(BrokerMachineResourceLockAttempt::Acquired(
                BrokerMachineResourceLock { path, token },
            ));
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }

    if !machine_lock_file_is_stale(&path) {
        return Ok(BrokerMachineResourceLockAttempt::Busy(
            BrokerMachineResourceBusy {
                snapshot: machine_resource_snapshot(resource),
            },
        ));
    }

    let _ = fs::remove_file(&path);
    match create_lock_file(&path, &content) {
        Ok(()) => Ok(BrokerMachineResourceLockAttempt::Acquired(
            BrokerMachineResourceLock { path, token },
        )),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(
            BrokerMachineResourceLockAttempt::Busy(BrokerMachineResourceBusy {
                snapshot: machine_resource_snapshot(resource),
            }),
        ),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn acquire_machine_resource_lock_with_wait(
    resource: &str,
    scope: &BrokerScope,
    wait: Duration,
    poll: Duration,
) -> Result<BrokerMachineResourceLockAttempt> {
    let deadline = Instant::now() + wait;
    loop {
        match try_acquire_machine_resource_lock(resource, scope)? {
            acquired @ BrokerMachineResourceLockAttempt::Acquired(_) => return Ok(acquired),
            busy @ BrokerMachineResourceLockAttempt::Busy(_) => {
                if Instant::now() >= deadline {
                    return Ok(busy);
                }
                std::thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    }
}

pub(crate) fn reconcile_before_enqueue(
    project_root: &Path,
    cache_root: &Path,
    run_id: Option<&str>,
    cli_version: &str,
) -> BrokerReconciliationSnapshot {
    if let Some(active) = ready_repair_status::active_ready_repair_status(project_root, run_id) {
        return BrokerReconciliationSnapshot {
            status: "active_repair".to_string(),
            cleanup_performed: false,
            stale_status_paths_removed: Vec::new(),
            stale_lock_paths_removed: Vec::new(),
            abandoned_repairs: Vec::new(),
            local_refresh_cleanups: Vec::new(),
            active_repair: Some(operation_from_ready_status(
                project_root,
                cli_version,
                active,
                "running",
            )),
            unresolved_orphan_reason: None,
        };
    }

    let cleanups = ready_repair_status::cleanup_abandoned_ready_repair_status(project_root, run_id);
    let mut stale_status_paths_removed = Vec::new();
    let mut stale_lock_paths_removed = Vec::new();
    let mut abandoned_repairs = Vec::new();
    let mut local_refresh_cleanups = Vec::new();
    for cleanup in cleanups {
        if cleanup.removed_status_path {
            stale_status_paths_removed.push(clean_path(&cleanup.status_path));
        }
        stale_lock_paths_removed.extend(
            cleanup
                .removed_lock_paths
                .iter()
                .map(|path| clean_path(path)),
        );
        abandoned_repairs.push(operation_from_ready_status(
            project_root,
            cli_version,
            cleanup.status,
            "abandoned_cleaned",
        ));
    }
    if let Some(cleanup) =
        local_refresh_status::cleanup_stale_local_refresh_state(cache_root, project_root)
    {
        if cleanup.removed_status_path {
            stale_status_paths_removed.push(clean_path(&cleanup.status_path));
        }
        if cleanup.removed_lock_path {
            stale_lock_paths_removed.push(clean_path(&cleanup.lock_path));
        }
        let operation = match cleanup.status {
            Some(status) => operation_from_local_refresh_status(
                project_root,
                cli_version,
                status,
                "stale_cleaned",
                Some(cleanup.reason),
            ),
            None => operation_from_local_refresh_lock_cleanup(project_root, cli_version, cleanup),
        };
        local_refresh_cleanups.push(operation);
    }
    let cleanup_performed =
        !stale_status_paths_removed.is_empty() || !stale_lock_paths_removed.is_empty();
    BrokerReconciliationSnapshot {
        status: if cleanup_performed {
            "stale_state_cleaned".to_string()
        } else {
            "clean".to_string()
        },
        cleanup_performed,
        stale_status_paths_removed,
        stale_lock_paths_removed,
        abandoned_repairs,
        local_refresh_cleanups,
        active_repair: None,
        unresolved_orphan_reason: None,
    }
}

pub(crate) fn refresh_broker_snapshot(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    let mut snapshot = build_broker_snapshot(input);
    let path = broker_snapshot_path(&snapshot.canonical_root_hash);
    snapshot.snapshot_path = Some(clean_path(&path));
    snapshot.persistence_status = "persisted".to_string();
    snapshot.persistence_error = None;
    match write_snapshot_file(&path, &snapshot) {
        Ok(()) => {}
        Err(error) => {
            snapshot.persistence_status = "failed".to_string();
            snapshot.persistence_error = Some(error.to_string());
        }
    }
    snapshot
}

pub(crate) fn gpu_proof(input: BrokerGpuProofInput) -> BrokerGpuProofSnapshot {
    let requested = input.embedding_accelerator_requested.unwrap_or(false);
    let cpu_allowed = input.embedding_cpu_allowed.unwrap_or(false);
    let observed_state = input.embedding_device_state.clone();
    let accelerated = observed_state.as_deref() == Some("accelerated");
    let proof_status = if accelerated && !cpu_allowed {
        "verified"
    } else if requested {
        "gpu_unverified"
    } else if cpu_allowed {
        "cpu_allowed"
    } else {
        "not_requested"
    };
    let degraded_reason = if proof_status == "gpu_unverified" {
        Some("gpu_unverified".to_string())
    } else {
        input.degraded_reason.clone()
    };
    BrokerGpuProofSnapshot {
        requested,
        requested_provider: input.embedding_accelerator_request_provider,
        requested_device: input.embedding_accelerator_request_device,
        policy: input.embedding_device_policy,
        observed_state,
        observation_source: input.embedding_device_observation_source,
        detected_provider: input.embedding_detected_provider,
        detected_gpu: input.embedding_detected_gpu,
        cpu_allowed,
        proof_status: proof_status.to_string(),
        meaningful_accelerator_work_proven: proof_status == "verified",
        degraded_reason,
    }
}

fn build_broker_snapshot(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    let canonical_root = clean_path_text(&input.project_root);
    let canonical_root_hash = hash_text(&canonical_root);
    let project_id = project_id_from_hash(&canonical_root_hash);
    let cli_version = input.cli_version;
    let mut operations = Vec::new();
    if let Some(active) = ready_repair_status::active_ready_repair_status(
        &input.project_root,
        input.agent_run_id.as_deref(),
    ) {
        operations.push(operation_from_ready_status(
            &input.project_root,
            &cli_version,
            active,
            "running",
        ));
    } else if let Some(abandoned) = ready_repair_status::abandoned_ready_repair_status(
        &input.project_root,
        input.agent_run_id.as_deref(),
    ) {
        operations.push(operation_from_ready_status(
            &input.project_root,
            &cli_version,
            abandoned,
            "abandoned",
        ));
    }
    if let Some(local_refresh) =
        local_refresh_status::active_local_refresh_status(&input.cache_root, &input.project_root)
    {
        operations.push(operation_from_local_refresh(
            &input.project_root,
            &cli_version,
            local_refresh,
        ));
    }

    let mut resources = BTreeMap::new();
    resources.insert(
        NATIVE_EMBEDDING_RESOURCE.to_string(),
        machine_resource_snapshot(NATIVE_EMBEDDING_RESOURCE),
    );
    let reconciliation = input
        .reconciliation
        .unwrap_or_else(|| BrokerReconciliationSnapshot {
            status: "observed".to_string(),
            cleanup_performed: false,
            stale_status_paths_removed: Vec::new(),
            stale_lock_paths_removed: Vec::new(),
            abandoned_repairs: Vec::new(),
            local_refresh_cleanups: Vec::new(),
            active_repair: operations
                .iter()
                .find(|operation| {
                    operation.operation_kind == "agent_repair" && operation.status == "running"
                })
                .cloned(),
            unresolved_orphan_reason: None,
        });

    ReadinessBrokerSnapshot {
        schema_version: BROKER_SCHEMA_VERSION,
        install_id: install_id(),
        project_id,
        canonical_root_hash,
        workspace_root: display::clean_path_string(&input.project_root.to_string_lossy()),
        cli_version,
        updated_at_epoch_ms: now_epoch_ms(),
        snapshot_path: None,
        persistence_status: "pending".to_string(),
        persistence_error: None,
        operations,
        resources,
        reconciliation,
        gpu_proof: input.gpu_proof.map(gpu_proof),
    }
}

fn operation_from_ready_status(
    project_root: &Path,
    cli_version: &str,
    status: ready_repair_status::ReadyRepairStatus,
    operation_status: &str,
) -> BrokerOperationSnapshot {
    let scope = agent_repair_scope(project_root, status.run_id.as_deref(), cli_version);
    BrokerOperationSnapshot {
        operation_id: broker_operation_id(&scope),
        operation_kind: "agent_repair".to_string(),
        status: operation_status.to_string(),
        project_id: scope.project_id,
        workspace_root: scope.workspace_root,
        profile: status.profile,
        run_id: status.run_id.clone(),
        agent_id: status.run_id,
        namespace: Some(status.namespace),
        compose_project: Some(status.compose_project),
        phase: Some(status.phase),
        pid: Some(status.pid),
        started_at_epoch_ms: Some(status.started_at_epoch_ms),
        updated_at_epoch_ms: Some(status.updated_at_epoch_ms),
        degraded_reason: None,
    }
}

fn operation_from_local_refresh(
    project_root: &Path,
    cli_version: &str,
    status: local_refresh_status::LocalRefreshStatus,
) -> BrokerOperationSnapshot {
    operation_from_local_refresh_status(project_root, cli_version, status, "running", None)
}

fn operation_from_local_refresh_status(
    project_root: &Path,
    cli_version: &str,
    status: local_refresh_status::LocalRefreshStatus,
    operation_status: &str,
    degraded_reason: Option<String>,
) -> BrokerOperationSnapshot {
    let canonical_root = clean_path_text(project_root);
    let canonical_root_hash = hash_text(&canonical_root);
    let project_id = project_id_from_hash(&canonical_root_hash);
    BrokerOperationSnapshot {
        operation_id: format!("local_graph_refresh:{project_id}"),
        operation_kind: "local_graph_refresh".to_string(),
        status: operation_status.to_string(),
        project_id,
        workspace_root: display::clean_path_string(&project_root.to_string_lossy()),
        profile: "local".to_string(),
        run_id: None,
        agent_id: None,
        namespace: None,
        compose_project: None,
        phase: Some(status.phase),
        pid: Some(status.pid),
        started_at_epoch_ms: Some(status.started_at_epoch_ms),
        updated_at_epoch_ms: Some(status.updated_at_epoch_ms),
        degraded_reason: degraded_reason
            .or(status.last_failure_reason)
            .or_else(|| (cli_version.is_empty()).then(|| "missing_cli_version".to_string())),
    }
}

fn operation_from_local_refresh_lock_cleanup(
    project_root: &Path,
    cli_version: &str,
    cleanup: local_refresh_status::LocalRefreshCleanup,
) -> BrokerOperationSnapshot {
    let canonical_root = clean_path_text(project_root);
    let canonical_root_hash = hash_text(&canonical_root);
    let project_id = project_id_from_hash(&canonical_root_hash);
    BrokerOperationSnapshot {
        operation_id: format!("local_graph_refresh:{project_id}"),
        operation_kind: "local_graph_refresh".to_string(),
        status: "stale_cleaned".to_string(),
        project_id,
        workspace_root: display::clean_path_string(&project_root.to_string_lossy()),
        profile: "local".to_string(),
        run_id: None,
        agent_id: None,
        namespace: None,
        compose_project: None,
        phase: Some("unknown".to_string()),
        pid: cleanup.lock_pid,
        started_at_epoch_ms: cleanup.lock_started_at_epoch_ms,
        updated_at_epoch_ms: None,
        degraded_reason: Some(cleanup.reason)
            .or_else(|| (cli_version.is_empty()).then(|| "missing_cli_version".to_string())),
    }
}

fn machine_resource_snapshot(resource: &str) -> BrokerResourceSnapshot {
    let path = machine_resource_lock_path(resource);
    let lock = read_machine_resource_lock_file(&path);
    let stale = lock
        .as_ref()
        .is_some_and(|_| machine_lock_file_is_stale(&path));
    let status = match lock.as_ref() {
        Some(_) if stale => "stale",
        Some(_) => "busy",
        None => "available",
    };
    BrokerResourceSnapshot {
        resource: resource.to_string(),
        scope: "machine".to_string(),
        status: status.to_string(),
        owner_pid: lock.as_ref().map(|lock| lock.pid),
        owner_operation_id: lock.as_ref().map(|lock| lock.operation_id.clone()),
        owner_project_id: lock.as_ref().map(|lock| lock.scope.project_id.clone()),
        owner_workspace_root: lock.as_ref().map(|lock| lock.scope.workspace_root.clone()),
        started_at_epoch_ms: lock.as_ref().map(|lock| lock.started_at_epoch_ms),
        lock_path: clean_path(&path),
        queued_reason: (status == "busy").then(|| "machine_resource_busy".to_string()),
    }
}

fn machine_lock_file_is_stale(path: &Path) -> bool {
    let now = now_epoch_ms();
    if let Some(lock) = read_machine_resource_lock_file(path) {
        if !ready_repair_status::process_is_running(lock.pid) {
            return true;
        }
        return now.saturating_sub(lock.started_at_epoch_ms)
            > MACHINE_LOCK_STALE_TTL.as_millis() as i64;
    }
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| {
            let modified_ms = modified.as_millis().min(i64::MAX as u128) as i64;
            now.saturating_sub(modified_ms) > MACHINE_LOCK_STALE_TTL.as_millis() as i64
        })
        .unwrap_or(true)
}

fn create_lock_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn read_machine_resource_lock_file(path: &Path) -> Option<BrokerMachineResourceLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(|lock: &BrokerMachineResourceLockFile| {
            lock.schema_version == MACHINE_LOCK_SCHEMA_VERSION
        })
}

fn write_snapshot_file(path: &Path, snapshot: &ReadinessBrokerSnapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_file_name(format!(
        ".{}.{}.tmp",
        BROKER_SNAPSHOT_FILE,
        std::process::id()
    ));
    fs::write(&temp_path, serde_json::to_string_pretty(snapshot)?)?;
    let _ = fs::remove_file(path);
    fs::rename(temp_path, path)?;
    Ok(())
}

fn broker_snapshot_path(canonical_root_hash: &str) -> PathBuf {
    broker_cache_root()
        .join(BROKER_DIR)
        .join("projects")
        .join(canonical_root_hash)
        .join(BROKER_SNAPSHOT_FILE)
}

fn machine_resource_lock_path(resource: &str) -> PathBuf {
    broker_cache_root()
        .join(BROKER_DIR)
        .join(MACHINE_RESOURCE_DIR)
        .join(format!("{}.lock", safe_name(resource)))
}

fn install_id() -> String {
    for name in [
        "CODESTORY_INSTALL_ID",
        "CODESTORY_PLUGIN_INSTALL_ID",
        "CODESTORY_PLUGIN_DATA",
        "PLUGIN_DATA",
        "COPILOT_PLUGIN_DATA",
    ] {
        if let Ok(value) = std::env::var(name)
            && !value.trim().is_empty()
        {
            return format!("{}-{}", safe_name(name), &hash_text(value.trim())[..16]);
        }
    }
    format!(
        "cache-{}",
        &hash_text(&clean_path_text(&broker_cache_root()))[..16]
    )
}

fn broker_cache_root() -> PathBuf {
    codestory_retrieval::SidecarRuntimeConfig::local()
        .layout
        .state_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
}

fn project_id_from_hash(hash: &str) -> String {
    format!("codestory-{}", &hash[..16])
}

fn safe_name(value: &str) -> String {
    let mut name = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while name.contains("--") {
        name = name.replace("--", "-");
    }
    name.trim_matches('-').to_string()
}

fn clean_path(path: &Path) -> String {
    display::clean_path_string(&path.to_string_lossy())
}

fn clean_path_text(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn gpu_proof_requires_observed_acceleration_when_requested() {
        let proof = gpu_proof(BrokerGpuProofInput {
            embedding_device_policy: Some("accelerator_required".to_string()),
            embedding_device_state: Some("unknown".to_string()),
            embedding_device_observation_source: Some("native_device_list".to_string()),
            embedding_detected_provider: None,
            embedding_detected_gpu: None,
            embedding_accelerator_requested: Some(true),
            embedding_accelerator_request_provider: Some("vulkan".to_string()),
            embedding_accelerator_request_device: Some("Vulkan0".to_string()),
            embedding_cpu_allowed: Some(false),
            degraded_reason: Some("embedding_device_unverified".to_string()),
        });
        assert_eq!(proof.proof_status, "gpu_unverified");
        assert!(!proof.meaningful_accelerator_work_proven);
        assert_eq!(proof.degraded_reason.as_deref(), Some("gpu_unverified"));
    }

    #[test]
    fn broker_scope_carries_project_and_run_identity() {
        let project = tempdir().expect("temp project");
        let scope = agent_repair_scope(project.path(), Some("agent-1"), "9.9.9");
        assert_eq!(scope.schema_version, BROKER_SCHEMA_VERSION);
        assert_eq!(scope.profile, "agent");
        assert_eq!(scope.run_id.as_deref(), Some("agent-1"));
        assert_eq!(scope.agent_id.as_deref(), Some("agent-1"));
        assert!(scope.project_id.starts_with("codestory-"));
        assert_eq!(scope.cli_version, "9.9.9");
    }
}
