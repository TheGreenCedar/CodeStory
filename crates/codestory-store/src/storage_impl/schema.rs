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
    "CREATE TABLE IF NOT EXISTS llm_symbol_doc (
        node_id INTEGER PRIMARY KEY,
        file_node_id INTEGER,
        kind INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        qualified_name TEXT,
        file_path TEXT,
        start_line INTEGER,
        doc_text TEXT NOT NULL,
        embedding_model TEXT NOT NULL,
        embedding_dim INTEGER NOT NULL,
        embedding_blob BLOB NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL,
        FOREIGN KEY(node_id) REFERENCES node(id),
        FOREIGN KEY(file_node_id) REFERENCES node(id)
    )",
    "CREATE TABLE IF NOT EXISTS callable_projection_state (
        file_id INTEGER NOT NULL,
        symbol_key TEXT NOT NULL,
        node_id INTEGER NOT NULL,
        signature_hash INTEGER NOT NULL,
        body_hash INTEGER NOT NULL,
        start_line INTEGER NOT NULL,
        end_line INTEGER NOT NULL,
        PRIMARY KEY (file_id, symbol_key),
        FOREIGN KEY(file_id) REFERENCES file(id),
        FOREIGN KEY(node_id) REFERENCES node(id)
    )",
    "CREATE TABLE IF NOT EXISTS grounding_snapshot_meta (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        snapshot_version INTEGER NOT NULL,
        summary_state INTEGER NOT NULL,
        detail_state INTEGER NOT NULL,
        summary_built_at_epoch_ms INTEGER,
        detail_built_at_epoch_ms INTEGER
    )",
    "CREATE TABLE IF NOT EXISTS grounding_repo_stats_snapshot (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        node_count INTEGER NOT NULL,
        edge_count INTEGER NOT NULL,
        file_count INTEGER NOT NULL,
        error_count INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS grounding_file_snapshot (
        file_id INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        language TEXT NOT NULL,
        modification_time INTEGER NOT NULL,
        indexed INTEGER NOT NULL,
        complete INTEGER NOT NULL,
        line_count INTEGER NOT NULL,
        symbol_count INTEGER NOT NULL,
        best_node_rank INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS grounding_node_snapshot (
        node_id INTEGER PRIMARY KEY,
        kind INTEGER NOT NULL,
        serialized_name TEXT NOT NULL,
        qualified_name TEXT,
        canonical_id TEXT,
        file_node_id INTEGER,
        start_line INTEGER,
        start_col INTEGER,
        end_line INTEGER,
        end_col INTEGER,
        display_name TEXT NOT NULL,
        file_path TEXT,
        node_rank INTEGER NOT NULL,
        sort_start_line INTEGER NOT NULL,
        is_root INTEGER NOT NULL,
        file_symbol_rank INTEGER
    )",
    "CREATE TABLE IF NOT EXISTS grounding_node_summary_snapshot (
        node_id INTEGER PRIMARY KEY,
        member_count INTEGER NOT NULL,
        fallback_occurrence_line INTEGER
    )",
    "CREATE TABLE IF NOT EXISTS grounding_node_edge_digest_snapshot (
        node_id INTEGER NOT NULL,
        kind INTEGER NOT NULL,
        count INTEGER NOT NULL,
        PRIMARY KEY (node_id, kind)
    )",
    "CREATE TABLE IF NOT EXISTS index_artifact_cache (
        file_path TEXT PRIMARY KEY,
        cache_key TEXT NOT NULL,
        artifact_blob BLOB NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS resolution_support_snapshot (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        snapshot_version INTEGER NOT NULL,
        state INTEGER NOT NULL,
        snapshot_blob BLOB,
        built_at_epoch_ms INTEGER
    )",
    "INSERT OR IGNORE INTO resolution_support_snapshot (
        id,
        snapshot_version,
        state,
        snapshot_blob,
        built_at_epoch_ms
    ) VALUES (1, 0, 0, NULL, NULL)",
];

const LOAD_TIME_INDEX_STATEMENTS: &[&str] = &[
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_occurrence_unique
     ON occurrence(element_id, file_node_id, start_line, start_col, end_line, end_col)",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_component_access_node ON component_access(node_id)",
];

const SECONDARY_INDEX_STATEMENTS: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS idx_occurrence_element ON occurrence(element_id)",
    "CREATE INDEX IF NOT EXISTS idx_occurrence_element_start_line ON occurrence(element_id, start_line)",
    "CREATE INDEX IF NOT EXISTS idx_occurrence_file ON occurrence(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_source ON edge(source_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_target ON edge(target_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_file ON edge(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_scope_unresolved
     ON edge(kind, resolved_target_node_id, source_node_id, file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_node_file ON node(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_node_file_kind_line ON node(file_node_id, kind, start_line)",
    "CREATE INDEX IF NOT EXISTS idx_node_file_kind_name ON node(file_node_id, kind, qualified_name, serialized_name)",
    "CREATE INDEX IF NOT EXISTS idx_node_qualified_name ON node(qualified_name)",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_node_category ON bookmark_node(category_id)",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_node_node ON bookmark_node(node_id)",
    "CREATE INDEX IF NOT EXISTS idx_node_kind_serialized_name ON node(kind, serialized_name)",
    "CREATE INDEX IF NOT EXISTS idx_edge_resolved_source ON edge(resolved_source_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_resolved_target ON edge(resolved_target_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_kind_source ON edge(kind, source_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_kind_target ON edge(kind, target_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_kind_resolved_target ON edge(kind, resolved_target_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_edge_line ON edge(line)",
    "CREATE INDEX IF NOT EXISTS idx_edge_callsite_identity ON edge(callsite_identity)",
    "CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_file_node ON llm_symbol_doc(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_kind ON llm_symbol_doc(kind)",
    "CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_updated_at ON llm_symbol_doc(updated_at_epoch_ms)",
    "CREATE INDEX IF NOT EXISTS idx_callable_projection_state_node_id ON callable_projection_state(node_id)",
    "CREATE INDEX IF NOT EXISTS idx_callable_projection_state_file_node ON callable_projection_state(file_id, node_id)",
    "CREATE INDEX IF NOT EXISTS idx_grounding_file_snapshot_path ON grounding_file_snapshot(path)",
    "CREATE INDEX IF NOT EXISTS idx_grounding_file_snapshot_rank
     ON grounding_file_snapshot(best_node_rank, symbol_count DESC, path)",
    "CREATE INDEX IF NOT EXISTS idx_grounding_node_snapshot_file_rank
     ON grounding_node_snapshot(file_node_id, file_symbol_rank, node_id)",
    "CREATE INDEX IF NOT EXISTS idx_grounding_node_snapshot_root_rank
     ON grounding_node_snapshot(is_root, node_rank, sort_start_line, display_name, node_id)",
    "CREATE INDEX IF NOT EXISTS idx_index_artifact_cache_key
     ON index_artifact_cache(cache_key)",
];

pub(super) fn create_tables(conn: &Connection) -> Result<(), StorageError> {
    for statement in TABLE_STATEMENTS {
        conn.execute(statement, [])?;
    }
    Ok(())
}

pub(super) fn create_load_indexes(conn: &Connection) -> Result<(), StorageError> {
    for statement in LOAD_TIME_INDEX_STATEMENTS {
        conn.execute(statement, [])?;
    }
    Ok(())
}

pub(super) fn create_secondary_indexes(conn: &Connection) -> Result<(), StorageError> {
    for statement in SECONDARY_INDEX_STATEMENTS {
        conn.execute(statement, [])?;
    }
    Ok(())
}

pub(super) fn create_indexes(conn: &Connection, mode: StorageOpenMode) -> Result<(), StorageError> {
    create_load_indexes(conn)?;
    if matches!(mode, StorageOpenMode::Live) {
        create_secondary_indexes(conn)?;
    }
    Ok(())
}

pub(super) fn create_deferred_indexes(conn: &Connection) -> Result<(), StorageError> {
    create_secondary_indexes(conn)
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

    if stored_version < 3 {
        migrate_v3_llm_symbol_projection(&storage.conn)?;
        storage.set_schema_version(3)?;
    }

    if stored_version < 4 {
        migrate_v4_reset_projection_state(storage)?;
        storage.set_schema_version(4)?;
    }

    if stored_version < 5 {
        migrate_v5_index_coverage(&storage.conn)?;
        storage.set_schema_version(5)?;
    }

    if stored_version < 6 {
        migrate_v6_grounding_snapshot_tables(&storage.conn)?;
        storage.set_schema_version(6)?;
    }

    if stored_version < 7 {
        migrate_v7_grounding_snapshot_relayout(&storage.conn)?;
        storage.set_schema_version(7)?;
    }

    if stored_version < 8 {
        migrate_v8_incremental_cache_tables(&storage.conn)?;
        storage.set_schema_version(8)?;
    }

    if stored_version < 9 {
        migrate_v9_grounding_snapshot_tiers(&storage.conn)?;
        storage.set_schema_version(9)?;
    }

    if storage.deferred_secondary_indexes {
        create_load_indexes(&storage.conn)?;
    } else {
        create_secondary_indexes(&storage.conn)?;
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
    Ok(())
}

pub(super) fn migrate_v3_llm_symbol_projection(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS llm_symbol_doc (
            node_id INTEGER PRIMARY KEY,
            file_node_id INTEGER,
            kind INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            qualified_name TEXT,
            file_path TEXT,
            start_line INTEGER,
            doc_text TEXT NOT NULL,
            embedding_model TEXT NOT NULL,
            embedding_dim INTEGER NOT NULL,
            embedding_blob BLOB NOT NULL,
            updated_at_epoch_ms INTEGER NOT NULL,
            FOREIGN KEY(node_id) REFERENCES node(id),
            FOREIGN KEY(file_node_id) REFERENCES node(id)
        )",
        [],
    )?;
    Ok(())
}

pub(super) fn migrate_v4_reset_projection_state(storage: &Storage) -> Result<(), StorageError> {
    storage.clear()?;
    storage.conn.execute("DELETE FROM bookmark_category", [])?;
    Ok(())
}

pub(super) fn migrate_v5_index_coverage(conn: &Connection) -> Result<(), StorageError> {
    create_load_indexes(conn)
}

pub(super) fn migrate_v6_grounding_snapshot_tables(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS grounding_snapshot_meta (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            snapshot_version INTEGER NOT NULL,
            summary_state INTEGER NOT NULL,
            detail_state INTEGER NOT NULL,
            summary_built_at_epoch_ms INTEGER,
            detail_built_at_epoch_ms INTEGER
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS grounding_repo_stats_snapshot (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            node_count INTEGER NOT NULL,
            edge_count INTEGER NOT NULL,
            file_count INTEGER NOT NULL,
            error_count INTEGER NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS grounding_file_snapshot (
            file_id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            language TEXT NOT NULL,
            modification_time INTEGER NOT NULL,
            indexed INTEGER NOT NULL,
            complete INTEGER NOT NULL,
            line_count INTEGER NOT NULL,
            symbol_count INTEGER NOT NULL,
            best_node_rank INTEGER NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS grounding_node_snapshot (
            node_id INTEGER PRIMARY KEY,
            kind INTEGER NOT NULL,
            serialized_name TEXT NOT NULL,
            qualified_name TEXT,
            canonical_id TEXT,
            file_node_id INTEGER,
            start_line INTEGER,
            start_col INTEGER,
            end_line INTEGER,
            end_col INTEGER,
            display_name TEXT NOT NULL,
            file_path TEXT,
            node_rank INTEGER NOT NULL,
            sort_start_line INTEGER NOT NULL,
            is_root INTEGER NOT NULL,
            file_symbol_rank INTEGER
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS grounding_node_summary_snapshot (
            node_id INTEGER PRIMARY KEY,
            member_count INTEGER NOT NULL,
            fallback_occurrence_line INTEGER
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS grounding_node_edge_digest_snapshot (
            node_id INTEGER NOT NULL,
            kind INTEGER NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (node_id, kind)
        )",
        [],
    )?;
    conn.execute(
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
        params![GROUNDING_SNAPSHOT_VERSION, GROUNDING_SNAPSHOT_STATE_DIRTY],
    )?;
    Ok(())
}

pub(super) fn migrate_v7_grounding_snapshot_relayout(
    conn: &Connection,
) -> Result<(), StorageError> {
    conn.execute("DROP TABLE IF EXISTS grounding_root_symbol", [])?;
    conn.execute("DROP TABLE IF EXISTS grounding_file_symbol", [])?;
    conn.execute("DROP TABLE IF EXISTS grounding_file_summary", [])?;
    conn.execute("DROP TABLE IF EXISTS grounding_snapshot_state", [])?;
    migrate_v6_grounding_snapshot_tables(conn)
}

pub(super) fn migrate_v8_incremental_cache_tables(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS index_artifact_cache (
            file_path TEXT PRIMARY KEY,
            cache_key TEXT NOT NULL,
            artifact_blob BLOB NOT NULL,
            updated_at_epoch_ms INTEGER NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS resolution_support_snapshot (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            snapshot_version INTEGER NOT NULL,
            state INTEGER NOT NULL,
            snapshot_blob BLOB,
            built_at_epoch_ms INTEGER
        )",
        [],
    )?;
    conn.execute(
        "INSERT INTO resolution_support_snapshot (
            id,
            snapshot_version,
            state,
            snapshot_blob,
            built_at_epoch_ms
        )
         VALUES (1, 0, 0, NULL, NULL)
         ON CONFLICT(id) DO NOTHING",
        [],
    )?;
    Ok(())
}

pub(super) fn migrate_v9_grounding_snapshot_tiers(conn: &Connection) -> Result<(), StorageError> {
    let column_names = conn
        .prepare("PRAGMA table_info(grounding_snapshot_meta)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_legacy_state = column_names.iter().any(|name| name == "state");
    let has_legacy_built_at = column_names.iter().any(|name| name == "built_at_epoch_ms");
    let has_summary_state = column_names.iter().any(|name| name == "summary_state");
    let has_detail_state = column_names.iter().any(|name| name == "detail_state");
    let has_summary_built_at = column_names
        .iter()
        .any(|name| name == "summary_built_at_epoch_ms");
    let has_detail_built_at = column_names
        .iter()
        .any(|name| name == "detail_built_at_epoch_ms");

    let summary_state_expr = if has_summary_state {
        "summary_state".to_string()
    } else if has_legacy_state {
        "state".to_string()
    } else {
        GROUNDING_SNAPSHOT_STATE_DIRTY.to_string()
    };
    let detail_state_expr = if has_detail_state {
        "detail_state".to_string()
    } else if has_legacy_state {
        "state".to_string()
    } else {
        GROUNDING_SNAPSHOT_STATE_DIRTY.to_string()
    };
    let summary_built_at_expr = if has_summary_built_at {
        "summary_built_at_epoch_ms".to_string()
    } else if has_legacy_built_at {
        "built_at_epoch_ms".to_string()
    } else {
        "NULL".to_string()
    };
    let detail_built_at_expr = if has_detail_built_at {
        "detail_built_at_epoch_ms".to_string()
    } else if has_legacy_built_at {
        "built_at_epoch_ms".to_string()
    } else {
        "NULL".to_string()
    };

    conn.execute("DROP TABLE IF EXISTS grounding_snapshot_meta_legacy", [])?;
    conn.execute(
        "ALTER TABLE grounding_snapshot_meta RENAME TO grounding_snapshot_meta_legacy",
        [],
    )?;
    conn.execute(
        "CREATE TABLE grounding_snapshot_meta (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            snapshot_version INTEGER NOT NULL,
            summary_state INTEGER NOT NULL,
            detail_state INTEGER NOT NULL,
            summary_built_at_epoch_ms INTEGER,
            detail_built_at_epoch_ms INTEGER
        )",
        [],
    )?;
    let copy_sql = format!(
        "INSERT INTO grounding_snapshot_meta (
            id,
            snapshot_version,
            summary_state,
            detail_state,
            summary_built_at_epoch_ms,
            detail_built_at_epoch_ms
        )
         SELECT
            id,
            snapshot_version,
            COALESCE({summary_state_expr}, {dirty}),
            COALESCE({detail_state_expr}, {dirty}),
            {summary_built_at_expr},
            {detail_built_at_expr}
         FROM grounding_snapshot_meta_legacy",
        dirty = GROUNDING_SNAPSHOT_STATE_DIRTY,
    );
    conn.execute(&copy_sql, [])?;
    conn.execute("DROP TABLE grounding_snapshot_meta_legacy", [])?;
    conn.execute(
        "INSERT OR IGNORE INTO grounding_snapshot_meta (
            id,
            snapshot_version,
            summary_state,
            detail_state,
            summary_built_at_epoch_ms,
            detail_built_at_epoch_ms
        )
         VALUES (1, ?1, ?2, ?2, NULL, NULL)",
        params![GROUNDING_SNAPSHOT_VERSION, GROUNDING_SNAPSHOT_STATE_DIRTY],
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
