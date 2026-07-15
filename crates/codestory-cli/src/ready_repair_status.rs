use anyhow::Result;
use codestory_contracts::api::{ApiError, ApiErrorDetails, CommandFailureEnvelope};
pub(crate) use codestory_retrieval::ProcessOwnerState;
use codestory_retrieval::{
    DEFAULT_AGENT_RUN_ID, ProcessStartProbe, SidecarProfile, SidecarRuntimeConfig,
    probe_process_start_identity, process_owner_state as classify_process_owner_state,
};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const READY_REPAIR_STATUS_FILE: &str = "ready-repair-status.json";
const READY_REPAIR_RESULT_FILE: &str = "ready-repair-result.json";
const READY_REPAIR_ENQUEUE_LOCK_FILE: &str = "ready-repair-enqueue.lock";
const READY_REPAIR_COORDINATION_LOCK_FILE: &str = "ready-repair-coordination.lock";
const READY_REPAIR_LOCK_FILE: &str = "ready-repair.lock";
const READY_REPAIR_PROJECT_LOCK_FILE: &str = "ready-repair-project.lock";
pub(crate) const READY_REPAIR_STATUS_SCHEMA_VERSION: u32 = 1;
const READY_REPAIR_STATUS_TTL: Duration = Duration::from_secs(30);
const READY_REPAIR_LOCK_STALE_TTL: Duration = Duration::from_secs(120);
const READY_REPAIR_COORDINATION_TIMEOUT: Duration = Duration::from_secs(5);
const READY_REPAIR_COORDINATION_POLL: Duration = Duration::from_millis(10);
pub(crate) const READY_REPAIR_ATTEMPT_ENV: &str = "CODESTORY_READY_REPAIR_ATTEMPT_ID";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReadyRepairStatus {
    pub(crate) schema_version: u32,
    pub(crate) status: String,
    pub(crate) project_root: String,
    pub(crate) profile: String,
    pub(crate) run_id: Option<String>,
    pub(crate) namespace: String,
    pub(crate) phase: String,
    pub(crate) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process_start_identity: Option<String>,
    pub(crate) started_at_epoch_ms: i64,
    pub(crate) updated_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ReadyRepairReservationFile {
    schema_version: u32,
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_start_identity: Option<String>,
    started_at_epoch_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at_epoch_ms: Option<i64>,
    #[serde(rename = "token", alias = "attempt_id")]
    attempt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(default)]
    adopted: bool,
}

#[derive(Debug)]
pub(crate) struct ReadyRepairReservation {
    path: PathBuf,
    attempt_id: String,
    started_at_epoch_ms: i64,
    armed: bool,
}

#[derive(Debug)]
struct ReadyRepairCoordinationGuard {
    file: File,
}

impl Drop for ReadyRepairCoordinationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

impl ReadyRepairReservation {
    pub(crate) fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    pub(crate) fn started_at_epoch_ms(&self) -> i64 {
        self.started_at_epoch_ms
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ReadyRepairReservation {
    fn drop(&mut self) {
        if self.armed {
            remove_ready_repair_reservation_if_attempt_at(&self.path, &self.attempt_id);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReadyRepairWorkerResult {
    pub(crate) schema_version: u32,
    pub(crate) attempt_id: String,
    pub(crate) project_root: String,
    pub(crate) profile: String,
    pub(crate) run_id: Option<String>,
    pub(crate) namespace: String,
    pub(crate) pid: u32,
    pub(crate) started_at_epoch_ms: i64,
    pub(crate) finished_at_epoch_ms: i64,
    pub(crate) outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) auto_retry_fingerprint: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) wait_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminal_envelope: Option<CommandFailureEnvelope>,
    pub(crate) stdout_tail: String,
    pub(crate) stderr_tail: String,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

fn abandoned_repair_envelope(message: &str) -> CommandFailureEnvelope {
    CommandFailureEnvelope::new(ApiError::with_details(
        "background_repair_abandoned",
        message,
        ApiErrorDetails {
            failed_layer: Some("background_repair".to_string()),
            project: None,
            next_commands: Vec::new(),
            minimum_next: Vec::new(),
            full_repair: Vec::new(),
            readiness: None,
        },
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ReadyRepairLockFile {
    schema_version: u32,
    project_root: String,
    profile: String,
    run_id: Option<String>,
    namespace: String,
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_start_identity: Option<String>,
    started_at_epoch_ms: i64,
    token: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReadyRepairBusy {
    pub(crate) status: Option<ReadyRepairStatus>,
    pub(crate) lock_path: PathBuf,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ReadyRepairCleanup {
    pub(crate) status: ReadyRepairStatus,
    pub(crate) status_path: PathBuf,
    pub(crate) removed_status_path: bool,
    pub(crate) removed_lock_paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub(crate) enum ReadyRepairLockAttempt {
    Acquired(ReadyRepairLock),
    Busy(Box<ReadyRepairBusy>),
}

#[derive(Debug)]
pub(crate) struct ReadyRepairLock {
    path: PathBuf,
    token: String,
}

impl Drop for ReadyRepairLock {
    fn drop(&mut self) {
        let Some(lock) = read_ready_repair_lock_file(&self.path) else {
            return;
        };
        if lock.token == self.token {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn new_ready_repair_attempt_id() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}:{nonce}", std::process::id())
}

fn acquire_ready_repair_coordination(
    sidecar: &SidecarRuntimeConfig,
) -> Result<ReadyRepairCoordinationGuard> {
    acquire_ready_repair_coordination_at(&ready_repair_reservation_path(sidecar))
}

fn acquire_ready_repair_coordination_at(
    reservation_path: &Path,
) -> Result<ReadyRepairCoordinationGuard> {
    let path = reservation_path.with_file_name(READY_REPAIR_COORDINATION_LOCK_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    let deadline = Instant::now() + READY_REPAIR_COORDINATION_TIMEOUT;
    loop {
        if FileExt::try_lock_exclusive(&file)? {
            return Ok(ReadyRepairCoordinationGuard { file });
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "timed out coordinating ready-repair state at {}",
                path.display()
            ));
        }
        thread::sleep(READY_REPAIR_COORDINATION_POLL);
    }
}

pub(crate) fn try_reserve_ready_repair(
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
) -> Result<Option<ReadyRepairReservation>> {
    let path = ready_repair_reservation_path(sidecar);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let attempt_id = new_ready_repair_attempt_id();
    let pid = std::process::id();
    let started_at_epoch_ms = now_epoch_ms();
    let reservation = ReadyRepairReservationFile {
        schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
        pid,
        process_start_identity: recorded_process_start_identity(pid),
        started_at_epoch_ms,
        created_at_epoch_ms: Some(started_at_epoch_ms),
        attempt_id: attempt_id.clone(),
        project_root: Some(clean_path_text(project_root)),
        profile: Some(sidecar.profile.as_str().to_string()),
        run_id: sidecar.run_id.clone(),
        namespace: Some(sidecar.namespace.clone()),
        adopted: false,
    };
    let content = serde_json::to_vec_pretty(&reservation)?;
    let _coordination = acquire_ready_repair_coordination(sidecar)?;
    match create_ready_repair_lock_file(&path, &content) {
        Ok(()) => {
            return Ok(Some(ReadyRepairReservation {
                path,
                attempt_id,
                started_at_epoch_ms,
                armed: true,
            }));
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if !ready_repair_reservation_is_stale(&path) {
        return Ok(None);
    }
    if let Some(stale) = read_ready_repair_reservation_file(&path)
        && !ready_repair_terminal_result_matches(sidecar, &stale.attempt_id)
    {
        let abandoned = ReadyRepairWorkerResult {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            attempt_id: stale.attempt_id.clone(),
            project_root: stale
                .project_root
                .unwrap_or_else(|| clean_path_text(project_root)),
            profile: stale
                .profile
                .unwrap_or_else(|| sidecar.profile.as_str().to_string()),
            run_id: stale.run_id,
            namespace: stale.namespace.unwrap_or_else(|| sidecar.namespace.clone()),
            pid: stale.pid,
            started_at_epoch_ms: stale
                .created_at_epoch_ms
                .unwrap_or(stale.started_at_epoch_ms),
            finished_at_epoch_ms: now_epoch_ms(),
            outcome: "abandoned".to_string(),
            auto_retry_fingerprint: None,
            exit_code: None,
            wait_error: Some(
                "repair worker reservation owner exited before recording a terminal result"
                    .to_string(),
            ),
            terminal_envelope: Some(abandoned_repair_envelope(
                "repair worker reservation owner exited before recording a terminal result",
            )),
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
        if write_ready_repair_worker_result_locked(sidecar, &abandoned).is_err() {
            return Ok(None);
        }
    }
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    match create_ready_repair_lock_file(&path, &content) {
        Ok(()) => Ok(Some(ReadyRepairReservation {
            path,
            attempt_id,
            started_at_epoch_ms,
            armed: true,
        })),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn adopt_ready_repair_reservation(
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
    attempt_id: &str,
) -> Result<ReadyRepairReservation> {
    let pid = std::process::id();
    let process_start_identity = recorded_process_start_identity(pid)
        .filter(|identity| !identity.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("could not prove ready repair worker start identity"))?;
    let _coordination = acquire_ready_repair_coordination(sidecar)?;
    let path = ready_repair_reservation_path(sidecar);
    let mut reservation = read_ready_repair_reservation_file(&path)
        .ok_or_else(|| anyhow::anyhow!("ready repair reservation is missing or unreadable"))?;
    let project_root = clean_path_text(project_root);
    if reservation.attempt_id != attempt_id
        || reservation.project_root.as_deref() != Some(project_root.as_str())
        || reservation.profile.as_deref() != Some(sidecar.profile.as_str())
        || reservation.run_id != sidecar.run_id
        || reservation.namespace.as_deref() != Some(sidecar.namespace.as_str())
    {
        anyhow::bail!("ready repair reservation does not match the requested attempt and scope");
    }
    if process_owner_state(
        reservation.pid,
        reservation.process_start_identity.as_deref(),
    ) == ProcessOwnerState::GoneOrReused
    {
        anyhow::bail!("ready repair reservation parent is no longer the recorded process");
    }
    reservation.pid = pid;
    reservation.process_start_identity = Some(process_start_identity);
    reservation.started_at_epoch_ms = now_epoch_ms();
    reservation.adopted = true;
    crate::file_state::write_json_atomic(&path, "ready-repair-reservation", &reservation)?;
    Ok(ReadyRepairReservation {
        path,
        attempt_id: attempt_id.to_string(),
        started_at_epoch_ms: reservation
            .created_at_epoch_ms
            .unwrap_or(reservation.started_at_epoch_ms),
        armed: true,
    })
}

pub(crate) fn wait_for_ready_repair_reservation_adoption(
    sidecar: &SidecarRuntimeConfig,
    attempt_id: &str,
    pid: u32,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        {
            let _coordination = acquire_ready_repair_coordination(sidecar)?;
            let reservation =
                read_ready_repair_reservation_file(&ready_repair_reservation_path(sidecar))
                    .ok_or_else(|| {
                        anyhow::anyhow!("ready repair reservation disappeared during handoff")
                    })?;
            if reservation.attempt_id != attempt_id {
                anyhow::bail!("ready repair reservation changed attempt during handoff");
            }
            if reservation.adopted {
                if reservation.pid != pid {
                    anyhow::bail!("ready repair reservation adopted an invalid worker identity");
                }
                let expected_start_identity = reservation
                    .process_start_identity
                    .as_deref()
                    .filter(|identity| !identity.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "ready repair reservation adopted without a worker start identity"
                        )
                    })?;
                match probe_process_start_identity(reservation.pid) {
                    ProcessStartProbe::NotRunning => return Ok(()),
                    ProcessStartProbe::Running {
                        start_identity: actual,
                    } if actual == expected_start_identity => return Ok(()),
                    ProcessStartProbe::Running { .. } | ProcessStartProbe::Unknown { .. } => {
                        anyhow::bail!(
                            "ready repair reservation adopted an invalid worker identity"
                        );
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for ready repair reservation adoption");
        }
        thread::sleep(READY_REPAIR_COORDINATION_POLL);
    }
}

pub(crate) fn remove_ready_repair_reservation_if_attempt(
    sidecar: &SidecarRuntimeConfig,
    attempt_id: &str,
) {
    remove_ready_repair_reservation_if_attempt_at(
        &ready_repair_reservation_path(sidecar),
        attempt_id,
    );
}

fn remove_ready_repair_reservation_if_attempt_at(path: &Path, attempt_id: &str) {
    let Ok(_coordination) = acquire_ready_repair_coordination_at(path) else {
        return;
    };
    remove_ready_repair_reservation_if_attempt_locked(path, attempt_id);
}

fn remove_ready_repair_reservation_if_attempt_locked(path: &Path, attempt_id: &str) {
    let Some(reservation) = read_ready_repair_reservation_file(path) else {
        return;
    };
    if reservation.attempt_id == attempt_id {
        let _ = fs::remove_file(path);
    }
}

pub(crate) fn heartbeat_ready_repair_reservation(
    sidecar: &SidecarRuntimeConfig,
    attempt_id: &str,
) -> Result<bool> {
    let _coordination = acquire_ready_repair_coordination(sidecar)?;
    let path = ready_repair_reservation_path(sidecar);
    let Some(mut reservation) = read_ready_repair_reservation_file(&path) else {
        return Ok(false);
    };
    if reservation.attempt_id != attempt_id {
        return Ok(false);
    }
    reservation.started_at_epoch_ms = now_epoch_ms();
    crate::file_state::write_json_atomic(&path, "ready-repair-reservation", &reservation)?;
    Ok(true)
}

pub(crate) fn write_ready_repair_worker_result(
    sidecar: &SidecarRuntimeConfig,
    result: &ReadyRepairWorkerResult,
) -> Result<()> {
    let _coordination = acquire_ready_repair_coordination(sidecar)?;
    write_ready_repair_worker_result_locked(sidecar, result)
}

fn write_ready_repair_worker_result_locked(
    sidecar: &SidecarRuntimeConfig,
    result: &ReadyRepairWorkerResult,
) -> Result<()> {
    crate::file_state::write_json_atomic(
        &ready_repair_result_path(sidecar),
        "ready-repair-result",
        result,
    )
}

pub(crate) fn read_ready_repair_worker_result_for_sidecar(
    sidecar: &SidecarRuntimeConfig,
) -> Option<ReadyRepairWorkerResult> {
    read_ready_repair_worker_results_for_sidecar(sidecar)
        .into_iter()
        .max_by_key(|result| result.finished_at_epoch_ms)
}

fn ready_repair_terminal_result_matches(sidecar: &SidecarRuntimeConfig, attempt_id: &str) -> bool {
    read_ready_repair_worker_results_for_sidecar(sidecar)
        .into_iter()
        .any(|result| {
            result.attempt_id == attempt_id
                && matches!(
                    result.outcome.as_str(),
                    "succeeded" | "failed" | "abandoned"
                )
        })
}

fn read_ready_repair_worker_results_for_sidecar(
    sidecar: &SidecarRuntimeConfig,
) -> Vec<ReadyRepairWorkerResult> {
    fs::read_to_string(ready_repair_result_path(sidecar))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .into_iter()
        .collect()
}

pub(crate) fn try_acquire_ready_repair_lock(
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
) -> Result<ReadyRepairLockAttempt> {
    try_acquire_ready_repair_lock_at(
        ready_repair_lock_path(sidecar),
        sidecar,
        project_root,
        sidecar.run_id.as_deref(),
    )
}

pub(crate) fn try_acquire_project_ready_repair_lock(
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
) -> Result<ReadyRepairLockAttempt> {
    try_acquire_ready_repair_lock_at(
        project_ready_repair_lock_path(sidecar),
        sidecar,
        project_root,
        None,
    )
}

fn try_acquire_ready_repair_lock_at(
    path: PathBuf,
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
    active_run_id: Option<&str>,
) -> Result<ReadyRepairLockAttempt> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let started_at_epoch_ms = now_epoch_ms();
    let pid = std::process::id();
    let process_start_identity = recorded_process_start_identity(pid);
    let token = format!("{pid}:{started_at_epoch_ms}");
    let lock = ReadyRepairLockFile {
        schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
        project_root: clean_path_text(project_root),
        profile: sidecar.profile.as_str().to_string(),
        run_id: sidecar.run_id.clone(),
        namespace: sidecar.namespace.clone(),
        pid,
        process_start_identity,
        started_at_epoch_ms,
        token: token.clone(),
    };
    let content = serde_json::to_vec_pretty(&lock)?;

    match create_ready_repair_lock_file(&path, &content) {
        Ok(()) => {
            return Ok(ReadyRepairLockAttempt::Acquired(ReadyRepairLock {
                path,
                token,
            }));
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }

    let scan_all_runs = active_run_id.is_none();
    if let Some(status) = active_ready_repair_status_for_lock(project_root, sidecar, scan_all_runs)
    {
        return Ok(ReadyRepairLockAttempt::Busy(Box::new(ReadyRepairBusy {
            status: Some(status),
            lock_path: path,
            reason: None,
        })));
    }
    let stale_live_status =
        stale_live_ready_repair_status_for_lock(project_root, sidecar, scan_all_runs);

    if !ready_repair_lock_file_is_stale(&path) {
        return Ok(ReadyRepairLockAttempt::Busy(Box::new(ReadyRepairBusy {
            status: stale_live_status,
            lock_path: path,
            reason: Some("live_repair_lock".to_string()),
        })));
    }

    let _ = fs::remove_file(&path);
    match create_ready_repair_lock_file(&path, &content) {
        Ok(()) => Ok(ReadyRepairLockAttempt::Acquired(ReadyRepairLock {
            path,
            token,
        })),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            Ok(ReadyRepairLockAttempt::Busy(Box::new(ReadyRepairBusy {
                status: active_ready_repair_status_for_lock(project_root, sidecar, scan_all_runs),
                lock_path: path,
                reason: Some("lock_contention".to_string()),
            })))
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
pub(crate) fn write_ready_repair_status(
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
    phase: &str,
    started_at_epoch_ms: i64,
    pid: u32,
) -> Result<()> {
    write_ready_repair_status_for_attempt(
        sidecar,
        project_root,
        phase,
        started_at_epoch_ms,
        pid,
        None,
    )
}

pub(crate) fn write_ready_repair_status_for_attempt(
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
    phase: &str,
    started_at_epoch_ms: i64,
    pid: u32,
    attempt_id: Option<&str>,
) -> Result<()> {
    let path = ready_repair_status_path(sidecar);
    let status = ReadyRepairStatus {
        schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
        status: "repairing".to_string(),
        project_root: clean_path_text(project_root),
        profile: sidecar.profile.as_str().to_string(),
        run_id: sidecar.run_id.clone(),
        namespace: sidecar.namespace.clone(),
        phase: phase.to_string(),
        pid,
        attempt_id: attempt_id.map(str::to_string),
        process_start_identity: recorded_process_start_identity(pid),
        started_at_epoch_ms,
        updated_at_epoch_ms: now_epoch_ms(),
    };
    let _coordination = acquire_ready_repair_coordination(sidecar)?;
    crate::file_state::write_json_atomic(&path, "ready-repair-status", &status)
}

#[cfg(test)]
pub(crate) fn clear_ready_repair_status(
    sidecar: &SidecarRuntimeConfig,
    started_at_epoch_ms: i64,
    pid: u32,
) {
    clear_ready_repair_status_for_attempt(sidecar, started_at_epoch_ms, pid, None);
}

pub(crate) fn clear_ready_repair_status_for_attempt(
    sidecar: &SidecarRuntimeConfig,
    started_at_epoch_ms: i64,
    pid: u32,
    attempt_id: Option<&str>,
) {
    let Ok(_coordination) = acquire_ready_repair_coordination(sidecar) else {
        return;
    };
    let path = ready_repair_status_path(sidecar);
    let Some(status) = read_ready_repair_status_file(&path) else {
        return;
    };
    if status.pid == pid
        && status.started_at_epoch_ms == started_at_epoch_ms
        && attempt_id.is_none_or(|attempt_id| status.attempt_id.as_deref() == Some(attempt_id))
    {
        let _ = fs::remove_file(path);
    }
}

pub(crate) fn clear_ready_repair_status_by_attempt(
    sidecar: &SidecarRuntimeConfig,
    attempt_id: &str,
) {
    let Ok(_coordination) = acquire_ready_repair_coordination(sidecar) else {
        return;
    };
    let path = ready_repair_status_path(sidecar);
    let Some(status) = read_ready_repair_status_file(&path) else {
        return;
    };
    if status.attempt_id.as_deref() == Some(attempt_id) {
        let _ = fs::remove_file(path);
    }
}

pub(crate) fn active_ready_repair_status(
    project_root: &Path,
    run_id: Option<&str>,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths(project_root, run_id)
        .into_iter()
        .filter_map(|path| read_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

pub(crate) fn active_ready_repair_status_for_sidecar(
    project_root: &Path,
    default_sidecar: &SidecarRuntimeConfig,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths_for_sidecar(default_sidecar)
        .into_iter()
        .filter_map(|path| read_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

fn active_ready_repair_status_for_lock(
    project_root: &Path,
    sidecar: &SidecarRuntimeConfig,
    scan_all_runs: bool,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths_for_lock(sidecar, scan_all_runs)
        .into_iter()
        .filter_map(|path| read_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

fn stale_live_ready_repair_status_for_lock(
    project_root: &Path,
    sidecar: &SidecarRuntimeConfig,
    scan_all_runs: bool,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths_for_lock(sidecar, scan_all_runs)
        .into_iter()
        .filter_map(|path| read_stale_live_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

pub(crate) fn abandoned_ready_repair_status(
    project_root: &Path,
    run_id: Option<&str>,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths(project_root, run_id)
        .into_iter()
        .filter_map(|path| read_abandoned_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

pub(crate) fn abandoned_ready_repair_status_for_sidecar(
    project_root: &Path,
    default_sidecar: &SidecarRuntimeConfig,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths_for_sidecar(default_sidecar)
        .into_iter()
        .filter_map(|path| read_abandoned_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

pub(crate) fn stale_live_ready_repair_status_for_sidecar(
    project_root: &Path,
    default_sidecar: &SidecarRuntimeConfig,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths_for_sidecar(default_sidecar)
        .into_iter()
        .filter_map(|path| read_stale_live_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

pub(crate) fn stale_live_ready_repair_status(
    project_root: &Path,
    run_id: Option<&str>,
) -> Option<ReadyRepairStatus> {
    let now = now_epoch_ms();
    ready_repair_status_paths(project_root, run_id)
        .into_iter()
        .filter_map(|path| read_stale_live_ready_repair_status(&path, project_root, now))
        .max_by_key(|status| status.updated_at_epoch_ms)
}

pub(crate) fn cleanup_abandoned_ready_repair_status(
    project_root: &Path,
    run_id: Option<&str>,
) -> Vec<ReadyRepairCleanup> {
    let default_sidecar = crate::sidecar_runtime::for_project_with_run_id(
        project_root,
        SidecarProfile::Agent,
        run_id.or(Some(DEFAULT_AGENT_RUN_ID)),
    );
    cleanup_abandoned_ready_repair_status_from_paths(
        project_root,
        &default_sidecar,
        ready_repair_status_paths(project_root, run_id),
    )
}

pub(crate) fn cleanup_abandoned_ready_repair_status_for_sidecar(
    project_root: &Path,
    default_sidecar: &SidecarRuntimeConfig,
) -> Vec<ReadyRepairCleanup> {
    cleanup_abandoned_ready_repair_status_from_paths(
        project_root,
        default_sidecar,
        ready_repair_status_paths_for_sidecar(default_sidecar),
    )
}

fn cleanup_abandoned_ready_repair_status_from_paths(
    project_root: &Path,
    default_sidecar: &SidecarRuntimeConfig,
    paths: Vec<PathBuf>,
) -> Vec<ReadyRepairCleanup> {
    let now = now_epoch_ms();
    paths
        .into_iter()
        .filter_map(|path| {
            let observed = read_abandoned_ready_repair_status(&path, project_root, now)?;
            let run_id = observed.run_id.as_deref().unwrap_or(DEFAULT_AGENT_RUN_ID);
            let sidecar = default_sidecar.with_profile_and_run_id(
                Some(project_root),
                SidecarProfile::Agent,
                Some(run_id),
            );
            let observed_stale_locks = ready_repair_lock_paths_for_sidecar(&sidecar)
                .into_iter()
                .filter_map(|lock_path| {
                    let lock = read_ready_repair_lock_file(&lock_path)?;
                    ready_repair_lock_file_is_stale(&lock_path).then_some((lock_path, lock))
                })
                .collect::<Vec<_>>();
            let _coordination = acquire_ready_repair_coordination(&sidecar).ok()?;
            let status = read_ready_repair_status_file(&path)?;
            if status != observed {
                return None;
            }
            let result = ReadyRepairWorkerResult {
                schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
                attempt_id: status.attempt_id.clone().unwrap_or_else(|| {
                    format!("legacy:{}:{}", status.pid, status.started_at_epoch_ms)
                }),
                project_root: status.project_root.clone(),
                profile: status.profile.clone(),
                run_id: status.run_id.clone(),
                namespace: status.namespace.clone(),
                pid: status.pid,
                started_at_epoch_ms: status.started_at_epoch_ms,
                finished_at_epoch_ms: now,
                outcome: "abandoned".to_string(),
                auto_retry_fingerprint: None,
                exit_code: None,
                wait_error: Some(
                    "repair worker process exited without recording a terminal result".to_string(),
                ),
                terminal_envelope: Some(abandoned_repair_envelope(
                    "repair worker process exited without recording a terminal result",
                )),
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
            };
            if !ready_repair_terminal_result_matches(&sidecar, &result.attempt_id)
                && write_ready_repair_worker_result_locked(&sidecar, &result).is_err()
            {
                return None;
            }
            let removed_status_path = fs::remove_file(&path).is_ok();
            let mut removed_lock_paths = Vec::new();
            for (lock_path, observed_lock) in observed_stale_locks {
                if read_ready_repair_lock_file(&lock_path).as_ref() == Some(&observed_lock)
                    && fs::remove_file(&lock_path).is_ok()
                {
                    removed_lock_paths.push(lock_path);
                }
            }
            Some(ReadyRepairCleanup {
                status,
                status_path: path,
                removed_status_path,
                removed_lock_paths,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn ready_repair_status_cache_fingerprint(project_root: &Path) -> String {
    let sidecar = crate::sidecar_runtime::for_project_with_run_id(
        project_root,
        SidecarProfile::Agent,
        Some(DEFAULT_AGENT_RUN_ID),
    );
    ready_repair_status_cache_fingerprint_for_sidecar(&sidecar)
}

pub(crate) fn ready_repair_status_cache_fingerprint_for_sidecar(
    sidecar: &SidecarRuntimeConfig,
) -> String {
    ready_repair_status_cache_fingerprint_for_paths(ready_repair_status_paths_for_sidecar(sidecar))
}

fn ready_repair_status_cache_fingerprint_for_paths(paths: Vec<PathBuf>) -> String {
    paths
        .into_iter()
        .flat_map(|path| {
            [
                path_fingerprint(&path),
                path_fingerprint(&path.with_file_name(READY_REPAIR_RESULT_FILE)),
                path_fingerprint(&path.with_file_name(READY_REPAIR_ENQUEUE_LOCK_FILE)),
            ]
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn create_ready_repair_lock_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    crate::file_state::write_synced_new_file(path, content)
}

fn ready_repair_lock_path(sidecar: &SidecarRuntimeConfig) -> PathBuf {
    sidecar
        .layout
        .state_file
        .with_file_name(READY_REPAIR_LOCK_FILE)
}

fn project_ready_repair_lock_path(sidecar: &SidecarRuntimeConfig) -> PathBuf {
    let Some(run_id) = sidecar.run_id.as_deref() else {
        return ready_repair_lock_path(sidecar).with_file_name(READY_REPAIR_PROJECT_LOCK_FILE);
    };
    let Some(namespace_prefix) = sidecar.namespace.strip_suffix(run_id) else {
        return ready_repair_lock_path(sidecar).with_file_name(READY_REPAIR_PROJECT_LOCK_FILE);
    };
    let Some(namespace_dir) = sidecar.layout.state_file.parent() else {
        return ready_repair_lock_path(sidecar).with_file_name(READY_REPAIR_PROJECT_LOCK_FILE);
    };
    let Some(sidecars_root) = namespace_dir.parent() else {
        return ready_repair_lock_path(sidecar).with_file_name(READY_REPAIR_PROJECT_LOCK_FILE);
    };
    sidecars_root
        .join(format!("{namespace_prefix}project"))
        .join(READY_REPAIR_PROJECT_LOCK_FILE)
}

fn ready_repair_status_path(sidecar: &SidecarRuntimeConfig) -> PathBuf {
    sidecar
        .layout
        .state_file
        .with_file_name(READY_REPAIR_STATUS_FILE)
}

fn ready_repair_result_path(sidecar: &SidecarRuntimeConfig) -> PathBuf {
    sidecar
        .layout
        .state_file
        .with_file_name(READY_REPAIR_RESULT_FILE)
}

fn ready_repair_reservation_path(sidecar: &SidecarRuntimeConfig) -> PathBuf {
    sidecar
        .layout
        .state_file
        .with_file_name(READY_REPAIR_ENQUEUE_LOCK_FILE)
}

fn read_ready_repair_reservation_file(path: &Path) -> Option<ReadyRepairReservationFile> {
    crate::file_state::read_json(path)
}

fn ready_repair_reservation_is_stale(path: &Path) -> bool {
    if let Some(reservation) = read_ready_repair_reservation_file(path) {
        return match process_owner_state(
            reservation.pid,
            reservation.process_start_identity.as_deref(),
        ) {
            ProcessOwnerState::Matching => false,
            ProcessOwnerState::GoneOrReused => true,
            ProcessOwnerState::Unknown => crate::file_state::file_modified_age_exceeds(
                path,
                READY_REPAIR_LOCK_STALE_TTL,
                now_epoch_ms(),
            ),
        };
    }
    crate::file_state::file_modified_age_exceeds(path, READY_REPAIR_LOCK_STALE_TTL, now_epoch_ms())
}

fn ready_repair_lock_file_is_stale(path: &Path) -> bool {
    if let Some(lock) = read_ready_repair_lock_file(path) {
        return process_owner_state(lock.pid, lock.process_start_identity.as_deref())
            == ProcessOwnerState::GoneOrReused;
    }
    let now = now_epoch_ms();
    crate::file_state::file_modified_age_exceeds(path, READY_REPAIR_LOCK_STALE_TTL, now)
}

fn read_ready_repair_status(
    path: &Path,
    project_root: &Path,
    now_epoch_ms: i64,
) -> Option<ReadyRepairStatus> {
    let status = read_ready_repair_status_file(path)?;
    if status.schema_version != READY_REPAIR_STATUS_SCHEMA_VERSION
        || status.status != "repairing"
        || status.profile != SidecarProfile::Agent.as_str()
        || !codestory_workspace::same_workspace_path(Path::new(&status.project_root), project_root)
    {
        return None;
    }
    let age_ms = now_epoch_ms.saturating_sub(status.updated_at_epoch_ms);
    if age_ms > READY_REPAIR_STATUS_TTL.as_millis() as i64 {
        return None;
    }
    if process_owner_state(status.pid, status.process_start_identity.as_deref())
        == ProcessOwnerState::GoneOrReused
    {
        return None;
    }
    Some(status)
}

fn read_abandoned_ready_repair_status(
    path: &Path,
    project_root: &Path,
    _now_epoch_ms: i64,
) -> Option<ReadyRepairStatus> {
    let status = read_ready_repair_status_file(path)?;
    if status.schema_version != READY_REPAIR_STATUS_SCHEMA_VERSION
        || status.status != "repairing"
        || status.profile != SidecarProfile::Agent.as_str()
        || !codestory_workspace::same_workspace_path(Path::new(&status.project_root), project_root)
    {
        return None;
    }
    if process_owner_state(status.pid, status.process_start_identity.as_deref())
        != ProcessOwnerState::GoneOrReused
    {
        return None;
    }
    Some(status)
}

fn read_stale_live_ready_repair_status(
    path: &Path,
    project_root: &Path,
    now_epoch_ms: i64,
) -> Option<ReadyRepairStatus> {
    let status = read_ready_repair_status_file(path)?;
    if status.schema_version != READY_REPAIR_STATUS_SCHEMA_VERSION
        || status.status != "repairing"
        || status.profile != SidecarProfile::Agent.as_str()
        || !codestory_workspace::same_workspace_path(Path::new(&status.project_root), project_root)
    {
        return None;
    }
    let age_ms = now_epoch_ms.saturating_sub(status.updated_at_epoch_ms);
    if age_ms <= READY_REPAIR_STATUS_TTL.as_millis() as i64 {
        return None;
    }
    if process_owner_state(status.pid, status.process_start_identity.as_deref())
        == ProcessOwnerState::GoneOrReused
    {
        return None;
    }
    Some(status)
}

pub(crate) fn process_is_running(pid: u32) -> bool {
    process_owner_state(pid, None) != ProcessOwnerState::GoneOrReused
}

pub(crate) fn process_owner_state(
    pid: u32,
    expected_start_identity: Option<&str>,
) -> ProcessOwnerState {
    classify_process_owner_state(&probe_process_start_identity(pid), expected_start_identity)
}

pub(crate) fn recorded_process_start_identity(pid: u32) -> Option<String> {
    match probe_process_start_identity(pid) {
        ProcessStartProbe::Running { start_identity } => Some(start_identity),
        ProcessStartProbe::NotRunning | ProcessStartProbe::Unknown { .. } => None,
    }
}

fn ready_repair_lock_paths_for_sidecar(sidecar: &SidecarRuntimeConfig) -> Vec<PathBuf> {
    vec![
        ready_repair_lock_path(sidecar),
        project_ready_repair_lock_path(sidecar),
    ]
}

fn read_ready_repair_lock_file(path: &Path) -> Option<ReadyRepairLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn read_ready_repair_status_file(path: &Path) -> Option<ReadyRepairStatus> {
    crate::file_state::read_json(path)
}

fn ready_repair_status_paths(project_root: &Path, run_id: Option<&str>) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    if let Some(run_id) = run_id {
        let sidecar = crate::sidecar_runtime::for_project_with_run_id(
            project_root,
            SidecarProfile::Agent,
            Some(run_id),
        );
        paths.insert(ready_repair_status_path(&sidecar));
        return paths.into_iter().collect();
    }

    let default_sidecar = crate::sidecar_runtime::for_project_with_run_id(
        project_root,
        SidecarProfile::Agent,
        Some(DEFAULT_AGENT_RUN_ID),
    );
    ready_repair_status_paths_for_sidecar(&default_sidecar)
}

fn ready_repair_status_paths_for_sidecar(default_sidecar: &SidecarRuntimeConfig) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    paths.insert(ready_repair_status_path(default_sidecar));

    if let Some((sidecars_root, namespace_prefix)) = agent_sidecars_scan_root(default_sidecar)
        && let Ok(entries) = fs::read_dir(sidecars_root)
    {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let namespace = entry.file_name().to_string_lossy().to_string();
            if namespace.starts_with(&namespace_prefix) {
                paths.insert(entry.path().join(READY_REPAIR_STATUS_FILE));
            }
        }
    }

    paths.into_iter().collect()
}

fn ready_repair_status_paths_for_lock(
    sidecar: &SidecarRuntimeConfig,
    scan_all_runs: bool,
) -> Vec<PathBuf> {
    if scan_all_runs {
        ready_repair_status_paths_for_sidecar(sidecar)
    } else {
        vec![ready_repair_status_path(sidecar)]
    }
}

fn agent_sidecars_scan_root(sidecar: &SidecarRuntimeConfig) -> Option<(PathBuf, String)> {
    let namespace_prefix = sidecar.namespace.strip_suffix(DEFAULT_AGENT_RUN_ID)?;
    let namespace_dir = sidecar.layout.state_file.parent()?;
    let sidecars_root = namespace_dir.parent()?;
    Some((sidecars_root.to_path_buf(), namespace_prefix.to_string()))
}

fn clean_path_text(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn path_fingerprint(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) => {
            let modified_ms = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or_default();
            format!("{}:{}:{}", path.display(), metadata.len(), modified_ms)
        }
        Err(_) => format!("{}:missing", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_retrieval::SidecarLayout;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn test_sidecar(root: &Path) -> SidecarRuntimeConfig {
        test_sidecar_with_run_id(root, "test-proof")
    }

    fn test_sidecar_with_run_id(root: &Path, run_id: &str) -> SidecarRuntimeConfig {
        let namespace = format!("codestory-agent-{run_id}");
        SidecarRuntimeConfig {
            project_identity: None,
            layout: SidecarLayout {
                lexical_data_dir: root.join("lexical"),
                semantic_data_dir: root.join("semantic"),
                scip_artifacts_root: root.join("scip"),
                state_file: root.join(&namespace).join("retrieval-sidecars.json"),
            },
            profile: SidecarProfile::Agent,
            run_id: Some(run_id.to_string()),
            namespace: namespace.clone(),
            embed_http_port: 8080,
            cleanup_command: "codestory-cli retrieval down".to_string(),
            ..crate::sidecar_runtime::local()
        }
    }

    #[test]
    fn ready_repair_reservation_transfers_exact_attempt_and_scope() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let mut parent = try_reserve_ready_repair(&sidecar, project.path())
            .expect("reserve")
            .expect("reservation acquired");
        let attempt_id = parent.attempt_id().to_string();
        let serialized = fs::read_to_string(ready_repair_reservation_path(&sidecar))
            .expect("serialized reservation");
        assert!(serialized.contains("\"token\""));
        assert!(!serialized.contains("\"attempt_id\""));
        parent.disarm();

        let adopted = adopt_ready_repair_reservation(&sidecar, project.path(), &attempt_id)
            .expect("adopt exact reservation");
        let stored = read_ready_repair_reservation_file(&ready_repair_reservation_path(&sidecar))
            .expect("stored adopted reservation");

        assert_eq!(stored.attempt_id, attempt_id);
        assert!(stored.adopted);
        assert_eq!(stored.pid, std::process::id());
        wait_for_ready_repair_reservation_adoption(
            &sidecar,
            &attempt_id,
            std::process::id(),
            Duration::ZERO,
        )
        .expect("parent observes exact adopted worker");
        drop(adopted);
        assert!(!ready_repair_reservation_path(&sidecar).exists());
    }

    #[test]
    fn ready_repair_reservation_rejects_scope_mismatch() {
        let project = tempfile::tempdir().expect("project");
        let other = tempfile::tempdir().expect("other project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let reservation = try_reserve_ready_repair(&sidecar, project.path())
            .expect("reserve")
            .expect("reservation acquired");

        let error =
            adopt_ready_repair_reservation(&sidecar, other.path(), reservation.attempt_id())
                .expect_err("scope mismatch must fail");

        assert!(error.to_string().contains("does not match"));
        assert!(ready_repair_reservation_path(&sidecar).exists());
    }

    #[test]
    fn aged_malformed_reservation_is_recoverable() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_reservation_path(&sidecar);
        fs::create_dir_all(path.parent().expect("reservation parent")).expect("state dir");
        fs::write(&path, "{not-json").expect("malformed reservation");
        let old = SystemTime::now()
            .checked_sub(READY_REPAIR_LOCK_STALE_TTL + Duration::from_secs(1))
            .expect("old file time");
        fs::File::options()
            .write(true)
            .open(&path)
            .expect("open reservation")
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .expect("age reservation");

        let replacement = try_reserve_ready_repair(&sidecar, project.path())
            .expect("replace malformed reservation")
            .expect("replacement acquired");

        assert_ne!(replacement.attempt_id(), "");
        assert!(read_ready_repair_reservation_file(&path).is_some());
    }

    #[test]
    fn stale_reservation_does_not_overwrite_matching_terminal_result() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_reservation_path(&sidecar);
        fs::create_dir_all(path.parent().expect("reservation parent")).expect("state dir");
        let attempt_id = "finished-attempt";
        crate::file_state::write_json_atomic(
            &path,
            "stale-reservation",
            &ReadyRepairReservationFile {
                schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
                pid: u32::MAX,
                process_start_identity: None,
                started_at_epoch_ms: 100,
                created_at_epoch_ms: Some(100),
                attempt_id: attempt_id.to_string(),
                project_root: Some(clean_path_text(project.path())),
                profile: Some("agent".to_string()),
                run_id: sidecar.run_id.clone(),
                namespace: Some(sidecar.namespace.clone()),
                adopted: true,
            },
        )
        .expect("stale reservation");
        let terminal = ReadyRepairWorkerResult {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            attempt_id: attempt_id.to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: sidecar.run_id.clone(),
            namespace: sidecar.namespace.clone(),
            pid: u32::MAX,
            started_at_epoch_ms: 100,
            finished_at_epoch_ms: 200,
            outcome: "succeeded".to_string(),
            auto_retry_fingerprint: None,
            exit_code: Some(0),
            wait_error: None,
            terminal_envelope: None,
            stdout_tail: "done".to_string(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
        write_ready_repair_worker_result(&sidecar, &terminal).expect("terminal result");

        let _replacement = try_reserve_ready_repair(&sidecar, project.path())
            .expect("replace stale reservation")
            .expect("replacement acquired");
        let preserved: ReadyRepairWorkerResult =
            crate::file_state::read_json(&ready_repair_result_path(&sidecar))
                .expect("preserved terminal result");

        assert_eq!(preserved, terminal);
    }

    #[test]
    fn concurrent_stale_reservation_reclaim_has_one_winner() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_reservation_path(&sidecar);
        fs::create_dir_all(path.parent().expect("reservation parent")).expect("state dir");
        crate::file_state::write_json_atomic(
            &path,
            "stale-reservation",
            &ReadyRepairReservationFile {
                schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
                pid: u32::MAX,
                process_start_identity: None,
                started_at_epoch_ms: 100,
                created_at_epoch_ms: Some(100),
                attempt_id: "stale-attempt".to_string(),
                project_root: Some(clean_path_text(project.path())),
                profile: Some("agent".to_string()),
                run_id: sidecar.run_id.clone(),
                namespace: Some(sidecar.namespace.clone()),
                adopted: true,
            },
        )
        .expect("stale reservation");
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let barrier = Arc::clone(&barrier);
            let sidecar = sidecar.clone();
            let project_root = project.path().to_path_buf();
            workers.push(thread::spawn(move || {
                barrier.wait();
                try_reserve_ready_repair(&sidecar, &project_root).expect("reclaim attempt")
            }));
        }
        barrier.wait();

        let reservations = workers
            .into_iter()
            .map(|worker| worker.join().expect("reclaimer"))
            .collect::<Vec<_>>();

        assert_eq!(
            reservations
                .iter()
                .filter(|reservation| reservation.is_some())
                .count(),
            1,
            "coordination must allow exactly one replacement attempt"
        );
        let stored = read_ready_repair_reservation_file(&path).expect("winning reservation");
        assert_ne!(stored.attempt_id, "stale-attempt");
    }

    #[test]
    fn reservation_heartbeat_preserves_original_start_time() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let reservation = try_reserve_ready_repair(&sidecar, project.path())
            .expect("reserve")
            .expect("reservation acquired");
        let before = read_ready_repair_reservation_file(&ready_repair_reservation_path(&sidecar))
            .expect("reservation before heartbeat");
        thread::sleep(Duration::from_millis(2));

        assert!(
            heartbeat_ready_repair_reservation(&sidecar, reservation.attempt_id())
                .expect("heartbeat")
        );
        let after = read_ready_repair_reservation_file(&ready_repair_reservation_path(&sidecar))
            .expect("reservation after heartbeat");

        assert_eq!(after.created_at_epoch_ms, before.created_at_epoch_ms);
        assert!(after.started_at_epoch_ms >= before.started_at_epoch_ms);
    }

    #[test]
    fn ready_repair_worker_result_round_trips() {
        let _env_lock = crate::config::config_env_test_lock();
        let project = tempfile::tempdir().expect("project");
        let sidecar = crate::sidecar_runtime::for_project_with_run_id(
            project.path(),
            SidecarProfile::Agent,
            Some(DEFAULT_AGENT_RUN_ID),
        );
        let result = ReadyRepairWorkerResult {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            attempt_id: "test-attempt".to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some(DEFAULT_AGENT_RUN_ID.to_string()),
            namespace: sidecar.namespace.clone(),
            pid: 42,
            started_at_epoch_ms: 100,
            finished_at_epoch_ms: 200,
            outcome: "failed".to_string(),
            auto_retry_fingerprint: None,
            exit_code: Some(17),
            wait_error: None,
            terminal_envelope: Some(CommandFailureEnvelope::new(ApiError::internal(
                "worker failed",
            ))),
            stdout_tail: "stdout".to_string(),
            stderr_tail: "stderr".to_string(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let fingerprint_before = ready_repair_status_cache_fingerprint(project.path());

        write_ready_repair_worker_result(&sidecar, &result).expect("write worker result");

        assert_ne!(
            ready_repair_status_cache_fingerprint(project.path()),
            fingerprint_before,
            "terminal result should invalidate cached MCP status"
        );
        assert_eq!(
            read_ready_repair_worker_result_for_sidecar(&sidecar),
            Some(result)
        );
        let _ = fs::remove_dir_all(
            sidecar
                .layout
                .state_file
                .parent()
                .expect("sidecar state parent"),
        );
    }

    #[test]
    fn concurrent_terminal_write_and_abandoned_cleanup_preserve_terminal_result() {
        let _env_lock = crate::config::config_env_test_lock();
        let project = tempfile::tempdir().expect("project");
        let sidecar = crate::sidecar_runtime::for_project_with_run_id(
            project.path(),
            SidecarProfile::Agent,
            Some(DEFAULT_AGENT_RUN_ID),
        );
        let status_path = ready_repair_status_path(&sidecar);
        fs::create_dir_all(status_path.parent().expect("status parent")).expect("state dir");
        let attempt_id = "terminal-race";
        let status = ReadyRepairStatus {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            status: "repairing".to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some(DEFAULT_AGENT_RUN_ID.to_string()),
            namespace: sidecar.namespace.clone(),
            phase: "starting".to_string(),
            pid: u32::MAX,
            attempt_id: Some(attempt_id.to_string()),
            process_start_identity: None,
            started_at_epoch_ms: 100,
            updated_at_epoch_ms: 100,
        };
        fs::write(
            &status_path,
            serde_json::to_string(&status).expect("status json"),
        )
        .expect("abandoned status");
        let terminal = ReadyRepairWorkerResult {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            attempt_id: attempt_id.to_string(),
            project_root: status.project_root.clone(),
            profile: status.profile.clone(),
            run_id: status.run_id.clone(),
            namespace: status.namespace.clone(),
            pid: status.pid,
            started_at_epoch_ms: status.started_at_epoch_ms,
            finished_at_epoch_ms: 200,
            outcome: "succeeded".to_string(),
            auto_retry_fingerprint: None,
            exit_code: Some(0),
            wait_error: None,
            terminal_envelope: None,
            stdout_tail: "success".to_string(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let barrier = Arc::new(Barrier::new(3));
        let writer = {
            let barrier = Arc::clone(&barrier);
            let sidecar = sidecar.clone();
            let terminal = terminal.clone();
            crate::sidecar_runtime::spawn_with_cache_access(move || {
                barrier.wait();
                write_ready_repair_worker_result(&sidecar, &terminal).expect("terminal write");
            })
        };
        let cleaner = {
            let barrier = Arc::clone(&barrier);
            let project_root = project.path().to_path_buf();
            crate::sidecar_runtime::spawn_with_cache_access(move || {
                barrier.wait();
                cleanup_abandoned_ready_repair_status(&project_root, Some(DEFAULT_AGENT_RUN_ID));
            })
        };
        barrier.wait();
        writer.join().expect("terminal writer");
        cleaner.join().expect("abandoned cleaner");

        assert_eq!(
            read_ready_repair_worker_result_for_sidecar(&sidecar),
            Some(terminal),
            "terminal success must win regardless of cleanup ordering"
        );
        let _ = fs::remove_dir_all(
            sidecar
                .layout
                .state_file
                .parent()
                .expect("sidecar state parent"),
        );
    }

    #[test]
    fn repair_status_file_round_trips_current_phase_and_clears() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let started_at = now_epoch_ms();
        let pid = std::process::id();

        write_ready_repair_status(
            &sidecar,
            project.path(),
            "Semantic finalize",
            started_at,
            pid,
        )
        .expect("write repair status");
        let path = ready_repair_status_path(&sidecar);
        let status = read_ready_repair_status(&path, project.path(), now_epoch_ms())
            .expect("active repair status");

        assert_eq!(status.status, "repairing");
        assert_eq!(status.phase, "Semantic finalize");
        assert_eq!(status.run_id.as_deref(), Some("test-proof"));
        assert_eq!(status.namespace, "codestory-agent-test-proof");
        assert!(status.process_start_identity.is_some());

        clear_ready_repair_status(&sidecar, started_at, pid);
        assert!(
            !path.exists(),
            "drop cleanup should remove the matching repair state file"
        );
    }

    #[test]
    fn ready_repair_status_publication_never_exposes_partial_json() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_status_path(&sidecar);
        let started_at = now_epoch_ms();
        let pid = std::process::id();
        write_ready_repair_status(&sidecar, project.path(), "initial", started_at, pid)
            .expect("initial repair status");

        let reader_path = path.clone();
        let reader_started = Arc::new(AtomicBool::new(false));
        let writer_done = Arc::new(AtomicBool::new(false));
        let reader = {
            let reader_started = Arc::clone(&reader_started);
            let writer_done = Arc::clone(&writer_done);
            thread::spawn(move || {
                reader_started.store(true, Ordering::Release);
                let mut reads = 0;
                while !writer_done.load(Ordering::Acquire) {
                    let status = read_ready_repair_status_file(&reader_path)
                        .expect("complete repair status json");
                    assert_eq!(status.schema_version, READY_REPAIR_STATUS_SCHEMA_VERSION);
                    assert_eq!(status.pid, pid);
                    reads += 1;
                    thread::yield_now();
                }
                reads
            })
        };
        while !reader_started.load(Ordering::Acquire) {
            thread::yield_now();
        }

        let payload = "x".repeat(32 * 1024);
        for iteration in 0..200 {
            write_ready_repair_status(
                &sidecar,
                project.path(),
                &format!("iteration-{iteration}-{payload}"),
                started_at,
                pid,
            )
            .expect("replace repair status");
            thread::yield_now();
        }
        writer_done.store(true, Ordering::Release);

        assert!(reader.join().expect("repair status reader") > 0);
        assert!(read_ready_repair_status_file(&path).is_some());
    }

    #[test]
    fn repair_lock_is_single_flight_and_clears_on_drop() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());

        let lock = match try_acquire_ready_repair_lock(&sidecar, project.path())
            .expect("first lock attempt")
        {
            ReadyRepairLockAttempt::Acquired(lock) => lock,
            ReadyRepairLockAttempt::Busy(busy) => {
                panic!(
                    "first lock should be acquired, got busy at {:?}",
                    busy.lock_path
                )
            }
        };

        match try_acquire_ready_repair_lock(&sidecar, project.path()).expect("second lock attempt")
        {
            ReadyRepairLockAttempt::Busy(busy) => {
                assert!(busy.status.is_none());
                assert!(busy.lock_path.exists());
            }
            ReadyRepairLockAttempt::Acquired(_) => panic!("second lock must not be acquired"),
        }

        drop(lock);
        match try_acquire_ready_repair_lock(&sidecar, project.path()).expect("third lock attempt") {
            ReadyRepairLockAttempt::Acquired(_) => {}
            ReadyRepairLockAttempt::Busy(busy) => {
                panic!(
                    "lock should be reusable after drop, got busy at {:?}",
                    busy.lock_path
                )
            }
        }
    }

    #[test]
    fn live_pid_repair_lock_does_not_age_out() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_lock_path(&sidecar);
        fs::create_dir_all(path.parent().expect("repair lock parent")).expect("state dir");
        let now = now_epoch_ms();
        let pid = std::process::id();
        let lock = ReadyRepairLockFile {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("test-proof".to_string()),
            namespace: "codestory-agent-test-proof".to_string(),
            pid,
            process_start_identity: None,
            started_at_epoch_ms: now - READY_REPAIR_LOCK_STALE_TTL.as_millis() as i64 - 1,
            token: format!("{pid}:{now}"),
        };
        fs::write(&path, serde_json::to_string(&lock).expect("lock json"))
            .expect("write live-pid lock");

        match try_acquire_ready_repair_lock(&sidecar, project.path()).expect("lock attempt") {
            ReadyRepairLockAttempt::Busy(busy) => {
                assert!(busy.status.is_none());
                assert_eq!(busy.lock_path, path);
            }
            ReadyRepairLockAttempt::Acquired(_) => {
                panic!("live-pid repair lock must remain busy regardless of age")
            }
        }
    }

    #[test]
    fn project_repair_lock_serializes_different_run_ids() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let first = test_sidecar_with_run_id(state.path(), "first");
        let second = test_sidecar_with_run_id(state.path(), "second");

        let project_lock = match try_acquire_project_ready_repair_lock(&first, project.path())
            .expect("first project lock")
        {
            ReadyRepairLockAttempt::Acquired(lock) => lock,
            ReadyRepairLockAttempt::Busy(busy) => {
                panic!("first project lock should be acquired, got {busy:?}")
            }
        };

        match try_acquire_project_ready_repair_lock(&second, project.path())
            .expect("second project lock")
        {
            ReadyRepairLockAttempt::Busy(busy) => {
                assert!(busy.status.is_none());
                assert!(busy.lock_path.exists());
            }
            ReadyRepairLockAttempt::Acquired(_) => {
                panic!("same project with different run id must be serialized")
            }
        }

        match try_acquire_ready_repair_lock(&second, project.path()).expect("namespace lock") {
            ReadyRepairLockAttempt::Acquired(namespace_lock) => drop(namespace_lock),
            ReadyRepairLockAttempt::Busy(busy) => {
                panic!("namespace lock should remain separate, got {busy:?}")
            }
        }

        drop(project_lock);
        match try_acquire_project_ready_repair_lock(&second, project.path())
            .expect("project lock after drop")
        {
            ReadyRepairLockAttempt::Acquired(_) => {}
            ReadyRepairLockAttempt::Busy(busy) => {
                panic!("project lock should be reusable after drop, got {busy:?}")
            }
        }
    }

    #[test]
    fn stale_repair_status_expires() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_status_path(&sidecar);
        fs::create_dir_all(path.parent().expect("repair status parent")).expect("state dir");
        let now = now_epoch_ms();
        let stale = ReadyRepairStatus {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            status: "repairing".to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("test-proof".to_string()),
            namespace: "codestory-agent-test-proof".to_string(),
            phase: "embeddings".to_string(),
            pid: 4242,
            attempt_id: None,
            process_start_identity: None,
            started_at_epoch_ms: now - 60_000,
            updated_at_epoch_ms: now - READY_REPAIR_STATUS_TTL.as_millis() as i64 - 1,
        };
        fs::write(&path, serde_json::to_string(&stale).expect("status json"))
            .expect("write stale status");

        assert_eq!(
            read_ready_repair_status(&path, project.path(), now),
            None,
            "stale repair state must not mask final readiness"
        );
    }

    #[test]
    fn dead_pid_repair_status_is_abandoned_immediately() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_status_path(&sidecar);
        fs::create_dir_all(path.parent().expect("repair status parent")).expect("state dir");
        let now = now_epoch_ms();
        let dead_pid = u32::MAX;
        let status = ReadyRepairStatus {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            status: "repairing".to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("test-proof".to_string()),
            namespace: "codestory-agent-test-proof".to_string(),
            phase: "graph artifact".to_string(),
            pid: dead_pid,
            attempt_id: None,
            process_start_identity: None,
            started_at_epoch_ms: now,
            updated_at_epoch_ms: now,
        };
        fs::write(&path, serde_json::to_string(&status).expect("status json"))
            .expect("write dead-pid status");

        assert_eq!(
            read_ready_repair_status(&path, project.path(), now),
            None,
            "dead repair pid must not block a fresh MCP repair"
        );
        assert_eq!(
            read_abandoned_ready_repair_status(&path, project.path(), now),
            Some(status),
            "dead repair pid should still be reported as abandoned evidence"
        );
    }

    #[test]
    fn live_pid_stale_repair_status_is_preserved_and_reported_busy() {
        let project = tempfile::tempdir().expect("project");
        let sidecar = crate::sidecar_runtime::for_project_with_run_id(
            project.path(),
            SidecarProfile::Agent,
            Some("test-proof"),
        );
        let status_path = ready_repair_status_path(&sidecar);
        let lock_path = ready_repair_lock_path(&sidecar);
        fs::create_dir_all(status_path.parent().expect("repair status parent")).expect("state dir");
        let now = now_epoch_ms();
        let old = now - READY_REPAIR_STATUS_TTL.as_millis() as i64 - 1_000;
        let pid = std::process::id();
        let status = ReadyRepairStatus {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            status: "repairing".to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("test-proof".to_string()),
            namespace: "codestory-agent-test-proof".to_string(),
            phase: "Embedding documents".to_string(),
            pid,
            attempt_id: None,
            process_start_identity: None,
            started_at_epoch_ms: old,
            updated_at_epoch_ms: old,
        };
        fs::write(
            &status_path,
            serde_json::to_string(&status).expect("status json"),
        )
        .expect("write live stale status");
        fs::write(
            &lock_path,
            serde_json::to_string(&ReadyRepairLockFile {
                schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
                project_root: clean_path_text(project.path()),
                profile: "agent".to_string(),
                run_id: Some("test-proof".to_string()),
                namespace: "codestory-agent-test-proof".to_string(),
                pid,
                process_start_identity: None,
                started_at_epoch_ms: old,
                token: format!("{pid}:{old}"),
            })
            .expect("lock json"),
        )
        .expect("write live stale lock");

        assert_eq!(
            read_abandoned_ready_repair_status(&status_path, project.path(), now),
            None,
            "live repair status must not be treated as abandoned by age"
        );
        assert!(
            cleanup_abandoned_ready_repair_status(project.path(), Some("test-proof")).is_empty(),
            "cleanup must not remove live repair status"
        );
        match try_acquire_ready_repair_lock(&sidecar, project.path()).expect("repair lock") {
            ReadyRepairLockAttempt::Busy(busy) => {
                let busy_status = busy.status.expect("stale live repair status");
                assert_eq!(busy_status.pid, pid);
                assert_eq!(busy_status.phase, "Embedding documents");
                assert_eq!(busy.reason.as_deref(), Some("live_repair_lock"));
            }
            ReadyRepairLockAttempt::Acquired(_) => panic!("live stale repair lock must stay busy"),
        }
    }

    #[test]
    fn stale_live_repair_with_exact_identity_is_never_terminated_by_age() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar_with_run_id(state.path(), "shared-agent");
        let status_path = ready_repair_status_path(&sidecar);
        fs::create_dir_all(status_path.parent().expect("repair status parent")).expect("state dir");
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .expect("long-lived child");
        #[cfg(unix)]
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("long-lived child");
        let pid = child.id();
        let process_start_identity =
            recorded_process_start_identity(pid).expect("child process start identity");
        let old = now_epoch_ms() - READY_REPAIR_LOCK_STALE_TTL.as_millis() as i64 - 60_000;
        let status = ReadyRepairStatus {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            status: "repairing".to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("shared-agent".to_string()),
            namespace: sidecar.namespace.clone(),
            phase: "Embedding documents".to_string(),
            pid,
            attempt_id: Some("stale-live-attempt".to_string()),
            process_start_identity: Some(process_start_identity),
            started_at_epoch_ms: old,
            updated_at_epoch_ms: old,
        };
        fs::write(
            &status_path,
            serde_json::to_string(&status).expect("status json"),
        )
        .expect("write stale live status");

        assert_eq!(
            read_stale_live_ready_repair_status(&status_path, project.path(), now_epoch_ms()),
            Some(status),
            "heartbeat age must remain diagnostic while exact process ownership is live"
        );
        assert_eq!(
            read_abandoned_ready_repair_status(&status_path, project.path(), now_epoch_ms()),
            None,
            "age alone must never abandon a live exact-identity owner"
        );
        assert!(child.try_wait().expect("probe child").is_none());
        child.kill().expect("stop probe child");
        child.wait().expect("reap probe child");
    }

    #[test]
    fn dead_pid_repair_lock_is_reclaimable_immediately() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_lock_path(&sidecar);
        fs::create_dir_all(path.parent().expect("repair lock parent")).expect("state dir");
        let now = now_epoch_ms();
        let lock = ReadyRepairLockFile {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("test-proof".to_string()),
            namespace: "codestory-agent-test-proof".to_string(),
            pid: u32::MAX,
            process_start_identity: None,
            started_at_epoch_ms: now,
            token: format!("{}:{now}", u32::MAX),
        };
        fs::write(&path, serde_json::to_string(&lock).expect("lock json"))
            .expect("write dead-pid lock");

        match try_acquire_ready_repair_lock(&sidecar, project.path()).expect("lock attempt") {
            ReadyRepairLockAttempt::Acquired(_) => {}
            ReadyRepairLockAttempt::Busy(busy) => {
                panic!("dead-pid repair lock should be reclaimed, got busy at {busy:?}")
            }
        }
    }

    #[test]
    fn reused_pid_repair_lock_is_reclaimable_immediately() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let path = ready_repair_lock_path(&sidecar);
        fs::create_dir_all(path.parent().expect("repair lock parent")).expect("state dir");
        let now = now_epoch_ms();
        let pid = std::process::id();
        let lock = ReadyRepairLockFile {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("test-proof".to_string()),
            namespace: "codestory-agent-test-proof".to_string(),
            pid,
            process_start_identity: Some("different-process-start".to_string()),
            started_at_epoch_ms: now,
            token: format!("{pid}:{now}"),
        };
        fs::write(&path, serde_json::to_string(&lock).expect("lock json"))
            .expect("write reused-pid lock");

        match try_acquire_ready_repair_lock(&sidecar, project.path()).expect("lock attempt") {
            ReadyRepairLockAttempt::Acquired(_) => {}
            ReadyRepairLockAttempt::Busy(busy) => {
                panic!("reused-pid repair lock should be reclaimed, got busy at {busy:?}")
            }
        }
    }

    #[test]
    fn cleanup_abandoned_ready_repair_status_removes_dead_pid_status_and_stale_locks() {
        let _env_lock = crate::config::config_env_test_lock();
        let project = tempfile::tempdir().expect("project");
        let sidecar = crate::sidecar_runtime::for_project_with_run_id(
            project.path(),
            SidecarProfile::Agent,
            Some("shared-agent"),
        );
        let status_path = ready_repair_status_path(&sidecar);
        let lock_path = ready_repair_lock_path(&sidecar);
        let project_lock_path = project_ready_repair_lock_path(&sidecar);
        fs::create_dir_all(status_path.parent().expect("status parent")).expect("state dir");
        if let Some(parent) = project_lock_path.parent() {
            fs::create_dir_all(parent).expect("project lock parent");
        }
        let now = now_epoch_ms();
        let status = ReadyRepairStatus {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            status: "repairing".to_string(),
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("shared-agent".to_string()),
            namespace: sidecar.namespace.clone(),
            phase: "graph artifact".to_string(),
            pid: u32::MAX,
            attempt_id: Some("abandoned-test".to_string()),
            process_start_identity: None,
            started_at_epoch_ms: now,
            updated_at_epoch_ms: now,
        };
        fs::write(
            &status_path,
            serde_json::to_string(&status).expect("status json"),
        )
        .expect("write abandoned status");
        let stale_lock = ReadyRepairLockFile {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            project_root: clean_path_text(project.path()),
            profile: "agent".to_string(),
            run_id: Some("shared-agent".to_string()),
            namespace: sidecar.namespace.clone(),
            pid: u32::MAX,
            process_start_identity: None,
            started_at_epoch_ms: now,
            token: format!("{}:{now}", u32::MAX),
        };
        fs::write(
            &lock_path,
            serde_json::to_string(&stale_lock).expect("lock json"),
        )
        .expect("write stale namespace lock");
        fs::write(
            &project_lock_path,
            serde_json::to_string(&stale_lock).expect("project lock json"),
        )
        .expect("write stale project lock");

        let cleanups = cleanup_abandoned_ready_repair_status_for_sidecar(project.path(), &sidecar);

        assert_eq!(cleanups.len(), 1);
        assert!(cleanups[0].removed_status_path);
        assert!(!status_path.exists());
        assert!(
            cleanups[0]
                .removed_lock_paths
                .iter()
                .any(|path| path == &lock_path)
                || !lock_path.exists(),
            "stale namespace lock should be cleaned: {:?}",
            cleanups[0].removed_lock_paths
        );
        assert!(
            active_ready_repair_status(project.path(), Some("shared-agent")).is_none(),
            "abandoned cleanup must leave no active repair"
        );
        let result = read_ready_repair_worker_result_for_sidecar(&sidecar)
            .expect("abandoned cleanup terminal result");
        assert_eq!(result.attempt_id, "abandoned-test");
        assert_eq!(result.outcome, "abandoned");

        let terminal = ReadyRepairWorkerResult {
            schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
            attempt_id: "abandoned-test".to_string(),
            project_root: status.project_root.clone(),
            profile: status.profile.clone(),
            run_id: status.run_id.clone(),
            namespace: status.namespace.clone(),
            pid: status.pid,
            started_at_epoch_ms: status.started_at_epoch_ms,
            finished_at_epoch_ms: now + 1,
            outcome: "failed".to_string(),
            auto_retry_fingerprint: None,
            exit_code: Some(17),
            wait_error: None,
            terminal_envelope: None,
            stdout_tail: "terminal evidence".to_string(),
            stderr_tail: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
        write_ready_repair_worker_result(&sidecar, &terminal).expect("terminal result");
        fs::write(
            &status_path,
            serde_json::to_string(&status).expect("status json"),
        )
        .expect("recreate abandoned status");

        let repeated = cleanup_abandoned_ready_repair_status_for_sidecar(project.path(), &sidecar);

        assert_eq!(repeated.len(), 1);
        assert_eq!(
            read_ready_repair_worker_result_for_sidecar(&sidecar),
            Some(terminal),
            "cleanup must preserve an existing matching terminal result"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persisted_ready_repair_root_preserves_unix_case() {
        let parent = tempfile::tempdir().expect("parent");
        let project = parent.path().join("CaseSensitiveProject");
        fs::create_dir_all(&project).expect("project");
        assert!(clean_path_text(&project).ends_with("CaseSensitiveProject"));
    }
}
