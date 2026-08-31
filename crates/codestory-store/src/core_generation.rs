//! Immutable core-generation layout and publication pointer.

use crate::StorageError;
use codestory_contracts::core_publication::{
    CORE_PUBLICATION_POINTER_SCHEMA_VERSION, CoreGenerationIdentityV1, CorePublicationPointerV1,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CORE_DIRECTORY: &str = "core";
pub const CORE_GENERATIONS_DIRECTORY: &str = "generations";
pub const CORE_STAGING_DIRECTORY: &str = "staging";
pub const CORE_DATABASE_FILE: &str = "codestory.db";
pub const CORE_PUBLICATION_FILE: &str = "publication.json";
pub const RETRIEVAL_PUBLICATION_FILE: &str = "retrieval-publication.sqlite3";

/// Prefix for StorageError messages when block cloning cannot stage a core image.
///
/// Callers must escalate to a disposable complete-build rather than silently
/// byte-copying the live database in production.
pub const CORE_COPY_ON_WRITE_UNAVAILABLE: &str = "core_copy_on_write_unavailable";

#[cfg(test)]
pub(crate) const CORE_PUBLICATION_ABORT_POINT_ENV: &str =
    "CODESTORY_TEST_CORE_PUBLICATION_ABORT_POINT";
#[cfg(test)]
pub(crate) const CORE_PUBLICATION_ABORT_SENTINEL_ENV: &str =
    "CODESTORY_TEST_CORE_PUBLICATION_ABORT_SENTINEL";

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static CORE_CLONE_DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Force core CoW clones to report unavailable for the duration of `action`.
///
/// Production stays fail-closed without CoW. Tests normally get a test-only
/// `fs::copy` fallback on non-reflink filesystems; this helper disables that
/// fallback so callers can prove the complete-build escalate path.
#[cfg(any(test, feature = "test-support"))]
pub fn with_core_clone_disabled<T>(action: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            CORE_CLONE_DISABLED.set(self.0);
        }
    }

    CORE_CLONE_DISABLED.with(|disabled| {
        let restore = Restore(disabled.replace(true));
        let result = action();
        drop(restore);
        result
    })
}

#[cfg(any(test, feature = "test-support"))]
fn core_clone_disabled() -> bool {
    CORE_CLONE_DISABLED.get()
}

/// True when incremental staging failed because the filesystem cannot CoW-clone.
pub fn is_core_copy_on_write_unavailable(error: &StorageError) -> bool {
    match error {
        StorageError::Other(message) => message.starts_with(CORE_COPY_ON_WRITE_UNAVAILABLE),
        _ => false,
    }
}

const MAX_POINTER_BYTES: u64 = 16 * 1024;
const MAX_GENERATION_ID_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorePublicationLayout {
    legacy_storage_path: PathBuf,
    root: PathBuf,
}

impl CorePublicationLayout {
    pub fn from_storage_path(storage_path: &Path) -> Result<Self, StorageError> {
        let parent = storage_path.parent().ok_or_else(|| {
            core_publication_error(format!(
                "Core storage path has no parent: {}",
                storage_path.display()
            ))
        })?;
        Ok(Self {
            legacy_storage_path: storage_path.to_path_buf(),
            root: parent.join(CORE_DIRECTORY),
        })
    }

    pub fn legacy_storage_path(&self) -> &Path {
        &self.legacy_storage_path
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn publication_path(&self) -> PathBuf {
        self.root.join(CORE_PUBLICATION_FILE)
    }

    pub fn retrieval_publication_path(&self) -> PathBuf {
        self.root.join(RETRIEVAL_PUBLICATION_FILE)
    }

    pub fn generations_root(&self) -> PathBuf {
        self.root.join(CORE_GENERATIONS_DIRECTORY)
    }

    pub fn staging_root(&self) -> PathBuf {
        self.root.join(CORE_STAGING_DIRECTORY)
    }

    pub fn generation_directory(&self, generation_id: &str) -> Result<PathBuf, StorageError> {
        validate_generation_id(generation_id)?;
        Ok(self.generations_root().join(generation_id))
    }

    pub fn generation_database_path(&self, generation_id: &str) -> Result<PathBuf, StorageError> {
        Ok(self
            .generation_directory(generation_id)?
            .join(CORE_DATABASE_FILE))
    }

    pub fn create_staging_database_path(&self) -> Result<PathBuf, StorageError> {
        fs::create_dir_all(self.staging_root())
            .map_err(|error| core_path_error("create staging root", &self.staging_root(), error))?;
        let directory = self.staging_root().join(format!(
            "stage-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&directory)
            .map_err(|error| core_path_error("create stage", &directory, error))?;
        Ok(directory.join(CORE_DATABASE_FILE))
    }

    pub fn read_pointer(&self) -> Result<Option<CorePublicationPointerV1>, StorageError> {
        let path = self.publication_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(core_path_error("inspect pointer", &path, error)),
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_POINTER_BYTES {
            return Err(core_publication_error(format!(
                "Core publication pointer is not a bounded regular file: {}",
                path.display()
            )));
        }
        let bytes =
            fs::read(&path).map_err(|error| core_path_error("read pointer", &path, error))?;
        let pointer: CorePublicationPointerV1 =
            serde_json::from_slice(&bytes).map_err(|error| {
                core_publication_error(format!("Invalid core publication pointer: {error}"))
            })?;
        validate_pointer(&pointer)?;
        Ok(Some(pointer))
    }

    pub fn resolve_active_database(&self) -> Result<Option<PathBuf>, StorageError> {
        let Some(pointer) = self.read_pointer()? else {
            return Ok(self
                .legacy_storage_path
                .is_file()
                .then(|| self.legacy_storage_path.clone()));
        };
        let path = self.generation_database_path(&pointer.active.generation_id)?;
        require_regular_generation_file(&path)?;
        Ok(Some(path))
    }

    pub fn resolve_generation_database(
        &self,
        generation_id: &str,
    ) -> Result<PathBuf, StorageError> {
        let path = self.generation_database_path(generation_id)?;
        require_regular_generation_file(&path)?;
        Ok(path)
    }

    pub fn publish_pointer(
        &self,
        active: CoreGenerationIdentityV1,
        rollback: Option<CoreGenerationIdentityV1>,
    ) -> Result<CorePublicationPointerV1, StorageError> {
        validate_generation_identity(&active)?;
        if let Some(rollback) = rollback.as_ref() {
            validate_generation_identity(rollback)?;
            if rollback.generation_id == active.generation_id {
                return Err(core_publication_error(
                    "Core active and rollback generations must be distinct".into(),
                ));
            }
        }
        require_regular_generation_file(&self.generation_database_path(&active.generation_id)?)?;
        if let Some(rollback) = rollback.as_ref() {
            require_regular_generation_file(
                &self.generation_database_path(&rollback.generation_id)?,
            )?;
        }
        let mut pointer = CorePublicationPointerV1 {
            schema_version: CORE_PUBLICATION_POINTER_SCHEMA_VERSION,
            active,
            rollback,
            receipt_digest: String::new(),
        };
        pointer.receipt_digest = pointer_receipt_digest(&pointer)?;
        write_pointer_atomic(&self.publication_path(), &pointer)?;
        Ok(pointer)
    }

    pub(crate) fn install_staging_generation(
        &self,
        staged_database: &Path,
        generation_id: &str,
    ) -> Result<PathBuf, StorageError> {
        let staged_directory = staged_database.parent().ok_or_else(|| {
            core_publication_error(format!(
                "Core stage has no directory: {}",
                staged_database.display()
            ))
        })?;
        if staged_directory.parent() != Some(self.staging_root().as_path())
            || staged_database.file_name() != Some(std::ffi::OsStr::new(CORE_DATABASE_FILE))
        {
            return Err(core_publication_error(format!(
                "Core stage is outside the owned staging layout: {}",
                staged_database.display()
            )));
        }
        require_regular_generation_file(staged_database)?;
        make_file_immutable(staged_database)?;
        let generation_directory = self.generation_directory(generation_id)?;
        fs::create_dir_all(self.generations_root()).map_err(|error| {
            core_path_error("create generations root", &self.generations_root(), error)
        })?;
        if fs::symlink_metadata(&generation_directory).is_ok() {
            let _ = make_file_owner_writable(staged_database);
            return Err(core_publication_error(format!(
                "Core generation destination already exists: {}",
                generation_directory.display()
            )));
        }
        if let Err(error) = fs::rename(staged_directory, &generation_directory) {
            let _ = make_file_owner_writable(staged_database);
            return Err(core_path_error(
                "rename sealed generation",
                &generation_directory,
                error,
            ));
        }
        sync_parent(&generation_directory)?;
        Ok(generation_directory.join(CORE_DATABASE_FILE))
    }

    pub(crate) fn materialize_existing_generation(
        &self,
        source_database: &Path,
        generation_id: &str,
    ) -> Result<PathBuf, StorageError> {
        let destination = self.generation_database_path(generation_id)?;
        if destination.is_file() {
            return Ok(destination);
        }
        let staged = self.create_staging_database_path()?;
        let cloned = clone_file_copy_on_write(source_database, &staged)?;
        if !cloned {
            let _ = remove_staging_database(&staged);
            return Err(StorageError::Other(format!(
                "{CORE_COPY_ON_WRITE_UNAVAILABLE}: the filesystem cannot materialize immutable core generation {generation_id} without a foreground full copy"
            )));
        }
        make_file_owner_writable(&staged)?;
        self.install_staging_generation(&staged, generation_id)
    }
}

pub fn resolve_core_database_path(storage_path: &Path) -> Result<PathBuf, StorageError> {
    CorePublicationLayout::from_storage_path(storage_path)?
        .resolve_active_database()?
        .ok_or_else(|| {
            core_publication_error(format!(
                "No published core database exists for {}",
                storage_path.display()
            ))
        })
}

pub fn resolve_core_generation_database_path(
    storage_path: &Path,
    generation_id: &str,
) -> Result<PathBuf, StorageError> {
    let layout = CorePublicationLayout::from_storage_path(storage_path)?;
    if layout.read_pointer()?.is_some() {
        return layout.resolve_generation_database(generation_id);
    }
    layout.resolve_active_database()?.ok_or_else(|| {
        core_publication_error(format!(
            "No core database exists for legacy generation {generation_id}"
        ))
    })
}

pub fn core_database_exists(storage_path: &Path) -> Result<bool, StorageError> {
    Ok(CorePublicationLayout::from_storage_path(storage_path)?
        .resolve_active_database()?
        .is_some())
}

/// Clone a sealed generation into a distinct mutable stage without copying
/// unchanged extents. `Ok(false)` means the current platform/filesystem cannot
/// satisfy the copy-on-write contract; callers must not silently turn an
/// incremental refresh into a foreground full copy.
pub(crate) fn clone_file_copy_on_write(
    source: &Path,
    destination: &Path,
) -> Result<bool, StorageError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| core_path_error("inspect clone source", source, error))?;
    if !metadata.file_type().is_file() {
        return Err(core_publication_error(format!(
            "Core clone source is not a regular file: {}",
            source.display()
        )));
    }
    if fs::symlink_metadata(destination).is_ok() {
        return Err(core_publication_error(format!(
            "Core clone destination already exists: {}",
            destination.display()
        )));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| core_path_error("create clone parent", parent, error))?;
    }
    #[cfg(any(test, feature = "test-support"))]
    if core_clone_disabled() {
        return Ok(false);
    }
    let cloned = clone_file_copy_on_write_platform(source, destination)?;
    if cloned {
        return Ok(true);
    }
    // Production stays fail-closed without CoW. Tests still need to exercise
    // publication atomicity on filesystems (ext4 CI) that cannot reflink.
    // `with_core_clone_disabled` skips this fallback so escalate paths can run.
    #[cfg(any(test, feature = "test-support"))]
    {
        fs::copy(source, destination)
            .map_err(|error| core_path_error("test-only full copy stage", destination, error))?;
        return Ok(true);
    }
    #[cfg(not(any(test, feature = "test-support")))]
    Ok(false)
}

pub(crate) fn make_file_owner_writable(path: &Path) -> Result<(), StorageError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| core_path_error("inspect stage permissions", path, error))?;
    if !metadata.file_type().is_file() {
        return Err(core_publication_error(format!(
            "Core stage is not a regular file: {}",
            path.display()
        )));
    }
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|error| core_path_error("make stage writable", path, error))
}

pub(crate) fn make_file_immutable(path: &Path) -> Result<(), StorageError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| core_path_error("inspect generation permissions", path, error))?;
    if !metadata.file_type().is_file() {
        return Err(core_publication_error(format!(
            "Core generation is not a regular file: {}",
            path.display()
        )));
    }
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() & !0o222);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| core_path_error("make generation immutable", path, error))?;
    if !fs::symlink_metadata(path)
        .map_err(|error| core_path_error("reinspect generation permissions", path, error))?
        .permissions()
        .readonly()
    {
        return Err(core_publication_error(format!(
            "Core generation remained owner-writable: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn clone_file_copy_on_write_platform(
    source: &Path,
    destination: &Path,
) -> Result<bool, StorageError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source_c = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| core_publication_error("Core clone source contains an interior NUL".into()))?;
    let destination_c = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        core_publication_error("Core clone destination contains an interior NUL".into())
    })?;
    // SAFETY: both paths are live NUL-terminated buffers and clonefile retains
    // neither pointer.
    let result = unsafe { libc::clonefile(source_c.as_ptr(), destination_c.as_ptr(), 0) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    let _ = fs::remove_file(destination);
    match error.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::EINVAL) => Ok(false),
        _ => Err(core_path_error("clone core generation", destination, error)),
    }
}

#[cfg(target_os = "linux")]
fn clone_file_copy_on_write_platform(
    source: &Path,
    destination: &Path,
) -> Result<bool, StorageError> {
    use std::os::fd::AsRawFd;

    const FICLONE: libc::c_ulong = 0x4004_9409;
    let source_file =
        File::open(source).map_err(|error| core_path_error("open clone source", source, error))?;
    let destination_file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(destination)
        .map_err(|error| core_path_error("create clone destination", destination, error))?;
    // SAFETY: both descriptors remain valid for the ioctl and the kernel
    // retains neither descriptor.
    let result = unsafe {
        libc::ioctl(
            destination_file.as_raw_fd(),
            FICLONE,
            source_file.as_raw_fd(),
        )
    };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    drop(destination_file);
    let _ = fs::remove_file(destination);
    match error.raw_os_error() {
        Some(libc::EOPNOTSUPP | libc::EXDEV | libc::ENOTTY | libc::EINVAL) => Ok(false),
        _ => Err(core_path_error("clone core generation", destination, error)),
    }
}

#[cfg(windows)]
fn clone_file_copy_on_write_platform(
    source: &Path,
    destination: &Path,
) -> Result<bool, StorageError> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    const FSCTL_DUPLICATE_EXTENTS_TO_FILE: u32 = 0x0009_8344;
    const ERROR_INVALID_FUNCTION: i32 = 1;
    const ERROR_NOT_SUPPORTED: i32 = 50;
    const ERROR_INVALID_PARAMETER: i32 = 87;

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
    }

    let source_file =
        File::open(source).map_err(|error| core_path_error("open clone source", source, error))?;
    let length = source_file
        .metadata()
        .map_err(|error| core_path_error("inspect clone source", source, error))?
        .len();
    let byte_count = i64::try_from(length).map_err(|_| {
        core_publication_error("Core generation is too large for Windows block cloning".into())
    })?;
    let destination_file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(destination)
        .map_err(|error| core_path_error("create clone destination", destination, error))?;
    destination_file
        .set_len(length)
        .map_err(|error| core_path_error("size clone destination", destination, error))?;
    let mut request = DuplicateExtentsData {
        file_handle: source_file.as_raw_handle().cast(),
        source_file_offset: 0,
        target_file_offset: 0,
        byte_count,
    };
    let mut bytes_returned = 0_u32;
    // SAFETY: both file handles and the request remain live for the synchronous
    // call. The control operation retains no pointer.
    let result = unsafe {
        DeviceIoControl(
            destination_file.as_raw_handle().cast(),
            FSCTL_DUPLICATE_EXTENTS_TO_FILE,
            (&mut request as *mut DuplicateExtentsData).cast(),
            std::mem::size_of::<DuplicateExtentsData>() as u32,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
        )
    };
    if result != 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    drop(destination_file);
    let _ = fs::remove_file(destination);
    match error.raw_os_error() {
        Some(ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED | ERROR_INVALID_PARAMETER) => Ok(false),
        _ => Err(core_path_error("clone core generation", destination, error)),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn clone_file_copy_on_write_platform(
    _source: &Path,
    _destination: &Path,
) -> Result<bool, StorageError> {
    Ok(false)
}

pub(crate) fn pointer_receipt_digest(
    pointer: &CorePublicationPointerV1,
) -> Result<String, StorageError> {
    #[derive(Serialize)]
    struct ReceiptInput<'a> {
        schema_version: u32,
        active: &'a CoreGenerationIdentityV1,
        rollback: &'a Option<CoreGenerationIdentityV1>,
    }
    let bytes = serde_json::to_vec(&ReceiptInput {
        schema_version: pointer.schema_version,
        active: &pointer.active,
        rollback: &pointer.rollback,
    })
    .map_err(|error| core_publication_error(format!("Serialize core pointer receipt: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_pointer(pointer: &CorePublicationPointerV1) -> Result<(), StorageError> {
    if pointer.schema_version != CORE_PUBLICATION_POINTER_SCHEMA_VERSION {
        return Err(core_publication_error(format!(
            "Unsupported core publication pointer schema: {}",
            pointer.schema_version
        )));
    }
    validate_generation_identity(&pointer.active)?;
    if let Some(rollback) = pointer.rollback.as_ref() {
        validate_generation_identity(rollback)?;
        if rollback.generation_id == pointer.active.generation_id {
            return Err(core_publication_error(
                "Core active and rollback generations must be distinct".into(),
            ));
        }
    }
    if pointer.receipt_digest != pointer_receipt_digest(pointer)? {
        return Err(core_publication_error(
            "Core publication pointer receipt digest does not match its identities".into(),
        ));
    }
    Ok(())
}

fn validate_generation_identity(identity: &CoreGenerationIdentityV1) -> Result<(), StorageError> {
    validate_generation_id(&identity.generation_id)?;
    if identity.run_id.trim().is_empty()
        || identity.run_id.len() > MAX_GENERATION_ID_BYTES
        || identity.logical_bytes == 0
        || identity.published_at_epoch_ms < 0
    {
        return Err(core_publication_error(
            "Core generation identity contains an empty, oversized, zero, or negative field".into(),
        ));
    }
    Ok(())
}

fn validate_generation_id(generation_id: &str) -> Result<(), StorageError> {
    if generation_id.is_empty()
        || generation_id.len() > MAX_GENERATION_ID_BYTES
        || !generation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(core_publication_error(format!(
            "Core generation id is not a safe path atom: {generation_id:?}"
        )));
    }
    Ok(())
}

fn require_regular_generation_file(path: &Path) -> Result<(), StorageError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| core_path_error("inspect generation", path, error))?;
    if !metadata.file_type().is_file() {
        return Err(core_publication_error(format!(
            "Core generation database is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn write_pointer_atomic(
    destination: &Path,
    pointer: &CorePublicationPointerV1,
) -> Result<(), StorageError> {
    let parent = destination.parent().ok_or_else(|| {
        core_publication_error(format!(
            "Core publication pointer has no parent: {}",
            destination.display()
        ))
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| core_path_error("create pointer parent", parent, error))?;
    let bytes = serde_json::to_vec(pointer)
        .map_err(|error| core_publication_error(format!("Serialize core pointer: {error}")))?;
    let temporary = parent.join(format!(
        ".{CORE_PUBLICATION_FILE}.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| core_path_error("create pointer candidate", &temporary, error))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|error| core_path_error("write pointer candidate", &temporary, error))?;
        file.sync_all()
            .map_err(|error| core_path_error("sync pointer candidate", &temporary, error))?;
        drop(file);
        #[cfg(test)]
        abort_after_publication_point("pointer_write")?;
        replace_file_atomic(&temporary, destination)?;
        #[cfg(test)]
        abort_after_publication_point("pointer_replacement")?;
        sync_parent(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn sync_staging_database(path: &Path) -> Result<(), StorageError> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| core_path_error("sync staged generation", path, error))?;
    sync_parent(path)
}

#[cfg(test)]
pub(crate) fn abort_after_publication_point(point: &str) -> Result<(), StorageError> {
    if std::env::var(CORE_PUBLICATION_ABORT_POINT_ENV).as_deref() != Ok(point) {
        return Ok(());
    }
    let sentinel_path = std::env::var_os(CORE_PUBLICATION_ABORT_SENTINEL_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| {
            core_publication_error(format!(
                "Crash injection point {point} has no sentinel path"
            ))
        })?;
    let mut sentinel = File::create(&sentinel_path)
        .map_err(|error| core_path_error("create crash sentinel", &sentinel_path, error))?;
    sentinel
        .write_all(format!("{point}\n").as_bytes())
        .map_err(|error| core_path_error("write crash sentinel", &sentinel_path, error))?;
    sentinel
        .sync_all()
        .map_err(|error| core_path_error("sync crash sentinel", &sentinel_path, error))?;
    std::process::abort();
}

#[cfg(not(windows))]
fn replace_file_atomic(source: &Path, destination: &Path) -> Result<(), StorageError> {
    fs::rename(source, destination)
        .map_err(|error| core_path_error("replace pointer", destination, error))
}

#[cfg(windows)]
fn replace_file_atomic(source: &Path, destination: &Path) -> Result<(), StorageError> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both strings are live, NUL-terminated UTF-16 buffers and the
    // call retains neither pointer.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        return Err(core_path_error(
            "replace pointer",
            destination,
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<(), StorageError> {
    #[cfg(not(windows))]
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| core_path_error("sync pointer directory", parent, error))?;
    }
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

pub(crate) fn remove_staging_database(path: &Path) -> Result<(), StorageError> {
    let directory = path.parent().ok_or_else(|| {
        core_publication_error(format!("Stage has no directory: {}", path.display()))
    })?;
    if directory.parent().and_then(Path::file_name)
        != Some(std::ffi::OsStr::new(CORE_STAGING_DIRECTORY))
    {
        return Err(core_publication_error(format!(
            "Refusing to remove a non-core staging directory: {}",
            directory.display()
        )));
    }
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(core_path_error("remove stage", directory, error)),
    }
}

fn core_publication_error(message: String) -> StorageError {
    StorageError::Other(format!("core_publication_invalid: {message}"))
}

fn core_path_error(operation: &str, path: &Path, error: std::io::Error) -> StorageError {
    StorageError::Other(format!(
        "core_publication_io: Failed to {operation} {}: {error}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(label: &str, bytes: u64) -> CoreGenerationIdentityV1 {
        CoreGenerationIdentityV1 {
            generation_id: format!("generation-{label}"),
            run_id: format!("run-{label}"),
            logical_bytes: bytes,
            published_at_epoch_ms: 1,
        }
    }

    fn seed_generation(layout: &CorePublicationLayout, id: &CoreGenerationIdentityV1) {
        let path = layout
            .generation_database_path(&id.generation_id)
            .expect("generation path");
        fs::create_dir_all(path.parent().expect("generation parent")).expect("create generation");
        fs::write(path, b"SQLite generation fixture").expect("seed generation");
    }

    #[test]
    fn pointer_selects_one_active_and_one_rollback_generation() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let layout = CorePublicationLayout::from_storage_path(&root.path().join("codestory.db"))
            .expect("layout");
        let first = identity("one", 4_096);
        let second = identity("two", 8_192);
        seed_generation(&layout, &first);
        seed_generation(&layout, &second);

        let pointer = layout
            .publish_pointer(second.clone(), Some(first.clone()))
            .expect("publish pointer");

        assert_eq!(layout.read_pointer().expect("read"), Some(pointer));
        assert_eq!(
            layout.resolve_active_database().expect("resolve"),
            Some(
                layout
                    .generation_database_path(&second.generation_id)
                    .expect("active path")
            )
        );
    }

    #[test]
    fn receipt_tampering_is_rejected_before_generation_resolution() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let layout = CorePublicationLayout::from_storage_path(&root.path().join("codestory.db"))
            .expect("layout");
        let active = identity("active", 4_096);
        seed_generation(&layout, &active);
        layout
            .publish_pointer(active, None)
            .expect("publish pointer");
        let path = layout.publication_path();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read pointer")).expect("parse");
        value["active"]["run_id"] = serde_json::Value::String("tampered".into());
        fs::write(&path, serde_json::to_vec(&value).expect("encode")).expect("tamper");

        let error = layout.read_pointer().expect_err("tampering must fail");

        assert!(error.to_string().contains("receipt digest"));
    }

    #[test]
    fn generation_id_cannot_escape_the_owned_directory() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let layout = CorePublicationLayout::from_storage_path(&root.path().join("codestory.db"))
            .expect("layout");

        let error = layout
            .generation_database_path("../outside")
            .expect_err("path traversal must fail");

        assert!(error.to_string().contains("safe path atom"));
    }

    #[test]
    fn copy_on_write_stage_is_distinct_when_the_filesystem_supports_it() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = root.path().join("source.db");
        let destination = root.path().join("stage.db");
        fs::write(&source, b"immutable generation").expect("source");

        if !clone_file_copy_on_write(&source, &destination).expect("clone") {
            return;
        }
        fs::write(&destination, b"candidate generation").expect("mutate stage");

        assert_eq!(
            fs::read(source).expect("source bytes"),
            b"immutable generation"
        );
        assert_eq!(
            fs::read(destination).expect("stage bytes"),
            b"candidate generation"
        );
    }

    #[test]
    fn clone_disabled_never_silent_copies_and_reports_unavailable() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let source = root.path().join("source.db");
        let destination = root.path().join("stage.db");
        fs::write(&source, b"immutable generation").expect("source");

        let cloned = with_core_clone_disabled(|| {
            clone_file_copy_on_write(&source, &destination).expect("clone probe")
        });

        assert!(
            !cloned,
            "disabled CoW must return Ok(false), not a silent full copy"
        );
        assert!(
            !destination.exists(),
            "disabled CoW must not materialize a destination via fs::copy"
        );
    }
}
