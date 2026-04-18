use codestory_contracts::graph::{
    AccessKind, Bookmark, BookmarkCategory, CallableProjectionState, Edge, EdgeKind,
    EnumConversionError, Node, NodeId, NodeKind, Occurrence, OccurrenceKind, ResolutionCertainty,
    TrailCallerScope, TrailConfig, TrailDirection, TrailMode, TrailResult,
};
use parking_lot::RwLock;
use rusqlite::{Connection, MAIN_DB, Result, Row, params, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

mod bookmarks;
mod helpers;
mod row_mapping;
mod schema;
mod trail;

use helpers::{
    decode_embedding_blob, deserialize_candidate_targets, encode_embedding_blob,
    numbered_placeholders, question_placeholders, serialize_candidate_targets,
};

const SCHEMA_VERSION: u32 = 9;
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

fn clamp_i64_to_u32(value: i64) -> u32 {
    if value <= 0 {
        0
    } else if value > u32::MAX as i64 {
        u32::MAX
    } else {
        value as u32
    }
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
            fs::remove_file(&candidate).map_err(|err| {
                StorageError::Other(format!(
                    "Failed to remove SQLite artifact {}: {err}",
                    candidate.display()
                ))
            })?;
        }
    }
    Ok(())
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

fn grounding_node_rank_sql(alias: &str) -> String {
    let display_name = grounding_trimmed_name_expr(alias);
    format!(
        "CASE
            WHEN {alias}.kind IN ({module_kind}, {namespace_kind}, {package_kind}) AND (
                (substr({display_name}, 1, 1) = char(34) AND substr({display_name}, length({display_name}), 1) = char(34))
                OR (substr({display_name}, 1, 1) = char(39) AND substr({display_name}, length({display_name}), 1) = char(39))
                OR (substr({display_name}, 1, 1) = '<' AND substr({display_name}, length({display_name}), 1) = '>')
                OR {display_name} LIKE './%'
                OR {display_name} LIKE '../%'
                OR instr({display_name}, '/') > 0
            ) THEN 5
            WHEN {alias}.kind IN ({class_kind}, {struct_kind}, {interface_kind}, {enum_kind}, {union_kind}, {annotation_kind}, {typedef_kind}) THEN 0
            WHEN {alias}.kind IN ({function_kind}, {method_kind}, {macro_kind}) THEN 1
            WHEN {alias}.kind IN ({module_kind}, {namespace_kind}, {package_kind}) THEN 2
            WHEN {alias}.kind IN ({field_kind}, {variable_kind}, {global_variable_kind}, {constant_kind}, {enum_constant_kind}, {type_parameter_kind}) THEN 3
            ELSE 4
        END",
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

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Invalid enum value: {0}")]
    EnumConversion(#[from] EnumConversionError),
    #[error("Other error: {0}")]
    Other(String),
}

pub struct Storage {
    conn: Connection,
    cache: StorageCache,
    deferred_secondary_indexes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOpenMode {
    Live,
    Build,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub id: i64,
    pub path: PathBuf,
    pub language: String,
    pub modification_time: i64,
    pub indexed: bool,
    pub complete: bool,
    pub line_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub node_count: i64,
    pub edge_count: i64,
    pub file_count: i64,
    pub error_count: i64,
}

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
    pub embedding_model: String,
    pub embedding_dim: u32,
    pub embedding: Vec<f32>,
    pub updated_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmSymbolDocStats {
    pub doc_count: u32,
    pub embedding_model: Option<String>,
}

impl Storage {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        Self::open_with_mode(path, StorageOpenMode::Live)
    }

    pub fn open_build<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path = path.as_ref();
        cleanup_sqlite_sidecars(path)?;
        Self::open_with_mode(path, StorageOpenMode::Build)
    }

    pub fn open_with_mode<P: AsRef<Path>>(
        path: P,
        mode: StorageOpenMode,
    ) -> Result<Self, StorageError> {
        let path = path.as_ref();
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

    /// Expose raw connection for advanced operations (like batch processing).
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

    pub fn update_file_metadata(&self, info: &FileInfo) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE file
             SET path = ?2,
                 language = ?3,
                 modification_time = ?4,
                 indexed = ?5,
                 complete = ?6,
                 line_count = ?7
             WHERE id = ?1",
            params![
                info.id,
                info.path.to_string_lossy(),
                info.language,
                info.modification_time,
                i32::from(info.indexed),
                i32::from(info.complete),
                info.line_count,
            ],
        )?;
        self.mark_grounding_snapshots_dirty()?;
        Ok(())
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
        let backup_path = live_path.with_extension("sqlite.backup");
        let live_exists = live_path.exists();
        cleanup_sqlite_sidecars(&backup_path)?;
        let mut live_conn = Connection::open(live_path)?;
        let _ = live_conn.busy_timeout(Duration::from_millis(2_500));

        if live_exists {
            live_conn.backup(
                MAIN_DB,
                &backup_path,
                None::<fn(rusqlite::backup::Progress)>,
            )?;
        }

        if let Err(err) =
            live_conn.restore(MAIN_DB, staged_path, None::<fn(rusqlite::backup::Progress)>)
        {
            if live_exists && backup_path.exists() {
                let _ = live_conn.restore(
                    MAIN_DB,
                    &backup_path,
                    None::<fn(rusqlite::backup::Progress)>,
                );
            } else {
                let _ = cleanup_sqlite_sidecars(live_path);
            }
            return Err(StorageError::Other(format!(
                "Failed to promote staged snapshot {} -> {}: {err}",
                staged_path.display(),
                live_path.display()
            )));
        }
        drop(live_conn);
        cleanup_sqlite_sidecars(staged_path)?;
        if backup_path.exists() {
            let _ = cleanup_sqlite_sidecars(&backup_path);
        }
        Ok(())
    }

    pub fn discard_staged_snapshot(staged_path: &Path) -> Result<(), StorageError> {
        cleanup_sqlite_sidecars(staged_path)
    }

    fn init(&self, mode: StorageOpenMode) -> Result<(), StorageError> {
        self.create_tables()?;
        self.create_indexes(mode)?;
        if self.schema_version()? == 0 {
            self.set_schema_version(SCHEMA_VERSION)?;
        }
        self.apply_schema_migrations()
    }

    fn create_tables(&self) -> Result<(), StorageError> {
        schema::create_tables(&self.conn)
    }

    fn create_indexes(&self, mode: StorageOpenMode) -> Result<(), StorageError> {
        schema::create_indexes(&self.conn, mode)
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

    pub fn insert_node(&self, node: &Node) -> Result<(), StorageError> {
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
            ],
        )?;
        // Update cache
        self.cache.nodes.write().insert(node.id, node.clone());
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
            for node in nodes
                .iter()
                .filter(|node| node.kind == NodeKind::FILE)
                .chain(nodes.iter().filter(|node| node.kind != NodeKind::FILE))
            {
                Self::insert_node_with_stmt(&mut stmt, node)?;
            }
        }
        tx.commit()?;

        // Update cache
        let mut cache = self.cache.nodes.write();
        for node in nodes {
            cache.insert(node.id, node.clone());
        }

        self.invalidate_grounding_snapshots()?;
        Ok(())
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

    pub fn flush_projection_batch(
        &mut self,
        batch: ProjectionBatch<'_>,
    ) -> Result<ProjectionFlushBreakdown, StorageError> {
        let mut breakdown = ProjectionFlushBreakdown::default();
        if batch.files.is_empty()
            && batch.nodes.is_empty()
            && batch.edges.is_empty()
            && batch.occurrences.is_empty()
            && batch.component_access.is_empty()
            && batch.callable_projection_states.is_empty()
        {
            return Ok(breakdown);
        }

        let tx = self.conn.transaction()?;

        if !batch.files.is_empty() {
            let started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    modification_time=excluded.modification_time,
                    indexed=excluded.indexed,
                    complete=excluded.complete,
                    line_count=excluded.line_count",
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
                ])?;
            }
            breakdown.files_ms = clamp_i64_to_u32(started.elapsed().as_millis() as i64);
        }

        if !batch.nodes.is_empty() {
            let started = std::time::Instant::now();
            let mut stmt = tx.prepare(
                "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(id) DO NOTHING",
            )?;
            for node in batch
                .nodes
                .iter()
                .filter(|node| node.kind == NodeKind::FILE)
                .chain(
                    batch
                        .nodes
                        .iter()
                        .filter(|node| node.kind != NodeKind::FILE),
                )
            {
                Self::insert_node_with_stmt(&mut stmt, node)?;
            }
            breakdown.nodes_ms = clamp_i64_to_u32(started.elapsed().as_millis() as i64);
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
                ])?;
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
                ])?;
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
                ])?;
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
                ])?;
            }
            breakdown.callable_projection_ms =
                clamp_i64_to_u32(started.elapsed().as_millis() as i64);
        }

        tx.commit()?;

        if !batch.nodes.is_empty() {
            let mut cache = self.cache.nodes.write();
            for node in batch.nodes {
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
                    embedding_model,
                    embedding_dim,
                    embedding_blob,
                    updated_at_epoch_ms
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
                 )
                 ON CONFLICT(node_id) DO UPDATE SET
                    file_node_id = excluded.file_node_id,
                    kind = excluded.kind,
                    display_name = excluded.display_name,
                    qualified_name = excluded.qualified_name,
                    file_path = excluded.file_path,
                    start_line = excluded.start_line,
                    doc_text = excluded.doc_text,
                    embedding_model = excluded.embedding_model,
                    embedding_dim = excluded.embedding_dim,
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
                    doc.embedding_model,
                    doc.embedding_dim as i64,
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
                embedding_model,
                embedding_dim,
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
            let embedding_dim: i64 = row.get(9)?;
            let embedding_blob: Vec<u8> = row.get(10)?;
            docs.push(LlmSymbolDoc {
                node_id: NodeId(row.get(0)?),
                file_node_id: row.get::<_, Option<i64>>(1)?.map(NodeId),
                kind: NodeKind::try_from(kind)?,
                display_name: row.get(3)?,
                qualified_name: row.get(4)?,
                file_path: row.get(5)?,
                start_line: row.get(6)?,
                doc_text: row.get(7)?,
                embedding_model: row.get(8)?,
                embedding_dim: embedding_dim.max(0) as u32,
                embedding: decode_embedding_blob(&embedding_blob)?,
                updated_at_epoch_ms: row.get(11)?,
            });
        }

        Ok(docs)
    }

    pub fn get_llm_symbol_doc_stats(&self) -> Result<LlmSymbolDocStats, StorageError> {
        let (doc_count, min_model, max_model) = self.conn.query_row(
            "SELECT COUNT(*), MIN(embedding_model), MAX(embedding_model) FROM llm_symbol_doc",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )?;
        let embedding_model = match (min_model, max_model) {
            (Some(min_model), Some(max_model)) if min_model == max_model => Some(min_model),
            _ => None,
        };

        Ok(LlmSymbolDocStats {
            doc_count: doc_count.max(0).min(u32::MAX as i64) as u32,
            embedding_model,
        })
    }

    pub fn get_all_llm_symbol_docs(&self) -> Result<Vec<LlmSymbolDoc>, StorageError> {
        self.get_llm_symbol_docs_batch_after(None, usize::MAX)
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
                embedding_model,
                embedding_dim,
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
            let embedding_dim: i64 = row.get(9)?;
            let embedding_blob: Vec<u8> = row.get(10)?;
            docs.push(LlmSymbolDoc {
                node_id: NodeId(row.get(0)?),
                file_node_id: row.get::<_, Option<i64>>(1)?.map(NodeId),
                kind: NodeKind::try_from(kind)?,
                display_name: row.get(3)?,
                qualified_name: row.get(4)?,
                file_path: row.get(5)?,
                start_line: row.get(6)?,
                doc_text: row.get(7)?,
                embedding_model: row.get(8)?,
                embedding_dim: embedding_dim.max(0) as u32,
                embedding: decode_embedding_blob(&embedding_blob)?,
                updated_at_epoch_ms: row.get(11)?,
            });
        }
        Ok(docs)
    }

    pub fn clear_llm_symbol_docs(&mut self) -> Result<usize, StorageError> {
        let removed = self.conn.execute("DELETE FROM llm_symbol_doc", [])?;
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
            "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) 
             ON CONFLICT(id) DO UPDATE SET 
                path=excluded.path, 
                language=excluded.language, 
                modification_time=excluded.modification_time, 
                indexed=excluded.indexed, 
                complete=excluded.complete, 
                line_count=excluded.line_count",
            params![
                info.id,
                info.path.to_string_lossy(),
                info.language,
                info.modification_time,
                i32::from(info.indexed),
                i32::from(info.complete),
                info.line_count,
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
                "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count) 
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) 
                 ON CONFLICT(id) DO UPDATE SET 
                    path=excluded.path, 
                    language=excluded.language, 
                    modification_time=excluded.modification_time, 
                    indexed=excluded.indexed, 
                    complete=excluded.complete, 
                    line_count=excluded.line_count"
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
                ])?;
            }
        }
        tx.commit()?;
        self.invalidate_grounding_snapshots()?;
        Ok(())
    }

    pub fn get_files(&self) -> Result<Vec<FileInfo>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, language, modification_time, indexed, complete, line_count FROM file",
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
                "SELECT id, path, language, modification_time, indexed, complete, line_count
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
                };
                files.insert(file.path.clone(), file);
            }
        }
        Ok(files)
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
            "SELECT id, path, language, modification_time, indexed, complete, line_count FROM file WHERE path = ?1",
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

    pub fn get_stats(&self) -> Result<StorageStats, StorageError> {
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
        })
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

        tx.execute(
            &format!(
                "UPDATE edge
                 SET resolved_source_node_id = NULL
                 WHERE resolved_source_node_id IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 AND source_node_id NOT IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 AND target_node_id NOT IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 AND (file_node_id IS NULL OR file_node_id != ?1)"
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
                 AND source_node_id NOT IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 AND target_node_id NOT IN (SELECT node_id FROM {RELATED_NODE_IDS_TABLE})
                 AND (file_node_id IS NULL OR file_node_id != ?1)"
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
            "DELETE FROM callable_projection_state WHERE file_id = ?1",
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
        || normalized.contains("/tests/")
        || normalized.contains("/test/")
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

#[cfg(test)]
mod tests;
