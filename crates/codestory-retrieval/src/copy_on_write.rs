use anyhow::{Context, Result, bail};
use std::path::Path;

#[cfg(test)]
thread_local! {
    static CLONE_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PUBLICATION_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn with_clone_disabled<T>(action: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            CLONE_DISABLED.set(self.0);
        }
    }

    CLONE_DISABLED.with(|disabled| {
        let restore = Restore(disabled.replace(true));
        let result = action();
        drop(restore);
        result
    })
}

#[cfg(test)]
fn with_publication_disabled<T>(action: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PUBLICATION_DISABLED.set(self.0);
        }
    }

    PUBLICATION_DISABLED.with(|disabled| {
        let restore = Restore(disabled.replace(true));
        let result = action();
        drop(restore);
        result
    })
}

/// Clone one immutable component into a distinct candidate file without
/// copying its unchanged physical extents.
///
/// `Ok(false)` means the current filesystem/OS cannot provide a copy-on-write
/// clone. Callers must fall back to their complete staged builder; a byte copy
/// would preserve correctness but recreate the full-work defect this boundary
/// exists to avoid.
pub(crate) fn clone_file(source: &Path, destination: &Path) -> Result<bool> {
    #[cfg(test)]
    if CLONE_DISABLED.get() {
        return Ok(false);
    }
    let source_metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("inspect clone source {}", source.display()))?;
    if !source_metadata.file_type().is_file() {
        bail!("copy-on-write clone source is not a regular file");
    }
    if std::fs::symlink_metadata(destination).is_ok() {
        bail!("copy-on-write clone destination already exists");
    }

    clone_file_platform(source, destination)
}

/// Give another immutable generation a direct filesystem reference to the
/// exact same component bytes.
///
/// A hard link is safe here because published components are immutable. It is
/// also independent of reflink support: publication-only churn does not copy
/// the file or open it for mutation. `Ok(false)` is a normal cross-device or
/// unsupported-filesystem result; callers retain their COW/full fallback.
pub(crate) fn reference_file(source: &Path, destination: &Path) -> Result<bool> {
    let source_metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("inspect component reference source {}", source.display()))?;
    if !source_metadata.file_type().is_file() {
        bail!("component reference source is not a regular file");
    }
    if !source_metadata.permissions().readonly() {
        bail!("component reference source is not immutable");
    }
    if std::fs::symlink_metadata(destination).is_ok() {
        bail!("component reference destination already exists");
    }
    match std::fs::hard_link(source, destination) {
        Ok(()) => {
            if !codestory_workspace::same_workspace_path(source, destination) {
                let _ = std::fs::remove_file(destination);
                bail!("component reference did not preserve native file identity");
            }
            Ok(true)
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied
            ) || error.raw_os_error().is_some_and(|code| {
                matches!(
                    code,
                    libc::EXDEV | libc::EPERM | libc::EOPNOTSUPP | libc::EINVAL
                )
            }) =>
        {
            Ok(false)
        }
        Err(error) => Err(error).context("reference immutable component with a hard link"),
    }
}

pub(crate) fn make_file_immutable(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect component permissions {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("immutable component is not a regular file");
    }
    let permissions = immutable_permissions(metadata.permissions());
    if permissions.readonly() {
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("make component immutable {}", path.display()))?;
    } else {
        bail!("component immutable permissions remained writable");
    }
    if !std::fs::symlink_metadata(path)?.permissions().readonly() {
        bail!("component remained owner-writable after immutable publication");
    }
    Ok(())
}

pub(crate) fn make_file_owner_writable(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect staged component permissions {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("staged component is not a regular file");
    }
    let permissions = owner_writable_permissions(metadata.permissions());
    std::fs::set_permissions(path, permissions)
        .with_context(|| format!("make staged component owner-writable {}", path.display()))
}

pub(crate) fn publish_immutable_file_atomic(temp_path: &Path, destination: &Path) -> Result<()> {
    make_file_immutable(temp_path)?;
    let previous = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                bail!("immutable component replacement destination is not a regular file");
            }
            let handle = open_permission_handle(destination)?;
            handle
                .set_permissions(owner_writable_permissions(metadata.permissions()))
                .with_context(|| {
                    format!(
                        "temporarily permit immutable component replacement {}",
                        destination.display()
                    )
                })?;
            Some(handle)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("inspect immutable component destination"),
    };

    #[cfg(test)]
    let publication = if PUBLICATION_DISABLED.get() {
        Err(anyhow::anyhow!(
            "simulated immutable component publication failure"
        ))
    } else {
        codestory_workspace::atomic_file::publish_existing_file_atomic(temp_path, destination)
    };
    #[cfg(not(test))]
    let publication =
        codestory_workspace::atomic_file::publish_existing_file_atomic(temp_path, destination);
    let previous_restoration = previous.map_or(Ok(()), |handle| {
        let permissions = handle.metadata()?.permissions();
        handle
            .set_permissions(immutable_permissions(permissions))
            .context("restore replaced immutable component permissions")
    });

    match publication {
        Ok(()) => {
            previous_restoration?;
            make_file_immutable(destination)
        }
        Err(error) => {
            if let Err(restoration) = previous_restoration {
                return Err(restoration).context(format!(
                    "restore immutable destination after failed publication: {error:#}"
                ));
            }
            let _ = make_file_immutable(destination);
            let _ = make_file_owner_writable(temp_path);
            Err(error)
        }
    }
}

fn immutable_permissions(mut permissions: std::fs::Permissions) -> std::fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() & !0o222);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(true);
    permissions
}

fn owner_writable_permissions(mut permissions: std::fs::Permissions) -> std::fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    permissions
}

#[cfg(windows)]
fn open_permission_handle(path: &Path) -> Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
    std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
        .with_context(|| format!("open immutable component permissions {}", path.display()))
}

#[cfg(not(windows))]
fn open_permission_handle(path: &Path) -> Result<std::fs::File> {
    std::fs::File::open(path)
        .with_context(|| format!("open immutable component permissions {}", path.display()))
}

#[cfg(target_os = "macos")]
fn clone_file_platform(source: &Path, destination: &Path) -> Result<bool> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())
        .context("copy-on-write clone source contains NUL")?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .context("copy-on-write clone destination contains NUL")?;
    // SAFETY: both pointers come from live NUL-terminated C strings and the
    // destination was proven absent above. `clonefile` creates a distinct file
    // whose unchanged extents are shared copy-on-write by APFS.
    let result = unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), 0) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    let _ = std::fs::remove_file(Path::new(std::ffi::OsStr::from_bytes(
        destination.as_bytes(),
    )));
    match error.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::EINVAL) => Ok(false),
        _ => Err(error).context("clone immutable component with clonefile"),
    }
}

#[cfg(target_os = "linux")]
fn clone_file_platform(source: &Path, destination: &Path) -> Result<bool> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    // Linux's FICLONE ioctl is stable across the filesystems that implement
    // reflinks. It either clones the complete file or leaves a candidate that
    // we remove before reporting unsupported/failure.
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let source = std::fs::File::open(source).context("open copy-on-write clone source")?;
    let destination_file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(destination)
        .context("create copy-on-write clone destination")?;
    // SAFETY: both descriptors are valid regular files for the duration of the
    // call and FICLONE does not retain their addresses.
    let result = unsafe { libc::ioctl(destination_file.as_raw_fd(), FICLONE, source.as_raw_fd()) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    drop(destination_file);
    let _ = std::fs::remove_file(destination);
    match error.raw_os_error() {
        Some(libc::EOPNOTSUPP | libc::EXDEV | libc::ENOTTY | libc::EINVAL) => Ok(false),
        _ => Err(error).context("clone immutable component with FICLONE"),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_file_platform(_source: &Path, _destination: &Path) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_is_distinct_and_copy_on_write_when_supported() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        std::fs::write(&source, b"immutable-source").expect("source");

        if !clone_file(&source, &destination).expect("clone") {
            return;
        }
        std::fs::write(&destination, b"candidate").expect("mutate candidate");

        assert_eq!(std::fs::read(&source).unwrap(), b"immutable-source");
        assert_eq!(std::fs::read(&destination).unwrap(), b"candidate");
    }

    #[test]
    fn clone_refuses_an_existing_destination() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        std::fs::write(&source, b"source").expect("source");
        std::fs::write(&destination, b"existing").expect("destination");

        let error = clone_file(&source, &destination).expect_err("must refuse overwrite");

        assert!(error.to_string().contains("destination already exists"));
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
    }

    #[test]
    fn reference_reuses_the_exact_immutable_file_without_clone_support() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        std::fs::write(&source, b"immutable-source").expect("source");
        make_file_immutable(&source).expect("immutable source");

        assert!(with_clone_disabled(|| reference_file(&source, &destination)).expect("reference"));
        assert_eq!(std::fs::read(&destination).unwrap(), b"immutable-source");
        assert!(codestory_workspace::same_workspace_path(
            &source,
            &destination
        ));
        assert!(std::fs::metadata(&source).unwrap().permissions().readonly());
        assert!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .readonly()
        );
    }

    #[test]
    fn immutable_publication_replaces_a_readonly_hard_link_without_mutating_its_predecessor() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let predecessor = root.path().join("predecessor");
        let current = root.path().join("current");
        let staged = root.path().join("staged");
        std::fs::write(&predecessor, b"old").expect("predecessor");
        make_file_immutable(&predecessor).expect("immutable predecessor");
        assert!(reference_file(&predecessor, &current).expect("current hard link"));
        std::fs::write(&staged, b"new").expect("staged");

        publish_immutable_file_atomic(&staged, &current).expect("replace readonly current");

        assert_eq!(std::fs::read(&predecessor).unwrap(), b"old");
        assert_eq!(std::fs::read(&current).unwrap(), b"new");
        assert!(!codestory_workspace::same_workspace_path(
            &predecessor,
            &current
        ));
        assert!(
            std::fs::metadata(&predecessor)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert!(
            std::fs::metadata(&current)
                .unwrap()
                .permissions()
                .readonly()
        );
    }

    #[test]
    fn failed_immutable_publication_restores_the_hard_linked_predecessor() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let predecessor = root.path().join("predecessor");
        let current = root.path().join("current");
        let staged = root.path().join("staged");
        std::fs::write(&predecessor, b"old").expect("predecessor");
        make_file_immutable(&predecessor).expect("immutable predecessor");
        assert!(reference_file(&predecessor, &current).expect("current hard link"));
        std::fs::write(&staged, b"new").expect("staged");

        let error = with_publication_disabled(|| publish_immutable_file_atomic(&staged, &current))
            .expect_err("injected publication must fail");

        assert!(error.to_string().contains("simulated immutable component"));
        assert_eq!(std::fs::read(&predecessor).unwrap(), b"old");
        assert_eq!(std::fs::read(&current).unwrap(), b"old");
        assert!(codestory_workspace::same_workspace_path(
            &predecessor,
            &current
        ));
        assert!(
            std::fs::metadata(&predecessor)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert!(
            std::fs::metadata(&current)
                .unwrap()
                .permissions()
                .readonly()
        );
    }
}
