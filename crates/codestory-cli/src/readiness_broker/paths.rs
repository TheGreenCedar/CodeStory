use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::display;
use crate::ready_repair_status;

pub(crate) const BROKER_DIR: &str = "readiness-broker";
pub(crate) const BROKER_SNAPSHOT_FILE: &str = "snapshot.json";
pub(crate) const MACHINE_RESOURCE_DIR: &str = "machine";

pub(crate) fn broker_snapshot_path(canonical_root_hash: &str) -> PathBuf {
    broker_cache_root()
        .join(BROKER_DIR)
        .join("projects")
        .join(canonical_root_hash)
        .join(BROKER_SNAPSHOT_FILE)
}

pub(crate) fn machine_resource_lock_path(resource: &str) -> PathBuf {
    broker_cache_root()
        .join(BROKER_DIR)
        .join(MACHINE_RESOURCE_DIR)
        .join(format!("{}.lock", safe_name(resource)))
}

pub(crate) fn machine_resource_reaper_lock_path(resource: &str) -> PathBuf {
    broker_cache_root()
        .join(BROKER_DIR)
        .join(MACHINE_RESOURCE_DIR)
        .join(format!("{}.reap.lock", safe_name(resource)))
}

pub(crate) fn machine_resource_reaper_takeover_lock_path(resource: &str) -> PathBuf {
    broker_cache_root()
        .join(BROKER_DIR)
        .join(MACHINE_RESOURCE_DIR)
        .join(format!("{}.reap.takeover.lock", safe_name(resource)))
}

pub(crate) fn machine_resource_cache_fingerprint(resource: &str) -> String {
    format!(
        "lock:{}|reaper:{}",
        path_fingerprint(&machine_resource_lock_path(resource)),
        path_fingerprint(&machine_resource_reaper_lock_path(resource))
    )
}

pub(crate) fn path_fingerprint(path: &Path) -> String {
    let Ok(metadata) = fs::metadata(path) else {
        return "missing".to_string();
    };
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let content_hash = fs::read(path)
        .map(|bytes| {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            format!("{:x}", hasher.finalize())
        })
        .unwrap_or_else(|error| format!("read_error:{error}"));
    format!(
        "len:{}:mtime_ns:{}:sha256:{}",
        metadata.len(),
        modified_ns,
        &content_hash[..content_hash.len().min(16)]
    )
}

pub(crate) fn install_id() -> String {
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

#[cfg(not(test))]
pub(crate) fn broker_cache_root() -> PathBuf {
    crate::sidecar_runtime::local()
        .layout
        .state_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
pub(crate) fn broker_cache_root() -> PathBuf {
    crate::sidecar_runtime::prepare_cache_access();
    if let Some(root) = codestory_retrieval::active_test_cache_root() {
        return root;
    }
    static PROCESS_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let process_root = PROCESS_ROOT.get_or_init(|| {
        let nonce = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir()
            .join("codestory-cli-unit-tests")
            .join(format!("{}-{nonce}", std::process::id()))
    });
    let thread = std::thread::current();
    let label = thread
        .name()
        .map(safe_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{:?}", thread.id()));
    let prefix = &label[..label.len().min(32)];
    let root = process_root.join(format!("{prefix}-{}", &hash_text(&label)[..12]));
    std::fs::create_dir_all(root.join("sidecars")).expect("create broker test state root");
    root
}

#[cfg(test)]
pub(crate) fn with_test_broker_root<T>(task: impl FnOnce() -> T) -> T {
    codestory_retrieval::with_test_cache_root(&broker_cache_root(), task)
}

#[cfg(test)]
pub(crate) fn spawn_with_test_broker_root<T: Send + 'static>(
    task: impl FnOnce() -> T + Send + 'static,
) -> std::thread::JoinHandle<T> {
    let root = broker_cache_root();
    std::thread::spawn(move || codestory_retrieval::with_test_cache_root(&root, task))
}

pub(crate) fn safe_name(value: &str) -> String {
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

pub(crate) fn clean_path(path: &Path) -> String {
    display::clean_path_string(&path.to_string_lossy())
}

pub(crate) fn clean_path_text(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

pub(crate) fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn now_epoch_ms() -> i64 {
    ready_repair_status::now_epoch_ms()
}
