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

/// The stage at which [`publish_new_private_file_atomic`] gave up, so a caller
/// can attach its own vocabulary to each outcome without repeating the
/// mechanism that produced it.
#[derive(Debug)]
pub enum PublishNewFileError {
    /// The destination has no parent directory to publish into.
    NoParent,
    /// The temporary prefix contained a path separator or traversal component,
    /// so the temporary file would not have been a sibling of the destination.
    UnsafeTempPrefix,
    /// No collision-free temporary name was available beside the destination.
    TempNamesExhausted,
    /// The temporary file could not be created.
    CreateTemp(std::io::Error),
    /// Writing, syncing, renaming, or making the rename durable failed. The
    /// temporary file has already been removed.
    Publish(std::io::Error),
}

impl std::fmt::Display for PublishNewFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoParent => formatter.write_str("destination has no parent directory"),
            Self::UnsafeTempPrefix => {
                formatter.write_str("temporary prefix is not a plain file-name component")
            }
            Self::TempNamesExhausted => {
                formatter.write_str("no free temporary name beside the destination")
            }
            Self::CreateTemp(error) => write!(formatter, "create temporary file: {error}"),
            Self::Publish(error) => write!(formatter, "publish temporary file: {error}"),
        }
    }
}

impl std::error::Error for PublishNewFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoParent | Self::TempNamesExhausted | Self::UnsafeTempPrefix => None,
            Self::CreateTemp(error) | Self::Publish(error) => Some(error),
        }
    }
}

/// How many temporary names are tried before giving up, so a wedged or hostile
/// directory cannot spin forever.
const PRIVATE_PUBLISH_ATTEMPTS: usize = 32;

/// Kept apart from [`TEMP_COUNTER`] so publications through this entry point
/// cannot perturb the names [`atomic_temp_path`] hands out.
static PRIVATE_PUBLISH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Publish `content` at `path` through an owner-only temporary file beside it.
///
/// The caller owns the directory, and must have proven it private before
/// calling: the content is written there in the clear before the rename, and
/// this never creates the directory. The caller also owns the check that
/// `path` does not already exist - the rename is what publishes, and this
/// reports only how that publication went.
///
/// The order is load-bearing. The content is written, `fsync`ed, and the
/// handle dropped *before* [`fs::rename`], so the rename can only ever expose
/// a complete file: a reader at the destination sees the old name or the whole
/// new one, never a partial write, on every platform. The rename stays a plain
/// [`fs::rename`] rather than the `ReplaceFileW` path in [`write_file_atomic`],
/// which exists to swap a destination that is already published. Afterwards
/// `sync_parent_directory` makes the new entry durable where the platform
/// supports it - and is the single place that decision is made, so it is not
/// linked here: it is private, and a public doc link to it fails the
/// deny-rustdoc-warnings gate. Any failure removes the temporary file and
/// leaves the destination as it was.
pub fn publish_new_private_file_atomic(
    path: &Path,
    temp_prefix: &str,
    content: &[u8],
) -> std::result::Result<(), PublishNewFileError> {
    let parent = path.parent().ok_or(PublishNewFileError::NoParent)?;
    // The prefix reaches a filename that is joined onto the destination's
    // parent, so a separator or traversal component in it would place the
    // temporary file outside the directory the caller validated.
    // `..` is a single component too, so the component must also be a normal
    // one rather than a traversal or a root.
    let mut components = Path::new(temp_prefix).components();
    let plain_component = matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    );
    if temp_prefix.contains(['/', '\\']) || !plain_component {
        return Err(PublishNewFileError::UnsafeTempPrefix);
    }
    for _ in 0..PRIVATE_PUBLISH_ATTEMPTS {
        let sequence = PRIVATE_PUBLISH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{temp_prefix}-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&temp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(PublishNewFileError::CreateTemp(error)),
        };
        let result = (|| {
            file.write_all(content)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)?;
            sync_parent_directory(path)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(PublishNewFileError::Publish(error));
        }
        return Ok(());
    }
    Err(PublishNewFileError::TempNamesExhausted)
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

/// Make an already-published name durable.
///
/// The rename is what makes a publication atomic; this step only decides
/// whether the new directory entry survives a crash, so it must never be able
/// to fail a publication that already happened.
///
/// Unix fsyncs the parent directory. Windows skips it, because `File::open`
/// does not pass `FILE_FLAG_BACKUP_SEMANTICS` and `CreateFileW` therefore
/// refuses a directory with `ERROR_ACCESS_DENIED`. That denial, not the
/// rename, is what failed the windows-x64 vulkan cell of calibration run
/// 30304210146 with `publish atomic qualification output: Access is denied.
/// (os error 5)` - in the two hand-rolled copies of this step that used to sit
/// in the embedding-qualification worker and its benchmark driver, and that
/// now publish through [`publish_new_private_file_atomic`] instead.
///
/// Opening the directory properly is not the alternative. Windows documents no
/// directory-fsync contract, and the behaviour bears that out: measured on
/// windows 11 26200 / NTFS, with and without backup-class privileges,
/// `FlushFileBuffers` on a backup-semantics handle is denied when the handle
/// is `GENERIC_READ` and accepted when it is `GENERIC_WRITE`, so what it would
/// commit is an accident of access mode rather than a guarantee. Windows
/// rename durability, if it is ever wanted, comes from
/// `MoveFileExW(.., MOVEFILE_WRITE_THROUGH)` - which
/// `codestory_cli::native_launcher` already uses - not from a directory fsync.
///
/// The other atomic publishers here already skip the step the same way
/// (`codestory_cli::native_launcher`, `codestory_llama_sys::native_staging`,
/// `codestory_store::sync_promotion_parent`,
/// `codestory_retrieval::sync_qualification_directory`).
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
    fn publishing_a_new_private_file_succeeds_and_leaves_only_the_complete_file() {
        // Regression: calibration run 30304210146, windows-x64 vulkan cell.
        // The embedding-qualification worker serialized, synced and renamed
        // `publication-fault-residency-1-worker-output.json` into place and
        // then exited 1 with `publish atomic qualification output: Access is
        // denied. (os error 5)`, because its own copy of the post-rename
        // durability step opened the parent directory with `File::open`, which
        // Windows refuses without FILE_FLAG_BACKUP_SEMANTICS. The preserved
        // failure evidence holds the complete published output next to the
        // failed command, so the publication had already happened when the
        // step reported denial.
        //
        // That copy and its twin in the benchmark driver now publish through
        // here, so `sync_parent_directory` is the single place the decision is
        // made. Reverting it to an unconditional `File::open(parent)?
        // .sync_all()` fails this test with the calibration error.
        //
        // The revert value is windows-only. On unix the reverted body is
        // exactly the `#[cfg(not(windows))]` arm this already exercises, so a
        // unix-only lane passes with or without the fix; the revert-proof is
        // run on windows.
        let directory = tempfile::tempdir().expect("publication directory");
        // Callers publish into a directory they have proven private; reproduce
        // that here so the publish is exercised under its real preconditions.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("make the publication directory private");
        }
        let path = directory.path().join("worker-output.json");

        publish_new_private_file_atomic(&path, "codestory-test", b"{\"schema_version\":2}\n")
            .expect("publish new private file");

        assert_eq!(
            fs::read(&path).expect("read published file"),
            b"{\"schema_version\":2}\n"
        );
        // The rename carries the temporary file's inode, so the published mode
        // is the 0o600 the temporary was created with.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path)
                    .expect("published metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let residue = fs::read_dir(directory.path())
            .expect("list publication directory")
            .map(|entry| entry.expect("directory entry").file_name())
            .filter(|name| name != "worker-output.json")
            .collect::<Vec<_>>();
        assert!(
            residue.is_empty(),
            "publish left temporaries beside the file: {residue:?}"
        );
    }

    #[test]
    fn failed_publication_removes_the_temporary_and_spares_the_destination() {
        let directory = tempfile::tempdir().expect("publication directory");
        // A non-empty directory at the destination is a rename target no
        // platform will accept, which fails the publish after the temporary
        // file has been written and synced.
        let destination = directory.path().join("occupied");
        fs::create_dir(&destination).expect("destination directory");
        fs::write(destination.join("resident"), b"resident").expect("resident file");

        let error = publish_new_private_file_atomic(&destination, "codestory-test", b"new")
            .expect_err("publishing onto a non-empty directory must fail");

        assert!(
            matches!(error, PublishNewFileError::Publish(_)),
            "{error:?}"
        );
        assert_eq!(
            fs::read(destination.join("resident")).expect("read resident file"),
            b"resident"
        );
        let residue = fs::read_dir(directory.path())
            .expect("list publication directory")
            .map(|entry| entry.expect("directory entry").file_name())
            .filter(|name| name != "occupied")
            .collect::<Vec<_>>();
        assert!(
            residue.is_empty(),
            "failed publish left temporaries behind: {residue:?}"
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

    #[test]
    fn a_temp_prefix_that_escapes_the_directory_is_refused() {
        // The prefix reaches a filename joined onto the destination's parent, so
        // anything that is not a plain component could publish the temporary file
        // outside the directory the caller validated.
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("published.json");
        for prefix in ["../escape", "nested/prefix", "..", "", "a\\b"] {
            let error = publish_new_private_file_atomic(&destination, prefix, b"{}")
                .expect_err("an escaping prefix must be refused");
            assert!(
                matches!(error, PublishNewFileError::UnsafeTempPrefix),
                "prefix {prefix:?} produced {error:?}"
            );
        }
        assert!(
            !destination.exists(),
            "a refused publish must write nothing"
        );
    }
}
