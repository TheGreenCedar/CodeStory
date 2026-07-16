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

/// Native filesystem identity captured for a trusted publication directory.
///
/// The token is intentionally opaque. Callers capture it while making their
/// trust decision, then require [`OwnedFilePublicationRoot::open_verified`] to
/// prove that the opened directory handle names the same filesystem object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedFilePublicationIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume_serial_number: u64,
    #[cfg(windows)]
    file_id: [u8; 16],
}

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
    identity: OwnedFilePublicationIdentity,
    #[cfg(windows)]
    stable_path: std::path::PathBuf,
}

impl OwnedFilePublicationRoot {
    /// Capture the native identity used to establish a trusted directory.
    pub fn capture_identity(root: &Path) -> io::Result<OwnedFilePublicationIdentity> {
        let root = open_root(root)?;
        identity_from_file(&root)
    }

    /// Capture and pin a directory without a separate caller trust decision.
    pub fn open(root: &Path) -> io::Result<Self> {
        let identity = Self::capture_identity(root)?;
        Self::open_verified(root, identity)
    }

    /// Pin a directory only when its handle matches a previously captured identity.
    pub fn open_verified(
        root: &Path,
        expected_identity: OwnedFilePublicationIdentity,
    ) -> io::Result<Self> {
        let root_handle = open_root(root)?;
        let opened_identity = identity_from_file(&root_handle)?;
        if opened_identity != expected_identity {
            return Err(io::Error::other(
                "owned publication root identity changed before it was pinned",
            ));
        }
        #[cfg(windows)]
        let stable_path = fs::canonicalize(root)?;
        Ok(Self {
            root: root_handle,
            identity: opened_identity,
            #[cfg(windows)]
            stable_path,
        })
    }

    /// Return whether an existing path still names the pinned directory object.
    pub fn matches_path(&self, path: &Path) -> io::Result<bool> {
        Ok(Self::capture_identity(path)? == self.identity)
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
fn identity_from_file(file: &File) -> io::Result<OwnedFilePublicationIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = file.metadata()?;
    Ok(OwnedFilePublicationIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn identity_from_file(file: &File) -> io::Result<OwnedFilePublicationIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };

    let mut information = MaybeUninit::<FILE_ID_INFO>::uninit();
    // SAFETY: `file` owns a valid handle for the duration of the call and the
    // output points to correctly sized, writable storage.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            information.as_mut_ptr().cast(),
            u32::try_from(std::mem::size_of::<FILE_ID_INFO>()).expect("FILE_ID_INFO size fits u32"),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful `GetFileInformationByHandleEx` initializes all fields.
    let information = unsafe { information.assume_init() };
    Ok(OwnedFilePublicationIdentity {
        volume_serial_number: information.VolumeSerialNumber,
        file_id: information.FileId.Identifier,
    })
}

#[cfg(not(any(unix, windows)))]
fn identity_from_file(_file: &File) -> io::Result<OwnedFilePublicationIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "owned publication identity is unsupported on this platform",
    ))
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

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_a_replacement_between_identity_capture_and_open() {
        let root = tempdir().expect("create test root");
        let trusted = root.path().join("trusted");
        let displaced = root.path().join("displaced");
        fs::create_dir(&trusted).expect("create trusted root");
        let identity =
            OwnedFilePublicationRoot::capture_identity(&trusted).expect("capture trusted identity");

        fs::rename(&trusted, &displaced).expect("displace trusted root");
        fs::create_dir(&trusted).expect("create replacement root");
        let error = OwnedFilePublicationRoot::open_verified(&trusted, identity)
            .expect_err("replacement root must not satisfy captured identity");
        assert!(error.to_string().contains("identity changed"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn publication_remains_bound_to_the_opened_directory() {
        let root = tempdir().expect("create test root");
        let trusted = root.path().join("trusted");
        let displaced = root.path().join("displaced");
        fs::create_dir(&trusted).expect("create trusted root");
        let publication =
            OwnedFilePublicationRoot::open(&trusted).expect("pin trusted publication root");

        let rename = fs::rename(&trusted, &displaced);
        #[cfg(unix)]
        {
            rename.expect("rename opened directory");
            fs::create_dir(&trusted).expect("create replacement root");
            publication
                .publish_new_bytes("fixture.json".as_ref(), "fixture", b"pinned")
                .expect("publish through retained directory handle");
            assert_eq!(
                fs::read(displaced.join("fixture.json")).expect("read pinned publication"),
                b"pinned"
            );
            assert!(!trusted.join("fixture.json").exists());
        }
        #[cfg(windows)]
        {
            rename.expect_err("opened directory must reject replacement on Windows");
            publication
                .publish_new_bytes("fixture.json".as_ref(), "fixture", b"pinned")
                .expect("publish through retained directory handle");
            assert_eq!(
                fs::read(trusted.join("fixture.json")).expect("read pinned publication"),
                b"pinned"
            );
            assert!(!displaced.exists());
        }
    }
}
