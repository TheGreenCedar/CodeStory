use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{display, local_refresh_status, ready_repair_status};

const BROKER_SCHEMA_VERSION: u32 = 1;
const BROKER_DIR: &str = "readiness-broker";
const BROKER_SNAPSHOT_FILE: &str = "snapshot.json";
const MACHINE_RESOURCE_DIR: &str = "machine";
const MACHINE_LOCK_SCHEMA_VERSION: u32 = 1;
const MACHINE_LOCK_STALE_TTL: Duration = Duration::from_secs(20 * 60);
const MACHINE_REAPER_LOCK_STALE_TTL: Duration = Duration::from_secs(2 * 60);
pub(crate) const NATIVE_EMBEDDING_RESOURCE: &str = "native_embedding_runtime";
static SNAPSHOT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct BrokerMachineResourceReaperLockFile {
    schema_version: u32,
    resource: String,
    pid: u32,
    started_at_epoch_ms: i64,
    token: String,
}

#[derive(Debug)]
pub(crate) enum BrokerMachineResourceLockAttempt {
    Acquired(BrokerMachineResourceLock),
    Busy(BrokerMachineResourceBusy),
}

#[derive(Debug)]
pub(crate) enum BrokerNativeEmbeddingResourceLease {
    Acquired(BrokerMachineResourceLock),
    Reused { pid: u32 },
}

#[derive(Debug, Clone)]
pub(crate) struct BrokerMachineResourceBusy {
    pub(crate) snapshot: BrokerResourceSnapshot,
}

#[derive(Debug)]
pub(crate) struct BrokerMachineResourceLock {
    path: PathBuf,
    token: String,
    release_on_drop: bool,
}

#[derive(Debug)]
struct BrokerMachineResourceReaperLock {
    path: PathBuf,
    token: String,
}

impl Drop for BrokerMachineResourceLock {
    fn drop(&mut self) {
        if !self.release_on_drop {
            return;
        }
        let Some(lock) = read_machine_resource_lock_file(&self.path) else {
            return;
        };
        if lock.token == self.token {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Drop for BrokerMachineResourceReaperLock {
    fn drop(&mut self) {
        let Some(lock) = read_machine_resource_reaper_lock_file(&self.path) else {
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
                BrokerMachineResourceLock {
                    path,
                    token,
                    release_on_drop: true,
                },
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

    if !reap_stale_machine_resource_lock(resource, &path)? {
        return Ok(BrokerMachineResourceLockAttempt::Busy(
            BrokerMachineResourceBusy {
                snapshot: machine_resource_snapshot(resource),
            },
        ));
    }
    match create_lock_file(&path, &content) {
        Ok(()) => Ok(BrokerMachineResourceLockAttempt::Acquired(
            BrokerMachineResourceLock {
                path,
                token,
                release_on_drop: true,
            },
        )),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(
            BrokerMachineResourceLockAttempt::Busy(BrokerMachineResourceBusy {
                snapshot: machine_resource_snapshot(resource),
            }),
        ),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn transfer_machine_resource_lock_to_pid(
    lock: &mut BrokerMachineResourceLock,
    pid: u32,
) -> Result<bool> {
    let Some(mut file_lock) = read_machine_resource_lock_file(&lock.path) else {
        return Ok(false);
    };
    if file_lock.token != lock.token {
        return Ok(false);
    }
    file_lock.pid = pid;
    file_lock.started_at_epoch_ms = now_epoch_ms();
    fs::write(&lock.path, serde_json::to_vec_pretty(&file_lock)?)?;
    lock.release_on_drop = false;
    Ok(true)
}

pub(crate) fn transfer_native_embedding_resource_lease(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    state: &codestory_retrieval::SidecarStateFile,
) -> Result<()> {
    let Some(launch) = native_embedding_launch_from_sidecar_state(state) else {
        if matches!(
            lease,
            Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
        ) {
            bail!("reused native embedding broker lease missing final state pid");
        }
        return Ok(());
    };
    let Some(pid) = launch.pid else {
        if matches!(
            lease,
            Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
        ) {
            bail!("reused native embedding broker lease missing final state pid");
        }
        return Ok(());
    };
    match lease {
        Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)) => {
            let validated_pid = codestory_retrieval::ensure_native_embedding_launch_identity(
                launch,
            )
            .with_context(|| format!("validate native embedding broker handoff pid {pid}"))?;
            if validated_pid != pid {
                bail!(
                    "validated native embedding broker handoff pid mismatch: expected {pid}, got {validated_pid}"
                );
            }
            if !transfer_machine_resource_lock_to_pid(lock, pid)? {
                bail!("native embedding broker lock handoff failed for pid {pid}");
            }
        }
        Some(BrokerNativeEmbeddingResourceLease::Reused { pid: reused_pid }) => {
            if *reused_pid != pid {
                bail!(
                    "reused native embedding broker lease pid mismatch: expected {reused_pid}, got {pid}"
                );
            }
        }
        None => bail!("native embedding process spawned without broker machine lock"),
    }
    Ok(())
}

pub(crate) fn cleanup_native_embedding_resource_lease_after_bootstrap_error(
    lease: &Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<()> {
    if matches!(lease, Some(BrokerNativeEmbeddingResourceLease::Acquired(_))) {
        codestory_retrieval::sidecar_down_for_runtime(sidecar)?;
    }
    Ok(())
}

pub(crate) fn native_embedding_pid_from_sidecar_state_file(
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<Option<u32>> {
    Ok(read_sidecar_state_file(sidecar)?
        .and_then(|state| native_embedding_pid_from_sidecar_state(&state)))
}

fn read_sidecar_state_file(
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<Option<codestory_retrieval::SidecarStateFile>> {
    if !sidecar.layout.state_file.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&sidecar.layout.state_file)
        .with_context(|| format!("read {}", sidecar.layout.state_file.display()))?;
    let state = serde_json::from_str(&contents)
        .with_context(|| format!("parse {}", sidecar.layout.state_file.display()))?;
    Ok(Some(state))
}

fn native_embedding_pid_from_sidecar_state(
    state: &codestory_retrieval::SidecarStateFile,
) -> Option<u32> {
    native_embedding_launch_from_sidecar_state(state)?.pid
}

fn native_embedding_launch_from_sidecar_state(
    state: &codestory_retrieval::SidecarStateFile,
) -> Option<&codestory_retrieval::EmbeddingLaunchMetadata> {
    state.embedding_launch.as_ref().filter(|launch| {
        launch.launch_mode == codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned.as_str()
    })
}

pub(crate) fn release_machine_resource_lock_for_pid(resource: &str, pid: u32) -> Result<bool> {
    let path = machine_resource_lock_path(resource);
    let Some(file_lock) = read_machine_resource_lock_file(&path) else {
        return Ok(false);
    };
    if file_lock.pid != pid {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

fn reap_stale_machine_resource_lock(resource: &str, path: &Path) -> Result<bool> {
    let Some(_reaper) = try_acquire_machine_resource_reaper_lock(resource)? else {
        return Ok(false);
    };
    if !machine_lock_file_is_stale(path) {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

pub(crate) fn acquire_native_embedding_resource_lease_if_needed(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    wait: Duration,
    poll: Duration,
) -> Result<Option<BrokerNativeEmbeddingResourceLease>> {
    acquire_native_embedding_resource_lease_if_needed_with_validator(
        scope,
        sidecar,
        wait,
        poll,
        codestory_retrieval::ensure_native_embedding_launch_identity,
    )
}

fn acquire_native_embedding_resource_lease_if_needed_with_validator(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    wait: Duration,
    poll: Duration,
    mut validate_launch: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<BrokerNativeEmbeddingResourceLease>> {
    if codestory_retrieval::embedding_server_launch_mode()?
        != codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned
    {
        return Ok(None);
    }
    let deadline = Instant::now() + wait;
    loop {
        match try_acquire_machine_resource_lock(NATIVE_EMBEDDING_RESOURCE, scope)? {
            BrokerMachineResourceLockAttempt::Acquired(lock) => {
                return Ok(Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)));
            }
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                if let Some(pid) = reusable_native_embedding_resource_pid(
                    scope,
                    sidecar,
                    &busy,
                    &mut validate_launch,
                )? {
                    return Ok(Some(BrokerNativeEmbeddingResourceLease::Reused { pid }));
                }
                if Instant::now() >= deadline {
                    return bail_native_embedding_busy(&busy);
                }
                std::thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    }
}

fn reusable_native_embedding_resource_pid(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    busy: &BrokerMachineResourceBusy,
    validate_launch: &mut impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<u32>> {
    let Some(owner_pid) = busy.snapshot.owner_pid else {
        return Ok(None);
    };
    if busy.snapshot.owner_project_id.as_deref() != Some(scope.project_id.as_str())
        || busy.snapshot.owner_workspace_root.as_deref() != Some(scope.workspace_root.as_str())
    {
        return Ok(None);
    }
    let Some(state) = read_sidecar_state_file(sidecar)? else {
        return Ok(None);
    };
    if !sidecar_state_matches_runtime(&state, sidecar) {
        return Ok(None);
    }
    let Some(launch) = state.embedding_launch.as_ref() else {
        return Ok(None);
    };
    if launch.launch_mode != codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned.as_str()
        || launch.endpoint
            != codestory_retrieval::SidecarLayout::embed_base_url(sidecar.embed_http_port)
        || launch.pid != Some(owner_pid)
    {
        return Ok(None);
    }
    let validated_pid = validate_launch(launch)
        .with_context(|| format!("validate reusable native embedding pid {owner_pid}"))?;
    if validated_pid != owner_pid {
        bail!(
            "validated reusable native embedding pid mismatch: expected {owner_pid}, got {validated_pid}"
        );
    }
    Ok(Some(owner_pid))
}

fn sidecar_state_matches_runtime(
    state: &codestory_retrieval::SidecarStateFile,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> bool {
    state.owner == "codestory"
        && state.namespace == sidecar.namespace
        && state.compose_project == sidecar.compose_project
        && state.profile == sidecar.profile.as_str()
        && state.run_id.as_deref() == sidecar.run_id.as_deref()
        && state.embed_http_port == sidecar.embed_http_port
        && state.embed_url
            == codestory_retrieval::SidecarLayout::embed_base_url(sidecar.embed_http_port)
}

fn bail_native_embedding_busy<T>(busy: &BrokerMachineResourceBusy) -> Result<T> {
    let owner = busy
        .snapshot
        .owner_workspace_root
        .as_deref()
        .unwrap_or("unknown");
    bail!(
        "native embedding runtime is busy for another CodeStory operation: resource={} owner_project={} owner_workspace={} owner_pid={:?}; retry after the current repair reaches full retrieval",
        busy.snapshot.resource,
        busy.snapshot
            .owner_project_id
            .as_deref()
            .unwrap_or("unknown"),
        owner,
        busy.snapshot.owner_pid
    );
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
        return !ready_repair_status::process_is_running(lock.pid);
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

fn try_acquire_machine_resource_reaper_lock(
    resource: &str,
) -> Result<Option<BrokerMachineResourceReaperLock>> {
    let path = machine_resource_reaper_lock_path(resource);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let started_at_epoch_ms = now_epoch_ms();
    let pid = std::process::id();
    let token = format!("{pid}:{started_at_epoch_ms}:reaper");
    let lock = BrokerMachineResourceReaperLockFile {
        schema_version: MACHINE_LOCK_SCHEMA_VERSION,
        resource: resource.to_string(),
        pid,
        started_at_epoch_ms,
        token: token.clone(),
    };
    let content = serde_json::to_vec_pretty(&lock)?;

    match create_lock_file(&path, &content) {
        Ok(()) => Ok(Some(BrokerMachineResourceReaperLock { path, token })),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            if !machine_reaper_lock_file_is_stale(&path) {
                return Ok(None);
            }
            fs::write(&path, &content)?;
            match read_machine_resource_reaper_lock_file(&path) {
                Some(current) if current.token == token => {
                    Ok(Some(BrokerMachineResourceReaperLock { path, token }))
                }
                _ => Ok(None),
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn read_machine_resource_lock_file(path: &Path) -> Option<BrokerMachineResourceLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(|lock: &BrokerMachineResourceLockFile| {
            lock.schema_version == MACHINE_LOCK_SCHEMA_VERSION
        })
}

fn read_machine_resource_reaper_lock_file(
    path: &Path,
) -> Option<BrokerMachineResourceReaperLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(|lock: &BrokerMachineResourceReaperLockFile| {
            lock.schema_version == MACHINE_LOCK_SCHEMA_VERSION
        })
}

fn machine_reaper_lock_file_is_stale(path: &Path) -> bool {
    let now = now_epoch_ms();
    if let Some(lock) = read_machine_resource_reaper_lock_file(path) {
        if !ready_repair_status::process_is_running(lock.pid) {
            return true;
        }
        return now.saturating_sub(lock.started_at_epoch_ms)
            > MACHINE_REAPER_LOCK_STALE_TTL.as_millis() as i64;
    }
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| {
            let modified_ms = modified.as_millis().min(i64::MAX as u128) as i64;
            now.saturating_sub(modified_ms) > MACHINE_REAPER_LOCK_STALE_TTL.as_millis() as i64
        })
        .unwrap_or(true)
}

fn write_snapshot_file(path: &Path, snapshot: &ReadinessBrokerSnapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let counter = SNAPSHOT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        BROKER_SNAPSHOT_FILE,
        std::process::id(),
        counter
    ));
    fs::write(&temp_path, serde_json::to_string_pretty(snapshot)?)?;
    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            let _ = fs::remove_file(path);
            fs::rename(&temp_path, path)?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(error.into())
        }
    }
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

fn machine_resource_reaper_lock_path(resource: &str) -> PathBuf {
    broker_cache_root()
        .join(BROKER_DIR)
        .join(MACHINE_RESOURCE_DIR)
        .join(format!("{}.reap.lock", safe_name(resource)))
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
    use std::thread;
    use tempfile::tempdir;

    fn unique_resource(prefix: &str) -> String {
        format!("{prefix}-{}-{}", std::process::id(), now_epoch_ms())
    }

    fn cleanup_machine_resource(resource: &str) {
        let _ = fs::remove_file(machine_resource_lock_path(resource));
        let _ = fs::remove_file(machine_resource_reaper_lock_path(resource));
    }

    fn test_scope(project: &Path, run_id: &str) -> BrokerScope {
        agent_repair_scope(project, Some(run_id), "9.9.9")
    }

    fn write_machine_lock(resource: &str, scope: &BrokerScope, pid: u32) -> PathBuf {
        write_machine_lock_at(resource, scope, pid, now_epoch_ms())
    }

    fn write_machine_lock_at(
        resource: &str,
        scope: &BrokerScope,
        pid: u32,
        started_at_epoch_ms: i64,
    ) -> PathBuf {
        let path = machine_resource_lock_path(resource);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create lock parent");
        }
        let lock = BrokerMachineResourceLockFile {
            schema_version: MACHINE_LOCK_SCHEMA_VERSION,
            resource: resource.to_string(),
            scope: scope.clone(),
            pid,
            started_at_epoch_ms,
            token: format!("test:{pid}:{started_at_epoch_ms}"),
            operation_id: broker_operation_id(scope),
        };
        fs::write(
            &path,
            serde_json::to_vec_pretty(&lock).expect("serialize lock"),
        )
        .expect("write lock");
        path
    }

    fn sample_snapshot(project: &Path) -> ReadinessBrokerSnapshot {
        let canonical_root = clean_path_text(project);
        let canonical_root_hash = hash_text(&canonical_root);
        ReadinessBrokerSnapshot {
            schema_version: BROKER_SCHEMA_VERSION,
            install_id: "test-install".to_string(),
            project_id: project_id_from_hash(&canonical_root_hash),
            canonical_root_hash,
            workspace_root: clean_path(project),
            cli_version: "9.9.9".to_string(),
            updated_at_epoch_ms: now_epoch_ms(),
            snapshot_path: None,
            persistence_status: "pending".to_string(),
            persistence_error: None,
            operations: Vec::new(),
            resources: BTreeMap::new(),
            reconciliation: BrokerReconciliationSnapshot {
                status: "observed".to_string(),
                cleanup_performed: false,
                stale_status_paths_removed: Vec::new(),
                stale_lock_paths_removed: Vec::new(),
                abandoned_repairs: Vec::new(),
                local_refresh_cleanups: Vec::new(),
                active_repair: None,
                unresolved_orphan_reason: None,
            },
            gpu_proof: None,
        }
    }

    fn native_sidecar_state(
        spawned_at_epoch_ms: Option<i64>,
    ) -> codestory_retrieval::SidecarStateFile {
        codestory_retrieval::SidecarStateFile {
            owner: "codestory".to_string(),
            profile: "agent".to_string(),
            namespace: "codestory-test".to_string(),
            compose_project: "codestory-test".to_string(),
            run_id: Some("shared-agent".to_string()),
            zoekt_http_port: 37031,
            qdrant_http_port: 37032,
            qdrant_grpc_port: 37033,
            embed_http_port: 37040,
            embed_url: "http://127.0.0.1:37040/v1/embeddings".to_string(),
            embedding_device_policy: "accelerator_required".to_string(),
            embedding_device_state: "gpu_verified".to_string(),
            embedding_device_observation_source: "test".to_string(),
            embedding_detected_provider: Some("vulkan".to_string()),
            embedding_detected_gpu: Some("Vulkan0".to_string()),
            embedding_accelerator_requested: true,
            embedding_accelerator_request_provider: Some("vulkan".to_string()),
            embedding_accelerator_request_device: Some("Vulkan0".to_string()),
            embedding_cpu_allowed: false,
            embedding_launch: Some(codestory_retrieval::EmbeddingLaunchMetadata {
                provider: "llamacpp".to_string(),
                launch_mode: codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned
                    .as_str()
                    .to_string(),
                endpoint: "http://127.0.0.1:37040/v1/embeddings".to_string(),
                pid: Some(1234),
                spawned_at_epoch_ms,
                launch_args: vec!["--port".to_string(), "37040".to_string()],
                launch_fingerprint_sha256: Some("fingerprint".to_string()),
                executable_source: Some("test".to_string()),
                executable_path: Some("C:/cache/llama-server.exe".to_string()),
                model_path: Some("C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string()),
                requested_device: Some("Vulkan0".to_string()),
            }),
            sidecar_images: codestory_retrieval::default_sidecar_image_pins(),
            zoekt_data_dir: "C:/cache/zoekt".to_string(),
            qdrant_data_dir: "C:/cache/qdrant".to_string(),
            scip_artifacts_root: "C:/cache/scip".to_string(),
            compose_file: None,
            cleanup_command: "codestory-cli retrieval down".to_string(),
            started_at_epoch_ms: 100,
        }
    }

    fn write_matching_native_sidecar_state(
        sidecar: &codestory_retrieval::SidecarRuntimeConfig,
        pid: u32,
    ) {
        let mut state = native_sidecar_state(Some(now_epoch_ms()));
        state.profile = sidecar.profile.as_str().to_string();
        state.namespace = sidecar.namespace.clone();
        state.compose_project = sidecar.compose_project.clone();
        state.run_id = sidecar.run_id.clone();
        state.embed_http_port = sidecar.embed_http_port;
        state.embed_url =
            codestory_retrieval::SidecarLayout::embed_base_url(sidecar.embed_http_port);
        if let Some(launch) = state.embedding_launch.as_mut() {
            launch.endpoint = state.embed_url.clone();
            launch.pid = Some(pid);
        }
        if let Some(parent) = sidecar.layout.state_file.parent() {
            fs::create_dir_all(parent).expect("create state parent");
        }
        fs::write(
            &sidecar.layout.state_file,
            serde_json::to_vec_pretty(&state).expect("serialize state"),
        )
        .expect("write state");
    }

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

    #[test]
    fn native_embedding_busy_lock_reuses_matching_sidecar_owner() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("native-reuse");
        cleanup_machine_resource(&resource);
        let scope = test_scope(project.path(), "shared-agent");
        let sidecar = codestory_retrieval::sidecar_runtime_for_project_with_run_id(
            project.path(),
            codestory_retrieval::SidecarProfile::Agent,
            Some("shared-agent"),
        );
        let owner_pid = std::process::id();
        write_machine_lock(&resource, &scope, owner_pid);
        write_matching_native_sidecar_state(&sidecar, owner_pid);
        let busy = BrokerMachineResourceBusy {
            snapshot: machine_resource_snapshot(&resource),
        };
        let mut validator_called = false;

        let reused =
            reusable_native_embedding_resource_pid(&scope, &sidecar, &busy, &mut |launch| {
                validator_called = true;
                assert_eq!(launch.pid, Some(owner_pid));
                Ok(owner_pid)
            })
            .expect("reuse check");

        assert_eq!(reused, Some(owner_pid));
        assert!(validator_called);
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn native_embedding_busy_lock_rejects_mismatched_state_pid() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("native-mismatch");
        cleanup_machine_resource(&resource);
        let scope = test_scope(project.path(), "shared-agent");
        let sidecar = codestory_retrieval::sidecar_runtime_for_project_with_run_id(
            project.path(),
            codestory_retrieval::SidecarProfile::Agent,
            Some("shared-agent"),
        );
        let owner_pid = std::process::id();
        write_machine_lock(&resource, &scope, owner_pid);
        write_matching_native_sidecar_state(&sidecar, owner_pid.saturating_add(1));
        let busy = BrokerMachineResourceBusy {
            snapshot: machine_resource_snapshot(&resource),
        };

        let reused = reusable_native_embedding_resource_pid(&scope, &sidecar, &busy, &mut |_| {
            panic!("mismatched pid must not reach live identity validation")
        })
        .expect("reuse check");

        assert_eq!(reused, None);
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn native_embedding_acquired_lease_releases_without_handoff_on_error() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("native-error-release");
        cleanup_machine_resource(&resource);
        let scope = test_scope(project.path(), "shared-agent");
        let sidecar = codestory_retrieval::sidecar_runtime_for_project_with_run_id(
            project.path(),
            codestory_retrieval::SidecarProfile::Agent,
            Some("shared-agent"),
        );
        let lock = match try_acquire_machine_resource_lock(&resource, &scope)
            .expect("acquire machine lock")
        {
            BrokerMachineResourceLockAttempt::Acquired(lock) => lock,
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                panic!("first lock should acquire, got {busy:?}")
            }
        };
        let path = machine_resource_lock_path(&resource);
        let lease = Some(BrokerNativeEmbeddingResourceLease::Acquired(lock));

        cleanup_native_embedding_resource_lease_after_bootstrap_error(&lease, &sidecar)
            .expect("cleanup after bootstrap error");
        drop(lease);

        assert!(!path.exists(), "untransferred lease should release on drop");
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn machine_resource_lock_reports_busy_until_owner_drops() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("single-owner");
        cleanup_machine_resource(&resource);
        let scope = test_scope(project.path(), "owner");

        let lock = match try_acquire_machine_resource_lock(&resource, &scope)
            .expect("acquire machine lock")
        {
            BrokerMachineResourceLockAttempt::Acquired(lock) => lock,
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                panic!("first lock should acquire, got {busy:?}")
            }
        };
        let busy =
            match try_acquire_machine_resource_lock(&resource, &scope).expect("second acquire") {
                BrokerMachineResourceLockAttempt::Acquired(_) => {
                    panic!("second lock should be busy")
                }
                BrokerMachineResourceLockAttempt::Busy(busy) => busy,
            };
        assert_eq!(busy.snapshot.status, "busy");
        assert_eq!(busy.snapshot.owner_pid, Some(std::process::id()));

        drop(lock);
        let reacquired =
            try_acquire_machine_resource_lock(&resource, &scope).expect("reacquire after drop");
        assert!(matches!(
            reacquired,
            BrokerMachineResourceLockAttempt::Acquired(_)
        ));
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn machine_resource_lock_reclaims_dead_owner() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("dead-owner");
        cleanup_machine_resource(&resource);
        let old_scope = test_scope(project.path(), "dead");
        let new_scope = test_scope(project.path(), "new");
        write_machine_lock(&resource, &old_scope, u32::MAX);

        let acquired =
            try_acquire_machine_resource_lock(&resource, &new_scope).expect("reclaim dead owner");
        assert!(matches!(
            acquired,
            BrokerMachineResourceLockAttempt::Acquired(_)
        ));
        let snapshot = machine_resource_snapshot(&resource);
        assert_eq!(snapshot.status, "busy");
        assert_eq!(
            snapshot.owner_operation_id,
            Some(broker_operation_id(&new_scope))
        );
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn machine_resource_lock_does_not_reclaim_live_old_owner() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("live-old-owner");
        cleanup_machine_resource(&resource);
        let old_scope = test_scope(project.path(), "live-old");
        let new_scope = test_scope(project.path(), "new");
        write_machine_lock_at(
            &resource,
            &old_scope,
            std::process::id(),
            now_epoch_ms() - MACHINE_LOCK_STALE_TTL.as_millis() as i64 - 10_000,
        );

        let busy = match try_acquire_machine_resource_lock(&resource, &new_scope)
            .expect("acquire attempt")
        {
            BrokerMachineResourceLockAttempt::Acquired(_) => {
                panic!("live owner should remain busy even when the lock is old")
            }
            BrokerMachineResourceLockAttempt::Busy(busy) => busy,
        };

        assert_eq!(busy.snapshot.status, "busy");
        assert_eq!(busy.snapshot.owner_pid, Some(std::process::id()));
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn machine_resource_lock_transfers_to_spawned_pid_until_pid_release() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("pid-transfer");
        cleanup_machine_resource(&resource);
        let scope = test_scope(project.path(), "owner");

        let mut lock = match try_acquire_machine_resource_lock(&resource, &scope)
            .expect("acquire machine lock")
        {
            BrokerMachineResourceLockAttempt::Acquired(lock) => lock,
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                panic!("first lock should acquire, got {busy:?}")
            }
        };

        assert!(
            transfer_machine_resource_lock_to_pid(&mut lock, std::process::id())
                .expect("transfer lock")
        );
        drop(lock);
        let path = machine_resource_lock_path(&resource);
        assert!(
            path.exists(),
            "transferred lock should outlive launcher lock drop"
        );
        assert!(
            release_machine_resource_lock_for_pid(&resource, std::process::id())
                .expect("release pid lock")
        );
        assert!(!path.exists(), "pid release should remove lock file");
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn machine_resource_reaper_leaves_fresh_lock_owned() {
        let project = tempdir().expect("temp project");
        let resource = unique_resource("fresh-recheck");
        cleanup_machine_resource(&resource);
        let scope = test_scope(project.path(), "fresh");
        let path = write_machine_lock(&resource, &scope, std::process::id());

        assert!(
            !reap_stale_machine_resource_lock(&resource, &path).expect("reap check"),
            "fresh lock should not be reaped"
        );
        let lock = read_machine_resource_lock_file(&path).expect("fresh lock remains");
        assert_eq!(lock.operation_id, broker_operation_id(&scope));
        cleanup_machine_resource(&resource);
    }

    #[test]
    fn snapshot_file_round_trips_json() {
        let dir = tempdir().expect("temp dir");
        let snapshot = sample_snapshot(dir.path());
        let path = dir.path().join("snapshot.json");

        write_snapshot_file(&path, &snapshot).expect("write snapshot");

        let parsed: ReadinessBrokerSnapshot =
            serde_json::from_str(&fs::read_to_string(&path).expect("read snapshot"))
                .expect("parse snapshot");
        assert_eq!(parsed.schema_version, BROKER_SCHEMA_VERSION);
        assert_eq!(parsed.project_id, snapshot.project_id);
    }

    #[test]
    fn snapshot_file_uses_unique_temp_names_for_same_process_writers() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("snapshot.json");
        let snapshot = sample_snapshot(dir.path());
        let mut handles = Vec::new();

        for index in 0..4 {
            let path = path.clone();
            let mut snapshot = snapshot.clone();
            snapshot.project_id = format!("codestory-thread-{index}");
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    write_snapshot_file(&path, &snapshot).expect("write snapshot");
                }
            }));
        }

        for handle in handles {
            handle.join().expect("snapshot writer thread");
        }
        let parsed: ReadinessBrokerSnapshot =
            serde_json::from_str(&fs::read_to_string(&path).expect("read final snapshot"))
                .expect("parse final snapshot");
        assert_eq!(parsed.schema_version, BROKER_SCHEMA_VERSION);
        assert!(parsed.project_id.starts_with("codestory-thread-"));
    }
}
