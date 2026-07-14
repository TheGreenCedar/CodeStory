//! Handle-relative deletion for resources below an already trusted directory.

use fs_at::{OpenOptions as AtOpenOptions, OpenOptionsWriteMode};
use remove_dir_all::RemoveDir as _;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io,
    path::{Component, Path},
};

/// A deletion boundary pinned to one open directory handle.
///
/// Callers establish ownership of `root` before opening this boundary, then
/// pass only owned relative names. Traversal and removal stay relative to the
/// open handle even if the ambient pathname is renamed or replaced.
#[derive(Debug)]
pub struct OwnedDeletionRoot {
    root: File,
}

impl OwnedDeletionRoot {
    /// Pin a directory that the caller has already established as trusted.
    pub fn open(root: &Path) -> io::Result<Self> {
        open_root(root).map(|root| Self { root })
    }

    /// Remove one owned file or directory below this boundary.
    ///
    /// Missing entries are already removed and return `false`. Absolute paths,
    /// parent traversal, and an empty/root path are always rejected.
    pub fn remove(&self, relative: &Path) -> io::Result<bool> {
        let parts = relative_owned_parts(relative)?;
        let (leaf, ancestors) = parts
            .split_last()
            .expect("relative_owned_parts rejects an empty path");
        let mut parent = self.root.try_clone()?;
        for ancestor in ancestors {
            parent = open_child_dir(&parent, ancestor)?;
        }

        match open_child_dir(&parent, leaf) {
            Ok(mut target) => {
                target.remove_dir_contents(Some(relative))?;
                remove_open_directory(&parent, leaf, target)?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(_) => remove_file_entry(&parent, leaf),
        }
    }
}

fn relative_owned_parts(path: &Path) -> io::Result<Vec<OsString>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_owned()),
            _ => return Err(invalid_relative_path(path)),
        }
    }
    if parts.is_empty() {
        return Err(invalid_relative_path(path));
    }
    Ok(parts)
}

fn invalid_relative_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "owned deletion requires a non-empty relative path without traversal: {}",
            path.display()
        ),
    )
}

fn open_child_dir(parent: &File, name: &OsStr) -> io::Result<File> {
    let mut options = AtOpenOptions::default();
    options
        .read(true)
        .write(OpenOptionsWriteMode::Write)
        .follow(false);
    let child = options.open_dir_at(parent, Path::new(name))?;
    let metadata = child.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("owned deletion target is not a directory"));
    }
    reject_windows_reparse(&metadata)?;
    Ok(child)
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
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let root = options.open(path)?;
    let metadata = root.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("owned deletion root is not a directory"));
    }
    reject_windows_reparse(&metadata)?;
    Ok(root)
}

#[cfg(unix)]
fn reject_windows_reparse(_: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn reject_windows_reparse(metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(io::Error::other("owned deletion refuses reparse points"))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn remove_open_directory(parent: &File, leaf: &OsStr, target: File) -> io::Result<()> {
    drop(target);
    AtOpenOptions::default().rmdir_at(parent, Path::new(leaf))
}

#[cfg(windows)]
fn remove_open_directory(_: &File, _: &OsStr, target: File) -> io::Result<()> {
    use fs_at::os::windows::FileExt as _;

    target.delete_by_handle().map_err(|(_, error)| error)
}

#[cfg(unix)]
fn remove_file_entry(parent: &File, leaf: &OsStr) -> io::Result<bool> {
    match AtOpenOptions::default().unlink_at(parent, Path::new(leaf)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn remove_file_entry(parent: &File, leaf: &OsStr) -> io::Result<bool> {
    use fs_at::os::windows::{FileExt as _, OpenOptionsExt as _};
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_READ_ATTRIBUTES};

    let mut options = AtOpenOptions::default();
    options
        .desired_access(DELETE | FILE_READ_ATTRIBUTES)
        .follow(false);
    let target = match options.open_at(parent, Path::new(leaf)) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = target.metadata()?;
    reject_windows_reparse(&metadata)?;
    if metadata.is_dir() {
        return Err(io::Error::other(
            "owned deletion target directory could not be opened safely",
        ));
    }
    target
        .delete_by_handle()
        .map(|()| true)
        .map_err(|(_, error)| error)
}

#[cfg(test)]
mod tests {
    use super::OwnedDeletionRoot;
    use std::fs;
    #[cfg(windows)]
    use std::process::Command;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn unix_root_swap_cannot_redirect_deletion() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("create temp root");
        let owned = temp.path().join("owned");
        let pinned = temp.path().join("pinned-owned");
        let outside = temp.path().join("outside");
        fs::create_dir_all(owned.join("generation")).expect("create owned generation");
        fs::create_dir_all(&outside).expect("create outside directory");
        let sentinel = outside.join("sentinel");
        fs::write(&sentinel, b"outside").expect("write outside sentinel");

        let deletion = OwnedDeletionRoot::open(&owned).expect("pin owned root");
        fs::rename(&owned, &pinned).expect("move ambient owned root");
        symlink(&outside, &owned).expect("replace ambient root with outside symlink");

        assert!(
            deletion
                .remove("generation".as_ref())
                .expect("remove owned generation")
        );
        assert!(
            sentinel.is_file(),
            "pinned deletion must not follow the replacement root"
        );
        assert!(!pinned.join("generation").exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_root_swap_cannot_redirect_deletion() {
        let temp = tempdir().expect("create temp root");
        let owned = temp.path().join("owned");
        let pinned = temp.path().join("pinned-owned");
        let outside = temp.path().join("outside");
        fs::create_dir_all(owned.join("generation")).expect("create owned generation");
        fs::create_dir_all(&outside).expect("create outside directory");
        let sentinel = outside.join("sentinel");
        fs::write(&sentinel, b"outside").expect("write outside sentinel");

        let deletion = OwnedDeletionRoot::open(&owned).expect("pin owned root");
        fs::rename(&owned, &pinned).expect("move ambient owned root");
        let junction = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&owned)
            .arg(&outside)
            .status()
            .expect("create replacement junction");
        assert!(junction.success(), "create replacement junction");

        assert!(
            deletion
                .remove("generation".as_ref())
                .expect("remove owned generation")
        );
        assert!(
            sentinel.is_file(),
            "pinned deletion must not follow the replacement root"
        );
        assert!(!pinned.join("generation").exists());
    }
}
