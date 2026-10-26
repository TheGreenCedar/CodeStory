//! Confined read-only subset of the reviewed handle-relative source reader.
use super::SourceContextDeclineReason as Reason;
use codestory_indexer::CancellationToken;
use codestory_workspace::{
    WorkspacePathIdentity, workspace_file_identity, workspace_path_identity,
};
use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

pub(super) struct Directory {
    file: File,
    logical: PathBuf,
    canonical: PathBuf,
    identity: WorkspacePathIdentity,
}

pub(super) struct SourceRead {
    pub bytes: Vec<u8>,
    pub identity: WorkspacePathIdentity,
}

fn check(cancellation: &CancellationToken) -> Result<(), Reason> {
    if cancellation.is_cancelled() {
        Err(Reason::Cancelled)
    } else {
        Ok(())
    }
}

fn name(value: &OsStr) -> Result<CString, Reason> {
    CString::new(value.as_bytes()).map_err(|_| Reason::InvalidPath)
}

fn open_at(parent: &File, value: &OsStr, directory: bool) -> Result<File, Reason> {
    let value = name(value)?;
    let flags = libc::O_RDONLY
        | libc::O_NONBLOCK
        | libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent owns a live descriptor; value remains NUL-terminated for
    // openat. A successful descriptor transfers exactly once into File.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), value.as_ptr(), flags) };
    if fd < 0 {
        return Err(Reason::UnreadableSource);
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn named_metadata(parent: &File, value: &OsStr) -> Result<libc::stat, Reason> {
    let value = name(value)?;
    let mut out = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: all buffers are live; only successful fstatat initializes out.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            value.as_ptr(),
            out.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(Reason::UnreadableSource);
    }
    Ok(unsafe { out.assume_init() })
}

fn same_file(named: &libc::stat, opened: &Metadata) -> bool {
    named.st_mode & libc::S_IFMT == libc::S_IFREG
        && named.st_dev as u64 == opened.dev()
        && named.st_ino as u64 == opened.ino()
}

fn stamp(m: &Metadata) -> (u64, i64, i64, i64, i64) {
    (
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

/// Normalize only lexical dot components. Parent traversal is never repaired.
fn relative_parts(path: &Path) -> Result<PathBuf, Reason> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(value) => {
                name(value)?;
                out.push(value);
            }
            _ => return Err(Reason::InvalidPath),
        }
    }
    Ok(out)
}

fn absolute_directory(path: &Path, cancellation: &CancellationToken) -> Result<File, Reason> {
    if !path.is_absolute() {
        return Err(Reason::InvalidPath);
    }
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open("/")
        .map_err(|_| Reason::InvalidProject)?;
    for part in path.components() {
        check(cancellation)?;
        match part {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(value) => directory = open_at(&directory, value, true)?,
            _ => return Err(Reason::InvalidPath),
        }
    }
    Ok(directory)
}

impl Directory {
    pub fn pin(path: &Path, cancellation: &CancellationToken) -> Result<Self, Reason> {
        check(cancellation)?;
        if !path.is_absolute() || path.components().any(|p| matches!(p, Component::ParentDir)) {
            return Err(Reason::InvalidProject);
        }
        // Trusted explicit project aliases may resolve only at admission. Both
        // the logical spelling and the pinned physical root stay bound below.
        let identity = workspace_path_identity(path).map_err(|_| Reason::InvalidProject)?;
        let canonical = std::fs::canonicalize(path).map_err(|_| Reason::InvalidProject)?;
        let file = absolute_directory(&canonical, cancellation)?;
        if workspace_file_identity(&file).map_err(|_| Reason::InvalidProject)? != identity {
            return Err(Reason::SourceDrift);
        }
        let result = Self {
            file,
            logical: path.to_owned(),
            canonical,
            identity,
        };
        result.validate_root(cancellation)?;
        Ok(result)
    }

    pub fn validate_root(&self, cancellation: &CancellationToken) -> Result<(), Reason> {
        check(cancellation)?;
        if workspace_path_identity(&self.logical).map_err(|_| Reason::SourceDrift)? != self.identity
            || workspace_path_identity(&self.canonical).map_err(|_| Reason::SourceDrift)?
                != self.identity
            || workspace_file_identity(&self.file).map_err(|_| Reason::SourceDrift)?
                != self.identity
        {
            return Err(Reason::SourceDrift);
        }
        Ok(())
    }

    fn from_absolute(&self, path: &Path) -> Result<PathBuf, Reason> {
        if !path.is_absolute() || path.components().any(|p| matches!(p, Component::ParentDir)) {
            return Err(Reason::InvalidPath);
        }
        let relative = path
            .strip_prefix(&self.logical)
            .or_else(|_| path.strip_prefix(&self.canonical))
            .map_err(|_| Reason::InvalidPath)?;
        relative_parts(relative)
    }

    fn directory(&self, relative: &Path, cancellation: &CancellationToken) -> Result<File, Reason> {
        let mut directory = self
            .file
            .try_clone()
            .map_err(|_| Reason::UnreadableSource)?;
        for part in relative.components() {
            check(cancellation)?;
            let Component::Normal(value) = part else {
                return Err(Reason::InvalidPath);
            };
            directory = open_at(&directory, value, true)?;
        }
        Ok(directory)
    }

    pub fn cwd(&self, path: &Path, cancellation: &CancellationToken) -> Result<PathBuf, Reason> {
        let relative = self.from_absolute(path).map_err(|_| Reason::InvalidCwd)?;
        self.directory(&relative, cancellation).map_err(|reason| {
            if reason == Reason::Cancelled {
                reason
            } else {
                Reason::InvalidCwd
            }
        })?;
        self.validate_root(cancellation)?;
        Ok(relative)
    }

    pub fn source_path(&self, cwd: &Path, path: &Path) -> Result<PathBuf, Reason> {
        let relative = if path.is_absolute() {
            self.from_absolute(path)?
        } else {
            cwd.join(relative_parts(path)?)
        };
        if relative.as_os_str().is_empty() {
            return Err(Reason::InvalidPath);
        }
        Ok(relative)
    }

    fn parent(
        &self,
        relative: &Path,
        cancellation: &CancellationToken,
    ) -> Result<(File, OsString), Reason> {
        let relative = relative_parts(relative)?;
        let value = relative
            .file_name()
            .ok_or(Reason::InvalidPath)?
            .to_os_string();
        let directory = self.directory(relative.parent().unwrap_or(Path::new("")), cancellation)?;
        Ok((directory, value))
    }

    fn validate_binding(
        &self,
        relative: &Path,
        opened: &Metadata,
        cancellation: &CancellationToken,
    ) -> Result<(), Reason> {
        self.validate_root(cancellation)?;
        let (parent, value) = self.parent(relative, cancellation).map_err(|reason| {
            if reason == Reason::Cancelled {
                reason
            } else {
                Reason::SourceDrift
            }
        })?;
        if !same_file(
            &named_metadata(&parent, &value).map_err(|_| Reason::SourceDrift)?,
            opened,
        ) {
            return Err(Reason::SourceDrift);
        }
        Ok(())
    }

    pub fn read(
        &self,
        relative: &Path,
        expected: Option<&WorkspacePathIdentity>,
        maximum: usize,
        remaining: &mut usize,
        cancellation: &CancellationToken,
    ) -> Result<SourceRead, Reason> {
        self.validate_root(cancellation)?;
        let (parent, value) = self.parent(relative, cancellation)?;
        let observed = named_metadata(&parent, &value)?;
        if observed.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(Reason::UnreadableSource);
        }
        #[cfg(test)]
        super::source_context_test_stage(super::SourceContextTestStage::BeforeOpen { relative });
        check(cancellation)?;
        let mut file = open_at(&parent, &value, false)?;
        let before = file.metadata().map_err(|_| Reason::UnreadableSource)?;
        let identity = workspace_file_identity(&file).map_err(|_| Reason::UnreadableSource)?;
        if !before.is_file()
            || !same_file(&observed, &before)
            || expected.is_some_and(|id| *id != identity)
        {
            return Err(Reason::SourceDrift);
        }
        if before.len() > maximum as u64 {
            return Err(Reason::FileTooLarge);
        }
        if before.len() > *remaining as u64 {
            return Err(Reason::ReadBudgetExceeded);
        }
        self.validate_binding(relative, &before, cancellation)?;
        let mut bytes = Vec::with_capacity(before.len() as usize);
        let mut chunk = [0u8; 8192];
        while bytes.len() < before.len() as usize {
            check(cancellation)?;
            let count = (before.len() as usize - bytes.len())
                .min(chunk.len())
                .min(*remaining);
            if count == 0 {
                return Err(Reason::ReadBudgetExceeded);
            }
            #[cfg(test)]
            super::source_context_test_stage(super::SourceContextTestStage::BeforeRead {
                file: &file,
            });
            check(cancellation)?;
            let read = match file.read(&mut chunk[..count]) {
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Reason::UnreadableSource),
            };
            *remaining -= read;
            #[cfg(test)]
            super::source_context_test_stage(super::SourceContextTestStage::AfterRead {
                file: &file,
                bytes: read,
            });
            if read == 0 {
                return Err(Reason::SourceDrift);
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        let after = file.metadata().map_err(|_| Reason::SourceDrift)?;
        if stamp(&before) != stamp(&after)
            || workspace_file_identity(&file).map_err(|_| Reason::SourceDrift)? != identity
        {
            return Err(Reason::SourceDrift);
        }
        self.validate_binding(relative, &after, cancellation)?;
        Ok(SourceRead { bytes, identity })
    }
}
