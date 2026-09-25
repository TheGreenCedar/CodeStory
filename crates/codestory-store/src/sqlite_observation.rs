//! Observation-only SQLite accounting and compact `VACUUM INTO` helpers.

use crate::StorageError;
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const ONE_MIB: u64 = 1024 * 1024;
const COMPACT_SAFETY_FLOOR_BYTES: u64 = 256 * ONE_MIB;

/// Read-only SQLite footprint for cache inventory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SqliteDatabaseObservation {
    pub path: String,
    pub page_size: u64,
    pub page_count: u64,
    pub freelist_count: u64,
    pub logical_bytes: u64,
    pub file_bytes: u64,
    pub wal_bytes: u64,
    pub shm_bytes: u64,
    pub auto_vacuum: i64,
}

/// Result of compacting one sealed database through `VACUUM INTO`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SqliteVacuumIntoStats {
    pub source_logical_bytes: u64,
    pub source_file_bytes: u64,
    pub source_freelist_count: u64,
    pub candidate_logical_bytes: u64,
    pub candidate_file_bytes: u64,
    pub candidate_freelist_count: u64,
    pub freelist_pages_reclaimed: u64,
    pub peak_space_required_bytes: u64,
    pub available_bytes: u64,
}

/// Peak free-space observation for compact rehydrate before any stage mutation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompactRehydratePeakSpace {
    pub stage_upper_bytes: u64,
    pub candidate_upper_bytes: u64,
    pub peak_space_required_bytes: u64,
    pub available_bytes: u64,
}

fn promotion_error(message: impl Into<String>) -> StorageError {
    StorageError::Other(message.into())
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .expect("sqlite path has a file name")
        .to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn sidecar_bytes(path: &Path) -> Result<Option<u64>, StorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(metadata.len())),
        Ok(_) => Err(promotion_error(format!(
            "SQLite logical observation unavailable: unsafe sidecar {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(promotion_error(format!(
            "SQLite logical observation unavailable: inspect sidecar {}: {error}",
            path.display()
        ))),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ObservationalSidecars {
    wal_bytes: u64,
    shm_bytes: u64,
}

fn checked_observational_sidecars(path: &Path) -> Result<ObservationalSidecars, StorageError> {
    let wal = sidecar_bytes(&sqlite_sidecar_path(path, "-wal"))?;
    let shm = sidecar_bytes(&sqlite_sidecar_path(path, "-shm"))?;
    let journal = sidecar_bytes(&sqlite_sidecar_path(path, "-journal"))?;
    // Even an empty journal is retained for the activating recovery owner:
    // filesystem metadata alone cannot prove that it is safe to ignore.
    if journal.is_some() {
        return Err(promotion_error(format!(
            "SQLite logical observation unavailable: rollback recovery is pending for {}",
            path.display()
        )));
    }
    if wal.is_some() != shm.is_some() {
        return Err(promotion_error(format!(
            "SQLite logical observation unavailable: incomplete WAL/SHM sidecar pair for {}",
            path.display()
        )));
    }
    if wal.unwrap_or(0) > 0 {
        return Err(promotion_error(format!(
            "SQLite logical observation unavailable: live WAL for {}",
            path.display()
        )));
    }
    if shm.unwrap_or(0) > 0 {
        return Err(promotion_error(format!(
            "SQLite logical observation unavailable: unsafe SHM without WAL content for {}",
            path.display()
        )));
    }
    Ok(ObservationalSidecars {
        wal_bytes: wal.unwrap_or(0),
        shm_bytes: shm.unwrap_or(0),
    })
}

fn open_observational_database(
    path: &Path,
) -> Result<(Connection, ObservationalSidecars), StorageError> {
    let sidecars = checked_observational_sidecars(path)?;
    #[cfg(test)]
    fail_if_observational_sqlite_open_forbidden();
    let connection = Connection::open_with_flags(
        crate::sqlite_path::observational_uri(path, true),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(StorageError::from)?;
    Ok((connection, sidecars))
}

#[cfg(test)]
thread_local! {
    static FORBID_OBSERVATIONAL_SQLITE_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn fail_if_observational_sqlite_open_forbidden() {
    FORBID_OBSERVATIONAL_SQLITE_OPEN.with(|flag| {
        assert!(!flag.get(), "unsafe observation reached SQLite open");
    });
}

fn pragma_u64(connection: &Connection, name: &str) -> Result<u64, StorageError> {
    let value: i64 = connection
        .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
        .map_err(StorageError::from)?;
    u64::try_from(value).map_err(|_| {
        promotion_error(format!(
            "SQLite reported an invalid negative {name}: {value}"
        ))
    })
}

/// Observe one SQLite database without mutating it or creating lock sidecars.
pub fn observe_sqlite_database(path: &Path) -> Result<SqliteDatabaseObservation, StorageError> {
    let (connection, sidecars_before) = open_observational_database(path)?;
    let page_size = pragma_u64(&connection, "page_size")?;
    let page_count = pragma_u64(&connection, "page_count")?;
    let freelist_count = pragma_u64(&connection, "freelist_count")?;
    let auto_vacuum: i64 = connection
        .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
        .map_err(StorageError::from)?;
    let logical_bytes = page_count.checked_mul(page_size).ok_or_else(|| {
        promotion_error(format!(
            "SQLite logical database bytes overflowed: page_count={page_count}, page_size={page_size}"
        ))
    })?;
    let file_bytes = fs::metadata(path)
        .map_err(|error| promotion_error(format!("inspect {}: {error}", path.display())))?
        .len();
    let sidecars_after = checked_observational_sidecars(path)?;
    if sidecars_after != sidecars_before {
        return Err(promotion_error(format!(
            "SQLite logical observation unavailable: sidecar state changed for {}",
            path.display()
        )));
    }
    Ok(SqliteDatabaseObservation {
        path: path.display().to_string(),
        page_size,
        page_count,
        freelist_count,
        logical_bytes,
        file_bytes,
        wal_bytes: sidecars_after.wal_bytes,
        shm_bytes: sidecars_after.shm_bytes,
        auto_vacuum,
    })
}

/// Upper bound for compact rehydrate temporary space.
pub fn compact_rehydrate_space_required(stage_upper_bytes: u64, candidate_upper_bytes: u64) -> u64 {
    let working = stage_upper_bytes.saturating_add(candidate_upper_bytes);
    let margin = COMPACT_SAFETY_FLOOR_BYTES.max(working / 10);
    working.saturating_add(margin)
}

/// Remaining free-space requirement after the stage copy already occupies disk.
pub fn compact_rehydrate_remaining_space_required(candidate_upper_bytes: u64) -> u64 {
    compact_rehydrate_space_required(0, candidate_upper_bytes)
}

/// Maximum acceptable on-disk size for a compact rehydrate candidate.
pub fn compact_candidate_size_limit(source_logical_bytes: u64) -> u64 {
    source_logical_bytes.saturating_add(ONE_MIB.max(source_logical_bytes / 20))
}

fn escape_sqlite_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn seal_database_for_vacuum(path: &Path) -> Result<(), StorageError> {
    let connection = Connection::open(path).map_err(StorageError::from)?;
    connection
        .pragma_update(None, "wal_checkpoint", "TRUNCATE")
        .map_err(StorageError::from)?;
    connection
        .execute_batch("PRAGMA optimize;")
        .map_err(StorageError::from)?;
    drop(connection);
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = sqlite_sidecar_path(path, suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(promotion_error(format!(
                    "remove sealed sidecar {}: {error}",
                    sidecar.display()
                )));
            }
        }
    }
    Ok(())
}

/// Upper bound of on-disk bytes for a database file plus live sidecars.
pub fn database_upper_bound(path: &Path) -> Result<u64, StorageError> {
    let observation = observe_sqlite_database(path)?;
    Ok(observation
        .file_bytes
        .saturating_add(observation.wal_bytes)
        .saturating_add(observation.shm_bytes)
        .max(observation.logical_bytes))
}

fn validate_compact_candidate(
    source: &SqliteDatabaseObservation,
    candidate_path: &Path,
) -> Result<SqliteVacuumIntoStats, StorageError> {
    let candidate = observe_sqlite_database(candidate_path)?;
    if candidate.freelist_count != 0 {
        return Err(promotion_error(format!(
            "compact candidate retained {} freelist pages",
            candidate.freelist_count
        )));
    }
    let size_limit = compact_candidate_size_limit(source.logical_bytes);
    if candidate.file_bytes > size_limit {
        return Err(promotion_error(format!(
            "compact candidate size {} exceeds limit {} for live bytes {}",
            candidate.file_bytes, size_limit, source.logical_bytes
        )));
    }
    let source_free_pages = source.freelist_count;
    let candidate_free_pages = candidate.freelist_count;
    let freelist_pages_reclaimed = source_free_pages.saturating_sub(candidate_free_pages);
    if source.page_count > 0 && source_free_pages * 100 / source.page_count >= 50 {
        let minimum_reclaim = source_free_pages * 95 / 100;
        if freelist_pages_reclaimed < minimum_reclaim {
            return Err(promotion_error(format!(
                "compact candidate reclaimed {freelist_pages_reclaimed} freelist pages but source retained {source_free_pages} free pages"
            )));
        }
    }
    Ok(SqliteVacuumIntoStats {
        source_logical_bytes: source.logical_bytes,
        source_file_bytes: source.file_bytes,
        source_freelist_count: source.freelist_count,
        candidate_logical_bytes: candidate.logical_bytes,
        candidate_file_bytes: candidate.file_bytes,
        candidate_freelist_count: candidate.freelist_count,
        freelist_pages_reclaimed,
        peak_space_required_bytes: 0,
        available_bytes: 0,
    })
}

/// Available bytes on the filesystem hosting `path`.
pub fn available_filesystem_bytes(path: &Path) -> Result<u64, StorageError> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(bytes) = test_available_filesystem_bytes_override() {
        return Ok(bytes);
    }
    available_filesystem_bytes_platform(path)
}

#[cfg(unix)]
fn available_filesystem_bytes_platform(path: &Path) -> Result<u64, StorageError> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        promotion_error(format!(
            "path contains an interior nul byte: {}",
            path.display()
        ))
    })?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if result != 0 {
        return Err(promotion_error(format!(
            "statvfs failed for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    let stat = unsafe { stat.assume_init() };
    // libc field widths differ by target (`fsblkcnt_t` is u32 on Darwin and
    // `c_ulong` on Linux). Keep an explicit widening conversion for both.
    #[allow(clippy::useless_conversion)]
    let block_size = u64::from(stat.f_frsize);
    #[allow(clippy::useless_conversion)]
    let available = u64::from(stat.f_bavail);
    block_size
        .checked_mul(available)
        .ok_or_else(|| promotion_error("available filesystem bytes overflowed".to_string()))
}

#[cfg(windows)]
fn available_filesystem_bytes_platform(path: &Path) -> Result<u64, StorageError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut root = path
        .ancestors()
        .find(|ancestor| ancestor.is_dir())
        .unwrap_or_else(|| Path::new("."));
    if root.as_os_str().is_empty() {
        root = Path::new(".");
    }
    let wide: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut available = 0_u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available as *mut u64,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(promotion_error(format!(
            "GetDiskFreeSpaceExW failed for {}: {}",
            root.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(available)
}

#[cfg(not(any(unix, windows)))]
fn available_filesystem_bytes_platform(_path: &Path) -> Result<u64, StorageError> {
    Err(promotion_error(
        "filesystem free-space observation is unsupported on this platform".to_string(),
    ))
}

/// Observe peak stage+candidate space for compact rehydrate without mutating.
pub fn measure_compact_rehydrate_peak_space(
    source: &Path,
    destination_parent: &Path,
) -> Result<CompactRehydratePeakSpace, StorageError> {
    let source_observation = observe_sqlite_database(source)?;
    let stage_upper_bytes = database_upper_bound(source)?;
    let candidate_upper_bytes = source_observation.logical_bytes;
    let peak_space_required_bytes =
        compact_rehydrate_space_required(stage_upper_bytes, candidate_upper_bytes);
    let available_bytes = available_filesystem_bytes(destination_parent)?;
    Ok(CompactRehydratePeakSpace {
        stage_upper_bytes,
        candidate_upper_bytes,
        peak_space_required_bytes,
        available_bytes,
    })
}

/// Fail closed when peak stage+candidate space is unavailable, before mutation.
pub fn ensure_compact_rehydrate_peak_space(
    source: &Path,
    destination_parent: &Path,
) -> Result<CompactRehydratePeakSpace, StorageError> {
    let measured = measure_compact_rehydrate_peak_space(source, destination_parent)?;
    if measured.available_bytes < measured.peak_space_required_bytes {
        return Err(insufficient_space_error(
            measured.peak_space_required_bytes,
            measured.available_bytes,
        ));
    }
    Ok(measured)
}

fn insufficient_space_error(required: u64, available: u64) -> StorageError {
    promotion_error(format!(
        "insufficient space for compact rehydrate: need at least {required} bytes, available {available} bytes"
    ))
}

/// True when `error` is the compact-rehydrate free-space preflight failure.
pub fn is_insufficient_compact_rehydrate_space(error: &StorageError) -> bool {
    match error {
        StorageError::Other(message) => {
            message.starts_with("insufficient space for compact rehydrate:")
        }
        _ => false,
    }
}

/// Seal `source`, preflight remaining candidate space, and write a compact database.
///
/// The source/stage is assumed to already occupy disk. Remaining free-space
/// accounting therefore covers only the compact candidate plus safety margin.
pub fn vacuum_into_database(
    source: &Path,
    destination: &Path,
) -> Result<SqliteVacuumIntoStats, StorageError> {
    if destination.exists() {
        return Err(promotion_error(format!(
            "compact destination already exists: {}",
            destination.display()
        )));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            promotion_error(format!(
                "create compact destination parent {}: {error}",
                parent.display()
            ))
        })?;
    }
    let source_observation = observe_sqlite_database(source)?;
    let candidate_upper = source_observation.logical_bytes;
    let peak_space_required_bytes = compact_rehydrate_remaining_space_required(candidate_upper);
    let available_bytes =
        available_filesystem_bytes(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    if available_bytes < peak_space_required_bytes {
        return Err(insufficient_space_error(
            peak_space_required_bytes,
            available_bytes,
        ));
    }
    seal_database_for_vacuum(source)?;
    let connection = Connection::open(source).map_err(StorageError::from)?;
    let sql = format!("VACUUM INTO '{}'", escape_sqlite_path(destination));
    connection.execute_batch(&sql).map_err(StorageError::from)?;
    drop(connection);
    let mut stats = validate_compact_candidate(&source_observation, destination)?;
    stats.peak_space_required_bytes = peak_space_required_bytes;
    stats.available_bytes = available_bytes;
    Ok(stats)
}

#[cfg(any(test, feature = "test-support"))]
mod available_override {
    use std::cell::Cell;

    thread_local! {
        static AVAILABLE_BYTES_OVERRIDE: Cell<Option<u64>> = const { Cell::new(None) };
    }

    pub(super) fn test_available_filesystem_bytes_override() -> Option<u64> {
        AVAILABLE_BYTES_OVERRIDE.with(Cell::get)
    }

    /// Force `available_filesystem_bytes` for the duration of `f`.
    pub fn with_available_filesystem_bytes_override<R>(bytes: u64, f: impl FnOnce() -> R) -> R {
        AVAILABLE_BYTES_OVERRIDE.with(|cell| {
            let previous = cell.replace(Some(bytes));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            cell.set(previous);
            match result {
                Ok(value) => value,
                Err(payload) => std::panic::resume_unwind(payload),
            }
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
use available_override::test_available_filesystem_bytes_override;
#[cfg(any(test, feature = "test-support"))]
pub use available_override::with_available_filesystem_bytes_override;

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn forbid_observational_sqlite_open<R>(f: impl FnOnce() -> R) -> R {
        FORBID_OBSERVATIONAL_SQLITE_OPEN.with(|flag| {
            let previous = flag.replace(true);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            flag.set(previous);
            match result {
                Ok(value) => value,
                Err(payload) => std::panic::resume_unwind(payload),
            }
        })
    }

    fn sqlite_tree_bytes(path: &Path) -> Vec<(String, Vec<u8>)> {
        ["", "-wal", "-shm", "-journal"]
            .into_iter()
            .filter_map(|suffix| {
                let file = if suffix.is_empty() {
                    path.to_path_buf()
                } else {
                    sqlite_sidecar_path(path, suffix)
                };
                fs::read(&file)
                    .ok()
                    .map(|bytes| (suffix.to_string(), bytes))
            })
            .collect()
    }

    fn create_database(path: &Path, pages: u64) {
        let connection = Connection::open(path).expect("open database");
        connection
            .pragma_update(None, "page_size", 1024)
            .expect("set page size");
        connection
            .execute_batch(
                "CREATE TABLE payload(value BLOB);
                 INSERT INTO payload(value) VALUES (zeroblob(1024));",
            )
            .expect("seed database");
        for _ in 1..pages {
            connection
                .execute("INSERT INTO payload(value) VALUES (zeroblob(1024))", [])
                .expect("grow database");
        }
        connection
            .execute("DELETE FROM payload WHERE rowid = 1", [])
            .expect("create freelist");
        connection
            .pragma_update(None, "wal_checkpoint", "TRUNCATE")
            .expect("checkpoint wal");
        drop(connection);
    }

    #[test]
    fn observe_sqlite_database_reports_freelist_without_sidecars() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("observe.sqlite3");
        create_database(&path, 4);
        let observation = observe_sqlite_database(&path).expect("observe database");
        assert_eq!(observation.page_size, 1024);
        assert!(observation.freelist_count >= 1);
        assert_eq!(observation.wal_bytes, 0);
        assert_eq!(observation.shm_bytes, 0);
    }

    #[test]
    fn live_wal_observation_refuses_sqlite_open_without_changing_files() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("live.sqlite3");
        create_database(&path, 4);
        let writer = Connection::open(&path).expect("open live writer");
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 INSERT INTO payload(value) VALUES (zeroblob(4096));",
            )
            .expect("hold committed live WAL");
        let wal = sqlite_sidecar_path(&path, "-wal");
        let shm = sqlite_sidecar_path(&path, "-shm");
        assert!(fs::metadata(&wal).expect("live WAL").len() > 0);
        assert!(shm.is_file(), "writer supplies complete WAL/SHM pair");
        let before = sqlite_tree_bytes(&path);

        let error = forbid_observational_sqlite_open(|| observe_sqlite_database(&path))
            .expect_err("live WAL has unavailable logical observation");
        assert!(error.to_string().contains("live WAL"), "{error}");
        assert_eq!(sqlite_tree_bytes(&path), before);
        drop(writer);
    }

    #[test]
    fn incomplete_or_unsafe_sidecars_refuse_sqlite_open() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("incomplete.sqlite3");
        create_database(&path, 1);
        let wal = sqlite_sidecar_path(&path, "-wal");
        let shm = sqlite_sidecar_path(&path, "-shm");

        for lone in [&wal, &shm] {
            fs::write(lone, b"").expect("install empty lone sidecar");
            let before = sqlite_tree_bytes(&path);
            let error = forbid_observational_sqlite_open(|| observe_sqlite_database(&path))
                .expect_err("incomplete sidecar pair must be unavailable");
            assert!(error.to_string().contains("incomplete WAL/SHM"), "{error}");
            assert_eq!(sqlite_tree_bytes(&path), before);
            fs::remove_file(lone).expect("remove lone sidecar");
        }

        fs::create_dir(&wal).expect("install nonregular WAL entry");
        let error = forbid_observational_sqlite_open(|| observe_sqlite_database(&path))
            .expect_err("nonregular sidecar must be unavailable");
        assert!(error.to_string().contains("unsafe"), "{error}");
        assert!(wal.is_dir(), "unsafe sidecar survives");
    }

    #[test]
    fn empty_complete_wal_pair_remains_immutable_and_unchanged() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("empty-pair.sqlite3");
        create_database(&path, 1);
        fs::write(sqlite_sidecar_path(&path, "-wal"), b"").expect("empty WAL");
        fs::write(sqlite_sidecar_path(&path, "-shm"), b"").expect("empty SHM");
        let before = sqlite_tree_bytes(&path);

        let observation = observe_sqlite_database(&path).expect("empty pair is harmless");
        assert!(observation.logical_bytes > 0);
        assert_eq!(observation.wal_bytes, 0);
        assert_eq!(observation.shm_bytes, 0);
        assert_eq!(sqlite_tree_bytes(&path), before);
    }

    #[test]
    fn abandoned_rollback_writer_child() {
        let Ok(path) = std::env::var("CODESTORY_C1_HOT_JOURNAL_CHILD_DB") else {
            return;
        };
        let connection = Connection::open(path).expect("open rollback writer child");
        connection
            .execute_batch(
                "PRAGMA cache_size=1;
                 PRAGMA cache_spill=ON;
                 BEGIN IMMEDIATE;
                 UPDATE payload SET value=zeroblob(3000);",
            )
            .expect("spill uncommitted pages and rollback journal");
        std::process::exit(0);
    }

    #[test]
    fn interrupted_rollback_journal_refuses_immutable_observation() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("interrupted.sqlite3");
        let connection = Connection::open(&path).expect("create rollback database");
        connection
            .execute_batch(
                "PRAGMA journal_mode=DELETE;
                 PRAGMA synchronous=FULL;
                 CREATE TABLE payload(value BLOB);
                 WITH RECURSIVE n(x) AS (
                   SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1000
                 ) INSERT INTO payload(value) SELECT randomblob(3000) FROM n;",
            )
            .expect("seed rollback database");
        drop(connection);
        let committed = fs::read(&path).expect("committed database bytes");
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "sqlite_observation::tests::abandoned_rollback_writer_child",
            ])
            .env("CODESTORY_C1_HOT_JOURNAL_CHILD_DB", &path)
            .status()
            .expect("run interrupted writer child");
        assert!(status.success(), "writer child reached uncommitted spill");
        let journal = sqlite_sidecar_path(&path, "-journal");
        let journal_bytes = fs::read(&journal).expect("rollback journal left by exited writer");
        assert!(journal_bytes.len() > 512, "journal has rollback pages");
        assert_eq!(
            &journal_bytes[..8],
            &[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7],
            "journal retains SQLite's hot-journal header"
        );
        assert_ne!(fs::read(&path).expect("unrecovered image"), committed);
        let before = sqlite_tree_bytes(&path);

        let error = forbid_observational_sqlite_open(|| observe_sqlite_database(&path))
            .expect_err("pending rollback recovery must be unavailable");
        assert!(error.to_string().contains("rollback"), "{error}");
        assert_eq!(sqlite_tree_bytes(&path), before);
    }

    #[test]
    fn vacuum_into_database_produces_zero_freelist_candidate() {
        let root = tempdir().expect("tempdir");
        let source = root.path().join("source.sqlite3");
        let destination = root.path().join("compact.sqlite3");
        create_database(&source, 8);
        let stats = vacuum_into_database(&source, &destination).expect("vacuum into");
        assert_eq!(stats.candidate_freelist_count, 0);
        assert!(
            stats.candidate_file_bytes <= compact_candidate_size_limit(stats.source_logical_bytes)
        );
        assert!(destination.is_file());
        assert_eq!(
            stats.peak_space_required_bytes,
            compact_rehydrate_remaining_space_required(stats.source_logical_bytes)
        );
    }

    #[test]
    fn compact_rehydrate_space_required_applies_floor_and_percent_margin() {
        assert_eq!(
            compact_rehydrate_space_required(0, 0),
            COMPACT_SAFETY_FLOOR_BYTES
        );
        assert_eq!(
            compact_rehydrate_space_required(ONE_MIB, ONE_MIB),
            (2 * ONE_MIB) + COMPACT_SAFETY_FLOOR_BYTES
        );
        assert_eq!(
            compact_rehydrate_remaining_space_required(ONE_MIB),
            ONE_MIB + COMPACT_SAFETY_FLOOR_BYTES
        );
    }

    #[test]
    fn ensure_compact_rehydrate_peak_space_rejects_before_destination_exists() {
        let root = tempdir().expect("tempdir");
        let source = root.path().join("source.sqlite3");
        let destination_parent = root.path().join("dest");
        fs::create_dir_all(&destination_parent).expect("create dest parent");
        create_database(&source, 4);
        let measured =
            measure_compact_rehydrate_peak_space(&source, &destination_parent).expect("measure");
        assert!(measured.peak_space_required_bytes >= measured.stage_upper_bytes);
        let error = with_available_filesystem_bytes_override(0, || {
            ensure_compact_rehydrate_peak_space(&source, &destination_parent)
        })
        .expect_err("insufficient space");
        assert!(is_insufficient_compact_rehydrate_space(&error));
        let entries: Vec<_> = fs::read_dir(&destination_parent)
            .expect("read dest")
            .collect();
        assert!(
            entries.is_empty(),
            "preflight must not create destination files"
        );
    }

    #[test]
    fn vacuum_into_remaining_space_does_not_require_stage_again() {
        let root = tempdir().expect("tempdir");
        let source = root.path().join("source.sqlite3");
        let destination = root.path().join("compact.sqlite3");
        create_database(&source, 8);
        let stage_upper = database_upper_bound(&source).expect("stage upper");
        let observation = observe_sqlite_database(&source).expect("observe");
        let full_peak = compact_rehydrate_space_required(stage_upper, observation.logical_bytes);
        let remaining = compact_rehydrate_remaining_space_required(observation.logical_bytes);
        assert!(remaining < full_peak);
        let stats = with_available_filesystem_bytes_override(remaining, || {
            vacuum_into_database(&source, &destination)
        })
        .expect("vacuum with remaining-only budget");
        assert_eq!(stats.peak_space_required_bytes, remaining);
        assert!(destination.is_file());
    }
}
