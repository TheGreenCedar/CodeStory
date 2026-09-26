//! Stage a sealed generation file with a native clone or a cancellable copy.
//!
//! Callers retain the source generation's reader lease for this entire call.
//! Mutable legacy SQLite databases use the online backup API instead.

use crate::StorageError;
use fs_at::OpenOptions as AtOpenOptions;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const COPY_CHUNK_BYTES: usize = 1024 * 1024;
#[cfg(any(windows, test))]
const WINDOWS_MAX_CLONE_RANGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[cfg(any(windows, test))]
fn windows_clone_shape(length: u64, cluster: u64) -> Result<(u64, u64), StorageError> {
    if cluster == 0 {
        return Err(error("Windows clone volume returned a zero cluster size"));
    }
    let rounded = length
        .checked_add(cluster - 1)
        .ok_or_else(|| error("Windows clone rounded length overflowed"))?
        / cluster
        * cluster;
    let max_range = WINDOWS_MAX_CLONE_RANGE_BYTES / cluster * cluster;
    if max_range == 0 {
        return Err(error("Windows cluster exceeds the clone range"));
    }
    Ok((rounded, max_range))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedStageStrategy {
    Cloned,
    Copied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealedStageStats {
    pub strategy: SealedStageStrategy,
    pub fallback_reason: Option<&'static str>,
    pub native_error_code: Option<i32>,
    pub source_bytes: u64,
    pub cloned_bytes: u64,
    pub copied_bytes: u64,
    pub wall_ms: u64,
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static NATIVE_CLONE_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[cfg(test)]
    static PARENT_SYNC_FAULT: std::cell::RefCell<Option<File>> = const { std::cell::RefCell::new(None) };
    #[cfg(test)]
    static FILE_SYNC_FAULT: std::cell::RefCell<Option<File>> = const { std::cell::RefCell::new(None) };
    #[cfg(test)]
    static NATIVE_CLONE_ATTEMPTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn faulty_sync_handle(_source: &Path) -> File {
    #[cfg(unix)]
    {
        use std::os::fd::{FromRawFd, IntoRawFd};
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        // SAFETY: into_raw_fd transfers the socket's only owned descriptor.
        unsafe { File::from_raw_fd(socket.into_raw_fd()) }
    }
    #[cfg(windows)]
    {
        File::open(_source).expect("read-only handle rejects FlushFileBuffers")
    }
}

#[cfg(test)]
pub(crate) fn with_parent_sync_failure<T>(source: &Path, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<File>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PARENT_SYNC_FAULT.with(|fault| {
                fault.replace(self.0.take());
            });
        }
    }
    let restore =
        Restore(PARENT_SYNC_FAULT.with(|fault| fault.replace(Some(faulty_sync_handle(source)))));
    let result = action();
    drop(restore);
    result
}

#[cfg(test)]
fn with_file_sync_failure<T>(source: &Path, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<File>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FILE_SYNC_FAULT.with(|fault| {
                fault.replace(self.0.take());
            });
        }
    }
    let restore =
        Restore(FILE_SYNC_FAULT.with(|fault| fault.replace(Some(faulty_sync_handle(source)))));
    let result = action();
    drop(restore);
    result
}

#[cfg(any(test, feature = "test-support"))]
pub fn with_native_clone_disabled<T>(action: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            NATIVE_CLONE_DISABLED.set(self.0);
        }
    }
    NATIVE_CLONE_DISABLED.with(|disabled| {
        let restore = Restore(disabled.replace(true));
        let result = action();
        drop(restore);
        result
    })
}

fn error(message: impl Into<String>) -> StorageError {
    StorageError::Other(message.into())
}

fn io_error(operation: &str, path: &Path, cause: std::io::Error) -> StorageError {
    error(format!("{operation} {}: {cause}", path.display()))
}

fn cancelled_error() -> StorageError {
    StorageError::Cancelled
}

fn source_sidecar(source: &Path, extension: &str) -> PathBuf {
    let mut name = source.as_os_str().to_os_string();
    name.push(extension);
    PathBuf::from(name)
}

// Keep the newly-created file's handle and its parent directory pinned until
// staging succeeds. A path spelling alone does not authorize cleanup if the
// child is replaced while this operation is failing.
struct OwnedDestination {
    parent: File,
    name: PathBuf,
    witness: Option<File>,
}

impl OwnedDestination {
    fn new(path: &Path) -> Result<Self, StorageError> {
        let parent_path = path
            .parent()
            .ok_or_else(|| error("sealed stage has no parent"))?;
        let name = path
            .file_name()
            .ok_or_else(|| error("sealed stage has no file name"))?;
        #[cfg(windows)]
        let parent = {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(parent_path)
        };
        #[cfg(not(windows))]
        let parent = File::open(parent_path);
        Ok(Self {
            parent: parent
                .map_err(|cause| io_error("open sealed stage parent", parent_path, cause))?,
            name: PathBuf::from(name),
            witness: None,
        })
    }

    fn record(&mut self, file: &File) -> Result<(), StorageError> {
        self.witness = Some(file.try_clone().map_err(|cause| {
            let cleanup = remove_matching_regular_file(&self.parent, &self.name, file);
            error(format!(
                "retain sealed stage handle: {cause}; cleanup: {cleanup:?}"
            ))
        })?);
        Ok(())
    }

    fn remove_if_owned(&mut self, path: &Path) -> Result<(), StorageError> {
        let Some(witness) = self.witness.take() else {
            return Ok(());
        };
        let removed = remove_matching_regular_file(&self.parent, &self.name, &witness)
            .map_err(|cause| io_error("remove owned sealed stage", path, cause))?;
        if !removed {
            return Err(error(format!(
                "owned sealed stage identity changed before cleanup: {}",
                path.display()
            )));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn same_file(left: &File, right: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(windows)]
fn same_file(left: &File, right: &File) -> std::io::Result<bool> {
    use std::ffi::c_void;
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(PartialEq, Eq)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut c_void,
            class: i32,
            information: *mut c_void,
            size: u32,
        ) -> i32;
    }
    fn identity(file: &File) -> std::io::Result<FileIdInfo> {
        const FILE_ID_INFO_CLASS: i32 = 18;
        let mut info = MaybeUninit::<FileIdInfo>::uninit();
        // SAFETY: the handle remains live and the output is correctly sized.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle().cast(),
                FILE_ID_INFO_CLASS,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<FileIdInfo>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a successful call initializes every field.
        Ok(unsafe { info.assume_init() })
    }
    Ok(identity(left)? == identity(right)?)
}

#[cfg(unix)]
fn remove_matching_regular_file(
    parent: &File,
    name: &Path,
    witness: &File,
) -> std::io::Result<bool> {
    let mut options = AtOpenOptions::default();
    options.read(true).follow(false);
    let opened = match options.open_at(parent, name) {
        Ok(file) => file,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(cause) => return Err(cause),
    };
    if !opened.metadata()?.is_file() || !same_file(&opened, witness)? {
        return Ok(false);
    }
    // The caller owns the staging directory and excludes cooperating writers.
    options.unlink_at(parent, name)?;
    Ok(true)
}

#[cfg(windows)]
fn remove_matching_regular_file(
    parent: &File,
    name: &Path,
    witness: &File,
) -> std::io::Result<bool> {
    use fs_at::os::windows::{FileExt as _, OpenOptionsExt as _};
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_READ_ATTRIBUTES};
    let mut options = AtOpenOptions::default();
    options
        .desired_access(DELETE | FILE_READ_ATTRIBUTES)
        .follow(false);
    let opened = match options.open_at(parent, name) {
        Ok(file) => file,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(cause) => return Err(cause),
    };
    if !opened.metadata()?.is_file() || !same_file(&opened, witness)? {
        return Ok(false);
    }
    opened.delete_by_handle().map_err(|(_, cause)| cause)?;
    Ok(true)
}

/// Stage a read-only regular file. The destination must be absent. Any file
/// created by this call is removed on error; an existing destination is never
/// removed or overwritten.
pub fn stage_sealed_file(
    source: &Path,
    destination: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<SealedStageStats, StorageError> {
    stage_sealed_file_impl(source, destination, cancelled)
}

fn stage_sealed_file_impl(
    source: &Path,
    destination: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<SealedStageStats, StorageError> {
    let started = Instant::now();
    let metadata = fs::symlink_metadata(source)
        .map_err(|cause| io_error("inspect sealed source", source, cause))?;
    if !metadata.file_type().is_file() || !metadata.permissions().readonly() {
        return Err(error(format!(
            "sealed stage source must be a read-only regular file: {}",
            source.display()
        )));
    }
    for suffix in ["-wal", "-shm"] {
        let sidecar = source_sidecar(source, suffix);
        match fs::symlink_metadata(&sidecar) {
            Ok(_) => {
                return Err(error(format!(
                    "sealed stage source has SQLite sidecar: {}",
                    sidecar.display()
                )));
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(io_error("inspect sealed source sidecar", &sidecar, cause)),
        }
    }
    if cancelled() {
        return Err(cancelled_error());
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(error(format!(
                "sealed stage destination already exists: {}",
                destination.display()
            )));
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => {
            return Err(io_error(
                "inspect sealed stage destination",
                destination,
                cause,
            ));
        }
    }
    let source_bytes = metadata.len();
    crate::ensure_full_size_write_capacity(
        destination.parent().unwrap_or_else(|| Path::new(".")),
        source_bytes,
        "sealed_component_copy",
    )?;
    let mut owned = OwnedDestination::new(destination)?;
    let result = (|| {
        #[cfg(any(test, feature = "test-support"))]
        let native = if NATIVE_CLONE_DISABLED.get() {
            CloneResult::Unsupported("native_clone_disabled", None)
        } else {
            #[cfg(test)]
            NATIVE_CLONE_ATTEMPTS.set(NATIVE_CLONE_ATTEMPTS.get() + 1);
            native_clone(source, destination, source_bytes, &mut owned, cancelled)?
        };
        #[cfg(not(any(test, feature = "test-support")))]
        let native = native_clone(source, destination, source_bytes, &mut owned, cancelled)?;

        let (strategy, fallback_reason, native_error_code, cloned_bytes, copied_bytes) =
            match native {
                CloneResult::Cloned {
                    cloned_bytes,
                    copied_bytes,
                } => (
                    SealedStageStrategy::Cloned,
                    None,
                    None,
                    cloned_bytes,
                    copied_bytes,
                ),
                CloneResult::Unsupported(reason, native_error_code) => {
                    let mut input = File::open(source)
                        .map_err(|cause| io_error("open sealed source", source, cause))?;
                    let mut output = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(destination)
                        .map_err(|cause| {
                            io_error("create sealed stage destination", destination, cause)
                        })?;
                    owned.record(&output)?;
                    let mut buffer = [0_u8; COPY_CHUNK_BYTES];
                    let mut copied = 0_u64;
                    loop {
                        if cancelled() {
                            return Err(cancelled_error());
                        }
                        let read = input
                            .read(&mut buffer)
                            .map_err(|cause| io_error("read sealed source", source, cause))?;
                        if read == 0 {
                            break;
                        }
                        output
                            .write_all(&buffer[..read])
                            .map_err(|cause| io_error("copy sealed stage", destination, cause))?;
                        copied += read as u64;
                    }
                    if copied != source_bytes {
                        return Err(error(format!(
                            "sealed source changed during copy: {}",
                            source.display()
                        )));
                    }
                    (
                        SealedStageStrategy::Copied,
                        Some(reason),
                        native_error_code,
                        0,
                        copied,
                    )
                }
            };
        if cancelled() {
            return Err(cancelled_error());
        }
        // clonefile can carry the source's read-only mode to the destination.
        // The stage is writable until the caller seals and publishes it.
        crate::core_generation::make_file_owner_writable(destination)?;
        OpenOptions::new()
            .write(true)
            .open(destination)
            .and_then(|file| sync_stage_handle(&file, false))
            .map_err(|cause| io_error("sync sealed stage", destination, cause))?;
        sync_parent(destination)?;
        Ok(SealedStageStats {
            strategy,
            fallback_reason,
            native_error_code,
            source_bytes,
            cloned_bytes,
            copied_bytes,
            wall_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    })();
    match result {
        Ok(stats) => Ok(stats),
        Err(cause) => {
            owned.remove_if_owned(destination)?;
            Err(cause)
        }
    }
}

enum CloneResult {
    Cloned {
        cloned_bytes: u64,
        copied_bytes: u64,
    },
    Unsupported(&'static str, Option<i32>),
}

fn sync_stage_handle(file: &File, _parent: bool) -> std::io::Result<()> {
    #[cfg(test)]
    let injected = if _parent {
        PARENT_SYNC_FAULT.with(|fault| fault.borrow().as_ref().map(File::try_clone).transpose())?
    } else {
        FILE_SYNC_FAULT.with(|fault| fault.borrow().as_ref().map(File::try_clone).transpose())?
    };
    #[cfg(test)]
    let target = injected.as_ref().unwrap_or(file);
    #[cfg(not(test))]
    let target = file;
    target.sync_all()
}

#[cfg(unix)]
pub(crate) fn sync_parent(path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|directory| sync_stage_handle(&directory, true))
            .map_err(|cause| io_error("sync sealed stage parent", parent, cause))?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn sync_parent(path: &Path) -> Result<(), StorageError> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    if let Some(parent) = path.parent() {
        OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(parent)
            .and_then(|directory| sync_stage_handle(&directory, true))
            .map_err(|cause| io_error("sync sealed stage parent", parent, cause))?;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn sync_parent(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn native_clone(
    source: &Path,
    destination: &Path,
    length: u64,
    owned: &mut OwnedDestination,
    _cancelled: &dyn Fn() -> bool,
) -> Result<CloneResult, StorageError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source_c = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| error("sealed source contains NUL"))?;
    let destination_c = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| error("sealed destination contains NUL"))?;
    // SAFETY: both C strings remain live and clonefile retains neither pointer.
    let result = unsafe { libc::clonefile(source_c.as_ptr(), destination_c.as_ptr(), 0) };
    if result == 0 {
        let witness = File::open(destination)
            .map_err(|cause| io_error("open cloned sealed stage", destination, cause))?;
        owned.record(&witness)?;
        return Ok(CloneResult::Cloned {
            cloned_bytes: length,
            copied_bytes: 0,
        });
    }
    let cause = std::io::Error::last_os_error();
    match cause.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::EINVAL) => Ok(CloneResult::Unsupported(
            "clonefile_unsupported",
            cause.raw_os_error(),
        )),
        _ => Err(io_error("clone sealed source", destination, cause)),
    }
}

#[cfg(windows)]
fn native_clone(
    source: &Path,
    destination: &Path,
    length: u64,
    owned: &mut OwnedDestination,
    cancelled: &dyn Fn() -> bool,
) -> Result<CloneResult, StorageError> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;

    const FSCTL_DUPLICATE_EXTENTS_TO_FILE: u32 = 0x0009_8344;
    #[repr(C)]
    struct DuplicateExtentsData {
        file_handle: *mut c_void,
        source_file_offset: i64,
        target_file_offset: i64,
        byte_count: i64,
    }
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn DeviceIoControl(
            device: *mut c_void,
            control_code: u32,
            input: *mut c_void,
            input_size: u32,
            output: *mut c_void,
            output_size: u32,
            bytes_returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn GetVolumePathNameW(
            file_name: *const u16,
            volume_path_name: *mut u16,
            buffer_length: u32,
        ) -> i32;
        fn GetDiskFreeSpaceW(
            root_path_name: *const u16,
            sectors_per_cluster: *mut u32,
            bytes_per_sector: *mut u32,
            free_clusters: *mut u32,
            total_clusters: *mut u32,
        ) -> i32;
    }

    if length == 0 {
        return Ok(CloneResult::Unsupported("windows_empty_file", None));
    }
    let source_file =
        File::open(source).map_err(|cause| io_error("open sealed clone source", source, cause))?;
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut volume = vec![0_u16; 32_768];
    // SAFETY: the buffers are live and NUL terminated for both synchronous calls.
    let found = unsafe {
        GetVolumePathNameW(
            destination_wide.as_ptr(),
            volume.as_mut_ptr(),
            volume.len() as u32,
        )
    };
    if found == 0 {
        return Err(io_error(
            "resolve clone volume",
            destination,
            std::io::Error::last_os_error(),
        ));
    }
    let mut sectors = 0_u32;
    let mut bytes_per_sector = 0_u32;
    let mut free = 0_u32;
    let mut total = 0_u32;
    // SAFETY: the returned volume buffer is NUL terminated and outputs are live.
    let observed = unsafe {
        GetDiskFreeSpaceW(
            volume.as_ptr(),
            &mut sectors,
            &mut bytes_per_sector,
            &mut free,
            &mut total,
        )
    };
    if observed == 0 {
        return Err(io_error(
            "observe clone cluster size",
            destination,
            std::io::Error::last_os_error(),
        ));
    }
    let cluster = u64::from(sectors) * u64::from(bytes_per_sector);
    let (rounded_length, max_range) = windows_clone_shape(length, cluster)?;

    let destination_file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(destination)
        .map_err(|cause| io_error("create sealed clone destination", destination, cause))?;
    owned.record(&destination_file)?;
    destination_file
        .set_len(rounded_length)
        .map_err(|cause| io_error("size sealed clone destination", destination, cause))?;
    let mut offset = 0_u64;
    while offset < rounded_length {
        if cancelled() {
            return Err(cancelled_error());
        }
        let range = (rounded_length - offset).min(max_range);
        let mut request = DuplicateExtentsData {
            file_handle: source_file.as_raw_handle().cast(),
            source_file_offset: i64::try_from(offset)
                .map_err(|_| error("clone offset exceeds Windows range"))?,
            target_file_offset: i64::try_from(offset)
                .map_err(|_| error("clone offset exceeds Windows range"))?,
            byte_count: i64::try_from(range)
                .map_err(|_| error("clone range exceeds Windows range"))?,
        };
        let mut returned = 0_u32;
        // SAFETY: both handles and the request remain live for the synchronous call.
        let result = unsafe {
            DeviceIoControl(
                destination_file.as_raw_handle().cast(),
                FSCTL_DUPLICATE_EXTENTS_TO_FILE,
                (&mut request as *mut DuplicateExtentsData).cast(),
                std::mem::size_of::<DuplicateExtentsData>() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if result == 0 {
            let cause = std::io::Error::last_os_error();
            let code = cause.raw_os_error().unwrap_or_default();
            drop(destination_file);
            owned.remove_if_owned(destination)?;
            if matches!(code, 1 | 5 | 17 | 50 | 87) {
                return Ok(CloneResult::Unsupported(
                    "windows_block_clone_unsupported",
                    Some(code),
                ));
            }
            return Err(io_error("clone sealed source", destination, cause));
        }
        offset += range;
    }
    if cancelled() {
        return Err(cancelled_error());
    }
    destination_file
        .set_len(length)
        .map_err(|cause| io_error("truncate rounded clone destination", destination, cause))?;
    Ok(CloneResult::Cloned {
        cloned_bytes: length,
        copied_bytes: 0,
    })
}

#[cfg(target_os = "linux")]
fn native_clone(
    source: &Path,
    destination: &Path,
    length: u64,
    owned: &mut OwnedDestination,
    _cancelled: &dyn Fn() -> bool,
) -> Result<CloneResult, StorageError> {
    use std::os::fd::AsRawFd;
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let input =
        File::open(source).map_err(|cause| io_error("open sealed clone source", source, cause))?;
    let output = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|cause| io_error("create sealed clone destination", destination, cause))?;
    owned.record(&output)?;
    // SAFETY: both descriptors remain live and the kernel retains neither.
    let result = unsafe { libc::ioctl(output.as_raw_fd(), FICLONE, input.as_raw_fd()) };
    if result == 0 {
        return Ok(CloneResult::Cloned {
            cloned_bytes: length,
            copied_bytes: 0,
        });
    }
    let cause = std::io::Error::last_os_error();
    drop(output);
    owned.remove_if_owned(destination)?;
    match cause.raw_os_error() {
        Some(libc::EOPNOTSUPP | libc::EXDEV | libc::ENOTTY | libc::EINVAL) => Ok(
            CloneResult::Unsupported("ficlone_unsupported", cause.raw_os_error()),
        ),
        _ => Err(io_error("clone sealed source", destination, cause)),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn native_clone(
    _source: &Path,
    _destination: &Path,
    _length: u64,
    _owned: &mut OwnedDestination,
    _cancelled: &dyn Fn() -> bool,
) -> Result<CloneResult, StorageError> {
    Ok(CloneResult::Unsupported("native_clone_unavailable", None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const BOUNDED_STACK_COPY_CHILD_ROOT_ENV: &str = "CODESTORY_SEALED_COPY_STACK_CHILD_ROOT";

    fn sealed_source(root: &Path, bytes: &[u8]) -> PathBuf {
        let path = root.join("source.db");
        fs::write(&path, bytes).expect("source");
        crate::core_generation::make_file_immutable(&path).expect("seal source");
        path
    }

    #[test]
    fn production_copy_reports_exact_bytes_and_native_fallback() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), b"sealed source bytes");
        let destination = root.path().join("candidate.db");
        let stats = with_native_clone_disabled(|| {
            stage_sealed_file(&source, &destination, &|| false).expect("copy stage")
        });
        assert_eq!(stats.strategy, SealedStageStrategy::Copied);
        assert_eq!(stats.fallback_reason, Some("native_clone_disabled"));
        assert_eq!(stats.native_error_code, None);
        assert_eq!(stats.cloned_bytes, 0);
        assert_eq!(stats.copied_bytes, stats.source_bytes);
        assert_eq!(
            fs::read(destination).expect("candidate"),
            fs::read(source).expect("source")
        );
    }

    #[test]
    fn bounded_stack_copy_child() {
        let Some(root) = std::env::var_os(BOUNDED_STACK_COPY_CHILD_ROOT_ENV) else {
            return;
        };
        let root = PathBuf::from(root);
        let source = root.join("source.db");
        let destination = root.join("candidate.db");
        let stats = std::thread::Builder::new()
            .name("sealed-copy-bounded-stack".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                with_native_clone_disabled(|| {
                    stage_sealed_file(&source, &destination, &|| false)
                        .expect("copy sealed file on bounded worker stack")
                })
            })
            .expect("start bounded-stack copy worker")
            .join()
            .expect("bounded-stack copy worker completed");
        assert_eq!(stats.strategy, SealedStageStrategy::Copied);
        assert_eq!(stats.fallback_reason, Some("native_clone_disabled"));
        assert!(stats.copied_bytes > COPY_CHUNK_BYTES as u64);
        assert_eq!(stats.copied_bytes, stats.source_bytes);
    }

    #[test]
    fn production_copy_fits_a_bounded_worker_stack() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), &vec![7_u8; COPY_CHUNK_BYTES * 2 + 17]);
        let destination = root.path().join("candidate.db");
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "sealed_file_stage::tests::bounded_stack_copy_child",
                "--nocapture",
            ])
            .env(BOUNDED_STACK_COPY_CHILD_ROOT_ENV, root.path())
            .current_dir(root.path())
            .output()
            .expect("run bounded-stack copy child");
        assert!(
            output.status.success(),
            "bounded-stack copy child failed: {:?}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            fs::read(&destination).expect("copied candidate")
                == fs::read(&source).expect("sealed source"),
            "bounded-stack copy preserves every source byte"
        );
    }

    #[test]
    fn insufficient_space_refuses_copy_before_creating_destination() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), b"published sealed bytes");
        let destination = root.path().join("candidate.db");
        let error = with_native_clone_disabled(|| {
            crate::with_available_filesystem_bytes_override(0, || {
                stage_sealed_file(&source, &destination, &|| false)
                    .expect_err("copy must refuse insufficient space")
            })
        });
        assert!(matches!(
            error,
            StorageError::InsufficientSpace {
                operation: "sealed_component_copy",
                available_bytes: 0,
                ..
            }
        ));
        assert!(!destination.exists());
        assert_eq!(
            fs::read(source).expect("published source"),
            b"published sealed bytes"
        );
    }

    #[test]
    fn insufficient_space_refuses_before_native_clone_attempt() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), b"published sealed bytes");
        let destination = root.path().join("candidate.db");
        let attempts_before = NATIVE_CLONE_ATTEMPTS.get();
        let error = crate::with_available_filesystem_bytes_override(0, || {
            stage_sealed_file(&source, &destination, &|| false)
                .expect_err("low space must refuse before native clone")
        });
        assert!(matches!(error, StorageError::InsufficientSpace { .. }));
        assert_eq!(NATIVE_CLONE_ATTEMPTS.get(), attempts_before);
        assert!(!destination.exists());
        assert_eq!(
            fs::read(source).expect("published source"),
            b"published sealed bytes"
        );
    }

    #[test]
    fn cancelled_copy_removes_only_its_owned_destination() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), &vec![7_u8; COPY_CHUNK_BYTES * 3]);
        let destination = root.path().join("candidate.db");
        let calls = AtomicUsize::new(0);
        let error = with_native_clone_disabled(|| {
            stage_sealed_file(&source, &destination, &|| {
                calls.fetch_add(1, Ordering::SeqCst) >= 2
            })
            .expect_err("cancel between chunks")
        });
        assert!(matches!(error, StorageError::Cancelled));
        assert!(!destination.exists());
        assert_eq!(
            fs::metadata(source).expect("source remains").len(),
            (COPY_CHUNK_BYTES * 3) as u64
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_preserves_a_replacement_destination() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), &vec![7_u8; COPY_CHUNK_BYTES * 2]);
        let destination = root.path().join("candidate.db");
        let moved = root.path().join("moved-owned.db");
        let calls = AtomicUsize::new(0);
        let error = with_native_clone_disabled(|| {
            stage_sealed_file(&source, &destination, &|| {
                if calls.fetch_add(1, Ordering::SeqCst) == 2 {
                    fs::rename(&destination, &moved).expect("move owned stage");
                    fs::write(&destination, b"another owner's file").expect("replacement");
                    return true;
                }
                false
            })
            .expect_err("cancel after replacement")
        });
        assert!(error.to_string().contains("identity changed"));
        assert_eq!(
            fs::read(destination).expect("replacement"),
            b"another owner's file"
        );
        assert!(moved.exists());
    }

    #[test]
    fn existing_destination_and_unsealed_sources_are_rejected_without_mutation() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), b"sealed");
        let destination = root.path().join("candidate.db");
        fs::write(&destination, b"other owner").expect("sentinel");
        assert!(stage_sealed_file(&source, &destination, &|| false).is_err());
        assert_eq!(fs::read(&destination).expect("sentinel"), b"other owner");
        fs::remove_file(&destination).expect("remove sentinel");
        let wal = source_sidecar(&source, "-wal");
        fs::write(&wal, b"pending").expect("sidecar");
        assert!(stage_sealed_file(&source, &destination, &|| false).is_err());
        assert!(!destination.exists());
        fs::remove_file(wal).expect("remove sidecar");
        crate::core_generation::make_file_owner_writable(&source).expect("unseal source");
        assert!(stage_sealed_file(&source, &destination, &|| false).is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn failed_parent_sync_removes_owned_stage() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), b"sealed");
        let destination = root.path().join("candidate.db");
        let error = with_parent_sync_failure(&source, || {
            with_native_clone_disabled(|| {
                stage_sealed_file(&source, &destination, &|| false).expect_err("sync failure")
            })
        });
        assert!(error.to_string().contains("sync sealed stage parent"));
        assert!(!destination.exists());
        assert_eq!(fs::read(source).expect("source"), b"sealed");
    }

    #[test]
    fn failed_file_sync_removes_owned_stage() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = sealed_source(root.path(), b"sealed");
        let destination = root.path().join("candidate.db");
        let error = with_file_sync_failure(&source, || {
            with_native_clone_disabled(|| {
                stage_sealed_file(&source, &destination, &|| false).expect_err("sync failure")
            })
        });
        assert!(error.to_string().contains("sync sealed stage "));
        assert!(!destination.exists());
        assert_eq!(fs::read(source).expect("source"), b"sealed");
    }

    #[test]
    fn windows_clone_shape_covers_tiny_odd_and_large_files() {
        assert_eq!(
            windows_clone_shape(0, 4096).expect("empty"),
            (0, WINDOWS_MAX_CLONE_RANGE_BYTES)
        );
        assert_eq!(windows_clone_shape(1, 4096).expect("tiny").0, 4096);
        assert_eq!(windows_clone_shape(4097, 4096).expect("odd").0, 8192);
        assert_eq!(
            windows_clone_shape(1, 65536).expect("large cluster").0,
            65536
        );
        let (rounded, max_range) =
            windows_clone_shape(4 * 1024 * 1024 * 1024 + 1, 4096).expect("large");
        assert_eq!(rounded, 4 * 1024 * 1024 * 1024 + 4096);
        assert_eq!(max_range, 2 * 1024 * 1024 * 1024);
        assert_eq!((rounded + max_range - 1) / max_range, 3);
        assert!(windows_clone_shape(u64::MAX, 4096).is_err());
    }
}
