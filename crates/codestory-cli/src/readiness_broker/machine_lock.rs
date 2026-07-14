use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ready_repair_status;

use super::paths::{
    clean_path, machine_resource_lock_path, machine_resource_reaper_lock_path,
    machine_resource_reaper_takeover_lock_path, now_epoch_ms,
};
use super::scope::broker_operation_id;
use super::scope::{
    BROKER_SCHEMA_VERSION, BROKER_SCHEMA_VERSION_V2, LEGACY_BROKER_SCHEMA_VERSION,
    effective_scope_identity,
};
use super::types::{BrokerResourceSnapshot, BrokerScope};

pub(crate) const MACHINE_LOCK_SCHEMA_VERSION: u32 = 3;
pub(crate) const MACHINE_LOCK_SCHEMA_VERSION_V2: u32 = 2;
pub(crate) const MACHINE_LOCK_STALE_TTL: Duration = Duration::from_secs(20 * 60);
pub(crate) const MACHINE_REAPER_LOCK_STALE_TTL: Duration = Duration::from_secs(2 * 60);
pub(crate) const NATIVE_EMBEDDING_RESOURCE: &str = "native_embedding_runtime";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerMachineResourceLockFile {
    pub(crate) schema_version: u32,
    pub(crate) resource: String,
    pub(crate) scope: BrokerScope,
    pub(crate) pid: u32,
    pub(crate) started_at_epoch_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process_start_identity: Option<String>,
    pub(crate) token: String,
    pub(crate) operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_embedding_launch: Option<codestory_retrieval::EmbeddingLaunchMetadata>,
    /// A launch recorded only so an interrupted/failed owner can be cleaned up exactly. Unlike a
    /// completed handoff, quarantined metadata must never be offered to borrowers for reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_embedding_quarantine_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BrokerMachineResourceReaperLockFile {
    pub(crate) schema_version: u32,
    pub(crate) resource: String,
    pub(crate) pid: u32,
    pub(crate) started_at_epoch_ms: i64,
    pub(crate) token: String,
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
    pub(crate) path: PathBuf,
    pub(crate) token: String,
    pub(crate) release_on_drop: bool,
}

#[derive(Debug)]
pub(crate) struct BrokerMachineResourceReaperLock {
    pub(crate) path: PathBuf,
    pub(crate) token: String,
}

impl BrokerMachineResourceReaperLock {
    pub(crate) fn is_current(&self) -> bool {
        read_machine_resource_reaper_lock_file(&self.path)
            .is_some_and(|lock| lock.token == self.token)
    }
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

fn busy_machine_resource(resource: &str) -> BrokerMachineResourceBusy {
    BrokerMachineResourceBusy {
        snapshot: machine_resource_snapshot(resource),
    }
}

fn busy_machine_resource_attempt(resource: &str) -> BrokerMachineResourceLockAttempt {
    BrokerMachineResourceLockAttempt::Busy(busy_machine_resource(resource))
}

fn acquired_machine_resource_lock(
    path: PathBuf,
    token: String,
) -> BrokerMachineResourceLockAttempt {
    BrokerMachineResourceLockAttempt::Acquired(BrokerMachineResourceLock {
        path,
        token,
        release_on_drop: true,
    })
}

#[cfg(test)]
pub(crate) fn try_acquire_machine_resource_lock(
    resource: &str,
    scope: &BrokerScope,
) -> Result<BrokerMachineResourceLockAttempt> {
    try_acquire_machine_resource_lock_with_native_resource(
        resource,
        scope,
        (resource == NATIVE_EMBEDDING_RESOURCE).then_some(NATIVE_EMBEDDING_RESOURCE),
    )
}

pub(crate) fn try_acquire_native_embedding_machine_resource_lock(
    resource: &str,
    scope: &BrokerScope,
) -> Result<BrokerMachineResourceLockAttempt> {
    try_acquire_machine_resource_lock_with_native_resource(resource, scope, Some(resource))
}

fn try_acquire_machine_resource_lock_with_native_resource(
    resource: &str,
    scope: &BrokerScope,
    native_resource: Option<&str>,
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
        process_start_identity: ready_repair_status::recorded_process_start_identity(pid),
        token: token.clone(),
        operation_id,
        native_embedding_launch: None,
        native_embedding_quarantine_reason: None,
    };
    let content = serde_json::to_vec_pretty(&lock)?;

    match create_lock_file(&path, &content) {
        Ok(()) => {
            return Ok(acquired_machine_resource_lock(path, token));
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }

    if !machine_lock_file_is_stale_for_native_resource(&path, native_resource) {
        return Ok(busy_machine_resource_attempt(resource));
    }

    if !reap_stale_machine_resource_lock_with_native_resource(resource, &path, native_resource)? {
        return Ok(busy_machine_resource_attempt(resource));
    }
    match create_lock_file(&path, &content) {
        Ok(()) => Ok(acquired_machine_resource_lock(path, token)),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Ok(busy_machine_resource_attempt(resource))
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn transfer_machine_resource_lock_to_native_launch(
    lock: &mut BrokerMachineResourceLock,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
) -> Result<bool> {
    transfer_machine_resource_lock_to_native_launch_with_publisher(
        lock,
        launch,
        publish_machine_resource_lock,
    )
}

pub(crate) fn transfer_machine_resource_lock_to_native_launch_with_publisher(
    lock: &mut BrokerMachineResourceLock,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
    publisher: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<bool> {
    publish_native_embedding_launch_to_machine_lock(lock, launch, None, true, publisher)
}

pub(crate) fn quarantine_machine_resource_lock_for_native_launch(
    lock: &mut BrokerMachineResourceLock,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
    reason: &str,
) -> Result<bool> {
    publish_native_embedding_launch_to_machine_lock(
        lock,
        launch,
        Some(reason.to_string()),
        false,
        publish_machine_resource_lock,
    )
}

pub(crate) fn release_owned_quarantined_machine_resource_lock(
    lock: &mut BrokerMachineResourceLock,
) -> Result<bool> {
    let Some(file_lock) = read_machine_resource_lock_file(&lock.path) else {
        return Ok(false);
    };
    if file_lock.token != lock.token || file_lock.native_embedding_quarantine_reason.is_none() {
        return Ok(false);
    }
    let Some(_release_guard) = try_acquire_machine_resource_reaper_lock(&file_lock.resource)?
    else {
        return Ok(false);
    };
    let Some(current) = read_machine_resource_lock_file(&lock.path) else {
        return Ok(false);
    };
    if current.token != lock.token || current.native_embedding_quarantine_reason.is_none() {
        return Ok(false);
    }
    fs::remove_file(&lock.path)?;
    lock.release_on_drop = false;
    Ok(true)
}

pub(crate) fn reset_owned_quarantined_machine_resource_lock(
    lock: &mut BrokerMachineResourceLock,
) -> Result<bool> {
    let Some(file_lock) = read_machine_resource_lock_file(&lock.path) else {
        return Ok(false);
    };
    if file_lock.token != lock.token {
        return Ok(false);
    }
    if file_lock.native_embedding_quarantine_reason.is_none() {
        lock.release_on_drop = true;
        return Ok(true);
    }
    let Some(_reset_guard) = try_acquire_machine_resource_reaper_lock(&file_lock.resource)? else {
        return Ok(false);
    };
    let Some(mut current) = read_machine_resource_lock_file(&lock.path) else {
        return Ok(false);
    };
    if current.token != lock.token || current.native_embedding_quarantine_reason.is_none() {
        return Ok(false);
    }
    current.native_embedding_launch = None;
    current.native_embedding_quarantine_reason = None;
    let content = serde_json::to_vec_pretty(&current)?;
    publish_machine_resource_lock(&lock.path, &content)?;
    lock.release_on_drop = true;
    Ok(true)
}

fn publish_native_embedding_launch_to_machine_lock(
    lock: &mut BrokerMachineResourceLock,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
    quarantine_reason: Option<String>,
    handoff_owner_to_launch: bool,
    publisher: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<bool> {
    let handoff_pid = handoff_owner_to_launch
        .then(|| {
            launch
                .pid
                .context("native embedding launch missing pid for broker handoff")
        })
        .transpose()?;
    let handoff_start_identity = handoff_owner_to_launch
        .then(|| {
            launch.process_start_identity.clone().context(
                "native embedding launch missing exact process start identity for broker handoff",
            )
        })
        .transpose()?;
    let Some(mut file_lock) = read_machine_resource_lock_file(&lock.path) else {
        return Ok(false);
    };
    if file_lock.token != lock.token {
        return Ok(false);
    }
    if handoff_owner_to_launch
        && file_lock.native_embedding_quarantine_reason.is_some()
        && file_lock.native_embedding_launch.as_ref() != Some(launch)
    {
        anyhow::bail!(
            "native embedding final handoff does not match the quarantined pre-handoff launch"
        );
    }
    if handoff_owner_to_launch {
        let handoff_pid = handoff_pid.expect("handoff pid validated above");
        file_lock.pid = handoff_pid;
        file_lock.started_at_epoch_ms = launch.spawned_at_epoch_ms.unwrap_or_else(now_epoch_ms);
        file_lock.process_start_identity = handoff_start_identity;
    }
    file_lock.native_embedding_launch = Some(launch.clone());
    file_lock.native_embedding_quarantine_reason = quarantine_reason;
    let content = serde_json::to_vec_pretty(&file_lock)?;
    publisher(&lock.path, &content)?;
    // Disable drop cleanup only after durably publishing the native PID and exact launch identity.
    lock.release_on_drop = false;
    Ok(true)
}

fn publish_machine_resource_lock(path: &Path, content: &[u8]) -> Result<()> {
    let temp_path = crate::file_state::atomic_json_path(
        path,
        &format!(
            "{}.{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("machine-resource.lock"),
            now_epoch_ms()
        ),
    );
    crate::file_state::write_synced_new_file(&temp_path, content)
        .with_context(|| format!("create temporary lock file {}", temp_path.display()))?;
    replace_machine_resource_lock(&temp_path, path)
        .with_context(|| format!("replace machine resource lock {}", path.display()))
}

#[cfg(windows)]
fn replace_machine_resource_lock(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let existing = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let new = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_machine_resource_lock(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp_path, path)
}

pub(crate) fn release_machine_resource_lock_for_native_launch_with_guard(
    resource: &str,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
    guard: &BrokerMachineResourceReaperLock,
) -> Result<bool> {
    if !guard.is_current() {
        return Ok(false);
    }
    let path = machine_resource_lock_path(resource);
    let Some(file_lock) = read_machine_resource_lock_file(&path) else {
        return Ok(false);
    };
    if file_lock.native_embedding_launch.as_ref() != Some(launch) {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

pub(crate) fn release_quarantined_machine_resource_lock_for_native_launch(
    resource: &str,
    expected_token: &str,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
) -> Result<bool> {
    let path = machine_resource_lock_path(resource);
    let Some(_release_guard) = try_acquire_machine_resource_reaper_lock(resource)? else {
        return Ok(false);
    };
    let Some(file_lock) = read_machine_resource_lock_file(&path) else {
        return Ok(false);
    };
    if file_lock.token != expected_token
        || file_lock.native_embedding_quarantine_reason.is_none()
        || file_lock.native_embedding_launch.as_ref() != Some(launch)
    {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

fn reap_stale_machine_resource_lock_with_native_resource(
    resource: &str,
    path: &Path,
    native_resource: Option<&str>,
) -> Result<bool> {
    let Some(reaper) = try_acquire_machine_resource_reaper_lock(resource)? else {
        return Ok(false);
    };
    if !machine_lock_file_is_stale_for_native_resource(path, native_resource)
        || !reaper.is_current()
    {
        return Ok(false);
    }
    let Some(stale_lock) = read_machine_resource_lock_file(path) else {
        match fs::remove_file(path) {
            Ok(()) => return Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    };
    if !machine_lock_is_stale_for_native_resource(&stale_lock, native_resource)
        || !reaper.is_current()
    {
        return Ok(false);
    }
    let Some(current) = read_machine_resource_lock_file(path) else {
        return Ok(false);
    };
    if current.token != stale_lock.token
        || !machine_lock_is_stale_for_native_resource(&current, native_resource)
    {
        return Ok(false);
    }
    cleanup_stale_native_embedding_owner_state(&current, native_resource)?;
    if !reaper.is_current() {
        return Ok(false);
    }
    let Some(current_after_cleanup) = read_machine_resource_lock_file(path) else {
        return Ok(false);
    };
    if current_after_cleanup.token != current.token
        || !machine_lock_is_stale_for_native_resource(&current_after_cleanup, native_resource)
    {
        return Ok(false);
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    Ok(true)
}

fn cleanup_stale_native_embedding_owner_state(
    lock: &BrokerMachineResourceLockFile,
    native_resource: Option<&str>,
) -> Result<()> {
    if native_resource != Some(lock.resource.as_str()) || lock.native_embedding_launch.is_some() {
        return Ok(());
    }
    let profile = match lock.scope.profile.as_str() {
        "agent" => codestory_retrieval::SidecarProfile::Agent,
        "local" => codestory_retrieval::SidecarProfile::Local,
        other => {
            anyhow::bail!("stale native embedding owner has unsupported sidecar profile {other:?}")
        }
    };
    let workspace_root = Path::new(&lock.scope.workspace_root);
    let runtime = crate::sidecar_runtime::for_project_with_run_id(
        workspace_root,
        profile,
        lock.scope.run_id.as_deref(),
    );
    if !runtime.layout.state_file.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&runtime.layout.state_file)
        .with_context(|| format!("read {}", runtime.layout.state_file.display()))?;
    let state: codestory_retrieval::SidecarStateFile = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", runtime.layout.state_file.display()))?;
    if !codestory_retrieval::sidecar_state_matches_runtime(&state, &runtime) {
        anyhow::bail!(
            "stale native embedding owner state does not match its recorded runtime: {}",
            runtime.layout.state_file.display()
        );
    }
    if state.embedding_launch.is_some() && !state.owns_embedding_launch() {
        anyhow::bail!(
            "stale pre-handoff native embedding lock points to attached rather than owned launch state"
        );
    }
    codestory_retrieval::sidecar_down_after_failed_bootstrap_for_runtime(&runtime)
        .context("clean exact stale pre-handoff native embedding owner state")
}

pub(crate) fn machine_resource_snapshot(resource: &str) -> BrokerResourceSnapshot {
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
    let queued_reason = match lock.as_ref() {
        Some(lock) if lock.native_embedding_quarantine_reason.is_some() => {
            Some("native_embedding_cleanup_pending".to_string())
        }
        Some(_) if status == "busy" => Some("machine_resource_busy".to_string()),
        _ => None,
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
        queued_reason,
    }
}

pub(crate) fn machine_lock_file_is_stale(path: &Path) -> bool {
    machine_lock_file_is_stale_for_native_resource(
        path,
        read_machine_resource_lock_file(path)
            .as_ref()
            .filter(|lock| lock.resource == NATIVE_EMBEDDING_RESOURCE)
            .map(|_| NATIVE_EMBEDDING_RESOURCE),
    )
}

fn machine_lock_file_is_stale_for_native_resource(
    path: &Path,
    native_resource: Option<&str>,
) -> bool {
    let now = now_epoch_ms();
    if let Some(lock) = read_machine_resource_lock_file(path) {
        return machine_lock_is_stale_for_native_resource(&lock, native_resource);
    }
    crate::file_state::file_modified_age_exceeds(path, MACHINE_LOCK_STALE_TTL, now)
}

fn machine_lock_is_stale_for_native_resource(
    lock: &BrokerMachineResourceLockFile,
    native_resource: Option<&str>,
) -> bool {
    if native_resource == Some(lock.resource.as_str())
        && let Some(launch) = lock.native_embedding_launch.as_ref()
    {
        if lock.native_embedding_quarantine_reason.is_some()
            && machine_lock_owner_state(lock)
                != ready_repair_status::ProcessOwnerState::GoneOrReused
        {
            return false;
        }
        return matches!(
            codestory_retrieval::native_embedding_launch_identity_status(launch),
            codestory_retrieval::NativeEmbeddingLaunchIdentityStatus::NotRunning { .. }
                | codestory_retrieval::NativeEmbeddingLaunchIdentityStatus::Mismatched { .. }
        );
    }
    // Pre-handoff locks stay held while the owner PID is alive; only reclaim when dead.
    machine_lock_owner_state(lock) == ready_repair_status::ProcessOwnerState::GoneOrReused
}

pub(crate) fn machine_lock_owner_state(
    lock: &BrokerMachineResourceLockFile,
) -> ready_repair_status::ProcessOwnerState {
    let state =
        ready_repair_status::process_owner_state(lock.pid, lock.process_start_identity.as_deref());
    if lock.schema_version == MACHINE_LOCK_SCHEMA_VERSION
        && lock.process_start_identity.is_none()
        && state != ready_repair_status::ProcessOwnerState::GoneOrReused
    {
        // A current-schema lock whose start identity could not be recorded must remain
        // fail-closed while that PID exists; PID-only matching would trust reuse.
        ready_repair_status::ProcessOwnerState::Unknown
    } else {
        state
    }
}

pub(crate) fn create_lock_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    crate::file_state::write_synced_new_file(path, content)
}

/// Shared create-or-takeover path for reaper and reaper-takeover lock files.
fn try_acquire_reaper_style_lock(
    path: PathBuf,
    resource: &str,
    token_suffix: &str,
    allow_nested_takeover: bool,
) -> Result<Option<BrokerMachineResourceReaperLock>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let started_at_epoch_ms = now_epoch_ms();
    let pid = std::process::id();
    let token = format!("{pid}:{started_at_epoch_ms}:{token_suffix}");
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
            if allow_nested_takeover {
                let Some(_takeover) = try_acquire_machine_resource_reaper_takeover_lock(resource)?
                else {
                    return Ok(None);
                };
                if !machine_reaper_lock_file_is_stale(&path) {
                    return Ok(None);
                }
            }
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            match create_lock_file(&path, &content) {
                Ok(()) => Ok(Some(BrokerMachineResourceReaperLock { path, token })),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn try_acquire_machine_resource_reaper_lock(
    resource: &str,
) -> Result<Option<BrokerMachineResourceReaperLock>> {
    try_acquire_reaper_style_lock(
        machine_resource_reaper_lock_path(resource),
        resource,
        "reaper",
        true,
    )
}

pub(crate) fn try_acquire_machine_resource_reaper_takeover_lock(
    resource: &str,
) -> Result<Option<BrokerMachineResourceReaperLock>> {
    try_acquire_reaper_style_lock(
        machine_resource_reaper_takeover_lock_path(resource),
        resource,
        "reaper-takeover",
        false,
    )
}

pub(crate) fn read_machine_resource_lock_file(
    path: &Path,
) -> Option<BrokerMachineResourceLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(machine_lock_has_valid_identity)
}

fn machine_lock_has_valid_identity(lock: &BrokerMachineResourceLockFile) -> bool {
    if effective_scope_identity(&lock.scope).is_none() {
        return false;
    }
    match lock.schema_version {
        MACHINE_LOCK_SCHEMA_VERSION => lock.scope.schema_version == BROKER_SCHEMA_VERSION,
        MACHINE_LOCK_SCHEMA_VERSION_V2 => lock.scope.schema_version == BROKER_SCHEMA_VERSION_V2,
        LEGACY_BROKER_SCHEMA_VERSION => lock.scope.schema_version == LEGACY_BROKER_SCHEMA_VERSION,
        _ => false,
    }
}

pub(crate) fn read_machine_resource_reaper_lock_file(
    path: &Path,
) -> Option<BrokerMachineResourceReaperLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(|lock: &BrokerMachineResourceReaperLockFile| {
            matches!(
                lock.schema_version,
                LEGACY_BROKER_SCHEMA_VERSION
                    | MACHINE_LOCK_SCHEMA_VERSION_V2
                    | MACHINE_LOCK_SCHEMA_VERSION
            )
        })
}

pub(crate) fn machine_reaper_lock_file_is_stale(path: &Path) -> bool {
    let now = now_epoch_ms();
    if let Some(lock) = read_machine_resource_reaper_lock_file(path) {
        if !ready_repair_status::process_is_running(lock.pid) {
            return true;
        }
        return now.saturating_sub(lock.started_at_epoch_ms)
            > MACHINE_REAPER_LOCK_STALE_TTL.as_millis() as i64;
    }
    crate::file_state::file_modified_age_exceeds(path, MACHINE_REAPER_LOCK_STALE_TTL, now)
}
