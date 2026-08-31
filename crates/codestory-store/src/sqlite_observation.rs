//! Observation-only SQLite accounting and compact `VACUUM INTO` helpers.

use crate::StorageError;
use rusqlite::{Connection, OpenFlags};
use std::fs;
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

fn sidecar_bytes(path: &Path) -> u64 {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_file())
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn open_observational_database(path: &Path) -> Result<Connection, StorageError> {
    let wal_path = sqlite_sidecar_path(path, "-wal");
    let has_live_wal = fs::metadata(&wal_path).is_ok_and(|metadata| metadata.len() > 0);
    if has_live_wal {
        Connection::open_with_flags(
            crate::sqlite_path::open_path(path),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(StorageError::from)
    } else {
        Connection::open_with_flags(
            crate::sqlite_path::observational_uri(path, true),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(StorageError::from)
    }
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
    let connection = open_observational_database(path)?;
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
    Ok(SqliteDatabaseObservation {
        path: path.display().to_string(),
        page_size,
        page_count,
        freelist_count,
        logical_bytes,
        file_bytes,
        wal_bytes: sidecar_bytes(&sqlite_sidecar_path(path, "-wal")),
        shm_bytes: sidecar_bytes(&sqlite_sidecar_path(path, "-shm")),
        auto_vacuum,
    })
}

/// Upper bound for compact rehydrate temporary space.
pub fn compact_rehydrate_space_required(stage_upper_bytes: u64, candidate_upper_bytes: u64) -> u64 {
    let working = stage_upper_bytes.saturating_add(candidate_upper_bytes);
    let margin = COMPACT_SAFETY_FLOOR_BYTES.max(working / 10);
    working.saturating_add(margin)
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

fn database_upper_bound(path: &Path) -> Result<u64, StorageError> {
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
    let block_size = u64::from(stat.f_frsize);
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

/// Seal `source`, preflight space, and write a standalone compact database at `destination`.
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
    let stage_upper = database_upper_bound(source)?;
    let candidate_upper = source_observation.logical_bytes;
    let peak_space_required_bytes = compact_rehydrate_space_required(stage_upper, candidate_upper);
    let available_bytes =
        available_filesystem_bytes(destination.parent().unwrap_or_else(|| Path::new(".")))?;
    if available_bytes < peak_space_required_bytes {
        return Err(promotion_error(format!(
            "insufficient space for compact rehydrate: need at least {peak_space_required_bytes} bytes, available {available_bytes} bytes"
        )));
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
    fn observe_sqlite_database_reports_freelist_and_sidecars() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("observe.sqlite3");
        create_database(&path, 4);
        fs::write(sqlite_sidecar_path(&path, "-wal"), b"wal").expect("write wal");
        let observation = observe_sqlite_database(&path).expect("observe database");
        assert_eq!(observation.page_size, 1024);
        assert!(observation.freelist_count >= 1);
        assert_eq!(observation.wal_bytes, 3);
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
    }
}
