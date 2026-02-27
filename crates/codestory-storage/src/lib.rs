use codestory_core::{
    AccessKind, Bookmark, BookmarkCategory, Edge, EdgeKind, EnumConversionError, Node, NodeId,
    NodeKind, Occurrence, OccurrenceKind, ResolutionCertainty, TrailCallerScope, TrailConfig,
    TrailDirection, TrailMode, TrailResult,
};
use parking_lot::RwLock;
use rusqlite::{Connection, Result, Row, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

mod bookmarks;
mod row_mapping;
mod schema;
mod trail;

const SCHEMA_VERSION: u32 = 2;
const RELATED_NODE_SUBQUERY: &str = "SELECT id FROM node WHERE id = ?1 OR file_node_id = ?1";
const EDGE_SELECT_BASE: &str = "SELECT e.id, e.source_node_id, e.target_node_id, e.kind, e.file_node_id, e.line, e.resolved_source_node_id, e.resolved_target_node_id, e.confidence, e.callsite_identity, e.certainty, e.candidate_target_node_ids, t.serialized_name, f.serialized_name
                 FROM edge e
                 JOIN node t ON t.id = e.target_node_id
                 LEFT JOIN node f ON f.id = e.file_node_id";

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
}

#[derive(Default)]
struct StorageCache {
    nodes: Arc<RwLock<HashMap<codestory_core::NodeId, codestory_core::Node>>>,
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
}

impl Storage {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        // Allow concurrent reads while indexing writes, and avoid flaky "database is locked" errors
        // in app shells when users query mid-index.
        let _ = conn.busy_timeout(Duration::from_millis(2_500));
        let _ = conn.pragma_update(None, "foreign_keys", "ON");
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let storage = Self {
            conn,
            cache: StorageCache::default(),
        };
        storage.init()?;
        Ok(storage)
    }

    pub fn new_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let _ = conn.pragma_update(None, "foreign_keys", "ON");
        let storage = Self {
            conn,
            cache: StorageCache::default(),
        };
        storage.init()?;
        Ok(storage)
    }

    /// Expose raw connection for advanced operations (like batch processing).
    pub fn get_connection(&self) -> &Connection {
        &self.conn
    }

    pub fn clear(&self) -> Result<(), StorageError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM occurrence", [])?;
        tx.execute("DELETE FROM edge", [])?;
        tx.execute("DELETE FROM component_access", [])?;
        tx.execute("DELETE FROM bookmark_node", [])?;
        tx.execute("DELETE FROM local_symbol", [])?;
        tx.execute("DELETE FROM error", [])?;
        tx.execute("DELETE FROM node", [])?;
        tx.execute("DELETE FROM file", [])?;
        tx.commit()?;

        self.cache.nodes.write().clear();
        Ok(())
    }

    fn init(&self) -> Result<(), StorageError> {
        self.create_tables()?;
        self.create_indexes()?;
        self.apply_schema_migrations()
    }

    fn create_tables(&self) -> Result<(), StorageError> {
        schema::create_tables(&self.conn)
    }

    fn create_indexes(&self) -> Result<(), StorageError> {
        schema::create_indexes(&self.conn)
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

    fn node_from_row(row: &Row) -> Result<Node, StorageError> {
        row_mapping::node_from_row(row)
    }

    fn edge_from_row(row: &Row) -> Result<Edge, StorageError> {
        row_mapping::edge_from_row(row)
    }

    fn occurrence_from_row(row: &Row) -> rusqlite::Result<Occurrence> {
        row_mapping::occurrence_from_row(row)
    }

    fn certainty_db_value(certainty: Option<ResolutionCertainty>) -> Option<&'static str> {
        row_mapping::certainty_db_value(certainty)
    }

    fn access_kind_db_value(access: AccessKind) -> i32 {
        row_mapping::access_kind_db_value(access)
    }

    fn access_kind_from_db(value: i32) -> AccessKind {
        row_mapping::access_kind_from_db(value)
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(id) DO NOTHING",
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
                Self::certainty_db_value(edge.certainty),
                serialize_candidate_targets(&edge.candidate_targets)?
            ],
        )?;
        Ok(())
    }

    // Batch operations
    pub fn insert_nodes_batch(&mut self, nodes: &[Node]) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(id) DO NOTHING",
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
                    Self::certainty_db_value(edge.certainty),
                    serialize_candidate_targets(&edge.candidate_targets)?
                ])?;
            }
        }
        tx.commit()?;
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
                stmt.execute(params![node_id.0, Self::access_kind_db_value(*access)])?;
            }
        }
        tx.commit()?;
        Ok(())
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
            return Ok(Some(Self::access_kind_from_db(raw)));
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

        let placeholders = std::iter::repeat_n("?", node_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql =
            format!("SELECT node_id, type FROM component_access WHERE node_id IN ({placeholders})");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(node_ids.iter().map(|id| id.0)))?;
        let mut map = HashMap::new();
        while let Some(row) = rows.next()? {
            let node_id: i64 = row.get(0)?;
            let raw: i32 = row.get(1)?;
            map.insert(NodeId(node_id), Self::access_kind_from_db(raw));
        }
        Ok(map)
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
        node_id: codestory_core::NodeId,
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
        file_node_id: codestory_core::NodeId,
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

    pub fn insert_error(&self, error: &codestory_core::ErrorInfo) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO error (message, file_id, line, column, fatal, indexed) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                error.message,
                error.file_id.map(|id| id.0),
                error.line,
                error.column,
                error.is_fatal as i32,
                (error.index_step == codestory_core::IndexStep::Indexing) as i32,
            ],
        )?;
        Ok(())
    }

    /// Get symbols that have no parent (root namespaces, top-level classes, etc.)
    pub fn get_root_symbols(&self) -> Result<Vec<Node>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line, start_col, end_line, end_col FROM node 
             WHERE id NOT IN (SELECT target_node_id FROM edge WHERE kind = ?1)
             AND kind != ?2", // Exclude files from symbol tree roots for now
        )?;
        let kind_member = codestory_core::EdgeKind::MEMBER as i32;
        let kind_file = codestory_core::NodeKind::FILE as i32;

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
        let kind_member = codestory_core::EdgeKind::MEMBER as i32;

        let mut nodes = Vec::new();
        let mut rows = stmt.query(params![parent_id.0, kind_member])?;
        while let Some(row) = rows.next()? {
            nodes.push(Self::node_from_row(row)?);
        }
        Ok(nodes)
    }

    pub fn get_stats(&self) -> Result<StorageStats, StorageError> {
        let node_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM node", [], |r| r.get(0))?;
        let edge_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM edge", [], |r| r.get(0))?;
        let file_count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM file", [], |r| r.get(0))?;
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

        let node_ids_query = format!("SELECT DISTINCT id FROM ({RELATED_NODE_SUBQUERY})");
        let mut related_node_ids = Vec::new();
        {
            let mut node_ids_stmt = tx.prepare(&node_ids_query)?;
            let mut node_rows = node_ids_stmt.query(params![file_node_id])?;
            while let Some(row) = node_rows.next()? {
                related_node_ids.push(row.get::<_, i64>(0)?);
            }
        }

        tx.execute(
            &format!(
                "UPDATE edge
                 SET resolved_source_node_id = NULL
                 WHERE resolved_source_node_id IN ({RELATED_NODE_SUBQUERY})
                 AND source_node_id NOT IN ({RELATED_NODE_SUBQUERY})
                 AND target_node_id NOT IN ({RELATED_NODE_SUBQUERY})
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
                 WHERE resolved_target_node_id IN ({RELATED_NODE_SUBQUERY})
                 AND source_node_id NOT IN ({RELATED_NODE_SUBQUERY})
                 AND target_node_id NOT IN ({RELATED_NODE_SUBQUERY})
                 AND (file_node_id IS NULL OR file_node_id != ?1)"
            ),
            params![file_node_id],
        )?;

        let removed_edges = tx.execute(
            &format!(
                "DELETE FROM edge
                 WHERE source_node_id IN ({RELATED_NODE_SUBQUERY})
                 OR target_node_id IN ({RELATED_NODE_SUBQUERY})
                 OR file_node_id = ?1"
            ),
            params![file_node_id],
        )?;

        let removed_occurrences = tx.execute(
            &format!(
                "DELETE FROM occurrence
                 WHERE file_node_id = ?1
                 OR element_id IN ({RELATED_NODE_SUBQUERY})"
            ),
            params![file_node_id],
        )?;

        let removed_bookmarks = tx.execute(
            &format!("DELETE FROM bookmark_node WHERE node_id IN ({RELATED_NODE_SUBQUERY})"),
            params![file_node_id],
        )?;

        let removed_component_access = tx.execute(
            &format!("DELETE FROM component_access WHERE node_id IN ({RELATED_NODE_SUBQUERY})"),
            params![file_node_id],
        )?;

        let removed_local_symbols = tx.execute(
            "DELETE FROM local_symbol WHERE file_id = ?1",
            params![file_node_id],
        )?;

        // Remove any node references in other projection tables.
        let removed_nodes = tx.execute(
            "DELETE FROM node WHERE id = ?1 OR file_node_id = ?1",
            params![file_node_id],
        )?;

        let removed_errors = tx.execute(
            "DELETE FROM error WHERE file_id = ?1",
            params![file_node_id],
        )?;

        let removed_file_rows =
            tx.execute("DELETE FROM file WHERE id = ?1", params![file_node_id])?;

        tx.commit()?;

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
        filter: Option<&codestory_core::ErrorFilter>,
    ) -> Result<Vec<codestory_core::ErrorInfo>, StorageError> {
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
            errors.push(codestory_core::ErrorInfo {
                message: row.get(1)?,
                file_id: row.get::<_, Option<i64>>(2)?.map(NodeId),
                line: row.get(3)?,
                column: row.get(4)?,
                is_fatal: fatal != 0,
                index_step: if indexed != 0 {
                    codestory_core::IndexStep::Indexing
                } else {
                    codestory_core::IndexStep::Collection
                },
            });
        }
        Ok(errors)
    }

    /// Clear all errors
    pub fn clear_errors(&self) -> Result<(), StorageError> {
        self.conn.execute("DELETE FROM error", [])?;
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

fn serialize_candidate_targets(candidates: &[NodeId]) -> Result<Option<String>, StorageError> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let raw: Vec<i64> = candidates.iter().map(|id| id.0).collect();
    Ok(Some(serde_json::to_string(&raw).map_err(|e| {
        StorageError::Other(format!("failed to serialize edge candidates: {e}"))
    })?))
}

fn deserialize_candidate_targets(payload: Option<&str>) -> Result<Vec<NodeId>, StorageError> {
    let Some(payload) = payload else {
        return Ok(Vec::new());
    };
    if payload.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Vec<i64> = serde_json::from_str(payload)
        .map_err(|e| StorageError::Other(format!("failed to parse edge candidate payload: {e}")))?;
    Ok(parsed.into_iter().map(NodeId).collect())
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
mod tests;
