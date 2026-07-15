use codestory_contracts::graph::{
    AccessKind, Bookmark, BookmarkCategory, CallableProjectionState, Edge, EdgeKind,
    EnumConversionError, Node, NodeId, NodeKind, Occurrence, OccurrenceKind, ResolutionCertainty,
    TrailCallerScope, TrailConfig, TrailDirection, TrailMode, TrailResult,
};
use fs4::fs_std::FileExt;
use parking_lot::RwLock;
use rusqlite::{
    Connection, MAIN_DB, OpenFlags, OptionalExtension, Result, Row, params, params_from_iter,
    types::Value,
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

mod bookmarks;
mod helpers;
mod retrieval_manifest;
mod row_mapping;
mod schema;
mod trail;

use helpers::{
    decode_embedding_blob, deserialize_candidate_targets, encode_embedding_blob,
    numbered_placeholders, question_placeholders, serialize_candidate_targets,
};

const SCHEMA_VERSION: u32 = 22;
// Reserved outside the sequential migration range so a future real schema version cannot
// accidentally be treated as an interrupted run from this release.
const INCOMPLETE_INCREMENTAL_SCHEMA_VERSION: u32 = 0x4353_0001;
/// Current SQLite schema version expected by `Store`.
pub const CURRENT_SCHEMA_VERSION: u32 = SCHEMA_VERSION;
const GROUNDING_SNAPSHOT_VERSION: i64 = 1;
const GROUNDING_SNAPSHOT_STATE_DIRTY: i64 = 0;
const GROUNDING_SNAPSHOT_STATE_BUILDING: i64 = 1;
const GROUNDING_SNAPSHOT_STATE_READY: i64 = 2;
const RELATED_NODE_SUBQUERY: &str = "SELECT id FROM node WHERE id = ?1 OR file_node_id = ?1";
const CALLER_CLEANUP_IDS_TABLE: &str = "caller_cleanup_ids";
const RELATED_NODE_IDS_TABLE: &str = "related_node_ids";
const EDGE_SELECT_BASE: &str = "SELECT e.id, e.source_node_id, e.target_node_id, e.kind, e.file_node_id, e.line, e.resolved_source_node_id, e.resolved_target_node_id, e.confidence, e.callsite_identity, e.certainty, e.candidate_target_node_ids, t.serialized_name, f.serialized_name
                 FROM edge e
                 JOIN node t ON t.id = e.target_node_id
                 LEFT JOIN node f ON f.id = e.file_node_id";
const EDGE_NODE_LOOKUP_BATCH_SIZE: usize = 200;
const NODE_LOOKUP_BATCH_SIZE: usize = 200;
const OCCURRENCE_LOOKUP_BATCH_SIZE: usize = 200;
#[cfg(test)]
const PROMOTION_ABORT_SENTINEL_ENV: &str = "CODESTORY_TEST_PROMOTION_ABORT_SENTINEL";
#[cfg(test)]
const PROMOTION_ABORT_SENTINEL: &[u8] = b"after-live-restore-step\n";
const PROMOTION_JOURNAL_VERSION: u32 = 1;

fn clamp_i64_to_u32(value: i64) -> u32 {
    if value <= 0 {
        0
    } else if value > u32::MAX as i64 {
        u32::MAX
    } else {
        value as u32
    }
}

fn uniform_optional_string(
    min_value: Option<String>,
    max_value: Option<String>,
) -> (Option<String>, bool) {
    match (min_value, max_value) {
        (Some(min_value), Some(max_value)) if min_value == max_value => (Some(min_value), false),
        (Some(_), Some(_)) => (None, true),
        (Some(value), None) | (None, Some(value)) => (Some(value), false),
        (None, None) => (None, false),
    }
}

fn uniform_optional_string_with_count(
    row_count: i64,
    value_count: i64,
    min_value: Option<String>,
    max_value: Option<String>,
) -> (Option<String>, bool) {
    if row_count <= 0 {
        return (None, false);
    }
    if value_count != row_count {
        let value = if value_count == 0 || min_value != max_value {
            None
        } else {
            min_value
        };
        return (value, true);
    }
    uniform_optional_string(min_value, max_value)
}

fn uniform_optional_u32(min_value: Option<i64>, max_value: Option<i64>) -> (Option<u32>, bool) {
    match (min_value, max_value) {
        (Some(min_value), Some(max_value)) if min_value == max_value => {
            (Some(clamp_i64_to_u32(min_value)), false)
        }
        (Some(_), Some(_)) => (None, true),
        (Some(value), None) | (None, Some(value)) => (Some(clamp_i64_to_u32(value)), false),
        (None, None) => (None, false),
    }
}

fn uniform_optional_u32_with_count(
    row_count: i64,
    value_count: i64,
    min_value: Option<i64>,
    max_value: Option<i64>,
) -> (Option<u32>, bool) {
    if row_count <= 0 {
        return (None, false);
    }
    if value_count != row_count {
        let value = if value_count == 0 || min_value != max_value {
            None
        } else {
            min_value.map(clamp_i64_to_u32)
        };
        return (value, true);
    }
    uniform_optional_u32(min_value, max_value)
}

fn current_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn compare_grounding_file_summaries(
    left: &GroundingFileSummary,
    right: &GroundingFileSummary,
) -> Ordering {
    left.best_node_rank
        .cmp(&right.best_node_rank)
        .then(right.symbol_count.cmp(&left.symbol_count))
        .then_with(|| left.file.path.cmp(&right.file.path))
}

fn sqlite_sidecar_paths(path: &Path) -> [PathBuf; 3] {
    [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
}

fn cleanup_sqlite_sidecars(path: &Path) -> Result<(), StorageError> {
    for candidate in sqlite_sidecar_paths(path) {
        if candidate.exists() {
            match fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(StorageError::Other(format!(
                        "Failed to remove SQLite artifact {}: {err}",
                        candidate.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

struct PromotionLock {
    file: File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PromotionJournal {
    version: u32,
    previous: Option<IndexPublicationRecord>,
    candidate: IndexPublicationRecord,
}

fn promotion_lock_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.promotion.lock", path.display()))
}

fn promotion_prepared_journal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.promotion.prepared.json", path.display()))
}

fn promotion_committed_journal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.promotion.committed.json", path.display()))
}

#[cfg(test)]
fn promotion_cleanup_failure_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.promotion.cleanup-blocked", path.display()))
}

fn promotion_artifacts_exist(path: &Path) -> bool {
    path.with_extension("sqlite.backup").exists()
        || promotion_prepared_journal_path(path).exists()
        || promotion_committed_journal_path(path).exists()
}

fn promotion_error(message: impl Into<String>) -> StorageError {
    StorageError::Other(message.into())
}

fn promotion_path_error(action: &str, path: &Path, error: impl std::fmt::Display) -> StorageError {
    promotion_error(format!("Failed to {action} {}: {error}", path.display()))
}

fn has_incomplete_incremental_marker(conn: &Connection) -> Result<bool, StorageError> {
    let table_exists: i64 = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'incomplete_index_run'
        )",
        [],
        |row| row.get(0),
    )?;
    if table_exists == 0 {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM incomplete_index_run WHERE id = 1)",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn inspect_promotion_database(path: &Path) -> Result<Option<(Connection, u32)>, StorageError> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let _ = conn.busy_timeout(Duration::from_millis(2_500));
    let quick_check: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        return Err(promotion_error(format!(
            "SQLite promotion artifact {} failed quick_check: {quick_check}",
            path.display()
        )));
    }
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let schema_version = version.max(0) as u32;
    Ok(Some((conn, schema_version)))
}

fn read_complete_promotion_database_identity(
    path: &Path,
) -> Result<Option<IndexPublicationRecord>, StorageError> {
    let Some((conn, schema_version)) = inspect_promotion_database(path)? else {
        return Ok(None);
    };
    if schema_version == 0 {
        return Ok(None);
    }
    if schema_version != SCHEMA_VERSION {
        return Err(promotion_error(format!(
            "SQLite promotion artifact {} has schema version {schema_version}, expected {SCHEMA_VERSION}",
            path.display()
        )));
    }
    read_complete_index_publication(&conn)
}

fn read_recovery_database_identity(
    path: &Path,
) -> Result<Option<IndexPublicationRecord>, StorageError> {
    let Some((conn, schema_version)) = inspect_promotion_database(path)? else {
        return Ok(None);
    };
    match schema_version {
        0 => Ok(None),
        SCHEMA_VERSION => read_complete_index_publication(&conn),
        INCOMPLETE_INCREMENTAL_SCHEMA_VERSION if has_incomplete_incremental_marker(&conn)? => {
            read_index_publication(&conn)
        }
        INCOMPLETE_INCREMENTAL_SCHEMA_VERSION => Err(promotion_error(format!(
            "SQLite recovery artifact {} uses the incomplete schema sentinel without its marker",
            path.display()
        ))),
        _ => Err(promotion_error(format!(
            "SQLite recovery artifact {} has unsupported schema version {schema_version}",
            path.display()
        ))),
    }
}

fn require_complete_promotion_database_identity(
    path: &Path,
    role: &str,
) -> Result<IndexPublicationRecord, StorageError> {
    read_complete_promotion_database_identity(path)?.ok_or_else(|| {
        promotion_error(format!(
            "{role} {} has no complete publication identity",
            path.display()
        ))
    })
}

fn require_recovery_database_identity(
    path: &Path,
    role: &str,
) -> Result<IndexPublicationRecord, StorageError> {
    read_recovery_database_identity(path)?.ok_or_else(|| {
        promotion_error(format!(
            "{role} {} has no complete publication identity",
            path.display()
        ))
    })
}

fn read_promotion_journal(path: &Path) -> Result<PromotionJournal, StorageError> {
    let bytes = fs::read(path).map_err(|error| promotion_path_error("read", path, error))?;
    let journal: PromotionJournal = serde_json::from_slice(&bytes)
        .map_err(|error| promotion_path_error("parse", path, error))?;
    if journal.version != PROMOTION_JOURNAL_VERSION {
        return Err(promotion_error(format!(
            "Unsupported promotion journal {}: version={}",
            path.display(),
            journal.version
        )));
    }
    Ok(journal)
}

fn sync_promotion_parent(path: &Path) -> Result<(), StorageError> {
    #[cfg(not(windows))]
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| promotion_path_error("sync directory", parent, error))?;
    }
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

fn write_promotion_journal(path: &Path, journal: &PromotionJournal) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec(journal)
        .map_err(|error| promotion_path_error("serialize", path, error))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| promotion_path_error("create", path, error))?;
    let write_result = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(path);
        let _ = sync_promotion_parent(path);
        return Err(promotion_path_error("persist", path, error));
    }
    if let Err(error) = sync_promotion_parent(path) {
        let _ = fs::remove_file(path);
        let _ = sync_promotion_parent(path);
        return Err(error);
    }
    Ok(())
}

fn commit_promotion_journal(
    prepared_path: &Path,
    committed_path: &Path,
) -> Result<(), StorageError> {
    if committed_path.exists() {
        return Err(promotion_error(format!(
            "Cannot commit promotion while prior journal {} remains",
            committed_path.display()
        )));
    }
    fs::rename(prepared_path, committed_path)
        .map_err(|error| promotion_path_error("commit journal as", committed_path, error))?;
    sync_promotion_parent(committed_path)
}

fn remove_promotion_file(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Ok(()) => sync_promotion_parent(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(promotion_path_error("remove", path, error)),
    }
}

impl PromotionLock {
    fn open(path: &Path) -> Result<File, StorageError> {
        let lock_path = promotion_lock_path(path);
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                StorageError::Other(format!(
                    "Failed to create promotion lock directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                StorageError::Other(format!(
                    "Failed to open promotion lock {}: {error}",
                    lock_path.display()
                ))
            })
    }

    fn acquire(path: &Path) -> Result<Self, StorageError> {
        let file = Self::open(path)?;
        FileExt::lock_exclusive(&file).map_err(|error| {
            StorageError::Other(format!(
                "Failed to acquire promotion lock for {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self { file })
    }

    fn try_acquire(path: &Path) -> Result<Option<Self>, StorageError> {
        let file = Self::open(path)?;
        FileExt::try_lock_exclusive(&file)
            .map(|locked| locked.then_some(Self { file }))
            .map_err(|error| {
                StorageError::Other(format!(
                    "Failed to inspect promotion lock for {}: {error}",
                    path.display()
                ))
            })
    }
}

impl Drop for PromotionLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn recover_interrupted_promotion(path: &Path) -> Result<(), StorageError> {
    if !promotion_artifacts_exist(path) {
        return Ok(());
    }
    let Some(_lock) = PromotionLock::try_acquire(path)? else {
        // A healthy promoter owns the lock. SQLite readers may wait for that
        // transaction, but must never interpret its backup as crash evidence.
        return Ok(());
    };
    recover_interrupted_promotion_locked(path)
}

fn recover_interrupted_promotion_locked(path: &Path) -> Result<(), StorageError> {
    let backup_path = path.with_extension("sqlite.backup");
    let prepared_path = promotion_prepared_journal_path(path);
    let committed_path = promotion_committed_journal_path(path);

    if committed_path.exists() && prepared_path.exists() {
        return Err(promotion_error(format!(
            "Promotion has both prepared and committed journals for {}",
            path.display()
        )));
    }

    if committed_path.exists() {
        let committed = read_promotion_journal(&committed_path)?;
        let live_identity =
            require_complete_promotion_database_identity(path, "Committed live database")?;
        if live_identity != committed.candidate {
            return Err(promotion_error(format!(
                "Committed promotion identity does not match live database {}",
                path.display()
            )));
        }
        if let Err(error) = cleanup_committed_promotion_artifacts(path) {
            tracing::warn!(
                live_path = %path.display(),
                error = %error,
                "committed promotion retained recovery artifacts"
            );
        }
        return Ok(());
    }

    if prepared_path.exists() {
        let prepared = read_promotion_journal(&prepared_path)?;
        return rollback_prepared_promotion(path, &prepared);
    }

    if backup_path.exists() {
        return recover_legacy_promotion_backup(path, &backup_path);
    }
    Ok(())
}

fn restore_promotion_database(source_path: &Path, live_path: &Path) -> Result<(), StorageError> {
    let mut live = Connection::open(live_path)?;
    let _ = live.busy_timeout(Duration::from_millis(2_500));
    live.restore(MAIN_DB, source_path, None::<fn(rusqlite::backup::Progress)>)?;
    Ok(())
}

fn rollback_prepared_promotion(
    live_path: &Path,
    prepared: &PromotionJournal,
) -> Result<(), StorageError> {
    let backup_path = live_path.with_extension("sqlite.backup");
    let prepared_path = promotion_prepared_journal_path(live_path);
    let live_identity = read_recovery_database_identity(live_path)?;
    if live_identity
        .as_ref()
        .is_some_and(|live| live != &prepared.candidate && Some(live) != prepared.previous.as_ref())
    {
        return Err(promotion_error(format!(
            "Prepared promotion for {} found an unrelated live publication",
            live_path.display()
        )));
    }

    match prepared.previous.as_ref() {
        Some(expected_previous) => {
            let backup_identity =
                require_recovery_database_identity(&backup_path, "Prepared recovery backup")?;
            if &backup_identity != expected_previous {
                return Err(promotion_error(format!(
                    "Prepared promotion backup identity does not match {}",
                    live_path.display()
                )));
            }
            if live_identity.as_ref() != Some(expected_previous) {
                restore_promotion_database(&backup_path, live_path)?;
            }
            let restored = require_recovery_database_identity(live_path, "Restored live database")?;
            if &restored != expected_previous {
                return Err(promotion_error(format!(
                    "Prepared promotion rollback did not restore the recorded identity for {}",
                    live_path.display()
                )));
            }
            remove_promotion_file(&prepared_path)?;
            cleanup_sqlite_sidecars(&backup_path)
        }
        None => {
            if backup_path.exists() {
                return Err(promotion_error(format!(
                    "Prepared first publication for {} unexpectedly has a backup",
                    live_path.display()
                )));
            }
            if live_identity.is_some() || live_path.exists() {
                cleanup_sqlite_sidecars(live_path)?;
            }
            remove_promotion_file(&prepared_path)
        }
    }
}

fn recover_legacy_promotion_backup(
    live_path: &Path,
    backup_path: &Path,
) -> Result<(), StorageError> {
    let backup_identity =
        require_recovery_database_identity(backup_path, "Legacy promotion backup")?;
    let live_identity = read_recovery_database_identity(live_path);
    let restore_backup = match live_identity {
        Ok(None) => true,
        Err(error) => {
            return Err(promotion_error(format!(
                "Cannot validate live database {} while a legacy promotion backup exists: {error}",
                live_path.display()
            )));
        }
        Ok(Some(ref live)) if live == &backup_identity => false,
        Ok(Some(ref live)) if live.generation > backup_identity.generation => false,
        Ok(Some(_)) => {
            return Err(promotion_error(format!(
                "Ambiguous legacy promotion backup for {}; refusing to overwrite the live database",
                live_path.display()
            )));
        }
    };
    if restore_backup {
        restore_promotion_database(backup_path, live_path)?;
        let restored = require_recovery_database_identity(live_path, "Recovered live database")?;
        if restored != backup_identity {
            return Err(promotion_error(format!(
                "Legacy promotion recovery produced an unexpected identity for {}",
                live_path.display()
            )));
        }
    }
    cleanup_sqlite_sidecars(backup_path)
}

fn cleanup_committed_promotion_artifacts(live_path: &Path) -> Result<(), StorageError> {
    #[cfg(test)]
    if promotion_cleanup_failure_path(live_path).exists() {
        return Err(promotion_error(
            "injected committed promotion cleanup failure",
        ));
    }

    let backup_path = live_path.with_extension("sqlite.backup");
    cleanup_sqlite_sidecars(&backup_path)?;
    let committed_path = promotion_committed_journal_path(live_path);
    remove_promotion_file(&committed_path)
}

fn grounding_display_name_expr(alias: &str) -> String {
    format!("COALESCE({alias}.qualified_name, {alias}.serialized_name)")
}

fn grounding_trimmed_name_expr(alias: &str) -> String {
    format!("TRIM({})", grounding_display_name_expr(alias))
}

fn grounding_indexable_predicate(alias: &str) -> String {
    format!(
        "{alias}.kind NOT IN ({}, {}, {})",
        NodeKind::FILE as i32,
        NodeKind::UNKNOWN as i32,
        NodeKind::BUILTIN_TYPE as i32
    )
}

fn grounding_import_like_symbol_predicate(alias: &str) -> String {
    let display_name = grounding_trimmed_name_expr(alias);
    format!(
        "{alias}.kind IN ({module_kind}, {namespace_kind}, {package_kind}) AND {}",
        grounding_import_like_name_predicate(&display_name),
        module_kind = NodeKind::MODULE as i32,
        namespace_kind = NodeKind::NAMESPACE as i32,
        package_kind = NodeKind::PACKAGE as i32,
    )
}

fn grounding_import_like_name_predicate(display_name: &str) -> String {
    let double_quoted_name = grounding_sql_same_delimiter_expr(display_name, "char(34)");
    let single_quoted_name = grounding_sql_same_delimiter_expr(display_name, "char(39)");
    let angle_wrapped_name = grounding_sql_surrounded_by_expr(display_name, "'<'", "'>'");
    let relative_current_dir_name = format!("{display_name} LIKE './%'");
    let relative_parent_dir_name = format!("{display_name} LIKE '../%'");
    let slash_separated_name = format!("instr({display_name}, '/') > 0");
    format!(
        "(
            {double_quoted_name}
            OR {single_quoted_name}
            OR {angle_wrapped_name}
            OR {relative_current_dir_name}
            OR {relative_parent_dir_name}
            OR {slash_separated_name}
        )"
    )
}

fn grounding_sql_same_delimiter_expr(display_name: &str, delimiter: &str) -> String {
    grounding_sql_surrounded_by_expr(display_name, delimiter, delimiter)
}

fn grounding_sql_surrounded_by_expr(
    display_name: &str,
    start_delimiter: &str,
    end_delimiter: &str,
) -> String {
    format!(
        "(substr({display_name}, 1, 1) = {start_delimiter} AND substr({display_name}, length({display_name}), 1) = {end_delimiter})"
    )
}

fn grounding_node_rank_sql(alias: &str) -> String {
    let import_like_symbol = grounding_import_like_symbol_predicate(alias);
    format!(
        "CASE
            WHEN {import_like_symbol} THEN 5
            WHEN {alias}.kind IN ({class_kind}, {struct_kind}, {interface_kind}, {enum_kind}, {union_kind}, {annotation_kind}, {typedef_kind}) THEN 0
            WHEN {alias}.kind IN ({function_kind}, {method_kind}, {macro_kind}) THEN 1
            WHEN {alias}.kind IN ({module_kind}, {namespace_kind}, {package_kind}) THEN 2
            WHEN {alias}.kind IN ({field_kind}, {variable_kind}, {global_variable_kind}, {constant_kind}, {enum_constant_kind}, {type_parameter_kind}) THEN 3
            ELSE 4
        END",
        import_like_symbol = import_like_symbol,
        module_kind = NodeKind::MODULE as i32,
        namespace_kind = NodeKind::NAMESPACE as i32,
        package_kind = NodeKind::PACKAGE as i32,
        class_kind = NodeKind::CLASS as i32,
        struct_kind = NodeKind::STRUCT as i32,
        interface_kind = NodeKind::INTERFACE as i32,
        enum_kind = NodeKind::ENUM as i32,
        union_kind = NodeKind::UNION as i32,
        annotation_kind = NodeKind::ANNOTATION as i32,
        typedef_kind = NodeKind::TYPEDEF as i32,
        function_kind = NodeKind::FUNCTION as i32,
        method_kind = NodeKind::METHOD as i32,
        macro_kind = NodeKind::MACRO as i32,
        field_kind = NodeKind::FIELD as i32,
        variable_kind = NodeKind::VARIABLE as i32,
        global_variable_kind = NodeKind::GLOBAL_VARIABLE as i32,
        constant_kind = NodeKind::CONSTANT as i32,
        enum_constant_kind = NodeKind::ENUM_CONSTANT as i32,
        type_parameter_kind = NodeKind::TYPE_PARAMETER as i32,
    )
}

fn outside_related_file_edge_predicate(file_param: &str) -> String {
    format!(
        "source_node_id NOT IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
         AND target_node_id NOT IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
         AND {}",
        outside_file_node_predicate(file_param)
    )
}

fn outside_file_node_predicate(file_param: &str) -> String {
    format!("(file_node_id IS NULL OR file_node_id != {file_param})")
}

/// Errors returned by storage facade operations.
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Invalid enum value: {0}")]
    EnumConversion(#[from] EnumConversionError),
    #[error("Other error: {0}")]
    Other(String),
}

/// SQLite-backed graph, search, file, and snapshot store.
///
/// The store is the persistence boundary for indexer output. It owns schema
/// migration, projection replacement, retrieval manifest rows, and derived
/// grounding snapshots. Callers must invalidate or rebuild derived snapshots
/// after mutating graph/search projections.
pub struct Storage {
    conn: Connection,
    cache: StorageCache,
    deferred_secondary_indexes: bool,
}

/// One coherent read view of a live SQLite store.
///
/// Publication replaces the live database in place. Grouping related reads in
/// one transaction prevents a reader from combining rows from two generations.
pub struct StorageReadSnapshot<'a> {
    storage: &'a Storage,
    active: bool,
}

/// One exclusive write view used when validation and publication must commit together.
pub struct StorageWriteTransaction<'a> {
    storage: &'a mut Storage,
    active: bool,
}

impl StorageReadSnapshot<'_> {
    pub fn storage(&self) -> &Storage {
        self.storage
    }

    pub fn finish(mut self) -> Result<(), StorageError> {
        self.storage.conn.execute_batch("COMMIT")?;
        self.active = false;
        Ok(())
    }
}

impl Drop for StorageReadSnapshot<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.storage.conn.execute_batch("ROLLBACK");
        }
    }
}

impl StorageWriteTransaction<'_> {
    pub fn storage(&self) -> &Storage {
        self.storage
    }

    pub fn storage_mut(&mut self) -> &mut Storage {
        self.storage
    }

    pub fn finish(mut self) -> Result<(), StorageError> {
        self.storage.conn.execute_batch("COMMIT")?;
        self.active = false;
        Ok(())
    }
}

impl Drop for StorageWriteTransaction<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = self.storage.conn.execute_batch("ROLLBACK");
        }
    }
}

/// Opening mode for a SQLite store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOpenMode {
    /// Live stores create all indexes immediately and are ready for mixed reads.
    Live,
    /// Build stores defer expensive secondary indexes until finalization.
    Build,
}

/// Per-table timing breakdown for a projection flush.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectionFlushBreakdown {
    pub files_ms: u32,
    pub nodes_ms: u32,
    pub edges_ms: u32,
    pub occurrences_ms: u32,
    pub component_access_ms: u32,
    pub callable_projection_ms: u32,
}

pub struct ProjectionBatch<'a> {
    pub files: &'a [FileInfo],
    pub file_content_hashes: &'a [FileContentHash],
    pub nodes: &'a [Node],
    pub edges: &'a [Edge],
    pub occurrences: &'a [Occurrence],
    pub component_access: &'a [(NodeId, AccessKind)],
    pub callable_projection_states: &'a [CallableProjectionState],
}

#[derive(Default)]
struct StorageCache {
    nodes:
        Arc<RwLock<HashMap<codestory_contracts::graph::NodeId, codestory_contracts::graph::Node>>>,
}

/// Stored file row persisted with graph projections.
///
/// `modification_time` is milliseconds since the Unix epoch and is compared by
/// workspace refresh planning. `indexed` marks whether the file produced a
/// completed projection in the current store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub id: i64,
    pub path: PathBuf,
    pub language: String,
    pub modification_time: i64,
    pub indexed: bool,
    pub complete: bool,
    pub line_count: u32,
    #[serde(default)]
    pub file_role: FileRole,
}

/// Verified content identity for one parser-backed file projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContentHash {
    pub file_id: i64,
    pub content_hash: String,
}

/// Heuristic role assigned to a file for ranking and summaries.
///
/// This role is diagnostic metadata; it must not be treated as parser-backed
/// evidence about symbols or language support.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    #[default]
    Source,
    Entrypoint,
    Test,
    Docs,
    Benchmark,
    Generated,
    Vendor,
}

impl FileRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Entrypoint => "entrypoint",
            Self::Test => "test",
            Self::Docs => "docs",
            Self::Benchmark => "benchmark",
            Self::Generated => "generated",
            Self::Vendor => "vendor",
        }
    }

    pub fn from_db_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "entrypoint" => Self::Entrypoint,
            "test" => Self::Test,
            "docs" => Self::Docs,
            "benchmark" => Self::Benchmark,
            "generated" => Self::Generated,
            "vendor" => Self::Vendor,
            _ => Self::Source,
        }
    }

    pub fn classify_path(path: &Path) -> Self {
        let mut normalized = path
            .to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase();
        let mut best_repo_relative: Option<(usize, String)> = None;
        for marker in ["/source/repos/", "source/repos/", "/repos/", "repos/"] {
            if let Some(index) = normalized.rfind(marker) {
                let remainder = &normalized[index + marker.len()..];
                if let Some((_, repo_relative)) = remainder.split_once('/')
                    && best_repo_relative
                        .as_ref()
                        .is_none_or(|(best_index, _)| index > *best_index)
                {
                    best_repo_relative = Some((index, repo_relative.to_string()));
                }
            }
        }
        if let Some((_, repo_relative)) = best_repo_relative {
            normalized = repo_relative;
        }
        let marked = format!("/{normalized}");
        let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());

        if marked.contains("/node_modules/")
            || marked.contains("/vendor/")
            || marked.contains("/third_party/")
            || marked.contains("/third-party/")
            || marked.contains("/external/")
        {
            return Self::Vendor;
        }
        if marked.contains("/target/")
            || marked.contains("/dist/")
            || marked.contains("/build/")
            || marked.contains("/generated/")
            || marked.contains("/schema/typescript/")
            || marked.contains(".generated.")
            || file_name.ends_with(".g.cs")
            || file_name.contains("payload-types")
        {
            return Self::Generated;
        }
        if marked.contains("/tests/")
            || marked.contains("/test/")
            || marked.contains("/spec/")
            || marked.contains("/fixtures/")
            || marked.contains("/__tests__/")
            || marked.contains("-test-client/")
            || marked.contains("_test_client/")
            || file_name.contains(".test.")
            || file_name.contains(".spec.")
            || file_name.ends_with("_test.rs")
            || file_name.ends_with("_tests.rs")
            || file_name.ends_with("_test.py")
            || file_name.ends_with("_tests.py")
            || file_name.ends_with("_test.ts")
            || file_name.ends_with("_tests.ts")
            || file_name.ends_with("_test.tsx")
            || file_name.ends_with("_tests.tsx")
        {
            return Self::Test;
        }
        if marked.contains("/docs/")
            || marked.contains("/doc/")
            || matches!(file_name, "readme.md" | "changelog.md")
        {
            return Self::Docs;
        }
        if marked.contains("/benchmarks/")
            || marked.contains("/benchmark/")
            || marked.contains("/benches/")
            || marked.contains("/bench/")
        {
            return Self::Benchmark;
        }
        if matches!(
            file_name,
            "main.rs"
                | "lib.rs"
                | "mod.rs"
                | "main.ts"
                | "main.tsx"
                | "main.js"
                | "main.jsx"
                | "index.ts"
                | "index.tsx"
                | "index.js"
                | "index.jsx"
        ) {
            return Self::Entrypoint;
        }
        Self::Source
    }
}

/// Counts describing the effective store contents.
///
/// When summary snapshots are ready, counts come from the snapshot read model;
/// otherwise they are computed from live tables. Fatal errors are always counted
/// from the error table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub node_count: i64,
    pub edge_count: i64,
    pub file_count: i64,
    pub error_count: i64,
    pub fatal_error_count: i64,
}

/// Indexing mode that produced one durable core database generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPublicationMode {
    Full,
    Incremental,
}

impl IndexPublicationMode {
    fn db_value(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }

    fn from_db(value: &str) -> Result<Self, StorageError> {
        match value {
            "full" => Ok(Self::Full),
            "incremental" => Ok(Self::Incremental),
            _ => Err(StorageError::Other(format!(
                "Unsupported index publication mode: {value}"
            ))),
        }
    }
}

/// Durable identity of the complete core database generation at the live path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPublicationRecord {
    pub generation: u64,
    pub generation_id: String,
    pub run_id: String,
    pub mode: IndexPublicationMode,
    pub published_at_epoch_ms: i64,
}

fn index_publication_record_from_values(
    generation: i64,
    generation_id: String,
    run_id: String,
    mode: String,
    published_at_epoch_ms: i64,
) -> Result<IndexPublicationRecord, StorageError> {
    let generation = u64::try_from(generation).map_err(|_| {
        StorageError::Other(format!(
            "Invalid index publication generation: {generation}"
        ))
    })?;
    if generation == 0
        || generation_id.trim().is_empty()
        || run_id.trim().is_empty()
        || published_at_epoch_ms < 0
    {
        return Err(StorageError::Other(
            "Index publication identity contains an empty or zero field".to_string(),
        ));
    }
    Ok(IndexPublicationRecord {
        generation,
        generation_id,
        run_id,
        mode: IndexPublicationMode::from_db(&mode)?,
        published_at_epoch_ms,
    })
}

fn read_index_publication(
    conn: &Connection,
) -> Result<Option<IndexPublicationRecord>, StorageError> {
    let table_exists: i64 = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'index_publication'
        )",
        [],
        |row| row.get(0),
    )?;
    if table_exists == 0 {
        return Ok(None);
    }
    let values = conn.query_row(
        "SELECT generation, generation_id, run_id, mode, published_at_epoch_ms
         FROM index_publication WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    );
    match values {
        Ok((generation, generation_id, run_id, mode, published_at_epoch_ms)) => {
            index_publication_record_from_values(
                generation,
                generation_id,
                run_id,
                mode,
                published_at_epoch_ms,
            )
            .map(Some)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_complete_index_publication(
    conn: &Connection,
) -> Result<Option<IndexPublicationRecord>, StorageError> {
    let values = conn.query_row(
        "SELECT generation, generation_id, run_id, mode, published_at_epoch_ms
         FROM index_publication
         WHERE id = 1
           AND NOT EXISTS (SELECT 1 FROM incomplete_index_run WHERE id = 1)",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    );
    match values {
        Ok((generation, generation_id, run_id, mode, published_at_epoch_ms)) => {
            index_publication_record_from_values(
                generation,
                generation_id,
                run_id,
                mode,
                published_at_epoch_ms,
            )
            .map(Some)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn is_framework_synthetic_node(node: &Node) -> bool {
    node.canonical_id.as_deref().is_some_and(|canonical_id| {
        canonical_id.starts_with("tauri:command:")
            || canonical_id.starts_with("payload:collection:")
            || canonical_id.starts_with("route_endpoint:")
            || canonical_id.starts_with("openapi:endpoint:")
    })
}

fn is_endpoint_synthetic_node(node: &Node) -> bool {
    node.canonical_id.as_deref().is_some_and(|canonical_id| {
        canonical_id.starts_with("route_endpoint:") || canonical_id.starts_with("openapi:endpoint:")
    })
}

fn preferred_framework_node(
    conn: &Connection,
    batch_file_paths: &HashMap<NodeId, PathBuf>,
    existing: Node,
    candidate: Node,
) -> Result<Node, StorageError> {
    let existing_rank = framework_node_source_rank(conn, batch_file_paths, &existing)?;
    let candidate_rank = framework_node_source_rank(conn, batch_file_paths, &candidate)?;
    if candidate_rank > existing_rank {
        Ok(candidate)
    } else if candidate_rank == existing_rank && is_endpoint_synthetic_node(&candidate) {
        let existing_key = framework_node_stable_source_key(conn, batch_file_paths, &existing)?;
        let candidate_key = framework_node_stable_source_key(conn, batch_file_paths, &candidate)?;
        if candidate_key < existing_key {
            Ok(candidate)
        } else {
            Ok(existing)
        }
    } else {
        Ok(existing)
    }
}

fn framework_node_source_rank(
    conn: &Connection,
    batch_file_paths: &HashMap<NodeId, PathBuf>,
    node: &Node,
) -> Result<u8, StorageError> {
    let canonical_id = node.canonical_id.as_deref().unwrap_or_default();
    let path = framework_node_file_path(conn, batch_file_paths, node)?
        .map(|path| normalize_framework_source_path(&path))
        .unwrap_or_default();

    if canonical_id.starts_with("tauri:command:") {
        if path.contains("/src-tauri/") || path.ends_with(".rs") {
            return Ok(4);
        }
        return Ok(u8::from(node.start_line.is_some()));
    }

    if canonical_id.starts_with("payload:collection:") {
        if path.contains("/collections/") || path.contains("/payload.config.") {
            return Ok(4);
        }
        if node.start_col == Some(1) {
            return Ok(3);
        }
        return Ok(u8::from(!path.is_empty()));
    }

    if canonical_id.starts_with("route_endpoint:") || canonical_id.starts_with("openapi:endpoint:")
    {
        if !path.is_empty() && node.start_line.is_some() {
            return Ok(4);
        }
        if !path.is_empty() {
            return Ok(3);
        }
        return Ok(u8::from(node.start_line.is_some()));
    }

    Ok(0)
}

fn framework_node_stable_source_key(
    conn: &Connection,
    batch_file_paths: &HashMap<NodeId, PathBuf>,
    node: &Node,
) -> Result<(String, u32, u32, String), StorageError> {
    let path = framework_node_file_path(conn, batch_file_paths, node)?
        .map(|path| normalize_framework_source_path(&path))
        .unwrap_or_default();
    Ok((
        path,
        node.start_line.unwrap_or(u32::MAX),
        node.start_col.unwrap_or(u32::MAX),
        node.serialized_name.clone(),
    ))
}

fn framework_node_file_path(
    conn: &Connection,
    batch_file_paths: &HashMap<NodeId, PathBuf>,
    node: &Node,
) -> Result<Option<PathBuf>, StorageError> {
    let Some(file_node_id) = node.file_node_id else {
        return Ok(None);
    };
    if let Some(path) = batch_file_paths.get(&file_node_id) {
        return Ok(Some(path.clone()));
    }

    let mut stmt = conn.prepare("SELECT path FROM file WHERE id = ?1")?;
    let mut rows = stmt.query(params![file_node_id.0])?;
    if let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        return Ok(Some(PathBuf::from(path)));
    }

    let mut stmt = conn.prepare("SELECT serialized_name FROM node WHERE id = ?1")?;
    let mut rows = stmt.query(params![file_node_id.0])?;
    if let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        return Ok(Some(PathBuf::from(path)));
    }

    Ok(None)
}

fn normalize_framework_source_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

/// Freshness state for derived grounding snapshot layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroundingSnapshotState {
    Dirty,
    Building,
    Ready,
}

impl GroundingSnapshotState {
    fn from_db(value: i64) -> Option<Self> {
        match value {
            GROUNDING_SNAPSHOT_STATE_DIRTY => Some(Self::Dirty),
            GROUNDING_SNAPSHOT_STATE_BUILDING => Some(Self::Building),
            GROUNDING_SNAPSHOT_STATE_READY => Some(Self::Ready),
            _ => None,
        }
    }

    fn db_value(self) -> i64 {
        match self {
            Self::Dirty => GROUNDING_SNAPSHOT_STATE_DIRTY,
            Self::Building => GROUNDING_SNAPSHOT_STATE_BUILDING,
            Self::Ready => GROUNDING_SNAPSHOT_STATE_READY,
        }
    }
}

/// Metadata row for derived grounding snapshots.
///
/// Ready states mean the corresponding read model has been built from the
/// current persisted graph at that point in time. Projection writes must mark
/// these states dirty before callers rely on them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundingSnapshotMetadata {
    pub version: i64,
    pub summary_state: GroundingSnapshotState,
    pub detail_state: GroundingSnapshotState,
    pub summary_built_at_epoch_ms: Option<i64>,
    pub detail_built_at_epoch_ms: Option<i64>,
}

impl GroundingSnapshotMetadata {
    fn has_ready_summary(self) -> bool {
        self.version == GROUNDING_SNAPSHOT_VERSION
            && self.summary_state == GroundingSnapshotState::Ready
    }

    fn has_ready_detail(self) -> bool {
        self.version == GROUNDING_SNAPSHOT_VERSION
            && self.detail_state == GroundingSnapshotState::Ready
    }
}

#[derive(Debug, Clone)]
pub struct GroundingFileSummary {
    pub file: FileInfo,
    pub symbol_count: u32,
    pub best_node_rank: u8,
}

#[derive(Debug, Clone)]
pub struct GroundingNodeRecord {
    pub node: Node,
    pub display_name: String,
    pub file_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroundingEdgeKindCount {
    pub node_id: NodeId,
    pub kind: EdgeKind,
    pub count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileProjectionRemovalSummary {
    pub canonical_file_node_id: i64,
    pub removed_node_count: usize,
    pub removed_edge_count: usize,
    pub removed_occurrence_count: usize,
    pub removed_error_count: usize,
    pub removed_bookmark_node_count: usize,
    pub removed_component_access_count: usize,
    pub removed_local_symbol_count: usize,
    pub removed_file_row_count: usize,
    pub removed_callable_projection_state_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerProjectionRemovalSummary {
    pub file_id: i64,
    pub removed_edge_count: usize,
    pub removed_occurrence_count: usize,
    pub removed_callable_projection_state_count: usize,
}

/// Lightweight symbol projection used by lexical search sidecars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSymbolProjection {
    pub node_id: NodeId,
    pub display_name: String,
}

/// Symbol projection plus source location details for review and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSymbolProjectionDetail {
    pub node_id: NodeId,
    pub display_name: String,
    pub node_kind: Option<i64>,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

/// Stored generated symbol document and embedding payload.
///
/// The document records graph-derived text and embedding metadata. Dense
/// readiness still depends on the retrieval manifest; the row alone does not
/// prove a sidecar is current.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmSymbolDoc {
    pub node_id: NodeId,
    pub file_node_id: Option<NodeId>,
    pub kind: NodeKind,
    pub display_name: String,
    pub qualified_name: Option<String>,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub doc_text: String,
    pub doc_version: u32,
    pub doc_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_profile: Option<String>,
    pub embedding_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_backend: Option<String>,
    pub embedding_dim: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_shape: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_policy_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dense_reason: Option<String>,
    pub embedding: Vec<f32>,
    pub updated_at_epoch_ms: i64,
}

/// Reuse metadata for deciding whether a stored symbol document is still fresh.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmSymbolDocReuseMetadata {
    pub node_id: NodeId,
    pub doc_version: u32,
    pub doc_hash: String,
    pub embedding_profile: Option<String>,
    pub embedding_model: String,
    pub embedding_backend: Option<String>,
    pub embedding_dim: u32,
    pub doc_shape: Option<String>,
    pub semantic_policy_version: Option<String>,
    pub dense_reason: Option<String>,
}

/// Aggregate metadata for stored generated symbol documents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmSymbolDocStats {
    pub doc_count: u32,
    pub embedding_profile: Option<String>,
    #[serde(rename = "cache_key")]
    pub embedding_model: Option<String>,
    pub embedding_backend: Option<String>,
    #[serde(rename = "dimension")]
    pub embedding_dim: Option<u32>,
    pub doc_version: Option<u32>,
    pub doc_shape: Option<String>,
    pub semantic_policy_version: Option<String>,
    pub mixed_embedding_profiles: bool,
    pub mixed_embedding_models: bool,
    pub mixed_embedding_backends: bool,
    pub mixed_dimensions: bool,
    pub mixed_doc_versions: bool,
    pub mixed_doc_shapes: bool,
    pub mixed_semantic_policy_versions: bool,
}

/// Counts of dense-anchor selection reasons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DenseReasonCounts {
    pub public_api: u32,
    pub entrypoint: u32,
    pub documented_nontrivial: u32,
    pub central_graph_node: u32,
    pub component_report: u32,
    pub unstructured_doc: u32,
}

/// Graph-native symbol-search document used by retrieval sidecars.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolSearchDoc {
    pub node_id: NodeId,
    pub file_node_id: Option<NodeId>,
    pub kind: NodeKind,
    pub display_name: String,
    pub qualified_name: Option<String>,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub doc_text: String,
    pub doc_version: u32,
    pub doc_hash: String,
    pub policy_version: String,
    pub source_provenance: String,
    pub updated_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolSummaryRecord {
    pub node_id: NodeId,
    pub content_hash: String,
    pub summary: String,
    pub model: String,
    pub updated_at_epoch_ms: i64,
}

impl Storage {
    /// Open a live store, applying schema migrations and secondary indexes.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        Self::open_with_mode(path, StorageOpenMode::Live)
    }

    /// Open the current schema without migrations or write-oriented pragmas.
    ///
    /// Published snapshots are immutable from a reader's perspective, so
    /// concurrent readers must not contend with a staged refresh merely by
    /// opening the live database.
    pub fn open_read_only<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path = path.as_ref();
        recover_interrupted_promotion(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(Duration::from_millis(2_500))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let version = version.max(0) as u32;
        if version != SCHEMA_VERSION {
            return Err(StorageError::Other(format!(
                "Read-only storage requires schema version {SCHEMA_VERSION}, found {version}"
            )));
        }
        Ok(Self {
            conn,
            cache: StorageCache::default(),
            deferred_secondary_indexes: false,
        })
    }

    pub fn read_snapshot(&self) -> Result<StorageReadSnapshot<'_>, StorageError> {
        self.conn.execute_batch("BEGIN DEFERRED TRANSACTION")?;
        Ok(StorageReadSnapshot {
            storage: self,
            active: true,
        })
    }

    pub fn write_transaction(&mut self) -> Result<StorageWriteTransaction<'_>, StorageError> {
        self.conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;
        Ok(StorageWriteTransaction {
            storage: self,
            active: true,
        })
    }

    /// Open a build-mode store after removing stale SQLite sidecars.
    pub fn open_build<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path = path.as_ref();
        cleanup_sqlite_sidecars(path)?;
        Self::open_with_mode(path, StorageOpenMode::Build)
    }

    /// Open a store with explicit live or build indexing behavior.
    pub fn open_with_mode<P: AsRef<Path>>(
        path: P,
        mode: StorageOpenMode,
    ) -> Result<Self, StorageError> {
        let path = path.as_ref();
        if matches!(mode, StorageOpenMode::Live) {
            recover_interrupted_promotion(path)?;
        }
        let conn = Connection::open(path)?;
        // Allow concurrent reads while indexing writes, and avoid flaky "database is locked" errors
        // in app shells when users query mid-index.
        conn.busy_timeout(Duration::from_millis(2_500))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        if matches!(mode, StorageOpenMode::Build) {
            // Favor fewer temp-file round trips and larger page caches while building
            // the staged full-refresh snapshot.
            conn.pragma_update(None, "temp_store", "MEMORY")?;
            conn.pragma_update(None, "cache_size", "-131072")?;
            conn.pragma_update(None, "mmap_size", "268435456")?;
        }
        let storage = Self {
            conn,
            cache: StorageCache::default(),
            deferred_secondary_indexes: matches!(mode, StorageOpenMode::Build),
        };
        storage.init(mode)?;
        Ok(storage)
    }

    pub fn database_schema_version(path: &Path) -> Result<u32, StorageError> {
        recover_interrupted_promotion(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(version.max(0) as u32)
    }

    /// Read the incomplete-run fence without migrating or otherwise mutating a live database.
    pub fn database_has_incomplete_incremental_run(path: &Path) -> Result<bool, StorageError> {
        recover_interrupted_promotion(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let version = version.max(0) as u32;
        if version != INCOMPLETE_INCREMENTAL_SCHEMA_VERSION && version > SCHEMA_VERSION {
            return Err(StorageError::Other(format!(
                "Unsupported database schema version: {version} (max supported: {SCHEMA_VERSION})"
            )));
        }
        let marked = has_incomplete_incremental_marker(&conn)?;
        if version == INCOMPLETE_INCREMENTAL_SCHEMA_VERSION && !marked {
            return Err(StorageError::Other(format!(
                "Database schema version {version} is only valid while an incremental index run is marked incomplete"
            )));
        }
        Ok(marked)
    }

    /// Read the durable publication identity without migrating or mutating the database.
    pub fn database_index_publication(
        path: &Path,
    ) -> Result<Option<IndexPublicationRecord>, StorageError> {
        recover_interrupted_promotion(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        read_index_publication(&conn)
    }

    /// Read the durable publication only when the same SQLite snapshot has no
    /// incomplete-run fence.
    pub fn database_complete_index_publication(
        path: &Path,
    ) -> Result<Option<IndexPublicationRecord>, StorageError> {
        recover_interrupted_promotion(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        read_complete_index_publication(&conn)
    }

    pub fn copy_database_snapshot(
        source_path: &Path,
        target_path: &Path,
    ) -> Result<(), StorageError> {
        recover_interrupted_promotion(source_path)?;
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                StorageError::Other(format!(
                    "Failed to create SQLite snapshot target dir {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        source.backup(MAIN_DB, target_path, None::<fn(rusqlite::backup::Progress)>)?;
        Ok(())
    }

    /// Create an in-memory store for tests and short-lived callers.
    pub fn new_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let storage = Self {
            conn,
            cache: StorageCache::default(),
            deferred_secondary_indexes: false,
        };
        storage.init(StorageOpenMode::Live)?;
        Ok(storage)
    }

    /// Expose the raw connection for advanced read/write operations.
    ///
    /// Prefer typed store methods when they exist; direct writes must preserve
    /// schema invariants and derived snapshot freshness manually.
    pub fn get_connection(&self) -> &Connection {
        &self.conn
    }

    pub fn get_index_artifact_cache(
        &self,
        path: &Path,
        cache_key: &str,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT artifact_blob
             FROM index_artifact_cache
             WHERE file_path = ?1
               AND cache_key = ?2",
        )?;
        let mut rows = stmt.query(params![path.to_string_lossy().to_string(), cache_key])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row.get(0)?));
        }
        Ok(None)
    }

    pub fn upsert_index_artifact_cache(
        &self,
        path: &Path,
        cache_key: &str,
        artifact_blob: &[u8],
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO index_artifact_cache (
                file_path,
                cache_key,
                artifact_blob,
                updated_at_epoch_ms
             )
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(file_path) DO UPDATE SET
                cache_key = excluded.cache_key,
                artifact_blob = excluded.artifact_blob,
                updated_at_epoch_ms = excluded.updated_at_epoch_ms",
            params![
                path.to_string_lossy().to_string(),
                cache_key,
                artifact_blob,
                current_epoch_ms()
            ],
        )?;
        Ok(())
    }

    pub fn copy_index_artifact_cache_from(
        &mut self,
        source_path: &Path,
    ) -> Result<usize, StorageError> {
        if !source_path.exists() {
            return Ok(0);
        }
        let source = source_path.to_string_lossy().to_string();
        self.conn
            .execute("ATTACH DATABASE ?1 AS source_snapshot", params![source])?;
        let copy_result = self.conn.execute(
            "INSERT OR REPLACE INTO index_artifact_cache (
                file_path,
                cache_key,
                artifact_blob,
                updated_at_epoch_ms
             )
             SELECT
                file_path,
                cache_key,
                artifact_blob,
                updated_at_epoch_ms
             FROM source_snapshot.index_artifact_cache",
            [],
        );
        let copied = copy_result?;
        let has_symbol_summary: bool = self.conn.query_row(
            "SELECT EXISTS (
                SELECT 1
                FROM source_snapshot.sqlite_master
                WHERE type = 'table' AND name = 'symbol_summary'
             )",
            [],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )?;
        if has_symbol_summary {
            self.conn.execute(
                "INSERT OR REPLACE INTO symbol_summary (
                    node_id,
                    content_hash,
                    summary,
                    model,
                    updated_at_epoch_ms
                 )
                 SELECT
                    source_summary.node_id,
                    source_summary.content_hash,
                    source_summary.summary,
                    source_summary.model,
                    source_summary.updated_at_epoch_ms
                 FROM source_snapshot.symbol_summary source_summary
                 WHERE EXISTS (
                    SELECT 1 FROM node WHERE node.id = source_summary.node_id
                 )",
                [],
            )?;
        }
        let detach_result = self.conn.execute("DETACH DATABASE source_snapshot", []);
        detach_result?;
        Ok(copied)
    }

    pub fn has_ready_resolution_support_snapshot(
        &self,
        snapshot_version: i64,
    ) -> Result<bool, StorageError> {
        Ok(self
            .get_resolution_support_snapshot(snapshot_version)?
            .is_some())
    }

    pub fn get_resolution_support_snapshot(
        &self,
        snapshot_version: i64,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT snapshot_blob
             FROM resolution_support_snapshot
             WHERE id = 1
               AND snapshot_version = ?1
               AND state = ?2
               AND snapshot_blob IS NOT NULL",
        )?;
        let mut rows = stmt.query(params![
            snapshot_version,
            GroundingSnapshotState::Ready.db_value()
        ])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row.get(0)?));
        }
        Ok(None)
    }

    pub fn put_resolution_support_snapshot(
        &self,
        snapshot_version: i64,
        snapshot_blob: &[u8],
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO resolution_support_snapshot (
                id,
                snapshot_version,
                state,
                snapshot_blob,
                built_at_epoch_ms
             )
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                snapshot_version = excluded.snapshot_version,
                state = excluded.state,
                snapshot_blob = excluded.snapshot_blob,
                built_at_epoch_ms = excluded.built_at_epoch_ms",
            params![
                snapshot_version,
                GroundingSnapshotState::Ready.db_value(),
                snapshot_blob,
                current_epoch_ms()
            ],
        )?;
        Ok(())
    }

    pub fn invalidate_resolution_support_snapshot(&self) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO resolution_support_snapshot (
                id,
                snapshot_version,
                state,
                snapshot_blob,
                built_at_epoch_ms
             )
             VALUES (1, 0, ?1, NULL, NULL)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                snapshot_blob = NULL,
                built_at_epoch_ms = NULL",
            params![GroundingSnapshotState::Dirty.db_value()],
        )?;
        Ok(())
    }

    pub fn has_ready_grounding_summary_snapshots(&self) -> Result<bool, StorageError> {
        Ok(self
            .get_grounding_snapshot_metadata()?
            .is_some_and(GroundingSnapshotMetadata::has_ready_summary))
    }

    pub fn has_ready_grounding_detail_snapshots(&self) -> Result<bool, StorageError> {
        Ok(self
            .get_grounding_snapshot_metadata()?
            .is_some_and(GroundingSnapshotMetadata::has_ready_detail))
    }

    pub fn has_ready_grounding_snapshots(&self) -> Result<bool, StorageError> {
        Ok(self.has_ready_grounding_summary_snapshots()?
            && self.has_ready_grounding_detail_snapshots()?)
    }

    pub fn get_grounding_snapshot_metadata(
        &self,
    ) -> Result<Option<GroundingSnapshotMetadata>, StorageError> {
        self.grounding_snapshot_metadata()
    }

    fn ensure_grounding_snapshot_meta_row(&self) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO grounding_snapshot_meta (
                id,
                snapshot_version,
                summary_state,
                detail_state,
                summary_built_at_epoch_ms,
                detail_built_at_epoch_ms
             )
             VALUES (1, ?1, ?2, ?2, NULL, NULL)",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                GroundingSnapshotState::Dirty.db_value()
            ],
        )?;
        Ok(())
    }

    fn write_grounding_snapshot_states(
        &self,
        summary_state: GroundingSnapshotState,
        detail_state: GroundingSnapshotState,
        summary_built_at_epoch_ms: Option<i64>,
        detail_built_at_epoch_ms: Option<i64>,
    ) -> Result<(), StorageError> {
        self.ensure_grounding_snapshot_meta_row()?;
        self.conn.execute(
            "UPDATE grounding_snapshot_meta
             SET snapshot_version = ?1,
                 summary_state = ?2,
                 detail_state = ?3,
                 summary_built_at_epoch_ms = ?4,
                 detail_built_at_epoch_ms = ?5
             WHERE id = 1",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                summary_state.db_value(),
                detail_state.db_value(),
                summary_built_at_epoch_ms,
                detail_built_at_epoch_ms,
            ],
        )?;
        Ok(())
    }

    fn mark_grounding_snapshots_dirty(&self) -> Result<(), StorageError> {
        self.write_grounding_snapshot_states(
            GroundingSnapshotState::Dirty,
            GroundingSnapshotState::Dirty,
            None,
            None,
        )
    }

    pub fn mark_grounding_detail_snapshots_dirty(&self) -> Result<(), StorageError> {
        self.ensure_grounding_snapshot_meta_row()?;
        self.conn.execute(
            "UPDATE grounding_snapshot_meta
             SET snapshot_version = ?1,
                 detail_state = ?2,
                 detail_built_at_epoch_ms = NULL
             WHERE id = 1",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                GroundingSnapshotState::Dirty.db_value()
            ],
        )?;
        Ok(())
    }

    pub fn invalidate_grounding_snapshots(&self) -> Result<(), StorageError> {
        self.mark_grounding_snapshots_dirty()?;
        self.invalidate_resolution_support_snapshot()?;
        Ok(())
    }

    /// Mark a live incremental index run incomplete before it mutates projections.
    pub fn begin_incremental_run(&self) -> Result<(), StorageError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO incomplete_index_run (id, started_at_epoch_ms)
             VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET
                started_at_epoch_ms = excluded.started_at_epoch_ms",
            params![current_epoch_ms()],
        )?;
        transaction.pragma_update(
            None,
            "user_version",
            INCOMPLETE_INCREMENTAL_SCHEMA_VERSION.to_string(),
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Whether a prior live incremental index run did not reach its success boundary.
    pub fn has_incomplete_incremental_run(&self) -> Result<bool, StorageError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM incomplete_index_run WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Clear the live incremental marker only after resolution and snapshots succeed.
    pub fn finish_incremental_run(&self) -> Result<(), StorageError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute("DELETE FROM incomplete_index_run WHERE id = 1", [])?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION.to_string())?;
        transaction.commit()?;
        Ok(())
    }

    /// Return the durable identity of the currently stored core generation.
    pub fn get_index_publication(&self) -> Result<Option<IndexPublicationRecord>, StorageError> {
        read_index_publication(&self.conn)
    }

    pub fn get_complete_index_publication(
        &self,
    ) -> Result<Option<IndexPublicationRecord>, StorageError> {
        read_complete_index_publication(&self.conn)
    }

    /// Store the identity that will describe this database once it is published.
    pub fn put_index_publication(
        &self,
        publication: &IndexPublicationRecord,
    ) -> Result<(), StorageError> {
        if publication.generation == 0
            || publication.generation > i64::MAX as u64
            || publication.generation_id.trim().is_empty()
            || publication.run_id.trim().is_empty()
            || publication.published_at_epoch_ms < 0
        {
            return Err(StorageError::Other(
                "Index publication identity contains an invalid field".to_string(),
            ));
        }
        self.conn.execute(
            "INSERT INTO index_publication (
                id, generation, generation_id, run_id, mode, published_at_epoch_ms
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                generation = excluded.generation,
                generation_id = excluded.generation_id,
                run_id = excluded.run_id,
                mode = excluded.mode,
                published_at_epoch_ms = excluded.published_at_epoch_ms",
            params![
                publication.generation as i64,
                publication.generation_id.as_str(),
                publication.run_id.as_str(),
                publication.mode.db_value(),
                publication.published_at_epoch_ms,
            ],
        )?;
        Ok(())
    }

    pub fn update_file_metadata(
        &self,
        info: &FileInfo,
        content_hash: Option<&str>,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE file
             SET path = ?2,
                 language = ?3,
                 modification_time = ?4,
                 indexed = ?5,
                 complete = ?6,
                 line_count = ?7,
                 file_role = ?8,
                 content_hash = ?9
             WHERE id = ?1",
            params![
                info.id,
                info.path.to_string_lossy(),
                info.language,
                info.modification_time,
                i32::from(info.indexed),
                i32::from(info.complete),
                info.line_count,
                info.file_role.as_str(),
                content_hash,
            ],
        )?;
        self.mark_grounding_snapshots_dirty()?;
        Ok(())
    }

    /// Read the verified parser source hash stored with one file projection.
    pub fn get_file_content_hash(&self, file_id: i64) -> Result<Option<String>, StorageError> {
        self.conn
            .query_row(
                "SELECT content_hash FROM file WHERE id = ?1",
                params![file_id],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(StorageError::from)
    }

    pub(crate) fn get_file_content_hashes(&self) -> Result<HashMap<i64, String>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, content_hash FROM file WHERE content_hash IS NOT NULL")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(StorageError::from)
    }

    pub fn refresh_grounding_summary_snapshots(&self) -> Result<(), StorageError> {
        let rank_sql = grounding_node_rank_sql("n");
        let display_name = grounding_display_name_expr("n");
        let indexable = grounding_indexable_predicate("n");
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO grounding_snapshot_meta (
                id,
                snapshot_version,
                summary_state,
                detail_state,
                summary_built_at_epoch_ms,
                detail_built_at_epoch_ms
             )
             VALUES (1, ?1, ?2, ?3, NULL, NULL)
             ON CONFLICT(id) DO UPDATE SET
                snapshot_version = excluded.snapshot_version,
                summary_state = excluded.summary_state,
                detail_state = excluded.detail_state,
                summary_built_at_epoch_ms = NULL,
                detail_built_at_epoch_ms = NULL",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                GroundingSnapshotState::Building.db_value(),
                GroundingSnapshotState::Dirty.db_value(),
            ],
        )?;
        tx.execute("DELETE FROM grounding_repo_stats_snapshot", [])?;
        tx.execute("DELETE FROM grounding_file_snapshot", [])?;
        tx.execute("DELETE FROM grounding_node_snapshot", [])?;
        tx.execute("DELETE FROM grounding_node_summary_snapshot", [])?;
        tx.execute("DELETE FROM grounding_node_edge_digest_snapshot", [])?;

        let node_snapshot_sql = format!(
            "WITH indexable_nodes AS (
                SELECT
                    n.id,
                    n.kind,
                    n.serialized_name,
                    n.qualified_name,
                    n.canonical_id,
                    n.file_node_id,
                    n.start_line,
                    n.start_col,
                    n.end_line,
                    n.end_col,
                    {display_name} AS display_name,
                    COALESCE(f.path, file_node.serialized_name) AS file_path,
                    {rank_sql} AS node_rank,
                    COALESCE(n.start_line, 2147483647) AS sort_start_line,
                    CASE
                        WHEN EXISTS (
                            SELECT 1
                            FROM edge e
                            WHERE e.kind = {member_kind}
                              AND e.target_node_id = n.id
                        ) THEN 0
                        ELSE 1
                    END AS is_root,
                    ROW_NUMBER() OVER (
                        PARTITION BY n.file_node_id
                        ORDER BY
                            {rank_sql},
                            COALESCE(n.start_line, 2147483647),
                            {display_name},
                            n.id
                    ) AS file_symbol_rank
                FROM node n
                LEFT JOIN file f ON f.id = n.file_node_id
                LEFT JOIN node file_node
                    ON file_node.id = n.file_node_id
                   AND file_node.kind = {file_kind}
                WHERE {indexable}
            )
            INSERT INTO grounding_node_snapshot (
                node_id,
                kind,
                serialized_name,
                qualified_name,
                canonical_id,
                file_node_id,
                start_line,
                start_col,
                end_line,
                end_col,
                display_name,
                file_path,
                node_rank,
                sort_start_line,
                is_root,
                file_symbol_rank
            )
            SELECT
                id,
                kind,
                serialized_name,
                qualified_name,
                canonical_id,
                file_node_id,
                start_line,
                start_col,
                end_line,
                end_col,
                display_name,
                file_path,
                node_rank,
                sort_start_line,
                is_root,
                file_symbol_rank
            FROM indexable_nodes",
            member_kind = EdgeKind::MEMBER as i32,
            file_kind = NodeKind::FILE as i32,
        );
        tx.execute(&node_snapshot_sql, [])?;

        let file_snapshot_sql = format!(
            "WITH all_files AS (
                SELECT id, path, language, modification_time, indexed, complete, line_count
                FROM file
                UNION ALL
                SELECT
                    n.id,
                    n.serialized_name,
                    '',
                    0,
                    1,
                    1,
                    0
                FROM node n
                WHERE n.kind = {file_kind}
                  AND NOT EXISTS (SELECT 1 FROM file f WHERE f.id = n.id)
            )
            INSERT INTO grounding_file_snapshot (
                file_id,
                path,
                language,
                modification_time,
                indexed,
                complete,
                line_count,
                symbol_count,
                best_node_rank
            )
            SELECT
                f.id,
                f.path,
                f.language,
                f.modification_time,
                f.indexed,
                f.complete,
                f.line_count,
                COUNT(gs.node_id) AS symbol_count,
                MIN(CASE WHEN gs.node_id IS NULL THEN 255 ELSE gs.node_rank END) AS best_node_rank
            FROM all_files f
            LEFT JOIN grounding_node_snapshot gs
              ON gs.file_node_id = f.id
            GROUP BY
                f.id,
                f.path,
                f.language,
                f.modification_time,
                f.indexed,
                f.complete,
                f.line_count",
            file_kind = NodeKind::FILE as i32,
        );
        tx.execute(&file_snapshot_sql, [])?;

        tx.execute(
            "UPDATE grounding_snapshot_meta
             SET snapshot_version = ?1,
                 summary_state = ?2,
                 detail_state = ?3,
                 summary_built_at_epoch_ms = ?4,
                 detail_built_at_epoch_ms = NULL
             WHERE id = 1",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                GroundingSnapshotState::Ready.db_value(),
                GroundingSnapshotState::Dirty.db_value(),
                current_epoch_ms(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn hydrate_grounding_detail_snapshots(&self) -> Result<(), StorageError> {
        if !self.has_ready_grounding_summary_snapshots()? {
            self.refresh_grounding_summary_snapshots()?;
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE grounding_snapshot_meta
             SET snapshot_version = ?1,
                 detail_state = ?2,
                 detail_built_at_epoch_ms = NULL
             WHERE id = 1",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                GroundingSnapshotState::Building.db_value()
            ],
        )?;
        tx.execute("DELETE FROM grounding_node_summary_snapshot", [])?;
        tx.execute("DELETE FROM grounding_node_edge_digest_snapshot", [])?;

        let node_summary_sql = format!(
            "WITH snapshot_nodes AS (
                SELECT node_id
                FROM grounding_node_snapshot
            ),
            member_counts AS (
                SELECT e.source_node_id AS node_id, COUNT(*) AS member_count
                FROM edge e
                JOIN snapshot_nodes snapshot_nodes
                  ON snapshot_nodes.node_id = e.source_node_id
                WHERE e.kind = {member_kind}
                GROUP BY e.source_node_id
            ),
            first_occurrences AS (
                SELECT o.element_id AS node_id, o.start_line
                FROM occurrence o
                JOIN (
                    SELECT o.element_id, MIN(o.rowid) AS first_rowid
                    FROM occurrence o
                    JOIN snapshot_nodes snapshot_nodes
                      ON snapshot_nodes.node_id = o.element_id
                    GROUP BY o.element_id
                ) first_seen
                  ON first_seen.first_rowid = o.rowid
            )
            INSERT INTO grounding_node_summary_snapshot (
                node_id,
                member_count,
                fallback_occurrence_line
            )
            SELECT
                snapshot_nodes.node_id,
                COALESCE(member_counts.member_count, 0),
                first_occurrences.start_line
            FROM snapshot_nodes
            LEFT JOIN member_counts
              ON member_counts.node_id = snapshot_nodes.node_id
            LEFT JOIN first_occurrences
              ON first_occurrences.node_id = snapshot_nodes.node_id",
            member_kind = EdgeKind::MEMBER as i32,
        );
        tx.execute(&node_summary_sql, [])?;

        let edge_digest_sql = format!(
            "WITH snapshot_nodes AS (
                SELECT node_id
                FROM grounding_node_snapshot
            ),
            edge_effective AS (
                SELECT
                    COALESCE(e.resolved_source_node_id, e.source_node_id) AS effective_source_node_id,
                    CASE
                        WHEN e.kind = {call_kind}
                         AND (
                            CASE
                                WHEN t.serialized_name LIKE '%seed_symbol_table%'
                                  OR t.serialized_name LIKE '%flush_projection_batch%'
                                  OR t.serialized_name LIKE '%flush_errors%' THEN 0
                                WHEN COALESCE(
                                    e.certainty,
                                    CASE
                                        WHEN e.confidence IS NULL THEN NULL
                                        WHEN e.confidence >= {certain_min} THEN 'certain'
                                        WHEN e.confidence >= {probable_min} THEN 'probable'
                                        ELSE 'uncertain'
                                    END
                                ) = 'uncertain' THEN 1
                                WHEN instr(t.serialized_name, '::') = 0
                                 AND instr(t.serialized_name, '.') = 0
                                 AND t.serialized_name IN (
                                    'add', 'all', 'any', 'append', 'clear', 'collect', 'contains',
                                    'dedup', 'extend', 'filter', 'insert', 'into_iter', 'iter',
                                    'iter_mut', 'len', 'map', 'pop', 'push', 'remove', 'retain',
                                    'sort', 'sort_by', 'sort_by_key', 'truncate'
                                 )
                                 AND COALESCE(
                                    e.certainty,
                                    CASE
                                        WHEN e.confidence IS NULL THEN NULL
                                        WHEN e.confidence >= {certain_min} THEN 'certain'
                                        WHEN e.confidence >= {probable_min} THEN 'probable'
                                        ELSE 'uncertain'
                                    END
                                 ) != 'certain' THEN 1
                                ELSE 0
                            END
                         ) = 1
                        THEN e.target_node_id
                        ELSE COALESCE(e.resolved_target_node_id, e.target_node_id)
                    END AS effective_target_node_id,
                    e.kind
                FROM edge e
                JOIN node t ON t.id = e.target_node_id
            ),
            per_endpoint AS (
                SELECT effective_source_node_id AS node_id, kind, effective_target_node_id
                FROM edge_effective
                UNION ALL
                SELECT effective_target_node_id AS node_id, kind, effective_source_node_id
                FROM edge_effective
                WHERE effective_target_node_id != effective_source_node_id
            ),
            filtered AS (
                SELECT per_endpoint.node_id, per_endpoint.kind
                FROM per_endpoint
                JOIN snapshot_nodes snapshot_nodes
                  ON snapshot_nodes.node_id = per_endpoint.node_id
            )
            INSERT INTO grounding_node_edge_digest_snapshot (node_id, kind, count)
            SELECT node_id, kind, COUNT(*)
            FROM filtered
            GROUP BY node_id, kind",
            call_kind = EdgeKind::CALL as i32,
            certain_min = ResolutionCertainty::CERTAIN_MIN,
            probable_min = ResolutionCertainty::PROBABLE_MIN,
        );
        tx.execute(&edge_digest_sql, [])?;

        tx.execute(
            "INSERT INTO grounding_repo_stats_snapshot (
                id,
                node_count,
                edge_count,
                file_count,
                error_count
             )
             SELECT
                1,
                (SELECT COUNT(*) FROM node),
                (SELECT COUNT(*) FROM edge),
                (SELECT COUNT(*) FROM grounding_file_snapshot),
                (SELECT COUNT(*) FROM error)
             ON CONFLICT(id) DO UPDATE SET
                node_count = excluded.node_count,
                edge_count = excluded.edge_count,
                file_count = excluded.file_count,
                error_count = excluded.error_count",
            [],
        )?;
        tx.execute(
            "UPDATE grounding_snapshot_meta
             SET snapshot_version = ?1,
                 detail_state = ?2,
                 detail_built_at_epoch_ms = ?3
             WHERE id = 1",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                GroundingSnapshotState::Ready.db_value(),
                current_epoch_ms()
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn refresh_grounding_snapshots(&self) -> Result<(), StorageError> {
        self.refresh_grounding_summary_snapshots()?;
        self.hydrate_grounding_detail_snapshots()
    }

    pub fn clear(&self) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM callable_projection_state", [])?;
        tx.execute("DELETE FROM occurrence", [])?;
        tx.execute("DELETE FROM edge", [])?;
        tx.execute("DELETE FROM llm_symbol_doc", [])?;
        tx.execute("DELETE FROM symbol_search_doc", [])?;
        tx.execute("DELETE FROM symbol_summary", [])?;
        tx.execute("DELETE FROM search_symbol_projection", [])?;
        tx.execute("DELETE FROM component_access", [])?;
        tx.execute("DELETE FROM bookmark_node", [])?;
        tx.execute("DELETE FROM local_symbol", [])?;
        tx.execute("DELETE FROM error", [])?;
        tx.execute("DELETE FROM node", [])?;
        tx.execute("DELETE FROM file", [])?;
        tx.execute("DELETE FROM grounding_repo_stats_snapshot", [])?;
        tx.execute("DELETE FROM grounding_file_snapshot", [])?;
        tx.execute("DELETE FROM grounding_node_snapshot", [])?;
        tx.execute("DELETE FROM grounding_node_summary_snapshot", [])?;
        tx.execute("DELETE FROM grounding_node_edge_digest_snapshot", [])?;
        tx.execute("DELETE FROM resolution_support_snapshot", [])?;
        tx.execute(
            "INSERT INTO grounding_snapshot_meta (
                id,
                snapshot_version,
                summary_state,
                detail_state,
                summary_built_at_epoch_ms,
                detail_built_at_epoch_ms
             )
             VALUES (1, ?1, ?2, ?2, NULL, NULL)
             ON CONFLICT(id) DO UPDATE SET
                snapshot_version = excluded.snapshot_version,
                summary_state = excluded.summary_state,
                detail_state = excluded.detail_state,
                summary_built_at_epoch_ms = NULL,
                detail_built_at_epoch_ms = NULL",
            params![
                GROUNDING_SNAPSHOT_VERSION,
                GroundingSnapshotState::Dirty.db_value()
            ],
        )?;
        tx.execute(
            "INSERT INTO resolution_support_snapshot (
                id,
                snapshot_version,
                state,
                snapshot_blob,
                built_at_epoch_ms
             )
             VALUES (1, 0, ?1, NULL, NULL)",
            params![GroundingSnapshotState::Dirty.db_value()],
        )?;
        tx.commit()?;

        self.cache.nodes.write().clear();
        Ok(())
    }

    pub fn rebase_rehydrated_path_bound_cache(
        &mut self,
        source_root: &Path,
        target_root: &Path,
    ) -> Result<(usize, usize), StorageError> {
        let source_root = source_root.to_string_lossy().to_string();
        let target_root = target_root.to_string_lossy().to_string();
        let mut updated = self.rebase_path_bound_text_columns(&source_root, &target_root)?;
        updated = updated.saturating_add(self.refresh_rebased_file_metadata()?);
        let invalidated_artifacts = self.clear_legacy_index_artifact_cache()?;
        self.cache.nodes.write().clear();
        Ok((updated, invalidated_artifacts))
    }

    fn rebase_path_bound_text_columns(
        &self,
        source_root: &str,
        target_root: &str,
    ) -> Result<usize, StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut updated = 0usize;
        for statement in [
            "UPDATE file
             SET path = replace(path, ?1, ?2)
             WHERE instr(path, ?1) > 0",
            "UPDATE node
             SET
                serialized_name = replace(serialized_name, ?1, ?2),
                qualified_name = replace(qualified_name, ?1, ?2),
                canonical_id = replace(canonical_id, ?1, ?2)
             WHERE instr(serialized_name, ?1) > 0
                OR instr(COALESCE(qualified_name, ''), ?1) > 0
                OR instr(COALESCE(canonical_id, ''), ?1) > 0",
            "UPDATE edge
             SET callsite_identity = replace(callsite_identity, ?1, ?2)
             WHERE instr(COALESCE(callsite_identity, ''), ?1) > 0",
            "UPDATE callable_projection_state
             SET symbol_key = replace(symbol_key, ?1, ?2)
             WHERE instr(symbol_key, ?1) > 0",
            "UPDATE error
             SET message = replace(message, ?1, ?2)
             WHERE instr(message, ?1) > 0",
            "UPDATE llm_symbol_doc
             SET
                display_name = replace(display_name, ?1, ?2),
                qualified_name = replace(qualified_name, ?1, ?2),
                file_path = replace(file_path, ?1, ?2),
                doc_text = replace(doc_text, ?1, ?2)
             WHERE instr(display_name, ?1) > 0
                OR instr(COALESCE(qualified_name, ''), ?1) > 0
                OR instr(COALESCE(file_path, ''), ?1) > 0
                OR instr(doc_text, ?1) > 0",
            "UPDATE symbol_search_doc
             SET
                display_name = replace(display_name, ?1, ?2),
                qualified_name = replace(qualified_name, ?1, ?2),
                file_path = replace(file_path, ?1, ?2),
                doc_text = replace(doc_text, ?1, ?2),
                source_provenance = replace(source_provenance, ?1, ?2)
             WHERE instr(display_name, ?1) > 0
                OR instr(COALESCE(qualified_name, ''), ?1) > 0
                OR instr(COALESCE(file_path, ''), ?1) > 0
                OR instr(doc_text, ?1) > 0
                OR instr(source_provenance, ?1) > 0",
            "UPDATE search_symbol_projection
             SET display_name = replace(display_name, ?1, ?2)
             WHERE instr(display_name, ?1) > 0",
            "UPDATE grounding_file_snapshot
             SET path = replace(path, ?1, ?2)
             WHERE instr(path, ?1) > 0",
            "UPDATE grounding_node_snapshot
             SET
                serialized_name = replace(serialized_name, ?1, ?2),
                qualified_name = replace(qualified_name, ?1, ?2),
                canonical_id = replace(canonical_id, ?1, ?2),
                display_name = replace(display_name, ?1, ?2),
                file_path = replace(file_path, ?1, ?2)
             WHERE instr(serialized_name, ?1) > 0
                OR instr(COALESCE(qualified_name, ''), ?1) > 0
                OR instr(COALESCE(canonical_id, ''), ?1) > 0
                OR instr(display_name, ?1) > 0
                OR instr(COALESCE(file_path, ''), ?1) > 0",
        ] {
            updated =
                updated.saturating_add(tx.execute(statement, params![source_root, target_root])?);
        }
        tx.commit()?;
        Ok(updated)
    }

    fn clear_legacy_index_artifact_cache(&self) -> Result<usize, StorageError> {
        // ponytail: v2 artifact keys are root-portable; delete this cleanup when migrations no longer see pre-v2 caches.
        Ok(self.conn.execute(
            "DELETE FROM index_artifact_cache
             WHERE cache_key NOT LIKE 'v2:%'",
            [],
        )?)
    }

    fn refresh_rebased_file_metadata(&self) -> Result<usize, StorageError> {
        let files = self.get_files()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut updated = 0usize;
        for file in files {
            let Ok(metadata) = fs::metadata(&file.path) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let Ok(duration) = modified.duration_since(UNIX_EPOCH) else {
                continue;
            };
            let modification_time = duration.as_millis().min(i64::MAX as u128) as i64;
            updated = updated.saturating_add(tx.execute(
                "UPDATE file
                 SET modification_time = ?2
                 WHERE id = ?1",
                params![file.id, modification_time],
            )?);
        }
        tx.commit()?;
        Ok(updated)
    }

    pub fn path_bound_text_match_count(&self, prefix: &str) -> Result<usize, StorageError> {
        let pattern = format!("%{prefix}%");
        let mut count = 0usize;
        for sql in [
            "SELECT COUNT(*) FROM file WHERE path LIKE ?1",
            "SELECT COUNT(*) FROM node WHERE serialized_name LIKE ?1 OR qualified_name LIKE ?1 OR canonical_id LIKE ?1",
            "SELECT COUNT(*) FROM edge WHERE callsite_identity LIKE ?1",
            "SELECT COUNT(*) FROM callable_projection_state WHERE symbol_key LIKE ?1",
            "SELECT COUNT(*) FROM error WHERE message LIKE ?1",
            "SELECT COUNT(*) FROM llm_symbol_doc WHERE display_name LIKE ?1 OR qualified_name LIKE ?1 OR file_path LIKE ?1 OR doc_text LIKE ?1",
            "SELECT COUNT(*) FROM symbol_search_doc WHERE display_name LIKE ?1 OR qualified_name LIKE ?1 OR file_path LIKE ?1 OR doc_text LIKE ?1 OR source_provenance LIKE ?1",
            "SELECT COUNT(*) FROM search_symbol_projection WHERE display_name LIKE ?1",
            "SELECT COUNT(*) FROM grounding_file_snapshot WHERE path LIKE ?1",
            "SELECT COUNT(*) FROM grounding_node_snapshot WHERE serialized_name LIKE ?1 OR qualified_name LIKE ?1 OR canonical_id LIKE ?1 OR display_name LIKE ?1 OR file_path LIKE ?1",
        ] {
            let matched: i64 = self
                .conn
                .query_row(sql, params![pattern], |row| row.get(0))?;
            count = count.saturating_add(matched.max(0) as usize);
        }
        Ok(count)
    }

    pub fn finalize_staged_snapshot(&self) -> Result<(), StorageError> {
        self.refresh_grounding_summary_snapshots()?;
        if self.deferred_secondary_indexes {
            schema::create_deferred_indexes(&self.conn)?;
        }
        Ok(())
    }

    pub fn create_deferred_secondary_indexes(&self) -> Result<(), StorageError> {
        if self.deferred_secondary_indexes {
            schema::create_deferred_indexes(&self.conn)?;
        }
        Ok(())
    }

    pub fn promote_staged_snapshot(
        staged_path: &Path,
        live_path: &Path,
    ) -> Result<(), StorageError> {
        let _promotion_lock = PromotionLock::acquire(live_path)?;
        recover_interrupted_promotion_locked(live_path)?;
        if promotion_artifacts_exist(live_path) {
            return Err(promotion_error(format!(
                "Cannot start a new promotion while prior artifacts remain for {}",
                live_path.display()
            )));
        }
        let backup_path = live_path.with_extension("sqlite.backup");
        let prepared_path = promotion_prepared_journal_path(live_path);
        let committed_path = promotion_committed_journal_path(live_path);
        let candidate = require_complete_promotion_database_identity(
            staged_path,
            "Staged promotion candidate",
        )?;
        let previous = read_recovery_database_identity(live_path)?;
        cleanup_sqlite_sidecars(&backup_path)?;

        if previous.is_some() {
            let live_conn = Connection::open(live_path)?;
            let _ = live_conn.busy_timeout(Duration::from_millis(2_500));
            live_conn.backup(
                MAIN_DB,
                &backup_path,
                None::<fn(rusqlite::backup::Progress)>,
            )?;
            drop(live_conn);
            let backup_identity =
                require_recovery_database_identity(&backup_path, "Promotion backup")?;
            if Some(&backup_identity) != previous.as_ref() {
                return Err(promotion_error(format!(
                    "Promotion backup identity does not match live database {}",
                    live_path.display()
                )));
            }
        }

        let prepared = PromotionJournal {
            version: PROMOTION_JOURNAL_VERSION,
            previous: previous.clone(),
            candidate: candidate.clone(),
        };
        if let Err(error) = write_promotion_journal(&prepared_path, &prepared) {
            if !prepared_path.exists() {
                let _ = cleanup_sqlite_sidecars(&backup_path);
            }
            return Err(error);
        }

        let mut live_conn = Connection::open(live_path)?;
        let _ = live_conn.busy_timeout(Duration::from_millis(2_500));

        #[cfg(test)]
        let restore_result = if let Some(sentinel_path) =
            std::env::var_os(PROMOTION_ABORT_SENTINEL_ENV).map(PathBuf::from)
        {
            live_conn.restore(
                MAIN_DB,
                staged_path,
                Some(move |_progress| {
                    let mut sentinel = std::fs::File::create(&sentinel_path)
                        .expect("create promotion abort sentinel");
                    sentinel
                        .write_all(PROMOTION_ABORT_SENTINEL)
                        .expect("write promotion abort sentinel");
                    sentinel.sync_all().expect("sync promotion abort sentinel");
                    std::process::abort();
                }),
            )
        } else {
            live_conn.restore(MAIN_DB, staged_path, None::<fn(rusqlite::backup::Progress)>)
        };
        #[cfg(not(test))]
        let restore_result =
            live_conn.restore(MAIN_DB, staged_path, None::<fn(rusqlite::backup::Progress)>);

        if let Err(err) = restore_result {
            drop(live_conn);
            let _ = rollback_prepared_promotion(live_path, &prepared);
            return Err(StorageError::Other(format!(
                "Failed to promote staged snapshot {} -> {}: {err}",
                staged_path.display(),
                live_path.display()
            )));
        }
        drop(live_conn);

        let published =
            require_complete_promotion_database_identity(live_path, "Promoted live database")?;
        if published != candidate {
            let _ = rollback_prepared_promotion(live_path, &prepared);
            return Err(promotion_error(format!(
                "Promoted live database identity does not match staged candidate {}",
                staged_path.display()
            )));
        }

        if let Err(error) = commit_promotion_journal(&prepared_path, &committed_path) {
            if !committed_path.exists() {
                let _ = rollback_prepared_promotion(live_path, &prepared);
            }
            return Err(error);
        }

        if let Err(error) = cleanup_sqlite_sidecars(staged_path) {
            tracing::warn!(
                staged_path = %staged_path.display(),
                error = %error,
                "committed promotion left a staged cleanup artifact"
            );
        }
        if let Err(error) = cleanup_committed_promotion_artifacts(live_path) {
            tracing::warn!(
                live_path = %live_path.display(),
                error = %error,
                "committed promotion retained recovery artifacts"
            );
        }
        Ok(())
    }

    pub fn discard_staged_snapshot(staged_path: &Path) -> Result<(), StorageError> {
        cleanup_sqlite_sidecars(staged_path)
    }

    fn init(&self, _mode: StorageOpenMode) -> Result<(), StorageError> {
        self.create_tables()?;
        if self.schema_version()? == 0 {
            self.set_schema_version(SCHEMA_VERSION)?;
        }
        self.apply_schema_migrations()
    }

    fn create_tables(&self) -> Result<(), StorageError> {
        schema::create_tables(&self.conn)
    }

    fn schema_version(&self) -> Result<u32, StorageError> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(version.max(0) as u32)
    }

    fn set_schema_version(&self, version: u32) -> Result<(), StorageError> {
        self.conn
            .pragma_update(None, "user_version", version.to_string())?;
        Ok(())
    }

    fn apply_schema_migrations(&self) -> Result<(), StorageError> {
        schema::apply_schema_migrations(self)
    }

    fn grounding_snapshot_metadata(
        &self,
    ) -> Result<Option<GroundingSnapshotMetadata>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                snapshot_version,
                summary_state,
                detail_state,
                summary_built_at_epoch_ms,
                detail_built_at_epoch_ms
             FROM grounding_snapshot_meta
             WHERE id = 1",
        )?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let version: i64 = row.get(0)?;
            let raw_summary_state: i64 = row.get(1)?;
            let raw_detail_state: i64 = row.get(2)?;
            let Some(summary_state) = GroundingSnapshotState::from_db(raw_summary_state) else {
                return Ok(None);
            };
            let Some(detail_state) = GroundingSnapshotState::from_db(raw_detail_state) else {
                return Ok(None);
            };
            return Ok(Some(GroundingSnapshotMetadata {
                version,
                summary_state,
                detail_state,
                summary_built_at_epoch_ms: row.get(3)?,
                detail_built_at_epoch_ms: row.get(4)?,
            }));
        }
        Ok(None)
    }

    fn effective_grounding_file_count(&self) -> Result<i64, StorageError> {
        self.conn
            .query_row(
                "WITH all_files AS (
                    SELECT id
                    FROM file
                    UNION
                    SELECT n.id
                    FROM node n
                    WHERE n.kind = ?1
                )
                SELECT COUNT(*)
                FROM all_files",
                params![NodeKind::FILE as i32],
                |row| row.get(0),
            )
            .map_err(StorageError::from)
    }

    fn grounding_file_summary_from_row(row: &Row) -> Result<GroundingFileSummary, StorageError> {
        Ok(GroundingFileSummary {
            file: FileInfo {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                language: row.get(2)?,
                modification_time: row.get(3)?,
                indexed: row.get::<_, i32>(4)? != 0,
                complete: row.get::<_, i32>(5)? != 0,
                line_count: row.get(6)?,
                file_role: FileRole::Source,
            },
            symbol_count: clamp_i64_to_u32(row.get::<_, i64>(7)?),
            best_node_rank: row.get::<_, i64>(8)?.min(u8::MAX as i64) as u8,
        })
    }

    fn node_from_row(row: &Row) -> Result<Node, StorageError> {
        row_mapping::node_from_row(row)
    }

    fn edge_from_row(row: &Row) -> Result<Edge, StorageError> {
        row_mapping::edge_from_row(row)
    }

    fn occurrence_from_row(row: &Row) -> rusqlite::Result<Occurrence> {
        row_mapping::occurrence_from_row(row)
    }

    fn insert_node_with_stmt(
        stmt: &mut rusqlite::Statement<'_>,
        node: &Node,
    ) -> rusqlite::Result<usize> {
        stmt.execute(params![
            node.id.0,
            node.kind as i32,
            node.serialized_name,
            node.qualified_name,
            node.canonical_id,
            node.file_node_id.map(|id| id.0),
            node.start_line,
            node.start_col,
            node.end_line,
            node.end_col
        ])
    }

    fn prepared_nodes_for_insert(&self, nodes: &[Node]) -> Result<Vec<Node>, StorageError> {
        self.prepared_nodes_for_insert_with_files(nodes, &[])
    }

    fn prepared_nodes_for_insert_with_files(
        &self,
        nodes: &[Node],
        files: &[FileInfo],
    ) -> Result<Vec<Node>, StorageError> {
        let mut batch_file_paths = files
            .iter()
            .map(|info| (NodeId(info.id), info.path.clone()))
            .collect::<HashMap<_, _>>();
        batch_file_paths.extend(
            nodes
                .iter()
                .filter(|node| node.kind == NodeKind::FILE)
                .map(|node| (node.id, PathBuf::from(&node.serialized_name))),
        );
        let mut prepared = Vec::new();
        let mut framework_nodes = HashMap::<NodeId, Node>::new();

        for node in nodes {
            if !is_framework_synthetic_node(node) {
                prepared.push(node.clone());
                continue;
            }

            let candidate = if let Some(existing) = framework_nodes.get(&node.id) {
                preferred_framework_node(
                    &self.conn,
                    &batch_file_paths,
                    existing.clone(),
                    node.clone(),
                )?
            } else if let Some(existing) = Self::node_by_id_from_conn(&self.conn, node.id)? {
                preferred_framework_node(&self.conn, &batch_file_paths, existing, node.clone())?
            } else {
                node.clone()
            };
            framework_nodes.insert(node.id, candidate);
        }

        prepared.extend(framework_nodes.into_values());
        Ok(prepared)
    }

    fn node_by_id_from_conn(conn: &Connection, id: NodeId) -> Result<Option<Node>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id.0])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::node_from_row(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn insert_node(&self, node: &Node) -> Result<(), StorageError> {
        let prepared = self
            .prepared_nodes_for_insert(std::slice::from_ref(node))?
            .into_iter()
            .next()
            .unwrap_or_else(|| node.clone());
        self.conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                serialized_name = excluded.serialized_name,
                qualified_name = excluded.qualified_name,
                canonical_id = excluded.canonical_id,
                file_node_id = excluded.file_node_id,
                start_line = excluded.start_line,
                start_col = excluded.start_col,
                end_line = excluded.end_line,
                end_col = excluded.end_col",
            params![
                prepared.id.0,
                prepared.kind as i32,
                &prepared.serialized_name,
                prepared.qualified_name.as_deref(),
                prepared.canonical_id.as_deref(),
                prepared.file_node_id.map(|id| id.0),
                prepared.start_line,
                prepared.start_col,
                prepared.end_line,
                prepared.end_col
            ],
        )?;
        // Update cache
        self.cache.nodes.write().insert(prepared.id, prepared);
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    pub fn insert_edge(&self, edge: &Edge) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO edge (id, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence, callsite_identity, certainty, candidate_target_node_ids)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT(id) DO NOTHING",
            params![
                edge.id.0,
                edge.source.0,
                edge.target.0,
                edge.kind as i32,
                edge.file_node_id.map(|id| id.0),
                edge.line,
                edge.resolved_source.map(|id| id.0),
                edge.resolved_target.map(|id| id.0),
                edge.confidence,
                edge.callsite_identity.as_deref(),
                row_mapping::certainty_db_value(edge.certainty),
                serialize_candidate_targets(&edge.candidate_targets)?
            ],
        )?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    // Batch operations
    pub fn insert_nodes_batch(&mut self, nodes: &[Node]) -> Result<(), StorageError> {
        let prepared_nodes = self.prepared_nodes_for_insert(nodes)?;
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    kind = excluded.kind,
                    serialized_name = excluded.serialized_name,
                    qualified_name = excluded.qualified_name,
                    canonical_id = excluded.canonical_id,
                    file_node_id = excluded.file_node_id,
                    start_line = excluded.start_line,
                    start_col = excluded.start_col,
                    end_line = excluded.end_line,
                    end_col = excluded.end_col",
            )?;
            // Insert FILE nodes first so foreign keys to file_node_id are satisfied.
            for node in prepared_nodes
                .iter()
                .filter(|node| node.kind == NodeKind::FILE)
                .chain(
                    prepared_nodes
                        .iter()
                        .filter(|node| node.kind != NodeKind::FILE),
                )
            {
                Self::insert_node_with_stmt(&mut stmt, node)?;
            }
        }
        tx.commit()?;

        // Update cache
        let mut cache = self.cache.nodes.write();
        for node in &prepared_nodes {
            cache.insert(node.id, node.clone());
        }

        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    pub fn upsert_retrieval_artifact_nodes_batch(
        &mut self,
        nodes: &[Node],
    ) -> Result<(), StorageError> {
        let prepared_nodes = self.prepared_nodes_for_insert(nodes)?;
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    kind = excluded.kind,
                    serialized_name = excluded.serialized_name,
                    qualified_name = excluded.qualified_name,
                    canonical_id = excluded.canonical_id,
                    file_node_id = excluded.file_node_id,
                    start_line = excluded.start_line,
                    start_col = excluded.start_col,
                    end_line = excluded.end_line,
                    end_col = excluded.end_col",
            )?;
            for node in &prepared_nodes {
                Self::insert_node_with_stmt(&mut stmt, node)?;
            }
        }
        tx.commit()?;

        let mut cache = self.cache.nodes.write();
        for node in &prepared_nodes {
            cache.insert(node.id, node.clone());
        }

        Ok(())
    }

    /// Remove stale generated retrieval artifacts and their semantic projections.
    /// Returns the number of removed projection rows.
    pub fn prune_retrieval_artifacts_to_node_ids(
        &mut self,
        keep_node_ids: &[NodeId],
        keep_dense_node_ids: &[NodeId],
    ) -> Result<usize, StorageError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS retrieval_artifact_keep (
                node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute("DELETE FROM temp.retrieval_artifact_keep", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO temp.retrieval_artifact_keep (node_id) VALUES (?1)",
            )?;
            for node_id in keep_node_ids {
                stmt.execute(params![node_id.0])?;
            }
        }
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS retrieval_artifact_dense_keep (
                node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute("DELETE FROM temp.retrieval_artifact_dense_keep", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO temp.retrieval_artifact_dense_keep (node_id) VALUES (?1)",
            )?;
            for node_id in keep_dense_node_ids {
                stmt.execute(params![node_id.0])?;
            }
        }
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS retrieval_artifact_stale (
                node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute("DELETE FROM temp.retrieval_artifact_stale", [])?;
        tx.execute(
            "INSERT INTO temp.retrieval_artifact_stale (node_id)
             SELECT id FROM node
             WHERE (serialized_name LIKE 'component_report:%'
                OR canonical_id LIKE 'codestory:component_report:%')
               AND NOT EXISTS (
                   SELECT 1 FROM temp.retrieval_artifact_keep keep
                   WHERE keep.node_id = node.id
               )",
            [],
        )?;
        let stale_node_ids = {
            let mut stmt = tx.prepare("SELECT node_id FROM temp.retrieval_artifact_stale")?;
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let removed_dense = tx.execute(
            "DELETE FROM llm_symbol_doc
             WHERE node_id IN (
                 SELECT id FROM node
                 WHERE serialized_name LIKE 'component_report:%'
                    OR canonical_id LIKE 'codestory:component_report:%'
             )
               AND NOT EXISTS (
                   SELECT 1 FROM temp.retrieval_artifact_dense_keep keep
                   WHERE keep.node_id = llm_symbol_doc.node_id
               )",
            [],
        )?;
        let removed_symbol = tx.execute(
            "DELETE FROM symbol_search_doc
             WHERE node_id IN (SELECT node_id FROM temp.retrieval_artifact_stale)",
            [],
        )?;
        tx.execute(
            "DELETE FROM search_symbol_projection
             WHERE node_id IN (SELECT node_id FROM temp.retrieval_artifact_stale)",
            [],
        )?;
        tx.execute(
            "DELETE FROM symbol_summary
             WHERE node_id IN (SELECT node_id FROM temp.retrieval_artifact_stale)",
            [],
        )?;
        tx.execute(
            "DELETE FROM bookmark_node
             WHERE node_id IN (SELECT node_id FROM temp.retrieval_artifact_stale)",
            [],
        )?;
        tx.execute(
            "DELETE FROM node
             WHERE id IN (SELECT node_id FROM temp.retrieval_artifact_stale)",
            [],
        )?;
        tx.execute("DROP TABLE temp.retrieval_artifact_stale", [])?;
        tx.execute("DROP TABLE temp.retrieval_artifact_dense_keep", [])?;
        tx.execute("DROP TABLE temp.retrieval_artifact_keep", [])?;
        tx.commit()?;

        let mut cache = self.cache.nodes.write();
        for node_id in stale_node_ids {
            cache.remove(&NodeId(node_id));
        }
        drop(cache);
        self.invalidate_grounding_snapshots()?;
        Ok(removed_dense.saturating_add(removed_symbol))
    }

    pub fn copy_retrieval_artifact_nodes_from(
        &mut self,
        source_path: &Path,
    ) -> Result<usize, StorageError> {
        if !source_path.exists() {
            return Ok(0);
        }
        drop(Storage::open(source_path)?);
        let source = source_path.to_string_lossy().to_string();
        self.conn
            .execute("ATTACH DATABASE ?1 AS source_snapshot", params![source])?;
        let copy_result = self.conn.execute(
            "INSERT OR REPLACE INTO node (
                id,
                kind,
                serialized_name,
                qualified_name,
                canonical_id,
                file_node_id,
                start_line,
                start_col,
                end_line,
                end_col
             )
             SELECT
                source_node.id,
                source_node.kind,
                source_node.serialized_name,
                source_node.qualified_name,
                source_node.canonical_id,
                source_node.file_node_id,
                source_node.start_line,
                source_node.start_col,
                source_node.end_line,
                source_node.end_col
             FROM source_snapshot.node source_node
             WHERE source_node.serialized_name LIKE 'component_report:%'
                OR source_node.canonical_id LIKE 'codestory:component_report:%'",
            [],
        );
        let detach_result = self.conn.execute("DETACH DATABASE source_snapshot", []);
        let copied = copy_result?;
        detach_result?;
        Ok(copied)
    }

    pub fn insert_edges_batch(&mut self, edges: &[Edge]) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO edge (id, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence, callsite_identity, certainty, candidate_target_node_ids)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT(id) DO NOTHING"
            )?;
            for edge in edges {
                stmt.execute(params![
                    edge.id.0,
                    edge.source.0,
                    edge.target.0,
                    edge.kind as i32,
                    edge.file_node_id.map(|id| id.0),
                    edge.line,
                    edge.resolved_source.map(|id| id.0),
                    edge.resolved_target.map(|id| id.0),
                    edge.confidence,
                    edge.callsite_identity.as_deref(),
                    row_mapping::certainty_db_value(edge.certainty),
                    serialize_candidate_targets(&edge.candidate_targets)?
                ])?;
            }
        }
        tx.commit()?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    pub fn insert_occurrences_batch(
        &mut self,
        occurrences: &[Occurrence],
    ) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO occurrence (element_id, kind, file_node_id, start_line, start_col, end_line, end_col) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            )?;
            for occ in occurrences {
                stmt.execute(params![
                    occ.element_id,
                    occ.kind as i32,
                    occ.location.file_node_id.0,
                    occ.location.start_line,
                    occ.location.start_col,
                    occ.location.end_line,
                    occ.location.end_col,
                ])?;
            }
        }
        tx.commit()?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }
    pub fn get_node_count(&self) -> Result<i64, StorageError> {
        let mut stmt = self.conn.prepare("SELECT count(*) FROM node")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;
        Ok(count)
    }

    pub fn get_edge_count(&self) -> Result<i64, StorageError> {
        let mut stmt = self.conn.prepare("SELECT count(*) FROM edge")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;
        Ok(count)
    }

    pub fn get_nodes(&self) -> Result<Vec<Node>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node")?;
        let mut nodes = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            nodes.push(Self::node_from_row(row)?);
        }
        Ok(nodes)
    }

    pub fn get_edges(&self) -> Result<Vec<Edge>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence, callsite_identity, certainty, candidate_target_node_ids FROM edge")?;
        let mut edges = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            edges.push(Self::edge_from_row(row)?);
        }
        Ok(edges)
    }

    pub fn get_edges_for_node_ids(
        &self,
        node_ids: &[NodeId],
    ) -> Result<HashMap<NodeId, Vec<Edge>>, StorageError> {
        if node_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut unique_node_ids = Vec::new();
        let mut seen_node_ids = HashSet::new();
        for node_id in node_ids {
            if seen_node_ids.insert(*node_id) {
                unique_node_ids.push(*node_id);
            }
        }

        let mut edges_by_node = unique_node_ids
            .iter()
            .copied()
            .map(|node_id| (node_id, Vec::new()))
            .collect::<HashMap<_, _>>();

        for chunk in unique_node_ids.chunks(EDGE_NODE_LOOKUP_BATCH_SIZE) {
            let source_placeholders = numbered_placeholders(1, chunk.len());
            let target_placeholders = numbered_placeholders(1 + chunk.len(), chunk.len());
            let resolved_source_placeholders =
                numbered_placeholders(1 + chunk.len() * 2, chunk.len());
            let resolved_target_placeholders =
                numbered_placeholders(1 + chunk.len() * 3, chunk.len());
            let query = format!(
                "{EDGE_SELECT_BASE}
                 WHERE e.source_node_id IN ({source_placeholders})
                    OR e.target_node_id IN ({target_placeholders})
                    OR e.resolved_source_node_id IN ({resolved_source_placeholders})
                    OR e.resolved_target_node_id IN ({resolved_target_placeholders})
                 ORDER BY e.id"
            );
            let params = chunk
                .iter()
                .map(|id| Value::from(id.0))
                .chain(chunk.iter().map(|id| Value::from(id.0)))
                .chain(chunk.iter().map(|id| Value::from(id.0)))
                .chain(chunk.iter().map(|id| Value::from(id.0)));
            let mut stmt = self.conn.prepare(&query)?;
            let mut rows = stmt.query(params_from_iter(params))?;
            let chunk_node_ids = chunk.iter().copied().collect::<HashSet<_>>();
            while let Some(row) = rows.next()? {
                let mut edge = Self::edge_from_row(row)?;
                let target_symbol: String = row.get(12)?;
                if edge.kind == EdgeKind::CALL
                    && edge.resolved_target.is_some()
                    && should_ignore_call_resolution(
                        &target_symbol,
                        edge.certainty,
                        edge.confidence,
                    )
                {
                    edge.resolved_target = None;
                    edge.confidence = None;
                    edge.certainty = None;
                }

                let (source, target) = edge.effective_endpoints();
                if chunk_node_ids.contains(&source)
                    && let Some(edges) = edges_by_node.get_mut(&source)
                {
                    edges.push(edge.clone());
                }
                if target != source
                    && chunk_node_ids.contains(&target)
                    && let Some(edges) = edges_by_node.get_mut(&target)
                {
                    edges.push(edge);
                }
            }
        }

        Ok(edges_by_node)
    }

    pub fn get_present_node_kinds(&self) -> Result<Vec<NodeKind>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT kind FROM node ORDER BY kind ASC")?;
        let mut kinds = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let raw: i32 = row.get(0)?;
            if let Ok(kind) = NodeKind::try_from(raw) {
                kinds.push(kind);
            }
        }
        Ok(kinds)
    }

    pub fn get_present_edge_kinds(&self) -> Result<Vec<EdgeKind>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT kind FROM edge ORDER BY kind ASC")?;
        let mut kinds = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let raw: i32 = row.get(0)?;
            if let Ok(kind) = EdgeKind::try_from(raw) {
                kinds.push(kind);
            }
        }
        Ok(kinds)
    }

    pub fn insert_component_access_batch(
        &mut self,
        entries: &[(NodeId, AccessKind)],
    ) -> Result<(), StorageError> {
        if entries.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO component_access (node_id, type)
                 VALUES (?1, ?2)
                 ON CONFLICT(node_id) DO UPDATE SET type = excluded.type",
            )?;
            for (node_id, access) in entries {
                stmt.execute(params![
                    node_id.0,
                    row_mapping::access_kind_db_value(*access)
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_callable_projection_states_for_file(
        &self,
        file_id: i64,
    ) -> Result<Vec<CallableProjectionState>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT file_id, symbol_key, node_id, signature_hash, body_hash, start_line, end_line
             FROM callable_projection_state
             WHERE file_id = ?1
             ORDER BY start_line, symbol_key",
        )?;
        let rows = stmt.query_map(params![file_id], |row| {
            Ok(CallableProjectionState {
                file_id: row.get(0)?,
                symbol_key: row.get(1)?,
                node_id: NodeId(row.get(2)?),
                signature_hash: row.get(3)?,
                body_hash: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_callable_projection_states(
        &mut self,
        states: &[CallableProjectionState],
    ) -> Result<(), StorageError> {
        if states.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO callable_projection_state (
                    file_id, symbol_key, node_id, signature_hash, body_hash, start_line, end_line
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(file_id, symbol_key) DO UPDATE SET
                    node_id = excluded.node_id,
                    signature_hash = excluded.signature_hash,
                    body_hash = excluded.body_hash,
                    start_line = excluded.start_line,
                    end_line = excluded.end_line",
            )?;
            for state in states {
                stmt.execute(params![
                    state.file_id,
                    state.symbol_key,
                    state.node_id.0,
                    state.signature_hash,
                    state.body_hash,
                    state.start_line,
                    state.end_line
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Persist one coherent set of file, graph, occurrence, and projection
    /// rows.
    ///
    /// File-scoped errors for refreshed files are cleared before insertion. A
    /// successful flush updates graph/search tables only; callers that maintain
    /// derived grounding snapshots must invalidate or refresh them separately.
    pub fn flush_projection_batch(
        &mut self,
        batch: ProjectionBatch<'_>,
    ) -> Result<ProjectionFlushBreakdown, StorageError> {
        let mut breakdown = ProjectionFlushBreakdown::default();
        if batch.files.is_empty()
            && batch.file_content_hashes.is_empty()
            && batch.nodes.is_empty()
            && batch.edges.is_empty()
            && batch.occurrences.is_empty()
            && batch.component_access.is_empty()
            && batch.callable_projection_states.is_empty()
        {
            return Ok(breakdown);
        }

        let nodes_prepare_started = std::time::Instant::now();
        let prepared_nodes = if batch.nodes.is_empty() {
            Vec::new()
        } else {
            self.prepared_nodes_for_insert_with_files(batch.nodes, batch.files)?
        };
        let pending_node_labels = prepared_nodes
            .iter()
            .map(|node| (node.id, format!("{:?}:{}", node.kind, node.serialized_name)))
            .collect::<HashMap<_, _>>();
        let nodes_prepare_ms = clamp_i64_to_u32(nodes_prepare_started.elapsed().as_millis() as i64);

        let file_content_hashes = batch
            .file_content_hashes
            .iter()
            .map(|identity| (identity.file_id, identity.content_hash.as_str()))
            .collect::<HashMap<_, _>>();
        let tx = self.conn.transaction()?;

        if !batch.files.is_empty() {
            let placeholders = question_placeholders(batch.files.len());
            tx.execute(
                &format!("DELETE FROM error WHERE file_id IN ({placeholders})"),
                params_from_iter(batch.files.iter().map(|file| file.id)),
            )?;

            let started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count, file_role, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    modification_time=excluded.modification_time,
                    indexed=excluded.indexed,
                    complete=excluded.complete,
                    line_count=excluded.line_count,
                    file_role=excluded.file_role,
                    content_hash=excluded.content_hash",
            )?;
            for info in batch.files {
                stmt.execute(params![
                    info.id,
                    info.path.to_string_lossy(),
                    info.language,
                    info.modification_time,
                    i32::from(info.indexed),
                    i32::from(info.complete),
                    info.line_count,
                    info.file_role.as_str(),
                    file_content_hashes.get(&info.id).copied(),
                ])?;
            }
            breakdown.files_ms = clamp_i64_to_u32(started.elapsed().as_millis() as i64);
        }

        if !prepared_nodes.is_empty() {
            let nodes_insert_started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    kind = excluded.kind,
                    serialized_name = excluded.serialized_name,
                    qualified_name = excluded.qualified_name,
                    canonical_id = excluded.canonical_id,
                    file_node_id = excluded.file_node_id,
                    start_line = excluded.start_line,
                    start_col = excluded.start_col,
                    end_line = excluded.end_line,
                    end_col = excluded.end_col",
            )?;
            for node in prepared_nodes
                .iter()
                .filter(|node| node.kind == NodeKind::FILE)
                .chain(
                    prepared_nodes
                        .iter()
                        .filter(|node| node.kind != NodeKind::FILE),
                )
            {
                Self::insert_node_with_stmt(&mut stmt, node).map_err(|err| {
                    StorageError::Other(format!(
                        "flush_projection_batch node insert failed for id={} kind={:?} name={} file_node_id={:?}: {err}",
                        node.id.0, node.kind, node.serialized_name, node.file_node_id.map(|id| id.0)
                    ))
                })?;
            }
            breakdown.nodes_ms = nodes_prepare_ms.saturating_add(clamp_i64_to_u32(
                nodes_insert_started.elapsed().as_millis() as i64,
            ));
        }

        if !batch.edges.is_empty() {
            let started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT INTO edge (id, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence, callsite_identity, certainty, candidate_target_node_ids)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT(id) DO NOTHING",
            )?;
            for edge in batch.edges {
                stmt.execute(params![
                    edge.id.0,
                    edge.source.0,
                    edge.target.0,
                    edge.kind as i32,
                    edge.file_node_id.map(|id| id.0),
                    edge.line,
                    edge.resolved_source.map(|id| id.0),
                    edge.resolved_target.map(|id| id.0),
                    edge.confidence,
                    edge.callsite_identity.as_deref(),
                    row_mapping::certainty_db_value(edge.certainty),
                    serialize_candidate_targets(&edge.candidate_targets)?
                ])
                .map_err(|err| {
                    let source_label = pending_node_labels
                        .get(&edge.source)
                        .map(String::as_str)
                        .unwrap_or("<not in pending batch>");
                    let target_label = pending_node_labels
                        .get(&edge.target)
                        .map(String::as_str)
                        .unwrap_or("<not in pending batch>");
                    let file_label = edge
                        .file_node_id
                        .and_then(|id| pending_node_labels.get(&id).map(String::as_str))
                        .unwrap_or("<not in pending batch>");
                    StorageError::Other(format!(
                        "flush_projection_batch edge insert failed for id={} kind={:?} source={} ({}) target={} ({}) file_node_id={:?} ({}) resolved_source={:?} resolved_target={:?}: {err}",
                        edge.id.0,
                        edge.kind,
                        edge.source.0,
                        source_label,
                        edge.target.0,
                        target_label,
                        edge.file_node_id.map(|id| id.0),
                        file_label,
                        edge.resolved_source.map(|id| id.0),
                        edge.resolved_target.map(|id| id.0)
                    ))
                })?;
            }
            breakdown.edges_ms = clamp_i64_to_u32(started.elapsed().as_millis() as i64);
        }

        if !batch.occurrences.is_empty() {
            let started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO occurrence (element_id, kind, file_node_id, start_line, start_col, end_line, end_col)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for occ in batch.occurrences {
                stmt.execute(params![
                    occ.element_id,
                    occ.kind as i32,
                    occ.location.file_node_id.0,
                    occ.location.start_line,
                    occ.location.start_col,
                    occ.location.end_line,
                    occ.location.end_col,
                ])
                .map_err(|err| {
                    StorageError::Other(format!(
                        "flush_projection_batch occurrence insert failed for element_id={} kind={:?} file_node_id={} range={}:{}-{}:{}: {err}",
                        occ.element_id,
                        occ.kind,
                        occ.location.file_node_id.0,
                        occ.location.start_line,
                        occ.location.start_col,
                        occ.location.end_line,
                        occ.location.end_col
                    ))
                })?;
            }
            breakdown.occurrences_ms = clamp_i64_to_u32(started.elapsed().as_millis() as i64);
        }

        if !batch.component_access.is_empty() {
            let started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT INTO component_access (node_id, type)
                 VALUES (?1, ?2)
                 ON CONFLICT(node_id) DO UPDATE SET type = excluded.type",
            )?;
            for (node_id, access) in batch.component_access {
                stmt.execute(params![
                    node_id.0,
                    row_mapping::access_kind_db_value(*access),
                ])
                .map_err(|err| {
                    StorageError::Other(format!(
                        "flush_projection_batch component_access insert failed for node_id={} access={:?}: {err}",
                        node_id.0, access
                    ))
                })?;
            }
            breakdown.component_access_ms = clamp_i64_to_u32(started.elapsed().as_millis() as i64);
        }

        if !batch.callable_projection_states.is_empty() {
            let started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT INTO callable_projection_state (
                    file_id, symbol_key, node_id, signature_hash, body_hash, start_line, end_line
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(file_id, symbol_key) DO UPDATE SET
                    node_id = excluded.node_id,
                    signature_hash = excluded.signature_hash,
                    body_hash = excluded.body_hash,
                    start_line = excluded.start_line,
                    end_line = excluded.end_line",
            )?;
            for state in batch.callable_projection_states {
                stmt.execute(params![
                    state.file_id,
                    state.symbol_key,
                    state.node_id.0,
                    state.signature_hash,
                    state.body_hash,
                    state.start_line,
                    state.end_line,
                ])
                .map_err(|err| {
                    StorageError::Other(format!(
                        "flush_projection_batch callable_projection_state insert failed for file_id={} node_id={} symbol_key={} range={}-{}: {err}",
                        state.file_id,
                        state.node_id.0,
                        state.symbol_key,
                        state.start_line,
                        state.end_line
                    ))
                })?;
            }
            breakdown.callable_projection_ms =
                clamp_i64_to_u32(started.elapsed().as_millis() as i64);
        }

        tx.commit()?;

        if !prepared_nodes.is_empty() {
            let mut cache = self.cache.nodes.write();
            for node in prepared_nodes {
                cache.insert(node.id, node.clone());
            }
        }

        self.invalidate_grounding_snapshots()?;
        Ok(breakdown)
    }

    pub fn delete_callable_projection_states_for_file(
        &mut self,
        file_id: i64,
    ) -> Result<usize, StorageError> {
        Ok(self.conn.execute(
            "DELETE FROM callable_projection_state WHERE file_id = ?1",
            params![file_id],
        )?)
    }

    pub fn delete_projection_for_callers(
        &mut self,
        file_id: i64,
        caller_ids: &[NodeId],
    ) -> Result<CallerProjectionRemovalSummary, StorageError> {
        if caller_ids.is_empty() {
            return Ok(CallerProjectionRemovalSummary {
                file_id,
                ..Default::default()
            });
        }

        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS caller_cleanup_ids (
                caller_id INTEGER PRIMARY KEY
             );
             DELETE FROM caller_cleanup_ids;",
        )?;
        {
            let mut insert_ids =
                tx.prepare("INSERT INTO caller_cleanup_ids (caller_id) VALUES (?1)")?;
            for caller_id in caller_ids {
                insert_ids.execute(params![caller_id.0])?;
            }
        }

        let removed_edges = tx.execute(
            &format!(
                "DELETE FROM edge
                 WHERE file_node_id = ?1
                 AND source_node_id IN (SELECT caller_id FROM {CALLER_CLEANUP_IDS_TABLE})
                 AND kind IN ({}, {})",
                EdgeKind::CALL as i32,
                EdgeKind::USAGE as i32
            ),
            params![file_id],
        )?;

        let removed_occurrences = tx.execute(
            &format!(
                "DELETE FROM occurrence
                 WHERE file_node_id = ?1
                 AND (
                    element_id IN (SELECT caller_id FROM {CALLER_CLEANUP_IDS_TABLE})
                    OR EXISTS (
                        SELECT 1
                        FROM callable_projection_state cps
                        JOIN {CALLER_CLEANUP_IDS_TABLE} cleanup
                          ON cleanup.caller_id = cps.node_id
                        WHERE cps.file_id = ?1
                        AND occurrence.start_line >= cps.start_line
                        AND occurrence.end_line <= cps.end_line
                    )
                 )"
            ),
            params![file_id],
        )?;

        let removed_callable_projection_state_count = tx.execute(
            &format!(
                "DELETE FROM callable_projection_state
                 WHERE file_id = ?1
                 AND node_id IN (SELECT caller_id FROM {CALLER_CLEANUP_IDS_TABLE})"
            ),
            params![file_id],
        )?;

        tx.commit()?;
        self.invalidate_grounding_snapshots()?;

        Ok(CallerProjectionRemovalSummary {
            file_id,
            removed_edge_count: removed_edges,
            removed_occurrence_count: removed_occurrences,
            removed_callable_projection_state_count,
        })
    }

    pub fn get_component_access(
        &self,
        node_id: NodeId,
    ) -> Result<Option<AccessKind>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT type FROM component_access WHERE node_id = ?1")?;
        let mut rows = stmt.query(params![node_id.0])?;
        if let Some(row) = rows.next()? {
            let raw: i32 = row.get(0)?;
            return Ok(Some(row_mapping::access_kind_from_db(raw)));
        }
        Ok(None)
    }

    pub fn get_component_access_map_for_nodes(
        &self,
        node_ids: &[NodeId],
    ) -> Result<HashMap<NodeId, AccessKind>, StorageError> {
        if node_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = question_placeholders(node_ids.len());
        let sql =
            format!("SELECT node_id, type FROM component_access WHERE node_id IN ({placeholders})");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(node_ids.iter().map(|id| id.0)))?;
        let mut map = HashMap::new();
        while let Some(row) = rows.next()? {
            let node_id: i64 = row.get(0)?;
            let raw: i32 = row.get(1)?;
            map.insert(NodeId(node_id), row_mapping::access_kind_from_db(raw));
        }
        Ok(map)
    }

    pub fn upsert_search_symbol_projection_batch(
        &mut self,
        symbols: &[SearchSymbolProjection],
    ) -> Result<(), StorageError> {
        if symbols.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO search_symbol_projection (
                    node_id,
                    display_name
                 ) VALUES (?1, ?2)
                 ON CONFLICT(node_id) DO UPDATE SET
                    display_name = excluded.display_name",
            )?;
            for symbol in symbols {
                stmt.execute(params![symbol.node_id.0, symbol.display_name])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_search_symbol_projection_batch_after(
        &self,
        after_node_id: Option<NodeId>,
        limit: usize,
    ) -> Result<Vec<SearchSymbolProjection>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT node_id, display_name
             FROM search_symbol_projection
             WHERE (?1 IS NULL OR node_id > ?1)
             ORDER BY node_id ASC
             LIMIT ?2",
        )?;
        let after_node_id = after_node_id.map(|id| id.0);
        let limit = limit.min(i64::MAX as usize) as i64;
        let mut rows = stmt.query(params![after_node_id, limit])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SearchSymbolProjection {
                node_id: NodeId(row.get(0)?),
                display_name: row.get(1)?,
            });
        }
        Ok(out)
    }

    pub fn get_search_symbol_projection_detail_batch_after(
        &self,
        after_node_id: Option<NodeId>,
        limit: usize,
    ) -> Result<Vec<SearchSymbolProjectionDetail>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                projection.node_id,
                projection.display_name,
                node.kind,
                file.serialized_name,
                node.start_line,
                node.end_line
             FROM search_symbol_projection projection
             LEFT JOIN node ON node.id = projection.node_id
             LEFT JOIN node file ON file.id = node.file_node_id
             WHERE (?1 IS NULL OR projection.node_id > ?1)
             ORDER BY projection.node_id ASC
             LIMIT ?2",
        )?;
        let after_node_id = after_node_id.map(|id| id.0);
        let limit = limit.min(i64::MAX as usize) as i64;
        let mut rows = stmt.query(params![after_node_id, limit])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SearchSymbolProjectionDetail {
                node_id: NodeId(row.get(0)?),
                display_name: row.get(1)?,
                node_kind: row.get(2)?,
                file_path: row.get(3)?,
                start_line: row.get(4)?,
                end_line: row.get(5)?,
            });
        }
        Ok(out)
    }

    pub fn get_search_symbol_projection_count(&self) -> Result<u32, StorageError> {
        let count =
            self.conn
                .query_row("SELECT COUNT(*) FROM search_symbol_projection", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        Ok(clamp_i64_to_u32(count))
    }

    pub fn upsert_symbol_search_docs_batch(
        &mut self,
        docs: &[SymbolSearchDoc],
    ) -> Result<(), StorageError> {
        if docs.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbol_search_doc (
                    node_id,
                    file_node_id,
                    kind,
                    display_name,
                    qualified_name,
                    file_path,
                    start_line,
                    doc_text,
                    doc_version,
                    doc_hash,
                    policy_version,
                    source_provenance,
                    updated_at_epoch_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
                 )
                 ON CONFLICT(node_id) DO UPDATE SET
                    file_node_id = excluded.file_node_id,
                    kind = excluded.kind,
                    display_name = excluded.display_name,
                    qualified_name = excluded.qualified_name,
                    file_path = excluded.file_path,
                    start_line = excluded.start_line,
                    doc_text = excluded.doc_text,
                    doc_version = excluded.doc_version,
                    doc_hash = excluded.doc_hash,
                    policy_version = excluded.policy_version,
                    source_provenance = excluded.source_provenance,
                    updated_at_epoch_ms = excluded.updated_at_epoch_ms",
            )?;
            for doc in docs {
                stmt.execute(params![
                    doc.node_id.0,
                    doc.file_node_id.map(|id| id.0),
                    doc.kind as i32,
                    doc.display_name,
                    doc.qualified_name,
                    doc.file_path,
                    doc.start_line,
                    doc.doc_text,
                    doc.doc_version as i64,
                    doc.doc_hash,
                    doc.policy_version,
                    doc.source_provenance,
                    doc.updated_at_epoch_ms,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_symbol_search_docs_batch_after(
        &self,
        after_node_id: Option<NodeId>,
        limit: usize,
    ) -> Result<Vec<SymbolSearchDoc>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                node_id,
                file_node_id,
                kind,
                display_name,
                qualified_name,
                file_path,
                start_line,
                doc_text,
                doc_version,
                doc_hash,
                policy_version,
                source_provenance,
                updated_at_epoch_ms
             FROM symbol_search_doc
             WHERE (?1 IS NULL OR node_id > ?1)
             ORDER BY node_id ASC
             LIMIT ?2",
        )?;
        let after_node_id = after_node_id.map(|id| id.0);
        let limit = limit.min(i64::MAX as usize) as i64;
        let mut rows = stmt.query(params![after_node_id, limit])?;
        let mut docs = Vec::new();
        while let Some(row) = rows.next()? {
            let kind: i32 = row.get(2)?;
            let doc_version: i64 = row.get(8)?;
            docs.push(SymbolSearchDoc {
                node_id: NodeId(row.get(0)?),
                file_node_id: row.get::<_, Option<i64>>(1)?.map(NodeId),
                kind: NodeKind::try_from(kind)?,
                display_name: row.get(3)?,
                qualified_name: row.get(4)?,
                file_path: row.get(5)?,
                start_line: row.get(6)?,
                doc_text: row.get(7)?,
                doc_version: doc_version.max(0).min(u32::MAX as i64) as u32,
                doc_hash: row.get(9)?,
                policy_version: row.get(10)?,
                source_provenance: row.get(11)?,
                updated_at_epoch_ms: row.get(12)?,
            });
        }
        Ok(docs)
    }

    pub fn get_symbol_search_doc_count(&self) -> Result<u32, StorageError> {
        let count = self
            .conn
            .query_row("SELECT COUNT(*) FROM symbol_search_doc", [], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(clamp_i64_to_u32(count))
    }

    pub fn has_symbol_search_doc_version_mismatch(
        &self,
        expected_version: u32,
    ) -> Result<bool, StorageError> {
        let mismatch = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM symbol_search_doc
                WHERE doc_version <> ?1
                LIMIT 1
            )",
            params![expected_version as i64],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(mismatch)
    }

    pub fn clear_symbol_search_docs(&mut self) -> Result<usize, StorageError> {
        let removed = self.conn.execute("DELETE FROM symbol_search_doc", [])?;
        Ok(removed)
    }

    pub fn copy_symbol_search_docs_from(
        &mut self,
        source_path: &Path,
    ) -> Result<usize, StorageError> {
        if !source_path.exists() {
            return Ok(0);
        }
        drop(Storage::open(source_path)?);
        let source = source_path.to_string_lossy().to_string();
        self.conn
            .execute("ATTACH DATABASE ?1 AS source_snapshot", params![source])?;
        let copy_result = self.conn.execute(
            "INSERT OR REPLACE INTO symbol_search_doc (
                node_id,
                file_node_id,
                kind,
                display_name,
                qualified_name,
                file_path,
                start_line,
                doc_text,
                doc_version,
                doc_hash,
                policy_version,
                source_provenance,
                updated_at_epoch_ms
             )
             SELECT
                source_doc.node_id,
                source_doc.file_node_id,
                source_doc.kind,
                source_doc.display_name,
                source_doc.qualified_name,
                source_doc.file_path,
                source_doc.start_line,
                source_doc.doc_text,
                source_doc.doc_version,
                source_doc.doc_hash,
                source_doc.policy_version,
                source_doc.source_provenance,
                source_doc.updated_at_epoch_ms
             FROM source_snapshot.symbol_search_doc source_doc
             WHERE EXISTS (
                SELECT 1 FROM node WHERE node.id = source_doc.node_id
             )
             AND (
                source_doc.file_node_id IS NULL
                OR EXISTS (
                    SELECT 1 FROM node WHERE node.id = source_doc.file_node_id
                )
             )",
            [],
        );
        let detach_result = self.conn.execute("DETACH DATABASE source_snapshot", []);
        let copied = copy_result?;
        detach_result?;
        Ok(copied)
    }

    pub fn prune_symbol_search_docs_to_node_ids(
        &mut self,
        keep_node_ids: &[NodeId],
    ) -> Result<usize, StorageError> {
        if keep_node_ids.is_empty() {
            return self.clear_symbol_search_docs();
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS symbol_search_doc_keep (
                node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute("DELETE FROM temp.symbol_search_doc_keep", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO temp.symbol_search_doc_keep (node_id) VALUES (?1)",
            )?;
            for node_id in keep_node_ids {
                stmt.execute(params![node_id.0])?;
            }
        }
        let removed = tx.execute(
            "DELETE FROM symbol_search_doc
             WHERE NOT EXISTS (
                SELECT 1
                FROM temp.symbol_search_doc_keep keep
                WHERE keep.node_id = symbol_search_doc.node_id
             )",
            [],
        )?;
        tx.execute("DROP TABLE temp.symbol_search_doc_keep", [])?;
        tx.commit()?;
        Ok(removed)
    }

    pub fn delete_symbol_search_docs_for_files_except_node_ids(
        &mut self,
        file_node_ids: &[NodeId],
        keep_node_ids: &[NodeId],
    ) -> Result<usize, StorageError> {
        if file_node_ids.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS symbol_search_doc_scope (
                file_node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS symbol_search_doc_keep (
                node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute("DELETE FROM temp.symbol_search_doc_scope", [])?;
        tx.execute("DELETE FROM temp.symbol_search_doc_keep", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO temp.symbol_search_doc_scope (file_node_id) VALUES (?1)",
            )?;
            for file_node_id in file_node_ids {
                stmt.execute(params![file_node_id.0])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO temp.symbol_search_doc_keep (node_id) VALUES (?1)",
            )?;
            for node_id in keep_node_ids {
                stmt.execute(params![node_id.0])?;
            }
        }
        let removed = tx.execute(
            "DELETE FROM symbol_search_doc
             WHERE file_node_id IN (
                SELECT file_node_id FROM temp.symbol_search_doc_scope
             )
             AND NOT EXISTS (
                SELECT 1
                FROM temp.symbol_search_doc_keep keep
                WHERE keep.node_id = symbol_search_doc.node_id
             )",
            [],
        )?;
        tx.execute("DROP TABLE temp.symbol_search_doc_scope", [])?;
        tx.execute("DROP TABLE temp.symbol_search_doc_keep", [])?;
        tx.commit()?;
        Ok(removed)
    }

    pub fn max_indexed_file_modification_time(&self) -> Result<Option<i64>, StorageError> {
        self.conn
            .query_row(
                "SELECT MAX(modification_time) FROM file WHERE indexed = 1",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn clear_search_symbol_projection(&mut self) -> Result<usize, StorageError> {
        let removed = self
            .conn
            .execute("DELETE FROM search_symbol_projection", [])?;
        Ok(removed)
    }

    pub fn rebuild_search_symbol_projection_from_node_table(
        &mut self,
    ) -> Result<u32, StorageError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM search_symbol_projection", [])?;
        let inserted = tx.execute(
            "INSERT INTO search_symbol_projection (
                node_id,
                display_name
             )
             SELECT
                id,
                CASE
                    WHEN qualified_name IS NOT NULL AND TRIM(qualified_name) != '' THEN qualified_name
                    ELSE serialized_name
                END
             FROM node",
            [],
        )?;
        tx.commit()?;
        Ok(inserted.min(u32::MAX as usize) as u32)
    }

    pub fn rebuild_search_symbol_projection_for_file_scope(
        &mut self,
        file_node_ids: &HashSet<NodeId>,
    ) -> Result<u32, StorageError> {
        if file_node_ids.is_empty() {
            return Ok(0);
        }

        let mut file_ids: Vec<i64> = file_node_ids.iter().map(|id| id.0).collect();
        file_ids.sort_unstable();
        file_ids.dedup();
        let placeholders = numbered_placeholders(1, file_ids.len());
        let file_scope_predicate =
            format!("id IN ({placeholders}) OR file_node_id IN ({placeholders})");

        let tx = self.conn.transaction()?;
        tx.execute(
            &format!(
                "DELETE FROM search_symbol_projection
                 WHERE node_id IN (
                    SELECT id FROM node WHERE {file_scope_predicate}
                 )"
            ),
            params_from_iter(file_ids.iter().copied()),
        )?;
        let inserted = tx.execute(
            &format!(
                "INSERT INTO search_symbol_projection (
                    node_id,
                    display_name
                 )
                 SELECT
                    id,
                    CASE
                        WHEN qualified_name IS NOT NULL AND TRIM(qualified_name) != '' THEN qualified_name
                        ELSE serialized_name
                    END
                 FROM node
                 WHERE {file_scope_predicate}"
            ),
            params_from_iter(file_ids.iter().copied()),
        )?;
        tx.commit()?;
        Ok(inserted.min(u32::MAX as usize) as u32)
    }

    pub fn upsert_llm_symbol_docs_batch(
        &mut self,
        docs: &[LlmSymbolDoc],
    ) -> Result<(), StorageError> {
        if docs.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO llm_symbol_doc (
                    node_id,
                    file_node_id,
                    kind,
                    display_name,
                    qualified_name,
                    file_path,
                    start_line,
                    doc_text,
                    doc_version,
                    doc_hash,
                    embedding_profile,
                    embedding_model,
                    embedding_backend,
                    embedding_dim,
                    doc_shape,
                    semantic_policy_version,
                    dense_reason,
                    embedding_blob,
                    updated_at_epoch_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
                 )
                 ON CONFLICT(node_id) DO UPDATE SET
                    file_node_id = excluded.file_node_id,
                    kind = excluded.kind,
                    display_name = excluded.display_name,
                    qualified_name = excluded.qualified_name,
                    file_path = excluded.file_path,
                    start_line = excluded.start_line,
                    doc_text = excluded.doc_text,
                    doc_version = excluded.doc_version,
                    doc_hash = excluded.doc_hash,
                    embedding_profile = excluded.embedding_profile,
                    embedding_model = excluded.embedding_model,
                    embedding_backend = excluded.embedding_backend,
                    embedding_dim = excluded.embedding_dim,
                    doc_shape = excluded.doc_shape,
                    semantic_policy_version = excluded.semantic_policy_version,
                    dense_reason = excluded.dense_reason,
                    embedding_blob = excluded.embedding_blob,
                    updated_at_epoch_ms = excluded.updated_at_epoch_ms",
            )?;

            for doc in docs {
                stmt.execute(params![
                    doc.node_id.0,
                    doc.file_node_id.map(|id| id.0),
                    doc.kind as i32,
                    doc.display_name,
                    doc.qualified_name,
                    doc.file_path,
                    doc.start_line,
                    doc.doc_text,
                    doc.doc_version as i64,
                    doc.doc_hash,
                    doc.embedding_profile,
                    doc.embedding_model,
                    doc.embedding_backend,
                    doc.embedding_dim as i64,
                    doc.doc_shape,
                    doc.semantic_policy_version,
                    doc.dense_reason,
                    encode_embedding_blob(&doc.embedding),
                    doc.updated_at_epoch_ms,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_llm_symbol_docs_by_node_ids(
        &self,
        node_ids: &[NodeId],
    ) -> Result<Vec<LlmSymbolDoc>, StorageError> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = question_placeholders(node_ids.len());
        let sql = format!(
            "SELECT
                node_id,
                file_node_id,
                kind,
                display_name,
                qualified_name,
                file_path,
                start_line,
                doc_text,
                doc_version,
                doc_hash,
                embedding_profile,
                embedding_model,
                embedding_backend,
                embedding_dim,
                doc_shape,
                semantic_policy_version,
                dense_reason,
                embedding_blob,
                updated_at_epoch_ms
             FROM llm_symbol_doc
             WHERE node_id IN ({placeholders})
             ORDER BY node_id ASC"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(node_ids.iter().map(|id| id.0)))?;
        let mut docs = Vec::new();

        while let Some(row) = rows.next()? {
            let kind: i32 = row.get(2)?;
            let doc_version: i64 = row.get(8)?;
            let embedding_dim: i64 = row.get(13)?;
            let embedding_blob: Vec<u8> = row.get(17)?;
            docs.push(LlmSymbolDoc {
                node_id: NodeId(row.get(0)?),
                file_node_id: row.get::<_, Option<i64>>(1)?.map(NodeId),
                kind: NodeKind::try_from(kind)?,
                display_name: row.get(3)?,
                qualified_name: row.get(4)?,
                file_path: row.get(5)?,
                start_line: row.get(6)?,
                doc_text: row.get(7)?,
                doc_version: doc_version.max(0).min(u32::MAX as i64) as u32,
                doc_hash: row.get(9)?,
                embedding_profile: row.get(10)?,
                embedding_model: row.get(11)?,
                embedding_backend: row.get(12)?,
                embedding_dim: embedding_dim.max(0) as u32,
                doc_shape: row.get(14)?,
                semantic_policy_version: row.get(15)?,
                dense_reason: row.get(16)?,
                embedding: decode_embedding_blob(&embedding_blob)?,
                updated_at_epoch_ms: row.get(18)?,
            });
        }

        Ok(docs)
    }

    pub fn get_llm_symbol_doc_stats(&self) -> Result<LlmSymbolDocStats, StorageError> {
        let (
            doc_count,
            min_profile,
            max_profile,
            profile_count,
            min_model,
            max_model,
            model_count,
            min_backend,
            max_backend,
            backend_count,
            min_dim,
            max_dim,
            dim_count,
            min_version,
            max_version,
            version_count,
            min_shape,
            max_shape,
            shape_count,
            min_policy,
            max_policy,
            policy_count,
        ) = self.conn.query_row(
            "SELECT
                COUNT(*),
                MIN(embedding_profile),
                MAX(embedding_profile),
                COUNT(embedding_profile),
                MIN(embedding_model),
                MAX(embedding_model),
                COUNT(embedding_model),
                MIN(embedding_backend),
                MAX(embedding_backend),
                COUNT(embedding_backend),
                MIN(embedding_dim),
                MAX(embedding_dim),
                COUNT(embedding_dim),
                MIN(doc_version),
                MAX(doc_version),
                COUNT(doc_version),
                MIN(doc_shape),
                MAX(doc_shape),
                COUNT(doc_shape),
                MIN(semantic_policy_version),
                MAX(semantic_policy_version),
                COUNT(semantic_policy_version)
             FROM llm_symbol_doc",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, Option<String>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                    row.get::<_, i64>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, i64>(21)?,
                ))
            },
        )?;
        let (embedding_profile, mixed_embedding_profiles) =
            uniform_optional_string_with_count(doc_count, profile_count, min_profile, max_profile);
        let (embedding_model, mixed_embedding_models) =
            uniform_optional_string_with_count(doc_count, model_count, min_model, max_model);
        let (embedding_backend, mixed_embedding_backends) =
            uniform_optional_string_with_count(doc_count, backend_count, min_backend, max_backend);
        let (doc_shape, mixed_doc_shapes) =
            uniform_optional_string_with_count(doc_count, shape_count, min_shape, max_shape);
        let (semantic_policy_version, mixed_semantic_policy_versions) =
            uniform_optional_string_with_count(doc_count, policy_count, min_policy, max_policy);
        let (embedding_dim, mixed_dimensions) =
            uniform_optional_u32_with_count(doc_count, dim_count, min_dim, max_dim);
        let (doc_version, mixed_doc_versions) =
            uniform_optional_u32_with_count(doc_count, version_count, min_version, max_version);

        Ok(LlmSymbolDocStats {
            doc_count: doc_count.max(0).min(u32::MAX as i64) as u32,
            embedding_profile,
            embedding_model,
            embedding_backend,
            embedding_dim,
            doc_version,
            doc_shape,
            semantic_policy_version,
            mixed_embedding_profiles,
            mixed_embedding_models,
            mixed_embedding_backends,
            mixed_dimensions,
            mixed_doc_versions,
            mixed_doc_shapes,
            mixed_semantic_policy_versions,
        })
    }

    pub fn upsert_symbol_summaries_batch(
        &mut self,
        summaries: &[SymbolSummaryRecord],
    ) -> Result<(), StorageError> {
        if summaries.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbol_summary (
                    node_id,
                    content_hash,
                    summary,
                    model,
                    updated_at_epoch_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(node_id, content_hash) DO UPDATE SET
                    summary = excluded.summary,
                    model = excluded.model,
                    updated_at_epoch_ms = excluded.updated_at_epoch_ms",
            )?;
            for summary in summaries {
                stmt.execute(params![
                    summary.node_id.0,
                    summary.content_hash,
                    summary.summary,
                    summary.model,
                    summary.updated_at_epoch_ms,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_current_symbol_summaries_by_node_ids(
        &self,
        node_ids: &[NodeId],
    ) -> Result<HashMap<NodeId, SymbolSummaryRecord>, StorageError> {
        if node_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = question_placeholders(node_ids.len());
        let sql = format!(
            "SELECT summary.node_id,
                    summary.content_hash,
                    summary.summary,
                    summary.model,
                    summary.updated_at_epoch_ms
             FROM symbol_summary summary
             INNER JOIN llm_symbol_doc doc
                ON doc.node_id = summary.node_id
               AND doc.doc_hash = summary.content_hash
             WHERE summary.node_id IN ({placeholders})
             ORDER BY summary.node_id ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(node_ids.iter().map(|id| id.0)))?;
        let mut summaries = HashMap::new();
        while let Some(row) = rows.next()? {
            let record = SymbolSummaryRecord {
                node_id: NodeId(row.get(0)?),
                content_hash: row.get(1)?,
                summary: row.get(2)?,
                model: row.get(3)?,
                updated_at_epoch_ms: row.get(4)?,
            };
            summaries.insert(record.node_id, record);
        }
        Ok(summaries)
    }

    pub fn get_all_current_symbol_summaries(
        &self,
    ) -> Result<HashMap<NodeId, SymbolSummaryRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT summary.node_id,
                    summary.content_hash,
                    summary.summary,
                    summary.model,
                    summary.updated_at_epoch_ms
             FROM symbol_summary summary
             INNER JOIN llm_symbol_doc doc
                ON doc.node_id = summary.node_id
               AND doc.doc_hash = summary.content_hash
             ORDER BY summary.node_id ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut summaries = HashMap::new();
        while let Some(row) = rows.next()? {
            let record = SymbolSummaryRecord {
                node_id: NodeId(row.get(0)?),
                content_hash: row.get(1)?,
                summary: row.get(2)?,
                model: row.get(3)?,
                updated_at_epoch_ms: row.get(4)?,
            };
            summaries.insert(record.node_id, record);
        }
        Ok(summaries)
    }

    pub fn get_all_llm_symbol_docs(&self) -> Result<Vec<LlmSymbolDoc>, StorageError> {
        self.get_llm_symbol_docs_batch_after(None, usize::MAX)
    }

    pub fn get_llm_symbol_doc_reuse_metadata(
        &self,
    ) -> Result<Vec<LlmSymbolDocReuseMetadata>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                node_id,
                doc_version,
                doc_hash,
                embedding_profile,
                embedding_model,
                embedding_backend,
                embedding_dim,
                doc_shape,
                semantic_policy_version,
                dense_reason
             FROM llm_symbol_doc
             ORDER BY node_id ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut docs = Vec::new();
        while let Some(row) = rows.next()? {
            let doc_version: i64 = row.get(1)?;
            let embedding_dim: i64 = row.get(6)?;
            docs.push(LlmSymbolDocReuseMetadata {
                node_id: NodeId(row.get(0)?),
                doc_version: doc_version.max(0).min(u32::MAX as i64) as u32,
                doc_hash: row.get(2)?,
                embedding_profile: row.get(3)?,
                embedding_model: row.get(4)?,
                embedding_backend: row.get(5)?,
                embedding_dim: embedding_dim.max(0).min(u32::MAX as i64) as u32,
                doc_shape: row.get(7)?,
                semantic_policy_version: row.get(8)?,
                dense_reason: row.get(9)?,
            });
        }
        Ok(docs)
    }

    pub fn get_llm_symbol_docs_batch_after(
        &self,
        after_node_id: Option<NodeId>,
        limit: usize,
    ) -> Result<Vec<LlmSymbolDoc>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                node_id,
                file_node_id,
                kind,
                display_name,
                qualified_name,
                file_path,
                start_line,
                doc_text,
                doc_version,
                doc_hash,
                embedding_profile,
                embedding_model,
                embedding_backend,
                embedding_dim,
                doc_shape,
                semantic_policy_version,
                dense_reason,
                embedding_blob,
                updated_at_epoch_ms
             FROM llm_symbol_doc
             WHERE (?1 IS NULL OR node_id > ?1)
             ORDER BY node_id ASC
             LIMIT ?2",
        )?;
        let after_node_id = after_node_id.map(|id| id.0);
        let limit = limit.min(i64::MAX as usize) as i64;
        let mut rows = stmt.query(params![after_node_id, limit])?;
        let mut docs = Vec::new();
        while let Some(row) = rows.next()? {
            let kind: i32 = row.get(2)?;
            let doc_version: i64 = row.get(8)?;
            let embedding_dim: i64 = row.get(13)?;
            let embedding_blob: Vec<u8> = row.get(17)?;
            docs.push(LlmSymbolDoc {
                node_id: NodeId(row.get(0)?),
                file_node_id: row.get::<_, Option<i64>>(1)?.map(NodeId),
                kind: NodeKind::try_from(kind)?,
                display_name: row.get(3)?,
                qualified_name: row.get(4)?,
                file_path: row.get(5)?,
                start_line: row.get(6)?,
                doc_text: row.get(7)?,
                doc_version: doc_version.max(0).min(u32::MAX as i64) as u32,
                doc_hash: row.get(9)?,
                embedding_profile: row.get(10)?,
                embedding_model: row.get(11)?,
                embedding_backend: row.get(12)?,
                embedding_dim: embedding_dim.max(0) as u32,
                doc_shape: row.get(14)?,
                semantic_policy_version: row.get(15)?,
                dense_reason: row.get(16)?,
                embedding: decode_embedding_blob(&embedding_blob)?,
                updated_at_epoch_ms: row.get(18)?,
            });
        }
        Ok(docs)
    }

    pub fn clear_llm_symbol_docs(&mut self) -> Result<usize, StorageError> {
        let removed = self.conn.execute("DELETE FROM llm_symbol_doc", [])?;
        Ok(removed)
    }

    pub fn copy_llm_symbol_docs_from(&mut self, source_path: &Path) -> Result<usize, StorageError> {
        if !source_path.exists() {
            return Ok(0);
        }
        drop(Storage::open(source_path)?);
        let source = source_path.to_string_lossy().to_string();
        self.conn
            .execute("ATTACH DATABASE ?1 AS source_snapshot", params![source])?;
        let copy_result = self.conn.execute(
            "INSERT OR REPLACE INTO llm_symbol_doc (
                node_id,
                file_node_id,
                kind,
                display_name,
                qualified_name,
                file_path,
                start_line,
                doc_text,
                doc_version,
                doc_hash,
                embedding_profile,
                embedding_model,
                embedding_backend,
                embedding_dim,
                doc_shape,
                semantic_policy_version,
                dense_reason,
                embedding_blob,
                updated_at_epoch_ms
             )
             SELECT
                source_doc.node_id,
                source_doc.file_node_id,
                source_doc.kind,
                source_doc.display_name,
                source_doc.qualified_name,
                source_doc.file_path,
                source_doc.start_line,
                source_doc.doc_text,
                source_doc.doc_version,
                source_doc.doc_hash,
                source_doc.embedding_profile,
                source_doc.embedding_model,
                source_doc.embedding_backend,
                source_doc.embedding_dim,
                source_doc.doc_shape,
                source_doc.semantic_policy_version,
                source_doc.dense_reason,
                source_doc.embedding_blob,
                source_doc.updated_at_epoch_ms
             FROM source_snapshot.llm_symbol_doc source_doc
             WHERE EXISTS (
                SELECT 1 FROM node WHERE node.id = source_doc.node_id
             )
             AND (
                source_doc.file_node_id IS NULL
                OR EXISTS (
                    SELECT 1 FROM node WHERE node.id = source_doc.file_node_id
                )
             )",
            [],
        );
        let detach_result = self.conn.execute("DETACH DATABASE source_snapshot", []);
        let copied = copy_result?;
        detach_result?;
        Ok(copied)
    }

    pub fn prune_llm_symbol_docs_to_node_ids(
        &mut self,
        keep_node_ids: &[NodeId],
    ) -> Result<usize, StorageError> {
        if keep_node_ids.is_empty() {
            return self.clear_llm_symbol_docs();
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS llm_symbol_doc_keep (
                node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute("DELETE FROM temp.llm_symbol_doc_keep", [])?;
        {
            let mut stmt =
                tx.prepare("INSERT OR IGNORE INTO temp.llm_symbol_doc_keep (node_id) VALUES (?1)")?;
            for node_id in keep_node_ids {
                stmt.execute(params![node_id.0])?;
            }
        }
        let removed = tx.execute(
            "DELETE FROM llm_symbol_doc
             WHERE NOT EXISTS (
                SELECT 1
                FROM temp.llm_symbol_doc_keep keep
                WHERE keep.node_id = llm_symbol_doc.node_id
             )",
            [],
        )?;
        tx.execute("DROP TABLE temp.llm_symbol_doc_keep", [])?;
        tx.commit()?;
        Ok(removed)
    }

    pub fn delete_llm_symbol_docs_for_files_except_node_ids(
        &mut self,
        file_node_ids: &[NodeId],
        keep_node_ids: &[NodeId],
    ) -> Result<usize, StorageError> {
        if file_node_ids.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS llm_symbol_doc_scope (
                file_node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS llm_symbol_doc_keep (
                node_id INTEGER PRIMARY KEY
             )",
            [],
        )?;
        tx.execute("DELETE FROM temp.llm_symbol_doc_scope", [])?;
        tx.execute("DELETE FROM temp.llm_symbol_doc_keep", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO temp.llm_symbol_doc_scope (file_node_id) VALUES (?1)",
            )?;
            for file_node_id in file_node_ids {
                stmt.execute(params![file_node_id.0])?;
            }
        }
        {
            let mut stmt =
                tx.prepare("INSERT OR IGNORE INTO temp.llm_symbol_doc_keep (node_id) VALUES (?1)")?;
            for node_id in keep_node_ids {
                stmt.execute(params![node_id.0])?;
            }
        }
        let removed = tx.execute(
            "DELETE FROM llm_symbol_doc
             WHERE file_node_id IN (
                SELECT file_node_id FROM temp.llm_symbol_doc_scope
             )
             AND NOT EXISTS (
                SELECT 1
                FROM temp.llm_symbol_doc_keep keep
                WHERE keep.node_id = llm_symbol_doc.node_id
             )",
            [],
        )?;
        tx.execute("DROP TABLE temp.llm_symbol_doc_scope", [])?;
        tx.execute("DROP TABLE temp.llm_symbol_doc_keep", [])?;
        tx.commit()?;
        Ok(removed)
    }

    pub fn delete_llm_symbol_docs_for_file(
        &mut self,
        file_node_id: NodeId,
    ) -> Result<usize, StorageError> {
        let removed = self.conn.execute(
            "DELETE FROM llm_symbol_doc WHERE file_node_id = ?1",
            params![file_node_id.0],
        )?;
        Ok(removed)
    }

    pub fn delete_symbol_search_docs_for_file(
        &mut self,
        file_node_id: NodeId,
    ) -> Result<usize, StorageError> {
        let removed = self.conn.execute(
            "DELETE FROM symbol_search_doc WHERE file_node_id = ?1",
            params![file_node_id.0],
        )?;
        Ok(removed)
    }

    pub fn get_occurrences(&self) -> Result<Vec<Occurrence>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT element_id, kind, file_node_id, start_line, start_col, end_line, end_col FROM occurrence"
        )?;
        let occ_iter = stmt.query_map([], Self::occurrence_from_row)?;

        let mut occurrences = Vec::new();
        for occ in occ_iter {
            occurrences.push(occ?);
        }
        Ok(occurrences)
    }

    pub fn get_occurrences_for_element(
        &self,
        element_id: i64,
    ) -> Result<Vec<Occurrence>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT element_id, kind, file_node_id, start_line, start_col, end_line, end_col FROM occurrence WHERE element_id = ?1"
        )?;
        let occ_iter = stmt.query_map([element_id], Self::occurrence_from_row)?;

        let mut occurrences = Vec::new();
        for occ in occ_iter {
            occurrences.push(occ?);
        }
        Ok(occurrences)
    }

    pub fn get_neighborhood(
        &self,
        center_id: NodeId,
    ) -> Result<(Vec<Node>, Vec<Edge>), StorageError> {
        let mut nodes = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node WHERE id = ?1")?;
            let mut rows = stmt.query(params![center_id.0])?;

            if let Some(row) = rows.next()? {
                nodes.push(Self::node_from_row(row)?);
            }
        }

        let edges = self.get_edges_for_node(
            center_id,
            &TrailDirection::Both,
            &[],
            TrailCallerScope::IncludeTestsAndBenches,
            true,
        )?;

        let mut neighbor_ids = HashSet::new();
        for edge in &edges {
            let (eff_source, eff_target) = edge.effective_endpoints();
            if eff_source != center_id {
                neighbor_ids.insert(eff_source);
            }
            if eff_target != center_id {
                neighbor_ids.insert(eff_target);
            }
        }

        for nid in neighbor_ids {
            let mut stmt = self
                .conn
                .prepare("SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node WHERE id = ?1")?;
            let mut rows = stmt.query(params![nid.0])?;

            if let Some(row) = rows.next()? {
                nodes.push(Self::node_from_row(row)?);
            }
        }

        Ok((nodes, edges))
    }

    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, StorageError> {
        if let Some(node) = self.cache.nodes.read().get(&id) {
            return Ok(Some(node.clone()));
        }

        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node WHERE id = ?1")?;
        let mut rows = stmt.query(params![id.0])?;

        if let Some(row) = rows.next()? {
            let node = Self::node_from_row(row)?;
            self.cache.nodes.write().insert(node.id, node.clone());
            Ok(Some(node))
        } else {
            Ok(None)
        }
    }

    pub fn get_nodes_by_ids(&self, ids: &[NodeId]) -> Result<HashMap<NodeId, Node>, StorageError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut unique_ids = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut nodes_by_id = HashMap::new();
        for id in ids {
            if !seen_ids.insert(*id) {
                continue;
            }
            if let Some(node) = self.cache.nodes.read().get(id) {
                nodes_by_id.insert(*id, node.clone());
            } else {
                unique_ids.push(*id);
            }
        }

        for chunk in unique_ids.chunks(NODE_LOOKUP_BATCH_SIZE) {
            let placeholders = numbered_placeholders(1, chunk.len());
            let query = format!(
                "SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node WHERE id IN ({placeholders})"
            );
            let params = chunk.iter().map(|id| Value::from(id.0));
            let mut stmt = self.conn.prepare(&query)?;
            let mut rows = stmt.query(params_from_iter(params))?;
            let mut cache = self.cache.nodes.write();
            while let Some(row) = rows.next()? {
                let node = Self::node_from_row(row)?;
                cache.insert(node.id, node.clone());
                nodes_by_id.insert(node.id, node);
            }
        }

        Ok(nodes_by_id)
    }

    pub fn get_occurrences_for_node_ids(
        &self,
        node_ids: &[NodeId],
    ) -> Result<HashMap<NodeId, Vec<Occurrence>>, StorageError> {
        if node_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut unique_ids = Vec::new();
        let mut seen_ids = HashSet::new();
        for node_id in node_ids {
            if seen_ids.insert(*node_id) {
                unique_ids.push(*node_id);
            }
        }

        let mut occurrences_by_node = unique_ids
            .iter()
            .copied()
            .map(|node_id| (node_id, Vec::new()))
            .collect::<HashMap<_, _>>();

        for chunk in unique_ids.chunks(OCCURRENCE_LOOKUP_BATCH_SIZE) {
            let placeholders = numbered_placeholders(1, chunk.len());
            let query = format!(
                "SELECT element_id, kind, file_node_id, start_line, start_col, end_line, end_col FROM occurrence WHERE element_id IN ({placeholders})"
            );
            let params = chunk.iter().map(|id| Value::from(id.0));
            let mut stmt = self.conn.prepare(&query)?;
            let mut rows = stmt.query(params_from_iter(params))?;
            while let Some(row) = rows.next()? {
                let occurrence = Self::occurrence_from_row(row)?;
                if let Some(occurrences) =
                    occurrences_by_node.get_mut(&NodeId(occurrence.element_id))
                {
                    occurrences.push(occurrence);
                }
            }
        }

        Ok(occurrences_by_node)
    }

    pub fn get_occurrences_for_node(
        &self,
        node_id: codestory_contracts::graph::NodeId,
    ) -> Result<Vec<Occurrence>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT element_id, kind, file_node_id, start_line, start_col, end_line, end_col FROM occurrence WHERE element_id = ?1"
        )?;
        let occ_iter = stmt.query_map(params![node_id.0], Self::occurrence_from_row)?;

        let mut occurrences = Vec::new();
        for occ in occ_iter {
            occurrences.push(occ?);
        }
        Ok(occurrences)
    }

    pub fn get_occurrences_for_file(
        &self,
        file_node_id: codestory_contracts::graph::NodeId,
    ) -> Result<Vec<Occurrence>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT element_id, kind, file_node_id, start_line, start_col, end_line, end_col FROM occurrence WHERE file_node_id = ?1"
        )?;
        let occ_iter = stmt.query_map(params![file_node_id.0], Self::occurrence_from_row)?;

        let mut occurrences = Vec::new();
        for occ in occ_iter {
            occurrences.push(occ?);
        }
        Ok(occurrences)
    }

    pub fn insert_file(&self, info: &FileInfo) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count, file_role)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                path=excluded.path,
                language=excluded.language,
                modification_time=excluded.modification_time,
                indexed=excluded.indexed,
                complete=excluded.complete,
                line_count=excluded.line_count,
                file_role=excluded.file_role",
            params![
                info.id,
                info.path.to_string_lossy(),
                info.language,
                info.modification_time,
                i32::from(info.indexed),
                i32::from(info.complete),
                info.line_count,
                info.file_role.as_str(),
            ],
        )?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    pub fn insert_files_batch(&mut self, files: &[FileInfo]) -> Result<(), StorageError> {
        if files.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count, file_role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                    path=excluded.path,
                    language=excluded.language,
                    modification_time=excluded.modification_time,
                    indexed=excluded.indexed,
                    complete=excluded.complete,
                    line_count=excluded.line_count,
                    file_role=excluded.file_role"
            )?;
            for info in files {
                stmt.execute(params![
                    info.id,
                    info.path.to_string_lossy(),
                    info.language,
                    info.modification_time,
                    i32::from(info.indexed),
                    i32::from(info.complete),
                    info.line_count,
                    info.file_role.as_str(),
                ])?;
            }
        }
        tx.commit()?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    pub fn get_files(&self) -> Result<Vec<FileInfo>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, language, modification_time, indexed, complete, line_count, file_role FROM file",
        )?;
        let file_iter = stmt.query_map([], |row| {
            Ok(FileInfo {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                language: row.get(2)?,
                modification_time: row.get(3)?,
                indexed: row.get::<_, i32>(4)? != 0,
                complete: row.get::<_, i32>(5)? != 0,
                line_count: row.get(6)?,
                file_role: FileRole::from_db_value(&row.get::<_, String>(7)?),
            })
        })?;

        let mut files = Vec::new();
        for file in file_iter {
            files.push(file?);
        }
        Ok(files)
    }

    pub fn get_files_ordered_limit(&self, limit: usize) -> Result<Vec<FileInfo>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, language, modification_time, indexed, complete, line_count
             , file_role
             FROM file
             ORDER BY path ASC, id ASC
             LIMIT ?1",
        )?;
        let file_iter = stmt.query_map(params![limit as i64], |row| {
            Ok(FileInfo {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                language: row.get(2)?,
                modification_time: row.get(3)?,
                indexed: row.get::<_, i32>(4)? != 0,
                complete: row.get::<_, i32>(5)? != 0,
                line_count: row.get(6)?,
                file_role: FileRole::from_db_value(&row.get::<_, String>(7)?),
            })
        })?;

        let mut files = Vec::new();
        for file in file_iter {
            files.push(file?);
        }
        Ok(files)
    }

    pub fn get_files_by_paths(
        &self,
        paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, FileInfo>, StorageError> {
        if paths.is_empty() {
            return Ok(HashMap::new());
        }
        let mut files = HashMap::with_capacity(paths.len());
        for chunk in paths.chunks(500) {
            let placeholders = question_placeholders(chunk.len());
            let sql = format!(
                "SELECT id, path, language, modification_time, indexed, complete, line_count, file_role
                 FROM file
                 WHERE path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(
                chunk.iter().map(|path| path.to_string_lossy().to_string()),
            ))?;
            while let Some(row) = rows.next()? {
                let file = FileInfo {
                    id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    language: row.get(2)?,
                    modification_time: row.get(3)?,
                    indexed: row.get::<_, i32>(4)? != 0,
                    complete: row.get::<_, i32>(5)? != 0,
                    line_count: row.get(6)?,
                    file_role: FileRole::from_db_value(&row.get::<_, String>(7)?),
                };
                files.insert(file.path.clone(), file);
            }
        }
        Ok(files)
    }

    pub fn get_file_roles_by_paths(
        &self,
        paths: &[String],
    ) -> Result<HashMap<String, FileRole>, StorageError> {
        if paths.is_empty() {
            return Ok(HashMap::new());
        }
        let mut roles = HashMap::with_capacity(paths.len());
        for chunk in paths.chunks(500) {
            let placeholders = question_placeholders(chunk.len());
            let sql = format!(
                "SELECT path, file_role
                 FROM file
                 WHERE path IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(chunk.iter().cloned()))?;
            while let Some(row) = rows.next()? {
                let path: String = row.get(0)?;
                let role = FileRole::from_db_value(&row.get::<_, String>(1)?);
                roles.insert(path, role);
            }
        }
        Ok(roles)
    }

    pub fn get_file_node_count(&self) -> Result<i64, StorageError> {
        self.conn
            .query_row(
                "SELECT count(*) FROM node WHERE kind = ?1",
                params![NodeKind::FILE as i32],
                |row| row.get(0),
            )
            .map_err(StorageError::from)
    }

    pub fn get_grounding_file_summary_count(&self) -> Result<u32, StorageError> {
        if self.has_ready_grounding_summary_snapshots()? {
            let snapshot_count: i64 =
                self.conn
                    .query_row("SELECT COUNT(*) FROM grounding_file_snapshot", [], |row| {
                        row.get(0)
                    })?;
            return Ok(clamp_i64_to_u32(snapshot_count));
        }

        Ok(clamp_i64_to_u32(self.effective_grounding_file_count()?))
    }

    pub fn get_grounding_file_summaries(&self) -> Result<Vec<GroundingFileSummary>, StorageError> {
        if self.has_ready_grounding_summary_snapshots()? {
            let mut stmt = self.conn.prepare(
                "SELECT
                    file_id,
                    path,
                    language,
                    modification_time,
                    indexed,
                    complete,
                    line_count,
                    symbol_count,
                    best_node_rank
                 FROM grounding_file_snapshot
                 ORDER BY path",
            )?;
            let mut rows = stmt.query([])?;
            let mut summaries = Vec::new();
            while let Some(row) = rows.next()? {
                summaries.push(Self::grounding_file_summary_from_row(row)?);
            }
            return Ok(summaries);
        }

        let rank_sql = grounding_node_rank_sql("n");
        let indexable = grounding_indexable_predicate("n");
        let query = format!(
            "WITH all_files AS (
                SELECT id, path, language, modification_time, indexed, complete, line_count
                FROM file
                UNION ALL
                SELECT
                    n.id,
                    n.serialized_name,
                    '',
                    0,
                    1,
                    1,
                    0
                FROM node n
                WHERE n.kind = {file_kind}
                    AND NOT EXISTS (SELECT 1 FROM file f WHERE f.id = n.id)
            )
            SELECT
                f.id,
                f.path,
                f.language,
                f.modification_time,
                f.indexed,
                f.complete,
                f.line_count,
                COUNT(n.id) AS symbol_count,
                MIN(CASE WHEN n.id IS NULL THEN 255 ELSE {rank_sql} END) AS best_node_rank
            FROM all_files f
            LEFT JOIN node n
                ON n.file_node_id = f.id
               AND {indexable}
            GROUP BY
                f.id,
                f.path,
                f.language,
                f.modification_time,
                f.indexed,
                f.complete,
                f.line_count
            ORDER BY f.path",
            file_kind = NodeKind::FILE as i32,
        );
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query([])?;
        let mut summaries = Vec::new();
        while let Some(row) = rows.next()? {
            summaries.push(Self::grounding_file_summary_from_row(row)?);
        }
        Ok(summaries)
    }

    pub fn get_grounding_ranked_file_summaries(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<GroundingFileSummary>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        if self.has_ready_grounding_summary_snapshots()? {
            let mut stmt = self.conn.prepare(
                "SELECT
                    file_id,
                    path,
                    language,
                    modification_time,
                    indexed,
                    complete,
                    line_count,
                    symbol_count,
                    best_node_rank
                 FROM grounding_file_snapshot
                 ORDER BY
                    best_node_rank ASC,
                    symbol_count DESC,
                    path ASC
                 LIMIT ?1 OFFSET ?2",
            )?;
            let mut rows = stmt.query(params![
                limit.min(i64::MAX as usize) as i64,
                offset.min(i64::MAX as usize) as i64
            ])?;
            let mut summaries = Vec::new();
            while let Some(row) = rows.next()? {
                summaries.push(Self::grounding_file_summary_from_row(row)?);
            }
            return Ok(summaries);
        }

        let mut summaries = self.get_grounding_file_summaries()?;
        summaries.sort_by(compare_grounding_file_summaries);
        let start = offset.min(summaries.len());
        let end = start.saturating_add(limit).min(summaries.len());
        Ok(summaries[start..end].to_vec())
    }

    pub fn get_grounding_top_symbols_for_files(
        &self,
        file_ids: &[i64],
        per_file_limit: usize,
    ) -> Result<Vec<GroundingNodeRecord>, StorageError> {
        if file_ids.is_empty() || per_file_limit == 0 {
            return Ok(Vec::new());
        }

        if self.has_ready_grounding_summary_snapshots()? {
            let placeholders = numbered_placeholders(2, file_ids.len());
            let query = format!(
                "SELECT
                    node_id,
                    kind,
                    serialized_name,
                    qualified_name,
                    canonical_id,
                    file_node_id,
                    start_line,
                    start_col,
                    end_line,
                    end_col,
                    display_name,
                    file_path
                 FROM grounding_node_snapshot
                 WHERE file_symbol_rank <= ?1
                   AND file_node_id IN ({placeholders})
                 ORDER BY file_node_id, file_symbol_rank"
            );
            let mut params = Vec::with_capacity(file_ids.len() + 1);
            params.push(Value::Integer(per_file_limit.min(i64::MAX as usize) as i64));
            params.extend(file_ids.iter().map(|id| Value::Integer(*id)));
            let mut stmt = self.conn.prepare(&query)?;
            let mut rows = stmt.query(params_from_iter(params))?;
            let mut nodes = Vec::new();
            while let Some(row) = rows.next()? {
                nodes.push(GroundingNodeRecord {
                    node: Self::node_from_row(row)?,
                    display_name: row.get(10)?,
                    file_path: row.get::<_, Option<String>>(11)?.map(PathBuf::from),
                });
            }
            return Ok(nodes);
        }

        let placeholders = numbered_placeholders(2, file_ids.len());
        let rank_sql = grounding_node_rank_sql("n");
        let indexable = grounding_indexable_predicate("n");
        let display_name = grounding_display_name_expr("n");
        let query = format!(
            "WITH ranked AS (
                SELECT
                    n.id,
                    n.kind,
                    n.serialized_name,
                    n.qualified_name,
                    n.canonical_id,
                    n.file_node_id,
                    n.start_line,
                    n.start_col,
                    n.end_line,
                    n.end_col,
                    {display_name} AS display_name,
                    COALESCE(f.path, file_node.serialized_name) AS file_path,
                    ROW_NUMBER() OVER (
                        PARTITION BY n.file_node_id
                        ORDER BY
                            {rank_sql},
                            COALESCE(n.start_line, 2147483647),
                            {display_name},
                            n.id
                    ) AS row_num
                FROM node n
                LEFT JOIN file f ON f.id = n.file_node_id
                LEFT JOIN node file_node
                    ON file_node.id = n.file_node_id
                   AND file_node.kind = {file_kind}
                WHERE {indexable}
                  AND n.file_node_id IN ({placeholders})
            )
            SELECT
                id,
                kind,
                serialized_name,
                qualified_name,
                canonical_id,
                file_node_id,
                start_line,
                start_col,
                end_line,
                end_col,
                display_name,
                file_path
            FROM ranked
            WHERE row_num <= ?1
            ORDER BY file_node_id, row_num",
            file_kind = NodeKind::FILE as i32,
        );
        let mut params = Vec::with_capacity(file_ids.len() + 1);
        params.push(Value::Integer(per_file_limit.min(i64::MAX as usize) as i64));
        params.extend(file_ids.iter().map(|id| Value::Integer(*id)));
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(GroundingNodeRecord {
                node: Self::node_from_row(row)?,
                display_name: row.get(10)?,
                file_path: row.get::<_, Option<String>>(11)?.map(PathBuf::from),
            });
        }
        Ok(nodes)
    }

    pub fn get_grounding_root_symbol_candidates(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<GroundingNodeRecord>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        if self.has_ready_grounding_summary_snapshots()? {
            let mut stmt = self.conn.prepare(
                "SELECT
                    node_id,
                    kind,
                    serialized_name,
                    qualified_name,
                    canonical_id,
                    file_node_id,
                    start_line,
                    start_col,
                    end_line,
                    end_col,
                    display_name,
                    file_path
                 FROM grounding_node_snapshot
                 WHERE is_root = 1
                 ORDER BY
                    node_rank,
                    sort_start_line,
                    display_name,
                    node_id
                 LIMIT ?1 OFFSET ?2",
            )?;
            let mut rows = stmt.query(params![
                limit.min(i64::MAX as usize) as i64,
                offset.min(i64::MAX as usize) as i64
            ])?;
            let mut nodes = Vec::new();
            while let Some(row) = rows.next()? {
                nodes.push(GroundingNodeRecord {
                    node: Self::node_from_row(row)?,
                    display_name: row.get(10)?,
                    file_path: row.get::<_, Option<String>>(11)?.map(PathBuf::from),
                });
            }
            return Ok(nodes);
        }

        let rank_sql = grounding_node_rank_sql("n");
        let display_name = grounding_display_name_expr("n");
        let indexable = grounding_indexable_predicate("n");
        let query = format!(
            "SELECT
                n.id,
                n.kind,
                n.serialized_name,
                n.qualified_name,
                n.canonical_id,
                n.file_node_id,
                n.start_line,
                n.start_col,
                n.end_line,
                n.end_col,
                {display_name} AS display_name,
                COALESCE(f.path, file_node.serialized_name) AS file_path
            FROM node n
            LEFT JOIN file f ON f.id = n.file_node_id
            LEFT JOIN node file_node
                ON file_node.id = n.file_node_id
               AND file_node.kind = {file_kind}
            WHERE {indexable}
              AND NOT EXISTS (
                    SELECT 1
                    FROM edge e
                    WHERE e.kind = {member_kind}
                      AND e.target_node_id = n.id
                )
            ORDER BY
                {rank_sql},
                COALESCE(n.start_line, 2147483647),
                {display_name},
                n.id
            LIMIT ?1 OFFSET ?2",
            file_kind = NodeKind::FILE as i32,
            member_kind = EdgeKind::MEMBER as i32,
        );
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query(params![
            limit.min(i64::MAX as usize) as i64,
            offset.min(i64::MAX as usize) as i64
        ])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(GroundingNodeRecord {
                node: Self::node_from_row(row)?,
                display_name: row.get(10)?,
                file_path: row.get::<_, Option<String>>(11)?.map(PathBuf::from),
            });
        }
        Ok(nodes)
    }

    pub fn get_grounding_member_counts(
        &self,
        node_ids: &[NodeId],
    ) -> Result<HashMap<NodeId, u32>, StorageError> {
        if node_ids.is_empty() {
            return Ok(HashMap::new());
        }

        if self.has_ready_grounding_detail_snapshots()? {
            let placeholders = question_placeholders(node_ids.len());
            let query = format!(
                "SELECT node_id, member_count
                 FROM grounding_node_summary_snapshot
                 WHERE node_id IN ({placeholders})
                   AND member_count > 0"
            );
            let mut stmt = self.conn.prepare(&query)?;
            let mut rows = stmt.query(params_from_iter(node_ids.iter().map(|id| id.0)))?;
            let mut counts = HashMap::new();
            while let Some(row) = rows.next()? {
                counts.insert(NodeId(row.get(0)?), clamp_i64_to_u32(row.get::<_, i64>(1)?));
            }
            return Ok(counts);
        }

        let placeholders = question_placeholders(node_ids.len());
        let query = format!(
            "SELECT source_node_id, COUNT(*)
            FROM edge
            WHERE kind = ?1
              AND source_node_id IN ({placeholders})
            GROUP BY source_node_id"
        );
        let mut params = Vec::with_capacity(node_ids.len() + 1);
        params.push(Value::Integer(EdgeKind::MEMBER as i32 as i64));
        params.extend(node_ids.iter().map(|id| Value::Integer(id.0)));
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        let mut counts = HashMap::new();
        while let Some(row) = rows.next()? {
            counts.insert(NodeId(row.get(0)?), clamp_i64_to_u32(row.get::<_, i64>(1)?));
        }
        Ok(counts)
    }

    pub fn get_grounding_min_occurrence_lines(
        &self,
        node_ids: &[NodeId],
    ) -> Result<HashMap<NodeId, u32>, StorageError> {
        if node_ids.is_empty() {
            return Ok(HashMap::new());
        }

        if self.has_ready_grounding_detail_snapshots()? {
            let placeholders = question_placeholders(node_ids.len());
            let query = format!(
                "SELECT node_id, fallback_occurrence_line
                 FROM grounding_node_summary_snapshot
                 WHERE node_id IN ({placeholders})
                   AND fallback_occurrence_line IS NOT NULL"
            );
            let mut stmt = self.conn.prepare(&query)?;
            let mut rows = stmt.query(params_from_iter(node_ids.iter().map(|id| id.0)))?;
            let mut counts = HashMap::new();
            while let Some(row) = rows.next()? {
                counts.insert(NodeId(row.get(0)?), row.get(1)?);
            }
            return Ok(counts);
        }

        let placeholders = question_placeholders(node_ids.len());
        let query = format!(
            "SELECT element_id, start_line
            FROM occurrence
            WHERE element_id IN ({placeholders})
            ORDER BY element_id ASC, rowid ASC"
        );
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query(params_from_iter(node_ids.iter().map(|id| id.0)))?;
        let mut counts = HashMap::new();
        while let Some(row) = rows.next()? {
            counts.entry(NodeId(row.get(0)?)).or_insert(row.get(1)?);
        }
        Ok(counts)
    }

    pub fn get_grounding_edge_digest_counts(
        &self,
        node_ids: &[NodeId],
    ) -> Result<Vec<GroundingEdgeKindCount>, StorageError> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }

        if self.has_ready_grounding_detail_snapshots()? {
            let placeholders = question_placeholders(node_ids.len());
            let query = format!(
                "SELECT node_id, kind, count
                 FROM grounding_node_edge_digest_snapshot
                 WHERE node_id IN ({placeholders})
                 ORDER BY node_id ASC, kind ASC"
            );
            let mut stmt = self.conn.prepare(&query)?;
            let mut rows = stmt.query(params_from_iter(node_ids.iter().map(|id| id.0)))?;
            let mut counts = Vec::new();
            while let Some(row) = rows.next()? {
                counts.push(GroundingEdgeKindCount {
                    node_id: NodeId(row.get(0)?),
                    kind: EdgeKind::try_from(row.get::<_, i32>(1)?)?,
                    count: clamp_i64_to_u32(row.get::<_, i64>(2)?),
                });
            }
            return Ok(counts);
        }

        let source_placeholders = numbered_placeholders(1, node_ids.len());
        let target_placeholders = numbered_placeholders(1 + node_ids.len(), node_ids.len());
        let resolved_source_placeholders =
            numbered_placeholders(1 + node_ids.len() * 2, node_ids.len());
        let resolved_target_placeholders =
            numbered_placeholders(1 + node_ids.len() * 3, node_ids.len());
        let query = format!(
            "{EDGE_SELECT_BASE}
             WHERE e.source_node_id IN ({source_placeholders})
                OR e.target_node_id IN ({target_placeholders})
                OR e.resolved_source_node_id IN ({resolved_source_placeholders})
                OR e.resolved_target_node_id IN ({resolved_target_placeholders})
             ORDER BY e.id"
        );
        let params = node_ids
            .iter()
            .map(|id| Value::from(id.0))
            .chain(node_ids.iter().map(|id| Value::from(id.0)))
            .chain(node_ids.iter().map(|id| Value::from(id.0)))
            .chain(node_ids.iter().map(|id| Value::from(id.0)));
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        let node_id_set = node_ids.iter().copied().collect::<HashSet<_>>();
        let mut counts_by_node = HashMap::<(NodeId, EdgeKind), u32>::new();
        while let Some(row) = rows.next()? {
            let mut edge = Self::edge_from_row(row)?;
            let target_symbol: String = row.get(12)?;
            if edge.kind == EdgeKind::CALL
                && edge.resolved_target.is_some()
                && should_ignore_call_resolution(&target_symbol, edge.certainty, edge.confidence)
            {
                edge.resolved_target = None;
                edge.confidence = None;
                edge.certainty = None;
            }

            let (source, target) = edge.effective_endpoints();
            if node_id_set.contains(&source) {
                *counts_by_node.entry((source, edge.kind)).or_insert(0) += 1;
            }
            if target != source && node_id_set.contains(&target) {
                *counts_by_node.entry((target, edge.kind)).or_insert(0) += 1;
            }
        }
        let mut counts = Vec::new();
        for ((node_id, kind), count) in counts_by_node {
            counts.push(GroundingEdgeKindCount {
                node_id,
                kind,
                count,
            });
        }
        counts.sort_by(|left, right| {
            left.node_id
                .0
                .cmp(&right.node_id.0)
                .then((left.kind as i32).cmp(&(right.kind as i32)))
        });
        Ok(counts)
    }

    pub fn get_file_by_path(&self, path: &Path) -> Result<Option<FileInfo>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, language, modification_time, indexed, complete, line_count, file_role FROM file WHERE path = ?1",
        )?;
        let mut rows = stmt.query(params![path.to_string_lossy()])?;

        if let Some(row) = rows.next()? {
            Ok(Some(FileInfo {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                language: row.get(2)?,
                modification_time: row.get(3)?,
                indexed: row.get::<_, i32>(4)? != 0,
                complete: row.get::<_, i32>(5)? != 0,
                line_count: row.get(6)?,
                file_role: FileRole::from_db_value(&row.get::<_, String>(7)?),
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_node_kinds_for_files(
        &self,
        file_ids: &[i64],
    ) -> Result<Vec<(NodeId, NodeKind)>, StorageError> {
        if file_ids.is_empty() {
            return Ok(Vec::new());
        }

        let file_placeholders = question_placeholders(file_ids.len());
        let sql = format!(
            "SELECT id, kind
             FROM node
             WHERE id IN ({file_placeholders})
                OR file_node_id IN ({file_placeholders})"
        );
        let params = file_ids
            .iter()
            .copied()
            .chain(file_ids.iter().copied())
            .collect::<Vec<_>>();
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let raw_kind: i32 = row.get(1)?;
            if let Ok(kind) = NodeKind::try_from(raw_kind) {
                out.push((NodeId(row.get(0)?), kind));
            }
        }
        Ok(out)
    }

    pub fn get_nodes_for_file_line(
        &self,
        path: &str,
        line: u32,
    ) -> Result<Vec<Node>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.kind, n.serialized_name, n.qualified_name, n.canonical_id, n.file_node_id, n.start_line, n.start_col, n.end_line, n.end_col FROM node n
             JOIN occurrence o ON n.id = o.element_id
             JOIN file f ON o.file_node_id = f.id
             WHERE f.path = ?1 AND ?2 >= o.start_line AND ?2 <= o.end_line",
        )?;
        let mut nodes = Vec::new();
        let mut rows = stmt.query(params![path, line])?;
        while let Some(row) = rows.next()? {
            nodes.push(Self::node_from_row(row)?);
        }
        Ok(nodes)
    }

    pub fn insert_error(
        &self,
        error: &codestory_contracts::graph::ErrorInfo,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO error (message, file_id, line, column, fatal, indexed) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                error.message,
                error.file_id.map(|id| id.0),
                error.line,
                error.column,
                error.is_fatal as i32,
                (error.index_step == codestory_contracts::graph::IndexStep::Indexing) as i32,
            ],
        )?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    pub fn insert_errors_batch(
        &mut self,
        errors: &[codestory_contracts::graph::ErrorInfo],
    ) -> Result<(), StorageError> {
        if errors.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO error (message, file_id, line, column, fatal, indexed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for error in errors {
                stmt.execute(params![
                    error.message,
                    error.file_id.map(|id| id.0),
                    error.line,
                    error.column,
                    error.is_fatal as i32,
                    (error.index_step == codestory_contracts::graph::IndexStep::Indexing) as i32,
                ])?;
            }
        }
        tx.commit()?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    /// Get symbols that have no parent (root namespaces, top-level classes, etc.)
    pub fn get_root_symbols(&self) -> Result<Vec<Node>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node
             WHERE id NOT IN (SELECT target_node_id FROM edge WHERE kind = ?1)
             AND kind != ?2", // Exclude files from symbol tree roots for now
        )?;
        let kind_member = codestory_contracts::graph::EdgeKind::MEMBER as i32;
        let kind_file = codestory_contracts::graph::NodeKind::FILE as i32;

        let mut nodes = Vec::new();
        let mut rows = stmt.query(params![kind_member, kind_file])?;
        while let Some(row) = rows.next()? {
            nodes.push(Self::node_from_row(row)?);
        }
        Ok(nodes)
    }

    /// Get children symbols for a parent symbol (members of a class/namespace)
    pub fn get_children_symbols(&self, parent_id: NodeId) -> Result<Vec<Node>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.kind, n.serialized_name, n.qualified_name, n.canonical_id, n.file_node_id, n.start_line, n.start_col, n.end_line, n.end_col FROM node n
             JOIN edge e ON n.id = e.target_node_id
             WHERE e.source_node_id = ?1 AND e.kind = ?2",
        )?;
        let kind_member = codestory_contracts::graph::EdgeKind::MEMBER as i32;

        let mut nodes = Vec::new();
        let mut rows = stmt.query(params![parent_id.0, kind_member])?;
        while let Some(row) = rows.next()? {
            nodes.push(Self::node_from_row(row)?);
        }
        Ok(nodes)
    }

    /// Return store counts, preferring ready summary snapshots when available.
    pub fn get_stats(&self) -> Result<StorageStats, StorageError> {
        let fatal_error_count = self.fatal_error_count()?;
        if self.has_ready_grounding_summary_snapshots()? {
            let mut stmt = self.conn.prepare(
                "SELECT node_count, edge_count, file_count, error_count
                 FROM grounding_repo_stats_snapshot
                 WHERE id = 1",
            )?;
            let mut rows = stmt.query([])?;
            if let Some(row) = rows.next()? {
                return Ok(StorageStats {
                    node_count: row.get(0)?,
                    edge_count: row.get(1)?,
                    file_count: row.get(2)?,
                    error_count: row.get(3)?,
                    fatal_error_count,
                });
            }
        }

        let node_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM node", [], |r| r.get(0))?;
        let edge_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM edge", [], |r| r.get(0))?;
        let file_count = self.effective_grounding_file_count()?;
        let error_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM error", [], |r| r.get(0))?;

        Ok(StorageStats {
            node_count,
            edge_count,
            file_count,
            error_count,
            fatal_error_count,
        })
    }

    fn fatal_error_count(&self) -> Result<i64, StorageError> {
        self.conn
            .query_row("SELECT count(*) FROM error WHERE fatal = 1", [], |r| {
                r.get(0)
            })
            .map_err(StorageError::from)
    }

    /// Delete all graph/search projection data linked to one canonical file node.
    pub fn delete_file_projection(
        &mut self,
        file_node_id: i64,
    ) -> Result<FileProjectionRemovalSummary, StorageError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS related_node_ids (
                node_id INTEGER PRIMARY KEY
             );
             DELETE FROM related_node_ids;",
        )?;
        tx.execute(
            &format!(
                "INSERT INTO {RELATED_NODE_IDS_TABLE} (node_id)
                 SELECT DISTINCT id FROM ({RELATED_NODE_SUBQUERY})"
            ),
            params![file_node_id],
        )?;

        let mut related_node_ids = Vec::new();
        {
            let mut node_ids_stmt =
                tx.prepare(&format!("SELECT node_id FROM {RELATED_NODE_IDS_TABLE}"))?;
            let mut node_rows = node_ids_stmt.query([])?;
            while let Some(row) = node_rows.next()? {
                related_node_ids.push(row.get::<_, i64>(0)?);
            }
        }

        let outside_related_file_edges = outside_related_file_edge_predicate("?1");

        tx.execute(
            &format!(
                "UPDATE edge
                 SET resolved_source_node_id = NULL
                 WHERE resolved_source_node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 AND {outside_related_file_edges}"
            ),
            params![file_node_id],
        )?;

        tx.execute(
            &format!(
                "UPDATE edge
                 SET resolved_target_node_id = NULL,
                     confidence = NULL,
                     certainty = NULL,
                     candidate_target_node_ids = NULL
                 WHERE resolved_target_node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 AND {outside_related_file_edges}"
            ),
            params![file_node_id],
        )?;

        let removed_edges = tx.execute(
            &format!(
                "DELETE FROM edge
                 WHERE source_node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 OR target_node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 OR file_node_id = ?1"
            ),
            params![file_node_id],
        )?;

        let removed_occurrences = tx.execute(
            &format!(
                "DELETE FROM occurrence
                 WHERE file_node_id = ?1
                 OR element_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})"
            ),
            params![file_node_id],
        )?;

        let removed_bookmarks = tx.execute(
            &format!(
                "DELETE FROM bookmark_node WHERE node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})"
            ),
            [],
        )?;

        let removed_component_access = tx.execute(
            &format!(
                "DELETE FROM component_access WHERE node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})"
            ),
            [],
        )?;

        let removed_callable_projection_state_count = tx.execute(
            &format!(
                "DELETE FROM callable_projection_state
                 WHERE file_id = ?1
                 OR node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})"
            ),
            params![file_node_id],
        )?;

        let removed_local_symbols = tx.execute(
            "DELETE FROM local_symbol WHERE file_id = ?1",
            params![file_node_id],
        )?;

        tx.execute(
            &format!(
                "DELETE FROM llm_symbol_doc
                 WHERE node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 OR file_node_id = ?1"
            ),
            params![file_node_id],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM symbol_search_doc
                 WHERE node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 OR file_node_id = ?1"
            ),
            params![file_node_id],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM search_symbol_projection
                 WHERE node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})"
            ),
            [],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM symbol_summary
                 WHERE node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})"
            ),
            [],
        )?;

        // Remove any node references in other projection tables.
        let removed_nodes = tx.execute(
            &format!("DELETE FROM node WHERE id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})"),
            [],
        )?;

        let removed_errors = tx.execute(
            "DELETE FROM error WHERE file_id = ?1",
            params![file_node_id],
        )?;

        let removed_file_rows =
            tx.execute("DELETE FROM file WHERE id = ?1", params![file_node_id])?;

        tx.commit()?;
        self.invalidate_grounding_snapshots()?;

        {
            let mut nodes = self.cache.nodes.write();
            for node_id in related_node_ids {
                nodes.remove(&NodeId(node_id));
            }
        }

        Ok(FileProjectionRemovalSummary {
            canonical_file_node_id: file_node_id,
            removed_node_count: removed_nodes,
            removed_edge_count: removed_edges,
            removed_occurrence_count: removed_occurrences,
            removed_error_count: removed_errors,
            removed_bookmark_node_count: removed_bookmarks,
            removed_component_access_count: removed_component_access,
            removed_local_symbol_count: removed_local_symbols,
            removed_file_row_count: removed_file_rows,
            removed_callable_projection_state_count,
        })
    }

    /// Delete a file and all associated projection data.
    pub fn delete_file(&mut self, file_id: i64) -> Result<(), StorageError> {
        self.delete_file_projection(file_id)?;
        Ok(())
    }

    /// Delete multiple files by their IDs
    pub fn delete_files_batch(&mut self, file_ids: &[i64]) -> Result<(), StorageError> {
        for id in file_ids {
            self.delete_file(*id)?;
        }
        Ok(())
    }

    // ========================================================================
    // Error Management
    // ========================================================================

    /// Get all errors with optional filtering
    pub fn get_errors(
        &self,
        filter: Option<&codestory_contracts::graph::ErrorFilter>,
    ) -> Result<Vec<codestory_contracts::graph::ErrorInfo>, StorageError> {
        let base_query = "SELECT id, message, file_id, line, column, fatal, indexed FROM error";
        let mut conditions = Vec::new();

        if let Some(f) = filter {
            if f.fatal_only {
                conditions.push("fatal = 1");
            }
            if f.indexed_only {
                conditions.push("indexed = 1");
            }
        }

        let query = if conditions.is_empty() {
            base_query.to_string()
        } else {
            format!("{} WHERE {}", base_query, conditions.join(" AND "))
        };

        let mut stmt = self.conn.prepare(&query)?;
        let mut errors = Vec::new();
        let mut rows = stmt.query([])?;

        while let Some(row) = rows.next()? {
            let fatal: i32 = row.get(5)?;
            let indexed: i32 = row.get(6)?;
            errors.push(codestory_contracts::graph::ErrorInfo {
                message: row.get(1)?,
                file_id: row.get::<_, Option<i64>>(2)?.map(NodeId),
                line: row.get(3)?,
                column: row.get(4)?,
                is_fatal: fatal != 0,
                index_step: if indexed != 0 {
                    codestory_contracts::graph::IndexStep::Indexing
                } else {
                    codestory_contracts::graph::IndexStep::Collection
                },
            });
        }
        Ok(errors)
    }

    /// Clear all errors
    pub fn clear_errors(&self) -> Result<(), StorageError> {
        self.conn.execute("DELETE FROM error", [])?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    /// Replace errors for reprocessed files after their latest projection is durable.
    pub fn replace_errors_for_files_batch(
        &mut self,
        file_ids: &[i64],
        errors: &[codestory_contracts::graph::ErrorInfo],
    ) -> Result<(), StorageError> {
        if file_ids.is_empty() {
            return Ok(());
        }

        let file_ids = file_ids.iter().copied().collect::<HashSet<_>>();
        debug_assert!(errors.iter().all(|error| {
            error
                .file_id
                .is_some_and(|file_id| file_ids.contains(&file_id.0))
        }));

        let tx = self.conn.transaction()?;
        let mut removed_error_count = 0;
        {
            let mut delete = tx.prepare("DELETE FROM error WHERE file_id = ?1")?;
            for file_id in &file_ids {
                removed_error_count += delete.execute(params![file_id])?;
            }
        }
        {
            let mut insert = tx.prepare(
                "INSERT INTO error (message, file_id, line, column, fatal, indexed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for error in errors {
                insert.execute(params![
                    error.message,
                    error.file_id.map(|id| id.0),
                    error.line,
                    error.column,
                    error.is_fatal as i32,
                    (error.index_step == codestory_contracts::graph::IndexStep::Indexing) as i32,
                ])?;
            }
        }
        tx.commit()?;
        if removed_error_count > 0 || !errors.is_empty() {
            self.invalidate_grounding_snapshots()?;
        }
        Ok(())
    }

    // ========================================================================
    // Bookmark Management
    // ========================================================================

    /// Create a bookmark category
    pub fn create_bookmark_category(&self, name: &str) -> Result<i64, StorageError> {
        bookmarks::create_bookmark_category(&self.conn, name)
    }

    /// Get all bookmark categories
    pub fn get_bookmark_categories(&self) -> Result<Vec<BookmarkCategory>, StorageError> {
        bookmarks::get_bookmark_categories(&self.conn)
    }

    /// Delete a bookmark category and all its bookmarks
    pub fn delete_bookmark_category(&self, id: i64) -> Result<(), StorageError> {
        bookmarks::delete_bookmark_category(&self.conn, id)
    }

    /// Rename a bookmark category
    pub fn rename_bookmark_category(&self, id: i64, new_name: &str) -> Result<bool, StorageError> {
        bookmarks::rename_bookmark_category(&self.conn, id, new_name)
    }

    /// Add a bookmark to a category
    pub fn add_bookmark(
        &self,
        category_id: i64,
        node_id: NodeId,
        comment: Option<&str>,
    ) -> Result<i64, StorageError> {
        bookmarks::add_bookmark(&self.conn, category_id, node_id, comment)
    }

    /// Get bookmarks, optionally filtered by category
    pub fn get_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<Bookmark>, StorageError> {
        bookmarks::get_bookmarks(&self.conn, category_id)
    }

    /// Update a bookmark's comment
    pub fn update_bookmark_comment(&self, id: i64, comment: &str) -> Result<(), StorageError> {
        bookmarks::update_bookmark_comment(&self.conn, id, comment)
    }

    /// Update bookmark fields.
    pub fn update_bookmark(
        &self,
        id: i64,
        category_id: Option<i64>,
        comment: Option<Option<&str>>,
    ) -> Result<(), StorageError> {
        bookmarks::update_bookmark(&self.conn, id, category_id, comment)
    }

    /// Delete a bookmark
    pub fn delete_bookmark(&self, id: i64) -> Result<(), StorageError> {
        bookmarks::delete_bookmark(&self.conn, id)
    }

    // ========================================================================
    // Trail Query (BFS-based subgraph exploration)
    // ========================================================================

    /// Get a trail (subgraph) starting from a root node up to a certain depth
    pub fn get_trail(&self, config: &TrailConfig) -> Result<TrailResult, StorageError> {
        trail::get_trail(self, config)
    }

    /// Helper: Get edges for a node in a specific direction
    fn get_edges_for_node(
        &self,
        node_id: NodeId,
        direction: &TrailDirection,
        edge_filter: &[EdgeKind],
        caller_scope: TrailCallerScope,
        show_utility_calls: bool,
    ) -> Result<Vec<Edge>, StorageError> {
        trail::get_edges_for_node(
            self,
            node_id,
            direction,
            edge_filter,
            caller_scope,
            show_utility_calls,
        )
    }

    /// Get all edges connected to a node (both directions)
    pub fn get_edges_for_node_id(&self, node_id: NodeId) -> Result<Vec<Edge>, StorageError> {
        trail::get_edges_for_node_id(self, node_id)
    }

    /// Get direct incoming edges for a node using the same filters as trail traversal.
    pub fn get_incoming_edges_for_node_id(
        &self,
        node_id: NodeId,
        edge_filter: &[EdgeKind],
        caller_scope: TrailCallerScope,
        show_utility_calls: bool,
    ) -> Result<Vec<Edge>, StorageError> {
        trail::get_edges_for_node(
            self,
            node_id,
            &TrailDirection::Incoming,
            edge_filter,
            caller_scope,
            show_utility_calls,
        )
    }
}

fn neighbor_for_direction(
    current_id: NodeId,
    direction: TrailDirection,
    edge: &Edge,
) -> Option<NodeId> {
    let (eff_source, eff_target) = edge.effective_endpoints();
    match direction {
        TrailDirection::Outgoing => {
            if eff_source == current_id {
                Some(eff_target)
            } else if edge.source == current_id {
                Some(edge.target)
            } else {
                None
            }
        }
        TrailDirection::Incoming => {
            if eff_target == current_id {
                Some(eff_source)
            } else if edge.target == current_id {
                Some(edge.source)
            } else {
                None
            }
        }
        TrailDirection::Both => {
            if eff_source == current_id {
                Some(eff_target)
            } else if eff_target == current_id {
                Some(eff_source)
            } else if edge.source == current_id {
                Some(edge.target)
            } else if edge.target == current_id {
                Some(edge.source)
            } else {
                None
            }
        }
    }
}

fn apply_trail_node_filter(result: &mut TrailResult, config: &TrailConfig) {
    if config.node_filter.is_empty() {
        return;
    }

    let mut allowed: HashSet<NodeId> = result
        .nodes
        .iter()
        .filter(|node| config.node_filter.contains(&node.kind))
        .map(|node| node.id)
        .collect();

    // Always keep endpoints.
    allowed.insert(config.root_id);
    if let Some(target) = config.target_id {
        allowed.insert(target);
    }

    result.nodes.retain(|node| allowed.contains(&node.id));
    result.edges.retain(|edge| {
        let (s, t) = edge.effective_endpoints();
        allowed.contains(&s) && allowed.contains(&t)
    });
    result.depth_map.retain(|id, _| allowed.contains(id));
}

fn is_caller_scope_allowed(scope: TrailCallerScope, caller_file_path: Option<&str>) -> bool {
    match scope {
        TrailCallerScope::IncludeTestsAndBenches => true,
        TrailCallerScope::ProductionOnly => caller_file_path
            .map(|path| !is_test_or_bench_path(path))
            .unwrap_or(true),
    }
}

fn is_test_or_bench_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    normalized.starts_with("tests/")
        || normalized.starts_with("test/")
        || normalized.starts_with("benches/")
        || normalized.starts_with("bench/")
        || normalized.starts_with("__tests__/")
        || normalized.starts_with("__test__/")
        || normalized.contains("/tests/")
        || normalized.contains("/test/")
        || normalized.contains("/__tests__/")
        || normalized.contains("/__test__/")
        || normalized.contains("/benches/")
        || normalized.contains("/bench/")
        || normalized.ends_with("_test.rs")
        || normalized.contains(".test.")
        || normalized.contains(".spec.")
}

fn should_ignore_call_resolution(
    target_symbol: &str,
    certainty: Option<ResolutionCertainty>,
    confidence: Option<f32>,
) -> bool {
    if is_indexer_helper_call(target_symbol) {
        return false;
    }

    let certainty = certainty.or_else(|| ResolutionCertainty::from_confidence(confidence));

    let Some(certainty) = certainty else {
        return false;
    };

    if matches!(certainty, ResolutionCertainty::Uncertain) {
        return true;
    }

    // For very common unqualified methods, only keep high-certainty resolutions.
    if is_common_unqualified_call_name(target_symbol)
        && !matches!(certainty, ResolutionCertainty::Certain)
    {
        return true;
    }

    false
}

fn is_indexer_helper_call(name: &str) -> bool {
    name.contains("seed_symbol_table")
        || name.contains("flush_projection_batch")
        || name.contains("flush_errors")
}

fn is_common_unqualified_call_name(name: &str) -> bool {
    if name.contains("::") || name.contains('.') {
        return false;
    }

    matches!(
        name,
        "add"
            | "all"
            | "any"
            | "append"
            | "clear"
            | "collect"
            | "contains"
            | "dedup"
            | "extend"
            | "filter"
            | "insert"
            | "into_iter"
            | "iter"
            | "iter_mut"
            | "len"
            | "map"
            | "pop"
            | "push"
            | "remove"
            | "retain"
            | "sort"
            | "sort_by"
            | "sort_by_key"
            | "truncate"
    )
}

#[cfg(test)]
mod grounding_snapshot_fast_path_tests {
    use super::*;

    fn insert_grounding_test_file(
        storage: &mut Storage,
        file_id: i64,
        path: &str,
        symbols: &[(i64, NodeKind, &str, u32)],
    ) -> Result<(), StorageError> {
        storage.insert_file(&FileInfo {
            id: file_id,
            path: PathBuf::from(path),
            language: "rust".to_string(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 32,
            file_role: FileRole::classify_path(Path::new(path)),
        })?;

        let mut nodes = vec![Node {
            id: NodeId(file_id),
            kind: NodeKind::FILE,
            serialized_name: path.to_string(),
            ..Default::default()
        }];
        for (node_id, kind, name, start_line) in symbols {
            nodes.push(Node {
                id: NodeId(*node_id),
                kind: *kind,
                serialized_name: (*name).to_string(),
                file_node_id: Some(NodeId(file_id)),
                start_line: Some(*start_line),
                ..Default::default()
            });
        }
        storage.insert_nodes_batch(&nodes)
    }

    #[test]
    fn test_grounding_file_summary_count_includes_file_nodes_without_file_rows()
    -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;
        storage.insert_nodes_batch(&[Node {
            id: NodeId(700),
            kind: NodeKind::FILE,
            serialized_name: "orphan.rs".to_string(),
            ..Default::default()
        }])?;

        assert_eq!(storage.get_grounding_file_summary_count()?, 1);
        assert_eq!(storage.get_stats()?.file_count, 1);

        storage.refresh_grounding_snapshots()?;

        assert_eq!(storage.get_grounding_file_summary_count()?, 1);
        assert_eq!(storage.get_stats()?.file_count, 1);
        assert_eq!(storage.get_grounding_file_summaries()?.len(), 1);
        Ok(())
    }

    #[test]
    fn test_grounding_ranked_file_summaries_match_snapshot_ordering() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;
        insert_grounding_test_file(
            &mut storage,
            10,
            "src/a.rs",
            &[(101, NodeKind::FUNCTION, "alpha", 10)],
        )?;
        insert_grounding_test_file(
            &mut storage,
            20,
            "src/b.rs",
            &[
                (201, NodeKind::CLASS, "Widget", 2),
                (202, NodeKind::FUNCTION, "helper", 20),
            ],
        )?;
        insert_grounding_test_file(
            &mut storage,
            30,
            "src/c.rs",
            &[(301, NodeKind::CLASS, "Controller", 3)],
        )?;

        let fallback_ids = storage
            .get_grounding_ranked_file_summaries(2, 1)?
            .into_iter()
            .map(|summary| summary.file.id)
            .collect::<Vec<_>>();
        assert_eq!(fallback_ids, vec![30, 10]);

        storage.refresh_grounding_snapshots()?;

        let snapshot_ids = storage
            .get_grounding_ranked_file_summaries(2, 1)?
            .into_iter()
            .map(|summary| summary.file.id)
            .collect::<Vec<_>>();
        assert_eq!(snapshot_ids, vec![30, 10]);

        Ok(())
    }

    #[test]
    fn test_grounding_summary_refresh_keeps_detail_tier_dirty() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;
        insert_grounding_test_file(
            &mut storage,
            10,
            "src/lib.rs",
            &[(101, NodeKind::STRUCT, "Controller", 2)],
        )?;
        storage.insert_nodes_batch(&[Node {
            id: NodeId(102),
            kind: NodeKind::FIELD,
            serialized_name: "value".to_string(),
            file_node_id: Some(NodeId(10)),
            start_line: Some(3),
            ..Default::default()
        }])?;
        storage.insert_edges_batch(&[Edge {
            id: codestory_contracts::graph::EdgeId(1),
            source: NodeId(101),
            target: NodeId(102),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        }])?;

        storage.refresh_grounding_summary_snapshots()?;

        assert!(storage.has_ready_grounding_summary_snapshots()?);
        assert!(!storage.has_ready_grounding_detail_snapshots()?);
        assert_eq!(storage.get_grounding_file_summary_count()?, 1);
        assert_eq!(
            storage
                .get_grounding_member_counts(&[NodeId(101)])?
                .get(&NodeId(101)),
            Some(&1)
        );

        storage.hydrate_grounding_detail_snapshots()?;

        assert!(storage.has_ready_grounding_summary_snapshots()?);
        assert!(storage.has_ready_grounding_detail_snapshots()?);
        assert_eq!(
            storage
                .get_grounding_member_counts(&[NodeId(101)])?
                .get(&NodeId(101)),
            Some(&1)
        );
        Ok(())
    }
}

pub use retrieval_manifest::RetrievalIndexManifest;

#[cfg(test)]
mod tests;
