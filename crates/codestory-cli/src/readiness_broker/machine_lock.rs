use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use crate::ready_repair_status;

use super::paths::{
    clean_path, machine_resource_lock_path, machine_resource_reaper_lock_path,
    machine_resource_reaper_takeover_lock_path, now_epoch_ms,
};
use super::scope::broker_operation_id;
use super::types::{BrokerResourceSnapshot, BrokerScope};

pub(crate) const MACHINE_LOCK_SCHEMA_VERSION: u32 = 1;
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
    pub(crate) token: String,
    pub(crate) operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_embedding_launch: Option<codestory_retrieval::EmbeddingLaunchMetadata>,
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
        native_embedding_launch: None,
    };
    let content = serde_json::to_vec_pretty(&lock)?;

    match create_lock_file(&path, &content) {
        Ok(()) => {
            return Ok(acquired_machine_resource_lock(path, token));
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }

    if !machine_lock_file_is_stale(&path) {
        return Ok(busy_machine_resource_attempt(resource));
    }

    if !reap_stale_machine_resource_lock(resource, &path)? {
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
    let pid = launch
        .pid
        .context("native embedding launch missing pid for broker handoff")?;
    let Some(mut file_lock) = read_machine_resource_lock_file(&lock.path) else {
        return Ok(false);
    };
    if file_lock.token != lock.token {
        return Ok(false);
    }
    file_lock.pid = pid;
    file_lock.started_at_epoch_ms = launch.spawned_at_epoch_ms.unwrap_or_else(now_epoch_ms);
    file_lock.native_embedding_launch = Some(launch.clone());
    fs::write(&lock.path, serde_json::to_vec_pretty(&file_lock)?)?;
    lock.release_on_drop = false;
    Ok(true)
}

pub(crate) fn release_machine_resource_lock_for_native_launch(
    resource: &str,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
) -> Result<bool> {
    let Some(pid) = launch.pid else {
        return Ok(false);
    };
    let path = machine_resource_lock_path(resource);
    let Some(_release_guard) = try_acquire_machine_resource_reaper_lock(resource)? else {
        return Ok(false);
    };
    let Some(file_lock) = read_machine_resource_lock_file(&path) else {
        return Ok(false);
    };
    if file_lock.pid != pid {
        return Ok(false);
    }
    if file_lock.native_embedding_launch.as_ref() != Some(launch) {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

pub(crate) fn reap_stale_machine_resource_lock(resource: &str, path: &Path) -> Result<bool> {
    let Some(reaper) = try_acquire_machine_resource_reaper_lock(resource)? else {
        return Ok(false);
    };
    if !machine_lock_file_is_stale(path) || !reaper.is_current() {
        return Ok(false);
    }
    let Some(stale_lock) = read_machine_resource_lock_file(path) else {
        match fs::remove_file(path) {
            Ok(()) => return Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    };
    if !machine_lock_is_stale(&stale_lock) || !reaper.is_current() {
        return Ok(false);
    }
    let Some(current) = read_machine_resource_lock_file(path) else {
        return Ok(false);
    };
    if current.token != stale_lock.token || !machine_lock_is_stale(&current) {
        return Ok(false);
    }
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    Ok(true)
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

pub(crate) fn machine_lock_file_is_stale(path: &Path) -> bool {
    let now = now_epoch_ms();
    if let Some(lock) = read_machine_resource_lock_file(path) {
        return machine_lock_is_stale(&lock);
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

pub(crate) fn machine_lock_is_stale(lock: &BrokerMachineResourceLockFile) -> bool {
    if lock.resource == NATIVE_EMBEDDING_RESOURCE
        && let Some(launch) = lock.native_embedding_launch.as_ref()
    {
        return matches!(
            codestory_retrieval::native_embedding_launch_identity_status(launch),
            codestory_retrieval::NativeEmbeddingLaunchIdentityStatus::NotRunning { .. }
                | codestory_retrieval::NativeEmbeddingLaunchIdentityStatus::Mismatched { .. }
        );
    }
    // Pre-handoff locks stay held while the owner PID is alive; only reclaim when dead.
    !ready_repair_status::process_is_running(lock.pid)
}

pub(crate) fn create_lock_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
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
        .filter(|lock: &BrokerMachineResourceLockFile| {
            lock.schema_version == MACHINE_LOCK_SCHEMA_VERSION
        })
}

pub(crate) fn read_machine_resource_reaper_lock_file(
    path: &Path,
) -> Option<BrokerMachineResourceReaperLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(|lock: &BrokerMachineResourceReaperLockFile| {
            lock.schema_version == MACHINE_LOCK_SCHEMA_VERSION
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
