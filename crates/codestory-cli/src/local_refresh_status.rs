use anyhow::Result;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOCAL_REFRESH_STATUS_FILE: &str = "local-refresh-status.json";
const LOCAL_REFRESH_LOCK_FILE: &str = "local-refresh.lock";
const LOCAL_REFRESH_STATE_GUARD_FILE: &str = "local-refresh-state.guard";
const LOCAL_REFRESH_STATUS_SCHEMA_VERSION: u32 = 1;
const LOCAL_REFRESH_STATUS_TTL: Duration = Duration::from_secs(30);
const LOCAL_REFRESH_LOCK_STALE_TTL: Duration = Duration::from_secs(120);
const LOCAL_REFRESH_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LocalRefreshStatus {
    pub(crate) schema_version: u32,
    pub(crate) status: String,
    pub(crate) project_root: String,
    pub(crate) phase: String,
    pub(crate) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process_start_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_token: Option<String>,
    pub(crate) started_at_epoch_ms: i64,
    pub(crate) updated_at_epoch_ms: i64,
    #[serde(default)]
    pub(crate) last_failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LocalRefreshLockFile {
    schema_version: u32,
    project_root: String,
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_start_identity: Option<String>,
    started_at_epoch_ms: i64,
    token: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalRefreshBusy {
    pub(crate) status: Option<LocalRefreshStatus>,
    pub(crate) lock_path: PathBuf,
    pub(crate) pid: Option<u32>,
    pub(crate) started_at_epoch_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct LocalRefreshCleanup {
    pub(crate) status: Option<LocalRefreshStatus>,
    pub(crate) status_path: PathBuf,
    pub(crate) lock_path: PathBuf,
    pub(crate) lock_pid: Option<u32>,
    pub(crate) lock_started_at_epoch_ms: Option<i64>,
    pub(crate) removed_status_path: bool,
    pub(crate) removed_lock_path: bool,
    pub(crate) reason: String,
}

#[derive(Debug)]
pub(crate) enum LocalRefreshLockAttempt {
    Acquired(LocalRefreshLock),
    Busy(LocalRefreshBusy),
}

#[derive(Debug)]
pub(crate) struct LocalRefreshLock {
    path: PathBuf,
    token: String,
    pid: u32,
    process_start_identity: Option<String>,
    started_at_epoch_ms: i64,
}

#[derive(Debug)]
struct LocalRefreshStateGuard {
    file: File,
}

impl Drop for LocalRefreshStateGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug)]
pub(crate) struct LocalRefreshHeartbeat {
    stop: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl LocalRefreshHeartbeat {
    pub(crate) fn start(lock: &LocalRefreshLock, project_root: &Path, phase: &str) -> Self {
        Self::start_with_interval(lock, project_root, phase, LOCAL_REFRESH_HEARTBEAT_INTERVAL)
    }

    fn start_with_interval(
        lock: &LocalRefreshLock,
        project_root: &Path,
        phase: &str,
        interval: Duration,
    ) -> Self {
        let owner = lock.owner();
        let cache_root = lock.cache_root().to_path_buf();
        let project_root = project_root.to_path_buf();
        let phase = phase.to_string();
        let (stop, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = receiver.recv_timeout(interval) {
                match renew_local_refresh_status(&cache_root, &project_root, &phase, &owner) {
                    Ok(true) | Err(_) => {}
                    Ok(false) => break,
                }
            }
        });
        Self {
            stop: Some(stop),
            worker: Some(worker),
        }
    }

    pub(crate) fn stop(mut self) {
        self.join();
    }

    fn join(&mut self) {
        self.stop.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for LocalRefreshHeartbeat {
    fn drop(&mut self) {
        self.join();
    }
}

#[derive(Clone, Debug)]
struct LocalRefreshOwner {
    token: String,
    pid: u32,
    process_start_identity: Option<String>,
    started_at_epoch_ms: i64,
}

impl Drop for LocalRefreshLock {
    fn drop(&mut self) {
        let Some(cache_root) = self.path.parent() else {
            return;
        };
        let Ok(_guard) = acquire_local_refresh_state_guard(cache_root) else {
            return;
        };
        let Some(lock) = read_local_refresh_lock_file(&self.path) else {
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

impl LocalRefreshLock {
    pub(crate) fn started_at_epoch_ms(&self) -> i64 {
        self.started_at_epoch_ms
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn write_status(
        &self,
        project_root: &Path,
        status: &str,
        phase: &str,
        last_failure_reason: Option<String>,
    ) -> Result<bool> {
        let cache_root = self.cache_root();
        let _guard = acquire_local_refresh_state_guard(cache_root)?;
        let owner = self.owner();
        if !local_refresh_lock_matches(&self.path, project_root, &owner) {
            return Ok(false);
        }
        write_local_refresh_status_for_owner(
            cache_root,
            project_root,
            status,
            phase,
            &owner,
            last_failure_reason,
        )?;
        Ok(true)
    }

    fn owner(&self) -> LocalRefreshOwner {
        LocalRefreshOwner {
            token: self.token.clone(),
            pid: self.pid,
            process_start_identity: self.process_start_identity.clone(),
            started_at_epoch_ms: self.started_at_epoch_ms,
        }
    }

    fn cache_root(&self) -> &Path {
        self.path
            .parent()
            .expect("local refresh lock path always has a cache root")
    }
}

pub(crate) fn try_acquire_local_refresh_lock(
    cache_root: &Path,
    project_root: &Path,
) -> Result<LocalRefreshLockAttempt> {
    let path = local_refresh_lock_path(cache_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _guard = acquire_local_refresh_state_guard(cache_root)?;

    let started_at_epoch_ms = now_epoch_ms();
    let pid = std::process::id();
    let process_start_identity = crate::ready_repair_status::recorded_process_start_identity(pid);
    let token = format!("{pid}:{started_at_epoch_ms}");
    let lock = LocalRefreshLockFile {
        schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
        project_root: clean_path_text(project_root),
        pid,
        process_start_identity: process_start_identity.clone(),
        started_at_epoch_ms,
        token: token.clone(),
    };
    let content = serde_json::to_vec_pretty(&lock)?;

    match create_local_refresh_lock_file(&path, &content) {
        Ok(()) => {
            return Ok(LocalRefreshLockAttempt::Acquired(LocalRefreshLock {
                path,
                token,
                pid,
                process_start_identity,
                started_at_epoch_ms,
            }));
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }

    if let Some(status) = active_local_refresh_status(cache_root, project_root) {
        return Ok(LocalRefreshLockAttempt::Busy(LocalRefreshBusy {
            status: Some(status),
            lock_path: path,
            pid: None,
            started_at_epoch_ms: None,
        }));
    }

    let existing = read_local_refresh_lock_file(&path);
    if !local_refresh_lock_file_is_stale(&path, None) {
        return Ok(LocalRefreshLockAttempt::Busy(LocalRefreshBusy {
            status: None,
            lock_path: path,
            pid: existing.as_ref().map(|lock| lock.pid),
            started_at_epoch_ms: existing.as_ref().map(|lock| lock.started_at_epoch_ms),
        }));
    }

    let _ = fs::remove_file(&path);
    match create_local_refresh_lock_file(&path, &content) {
        Ok(()) => Ok(LocalRefreshLockAttempt::Acquired(LocalRefreshLock {
            path,
            token,
            pid,
            process_start_identity,
            started_at_epoch_ms,
        })),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let existing = read_local_refresh_lock_file(&path);
            Ok(LocalRefreshLockAttempt::Busy(LocalRefreshBusy {
                status: active_local_refresh_status(cache_root, project_root),
                lock_path: path,
                pid: existing.as_ref().map(|lock| lock.pid),
                started_at_epoch_ms: existing.as_ref().map(|lock| lock.started_at_epoch_ms),
            }))
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
pub(crate) fn write_local_refresh_status(
    cache_root: &Path,
    project_root: &Path,
    status: &str,
    phase: &str,
    started_at_epoch_ms: i64,
    pid: u32,
    last_failure_reason: Option<String>,
) -> Result<()> {
    let _guard = acquire_local_refresh_state_guard(cache_root)?;
    let path = local_refresh_status_path(cache_root);
    let status = LocalRefreshStatus {
        schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
        status: status.to_string(),
        project_root: clean_path_text(project_root),
        phase: phase.to_string(),
        pid,
        process_start_identity: crate::ready_repair_status::recorded_process_start_identity(pid),
        owner_token: None,
        started_at_epoch_ms,
        updated_at_epoch_ms: now_epoch_ms(),
        last_failure_reason,
    };
    crate::file_state::write_json_atomic(&path, "local-refresh-status", &status)
}

fn write_local_refresh_status_for_owner(
    cache_root: &Path,
    project_root: &Path,
    status: &str,
    phase: &str,
    owner: &LocalRefreshOwner,
    last_failure_reason: Option<String>,
) -> Result<()> {
    let value = LocalRefreshStatus {
        schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
        status: status.to_string(),
        project_root: clean_path_text(project_root),
        phase: phase.to_string(),
        pid: owner.pid,
        process_start_identity: owner.process_start_identity.clone(),
        owner_token: Some(owner.token.clone()),
        started_at_epoch_ms: owner.started_at_epoch_ms,
        updated_at_epoch_ms: now_epoch_ms(),
        last_failure_reason,
    };
    crate::file_state::write_json_atomic(
        &local_refresh_status_path(cache_root),
        "local-refresh-status",
        &value,
    )
}

fn renew_local_refresh_status(
    cache_root: &Path,
    project_root: &Path,
    phase: &str,
    owner: &LocalRefreshOwner,
) -> Result<bool> {
    renew_local_refresh_status_with_hook(cache_root, project_root, phase, owner, || {})
}

fn renew_local_refresh_status_with_hook(
    cache_root: &Path,
    project_root: &Path,
    phase: &str,
    owner: &LocalRefreshOwner,
    before_write: impl FnOnce(),
) -> Result<bool> {
    let _guard = acquire_local_refresh_state_guard(cache_root)?;
    if !local_refresh_lock_matches(&local_refresh_lock_path(cache_root), project_root, owner) {
        return Ok(false);
    }
    let path = local_refresh_status_path(cache_root);
    let Some(mut status) = read_local_refresh_status_file(&path) else {
        return Ok(false);
    };
    if status.schema_version != LOCAL_REFRESH_STATUS_SCHEMA_VERSION
        || status.status != "refreshing"
        || status.phase != phase
        || !codestory_workspace::same_workspace_path(Path::new(&status.project_root), project_root)
        || status.pid != owner.pid
        || status.process_start_identity != owner.process_start_identity
        || status.owner_token.as_deref() != Some(owner.token.as_str())
        || status.started_at_epoch_ms != owner.started_at_epoch_ms
    {
        return Ok(false);
    }
    before_write();
    status.updated_at_epoch_ms = now_epoch_ms();
    crate::file_state::write_json_atomic(&path, "local-refresh-status", &status)?;
    Ok(true)
}

fn local_refresh_lock_matches(path: &Path, project_root: &Path, owner: &LocalRefreshOwner) -> bool {
    read_local_refresh_lock_file(path).is_some_and(|lock| {
        lock.schema_version == LOCAL_REFRESH_STATUS_SCHEMA_VERSION
            && codestory_workspace::same_workspace_path(Path::new(&lock.project_root), project_root)
            && lock.token == owner.token
            && lock.pid == owner.pid
            && lock.process_start_identity == owner.process_start_identity
            && lock.started_at_epoch_ms == owner.started_at_epoch_ms
    })
}

pub(crate) fn active_local_refresh_status(
    cache_root: &Path,
    project_root: &Path,
) -> Option<LocalRefreshStatus> {
    let now = now_epoch_ms();
    read_local_refresh_status(&local_refresh_status_path(cache_root), project_root, now)
}

pub(crate) fn cleanup_stale_local_refresh_state(
    cache_root: &Path,
    project_root: &Path,
) -> Option<LocalRefreshCleanup> {
    let _guard = acquire_local_refresh_state_guard(cache_root).ok()?;
    let status_path = local_refresh_status_path(cache_root);
    let lock_path = local_refresh_lock_path(cache_root);
    let now = now_epoch_ms();
    let status = read_local_refresh_status_file(&status_path).filter(|status| {
        status.schema_version == LOCAL_REFRESH_STATUS_SCHEMA_VERSION
            && status.status == "refreshing"
            && codestory_workspace::same_workspace_path(
                Path::new(&status.project_root),
                project_root,
            )
    });
    let lock = read_local_refresh_lock_file(&lock_path).filter(|lock| {
        lock.schema_version == LOCAL_REFRESH_STATUS_SCHEMA_VERSION
            && codestory_workspace::same_workspace_path(Path::new(&lock.project_root), project_root)
    });
    let status_owner_state = status.as_ref().map(|status| {
        crate::ready_repair_status::process_owner_state(
            status.pid,
            status.process_start_identity.as_deref(),
        )
    });
    let status_owner_live = status_owner_state
        .is_some_and(|state| state != crate::ready_repair_status::ProcessOwnerState::GoneOrReused);
    let status_owner_dead = status_owner_state
        .is_some_and(|state| state == crate::ready_repair_status::ProcessOwnerState::GoneOrReused);
    let status_lock_mismatch = status.as_ref().is_some_and(|status| {
        status.owner_token.is_some()
            && lock
                .as_ref()
                .is_none_or(|lock| !local_refresh_status_matches_lock(status, lock, project_root))
    });
    let status_age_stale = status.as_ref().is_some_and(|status| {
        now.saturating_sub(status.updated_at_epoch_ms)
            > LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64
    });
    let status_stale =
        status_owner_dead || status_lock_mismatch || (status_age_stale && !status_owner_live);
    let live_status_heartbeat_stale = status_age_stale && status_owner_live;
    let active_status = status.as_ref().filter(|_| !status_stale);
    let lock_stale = lock
        .as_ref()
        .is_some_and(|_| local_refresh_lock_file_is_stale(&lock_path, active_status));
    if !status_stale && !lock_stale && !live_status_heartbeat_stale {
        return None;
    }

    let reason = match (status_stale, lock_stale, live_status_heartbeat_stale) {
        (_, _, true) => "live_status_heartbeat_stale",
        (true, true, false) => "stale_status_and_lock",
        (true, false, false) => "stale_status",
        (false, true, false) => "stale_lock",
        (false, false, false) => "clean",
    };
    let lock_matches_stale_status = status_owner_dead
        && status
            .as_ref()
            .zip(lock.as_ref())
            .is_some_and(|(status, lock)| {
                local_refresh_status_matches_lock(status, lock, project_root)
            });
    let removed_status_path =
        status_stale && status.is_some() && fs::remove_file(&status_path).is_ok();
    let removed_lock_path = lock.is_some()
        && (lock_stale || lock_matches_stale_status)
        && fs::remove_file(&lock_path).is_ok();
    Some(LocalRefreshCleanup {
        status,
        status_path,
        lock_path,
        lock_pid: lock.as_ref().map(|lock| lock.pid),
        lock_started_at_epoch_ms: lock.as_ref().map(|lock| lock.started_at_epoch_ms),
        removed_status_path,
        removed_lock_path,
        reason: reason.to_string(),
    })
}

pub(crate) fn local_refresh_status_cache_fingerprint(cache_root: &Path) -> String {
    [
        local_refresh_status_path(cache_root),
        local_refresh_lock_path(cache_root),
    ]
    .into_iter()
    .map(|path| path_fingerprint(&path))
    .collect::<Vec<_>>()
    .join(";")
}

fn create_local_refresh_lock_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    crate::file_state::write_synced_new_file(path, content)
}

fn acquire_local_refresh_state_guard(cache_root: &Path) -> Result<LocalRefreshStateGuard> {
    fs::create_dir_all(cache_root)?;
    let path = cache_root.join(LOCAL_REFRESH_STATE_GUARD_FILE);
    let file = open_local_refresh_state_guard_file(&path)?;
    FileExt::lock_exclusive(&file)?;
    anyhow::ensure!(
        locked_guard_path_matches(&file, &path),
        "local refresh state guard was replaced at {}",
        path.display()
    );
    Ok(LocalRefreshStateGuard { file })
}

fn open_local_refresh_state_guard_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // A guard handle may be shared by contenders, but never for deletion:
        // replacing this persistent inode would split the serialization domain.
        const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
        options.share_mode(FILE_SHARE_READ_WRITE);
    }
    Ok(options.open(path)?)
}

#[cfg(unix)]
fn locked_guard_path_matches(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata()
        .ok()
        .zip(fs::metadata(path).ok())
        .is_some_and(|(locked, current)| {
            locked.dev() == current.dev() && locked.ino() == current.ino()
        })
}

#[cfg(not(unix))]
fn locked_guard_path_matches(_file: &File, path: &Path) -> bool {
    // Windows opens deny delete sharing above, so the locked handle cannot be
    // unlinked or replaced. Other supported hosts at least require the path.
    path.is_file()
}

fn local_refresh_status_path(cache_root: &Path) -> PathBuf {
    cache_root.join(LOCAL_REFRESH_STATUS_FILE)
}

fn local_refresh_lock_path(cache_root: &Path) -> PathBuf {
    cache_root.join(LOCAL_REFRESH_LOCK_FILE)
}

fn local_refresh_lock_file_is_stale(
    path: &Path,
    _active_status: Option<&LocalRefreshStatus>,
) -> bool {
    let now = now_epoch_ms();
    if let Some(lock) = read_local_refresh_lock_file(path) {
        return crate::ready_repair_status::process_owner_state(
            lock.pid,
            lock.process_start_identity.as_deref(),
        ) == crate::ready_repair_status::ProcessOwnerState::GoneOrReused;
    }
    crate::file_state::file_modified_age_exceeds(path, LOCAL_REFRESH_LOCK_STALE_TTL, now)
}

fn read_local_refresh_status(
    path: &Path,
    project_root: &Path,
    now_epoch_ms: i64,
) -> Option<LocalRefreshStatus> {
    let status = read_local_refresh_status_file(path)?;
    if status.schema_version != LOCAL_REFRESH_STATUS_SCHEMA_VERSION
        || status.status != "refreshing"
        || !codestory_workspace::same_workspace_path(Path::new(&status.project_root), project_root)
    {
        return None;
    }
    let age_ms = now_epoch_ms.saturating_sub(status.updated_at_epoch_ms);
    if age_ms > LOCAL_REFRESH_STATUS_TTL.as_millis() as i64 {
        return None;
    }
    if crate::ready_repair_status::process_owner_state(
        status.pid,
        status.process_start_identity.as_deref(),
    ) == crate::ready_repair_status::ProcessOwnerState::GoneOrReused
    {
        return None;
    }
    if status.owner_token.is_some()
        && read_local_refresh_lock_file(&path.with_file_name(LOCAL_REFRESH_LOCK_FILE))
            .is_none_or(|lock| !local_refresh_status_matches_lock(&status, &lock, project_root))
    {
        return None;
    }
    Some(status)
}

fn local_refresh_status_matches_lock(
    status: &LocalRefreshStatus,
    lock: &LocalRefreshLockFile,
    project_root: &Path,
) -> bool {
    lock.schema_version == LOCAL_REFRESH_STATUS_SCHEMA_VERSION
        && codestory_workspace::same_workspace_path(Path::new(&lock.project_root), project_root)
        && status.owner_token.as_deref() == Some(lock.token.as_str())
        && status.pid == lock.pid
        && status.process_start_identity == lock.process_start_identity
        && status.started_at_epoch_ms == lock.started_at_epoch_ms
}

fn read_local_refresh_status_file(path: &Path) -> Option<LocalRefreshStatus> {
    crate::file_state::read_json(path)
}

fn read_local_refresh_lock_file(path: &Path) -> Option<LocalRefreshLockFile> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    #[test]
    fn local_refresh_lock_is_single_flight_and_reports_owner() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");

        let lock = match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("first lock attempt")
        {
            LocalRefreshLockAttempt::Acquired(lock) => lock,
            LocalRefreshLockAttempt::Busy(busy) => {
                panic!(
                    "first lock should be acquired, got busy at {:?}",
                    busy.lock_path
                )
            }
        };

        match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("second lock attempt")
        {
            LocalRefreshLockAttempt::Busy(busy) => {
                assert!(busy.status.is_none());
                assert_eq!(busy.pid, Some(std::process::id()));
                assert!(busy.started_at_epoch_ms.is_some());
                assert!(busy.lock_path.exists());
            }
            LocalRefreshLockAttempt::Acquired(_) => panic!("second lock must not be acquired"),
        }

        drop(lock);
        match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("third lock attempt")
        {
            LocalRefreshLockAttempt::Acquired(_) => {}
            LocalRefreshLockAttempt::Busy(busy) => {
                panic!(
                    "lock should be reusable after drop, got busy at {:?}",
                    busy.lock_path
                )
            }
        }
    }

    #[test]
    fn local_refresh_lock_busy_prefers_active_status_phase() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let _lock = match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("first lock attempt")
        {
            LocalRefreshLockAttempt::Acquired(lock) => lock,
            LocalRefreshLockAttempt::Busy(busy) => {
                panic!(
                    "first lock should be acquired, got busy at {:?}",
                    busy.lock_path
                )
            }
        };
        let pid = std::process::id();
        write_local_refresh_status(
            cache.path(),
            project.path(),
            "refreshing",
            "incremental_index",
            now_epoch_ms(),
            pid,
            None,
        )
        .expect("write refresh status");

        match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("second lock attempt")
        {
            LocalRefreshLockAttempt::Busy(busy) => {
                let status = busy.status.expect("active status");
                assert_eq!(status.phase, "incremental_index");
                assert_eq!(status.pid, pid);
            }
            LocalRefreshLockAttempt::Acquired(_) => panic!("second lock must not be acquired"),
        }
    }

    #[test]
    fn local_refresh_status_publication_never_exposes_partial_json() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let path = local_refresh_status_path(cache.path());
        let started_at = now_epoch_ms();
        let pid = std::process::id();
        write_local_refresh_status(
            cache.path(),
            project.path(),
            "refreshing",
            "initial",
            started_at,
            pid,
            None,
        )
        .expect("initial refresh status");

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
                    let status = read_local_refresh_status_file(&reader_path)
                        .expect("complete refresh status json");
                    assert_eq!(status.schema_version, LOCAL_REFRESH_STATUS_SCHEMA_VERSION);
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
            write_local_refresh_status(
                cache.path(),
                project.path(),
                "refreshing",
                &format!("iteration-{iteration}-{payload}"),
                started_at,
                pid,
                None,
            )
            .expect("replace refresh status");
            thread::yield_now();
        }
        writer_done.store(true, Ordering::Release);

        assert!(reader.join().expect("refresh status reader") > 0);
        assert!(read_local_refresh_status_file(&path).is_some());
    }

    #[test]
    fn local_refresh_heartbeat_renews_owner_and_stops_before_terminal_status() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let lock = match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("lock attempt")
        {
            LocalRefreshLockAttempt::Acquired(lock) => lock,
            LocalRefreshLockAttempt::Busy(_) => panic!("lock should be acquired"),
        };
        assert!(
            lock.write_status(project.path(), "refreshing", "incremental_index", None,)
                .expect("initial status")
        );
        let path = local_refresh_status_path(cache.path());
        let initial = read_local_refresh_status_file(&path).expect("initial status");
        let heartbeat = LocalRefreshHeartbeat::start_with_interval(
            &lock,
            project.path(),
            "incremental_index",
            Duration::from_millis(5),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let current = read_local_refresh_status_file(&path).expect("renewed status");
            if current.updated_at_epoch_ms > initial.updated_at_epoch_ms {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "heartbeat did not renew"
            );
            thread::yield_now();
        }
        heartbeat.stop();
        assert!(
            lock.write_status(project.path(), "refreshed", "incremental_index", None,)
                .expect("terminal status")
        );
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            read_local_refresh_status_file(&path)
                .expect("terminal status remains")
                .status,
            "refreshed"
        );
    }

    #[test]
    fn local_refresh_renewal_rejects_changed_token_pid_or_start_identity() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let lock = match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("lock attempt")
        {
            LocalRefreshLockAttempt::Acquired(lock) => lock,
            LocalRefreshLockAttempt::Busy(_) => panic!("lock should be acquired"),
        };
        assert!(
            lock.write_status(project.path(), "refreshing", "incremental_index", None,)
                .expect("initial status")
        );
        let owner = lock.owner();
        let path = local_refresh_status_path(cache.path());
        let baseline = read_local_refresh_status_file(&path).expect("baseline status");

        for changed in ["token", "pid", "start_identity", "terminal"] {
            let mut status = baseline.clone();
            match changed {
                "token" => status.owner_token = Some("different-owner".to_string()),
                "pid" => status.pid = status.pid.saturating_add(1),
                "start_identity" => {
                    status.process_start_identity = Some("different-process-start".to_string())
                }
                "terminal" => status.status = "refreshed".to_string(),
                _ => unreachable!(),
            }
            crate::file_state::write_json_atomic(&path, "local-refresh-status", &status)
                .expect("changed status");
            assert!(
                !renew_local_refresh_status(
                    cache.path(),
                    project.path(),
                    "incremental_index",
                    &owner,
                )
                .expect("renewal result"),
                "renewal should reject changed {changed}"
            );
            assert_eq!(read_local_refresh_status_file(&path), Some(status));
        }

        crate::file_state::write_json_atomic(&path, "local-refresh-status", &baseline)
            .expect("restore owned status");
        let lock_path = local_refresh_lock_path(cache.path());
        let mut changed_lock = read_local_refresh_lock_file(&lock_path).expect("owner lock");
        changed_lock.token = "replacement-owner".to_string();
        crate::file_state::write_json_atomic(&lock_path, "local-refresh-lock", &changed_lock)
            .expect("replace lock owner");
        assert!(
            !renew_local_refresh_status(cache.path(), project.path(), "incremental_index", &owner,)
                .expect("changed lock renewal")
        );
        assert!(active_local_refresh_status(cache.path(), project.path()).is_none());
        let cleanup = cleanup_stale_local_refresh_state(cache.path(), project.path())
            .expect("mismatched status cleanup");
        assert_eq!(cleanup.reason, "stale_status");
        assert!(cleanup.removed_status_path);
        assert!(!cleanup.removed_lock_path);
        assert!(lock_path.exists());
    }

    #[test]
    fn paused_stale_renewal_cannot_overwrite_replacement_owner_terminal_status() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let lock = match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("lock attempt")
        {
            LocalRefreshLockAttempt::Acquired(lock) => lock,
            LocalRefreshLockAttempt::Busy(_) => panic!("lock should be acquired"),
        };
        assert!(
            lock.write_status(project.path(), "refreshing", "incremental_index", None,)
                .expect("initial status")
        );

        let owner = lock.owner();
        let stale_cache = cache.path().to_path_buf();
        let stale_project = project.path().to_path_buf();
        let (paused_tx, paused_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let stale_writer = thread::spawn(move || {
            renew_local_refresh_status_with_hook(
                &stale_cache,
                &stale_project,
                "incremental_index",
                &owner,
                || {
                    paused_tx.send(()).expect("announce paused writer");
                    resume_rx.recv().expect("resume paused writer");
                },
            )
            .expect("stale renewal")
        });
        paused_rx.recv().expect("writer paused after validation");

        let replacement_cache = cache.path().to_path_buf();
        let replacement_project = project.path().to_path_buf();
        let replacement_start = lock.started_at_epoch_ms().saturating_add(1);
        let replacement_pid = lock.pid();
        let replacement_identity = lock.process_start_identity.clone();
        let (replacement_attempt_tx, replacement_attempt_rx) = std::sync::mpsc::channel();
        let (contention_tx, contention_rx) = std::sync::mpsc::channel();
        let (retry_tx, retry_rx) = std::sync::mpsc::channel();
        let replacement_writer = thread::spawn(move || {
            let guard_path = replacement_cache.join(LOCAL_REFRESH_STATE_GUARD_FILE);
            let guard_file =
                open_local_refresh_state_guard_file(&guard_path).expect("open replacement guard");
            replacement_attempt_tx
                .send(())
                .expect("announce replacement lock attempt");
            contention_tx
                .send(FileExt::try_lock_exclusive(&guard_file).expect("try replacement guard"))
                .expect("report replacement contention");
            retry_rx.recv().expect("retry replacement guard");
            FileExt::lock_exclusive(&guard_file).expect("acquire replacement guard");
            let replacement_token = "replacement-owner".to_string();
            crate::file_state::write_json_atomic(
                &local_refresh_lock_path(&replacement_cache),
                "local-refresh-lock",
                &LocalRefreshLockFile {
                    schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                    project_root: clean_path_text(&replacement_project),
                    pid: replacement_pid,
                    process_start_identity: replacement_identity.clone(),
                    started_at_epoch_ms: replacement_start,
                    token: replacement_token.clone(),
                },
            )
            .expect("replace owner lock");
            crate::file_state::write_json_atomic(
                &local_refresh_status_path(&replacement_cache),
                "local-refresh-status",
                &LocalRefreshStatus {
                    schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                    status: "refreshed".to_string(),
                    project_root: clean_path_text(&replacement_project),
                    phase: "incremental_index".to_string(),
                    pid: replacement_pid,
                    process_start_identity: replacement_identity,
                    owner_token: Some(replacement_token),
                    started_at_epoch_ms: replacement_start,
                    updated_at_epoch_ms: now_epoch_ms(),
                    last_failure_reason: None,
                },
            )
            .expect("replacement terminal status");
            FileExt::unlock(&guard_file).expect("unlock replacement guard");
        });
        replacement_attempt_rx
            .recv()
            .expect("replacement reached lock attempt");
        assert!(
            !contention_rx.recv().expect("replacement contention result"),
            "replacement try-lock must fail while renewal owns the guard"
        );

        resume_tx.send(()).expect("resume stale writer");
        assert!(stale_writer.join().expect("stale writer joined"));
        retry_tx.send(()).expect("retry replacement writer");
        replacement_writer.join().expect("replacement joined");

        let terminal = read_local_refresh_status_file(&local_refresh_status_path(cache.path()))
            .expect("terminal status");
        assert_eq!(terminal.status, "refreshed");
        assert_eq!(terminal.owner_token.as_deref(), Some("replacement-owner"));
        assert!(cache.path().join(LOCAL_REFRESH_STATE_GUARD_FILE).exists());
    }

    #[test]
    fn local_refresh_lock_reclaims_stale_file_and_expires_status() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let old_started = now_epoch_ms() - LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64 - 1_000;
        fs::write(
            local_refresh_lock_path(cache.path()),
            serde_json::to_string(&LocalRefreshLockFile {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                project_root: clean_path_text(project.path()),
                pid: std::process::id(),
                process_start_identity: Some("different-process-start".to_string()),
                started_at_epoch_ms: old_started,
                token: "stale".to_string(),
            })
            .expect("lock json"),
        )
        .expect("write stale lock");
        fs::write(
            local_refresh_status_path(cache.path()),
            serde_json::to_string(&LocalRefreshStatus {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                status: "refreshing".to_string(),
                project_root: clean_path_text(project.path()),
                phase: "incremental_index".to_string(),
                pid: std::process::id(),
                process_start_identity: Some("different-process-start".to_string()),
                owner_token: None,
                started_at_epoch_ms: old_started,
                updated_at_epoch_ms: now_epoch_ms()
                    - LOCAL_REFRESH_STATUS_TTL.as_millis() as i64
                    - 1_000,
                last_failure_reason: None,
            })
            .expect("status json"),
        )
        .expect("write stale status");

        assert!(
            active_local_refresh_status(cache.path(), project.path()).is_none(),
            "stale refresh status must not block future refresh"
        );
        match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("stale lock attempt")
        {
            LocalRefreshLockAttempt::Acquired(_) => {}
            LocalRefreshLockAttempt::Busy(busy) => {
                panic!(
                    "stale lock should be reclaimed, got busy at {:?}",
                    busy.lock_path
                )
            }
        }
    }

    #[test]
    fn cleanup_stale_local_refresh_state_removes_dead_heartbeat_and_lock() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let old_started = now_epoch_ms() - LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64 - 1_000;
        fs::write(
            local_refresh_lock_path(cache.path()),
            serde_json::to_string(&LocalRefreshLockFile {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                project_root: clean_path_text(project.path()),
                pid: u32::MAX,
                process_start_identity: None,
                started_at_epoch_ms: old_started,
                token: "stale".to_string(),
            })
            .expect("lock json"),
        )
        .expect("write stale lock");
        fs::write(
            local_refresh_status_path(cache.path()),
            serde_json::to_string(&LocalRefreshStatus {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                status: "refreshing".to_string(),
                project_root: clean_path_text(project.path()),
                phase: "incremental_index".to_string(),
                pid: u32::MAX,
                process_start_identity: None,
                owner_token: None,
                started_at_epoch_ms: old_started,
                updated_at_epoch_ms: old_started,
                last_failure_reason: None,
            })
            .expect("status json"),
        )
        .expect("write stale status");

        drop(acquire_local_refresh_state_guard(cache.path()).expect("persistent guard"));
        let guard_path = cache.path().join(LOCAL_REFRESH_STATE_GUARD_FILE);
        let guard_identity_alias = cache.path().join("local-refresh-state.guard.identity-test");
        fs::hard_link(&guard_path, &guard_identity_alias).expect("guard identity alias");

        let cleanup = cleanup_stale_local_refresh_state(cache.path(), project.path())
            .expect("stale local refresh cleanup");
        assert_eq!(cleanup.reason, "stale_status_and_lock");
        assert!(cleanup.removed_status_path);
        assert!(cleanup.removed_lock_path);
        assert!(!local_refresh_status_path(cache.path()).exists());
        assert!(!local_refresh_lock_path(cache.path()).exists());
        assert!(guard_path.exists(), "cleanup must preserve the guard inode");
        assert!(
            codestory_workspace::same_workspace_path(&guard_path, &guard_identity_alias),
            "cleanup must not replace the persistent guard inode"
        );
        fs::remove_file(guard_identity_alias).expect("remove guard identity alias");
    }

    #[test]
    fn cleanup_stale_local_refresh_state_preserves_live_pid_lock_without_active_status() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let old_started = now_epoch_ms() - LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64 - 1_000;
        fs::write(
            local_refresh_lock_path(cache.path()),
            serde_json::to_string(&LocalRefreshLockFile {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                project_root: clean_path_text(project.path()),
                pid: std::process::id(),
                process_start_identity: None,
                started_at_epoch_ms: old_started,
                token: "live-old".to_string(),
            })
            .expect("lock json"),
        )
        .expect("write old live lock");
        fs::write(
            local_refresh_status_path(cache.path()),
            serde_json::to_string(&LocalRefreshStatus {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                status: "refreshing".to_string(),
                project_root: clean_path_text(project.path()),
                phase: "incremental_index".to_string(),
                pid: std::process::id(),
                process_start_identity: None,
                owner_token: None,
                started_at_epoch_ms: old_started,
                updated_at_epoch_ms: old_started,
                last_failure_reason: None,
            })
            .expect("status json"),
        )
        .expect("write old live status");

        let cleanup = cleanup_stale_local_refresh_state(cache.path(), project.path())
            .expect("stale live heartbeat evidence");
        assert_eq!(cleanup.reason, "live_status_heartbeat_stale");
        assert!(!cleanup.removed_status_path);
        assert!(!cleanup.removed_lock_path);
        assert!(local_refresh_status_path(cache.path()).exists());
        assert!(local_refresh_lock_path(cache.path()).exists());
        match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("old live lock attempt")
        {
            LocalRefreshLockAttempt::Busy(busy) => {
                assert!(busy.status.is_none());
                assert_eq!(busy.pid, Some(std::process::id()));
            }
            LocalRefreshLockAttempt::Acquired(_) => panic!("live-pid lock must not be reclaimed"),
        }
    }

    #[test]
    fn cleanup_stale_local_refresh_state_keeps_live_lock_with_active_heartbeat() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let old_started = now_epoch_ms() - LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64 - 1_000;
        fs::write(
            local_refresh_lock_path(cache.path()),
            serde_json::to_string(&LocalRefreshLockFile {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                project_root: clean_path_text(project.path()),
                pid: std::process::id(),
                process_start_identity: None,
                started_at_epoch_ms: old_started,
                token: "live-old".to_string(),
            })
            .expect("lock json"),
        )
        .expect("write old live lock");
        fs::write(
            local_refresh_status_path(cache.path()),
            serde_json::to_string(&LocalRefreshStatus {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                status: "refreshing".to_string(),
                project_root: clean_path_text(project.path()),
                phase: "incremental_index".to_string(),
                pid: std::process::id(),
                process_start_identity: None,
                owner_token: None,
                started_at_epoch_ms: old_started,
                updated_at_epoch_ms: now_epoch_ms(),
                last_failure_reason: None,
            })
            .expect("status json"),
        )
        .expect("write active status");

        assert!(
            cleanup_stale_local_refresh_state(cache.path(), project.path()).is_none(),
            "fresh heartbeat should keep the local refresh lock owned"
        );
        match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("active refresh lock attempt")
        {
            LocalRefreshLockAttempt::Busy(busy) => {
                assert!(busy.status.is_some());
                assert_eq!(busy.pid, None);
            }
            LocalRefreshLockAttempt::Acquired(_) => panic!("active heartbeat must stay busy"),
        }
    }

    #[test]
    fn cleanup_stale_local_refresh_state_preserves_fresh_lock() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let old_started = now_epoch_ms() - LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64 - 1_000;
        fs::write(
            local_refresh_lock_path(cache.path()),
            serde_json::to_string(&LocalRefreshLockFile {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                project_root: clean_path_text(project.path()),
                pid: std::process::id(),
                process_start_identity: None,
                started_at_epoch_ms: now_epoch_ms(),
                token: "fresh".to_string(),
            })
            .expect("lock json"),
        )
        .expect("write fresh lock");
        fs::write(
            local_refresh_status_path(cache.path()),
            serde_json::to_string(&LocalRefreshStatus {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                status: "refreshing".to_string(),
                project_root: clean_path_text(project.path()),
                phase: "incremental_index".to_string(),
                pid: u32::MAX,
                process_start_identity: None,
                owner_token: None,
                started_at_epoch_ms: old_started,
                updated_at_epoch_ms: old_started,
                last_failure_reason: None,
            })
            .expect("status json"),
        )
        .expect("write stale status");

        let cleanup = cleanup_stale_local_refresh_state(cache.path(), project.path())
            .expect("stale status cleanup");
        assert_eq!(cleanup.reason, "stale_status");
        assert!(cleanup.removed_status_path);
        assert!(!cleanup.removed_lock_path);
        assert!(!local_refresh_status_path(cache.path()).exists());
        assert!(
            local_refresh_lock_path(cache.path()).exists(),
            "fresh local refresh lock should remain owned"
        );
    }

    #[test]
    fn cleanup_stale_local_refresh_state_reclaims_reused_pid() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let pid = std::process::id();
        let old = now_epoch_ms() - LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64 - 1_000;
        let reused_identity = Some("different-process-start".to_string());
        fs::write(
            local_refresh_lock_path(cache.path()),
            serde_json::to_string(&LocalRefreshLockFile {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                project_root: clean_path_text(project.path()),
                pid,
                process_start_identity: reused_identity.clone(),
                started_at_epoch_ms: old,
                token: "reused-pid".to_string(),
            })
            .expect("lock json"),
        )
        .expect("write reused-pid lock");
        fs::write(
            local_refresh_status_path(cache.path()),
            serde_json::to_string(&LocalRefreshStatus {
                schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
                status: "refreshing".to_string(),
                project_root: clean_path_text(project.path()),
                phase: "incremental_index".to_string(),
                pid,
                process_start_identity: reused_identity,
                owner_token: None,
                started_at_epoch_ms: old,
                updated_at_epoch_ms: old,
                last_failure_reason: None,
            })
            .expect("status json"),
        )
        .expect("write reused-pid status");

        let cleanup = cleanup_stale_local_refresh_state(cache.path(), project.path())
            .expect("reused-pid cleanup");
        assert_eq!(cleanup.reason, "stale_status_and_lock");
        assert!(cleanup.removed_status_path);
        assert!(cleanup.removed_lock_path);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_local_refresh_root_preserves_unix_case() {
        let parent = tempfile::tempdir().expect("parent");
        let project = parent.path().join("CaseSensitiveProject");
        fs::create_dir_all(&project).expect("project");
        assert!(clean_path_text(&project).ends_with("CaseSensitiveProject"));
    }
}
