//! Lifetime pins and bounded reclamation for immutable core images.
//!
//! The store chooses and authenticates obsolete generations. The runtime holds
//! retrieval's global publication fence and supplies its handle-relative
//! deletion primitive, so this layer never depends on retrieval or workspace.

use super::{
    PromotionLock, RecoveryDatabaseContract, StorageError, read_recovery_database_identity,
};
use crate::core_generation::{
    CORE_DATABASE_FILE, CORE_GENERATIONS_DIRECTORY, CorePublicationLayout,
};
use codestory_contracts::bounded_locks::{
    self, FileLockKind, LockDeadline, PUBLICATION_LOCK_WAIT, acquire_with_deadline,
};
use codestory_contracts::core_publication::CorePublicationPointerV1;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub const LEGACY_RETIREMENT_RECEIPT_FILE: &str = "legacy-retirement.json";
const LEGACY_RETIREMENT_RECEIPT_VERSION: u32 = 1;
const MAX_LEGACY_RETIREMENT_RECEIPT_BYTES: u64 = 4096;
const LEGACY_SQLITE_SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum LegacyNativeIdentity {
    Unix { device: u64, inode: u64 },
    Windows { volume: u64, file_id: [u8; 16] },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LegacySidecarIdentity {
    suffix: String,
    identity: LegacyNativeIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LegacyRetirementReceipt {
    version: u32,
    source_identity: LegacyNativeIdentity,
    candidate_generation_id: String,
    sidecars: Vec<LegacySidecarIdentity>,
    committed: bool,
    retired: bool,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LegacyRetirementReport {
    pub retired: bool,
    pub pending: bool,
    pub legacy_bytes: u64,
    pub errors: Vec<String>,
}

pub(super) fn capture_legacy_identity(path: &Path) -> Result<LegacyNativeIdentity, StorageError> {
    let file = open_direct_regular_file(path)?;
    native_identity(&file)
}

fn native_identity(file: &File) -> Result<LegacyNativeIdentity, StorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = file
            .metadata()
            .map_err(|error| retention_error("identify legacy file", error))?;
        Ok(LegacyNativeIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle as _;
        #[repr(C)]
        struct FileIdInfo {
            volume: u64,
            file_id: [u8; 16],
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandleEx(
                handle: *mut c_void,
                class: i32,
                info: *mut c_void,
                size: u32,
            ) -> i32;
        }
        let mut info = MaybeUninit::<FileIdInfo>::uninit();
        // SAFETY: file owns a live handle and the correctly sized output buffer is writable.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle().cast(),
                18,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<FileIdInfo>() as u32,
            )
        } == 0
        {
            return Err(retention_error(
                "identify legacy file",
                io::Error::last_os_error(),
            ));
        }
        let info = unsafe { info.assume_init() };
        Ok(LegacyNativeIdentity::Windows {
            volume: info.volume,
            file_id: info.file_id,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(StorageError::Other(
            "Legacy retirement requires native file identity".into(),
        ))
    }
}

fn receipt_path(layout: &CorePublicationLayout) -> PathBuf {
    layout.root().join(LEGACY_RETIREMENT_RECEIPT_FILE)
}

fn read_legacy_receipt(
    layout: &CorePublicationLayout,
) -> Result<Option<LegacyRetirementReceipt>, StorageError> {
    use std::io::Read as _;
    let path = receipt_path(layout);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(retention_error("inspect legacy receipt", error)),
        Ok(_) => {}
    }
    let mut file = open_direct_regular_file(&path)?;
    let length = file
        .metadata()
        .map_err(|error| retention_error("inspect legacy receipt", error))?
        .len();
    if length > MAX_LEGACY_RETIREMENT_RECEIPT_BYTES {
        return Err(StorageError::Other(
            "Legacy retirement receipt exceeds its bound".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_LEGACY_RETIREMENT_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| retention_error("read legacy receipt", error))?;
    if bytes.len() as u64 > MAX_LEGACY_RETIREMENT_RECEIPT_BYTES {
        return Err(StorageError::Other(
            "Legacy retirement receipt exceeds its bound".into(),
        ));
    }
    let receipt: LegacyRetirementReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| retention_error("parse legacy receipt", error))?;
    let recorded_suffixes: BTreeSet<_> = receipt
        .sidecars
        .iter()
        .map(|sidecar| sidecar.suffix.as_str())
        .collect();
    if receipt.version != LEGACY_RETIREMENT_RECEIPT_VERSION
        || crate::core_generation::validate_generation_id(&receipt.candidate_generation_id).is_err()
        || (receipt.retired && !receipt.committed)
        || receipt.sidecars.len() > LEGACY_SQLITE_SIDECARS.len()
        || recorded_suffixes.len() != receipt.sidecars.len()
        || receipt
            .sidecars
            .iter()
            .any(|sidecar| !LEGACY_SQLITE_SIDECARS.contains(&sidecar.suffix.as_str()))
    {
        return Err(StorageError::Other(
            "Legacy retirement receipt has unsupported ownership evidence".into(),
        ));
    }
    Ok(Some(receipt))
}

fn write_legacy_receipt(
    layout: &CorePublicationLayout,
    receipt: &LegacyRetirementReceipt,
) -> Result<(), StorageError> {
    use std::io::Write as _;
    let path = receipt_path(layout);
    fs::create_dir_all(layout.root())
        .map_err(|error| retention_error("create legacy receipt directory", error))?;
    let temporary = layout.root().join(format!(
        ".{LEGACY_RETIREMENT_RECEIPT_FILE}.{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let bytes = serde_json::to_vec(receipt)
        .map_err(|error| retention_error("serialize legacy receipt", error))?;
    if bytes.len() as u64 > MAX_LEGACY_RETIREMENT_RECEIPT_BYTES {
        return Err(StorageError::Other(
            "Legacy retirement receipt exceeds its bound".into(),
        ));
    }
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| retention_error("create legacy receipt candidate", error))?;
        file.write_all(&bytes)
            .map_err(|error| retention_error("write legacy receipt candidate", error))?;
        file.sync_all()
            .map_err(|error| retention_error("sync legacy receipt candidate", error))?;
        drop(file);
        crate::core_generation::replace_file_atomic(&temporary, &path)?;
        crate::sealed_file_stage::sync_parent(&path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn prepare_legacy_retirement(
    layout: &CorePublicationLayout,
    source_identity: LegacyNativeIdentity,
    candidate_generation_id: &str,
) -> Result<(), StorageError> {
    if let Some(existing) = read_legacy_receipt(layout)?
        && existing.source_identity != source_identity
    {
        return Err(StorageError::Other(
            "Prior legacy retirement receipt names another native source; refusing replacement"
                .into(),
        ));
    }
    if capture_legacy_identity(layout.legacy_storage_path())? != source_identity {
        return Err(StorageError::Other(
            "Legacy database identity changed during rollback backup".into(),
        ));
    }
    let mut sidecars = Vec::new();
    for suffix in LEGACY_SQLITE_SIDECARS {
        let path = super::sqlite_sidecar_path(layout.legacy_storage_path(), suffix);
        match fs::symlink_metadata(&path) {
            Ok(_) => sidecars.push(LegacySidecarIdentity {
                suffix: suffix.into(),
                identity: capture_legacy_identity(&path)?,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(retention_error("inspect legacy sidecar", error)),
        }
    }
    write_legacy_receipt(
        layout,
        &LegacyRetirementReceipt {
            version: LEGACY_RETIREMENT_RECEIPT_VERSION,
            source_identity,
            candidate_generation_id: candidate_generation_id.into(),
            sidecars,
            committed: false,
            retired: false,
            last_error: None,
        },
    )
}

pub(super) fn mark_legacy_retirement_committed(
    layout: &CorePublicationLayout,
) -> Result<(), StorageError> {
    let Some(mut receipt) = read_legacy_receipt(layout)? else {
        return Ok(());
    };
    if receipt.committed {
        return Ok(());
    }
    let Some(pointer) = layout.read_pointer()? else {
        return Ok(());
    };
    if pointer.active.generation_id != receipt.candidate_generation_id
        && pointer
            .rollback
            .as_ref()
            .is_none_or(|rollback| rollback.generation_id != receipt.candidate_generation_id)
    {
        return Ok(());
    }
    receipt.committed = true;
    write_legacy_receipt(layout, &receipt)
}

pub fn observe_legacy_retirement(
    logical_path: &Path,
) -> Result<LegacyRetirementReport, StorageError> {
    let layout = CorePublicationLayout::from_storage_path(logical_path)?;
    let Some(receipt) = read_legacy_receipt(&layout)? else {
        return Ok(LegacyRetirementReport::default());
    };
    let mut report = LegacyRetirementReport {
        retired: receipt.retired,
        pending: !receipt.retired,
        ..LegacyRetirementReport::default()
    };
    if let Some(reason) = receipt.last_error {
        report.errors.push(reason);
    }
    if report.pending {
        match fs::symlink_metadata(logical_path) {
            Ok(_) => match open_direct_regular_file(logical_path) {
                Ok(file) => match native_identity(&file) {
                    Ok(identity) if identity == receipt.source_identity => {
                        report.legacy_bytes = file
                            .metadata()
                            .map_err(|error| {
                                retention_error("measure observed legacy file", error)
                            })?
                            .len();
                    }
                    Ok(_) => report
                        .errors
                        .push("legacy retirement refused replaced native identity".into()),
                    Err(error) => report
                        .errors
                        .push(format!("legacy database is not observable: {error}")),
                },
                Err(error) => report
                    .errors
                    .push(format!("legacy database is not observable: {error}")),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => report
                .errors
                .push(format!("legacy database is not observable: {error}")),
        }
    }
    if !receipt.committed && !receipt.retired {
        report
            .errors
            .push("legacy retirement awaits a committed core pointer".into());
    }
    Ok(report)
}

pub fn apply_legacy_retirement(
    logical_path: &Path,
    cancelled: &dyn Fn() -> bool,
    mut remove_owned_file: impl FnMut(&Path, &str, &File) -> Result<bool, StorageError>,
) -> Result<LegacyRetirementReport, StorageError> {
    let layout = CorePublicationLayout::from_storage_path(logical_path)?;
    if read_legacy_receipt(&layout)?.is_none() {
        return Ok(LegacyRetirementReport::default());
    }
    let Some(_promotion) = PromotionLock::try_acquire(logical_path)? else {
        return Ok(LegacyRetirementReport {
            pending: true,
            errors: vec!["core promotion is active".into()],
            ..LegacyRetirementReport::default()
        });
    };
    let Some(_acquisition) = acquire_acquisition(&layout, FileLockKind::Exclusive, true)? else {
        return Ok(LegacyRetirementReport {
            pending: true,
            errors: vec!["core acquisition is active".into()],
            ..LegacyRetirementReport::default()
        });
    };
    mark_legacy_retirement_committed(&layout)?;
    let Some(mut receipt) = read_legacy_receipt(&layout)? else {
        return Ok(LegacyRetirementReport::default());
    };
    if receipt.retired {
        return Ok(LegacyRetirementReport {
            retired: true,
            ..LegacyRetirementReport::default()
        });
    }
    let mut report = LegacyRetirementReport {
        pending: true,
        ..LegacyRetirementReport::default()
    };
    if !receipt.committed || cancelled() {
        report
            .errors
            .push("legacy retirement is not committed or was cancelled".into());
        return defer_legacy_retirement(&layout, &mut receipt, report);
    }
    let parent = logical_path
        .parent()
        .ok_or_else(|| StorageError::Other("Legacy path has no parent".into()))?;
    let name = logical_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StorageError::Other("Legacy database has no safe file name".into()))?;
    let mut owned = Vec::new();
    for sidecar in &receipt.sidecars {
        let sidecar_name = format!("{name}{}", sidecar.suffix);
        let path = parent.join(&sidecar_name);
        if let Some(file) = open_matching_legacy_file(&path, &sidecar.identity, &mut report)? {
            owned.push((sidecar_name, file));
        }
    }
    for suffix in LEGACY_SQLITE_SIDECARS {
        if receipt
            .sidecars
            .iter()
            .any(|sidecar| sidecar.suffix == suffix)
        {
            continue;
        }
        match fs::symlink_metadata(parent.join(format!("{name}{suffix}"))) {
            Ok(_) => report.errors.push(format!(
                "Unrecorded legacy sidecar {suffix} appeared; retirement deferred"
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => report
                .errors
                .push(format!("Cannot inspect legacy sidecar {suffix}: {error}")),
        }
    }
    let source = open_matching_legacy_file(logical_path, &receipt.source_identity, &mut report)?;
    if !report.errors.is_empty() {
        return defer_legacy_retirement(&layout, &mut receipt, report);
    }
    // Refuse an in-use database before touching its WAL or shared-memory
    // sidecars. Windows deletion sharing can reject the main file while its
    // SQLite client still needs those sidecars.
    if let Some(file) = source {
        report.legacy_bytes = file
            .metadata()
            .map_err(|error| retention_error("measure legacy file", error))?
            .len();
        match remove_owned_file(parent, name, &file) {
            Ok(true) => {}
            Ok(false) => report
                .errors
                .push("Legacy database changed before deletion".into()),
            Err(error) => report
                .errors
                .push(format!("Legacy database deletion deferred: {error}")),
        }
    }
    if !report.errors.is_empty() {
        return defer_legacy_retirement(&layout, &mut receipt, report);
    }
    for (sidecar_name, file) in &owned {
        match remove_owned_file(parent, sidecar_name, file) {
            Ok(true) => {}
            Ok(false) => report.errors.push(format!(
                "Legacy sidecar {sidecar_name} changed before deletion"
            )),
            Err(error) => report.errors.push(format!(
                "Legacy sidecar {sidecar_name} deletion deferred: {error}"
            )),
        }
    }
    if !report.errors.is_empty() {
        return defer_legacy_retirement(&layout, &mut receipt, report);
    }
    receipt.retired = true;
    receipt.last_error = None;
    write_legacy_receipt(&layout, &receipt)?;
    report.pending = false;
    report.retired = true;
    Ok(report)
}

fn defer_legacy_retirement(
    layout: &CorePublicationLayout,
    receipt: &mut LegacyRetirementReceipt,
    report: LegacyRetirementReport,
) -> Result<LegacyRetirementReport, StorageError> {
    receipt.last_error = Some(report.errors.join("; "));
    write_legacy_receipt(layout, receipt)?;
    Ok(report)
}

fn open_matching_legacy_file(
    path: &Path,
    expected: &LegacyNativeIdentity,
    report: &mut LegacyRetirementReport,
) -> Result<Option<File>, StorageError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(retention_error("inspect legacy retirement file", error)),
        Ok(_) => {}
    }
    let file = open_direct_regular_file(path)?;
    if &native_identity(&file)? != expected {
        report.errors.push(format!(
            "Legacy retirement refused replaced native identity at {}",
            path.display()
        ));
        return Ok(None);
    }
    Ok(Some(file))
}

#[cfg(test)]
thread_local! {
    static AFTER_POINTER_BEFORE_LEASE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static FAIL_ENUMERATION_AFTER: std::cell::RefCell<Option<usize>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn set_after_pointer_before_lease(hook: impl FnOnce() + 'static) {
    AFTER_POINTER_BEFORE_LEASE.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_pointer_before_lease() {
    AFTER_POINTER_BEFORE_LEASE.with(|slot| {
        let hook = slot.borrow_mut().take();
        if let Some(hook) = hook {
            hook();
        }
    });
}

#[cfg(test)]
fn fail_enumeration_after(entries: usize) {
    FAIL_ENUMERATION_AFTER.with(|slot| *slot.borrow_mut() = Some(entries));
}

#[cfg(test)]
fn inject_enumeration_failure() -> Result<(), StorageError> {
    FAIL_ENUMERATION_AFTER.with(|slot| {
        let mut remaining = slot.borrow_mut();
        match *remaining {
            Some(0) => {
                *remaining = None;
                Err(StorageError::Other(
                    "injected late core generation enumeration failure".into(),
                ))
            }
            Some(count) => {
                *remaining = Some(count - 1);
                Ok(())
            }
            None => Ok(()),
        }
    })
}

pub use codestory_contracts::owned_artifacts::CORE_LEASE_FILE;
use codestory_contracts::owned_artifacts::{self, CORE_ACQUISITION_FILE};
const MAX_RECLAIMS_PER_PASS: usize = 16;

pub(crate) struct CoreGenerationLease(File);

impl Drop for CoreGenerationLease {
    fn drop(&mut self) {
        let _ = bounded_locks::release(&self.0);
    }
}

struct CoreAcquisitionLock(File);

impl Drop for CoreAcquisitionLock {
    fn drop(&mut self) {
        let _ = bounded_locks::release(&self.0);
    }
}

/// Excludes writers and existing/new core readers without opening any database.
/// The caller holds retrieval's global exclusive fence before acquiring this.
/// Coordination paths remain in place throughout quarantine.
pub struct CoreResetExclusion {
    held: Vec<File>,
}

impl CoreResetExclusion {
    pub fn acquire(logical_path: &Path) -> Result<Self, StorageError> {
        let mut exclusion = Self { held: Vec::new() };
        let layout = CorePublicationLayout::from_storage_path(logical_path)?;
        let paths = owned_artifacts::derived_reset_held_lock_paths(logical_path);
        // Same order as indexing/promotion, then retention's acquisition fence.
        for path in &paths[..2] {
            let file = open_regular_lock(path, true)?.expect("create returns a file");
            acquire_with_deadline(
                &file,
                FileLockKind::Exclusive,
                LockDeadline::after(PUBLICATION_LOCK_WAIT),
                None,
            )
            .map_err(|error| retention_error("acquire derived reset writer exclusion", error))?;
            exclusion.held.push(file);
        }
        match fs::symlink_metadata(layout.root()) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(exclusion),
            Err(error) => return Err(retention_error("inspect reset core root", error)),
            Ok(_) => require_direct_directory(layout.root())?,
        }
        let acquisition = open_regular_lock(&paths[2], false)?.ok_or_else(|| {
            StorageError::Other("Derived reset requires a provisioned core acquisition lock; restore a compatible cache or stop older clients before manual recovery".into())
        })?;
        if !bounded_locks::try_acquire(&acquisition, FileLockKind::Exclusive)
            .map_err(|error| retention_error("exclude core acquisition for reset", error))?
        {
            return Err(StorageError::Other(
                "Core reader acquisition is active; retry derived reset when readers are idle"
                    .into(),
            ));
        }
        exclusion.held.push(acquisition);
        let generations = layout.generations_root();
        match fs::symlink_metadata(&generations) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(exclusion),
            Err(error) => return Err(retention_error("inspect reset generations", error)),
            Ok(_) => require_direct_directory(&generations)?,
        }
        // Finish enumeration and take every named lease before any caller moves
        // state. An unknown entry or absent old-format lease cannot prove idle.
        for entry in fs::read_dir(&generations)
            .map_err(|error| retention_error("enumerate reset generations", error))?
        {
            #[cfg(test)]
            inject_enumeration_failure()?;
            let entry = entry.map_err(|error| retention_error("read reset generation", error))?;
            let name = entry.file_name();
            let id = name.to_str().ok_or_else(|| {
                StorageError::Other("Reset generation has no safe UTF-8 identity".into())
            })?;
            let database = layout.generation_database_path(id)?;
            require_direct_directory(&entry.path())?;
            match try_acquire_generation_exclusive(&database)? {
                NamedLeaseTry::Held(file) => exclusion.held.push(file),
                NamedLeaseTry::Contended => return Err(StorageError::Other("Core reader is active; retry derived reset when readers are idle".into())),
                NamedLeaseTry::Unprovisioned => return Err(StorageError::Other("Derived reset refuses an unprovisioned core generation; restore a compatible cache or stop older clients before manual recovery".into())),
            }
        }
        Ok(exclusion)
    }
}

impl Drop for CoreResetExclusion {
    fn drop(&mut self) {
        for file in self.held.iter().rev() {
            let _ = bounded_locks::release(file);
        }
    }
}

pub(crate) struct PinnedActiveCore {
    pub pointer: CorePublicationPointerV1,
    pub path: PathBuf,
    pub lease: Option<CoreGenerationLease>,
}

/// The writer provisions the two lock artifacts before it installs a sealed
/// stage. An old generation without this evidence is never prune eligible.
pub(crate) fn provision_generation_locks(
    layout: &CorePublicationLayout,
    staged_directory: &Path,
) -> Result<(), StorageError> {
    match fs::symlink_metadata(layout.root()) {
        Ok(_) => require_direct_directory(layout.root())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(layout.root())
                .map_err(|error| retention_error("create core root", error))?;
        }
        Err(error) => return Err(retention_error("inspect core root", error)),
    }
    require_direct_directory(layout.root())?;
    let acquisition = layout.root().join(CORE_ACQUISITION_FILE);
    let file = open_regular_lock(&acquisition, true)?.expect("create returns a file");
    file.sync_all()
        .map_err(|error| retention_error("sync acquisition lock", error))?;
    require_direct_directory(staged_directory)?;
    let lease_path = staged_directory.join(CORE_LEASE_FILE);
    let lease = open_new_regular_lock(&lease_path)?;
    lease
        .sync_all()
        .map_err(|error| retention_error("sync generation lease", error))?;
    Ok(())
}

/// Pin the exact pointer-selected generation without creating any lock state.
/// The acquisition lock covers pointer resolution through the named lease.
pub(crate) fn pin_active_core(
    layout: &CorePublicationLayout,
) -> Result<Option<PinnedActiveCore>, StorageError> {
    let _acquisition = acquire_acquisition(layout, FileLockKind::Shared, false)?;
    let Some(pointer) = layout.read_pointer()? else {
        return Ok(None);
    };
    let path = layout.resolve_generation_database(&pointer.active.generation_id)?;
    #[cfg(test)]
    run_after_pointer_before_lease();
    let lease = acquire_generation_read_lease(&path)?;
    Ok(Some(PinnedActiveCore {
        pointer,
        path,
        lease,
    }))
}

/// Pin a path that may be a published core or a disposable test/staged image.
/// Only exact `core/generations/<id>/codestory.db` paths have a lease marker.
pub(crate) fn pin_exact_core(path: &Path) -> Result<Option<CoreGenerationLease>, StorageError> {
    let Some(generation_dir) = path.parent() else {
        return Ok(None);
    };
    let Some(generations_root) = generation_dir.parent() else {
        return Ok(None);
    };
    if path.file_name() != Some(std::ffi::OsStr::new(CORE_DATABASE_FILE))
        || generations_root.file_name() != Some(std::ffi::OsStr::new(CORE_GENERATIONS_DIRECTORY))
    {
        return Ok(None);
    }
    let Some(core_root) = generations_root.parent() else {
        return Ok(None);
    };
    let Some(logical_parent) = core_root.parent() else {
        return Ok(None);
    };
    let layout =
        CorePublicationLayout::from_storage_path(&logical_parent.join(CORE_DATABASE_FILE))?;
    if layout.root() != core_root {
        return Ok(None);
    }
    let generation_id = generation_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| StorageError::Other("Core generation has no safe UTF-8 id".into()))?;
    if layout.generation_database_path(generation_id)? != path {
        return Ok(None);
    }
    let _acquisition = acquire_acquisition(&layout, FileLockKind::Shared, false)?;
    require_direct_directory(generations_root)?;
    require_direct_directory(generation_dir)?;
    acquire_generation_read_lease(path)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CoreRetentionReport {
    pub reclaimed_images: usize,
    pub reclaimed_logical_bytes: u64,
    pub deferred_pins: usize,
    pub protected_generations: usize,
    pub unprovisioned_generations: usize,
    pub unknown_entries: usize,
    pub pruning_suppressed: bool,
    pub errors: Vec<String>,
}

/// Apply a pass with at most `MAX_RECLAIMS_PER_PASS` removals while the caller
/// holds retrieval's global exclusive publication lock. Enumerate every entry
/// before deleting anything, so an incomplete scan cannot authorize absence.
/// Discovery is O(the generation directory size) and cancellable; it is not a
/// constant-time operation. The callback removes only the authenticated image
/// and its lease marker through the runtime's pinned-directory primitive.
pub fn apply_core_retention(
    logical_path: &Path,
    cancelled: &dyn Fn() -> bool,
    mut remove_owned_generation: impl FnMut(
        &Path,
        &str,
        &File,
        &File,
        &File,
    ) -> Result<bool, StorageError>,
) -> Result<CoreRetentionReport, StorageError> {
    let mut report = CoreRetentionReport::default();
    let layout = CorePublicationLayout::from_storage_path(logical_path)?;
    let Some(_promotion) = PromotionLock::try_acquire(logical_path)? else {
        report.pruning_suppressed = true;
        report.errors.push("core promotion is active".into());
        return Ok(report);
    };
    let Some(_acquisition) = acquire_acquisition(&layout, FileLockKind::Exclusive, true)? else {
        report.pruning_suppressed = true;
        report
            .errors
            .push("core acquisition lock is absent or contended".into());
        return Ok(report);
    };
    let Some(pointer) = layout.read_pointer()? else {
        report.pruning_suppressed = true;
        report
            .errors
            .push("core publication pointer is absent".into());
        return Ok(report);
    };
    let mut protected = BTreeSet::new();
    protected.insert(pointer.active.generation_id);
    if let Some(rollback) = pointer.rollback {
        protected.insert(rollback.generation_id);
    }
    protected.extend(super::retrieval_manifest::retained_retrieval_core_ids(
        &layout.retrieval_publication_path(),
    )?);
    report.protected_generations = protected.len();

    let generations_root = layout.generations_root();
    require_direct_directory(&generations_root)?;
    let entries = fs::read_dir(&generations_root)
        .map_err(|error| retention_error("enumerate core generations", error))?;
    let mut candidates = Vec::new();
    for entry in entries {
        if cancelled() {
            report.pruning_suppressed = true;
            report
                .errors
                .push("core retention enumeration was cancelled".into());
            return Ok(report);
        }
        let entry = entry.map_err(|error| retention_error("read core generation entry", error))?;
        #[cfg(test)]
        inject_enumeration_failure()?;
        let Some(generation_id) = entry.file_name().to_str().map(str::to_owned) else {
            report.unknown_entries += 1;
            continue;
        };
        if layout.generation_database_path(&generation_id).is_err() {
            report.unknown_entries += 1;
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| retention_error("inspect core generation entry", error))?;
        if !file_type.is_dir() || file_type.is_symlink() {
            report.unknown_entries += 1;
            continue;
        }
        if !protected.contains(&generation_id) {
            candidates.push(generation_id);
        }
    }
    if cancelled() {
        report.pruning_suppressed = true;
        report
            .errors
            .push("core retention enumeration was cancelled".into());
        return Ok(report);
    }
    candidates.sort();
    for generation_id in candidates {
        if report.reclaimed_images >= MAX_RECLAIMS_PER_PASS || cancelled() {
            break;
        }
        let directory = layout.generation_directory(&generation_id)?;
        if let Err(error) = require_direct_directory(&directory) {
            report.errors.push(error.to_string());
            continue;
        }
        let database = directory.join(CORE_DATABASE_FILE);
        match candidate_file_shape(&directory)? {
            CandidateFileShape::Complete => {}
            CandidateFileShape::Unprovisioned => {
                report.unprovisioned_generations += 1;
                continue;
            }
            CandidateFileShape::Unknown => {
                report.unknown_entries += 1;
                continue;
            }
        }
        let opened_directory = open_direct_directory(&directory)?;
        let opened_database = open_direct_regular_file(&database)?;
        let lock = match try_acquire_generation_exclusive(&database)? {
            NamedLeaseTry::Unprovisioned => {
                report.unprovisioned_generations += 1;
                continue;
            }
            NamedLeaseTry::Contended => {
                report.deferred_pins += 1;
                continue;
            }
            NamedLeaseTry::Held(lock) => lock,
        };
        let candidate = match crate::sqlite_observation::observe_sqlite_database(&database)
            .and_then(|observation| {
                read_recovery_database_identity(
                    &database,
                    RecoveryDatabaseContract::CurrentPromotion,
                )
                .map(|identity| identity.map(|identity| (identity, observation.logical_bytes)))
            }) {
            Ok(Some(candidate)) if candidate.0.generation_id == generation_id => candidate,
            Ok(_) => {
                report.errors.push(format!(
                    "Core generation {generation_id} has no matching complete publication"
                ));
                continue;
            }
            Err(error) => {
                report.errors.push(format!(
                    "Core generation {generation_id} is not safely observable: {error}"
                ));
                continue;
            }
        };
        // The SQLite observer opens by name. Bind that validation to the
        // handles supplied to the remover, which rechecks native identity at
        // the final leaf operation. Unix still has a name-based last syscall;
        // the held publication/acquisition locks exclude cooperating writers.
        require_opened_matches_path(&opened_directory, &directory, false)?;
        require_opened_matches_path(&opened_database, &database, true)?;
        // Keep the original marker handle as deletion evidence. Its advisory
        // lock is released before the callback removes the marker, while the
        // exclusive acquisition fence still excludes cooperating readers.
        bounded_locks::release(&lock)
            .map_err(|error| retention_error("release retired core lease lock", error))?;
        if remove_owned_generation(
            &generations_root,
            &generation_id,
            &opened_directory,
            &opened_database,
            &lock,
        )? {
            report.reclaimed_images += 1;
            report.reclaimed_logical_bytes =
                report.reclaimed_logical_bytes.saturating_add(candidate.1);
        } else {
            report.errors.push(format!(
                "Core generation {generation_id} changed or removal was refused"
            ));
        }
    }
    Ok(report)
}

enum CandidateFileShape {
    Complete,
    Unprovisioned,
    Unknown,
}

fn candidate_file_shape(directory: &Path) -> Result<CandidateFileShape, StorageError> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| retention_error("enumerate generation files", error))?
    {
        let entry = entry.map_err(|error| retention_error("read generation file", error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| retention_error("inspect generation file", error))?;
        if !file_type.is_file() || file_type.is_symlink() {
            return Ok(CandidateFileShape::Unknown);
        }
        names.insert(entry.file_name());
    }
    if names.len() == 1 && names.contains(std::ffi::OsStr::new(CORE_DATABASE_FILE)) {
        return Ok(CandidateFileShape::Unprovisioned);
    }
    if names.len() == 2
        && names.contains(std::ffi::OsStr::new(CORE_DATABASE_FILE))
        && names.contains(std::ffi::OsStr::new(CORE_LEASE_FILE))
    {
        return Ok(CandidateFileShape::Complete);
    }
    Ok(CandidateFileShape::Unknown)
}

fn acquisition_path(layout: &CorePublicationLayout) -> PathBuf {
    layout.root().join(CORE_ACQUISITION_FILE)
}

fn acquire_acquisition(
    layout: &CorePublicationLayout,
    kind: FileLockKind,
    nonblocking: bool,
) -> Result<Option<CoreAcquisitionLock>, StorageError> {
    let Some(file) = open_regular_lock(&acquisition_path(layout), false)? else {
        return Ok(None);
    };
    if nonblocking {
        return bounded_locks::try_acquire(&file, kind)
            .map(|held| held.then_some(CoreAcquisitionLock(file)))
            .map_err(|error| retention_error("try core acquisition lock", error));
    }
    acquire_with_deadline(
        &file,
        kind,
        LockDeadline::after(PUBLICATION_LOCK_WAIT),
        None,
    )
    .map_err(|error| retention_error("acquire core generation lock", error))?;
    Ok(Some(CoreAcquisitionLock(file)))
}

enum NamedLeaseTry {
    Unprovisioned,
    Contended,
    Held(File),
}

fn acquire_generation_read_lease(
    database: &Path,
) -> Result<Option<CoreGenerationLease>, StorageError> {
    let directory = database
        .parent()
        .ok_or_else(|| StorageError::Other("Core image has no generation directory".into()))?;
    let Some(file) = open_regular_lock(&directory.join(CORE_LEASE_FILE), false)? else {
        return Ok(None);
    };
    acquire_with_deadline(
        &file,
        FileLockKind::Shared,
        LockDeadline::after(PUBLICATION_LOCK_WAIT),
        None,
    )
    .map_err(|error| retention_error("acquire named core lease", error))?;
    Ok(Some(CoreGenerationLease(file)))
}

fn try_acquire_generation_exclusive(database: &Path) -> Result<NamedLeaseTry, StorageError> {
    let directory = database
        .parent()
        .ok_or_else(|| StorageError::Other("Core image has no generation directory".into()))?;
    let Some(file) = open_regular_lock(&directory.join(CORE_LEASE_FILE), false)? else {
        return Ok(NamedLeaseTry::Unprovisioned);
    };
    match bounded_locks::try_acquire(&file, FileLockKind::Exclusive) {
        Ok(true) => Ok(NamedLeaseTry::Held(file)),
        Ok(false) => Ok(NamedLeaseTry::Contended),
        Err(error) => Err(retention_error("try named core lease", error)),
    }
}

fn open_new_regular_lock(path: &Path) -> Result<File, StorageError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    no_follow(&mut options);
    let file = options
        .open(path)
        .map_err(|error| retention_error("create generation lease", error))?;
    require_opened_matches_path(&file, path, true)?;
    Ok(file)
}

fn open_regular_lock(path: &Path, create: bool) -> Result<Option<File>, StorageError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create);
    no_follow(&mut options);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(retention_error("open core retention lock", error)),
    };
    require_opened_matches_path(&file, path, true)?;
    Ok(Some(file))
}

fn open_direct_regular_file(path: &Path) -> Result<File, StorageError> {
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let file = options
        .open(path)
        .map_err(|error| retention_error("open owned core image", error))?;
    require_opened_matches_path(&file, path, true)?;
    Ok(file)
}

fn open_direct_directory(path: &Path) -> Result<File, StorageError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .map_err(|error| retention_error("open core generation directory", error))?;
    require_opened_matches_path(&file, path, false)?;
    Ok(file)
}

#[cfg(unix)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt as _;
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
}

#[cfg(windows)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

fn require_opened_matches_path(
    file: &File,
    path: &Path,
    regular_file: bool,
) -> Result<(), StorageError> {
    let direct = fs::symlink_metadata(path)
        .map_err(|error| retention_error("inspect core retention lock", error))?;
    let opened = file
        .metadata()
        .map_err(|error| retention_error("inspect opened core object", error))?;
    if (regular_file && (!direct.file_type().is_file() || !opened.is_file()))
        || (!regular_file && (!direct.file_type().is_dir() || !opened.is_dir()))
        || is_reparse_point(&direct)
        || is_reparse_point(&opened)
    {
        return Err(StorageError::Other(format!(
            "Core retention object is not direct or changed type: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    let same_identity = same_native_metadata(&opened, &direct);
    #[cfg(windows)]
    let same_identity = windows_opened_matches_path(file, path, regular_file)?;
    if !same_identity {
        return Err(StorageError::Other(format!(
            "Core retention object identity changed: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn same_native_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    (left.dev(), left.ino()) == (right.dev(), right.ino())
}

#[cfg(windows)]
fn windows_opened_matches_path(
    opened: &File,
    path: &Path,
    regular_file: bool,
) -> Result<bool, StorageError> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let flags = if regular_file {
        FILE_FLAG_OPEN_REPARSE_POINT
    } else {
        FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS
    };
    let by_name = OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .map_err(|error| retention_error("pin core object by path", error))?;
    let by_name_metadata = by_name
        .metadata()
        .map_err(|error| retention_error("inspect pinned core object by path", error))?;
    if is_reparse_point(&by_name_metadata)
        || (regular_file && !by_name_metadata.is_file())
        || (!regular_file && !by_name_metadata.is_dir())
    {
        return Ok(false);
    }
    let opened_id = windows_file_identity(opened)?;
    let by_name_id = windows_file_identity(&by_name)?;
    Ok(opened_id == by_name_id)
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> Result<(u32, u64), StorageError> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the handle remains open and the native call writes the complete
    // output structure before `assume_init` is reached.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(retention_error(
            "identify opened core object",
            io::Error::last_os_error(),
        ));
    }
    let information = unsafe { information.assume_init() };
    Ok((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

#[cfg(unix)]
fn is_reparse_point(_: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn require_direct_directory(path: &Path) -> Result<(), StorageError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| retention_error("inspect core directory", error))?;
    if !metadata.file_type().is_dir() || is_reparse_point(&metadata) {
        return Err(StorageError::Other(format!(
            "Core retention directory is not direct: {}",
            path.display()
        )));
    }
    Ok(())
}

fn retention_error(action: &str, error: impl std::fmt::Display) -> StorageError {
    StorageError::Other(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::core_publication::CoreGenerationIdentityV1;
    use tempfile::tempdir;

    fn published_lock_fixture() -> (tempfile::TempDir, PathBuf, CorePublicationLayout) {
        let cache = tempdir().expect("cache");
        let logical = cache.path().join(CORE_DATABASE_FILE);
        let layout = CorePublicationLayout::from_storage_path(&logical).expect("layout");
        let directory = layout
            .generation_directory("owned-one")
            .expect("generation");
        fs::create_dir_all(&directory).expect("owned generation directory");
        fs::write(directory.join(CORE_DATABASE_FILE), b"candidate").expect("core image");
        fs::write(directory.join(CORE_LEASE_FILE), b"").expect("lease marker");
        fs::write(layout.root().join(CORE_ACQUISITION_FILE), b"").expect("acquisition lock");
        layout
            .publish_pointer(
                CoreGenerationIdentityV1 {
                    generation_id: "owned-one".into(),
                    run_id: "run-one".into(),
                    logical_bytes: 9,
                    published_at_epoch_ms: 1,
                },
                None,
            )
            .expect("publish fixture pointer");
        (cache, logical, layout)
    }

    #[test]
    fn reader_acquisition_prevents_prune_between_pointer_and_named_lease() {
        let (_cache, _logical, layout) = published_lock_fixture();
        let challenge_layout = layout.clone();
        set_after_pointer_before_lease(move || {
            assert!(
                acquire_acquisition(&challenge_layout, FileLockKind::Exclusive, true)
                    .expect("try cleanup acquisition")
                    .is_none(),
                "pruning must not pass the reader's pointer-to-lease window"
            );
        });
        let pinned = pin_active_core(&layout)
            .expect("pin active")
            .expect("pointer");
        assert!(pinned.lease.is_some());
        assert_eq!(pinned.pointer.active.generation_id, "owned-one");
    }

    #[test]
    fn reset_exclusion_refuses_reader_acquisition_and_named_pins() {
        let (_cache, logical, layout) = published_lock_fixture();
        let during_acquisition = logical.clone();
        set_after_pointer_before_lease(move || {
            let error = CoreResetExclusion::acquire(&during_acquisition)
                .err()
                .expect("reset must refuse pointer-to-lease window");
            assert!(
                error
                    .to_string()
                    .contains("Core reader acquisition is active")
            );
        });
        let pinned = pin_active_core(&layout)
            .expect("pin")
            .expect("complete pointer");
        let error = CoreResetExclusion::acquire(&logical)
            .err()
            .expect("live named reader");
        assert!(error.to_string().contains("Core reader is active"));
        assert_eq!(fs::read(&pinned.path).unwrap(), b"candidate");
        drop(pinned);
        let guard = CoreResetExclusion::acquire(&logical).expect("retry after reader release");
        assert!(
            acquire_acquisition(&layout, FileLockKind::Shared, true)
                .unwrap()
                .is_none()
        );
        drop(guard);
        assert!(pin_active_core(&layout).unwrap().unwrap().lease.is_some());
    }

    #[test]
    fn reset_exclusion_refuses_incomplete_or_unknown_enumeration() {
        let (_cache, logical, layout) = published_lock_fixture();
        let other = layout.generation_directory("owned-two").unwrap();
        fs::create_dir(&other).unwrap();
        fs::write(other.join(CORE_LEASE_FILE), b"").unwrap();
        fs::write(other.join(CORE_DATABASE_FILE), b"other").unwrap();
        let before = fs::read(layout.publication_path()).unwrap();
        fail_enumeration_after(1);
        assert!(
            CoreResetExclusion::acquire(&logical)
                .err()
                .unwrap()
                .to_string()
                .contains("injected late")
        );
        assert_eq!(fs::read(layout.publication_path()).unwrap(), before);
        let unknown = layout.generations_root().join("unknown-file");
        fs::write(&unknown, b"not a generation directory").unwrap();
        assert!(CoreResetExclusion::acquire(&logical).is_err());
        assert_eq!(fs::read(layout.publication_path()).unwrap(), before);
        fs::remove_file(unknown).unwrap();
        drop(CoreResetExclusion::acquire(&logical).expect("complete enumeration retry"));
        assert!(pin_active_core(&layout).unwrap().unwrap().lease.is_some());
    }

    #[test]
    fn promotion_lock_defers_prune_before_pointer_publication() {
        let (_cache, logical, _layout) = published_lock_fixture();
        let _promotion = PromotionLock::acquire(&logical).expect("promotion in progress");
        let report = apply_core_retention(&logical, &|| false, |_, _, _, _, _| {
            panic!("candidate removal cannot run during promotion")
        })
        .expect("deferred cleanup report");
        assert!(report.pruning_suppressed);
        assert_eq!(report.reclaimed_images, 0);
    }

    #[test]
    fn late_enumeration_failure_removes_no_previously_seen_candidate() {
        let (_cache, logical, layout) = published_lock_fixture();
        for id in ["obsolete-one", "obsolete-two"] {
            let directory = layout.generation_directory(id).expect("generation");
            fs::create_dir_all(&directory).expect("obsolete directory");
            fs::write(directory.join(CORE_DATABASE_FILE), b"unchanged").expect("old image");
            fs::write(directory.join(CORE_LEASE_FILE), b"").expect("old lease");
        }
        // The directory has multiple entries. Fail after one has been read,
        // before candidate validation/removal can use a partial scan.
        fail_enumeration_after(1);
        let error = apply_core_retention(&logical, &|| false, |_, _, _, _, _| {
            panic!("an incomplete root scan cannot authorize any removal")
        })
        .expect_err("late enumeration failure must fail the pass");
        assert!(error.to_string().contains("injected late"));
        for id in ["obsolete-one", "obsolete-two"] {
            assert_eq!(
                fs::read(layout.generation_database_path(id).unwrap()).unwrap(),
                b"unchanged"
            );
        }
    }
}
