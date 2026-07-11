use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process_start_identity: Option<String>,
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
    let process_start_identity = crate::ready_repair_status::recorded_process_start_identity(pid);
    let token = format!("{pid}:{started_at_epoch_ms}");
    let lock = LocalRefreshLockFile {
        schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
        project_root: clean_path_text(project_root),
        pid,
        process_start_identity,
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
    let status = LocalRefreshStatus {
        schema_version: LOCAL_REFRESH_STATUS_SCHEMA_VERSION,
        status: status.to_string(),
        project_root: clean_path_text(project_root),
        phase: phase.to_string(),
        pid,
        process_start_identity: crate::ready_repair_status::recorded_process_start_identity(pid),
        started_at_epoch_ms,
        updated_at_epoch_ms: now_epoch_ms(),
        last_failure_reason,
    };
    crate::file_state::write_json_atomic(&path, "local-refresh-status", &status)
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
    let status_age_stale = status.as_ref().is_some_and(|status| {
        now.saturating_sub(status.updated_at_epoch_ms)
            > LOCAL_REFRESH_LOCK_STALE_TTL.as_millis() as i64
    });
    let status_stale = status_owner_dead || (status_age_stale && !status_owner_live);
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
    crate::file_state::write_synced_new_file(path, content)
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
        || !same_path_text(Path::new(&status.project_root), project_root)
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
    Some(status)
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
}
