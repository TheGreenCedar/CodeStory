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
        line_count INTEGER DEFAULT 0,
        file_role TEXT NOT NULL DEFAULT 'source',
        content_hash TEXT
    )",
    "CREATE TABLE IF NOT EXISTS incomplete_index_run (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        started_at_epoch_ms INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS index_publication (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        generation INTEGER NOT NULL CHECK (generation > 0),
        generation_id TEXT NOT NULL UNIQUE CHECK (length(generation_id) > 0),
        run_id TEXT NOT NULL CHECK (length(run_id) > 0),
        mode TEXT NOT NULL CHECK (mode IN ('full', 'incremental')),
        published_at_epoch_ms INTEGER NOT NULL CHECK (published_at_epoch_ms >= 0)
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
        doc_version INTEGER NOT NULL DEFAULT 0,
        doc_hash TEXT NOT NULL DEFAULT '',
        embedding_model TEXT NOT NULL,
        embedding_profile TEXT,
        embedding_backend TEXT,
        embedding_dim INTEGER NOT NULL,
        doc_shape TEXT,
        semantic_policy_version TEXT,
        dense_reason TEXT,
        embedding_blob BLOB NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL,
        FOREIGN KEY(node_id) REFERENCES node(id),
        FOREIGN KEY(file_node_id) REFERENCES node(id)
    )",
    "CREATE TABLE IF NOT EXISTS dense_anchor_input (
        node_id INTEGER PRIMARY KEY,
        file_node_id INTEGER,
        kind INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        qualified_name TEXT,
        file_path TEXT,
        start_line INTEGER,
        end_line INTEGER,
        file_role TEXT NOT NULL,
        source_provenance TEXT NOT NULL,
        document_text TEXT NOT NULL,
        document_hash TEXT NOT NULL CHECK(length(document_hash) > 0),
        selection_reason TEXT NOT NULL,
        policy_version TEXT NOT NULL,
        source_identity TEXT NOT NULL CHECK(length(source_identity) > 0),
        updated_at_epoch_ms INTEGER NOT NULL,
        FOREIGN KEY(node_id) REFERENCES node(id),
        FOREIGN KEY(file_node_id) REFERENCES node(id)
    )",
    "CREATE TABLE IF NOT EXISTS dense_anchor_publication (
        id INTEGER PRIMARY KEY CHECK(id = 1),
        schema_version INTEGER NOT NULL,
        complete INTEGER NOT NULL CHECK(complete = 1),
        core_generation_id TEXT NOT NULL CHECK(length(core_generation_id) > 0),
        core_run_id TEXT NOT NULL CHECK(length(core_run_id) > 0),
        anchor_count INTEGER NOT NULL CHECK(anchor_count >= 0),
        anchor_digest TEXT NOT NULL CHECK(length(anchor_digest) = 64),
        policy_version TEXT NOT NULL CHECK(length(policy_version) > 0),
        migration_state TEXT NOT NULL CHECK(length(migration_state) > 0),
        published_at_epoch_ms INTEGER NOT NULL CHECK(published_at_epoch_ms >= 0)
    )",
    "CREATE TABLE IF NOT EXISTS symbol_search_doc (
        node_id INTEGER PRIMARY KEY,
        file_node_id INTEGER,
        kind INTEGER NOT NULL,
        display_name TEXT NOT NULL,
        qualified_name TEXT,
        file_path TEXT,
        start_line INTEGER,
        doc_text TEXT NOT NULL,
        doc_version INTEGER NOT NULL DEFAULT 0,
        doc_hash TEXT NOT NULL DEFAULT '',
        policy_version TEXT NOT NULL,
        source_provenance TEXT NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL,
        FOREIGN KEY(node_id) REFERENCES node(id),
        FOREIGN KEY(file_node_id) REFERENCES node(id)
    )",
    "CREATE TABLE IF NOT EXISTS symbol_summary (
        node_id INTEGER NOT NULL,
        content_hash TEXT NOT NULL,
        summary TEXT NOT NULL,
        model TEXT NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL,
        PRIMARY KEY(node_id, content_hash),
        FOREIGN KEY(node_id) REFERENCES node(id)
    )",
    "CREATE TABLE IF NOT EXISTS search_symbol_projection (
        node_id INTEGER PRIMARY KEY,
        display_name TEXT NOT NULL,
        FOREIGN KEY(node_id) REFERENCES node(id)
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
    "CREATE TABLE IF NOT EXISTS retrieval_index_manifest (
        project_id TEXT PRIMARY KEY,
        lexical_version TEXT NOT NULL,
        semantic_generation TEXT NOT NULL,
        scip_revision TEXT,
        built_at_epoch_ms INTEGER NOT NULL,
        disk_bytes INTEGER,
        degraded_modes_json TEXT NOT NULL DEFAULT '[]',
        embedding_backend TEXT,
        embedding_dim INTEGER,
        sidecar_schema_version INTEGER,
        sidecar_input_hash TEXT,
        sidecar_generation TEXT,
        projection_count INTEGER,
        symbol_doc_count INTEGER,
        dense_projection_count INTEGER,
        semantic_policy_version TEXT,
        graph_artifact_hash TEXT,
        dense_reason_counts_json TEXT,
        precise_semantic_import_status TEXT,
        precise_semantic_import_reason TEXT,
        precise_semantic_import_revision TEXT,
        precise_semantic_import_producer TEXT,
        rollback_record_json TEXT
    )",
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
    "CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_policy_reason
     ON llm_symbol_doc(semantic_policy_version, dense_reason)",
    "CREATE INDEX IF NOT EXISTS idx_dense_anchor_input_file_node
     ON dense_anchor_input(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_dense_anchor_input_reuse
     ON dense_anchor_input(policy_version, document_hash)",
    "CREATE INDEX IF NOT EXISTS idx_dense_anchor_input_source
     ON dense_anchor_input(source_identity)",
    "CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_file_node ON symbol_search_doc(file_node_id)",
    "CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_kind ON symbol_search_doc(kind)",
    "CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_policy ON symbol_search_doc(policy_version)",
    "CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_hash ON symbol_search_doc(doc_version, doc_hash)",
    "CREATE INDEX IF NOT EXISTS idx_search_symbol_projection_display_name
     ON search_symbol_projection(display_name)",
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
    "CREATE INDEX IF NOT EXISTS idx_retrieval_index_manifest_built_at
     ON retrieval_index_manifest(built_at_epoch_ms)",
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

    if stored_version != INCOMPLETE_INCREMENTAL_SCHEMA_VERSION && stored_version > SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "Unsupported database schema version: {stored_version} (max supported: {SCHEMA_VERSION})"
        )));
    }
    if stored_version == INCOMPLETE_INCREMENTAL_SCHEMA_VERSION
        && !storage.has_incomplete_incremental_run()?
    {
        return Err(StorageError::Other(format!(
            "Database schema version {stored_version} is only valid while an incremental index run is marked incomplete"
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

    if stored_version < 10 {
        migrate_v10_search_symbol_projection(&storage.conn)?;
        storage.set_schema_version(10)?;
    }

    if stored_version < 11 {
        migrate_v11_llm_symbol_doc_reuse_metadata(&storage.conn)?;
        storage.set_schema_version(11)?;
    }
    if stored_version < 12 {
        migrate_v12_symbol_summary(&storage.conn)?;
        storage.set_schema_version(12)?;
    }
    if stored_version < 13 {
        migrate_v13_llm_symbol_doc_embedding_contract(&storage.conn)?;
        storage.set_schema_version(13)?;
    }
    if stored_version < 14 {
        migrate_v14_retrieval_index_manifest(&storage.conn)?;
        storage.set_schema_version(14)?;
    }
    if stored_version < 15 {
        migrate_v15_retrieval_manifest_embedding(&storage.conn)?;
        storage.set_schema_version(15)?;
    }
    if stored_version < 16 {
        migrate_v16_file_role(&storage.conn)?;
        storage.set_schema_version(16)?;
    }
    if stored_version < 17 {
        migrate_v17_retrieval_manifest_sidecar_generation(&storage.conn)?;
        storage.set_schema_version(17)?;
    }
    if stored_version < 18 {
        migrate_v18_ast_first_symbol_docs(&storage.conn)?;
        storage.set_schema_version(18)?;
    }
    if stored_version >= 18 {
        // ponytail: v18 caches can be stamped current while missing additive columns; drop the rerun once v18-stamped caches are rebuilt.
        migrate_v18_ast_first_symbol_docs(&storage.conn)?;
    }
    if stored_version < 20 || stored_version == INCOMPLETE_INCREMENTAL_SCHEMA_VERSION {
        migrate_v20_file_content_hash(&storage.conn)?;
        if stored_version != INCOMPLETE_INCREMENTAL_SCHEMA_VERSION {
            storage.set_schema_version(20)?;
        }
    }
    // The interrupted-incremental sentinel sits outside the sequential version range, so inspect
    // the actual columns instead of skipping this idempotent rename while an older run recovers.
    migrate_v21_retrieval_manifest_lexical_version(&storage.conn)?;
    if stored_version < 21 {
        storage.set_schema_version(21)?;
    }
    migrate_v22_retrieval_manifest_semantic_generation(&storage.conn)?;
    if stored_version < 22 {
        storage.set_schema_version(22)?;
    }
    migrate_v23_dense_anchor_input(&storage.conn)?;
    if stored_version < 23 {
        storage.set_schema_version(23)?;
    }
    migrate_v24_dense_anchor_publication(&storage.conn)?;
    if stored_version < 24 {
        storage.set_schema_version(24)?;
    }
    migrate_v25_retrieval_rollback(&storage.conn)?;
    if stored_version < 25 {
        storage.set_schema_version(25)?;
    }
    create_llm_symbol_doc_reuse_index(&storage.conn)?;
    create_symbol_summary_indexes(&storage.conn)?;

    let index_mode = if storage.deferred_secondary_indexes {
        StorageOpenMode::Build
    } else {
        StorageOpenMode::Live
    };
    create_indexes(&storage.conn, index_mode)?;

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
            doc_version INTEGER NOT NULL DEFAULT 0,
            doc_hash TEXT NOT NULL DEFAULT '',
            embedding_model TEXT NOT NULL,
            embedding_profile TEXT,
            embedding_backend TEXT,
            embedding_dim INTEGER NOT NULL,
            doc_shape TEXT,
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

pub(super) fn migrate_v11_llm_symbol_doc_reuse_metadata(
    conn: &Connection,
) -> Result<(), StorageError> {
    try_add_column(
        conn,
        "llm_symbol_doc",
        "doc_version INTEGER NOT NULL DEFAULT 0",
    )?;
    try_add_column(conn, "llm_symbol_doc", "doc_hash TEXT NOT NULL DEFAULT ''")?;
    create_llm_symbol_doc_reuse_index(conn)?;
    Ok(())
}

fn create_llm_symbol_doc_reuse_index(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_model_hash
         ON llm_symbol_doc(embedding_model, doc_version, doc_hash)",
        [],
    )?;
    Ok(())
}

pub(super) fn migrate_v12_symbol_summary(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS symbol_summary (
            node_id INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            summary TEXT NOT NULL,
            model TEXT NOT NULL,
            updated_at_epoch_ms INTEGER NOT NULL,
            PRIMARY KEY(node_id, content_hash),
            FOREIGN KEY(node_id) REFERENCES node(id)
        )",
        [],
    )?;
    create_symbol_summary_indexes(conn)?;
    Ok(())
}

pub(super) fn migrate_v13_llm_symbol_doc_embedding_contract(
    conn: &Connection,
) -> Result<(), StorageError> {
    try_add_column(conn, "llm_symbol_doc", "embedding_profile TEXT")?;
    try_add_column(conn, "llm_symbol_doc", "embedding_backend TEXT")?;
    try_add_column(conn, "llm_symbol_doc", "doc_shape TEXT")?;
    Ok(())
}

pub(super) fn migrate_v15_retrieval_manifest_embedding(
    conn: &Connection,
) -> Result<(), StorageError> {
    try_add_column(conn, "retrieval_index_manifest", "embedding_backend TEXT")?;
    try_add_column(conn, "retrieval_index_manifest", "embedding_dim INTEGER")?;
    Ok(())
}

pub(super) fn migrate_v16_file_role(conn: &Connection) -> Result<(), StorageError> {
    try_add_column(conn, "file", "file_role TEXT NOT NULL DEFAULT 'source'")?;
    conn.execute(
        "UPDATE file
         SET file_role = 'source'
         WHERE file_role IS NULL OR TRIM(file_role) = ''",
        [],
    )?;
    Ok(())
}

pub(super) fn migrate_v17_retrieval_manifest_sidecar_generation(
    conn: &Connection,
) -> Result<(), StorageError> {
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "sidecar_schema_version INTEGER",
    )?;
    try_add_column(conn, "retrieval_index_manifest", "sidecar_input_hash TEXT")?;
    try_add_column(conn, "retrieval_index_manifest", "sidecar_generation TEXT")?;
    try_add_column(conn, "retrieval_index_manifest", "projection_count INTEGER")?;
    Ok(())
}

pub(super) fn migrate_v18_ast_first_symbol_docs(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS symbol_search_doc (
            node_id INTEGER PRIMARY KEY,
            file_node_id INTEGER,
            kind INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            qualified_name TEXT,
            file_path TEXT,
            start_line INTEGER,
            doc_text TEXT NOT NULL,
            doc_version INTEGER NOT NULL DEFAULT 0,
            doc_hash TEXT NOT NULL DEFAULT '',
            policy_version TEXT NOT NULL,
            source_provenance TEXT NOT NULL,
            updated_at_epoch_ms INTEGER NOT NULL,
            FOREIGN KEY(node_id) REFERENCES node(id),
            FOREIGN KEY(file_node_id) REFERENCES node(id)
        )",
        [],
    )?;
    try_add_column(conn, "llm_symbol_doc", "semantic_policy_version TEXT")?;
    try_add_column(conn, "llm_symbol_doc", "dense_reason TEXT")?;
    try_add_column(conn, "retrieval_index_manifest", "symbol_doc_count INTEGER")?;
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "dense_projection_count INTEGER",
    )?;
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "semantic_policy_version TEXT",
    )?;
    try_add_column(conn, "retrieval_index_manifest", "graph_artifact_hash TEXT")?;
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "dense_reason_counts_json TEXT",
    )?;
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "precise_semantic_import_status TEXT",
    )?;
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "precise_semantic_import_reason TEXT",
    )?;
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "precise_semantic_import_revision TEXT",
    )?;
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "precise_semantic_import_producer TEXT",
    )?;
    Ok(())
}

pub(super) fn migrate_v20_file_content_hash(conn: &Connection) -> Result<(), StorageError> {
    try_add_column(conn, "file", "content_hash TEXT")
}

pub(super) fn migrate_v14_retrieval_index_manifest(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS retrieval_index_manifest (
            project_id TEXT PRIMARY KEY,
            zoekt_version TEXT NOT NULL,
            semantic_generation TEXT NOT NULL,
            scip_revision TEXT,
            built_at_epoch_ms INTEGER NOT NULL,
            disk_bytes INTEGER,
            degraded_modes_json TEXT NOT NULL DEFAULT '[]',
            precise_semantic_import_status TEXT,
            precise_semantic_import_reason TEXT,
            precise_semantic_import_revision TEXT,
            precise_semantic_import_producer TEXT
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_retrieval_index_manifest_built_at
         ON retrieval_index_manifest(built_at_epoch_ms)",
        [],
    )?;
    Ok(())
}

pub(super) fn migrate_v21_retrieval_manifest_lexical_version(
    conn: &Connection,
) -> Result<(), StorageError> {
    let columns = table_columns(conn, "retrieval_index_manifest")?;
    let has_legacy = columns.iter().any(|column| column == "zoekt_version");
    let has_lexical = columns.iter().any(|column| column == "lexical_version");
    match (has_legacy, has_lexical) {
        (true, false) => {
            conn.execute(
                "ALTER TABLE retrieval_index_manifest RENAME COLUMN zoekt_version TO lexical_version",
                [],
            )?;
            Ok(())
        }
        (false, true) => Ok(()),
        (true, true) => Err(StorageError::Other(
            "retrieval_index_manifest contains both legacy and lexical version columns".to_string(),
        )),
        (false, false) => Err(StorageError::Other(
            "retrieval_index_manifest is missing lexical_version".to_string(),
        )),
    }
}

pub(super) fn migrate_v22_retrieval_manifest_semantic_generation(
    conn: &Connection,
) -> Result<(), StorageError> {
    let columns = table_columns(conn, "retrieval_index_manifest")?;
    let has_old_name = columns.iter().any(|column| column == "qdrant_collection");
    let has_semantic = columns.iter().any(|column| column == "semantic_generation");
    match (has_old_name, has_semantic) {
        (true, false) => {
            conn.execute(
                "ALTER TABLE retrieval_index_manifest RENAME COLUMN qdrant_collection TO semantic_generation",
                [],
            )?;
            Ok(())
        }
        (false, true) => Ok(()),
        (true, true) => Err(StorageError::Other(
            "retrieval_index_manifest contains both old and semantic generation columns".into(),
        )),
        (false, false) => Err(StorageError::Other(
            "retrieval_index_manifest is missing semantic_generation".into(),
        )),
    }
}

pub(super) fn migrate_v23_dense_anchor_input(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS dense_anchor_input (
            node_id INTEGER PRIMARY KEY,
            file_node_id INTEGER,
            kind INTEGER NOT NULL,
            display_name TEXT NOT NULL,
            qualified_name TEXT,
            file_path TEXT,
            start_line INTEGER,
            end_line INTEGER,
            file_role TEXT NOT NULL,
            source_provenance TEXT NOT NULL,
            document_text TEXT NOT NULL,
            document_hash TEXT NOT NULL CHECK(length(document_hash) > 0),
            selection_reason TEXT NOT NULL,
            policy_version TEXT NOT NULL,
            source_identity TEXT NOT NULL CHECK(length(source_identity) > 0),
            updated_at_epoch_ms INTEGER NOT NULL,
            FOREIGN KEY(node_id) REFERENCES node(id),
            FOREIGN KEY(file_node_id) REFERENCES node(id)
        )",
        [],
    )?;
    Ok(())
}

pub(super) fn migrate_v24_dense_anchor_publication(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS dense_anchor_publication (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            schema_version INTEGER NOT NULL,
            complete INTEGER NOT NULL CHECK(complete = 1),
            core_generation_id TEXT NOT NULL CHECK(length(core_generation_id) > 0),
            core_run_id TEXT NOT NULL CHECK(length(core_run_id) > 0),
            anchor_count INTEGER NOT NULL CHECK(anchor_count >= 0),
            anchor_digest TEXT NOT NULL CHECK(length(anchor_digest) = 64),
            policy_version TEXT NOT NULL CHECK(length(policy_version) > 0),
            migration_state TEXT NOT NULL CHECK(length(migration_state) > 0),
            published_at_epoch_ms INTEGER NOT NULL CHECK(published_at_epoch_ms >= 0)
        )",
        [],
    )?;
    Ok(())
}

pub(super) fn migrate_v25_retrieval_rollback(conn: &Connection) -> Result<(), StorageError> {
    try_add_column(
        conn,
        "retrieval_index_manifest",
        "rollback_record_json TEXT",
    )
}

fn create_symbol_summary_indexes(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_symbol_summary_node
         ON symbol_summary(node_id)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_symbol_summary_updated
         ON symbol_summary(updated_at_epoch_ms)",
        [],
    )?;
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

pub(super) fn migrate_v10_search_symbol_projection(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS search_symbol_projection (
            node_id INTEGER PRIMARY KEY,
            display_name TEXT NOT NULL,
            FOREIGN KEY(node_id) REFERENCES node(id)
        )",
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
    if table_columns(conn, table)?
        .iter()
        .any(|existing| existing == column_name)
    {
        return Ok(());
    }

    let sql = format!("ALTER TABLE {table} ADD COLUMN {column_sql}");
    conn.execute(&sql, [])?;
    Ok(())
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, StorageError> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let columns = stmt
        .query_map([], |row| row.get(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}
