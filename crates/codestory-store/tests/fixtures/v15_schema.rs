//! Verbatim `TABLE_STATEMENTS` from v0.15.0 (`git show v0.15.0:crates/codestory-store/src/storage_impl/schema.rs`).
//!
//! Frozen on purpose: this is the shape a 0.15 user's database actually has on disk, and the
//! migration path from it must keep working. Do not regenerate from current schema.rs.

pub(crate) const V15_TABLE_STATEMENTS: &[&str] = &[
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
        qdrant_collection TEXT NOT NULL,
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
        precise_semantic_import_producer TEXT
    )",
];
