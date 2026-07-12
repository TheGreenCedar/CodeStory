use anyhow::{Context, Result, bail};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::time::Duration;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn atomic_temp_path(path: &Path, stem: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{stem}.{}.{}.tmp", std::process::id(), counter))
}

pub fn write_synced_new_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

pub fn write_bytes_atomic(path: &Path, stem: &str, content: &[u8]) -> Result<()> {
    write_file_atomic(
        path,
        stem,
        |file| file.write_all(content).context("write temporary file"),
        |temp_path| {
            let actual = fs::read(temp_path).context("read temporary file for validation")?;
            if actual != content {
                bail!("temporary file validation failed: written bytes differ from input");
            }
            Ok(())
        },
    )
}

/// Publish a fully written temporary file with the same cross-platform replacement semantics as
/// [`write_file_atomic`]. The caller owns validation and must place the temporary file beside the
/// destination so the replacement stays on one filesystem.
pub fn publish_existing_file_atomic(temp_path: &Path, path: &Path) -> Result<()> {
    replace_file(temp_path, path).with_context(|| format!("publish {}", path.display()))?;
    sync_parent_directory(path).with_context(|| format!("sync parent of {}", path.display()))
}

pub fn write_file_atomic(
    path: &Path,
    stem: &str,
    write: impl FnOnce(&mut File) -> Result<()>,
    validate: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent directory {}", parent.display()))?;
    }
    let (temp_path, mut file) = create_unique_temp_file(path, stem)?;
    let result = (|| {
        write(&mut file)?;
        file.sync_all()
            .with_context(|| format!("sync temporary file {}", temp_path.display()))?;
        drop(file);
        validate(&temp_path)?;
        replace_file(&temp_path, path).with_context(|| format!("publish {}", path.display()))?;
        sync_parent_directory(path)
            .with_context(|| format!("sync parent of {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// Reserve a collision-free temporary file beside `path` using create-new semantics.
pub fn create_unique_temp_file(path: &Path, stem: &str) -> Result<(PathBuf, File)> {
    loop {
        let temp_path = atomic_temp_path(path, stem);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create temporary file {}", temp_path.display()));
            }
        }
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

        // `ReplaceFileW` does not add the extended-length prefix that Rust's
        // filesystem APIs apply internally. Canonicalization supplies that
        // form so isolated test and user cache paths can exceed MAX_PATH.
        let (replacement_path, replaced_path) =
            match (fs::canonicalize(source), fs::canonicalize(destination)) {
                (Ok(replacement), Ok(replaced)) => (replacement, replaced),
                (source_result, destination_result) => {
                    if !source.exists() && destination.exists() {
                        return Ok(());
                    }
                    let error = source_result
                        .err()
                        .or_else(|| destination_result.err())
                        .expect("one canonical path failed");
                    if attempt + 1 == REPLACE_ATTEMPTS {
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
            };
        let replacement = replacement_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let replaced = replaced_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();

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

#[cfg(not(windows))]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) => File::open(parent)?.sync_all(),
        None => Ok(()),
    }
}

#[cfg(windows)]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_complete_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        fs::write(&path, b"old").expect("old file");

        write_bytes_atomic(&path, "state", b"new").expect("atomic write");

        assert_eq!(fs::read(&path).expect("read"), b"new");
    }

    #[test]
    fn failed_write_or_validation_preserves_destination() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        fs::write(&path, b"old").expect("old file");

        let write_error = write_file_atomic(
            &path,
            "state",
            |file| {
                file.write_all(b"partial")?;
                bail!("short write")
            },
            |_| Ok(()),
        );
        assert!(write_error.is_err());
        assert_eq!(fs::read(&path).expect("read after write error"), b"old");

        let validation_error = write_file_atomic(
            &path,
            "state",
            |file| file.write_all(b"new").map_err(Into::into),
            |_| bail!("invalid"),
        );
        assert!(validation_error.is_err());
        assert_eq!(
            fs::read(&path).expect("read after validation error"),
            b"old"
        );
    }

    #[test]
    fn unique_temp_creation_skips_stale_collision() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        let next = TEMP_COUNTER.load(Ordering::Relaxed);
        let stale = path.with_file_name(format!(".state.{}.{}.tmp", std::process::id(), next));
        fs::write(&stale, b"stale").expect("stale collision");

        let (created, file) = create_unique_temp_file(&path, "state").expect("unique temp");
        drop(file);

        assert_ne!(created, stale);
        assert_eq!(fs::read(stale).expect("stale preserved"), b"stale");
        assert!(created.is_file());
    }

    #[cfg(windows)]
    #[test]
    fn blocked_windows_replacement_preserves_destination() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        fs::write(&path, b"old").expect("old file");
        let exclusive_reader = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("exclusive reader");

        let error = write_bytes_atomic(&path, "state", b"new")
            .expect_err("replacement must fail while destination is exclusively open");

        assert!(error.to_string().contains("publish"));
        drop(exclusive_reader);
        assert_eq!(fs::read(&path).expect("read old file"), b"old");
    }

    #[cfg(windows)]
    #[test]
    fn atomic_write_replaces_existing_file_beyond_max_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let long = "segment".repeat(12);
        let parent = dir.path().join(&long).join(&long).join(&long);
        fs::create_dir_all(&parent).expect("long parent");
        let path = parent.join("state.json");
        assert!(path.as_os_str().encode_wide().count() > 260);
        fs::write(&path, b"old").expect("old file");

        write_bytes_atomic(&path, "state", b"new").expect("atomic long-path write");

        assert_eq!(fs::read(&path).expect("read new file"), b"new");
    }
}
