use super::*;

const TABLE_STATEMENTS: &[&str] = &[
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
        callsite_identity TEXT,
        certainty TEXT,
        candidate_target_node_ids TEXT,
        FOREIGN KEY(source_node_id) REFERENCES node(id),
        FOREIGN KEY(target_node_id) REFERENCES node(id),
        FOREIGN KEY(file_node_id) REFERENCES node(id),
        FOREIGN KEY(resolved_source_node_id) REFERENCES node(id),
        FOREIGN KEY(resolved_target_node_id) REFERENCES node(id)
    )",
    "CREATE TABLE IF NOT EXISTS occurrence (
         element_id INTEGER NOT NULL,
         kind INTEGER NOT NULL,
         file_node_id INTEGER NOT NULL,
         start_line INTEGER NOT NULL,
         start_col INTEGER NOT NULL,
         end_line INTEGER NOT NULL,
         end_col INTEGER NOT NULL
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_occurrence_unique
     ON occurrence(element_id, file_node_id, start_line, start_col, end_line, end_col)",
    "CREATE TABLE IF NOT EXISTS file (
        id INTEGER PRIMARY KEY,
        path TEXT UNIQUE NOT NULL,
        language TEXT,
        modification_time INTEGER,
        indexed INTEGER DEFAULT 0,
        complete INTEGER DEFAULT 0,
        line_count INTEGER DEFAULT 0
    )",
    "CREATE TABLE IF NOT EXISTS local_symbol (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        file_id INTEGER,
        FOREIGN KEY(file_id) REFERENCES file(id)
    )",
    "CREATE TABLE IF NOT EXISTS component_access (
        node_id INTEGER,
        type INTEGER,
        FOREIGN KEY(node_id) REFERENCES node(id)
    )",
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
    "CREATE TABLE IF NOT EXISTS bookmark_category (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS bookmark_node (
        id INTEGER PRIMARY KEY,
        category_id INTEGER,
        node_id INTEGER,
        comment TEXT,
        FOREIGN KEY(category_id) REFERENCES bookmark_category(id),
        FOREIGN KEY(node_id) REFERENCES node(id)
    )",
];

const INDEX_STATEMENTS: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS idx_occurrence_element ON occurrence(element_id)",
    "CREATE INDEX IF NOT EXISTS idx_occurrence_file ON occurrence(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_source ON edge(source_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_target ON edge(target_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_node_file ON node(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_node_qualified_name ON node(qualified_name)",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_component_access_node ON component_access(node_id)",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_node_category ON bookmark_node(category_id)",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_node_node ON bookmark_node(node_id)",
    "CREATE INDEX IF NOT EXISTS idx_node_kind_serialized_name ON node(kind, serialized_name)",
    "CREATE INDEX IF NOT EXISTS idx_edge_resolved_source ON edge(resolved_source_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_resolved_target ON edge(resolved_target_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_kind_resolved_target ON edge(kind, resolved_target_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_line ON edge(line)",
    "CREATE INDEX IF NOT EXISTS idx_edge_callsite_identity ON edge(callsite_identity)",
];

pub(super) fn create_tables(conn: &Connection) -> Result<(), StorageError> {
    for statement in TABLE_STATEMENTS {
        conn.execute(statement, [])?;
    }
    Ok(())
}

pub(super) fn create_indexes(conn: &Connection) -> Result<(), StorageError> {
    for statement in INDEX_STATEMENTS {
        conn.execute(statement, [])?;
    }
    Ok(())
}

pub(super) fn apply_schema_migrations(storage: &Storage) -> Result<(), StorageError> {
    let stored_version = storage.schema_version()?;

    if stored_version > SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "Unsupported database schema version: {stored_version} (max supported: {SCHEMA_VERSION})"
        )));
    }

    if stored_version < 2 {
        migrate_v2_edge_metadata(&storage.conn)?;
        storage.set_schema_version(2)?;
    }

    if stored_version < SCHEMA_VERSION {
        storage.set_schema_version(SCHEMA_VERSION)?;
    }
    Ok(())
}

pub(super) fn migrate_v2_edge_metadata(conn: &Connection) -> Result<(), StorageError> {
    try_add_column(conn, "edge", "callsite_identity TEXT")?;
    try_add_column(conn, "edge", "certainty TEXT")?;
    try_add_column(conn, "edge", "candidate_target_node_ids TEXT")?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_edge_callsite_identity ON edge(callsite_identity)",
        [],
    )?;
    Ok(())
}

pub(super) fn try_add_column(
    conn: &Connection,
    table: &str,
    column_sql: &str,
) -> Result<(), StorageError> {
    let column_name = column_sql
        .split_whitespace()
        .next()
        .ok_or_else(|| StorageError::Other("missing column name in migration".to_string()))?;
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let existing_name: String = row.get(1)?;
        if existing_name == column_name {
            return Ok(());
        }
    }

    let sql = format!("ALTER TABLE {table} ADD COLUMN {column_sql}");
    conn.execute(&sql, [])?;
    Ok(())
}
