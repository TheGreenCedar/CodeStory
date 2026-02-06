use codestory_core::{
    Bookmark, BookmarkCategory, Edge, EdgeKind, EnumConversionError, Node, NodeId, NodeKind,
    Occurrence, OccurrenceKind, TrailConfig, TrailDirection, TrailMode, TrailResult,
};
use parking_lot::RwLock;
use rusqlite::{Connection, Result, Row, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

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

impl Storage {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        let storage = Self {
            conn,
            cache: StorageCache::default(),
        };
        storage.init()?;
        Ok(storage)
    }

    pub fn new_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
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
        self.conn.execute("DELETE FROM occurrence", [])?;
        self.conn.execute("DELETE FROM edge", [])?;
        self.conn.execute("DELETE FROM node", [])?;
        Ok(())
    }

    fn init(&self) -> Result<(), StorageError> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                qualified_name TEXT,
                canonical_id TEXT,
                file_node_id INTEGER,
                start_line INTEGER,
                start_col INTEGER,
                end_line INTEGER,
                end_col INTEGER,
                FOREIGN KEY(file_node_id) REFERENCES node(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS edge (
                id INTEGER PRIMARY KEY,
                source_node_id INTEGER NOT NULL,
                target_node_id INTEGER NOT NULL,
                kind INTEGER NOT NULL,
                file_node_id INTEGER,
                line INTEGER,
                resolved_source_node_id INTEGER,
                resolved_target_node_id INTEGER,
                confidence REAL,
                FOREIGN KEY(source_node_id) REFERENCES node(id),
                FOREIGN KEY(target_node_id) REFERENCES node(id),
                FOREIGN KEY(file_node_id) REFERENCES node(id),
                FOREIGN KEY(resolved_source_node_id) REFERENCES node(id),
                FOREIGN KEY(resolved_target_node_id) REFERENCES node(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS occurrence (
                 element_id INTEGER NOT NULL,
                 kind INTEGER NOT NULL,
                 file_node_id INTEGER NOT NULL,
                 start_line INTEGER NOT NULL,
                 start_col INTEGER NOT NULL,
                 end_line INTEGER NOT NULL,
                 end_col INTEGER NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_occurrence_unique \
             ON occurrence(element_id, file_node_id, start_line, start_col, end_line, end_col)",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS file (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                language TEXT,
                modification_time INTEGER,
                indexed INTEGER DEFAULT 0,
                complete INTEGER DEFAULT 0,
                line_count INTEGER DEFAULT 0
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS local_symbol (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                file_id INTEGER,
                FOREIGN KEY(file_id) REFERENCES file(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS component_access (
                node_id INTEGER,
                type INTEGER,
                FOREIGN KEY(node_id) REFERENCES node(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS error (
                id INTEGER PRIMARY KEY,
                message TEXT NOT NULL,
                file_id INTEGER,
                line INTEGER,
                column INTEGER,
                fatal INTEGER DEFAULT 0,
                indexed INTEGER DEFAULT 0,
                FOREIGN KEY(file_id) REFERENCES file(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS bookmark_category (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS bookmark_node (
                id INTEGER PRIMARY KEY,
                category_id INTEGER,
                node_id INTEGER,
                comment TEXT,
                FOREIGN KEY(category_id) REFERENCES bookmark_category(id),
                FOREIGN KEY(node_id) REFERENCES node(id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_occurrence_element ON occurrence(element_id)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_occurrence_file ON occurrence(file_node_id)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_edge_source ON edge(source_node_id)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_edge_target ON edge(target_node_id)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_node_file ON node(file_node_id)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_node_qualified_name ON node(qualified_name)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_edge_resolved_source ON edge(resolved_source_node_id)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_edge_resolved_target ON edge(resolved_target_node_id)",
            [],
        )?;

        self.conn
            .execute("CREATE INDEX IF NOT EXISTS idx_edge_line ON edge(line)", [])?;

        Ok(())
    }

    fn node_from_row(row: &Row) -> Result<Node, StorageError> {
        let kind_int: i32 = row.get(1)?;
        Ok(Node {
            id: NodeId(row.get(0)?),
            kind: NodeKind::try_from(kind_int)?,
            serialized_name: row.get(2)?,
            qualified_name: row.get(3)?,
            canonical_id: row.get(4)?,
            file_node_id: row.get::<_, Option<i64>>(5)?.map(NodeId),
            start_line: row.get(6)?,
            start_col: row.get(7)?,
            end_line: row.get(8)?,
            end_col: row.get(9)?,
        })
    }

    fn edge_from_row(row: &Row) -> Result<Edge, StorageError> {
        let kind_int: i32 = row.get(3)?;
        Ok(Edge {
            id: codestory_core::EdgeId(row.get(0)?),
            source: NodeId(row.get(1)?),
            target: NodeId(row.get(2)?),
            kind: EdgeKind::try_from(kind_int)?,
            file_node_id: row.get::<_, Option<i64>>(4)?.map(NodeId),
            line: row.get(5)?,
            resolved_source: row.get::<_, Option<i64>>(6)?.map(NodeId),
            resolved_target: row.get::<_, Option<i64>>(7)?.map(NodeId),
            confidence: row.get(8)?,
        })
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
            "INSERT INTO edge (id, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(id) DO NOTHING",
            params![
                edge.id.0,
                edge.source.0,
                edge.target.0,
                edge.kind as i32,
                edge.file_node_id.map(|id| id.0),
                edge.line,
                edge.resolved_source.map(|id| id.0),
                edge.resolved_target.map(|id| id.0),
                edge.confidence
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
            for node in nodes.iter().filter(|node| node.kind == NodeKind::FILE) {
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
                ])?;
            }
            for node in nodes.iter().filter(|node| node.kind != NodeKind::FILE) {
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
                ])?;
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
                "INSERT INTO edge (id, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(id) DO NOTHING"
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
                    edge.confidence
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
            .prepare("SELECT id, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence FROM edge")?;
        let mut edges = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            edges.push(Self::edge_from_row(row)?);
        }
        Ok(edges)
    }

    pub fn get_occurrences(&self) -> Result<Vec<Occurrence>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT element_id, kind, file_node_id, start_line, start_col, end_line, end_col FROM occurrence"
        )?;
        let occ_iter = stmt.query_map([], |row| {
            let kind_int: i32 = row.get(1)?;
            Ok(Occurrence {
                element_id: row.get(0)?,
                kind: OccurrenceKind::try_from(kind_int).unwrap_or(OccurrenceKind::UNKNOWN),
                location: codestory_core::SourceLocation {
                    file_node_id: codestory_core::NodeId(row.get(2)?),
                    start_line: row.get(3)?,
                    start_col: row.get(4)?,
                    end_line: row.get(5)?,
                    end_col: row.get(6)?,
                },
            })
        })?;

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
        let occ_iter = stmt.query_map([element_id], |row| {
            let kind_int: i32 = row.get(1)?;
            Ok(Occurrence {
                element_id: row.get(0)?,
                kind: OccurrenceKind::try_from(kind_int).unwrap_or(OccurrenceKind::UNKNOWN),
                location: codestory_core::SourceLocation {
                    file_node_id: codestory_core::NodeId(row.get(2)?),
                    start_line: row.get(3)?,
                    start_col: row.get(4)?,
                    end_line: row.get(5)?,
                    end_col: row.get(6)?,
                },
            })
        })?;

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

        let edges = self.get_edges_for_node(center_id, &TrailDirection::Both, &[])?;

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
        let occ_iter = stmt.query_map(params![node_id.0], |row| {
            let kind_int: i32 = row.get(1)?;
            Ok(Occurrence {
                element_id: row.get(0)?,
                kind: OccurrenceKind::try_from(kind_int).unwrap_or(OccurrenceKind::UNKNOWN),
                location: codestory_core::SourceLocation {
                    file_node_id: codestory_core::NodeId(row.get(2)?),
                    start_line: row.get(3)?,
                    start_col: row.get(4)?,
                    end_line: row.get(5)?,
                    end_col: row.get(6)?,
                },
            })
        })?;

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
        let occ_iter = stmt.query_map(params![file_node_id.0], |row| {
            let kind_int: i32 = row.get(1)?;
            Ok(Occurrence {
                element_id: row.get(0)?,
                kind: OccurrenceKind::try_from(kind_int).unwrap_or(OccurrenceKind::UNKNOWN),
                location: codestory_core::SourceLocation {
                    file_node_id: codestory_core::NodeId(row.get(2)?),
                    start_line: row.get(3)?,
                    start_col: row.get(4)?,
                    end_line: row.get(5)?,
                    end_col: row.get(6)?,
                },
            })
        })?;

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
                info.indexed as i32,
                info.complete as i32,
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

    /// Delete a file and all associated data (nodes, edges, occurrences, errors)
    pub fn delete_file(&mut self, file_id: i64) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;

        // Delete occurrences for this file
        tx.execute(
            "DELETE FROM occurrence WHERE file_node_id = ?1",
            params![file_id],
        )?;

        // Delete edges where source or target is a node from this file
        // (This is approximate - ideally we'd track which nodes belong to which file)
        tx.execute(
            "DELETE FROM edge WHERE source_node_id = ?1 OR target_node_id = ?1",
            params![file_id],
        )?;

        // Delete the file node itself
        tx.execute("DELETE FROM node WHERE id = ?1", params![file_id])?;

        // Delete errors for this file
        tx.execute("DELETE FROM error WHERE file_id = ?1", params![file_id])?;

        // Delete file record
        tx.execute("DELETE FROM file WHERE id = ?1", params![file_id])?;

        tx.commit()?;

        // Clear from cache
        self.cache
            .nodes
            .write()
            .remove(&codestory_core::NodeId(file_id));

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
        self.conn.execute(
            "INSERT INTO bookmark_category (name) VALUES (?1)",
            params![name],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Get all bookmark categories
    pub fn get_bookmark_categories(&self) -> Result<Vec<BookmarkCategory>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM bookmark_category")?;
        let mut categories = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            categories.push(BookmarkCategory {
                id: row.get(0)?,
                name: row.get(1)?,
            });
        }
        Ok(categories)
    }

    /// Delete a bookmark category and all its bookmarks
    pub fn delete_bookmark_category(&self, id: i64) -> Result<(), StorageError> {
        self.conn.execute(
            "DELETE FROM bookmark_node WHERE category_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM bookmark_category WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Rename a bookmark category
    pub fn rename_bookmark_category(&self, id: i64, new_name: &str) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE bookmark_category SET name = ?1 WHERE id = ?2",
            params![new_name, id],
        )?;
        Ok(())
    }

    /// Add a bookmark to a category
    pub fn add_bookmark(
        &self,
        category_id: i64,
        node_id: NodeId,
        comment: Option<&str>,
    ) -> Result<i64, StorageError> {
        self.conn.execute(
            "INSERT INTO bookmark_node (category_id, node_id, comment) VALUES (?1, ?2, ?3)",
            params![category_id, node_id.0, comment],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Get bookmarks, optionally filtered by category
    pub fn get_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<Bookmark>, StorageError> {
        let query = match category_id {
            Some(_) => {
                "SELECT id, category_id, node_id, comment FROM bookmark_node WHERE category_id = ?1"
            }
            None => "SELECT id, category_id, node_id, comment FROM bookmark_node",
        };
        let mut stmt = self.conn.prepare(query)?;
        let mut bookmarks = Vec::new();

        let mut rows = if let Some(cat_id) = category_id {
            stmt.query(params![cat_id])?
        } else {
            stmt.query([])?
        };

        while let Some(row) = rows.next()? {
            bookmarks.push(Bookmark {
                id: row.get(0)?,
                category_id: row.get(1)?,
                node_id: NodeId(row.get(2)?),
                comment: row.get(3)?,
            });
        }
        Ok(bookmarks)
    }

    /// Update a bookmark's comment
    pub fn update_bookmark_comment(&self, id: i64, comment: &str) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE bookmark_node SET comment = ?1 WHERE id = ?2",
            params![comment, id],
        )?;
        Ok(())
    }

    /// Delete a bookmark
    pub fn delete_bookmark(&self, id: i64) -> Result<(), StorageError> {
        self.conn
            .execute("DELETE FROM bookmark_node WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ========================================================================
    // Trail Query (BFS-based subgraph exploration)
    // ========================================================================

    /// Get a trail (subgraph) starting from a root node up to a certain depth
    pub fn get_trail(&self, config: &TrailConfig) -> Result<TrailResult, StorageError> {
        match config.mode {
            TrailMode::ToTargetSymbol => self.get_trail_to_target(config),
            _ => self.get_trail_bfs(config),
        }
    }

    fn get_trail_bfs(&self, config: &TrailConfig) -> Result<TrailResult, StorageError> {
        let mut result = TrailResult::default();
        let mut visited: HashSet<NodeId> = HashSet::new();
        let mut queue: VecDeque<(NodeId, u32)> = VecDeque::new();
        let max_depth = if config.depth == 0 {
            // `0` means "infinite depth", bounded by `max_nodes` and the visited set.
            u32::MAX
        } else {
            config.depth
        };

        // Some modes force an effective direction regardless of the caller-provided direction.
        let direction = match config.mode {
            TrailMode::AllReferenced => TrailDirection::Outgoing,
            TrailMode::AllReferencing => TrailDirection::Incoming,
            _ => config.direction,
        };

        // Start with root
        queue.push_back((config.root_id, 0));
        visited.insert(config.root_id);
        result.depth_map.insert(config.root_id, 0);

        // BFS traversal
        while let Some((current_id, depth)) = queue.pop_front() {
            if result.nodes.len() >= config.max_nodes {
                result.truncated = true;
                break;
            }

            // Fetch the current node
            if let Some(node) = self.get_node(current_id)? {
                result.nodes.push(node);
            }

            // If we haven't reached max depth, explore neighbors
            if depth < max_depth {
                let edges = self.get_edges_for_node(current_id, &direction, &config.edge_filter)?;

                for edge in edges {
                    result.edges.push(edge.clone());

                    let (eff_source, eff_target) = edge.effective_endpoints();
                    let neighbor_id = if eff_source == current_id {
                        eff_target
                    } else if eff_target == current_id {
                        eff_source
                    } else if edge.source == current_id {
                        edge.target
                    } else {
                        edge.source
                    };

                    if !visited.contains(&neighbor_id) {
                        visited.insert(neighbor_id);
                        result.depth_map.insert(neighbor_id, depth + 1);
                        queue.push_back((neighbor_id, depth + 1));
                    }
                }
            }
        }

        apply_trail_node_filter(&mut result, config);
        Ok(result)
    }

    fn get_trail_to_target(&self, config: &TrailConfig) -> Result<TrailResult, StorageError> {
        let target_id = config.target_id.ok_or_else(|| {
            StorageError::Other(
                "TrailMode::ToTargetSymbol requires TrailConfig.target_id".to_string(),
            )
        })?;

        let max_depth = if config.depth == 0 {
            // `0` means "infinite depth", bounded by node caps.
            u32::MAX
        } else {
            config.depth
        };

        // Allow some slack for distance discovery (so we can still return a meaningful path)
        // while keeping the query bounded.
        let bfs_cap = config
            .max_nodes
            .saturating_mul(4)
            .max(config.max_nodes)
            .min(100_000);

        let (dist_from_root, truncated_from_root) = self.bfs_distances(
            config.root_id,
            TrailDirection::Outgoing,
            &config.edge_filter,
            max_depth,
            bfs_cap,
        )?;
        let (dist_to_target, truncated_to_target) = self.bfs_distances(
            target_id,
            TrailDirection::Incoming,
            &config.edge_filter,
            max_depth,
            bfs_cap,
        )?;

        // If no path exists (within the discovered bounds), show the endpoints.
        let target_reachable = dist_from_root.contains_key(&target_id);
        if !target_reachable {
            let mut result = TrailResult::default();
            if let Some(node) = self.get_node(config.root_id)? {
                result.nodes.push(node);
                result.depth_map.insert(config.root_id, 0);
            }
            if target_id != config.root_id {
                if let Some(node) = self.get_node(target_id)? {
                    result.nodes.push(node);
                }
            }
            result.truncated = truncated_from_root || truncated_to_target;
            apply_trail_node_filter(&mut result, config);
            return Ok(result);
        }

        let mut included: HashSet<NodeId> = HashSet::new();
        for (id, d_root) in &dist_from_root {
            if let Some(d_to) = dist_to_target.get(id) {
                if max_depth == u32::MAX || (*d_root as u64 + *d_to as u64) <= max_depth as u64 {
                    included.insert(*id);
                }
            }
        }
        included.insert(config.root_id);
        included.insert(target_id);

        // Extract one shortest (or near-shortest) path to prioritize in truncation.
        let mut path_nodes: Vec<NodeId> = Vec::new();
        path_nodes.push(config.root_id);
        let mut current = config.root_id;
        while current != target_id {
            let Some(&d_cur) = dist_from_root.get(&current) else {
                break;
            };
            let edges =
                self.get_edges_for_node(current, &TrailDirection::Outgoing, &config.edge_filter)?;
            let mut best_next: Option<NodeId> = None;
            let mut best_key: Option<(u32, i64)> = None; // (dist_to_target, node_id)
            for edge in edges {
                let (src, dst) = edge.effective_endpoints();
                if src != current {
                    continue;
                }
                let next = dst;
                let Some(&d_next) = dist_from_root.get(&next) else {
                    continue;
                };
                if d_next != d_cur.saturating_add(1) {
                    continue;
                }
                let Some(&d_to) = dist_to_target.get(&next) else {
                    continue;
                };
                if max_depth != u32::MAX {
                    let path_len = d_cur as u64 + 1 + d_to as u64;
                    if path_len > max_depth as u64 {
                        continue;
                    }
                }
                if !included.contains(&next) {
                    continue;
                }
                let key = (d_to, next.0);
                if best_key.map_or(true, |k| key < k) {
                    best_key = Some(key);
                    best_next = Some(next);
                }
            }

            let Some(next) = best_next else {
                break;
            };
            path_nodes.push(next);
            current = next;
        }

        // Final node selection (bounded by max_nodes), always keeping the extracted path.
        fn push_unique(selected: &mut Vec<NodeId>, selected_set: &mut HashSet<NodeId>, id: NodeId) {
            if selected_set.insert(id) {
                selected.push(id);
            }
        }

        let mut selected: Vec<NodeId> = Vec::new();
        let mut selected_set: HashSet<NodeId> = HashSet::new();
        for id in &path_nodes {
            push_unique(&mut selected, &mut selected_set, *id);
        }
        push_unique(&mut selected, &mut selected_set, target_id);

        let mut other: Vec<NodeId> = included.iter().copied().collect();
        other.sort_by(|a, b| {
            let da = dist_from_root.get(a).copied().unwrap_or(u32::MAX);
            let db = dist_from_root.get(b).copied().unwrap_or(u32::MAX);
            let ta = dist_to_target.get(a).copied().unwrap_or(u32::MAX);
            let tb = dist_to_target.get(b).copied().unwrap_or(u32::MAX);
            (da.saturating_add(ta), da, a.0).cmp(&(db.saturating_add(tb), db, b.0))
        });
        for id in other {
            if selected.len() >= config.max_nodes {
                break;
            }
            push_unique(&mut selected, &mut selected_set, id);
        }

        let mut result = TrailResult::default();
        result.truncated = truncated_from_root
            || truncated_to_target
            || included.len() > config.max_nodes
            || selected.len() < included.len();

        // Populate nodes in stable order.
        selected.sort_by(|a, b| {
            let da = dist_from_root.get(a).copied().unwrap_or(u32::MAX);
            let db = dist_from_root.get(b).copied().unwrap_or(u32::MAX);
            (da, a.0).cmp(&(db, b.0))
        });
        for id in &selected {
            if let Some(node) = self.get_node(*id)? {
                result.nodes.push(node);
            }
            let depth = dist_from_root.get(id).copied().unwrap_or(0);
            result.depth_map.insert(*id, depth);
        }

        // Populate edges that are on at least one root->target path within max_depth.
        let selected_set: HashSet<NodeId> = selected.iter().copied().collect();
        let mut edge_ids: HashSet<codestory_core::EdgeId> = HashSet::new();
        for id in &selected {
            let Some(&d_root) = dist_from_root.get(id) else {
                continue;
            };
            let edges =
                self.get_edges_for_node(*id, &TrailDirection::Outgoing, &config.edge_filter)?;
            for edge in edges {
                let (src, dst) = edge.effective_endpoints();
                if src != *id {
                    continue;
                }
                if !selected_set.contains(&dst) {
                    continue;
                }
                let Some(&d_to) = dist_to_target.get(&dst) else {
                    continue;
                };
                if max_depth != u32::MAX {
                    let len = d_root as u64 + 1 + d_to as u64;
                    if len > max_depth as u64 {
                        continue;
                    }
                }
                if edge_ids.insert(edge.id) {
                    result.edges.push(edge);
                }
            }
        }
        result.edges.sort_by_key(|e| e.id.0);

        apply_trail_node_filter(&mut result, config);
        Ok(result)
    }

    fn bfs_distances(
        &self,
        start: NodeId,
        direction: TrailDirection,
        edge_filter: &[EdgeKind],
        max_depth: u32,
        max_nodes: usize,
    ) -> Result<(HashMap<NodeId, u32>, bool), StorageError> {
        let mut dist: HashMap<NodeId, u32> = HashMap::new();
        let mut queue: VecDeque<(NodeId, u32)> = VecDeque::new();
        let mut truncated = false;

        dist.insert(start, 0);
        queue.push_back((start, 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if dist.len() >= max_nodes {
                truncated = true;
                break;
            }
            if depth >= max_depth {
                continue;
            }

            let edges = self.get_edges_for_node(current_id, &direction, edge_filter)?;
            for edge in edges {
                let (eff_source, eff_target) = edge.effective_endpoints();
                let neighbor_id = match direction {
                    TrailDirection::Outgoing => {
                        if eff_source == current_id {
                            eff_target
                        } else {
                            continue;
                        }
                    }
                    TrailDirection::Incoming => {
                        if eff_target == current_id {
                            eff_source
                        } else {
                            continue;
                        }
                    }
                    TrailDirection::Both => {
                        // Not used by ToTargetSymbol distance discovery.
                        let other = if eff_source == current_id {
                            eff_target
                        } else if eff_target == current_id {
                            eff_source
                        } else {
                            continue;
                        };
                        other
                    }
                };

                if !dist.contains_key(&neighbor_id) {
                    let next_depth = depth.saturating_add(1);
                    dist.insert(neighbor_id, next_depth);
                    queue.push_back((neighbor_id, next_depth));
                }
            }
        }

        Ok((dist, truncated))
    }

    /// Helper: Get edges for a node in a specific direction
    fn get_edges_for_node(
        &self,
        node_id: NodeId,
        direction: &TrailDirection,
        edge_filter: &[EdgeKind],
    ) -> Result<Vec<Edge>, StorageError> {
        let query = match direction {
            TrailDirection::Outgoing => {
                "SELECT e.id, e.source_node_id, e.target_node_id, e.kind, e.file_node_id, e.line, e.resolved_source_node_id, e.resolved_target_node_id, e.confidence, t.serialized_name
                 FROM edge e
                 JOIN node t ON t.id = e.target_node_id
                 WHERE e.source_node_id = ?1 OR e.resolved_source_node_id = ?1"
            }
            TrailDirection::Incoming => {
                "SELECT e.id, e.source_node_id, e.target_node_id, e.kind, e.file_node_id, e.line, e.resolved_source_node_id, e.resolved_target_node_id, e.confidence, t.serialized_name
                 FROM edge e
                 JOIN node t ON t.id = e.target_node_id
                 WHERE e.target_node_id = ?1 OR e.resolved_target_node_id = ?1"
            }
            TrailDirection::Both => {
                "SELECT e.id, e.source_node_id, e.target_node_id, e.kind, e.file_node_id, e.line, e.resolved_source_node_id, e.resolved_target_node_id, e.confidence, t.serialized_name
                 FROM edge e
                 JOIN node t ON t.id = e.target_node_id
                 WHERE e.source_node_id = ?1 OR e.target_node_id = ?1 OR e.resolved_source_node_id = ?1 OR e.resolved_target_node_id = ?1"
            }
        };

        let mut stmt = self.conn.prepare(query)?;
        let mut edges = Vec::new();
        let mut rows = stmt.query(params![node_id.0])?;

        while let Some(row) = rows.next()? {
            let mut edge = Self::edge_from_row(row)?;
            let target_symbol: String = row.get(9)?;

            // Avoid polluting trail exploration with low-confidence or ambiguous CALL resolutions.
            // It's better to show an unresolved symbol node than traverse to the wrong concrete node.
            if edge.kind == EdgeKind::CALL && edge.resolved_target.is_some() {
                if should_ignore_call_resolution(&target_symbol, edge.confidence) {
                    edge.resolved_target = None;
                    edge.confidence = None;
                }
            }
            let kind = edge.kind;

            // Apply edge filter if specified
            if !edge_filter.is_empty() && !edge_filter.contains(&kind) {
                continue;
            }

            let (eff_source, eff_target) = edge.effective_endpoints();
            let matches_node = match direction {
                TrailDirection::Outgoing => eff_source == node_id,
                TrailDirection::Incoming => eff_target == node_id,
                TrailDirection::Both => eff_source == node_id || eff_target == node_id,
            };
            if matches_node {
                edges.push(edge);
            }
        }
        Ok(edges)
    }

    /// Get all edges connected to a node (both directions)
    pub fn get_edges_for_node_id(&self, node_id: NodeId) -> Result<Vec<Edge>, StorageError> {
        self.get_edges_for_node(node_id, &TrailDirection::Both, &[])
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

fn should_ignore_call_resolution(target_symbol: &str, confidence: Option<f32>) -> bool {
    let conf = confidence.unwrap_or(0.0);

    // Fuzzy matches from the resolution pass use a low confidence score; these have shown to
    // create incorrect edges that seriously degrade trail graphs.
    if conf <= 0.4 + f32::EPSILON {
        return true;
    }

    // Even with exact matching, some unqualified method names are so common (often stdlib/3rd-party)
    // that resolving them globally by name alone is frequently wrong. Treat low-confidence
    // resolutions for these symbols as unresolved.
    if is_common_unqualified_call_name(target_symbol) && conf <= 0.6 + f32::EPSILON {
        return true;
    }

    false
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
mod tests {
    use super::*;
    use codestory_core::{
        Edge, EdgeId, EdgeKind, NodeId, NodeKind, SourceLocation, TrailConfig, TrailDirection,
    };

    #[test]
    fn test_batch_inserts() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;

        let nodes = vec![
            Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "func1".to_string(),
                ..Default::default()
            },
            Node {
                id: NodeId(2),
                kind: NodeKind::CLASS,
                serialized_name: "Class1".to_string(),
                ..Default::default()
            },
        ];

        storage.insert_nodes_batch(&nodes)?;

        let mut stmt = storage.conn.prepare("SELECT count(*) FROM node")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;
        assert_eq!(count, 2);

        Ok(())
    }

    #[test]
    fn test_occurrence_insert() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;
        let nodes = vec![
            Node {
                id: NodeId(10),
                kind: NodeKind::FILE,
                serialized_name: "file.rs".to_string(),
                ..Default::default()
            },
            Node {
                id: NodeId(11),
                kind: NodeKind::FUNCTION,
                serialized_name: "foo".to_string(),
                ..Default::default()
            },
        ];
        storage.insert_nodes_batch(&nodes)?;
        let occurrences = vec![Occurrence {
            element_id: 11,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: NodeId(10),
                start_line: 1,
                start_col: 0,
                end_line: 1,
                end_col: 10,
            },
        }];
        storage.insert_occurrences_batch(&occurrences)?;
        let mut stmt = storage.conn.prepare("SELECT count(*) FROM occurrence")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;
        assert_eq!(count, 1);
        Ok(())
    }

    #[test]
    fn test_file_storage() -> Result<(), StorageError> {
        let storage = Storage::new_in_memory()?;
        let info = FileInfo {
            id: 1,
            path: PathBuf::from("src/main.rs"),
            language: "rust".to_string(),
            modification_time: 12345678,
            indexed: true,
            complete: true,
            line_count: 100,
        };
        storage.insert_file(&info)?;
        let files = storage.get_files()?;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, PathBuf::from("src/main.rs"));
        assert_eq!(files[0].line_count, 100);
        Ok(())
    }

    #[test]
    fn test_error_storage() -> Result<(), StorageError> {
        let storage = Storage::new_in_memory()?;
        let info = FileInfo {
            id: 1,
            path: PathBuf::from("src/main.rs"),
            language: "rust".to_string(),
            modification_time: 12345678,
            indexed: true,
            complete: true,
            line_count: 100,
        };
        storage.insert_file(&info)?;
        let error = codestory_core::ErrorInfo {
            message: "Syntax error".to_string(),
            file_id: Some(NodeId(1)),
            line: Some(10),
            column: Some(5),
            is_fatal: true,
            index_step: codestory_core::IndexStep::Indexing,
        };
        storage.insert_error(&error)?;
        let stats = storage.get_stats()?;
        assert_eq!(stats.error_count, 1);
        Ok(())
    }

    #[test]
    fn test_node_cache() -> Result<(), StorageError> {
        let storage = Storage::new_in_memory()?;
        let node = Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "test_node".to_string(),
            ..Default::default()
        };
        storage.insert_node(&node)?;
        {
            let cache = storage.cache.nodes.read();
            assert!(cache.contains_key(&NodeId(1)));
        }
        let fetched = storage.get_node(NodeId(1))?.unwrap();
        assert_eq!(fetched.serialized_name, "test_node");
        Ok(())
    }

    #[test]
    fn test_bookmark_crud() -> Result<(), StorageError> {
        let storage = Storage::new_in_memory()?;

        // Create category
        let cat_id = storage.create_bookmark_category("Favorites")?;
        assert!(cat_id > 0);

        // Get categories
        let categories = storage.get_bookmark_categories()?;
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].name, "Favorites");

        // Create node for bookmark
        let node = Node {
            id: NodeId(100),
            kind: NodeKind::FUNCTION,
            serialized_name: "my_function".to_string(),
            ..Default::default()
        };
        storage.insert_node(&node)?;

        // Add bookmark
        let bm_id = storage.add_bookmark(cat_id, NodeId(100), Some("Important function"))?;
        assert!(bm_id > 0);

        // Get bookmarks
        let bookmarks = storage.get_bookmarks(Some(cat_id))?;
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].node_id, NodeId(100));
        assert_eq!(bookmarks[0].comment, Some("Important function".to_string()));

        // Update comment
        storage.update_bookmark_comment(bm_id, "Updated comment")?;
        let bookmarks = storage.get_bookmarks(Some(cat_id))?;
        assert_eq!(bookmarks[0].comment, Some("Updated comment".to_string()));

        // Delete bookmark
        storage.delete_bookmark(bm_id)?;
        let bookmarks = storage.get_bookmarks(Some(cat_id))?;
        assert_eq!(bookmarks.len(), 0);

        // Delete category
        storage.delete_bookmark_category(cat_id)?;
        let categories = storage.get_bookmark_categories()?;
        assert_eq!(categories.len(), 0);

        Ok(())
    }

    #[test]
    fn test_get_errors() -> Result<(), StorageError> {
        let storage = Storage::new_in_memory()?;

        // Insert errors
        storage.insert_error(&codestory_core::ErrorInfo {
            message: "Fatal error".to_string(),
            file_id: None,
            line: Some(10),
            column: None,
            is_fatal: true,
            index_step: codestory_core::IndexStep::Indexing,
        })?;
        storage.insert_error(&codestory_core::ErrorInfo {
            message: "Warning".to_string(),
            file_id: None,
            line: Some(20),
            column: None,
            is_fatal: false,
            index_step: codestory_core::IndexStep::Collection,
        })?;

        // Get all errors
        let errors = storage.get_errors(None)?;
        assert_eq!(errors.len(), 2);

        // Get fatal errors only
        let filter = codestory_core::ErrorFilter {
            fatal_only: true,
            indexed_only: false,
        };
        let errors = storage.get_errors(Some(&filter))?;
        assert_eq!(errors.len(), 1);
        assert!(errors[0].is_fatal);

        Ok(())
    }

    #[test]
    fn test_trail_query() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;

        // Create a simple graph: A -> B -> C
        let nodes = vec![
            Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "A".to_string(),
                ..Default::default()
            },
            Node {
                id: NodeId(2),
                kind: NodeKind::FUNCTION,
                serialized_name: "B".to_string(),
                ..Default::default()
            },
            Node {
                id: NodeId(3),
                kind: NodeKind::FUNCTION,
                serialized_name: "C".to_string(),
                ..Default::default()
            },
        ];
        storage.insert_nodes_batch(&nodes)?;

        let edges = vec![
            Edge {
                id: codestory_core::EdgeId(1),
                source: NodeId(1),
                target: NodeId(2),
                kind: EdgeKind::CALL,
                ..Default::default()
            },
            Edge {
                id: codestory_core::EdgeId(2),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                ..Default::default()
            },
        ];
        storage.insert_edges_batch(&edges)?;

        // Trail from A, depth 1, should get A and B
        let config = TrailConfig {
            root_id: NodeId(1),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 1,
            direction: TrailDirection::Outgoing,
            edge_filter: vec![],
            node_filter: Vec::new(),
            max_nodes: 100,
        };
        let result = storage.get_trail(&config)?;
        assert_eq!(result.nodes.len(), 2);
        assert!(!result.truncated);

        // Trail from A, depth 2, should get A, B, and C
        let config = TrailConfig {
            root_id: NodeId(1),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 2,
            direction: TrailDirection::Outgoing,
            edge_filter: vec![],
            node_filter: Vec::new(),
            max_nodes: 100,
        };
        let result = storage.get_trail(&config)?;
        assert_eq!(result.nodes.len(), 3);

        // Trail from A, depth 0 (infinite), should also get A, B, and C (bounded by max_nodes)
        let config = TrailConfig {
            root_id: NodeId(1),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 0,
            direction: TrailDirection::Outgoing,
            edge_filter: vec![],
            node_filter: Vec::new(),
            max_nodes: 100,
        };
        let result = storage.get_trail(&config)?;
        assert_eq!(result.nodes.len(), 3);

        Ok(())
    }

    #[test]
    fn test_trail_to_target_symbol_simple_path() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;

        let nodes = vec![
            Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "A".to_string(),
                ..Default::default()
            },
            Node {
                id: NodeId(2),
                kind: NodeKind::FUNCTION,
                serialized_name: "B".to_string(),
                ..Default::default()
            },
            Node {
                id: NodeId(3),
                kind: NodeKind::FUNCTION,
                serialized_name: "C".to_string(),
                ..Default::default()
            },
        ];
        storage.insert_nodes_batch(&nodes)?;

        storage.insert_edges_batch(&[
            Edge {
                id: EdgeId(1),
                source: NodeId(1),
                target: NodeId(2),
                kind: EdgeKind::CALL,
                ..Default::default()
            },
            Edge {
                id: EdgeId(2),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                ..Default::default()
            },
        ])?;

        let result = storage.get_trail(&TrailConfig {
            root_id: NodeId(1),
            mode: TrailMode::ToTargetSymbol,
            target_id: Some(NodeId(3)),
            depth: 2,
            direction: TrailDirection::Outgoing, // ignored/forced by mode, but set for clarity
            edge_filter: vec![],
            node_filter: Vec::new(),
            max_nodes: 100,
        })?;

        assert_eq!(result.nodes.len(), 3);
        assert_eq!(result.edges.len(), 2);
        assert!(!result.truncated);

        Ok(())
    }

    #[test]
    fn test_trail_ignores_ambiguous_call_resolutions() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;

        let caller = Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_string(),
            qualified_name: Some("caller".to_string()),
            ..Default::default()
        };
        let call_symbol = Node {
            id: NodeId(10),
            kind: NodeKind::UNKNOWN,
            serialized_name: "add".to_string(),
            ..Default::default()
        };
        let resolved = Node {
            id: NodeId(3),
            kind: NodeKind::METHOD,
            serialized_name: "SomeType::add".to_string(),
            qualified_name: Some("SomeType::add".to_string()),
            ..Default::default()
        };

        storage.insert_nodes_batch(&[caller.clone(), call_symbol.clone(), resolved.clone()])?;
        storage.insert_edges_batch(&[Edge {
            id: EdgeId(100),
            source: caller.id,
            target: call_symbol.id,
            kind: EdgeKind::CALL,
            resolved_target: Some(resolved.id),
            confidence: Some(0.6),
            ..Default::default()
        }])?;

        // Exploring from the resolved target should not traverse this edge.
        let result = storage.get_trail(&TrailConfig {
            root_id: resolved.id,
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 1,
            direction: TrailDirection::Incoming,
            edge_filter: vec![EdgeKind::CALL],
            node_filter: Vec::new(),
            max_nodes: 50,
        })?;

        assert!(result.edges.is_empty());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].id, resolved.id);

        Ok(())
    }

    #[test]
    fn test_safe_enum_conversion() -> Result<(), StorageError> {
        let mut storage = Storage::new_in_memory()?;

        // Test that we can round-trip all NodeKind variants
        let node = Node {
            id: NodeId(1),
            kind: NodeKind::ENUM_CONSTANT,
            serialized_name: "test".to_string(),
            ..Default::default()
        };
        storage.insert_nodes_batch(&[node])?;

        let nodes = storage.get_nodes()?;
        assert_eq!(nodes[0].kind, NodeKind::ENUM_CONSTANT);

        // Test that we can round-trip all EdgeKind variants
        let edges = vec![Edge {
            id: codestory_core::EdgeId(1),
            source: NodeId(1),
            target: NodeId(1),
            kind: EdgeKind::ANNOTATION_USAGE,
            ..Default::default()
        }];
        storage.insert_edges_batch(&edges)?;

        let edges = storage.get_edges()?;
        assert_eq!(edges[0].kind, EdgeKind::ANNOTATION_USAGE);

        Ok(())
    }
}
