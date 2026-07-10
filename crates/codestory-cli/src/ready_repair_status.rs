use anyhow::Result;
use codestory_retrieval::{
    DEFAULT_AGENT_RUN_ID, SidecarProfile, SidecarRuntimeConfig,
    sidecar_runtime_for_project_with_run_id,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[cfg(any(windows, all(unix, not(target_os = "linux"))))]
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const READY_REPAIR_STATUS_FILE: &str = "ready-repair-status.json";
const READY_REPAIR_LOCK_FILE: &str = "ready-repair.lock";
const READY_REPAIR_PROJECT_LOCK_FILE: &str = "ready-repair-project.lock";
const READY_REPAIR_STATUS_SCHEMA_VERSION: u32 = 1;
const READY_REPAIR_STATUS_TTL: Duration = Duration::from_secs(30);
const READY_REPAIR_LOCK_STALE_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReadyRepairStatus {
    pub(crate) schema_version: u32,
    pub(crate) status: String,
    pub(crate) project_root: String,
    pub(crate) profile: String,
    pub(crate) run_id: Option<String>,
    pub(crate) namespace: String,
    pub(crate) compose_project: String,
    pub(crate) phase: String,
    pub(crate) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process_start_identity: Option<String>,
    pub(crate) started_at_epoch_ms: i64,
    pub(crate) updated_at_epoch_ms: i64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessOwnerState {
    Matching,
    GoneOrReused,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProcessProbe {
    Running { start_identity: Option<String> },
    NotRunning,
    Unknown,
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

    if let Some(status) = active_ready_repair_status(project_root, active_run_id) {
        return Ok(ReadyRepairLockAttempt::Busy(Box::new(ReadyRepairBusy {
            status: Some(status),
            lock_path: path,
            reason: None,
        })));
    }
    let stale_live_status = stale_live_ready_repair_status(project_root, active_run_id);

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
                status: active_ready_repair_status(project_root, active_run_id),
                lock_path: path,
                reason: Some("lock_contention".to_string()),
            })))
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write_ready_repair_status(
    sidecar: &SidecarRuntimeConfig,
    project_root: &Path,
    phase: &str,
    started_at_epoch_ms: i64,
    pid: u32,
) -> Result<()> {
    let path = ready_repair_status_path(sidecar);
    let status = ReadyRepairStatus {
        schema_version: READY_REPAIR_STATUS_SCHEMA_VERSION,
        status: "repairing".to_string(),
        project_root: clean_path_text(project_root),
        profile: sidecar.profile.as_str().to_string(),
        run_id: sidecar.run_id.clone(),
        namespace: sidecar.namespace.clone(),
        compose_project: sidecar.compose_project.clone(),
        phase: phase.to_string(),
        pid,
        process_start_identity: recorded_process_start_identity(pid),
        started_at_epoch_ms,
        updated_at_epoch_ms: now_epoch_ms(),
    };
    crate::file_state::write_json_atomic(&path, "ready-repair-status", &status)
}

pub(crate) fn clear_ready_repair_status(
    sidecar: &SidecarRuntimeConfig,
    started_at_epoch_ms: i64,
    pid: u32,
) {
    let path = ready_repair_status_path(sidecar);
    let Some(status) = read_ready_repair_status_file(&path) else {
        return;
    };
    if status.pid == pid && status.started_at_epoch_ms == started_at_epoch_ms {
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
    let now = now_epoch_ms();
    ready_repair_status_paths(project_root, run_id)
        .into_iter()
        .filter_map(|path| {
            let status = read_abandoned_ready_repair_status(&path, project_root, now)?;
            let removed_status_path = fs::remove_file(&path).is_ok();
            let mut removed_lock_paths = Vec::new();
            for lock_path in ready_repair_lock_paths_for_status(project_root, &status) {
                if ready_repair_lock_file_is_stale(&lock_path)
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

pub(crate) fn ready_repair_status_cache_fingerprint(project_root: &Path) -> String {
    ready_repair_status_paths(project_root, None)
        .into_iter()
        .map(|path| path_fingerprint(&path))
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
        || !same_path_text(Path::new(&status.project_root), project_root)
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
        || !same_path_text(Path::new(&status.project_root), project_root)
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
        || !same_path_text(Path::new(&status.project_root), project_root)
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
    process_owner_state_with(pid, expected_start_identity, probe_process)
}

fn process_owner_state_with(
    pid: u32,
    expected_start_identity: Option<&str>,
    probe: impl FnOnce(u32) -> ProcessProbe,
) -> ProcessOwnerState {
    match probe(pid) {
        ProcessProbe::NotRunning => ProcessOwnerState::GoneOrReused,
        ProcessProbe::Unknown => ProcessOwnerState::Unknown,
        ProcessProbe::Running { start_identity } => match expected_start_identity {
            None => ProcessOwnerState::Matching,
            Some(expected) => match start_identity {
                Some(actual) if actual == expected => ProcessOwnerState::Matching,
                Some(_) => ProcessOwnerState::GoneOrReused,
                None => ProcessOwnerState::Unknown,
            },
        },
    }
}

pub(crate) fn recorded_process_start_identity(pid: u32) -> Option<String> {
    match probe_process(pid) {
        ProcessProbe::Running { start_identity } => start_identity,
        ProcessProbe::NotRunning | ProcessProbe::Unknown => None,
    }
}

fn probe_process(pid: u32) -> ProcessProbe {
    static CURRENT_PROCESS_START_IDENTITY: OnceLock<String> = OnceLock::new();

    if pid == std::process::id()
        && let Some(identity) = CURRENT_PROCESS_START_IDENTITY.get()
    {
        return ProcessProbe::Running {
            start_identity: Some(identity.clone()),
        };
    }

    let probe = probe_process_platform(pid);
    if pid == std::process::id()
        && let ProcessProbe::Running {
            start_identity: Some(identity),
        } = &probe
    {
        let _ = CURRENT_PROCESS_START_IDENTITY.set(identity.clone());
    }
    probe
}

#[cfg(windows)]
fn probe_process_platform(pid: u32) -> ProcessProbe {
    let script = format!(
        "$p=Get-CimInstance Win32_Process -Filter \"ProcessId = {pid}\" -ErrorAction Stop; if ($null -eq $p) {{ exit 2 }}; $p.CreationDate.ToUniversalTime().Ticks"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output();
    let Ok(output) = output else {
        return ProcessProbe::Unknown;
    };
    if output.status.code() == Some(2) {
        return ProcessProbe::NotRunning;
    }
    if !output.status.success() {
        return ProcessProbe::Unknown;
    }
    let identity = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if identity.is_empty() {
        return ProcessProbe::Unknown;
    }
    ProcessProbe::Running {
        start_identity: Some(format!("windows:{identity}")),
    }
}

#[cfg(target_os = "linux")]
fn probe_process_platform(pid: u32) -> ProcessProbe {
    let path = Path::new("/proc").join(pid.to_string()).join("stat");
    let stat = match fs::read_to_string(path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == ErrorKind::NotFound => return ProcessProbe::NotRunning,
        Err(_) => return ProcessProbe::Unknown,
    };
    let Some((_, fields)) = stat.rsplit_once(") ") else {
        return ProcessProbe::Unknown;
    };
    let Some(start_ticks) = fields.split_whitespace().nth(19) else {
        return ProcessProbe::Unknown;
    };
    ProcessProbe::Running {
        start_identity: Some(format!("linux:{start_ticks}")),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn probe_process_platform(pid: u32) -> ProcessProbe {
    let output = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output();
    let Ok(output) = output else {
        return ProcessProbe::Unknown;
    };
    if !output.status.success() {
        return if output.status.code() == Some(1) {
            ProcessProbe::NotRunning
        } else {
            ProcessProbe::Unknown
        };
    }
    let identity = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if identity.is_empty() {
        return ProcessProbe::Unknown;
    }
    ProcessProbe::Running {
        start_identity: Some(format!("unix:{identity}")),
    }
}

#[cfg(not(any(windows, unix)))]
fn probe_process_platform(_pid: u32) -> ProcessProbe {
    ProcessProbe::Unknown
}

fn ready_repair_lock_paths_for_status(
    project_root: &Path,
    status: &ReadyRepairStatus,
) -> Vec<PathBuf> {
    let run_id = status.run_id.as_deref().unwrap_or(DEFAULT_AGENT_RUN_ID);
    let sidecar =
        sidecar_runtime_for_project_with_run_id(project_root, SidecarProfile::Agent, Some(run_id));
    vec![
        ready_repair_lock_path(&sidecar),
        project_ready_repair_lock_path(&sidecar),
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
        let sidecar = sidecar_runtime_for_project_with_run_id(
            project_root,
            SidecarProfile::Agent,
            Some(run_id),
        );
        paths.insert(ready_repair_status_path(&sidecar));
        return paths.into_iter().collect();
    }

    let default_sidecar = sidecar_runtime_for_project_with_run_id(
        project_root,
        SidecarProfile::Agent,
        Some(DEFAULT_AGENT_RUN_ID),
    );
    paths.insert(ready_repair_status_path(&default_sidecar));

    if let Some((sidecars_root, namespace_prefix)) = agent_sidecars_scan_root(&default_sidecar)
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
        .to_ascii_lowercase()
}

fn same_path_text(left: &Path, right: &Path) -> bool {
    clean_path_text(left) == clean_path_text(right)
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    fn test_sidecar(root: &Path) -> SidecarRuntimeConfig {
        test_sidecar_with_run_id(root, "test-proof")
    }

    fn test_sidecar_with_run_id(root: &Path, run_id: &str) -> SidecarRuntimeConfig {
        let namespace = format!("codestory-agent-{run_id}");
        SidecarRuntimeConfig {
            project_identity: None,
            layout: SidecarLayout {
                zoekt_http_port: 6070,
                qdrant_http_port: 6333,
                qdrant_grpc_port: 6334,
                zoekt_data_dir: root.join("zoekt"),
                qdrant_data_dir: root.join("qdrant"),
                scip_artifacts_root: root.join("scip"),
                state_file: root.join(&namespace).join("retrieval-sidecars.json"),
            },
            profile: SidecarProfile::Agent,
            run_id: Some(run_id.to_string()),
            namespace: namespace.clone(),
            compose_project: namespace,
            embed_http_port: 8080,
            cleanup_command: "codestory-cli retrieval down".to_string(),
            labels: Default::default(),
        }
    }

    #[test]
    fn process_owner_probe_matches_live_process_identity() {
        let state = process_owner_state_with(42, Some("start-a"), |_| ProcessProbe::Running {
            start_identity: Some("start-a".to_string()),
        });

        assert_eq!(state, ProcessOwnerState::Matching);
    }

    #[test]
    fn process_owner_probe_detects_dead_process() {
        let state = process_owner_state_with(42, Some("start-a"), |_| ProcessProbe::NotRunning);

        assert_eq!(state, ProcessOwnerState::GoneOrReused);
    }

    #[test]
    fn process_owner_probe_detects_pid_reuse() {
        let state = process_owner_state_with(42, Some("start-a"), |_| ProcessProbe::Running {
            start_identity: Some("start-b".to_string()),
        });

        assert_eq!(state, ProcessOwnerState::GoneOrReused);
    }

    #[test]
    fn process_owner_probe_uncertainty_preserves_ownership() {
        let failed_probe = process_owner_state_with(42, Some("start-a"), |_| ProcessProbe::Unknown);
        let missing_identity =
            process_owner_state_with(42, Some("start-a"), |_| ProcessProbe::Running {
                start_identity: None,
            });
        let legacy_live = process_owner_state_with(42, None, |_| ProcessProbe::Running {
            start_identity: Some("start-b".to_string()),
        });

        assert_eq!(failed_probe, ProcessOwnerState::Unknown);
        assert_eq!(missing_identity, ProcessOwnerState::Unknown);
        assert_eq!(legacy_live, ProcessOwnerState::Matching);
    }

    #[test]
    fn repair_status_file_round_trips_current_phase_and_clears() {
        let project = tempfile::tempdir().expect("project");
        let state = tempfile::tempdir().expect("state");
        let sidecar = test_sidecar(state.path());
        let started_at = now_epoch_ms();
        let pid = std::process::id();

        write_ready_repair_status(&sidecar, project.path(), "Qdrant finalize", started_at, pid)
            .expect("write repair status");
        let path = ready_repair_status_path(&sidecar);
        let status = read_ready_repair_status(&path, project.path(), now_epoch_ms())
            .expect("active repair status");

        assert_eq!(status.status, "repairing");
        assert_eq!(status.phase, "Qdrant finalize");
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
            compose_project: "codestory-agent-test-proof".to_string(),
            phase: "embeddings".to_string(),
            pid: 4242,
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
            compose_project: "codestory-agent-test-proof".to_string(),
            phase: "graph artifact".to_string(),
            pid: dead_pid,
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
        let sidecar = sidecar_runtime_for_project_with_run_id(
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
            compose_project: "codestory-agent-test-proof".to_string(),
            phase: "Embedding documents".to_string(),
            pid,
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
        let project = tempfile::tempdir().expect("project");
        let sidecar = sidecar_runtime_for_project_with_run_id(
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
            compose_project: sidecar.compose_project.clone(),
            phase: "graph artifact".to_string(),
            pid: u32::MAX,
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

        let cleanups = cleanup_abandoned_ready_repair_status(project.path(), Some("shared-agent"));

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
    }
}
