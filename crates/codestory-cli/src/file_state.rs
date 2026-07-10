use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static ATOMIC_JSON_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn write_synced_new_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn atomic_json_path(path: &Path, stem: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{stem}.{}.{}.tmp", std::process::id(), counter))
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, stem: &str, value: &T) -> Result<()> {
    let _write_guard = ATOMIC_JSON_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = atomic_json_path(path, stem);
    let content = serde_json::to_string_pretty(value)?;
    if let Err(error) = write_synced_new_file(&temp_path, content.as_bytes()) {
        let _ = fs::remove_file(&temp_path);
        return Err(error.into());
    }
    match replace_file(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(error.into())
        }
    }
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

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_UNABLE_TO_REMOVE_REPLACED: i32 = 1175;
    const ERROR_UNABLE_TO_MOVE_REPLACEMENT: i32 = 1176;
    const REPLACE_ATTEMPTS: usize = 50;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    let replacement = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    for attempt in 0..REPLACE_ATTEMPTS {
        if !destination.exists() {
            match fs::rename(source, destination) {
                Ok(()) => return Ok(()),
                Err(_) if !source.exists() && destination.exists() => return Ok(()),
                Err(error) if !destination.exists() => {
                    if attempt + 1 == REPLACE_ATTEMPTS {
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(_) => {}
            }
        }

        // SAFETY: both path buffers are null-terminated and remain alive for the call.
        let result = unsafe {
            ReplaceFileW(
                replaced.as_ptr(),
                replacement.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if result != 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        if !source.exists() && destination.exists() {
            return Ok(());
        }
        let retryable = matches!(
            error.raw_os_error(),
            Some(
                ERROR_FILE_NOT_FOUND
                    | ERROR_ACCESS_DENIED
                    | ERROR_SHARING_VIOLATION
                    | ERROR_UNABLE_TO_REMOVE_REPLACED
                    | ERROR_UNABLE_TO_MOVE_REPLACEMENT
            )
        );
        if !retryable || attempt + 1 == REPLACE_ATTEMPTS {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    unreachable!("replacement attempts always return")
}

pub(crate) fn file_modified_age_exceeds(path: &Path, ttl: Duration, now_epoch_ms: i64) -> bool {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| {
            let modified_ms = modified.as_millis().min(i64::MAX as u128) as i64;
            now_epoch_ms.saturating_sub(modified_ms) > ttl.as_millis() as i64
        })
        .unwrap_or(true)
}
