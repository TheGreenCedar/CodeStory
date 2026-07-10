use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

static ATOMIC_JSON_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn write_synced_new_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    codestory_workspace::atomic_file::write_synced_new_file(path, content)
}

pub(crate) fn atomic_json_path(path: &Path, stem: &str) -> PathBuf {
    codestory_workspace::atomic_file::atomic_temp_path(path, stem)
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, stem: &str, value: &T) -> Result<()> {
    let _write_guard = ATOMIC_JSON_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let content = serde_json::to_vec_pretty(value)?;
    codestory_workspace::atomic_file::write_bytes_atomic(path, stem, &content)
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    const READ_ATTEMPTS: usize = 10;

    for attempt in 0..READ_ATTEMPTS {
        match read_file(path) {
            Ok(content) => match serde_json::from_slice(&content) {
                Ok(value) => return Some(value),
                Err(_) if attempt + 1 < READ_ATTEMPTS => {}
                Err(_) => return None,
            },
            Err(error)
                if attempt + 1 < READ_ATTEMPTS && publication_read_error_is_transient(&error) => {}
            Err(_) => return None,
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    None
}

fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ_WRITE_DELETE: u32 = 0x1 | 0x2 | 0x4;
        options.share_mode(FILE_SHARE_READ_WRITE_DELETE);
    }
    let mut file = options.open(path)?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    Ok(content)
}

fn publication_read_error_is_transient(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::NotFound {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(5 | 32))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn file_modified_age_exceeds(path: &Path, ttl: Duration, now_epoch_ms: i64) -> bool {
    std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| {
            let modified_ms = modified.as_millis().min(i64::MAX as u128) as i64;
            now_epoch_ms.saturating_sub(modified_ms) > ttl.as_millis() as i64
        })
        .unwrap_or(true)
}
