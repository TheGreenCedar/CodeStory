use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOCAL_REFRESH_STATUS_FILE: &str = "local-refresh-status.json";
const LOCAL_REFRESH_LOCK_FILE: &str = "local-refresh.lock";
const LOCAL_REFRESH_STATUS_SCHEMA_VERSION: u32 = 1;
const LOCAL_REFRESH_STATUS_TTL: Duration = Duration::from_secs(30);
const LOCAL_REFRESH_LOCK_STALE_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LocalRefreshStatus {
    pub(crate) schema_version: u32,
    pub(crate) status: String,
    pub(crate) project_root: String,
    pub(crate) phase: String,
    pub(crate) pid: u32,
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
    started_at_epoch_ms: i64,
}

impl Drop for LocalRefreshLock {
    fn drop(&mut self) {
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
}

pub(crate) fn try_acquire_local_refresh_lock(
    cache_root: &Path,
    project_root: &Path,
) -> Result<LocalRefreshLockAttempt> {
    let path = local_refresh_lock_path(cache_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let started_at_epoch_ms = now_epoch_ms();
    let pid = std::process::id();
    let token = format!("{pid}:{started_at_epoch_ms}");
    let lock = LocalRefreshLockFile {
        schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
        project_root: clean_path_text(project_root),
        pid,
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
    if !local_refresh_lock_file_is_stale(&path) {
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

pub(crate) fn write_local_refresh_status(
    cache_root: &Path,
    project_root: &Path,
    status: &str,
    phase: &str,
    started_at_epoch_ms: i64,
    pid: u32,
    last_failure_reason: Option<String>,
) -> Result<()> {
    let path = local_refresh_status_path(cache_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let status = LocalRefreshStatus {
        schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
        status: status.to_string(),
        project_root: clean_path_text(project_root),
        phase: phase.to_string(),
        pid,
        started_at_epoch_ms,
        updated_at_epoch_ms: now_epoch_ms(),
        last_failure_reason,
    };
    Ok(fs::write(path, serde_json::to_string_pretty(&status)?)?)
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
    let status_path = local_refresh_status_path(cache_root);
    let lock_path = local_refresh_lock_path(cache_root);
    let now = now_epoch_ms();
    let status = read_local_refresh_status_file(&status_path).filter(|status| {
        status.schema_version == LOCAL_REFRESH_STATUS_SCHEMA_VERSION
            && status.status == "refreshing"
            && same_path_text(Path::new(&status.project_root), project_root)
    });
    let lock = read_local_refresh_lock_file(&lock_path).filter(|lock| {
        lock.schema_version == LOCAL_REFRESH_STATUS_SCHEMA_VERSION
            && same_path_text(Path::new(&lock.project_root), project_root)
    });
    let status_stale = status.as_ref().is_some_and(|status| {
        !crate::ready_repair_status::process_is_running(status.pid)
            || now.saturating_sub(status.updated_at_epoch_ms)
                > LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64
    });
    let lock_stale = lock
        .as_ref()
        .is_some_and(|_| local_refresh_lock_file_is_stale(&lock_path));
    if !status_stale && !lock_stale {
        return None;
    }

    let reason = match (status_stale, lock_stale) {
        (true, true) => "stale_status_and_lock",
        (true, false) => "stale_status",
        (false, true) => "stale_lock",
        (false, false) => "clean",
    };
    let lock_matches_stale_status = status_stale
        && status
            .as_ref()
            .zip(lock.as_ref())
            .is_some_and(|(status, lock)| {
                status.pid == lock.pid && status.started_at_epoch_ms == lock.started_at_epoch_ms
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
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn local_refresh_status_path(cache_root: &Path) -> PathBuf {
    cache_root.join(LOCAL_REFRESH_STATUS_FILE)
}

fn local_refresh_lock_path(cache_root: &Path) -> PathBuf {
    cache_root.join(LOCAL_REFRESH_LOCK_FILE)
}

fn local_refresh_lock_file_is_stale(path: &Path) -> bool {
    let now = now_epoch_ms();
    if let Some(lock) = read_local_refresh_lock_file(path) {
        if !crate::ready_repair_status::process_is_running(lock.pid) {
            return true;
        }
        return now.saturating_sub(lock.started_at_epoch_ms)
            > LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64;
    }
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| {
            let modified_ms = modified.as_millis().min(i64::MAX as u128) as i64;
            now.saturating_sub(modified_ms) > LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64
        })
        .unwrap_or(true)
}

fn read_local_refresh_status(
    path: &Path,
    project_root: &Path,
    now_epoch_ms: i64,
) -> Option<LocalRefreshStatus> {
    let status = read_local_refresh_status_file(path)?;
    if status.schema_version != LOCAL_REFRESH_STATUS_SCHEMA_VERSION
        || status.status != "refreshing"
        || !same_path_text(Path::new(&status.project_root), project_root)
    {
        return None;
    }
    let age_ms = now_epoch_ms.saturating_sub(status.updated_at_epoch_ms);
    if age_ms > LOCAL_REFRESH_STATUS_TTL.as_millis() as i64 {
        return None;
    }
    Some(status)
}

fn read_local_refresh_status_file(path: &Path) -> Option<LocalRefreshStatus> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
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
        write_local_refresh_status(
            cache.path(),
            project.path(),
            "refreshing",
            "incremental_index",
            now_epoch_ms(),
            4242,
            None,
        )
        .expect("write refresh status");

        match try_acquire_local_refresh_lock(cache.path(), project.path())
            .expect("second lock attempt")
        {
            LocalRefreshLockAttempt::Busy(busy) => {
                let status = busy.status.expect("active status");
                assert_eq!(status.phase, "incremental_index");
                assert_eq!(status.pid, 4242);
            }
            LocalRefreshLockAttempt::Acquired(_) => panic!("second lock must not be acquired"),
        }
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
                pid: 4242,
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
                pid: 4242,
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
                pid: std::process::id(),
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
                started_at_epoch_ms: old_started,
                updated_at_epoch_ms: old_started,
                last_failure_reason: None,
            })
            .expect("status json"),
        )
        .expect("write stale status");

        let cleanup = cleanup_stale_local_refresh_state(cache.path(), project.path())
            .expect("stale local refresh cleanup");
        assert_eq!(cleanup.reason, "stale_status_and_lock");
        assert!(cleanup.removed_status_path);
        assert!(cleanup.removed_lock_path);
        assert!(!local_refresh_status_path(cache.path()).exists());
        assert!(!local_refresh_lock_path(cache.path()).exists());
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
}
