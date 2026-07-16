//! Handle-bound, no-replace publication below an already trusted directory.

use fs_at::{OpenOptions as AtOpenOptions, OpenOptionsWriteMode};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A publication boundary pinned to one open directory handle.
///
/// Callers establish the directory identity before opening this boundary, then
/// retain it through staging, validation, and the final no-replace link. On
/// Unix every operation is relative to the open directory. On Windows the
/// directory is held without delete sharing, so its canonical path cannot be
/// renamed or replaced while the final hard link is created.
#[derive(Debug)]
pub struct OwnedFilePublicationRoot {
    root: File,
    #[cfg(windows)]
    stable_path: std::path::PathBuf,
}

impl OwnedFilePublicationRoot {
    /// Pin a directory that the caller has already established as trusted.
    pub fn open(root: &Path) -> io::Result<Self> {
        let root_handle = open_root(root)?;
        #[cfg(windows)]
        let stable_path = fs::canonicalize(root)?;
        Ok(Self {
            root: root_handle,
            #[cfg(windows)]
            stable_path,
        })
    }

    /// Publish complete bytes at a new leaf name without replacing any entry.
    pub fn publish_new_bytes(
        &self,
        file_name: &OsStr,
        temp_stem: &str,
        bytes: &[u8],
    ) -> io::Result<()> {
        validate_leaf_name(file_name)?;
        validate_leaf_name(OsStr::new(temp_stem))?;

        let (temp_name, mut temp_file) = self.create_unique_temp_file(temp_stem)?;
        let write_result = (|| {
            temp_file.write_all(bytes)?;
            temp_file.sync_all()?;
            verify_file_bytes(&mut temp_file, bytes)
        })();
        drop(temp_file);
        if let Err(error) = write_result {
            let _ = self.unlink(&temp_name);
            return Err(error);
        }

        let publish_result = self.link_new(&temp_name, file_name);
        let _ = self.unlink(&temp_name);
        publish_result?;
        self.sync_root()
    }

    fn create_unique_temp_file(&self, stem: &str) -> io::Result<(OsString, File)> {
        loop {
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = OsString::from(format!(".{stem}.{}.{}.tmp", std::process::id(), counter));
            let mut options = AtOpenOptions::default();
            options
                .read(true)
                .write(OpenOptionsWriteMode::Write)
                .create_new(true)
                .follow(false);
            match options.open_at(&self.root, Path::new(&name)) {
                Ok(file) => return Ok((name, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn unlink(&self, name: &OsStr) -> io::Result<()> {
        AtOpenOptions::default().unlink_at(&self.root, Path::new(name))
    }

    #[cfg(unix)]
    fn link_new(&self, source: &OsStr, destination: &OsStr) -> io::Result<()> {
        use std::ffi::CString;
        use std::os::fd::AsRawFd as _;
        use std::os::unix::ffi::OsStrExt as _;

        let source = CString::new(source.as_bytes()).map_err(invalid_name)?;
        let destination = CString::new(destination.as_bytes()).map_err(invalid_name)?;
        // SAFETY: both names are null-terminated, contain no interior nulls,
        // and are resolved relative to the live directory descriptor.
        if unsafe {
            libc::linkat(
                self.root.as_raw_fd(),
                source.as_ptr(),
                self.root.as_raw_fd(),
                destination.as_ptr(),
                0,
            )
        } == 0
        {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(windows)]
    fn link_new(&self, source: &OsStr, destination: &OsStr) -> io::Result<()> {
        fs::hard_link(
            self.stable_path.join(Path::new(source)),
            self.stable_path.join(Path::new(destination)),
        )
    }

    #[cfg(not(any(unix, windows)))]
    fn link_new(&self, _source: &OsStr, _destination: &OsStr) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "handle-bound publication is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    fn sync_root(&self) -> io::Result<()> {
        self.root.sync_all()
    }

    #[cfg(not(unix))]
    fn sync_root(&self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_leaf_name(name: &OsStr) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "owned publication requires one relative leaf name: {}",
                Path::new(name).display()
            ),
        ))
    }
}

fn verify_file_bytes(file: &mut File, expected: &[u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut offset = 0usize;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let end = offset
            .checked_add(count)
            .ok_or_else(|| io::Error::other("published file length overflow"))?;
        if end > expected.len() || buffer[..count] != expected[offset..end] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "temporary publication bytes differ from input",
            ));
        }
        offset = end;
    }
    if offset == expected.len() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "temporary publication bytes differ from input",
        ))
    }
}

#[cfg(unix)]
fn invalid_name(error: std::ffi::NulError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error)
}

#[cfg(unix)]
fn open_root(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(windows)]
fn open_root(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let root = options.open(path)?;
    let metadata = root.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other(
            "owned publication root is not a directory",
        ));
    }
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("owned publication refuses reparse points"));
    }
    Ok(root)
}

#[cfg(not(any(unix, windows)))]
fn open_root(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(test)]
mod tests {
    use super::OwnedFilePublicationRoot;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn publishes_complete_bytes_without_replacement() {
        let root = tempdir().expect("create publication root");
        let publication =
            OwnedFilePublicationRoot::open(root.path()).expect("pin publication root");
        publication
            .publish_new_bytes("fixture.json".as_ref(), "fixture", b"complete")
            .expect("publish fixture");
        assert_eq!(
            fs::read(root.path().join("fixture.json")).expect("read fixture"),
            b"complete"
        );

        let error = publication
            .publish_new_bytes("fixture.json".as_ref(), "fixture", b"replacement")
            .expect_err("published destination must not be replaced");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(root.path().join("fixture.json")).expect("read preserved fixture"),
            b"complete"
        );
        assert_eq!(fs::read_dir(root.path()).expect("read root").count(), 1);
    }

    #[test]
    fn rejects_non_leaf_destinations() {
        let root = tempdir().expect("create publication root");
        let publication =
            OwnedFilePublicationRoot::open(root.path()).expect("pin publication root");
        for name in ["", ".", "..", "nested/fixture.json"] {
            let error = publication
                .publish_new_bytes(name.as_ref(), "fixture", b"blocked")
                .expect_err("non-leaf destination must be rejected");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }
}
