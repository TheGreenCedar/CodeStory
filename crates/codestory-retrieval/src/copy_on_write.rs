use anyhow::{Context, Result, bail};
use std::path::Path;

/// Clone one immutable component into a distinct candidate file without
/// copying its unchanged physical extents.
///
/// `Ok(false)` means the current filesystem/OS cannot provide a copy-on-write
/// clone. Callers must fall back to their complete staged builder; a byte copy
/// would preserve correctness but recreate the full-work defect this boundary
/// exists to avoid.
pub(crate) fn clone_file(source: &Path, destination: &Path) -> Result<bool> {
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
}
