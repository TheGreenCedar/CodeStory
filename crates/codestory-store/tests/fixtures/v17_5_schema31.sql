-- Extracted from v0.17.5 (be4174a24ba93118e72a5f9eb2fd18f87b81ba7b)
-- crates/codestory-store/src/storage_impl/schema.rs.
-- Exact schema-31 table and live-index DDL for upgrade regression fixtures.
CREATE TABLE IF NOT EXISTS node (
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
    );
CREATE TABLE IF NOT EXISTS edge (
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
    );
CREATE TABLE IF NOT EXISTS occurrence (
         element_id INTEGER NOT NULL,
         kind INTEGER NOT NULL,
         file_node_id INTEGER NOT NULL,
         start_line INTEGER NOT NULL,
         start_col INTEGER NOT NULL,
         end_line INTEGER NOT NULL,
         end_col INTEGER NOT NULL
    );
CREATE TABLE IF NOT EXISTS file (
        id INTEGER PRIMARY KEY,
        path TEXT UNIQUE NOT NULL,
        language TEXT,
        modification_time INTEGER,
        indexed INTEGER DEFAULT 0,
        complete INTEGER DEFAULT 0,
        line_count INTEGER DEFAULT 0,
        file_role TEXT NOT NULL DEFAULT 'source',
        content_hash TEXT
    );
CREATE TABLE IF NOT EXISTS incomplete_index_run (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        started_at_epoch_ms INTEGER NOT NULL
    );
CREATE TABLE IF NOT EXISTS index_publication (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        generation INTEGER NOT NULL CHECK (generation > 0),
        generation_id TEXT NOT NULL UNIQUE CHECK (length(generation_id) > 0),
        run_id TEXT NOT NULL CHECK (length(run_id) > 0),
        mode TEXT NOT NULL CHECK (mode IN ('full', 'incremental', 'semantic_projection')),
        published_at_epoch_ms INTEGER NOT NULL CHECK (published_at_epoch_ms >= 0)
    );
CREATE TABLE IF NOT EXISTS structural_text_unit (
        node_id INTEGER PRIMARY KEY,
        file_id INTEGER NOT NULL,
        placement_id TEXT NOT NULL UNIQUE CHECK(length(placement_id) = 64),
        content_hash TEXT NOT NULL CHECK(length(content_hash) = 64),
        source_content_hash TEXT NOT NULL CHECK(length(source_content_hash) = 64),
        descriptor_version INTEGER NOT NULL CHECK(descriptor_version > 0),
        producer TEXT NOT NULL CHECK(length(producer) > 0),
        evidence_tier TEXT NOT NULL CHECK(evidence_tier = 'structural_text'),
        resolution TEXT NOT NULL CHECK(resolution = 'source_range_only'),
        language TEXT NOT NULL CHECK(length(language) > 0),
        kind INTEGER NOT NULL,
        start_line INTEGER NOT NULL CHECK(start_line > 0),
        start_col INTEGER NOT NULL CHECK(start_col > 0),
        end_line INTEGER NOT NULL CHECK(end_line > 0),
        end_col INTEGER NOT NULL CHECK(end_col > 0),
        file_role TEXT NOT NULL,
        FOREIGN KEY(node_id) REFERENCES node(id),
        FOREIGN KEY(file_id) REFERENCES file(id)
    );
CREATE TABLE IF NOT EXISTS structural_text_projection (
        file_id INTEGER PRIMARY KEY,
        source_content_hash TEXT NOT NULL CHECK(length(source_content_hash) = 64),
        descriptor_version INTEGER NOT NULL CHECK(descriptor_version > 0),
        producer TEXT NOT NULL CHECK(length(producer) > 0),
        language TEXT NOT NULL CHECK(length(language) > 0),
        file_role TEXT NOT NULL,
        unit_count INTEGER NOT NULL CHECK(unit_count >= 0),
        unit_digest TEXT NOT NULL CHECK(length(unit_digest) = 64),
        FOREIGN KEY(file_id) REFERENCES file(id)
    );
CREATE TABLE IF NOT EXISTS structural_text_artifact_cache (
        file_path TEXT PRIMARY KEY,
        file_id INTEGER NOT NULL UNIQUE,
        cache_key TEXT NOT NULL,
        source_content_hash TEXT NOT NULL CHECK(length(source_content_hash) = 64),
        descriptor_version INTEGER NOT NULL CHECK(descriptor_version > 0),
        producer TEXT NOT NULL CHECK(length(producer) > 0),
        artifact_digest TEXT NOT NULL CHECK(length(artifact_digest) = 64),
        artifact_blob BLOB NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL
    );
CREATE TABLE IF NOT EXISTS structural_text_unit_publication (
        id INTEGER PRIMARY KEY CHECK(id = 1),
        schema_version INTEGER NOT NULL,
        complete INTEGER NOT NULL CHECK(complete = 1),
        core_generation_id TEXT NOT NULL CHECK(length(core_generation_id) > 0),
        core_run_id TEXT NOT NULL CHECK(length(core_run_id) > 0),
        unit_count INTEGER NOT NULL CHECK(unit_count >= 0),
        unit_digest TEXT NOT NULL CHECK(length(unit_digest) = 64),
        projection_count INTEGER NOT NULL CHECK(projection_count >= 0),
        projection_digest TEXT NOT NULL CHECK(length(projection_digest) = 64),
        descriptor_version INTEGER NOT NULL CHECK(descriptor_version > 0),
        migration_state TEXT NOT NULL CHECK(length(migration_state) > 0),
        published_at_epoch_ms INTEGER NOT NULL CHECK(published_at_epoch_ms >= 0)
    );
CREATE TABLE IF NOT EXISTS local_symbol (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        file_id INTEGER,
        FOREIGN KEY(file_id) REFERENCES file(id)
    );
CREATE TABLE IF NOT EXISTS component_access (
        node_id INTEGER,
        type INTEGER,
        FOREIGN KEY(node_id) REFERENCES node(id)
    );
CREATE TABLE IF NOT EXISTS error (
        id INTEGER PRIMARY KEY,
        message TEXT NOT NULL,
        file_id INTEGER,
        line INTEGER,
        column INTEGER,
        fatal INTEGER DEFAULT 0,
        indexed INTEGER DEFAULT 0,
        coverage_reason TEXT,
        FOREIGN KEY(file_id) REFERENCES file(id)
    );
CREATE TABLE IF NOT EXISTS bookmark_category (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL
    );
CREATE TABLE IF NOT EXISTS bookmark_node (
        id INTEGER PRIMARY KEY,
        category_id INTEGER,
        node_id INTEGER,
        comment TEXT,
        FOREIGN KEY(category_id) REFERENCES bookmark_category(id),
        FOREIGN KEY(node_id) REFERENCES node(id)
    );
CREATE TABLE IF NOT EXISTS llm_symbol_doc (
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
    );
CREATE TABLE IF NOT EXISTS dense_anchor_input (
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
    );
CREATE TABLE IF NOT EXISTS dense_anchor_publication (
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
    );
CREATE TABLE IF NOT EXISTS source_policy_exclusion (
        normalized_path TEXT PRIMARY KEY CHECK(length(normalized_path) > 0),
        project_id TEXT NOT NULL CHECK(length(project_id) > 0),
        workspace_id TEXT NOT NULL CHECK(length(workspace_id) > 0),
        content_hash TEXT NOT NULL CHECK(length(content_hash) = 64),
        observed_size INTEGER NOT NULL CHECK(observed_size > 0),
        observed_unit_count INTEGER NOT NULL DEFAULT 0 CHECK(observed_unit_count >= 0),
        policy_version TEXT NOT NULL CHECK(length(policy_version) > 0),
        byte_cap INTEGER NOT NULL CHECK(byte_cap > 0),
        structural_unit_cap INTEGER NOT NULL DEFAULT 2048 CHECK(structural_unit_cap > 0),
        core_generation_id TEXT NOT NULL CHECK(length(core_generation_id) > 0),
        core_run_id TEXT NOT NULL CHECK(length(core_run_id) > 0)
    );
CREATE TABLE IF NOT EXISTS source_policy_exclusion_publication (
        id INTEGER PRIMARY KEY CHECK(id = 1),
        schema_version INTEGER NOT NULL,
        complete INTEGER NOT NULL CHECK(complete = 1),
        project_id TEXT NOT NULL CHECK(length(project_id) > 0),
        workspace_id TEXT NOT NULL CHECK(length(workspace_id) > 0),
        core_generation_id TEXT NOT NULL CHECK(length(core_generation_id) > 0),
        core_run_id TEXT NOT NULL CHECK(length(core_run_id) > 0),
        exclusion_count INTEGER NOT NULL CHECK(exclusion_count >= 0),
        exclusion_digest TEXT NOT NULL CHECK(length(exclusion_digest) = 64),
        policy_version TEXT NOT NULL CHECK(length(policy_version) > 0),
        byte_cap INTEGER NOT NULL CHECK(byte_cap > 0),
        structural_unit_cap INTEGER NOT NULL DEFAULT 2048 CHECK(structural_unit_cap > 0),
        published_at_epoch_ms INTEGER NOT NULL CHECK(published_at_epoch_ms >= 0)
    );
CREATE TABLE IF NOT EXISTS symbol_search_doc (
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
    );
CREATE TABLE IF NOT EXISTS symbol_summary (
        node_id INTEGER NOT NULL,
        content_hash TEXT NOT NULL,
        summary TEXT NOT NULL,
        model TEXT NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL,
        PRIMARY KEY(node_id, content_hash),
        FOREIGN KEY(node_id) REFERENCES node(id)
    );
CREATE TABLE IF NOT EXISTS search_symbol_projection (
        node_id INTEGER PRIMARY KEY,
        display_name TEXT NOT NULL,
        FOREIGN KEY(node_id) REFERENCES node(id)
    );
CREATE TABLE IF NOT EXISTS callable_projection_state (
        file_id INTEGER NOT NULL,
        symbol_key TEXT NOT NULL,
        node_id INTEGER NOT NULL,
        signature_hash INTEGER NOT NULL,
        normalized_signature TEXT,
        body_hash INTEGER NOT NULL,
        start_line INTEGER NOT NULL,
        end_line INTEGER NOT NULL,
        PRIMARY KEY (file_id, symbol_key),
        FOREIGN KEY(file_id) REFERENCES file(id),
        FOREIGN KEY(node_id) REFERENCES node(id)
    );
CREATE TABLE IF NOT EXISTS grounding_snapshot_meta (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        snapshot_version INTEGER NOT NULL,
        summary_state INTEGER NOT NULL,
        detail_state INTEGER NOT NULL,
        summary_built_at_epoch_ms INTEGER,
        detail_built_at_epoch_ms INTEGER
    );
CREATE TABLE IF NOT EXISTS grounding_repo_stats_snapshot (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        node_count INTEGER NOT NULL,
        edge_count INTEGER NOT NULL,
        file_count INTEGER NOT NULL,
        error_count INTEGER NOT NULL
    );
CREATE TABLE IF NOT EXISTS grounding_file_snapshot (
        file_id INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        language TEXT NOT NULL,
        modification_time INTEGER NOT NULL,
        indexed INTEGER NOT NULL,
        complete INTEGER NOT NULL,
        line_count INTEGER NOT NULL,
        symbol_count INTEGER NOT NULL,
        best_node_rank INTEGER NOT NULL
    );
CREATE TABLE IF NOT EXISTS grounding_node_snapshot (
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
    );
CREATE TABLE IF NOT EXISTS grounding_node_summary_snapshot (
        node_id INTEGER PRIMARY KEY,
        member_count INTEGER NOT NULL,
        fallback_occurrence_line INTEGER
    );
CREATE TABLE IF NOT EXISTS grounding_node_edge_digest_snapshot (
        node_id INTEGER NOT NULL,
        kind INTEGER NOT NULL,
        count INTEGER NOT NULL,
        PRIMARY KEY (node_id, kind)
    );
CREATE TABLE IF NOT EXISTS index_artifact_cache (
        file_path TEXT PRIMARY KEY,
        cache_key TEXT NOT NULL,
        artifact_blob BLOB NOT NULL,
        updated_at_epoch_ms INTEGER NOT NULL
    );
CREATE TABLE IF NOT EXISTS resolution_support_snapshot (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        snapshot_version INTEGER NOT NULL,
        state INTEGER NOT NULL,
        snapshot_blob BLOB,
        built_at_epoch_ms INTEGER
    );
INSERT OR IGNORE INTO resolution_support_snapshot (
        id,
        snapshot_version,
        state,
        snapshot_blob,
        built_at_epoch_ms
    ) VALUES (1, 0, 0, NULL, NULL);
CREATE TABLE IF NOT EXISTS retrieval_index_manifest (
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
    );
CREATE TABLE IF NOT EXISTS annotation_sidecar_cutover (
        id INTEGER PRIMARY KEY CHECK(id = 1),
        sidecar_schema_version INTEGER NOT NULL CHECK(sidecar_schema_version > 0),
        cutover_at_epoch_ms INTEGER NOT NULL CHECK(cutover_at_epoch_ms >= 0)
    );
CREATE UNIQUE INDEX IF NOT EXISTS idx_occurrence_unique
     ON occurrence(element_id, file_node_id, start_line, start_col, end_line, end_col);
CREATE UNIQUE INDEX IF NOT EXISTS idx_component_access_node ON component_access(node_id);
CREATE INDEX IF NOT EXISTS idx_error_file ON error(file_id);
CREATE INDEX IF NOT EXISTS idx_edge_source ON edge(source_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_target ON edge(target_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_resolved_source ON edge(resolved_source_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_resolved_target ON edge(resolved_target_node_id);
CREATE INDEX IF NOT EXISTS idx_occurrence_element ON occurrence(element_id);
CREATE INDEX IF NOT EXISTS idx_occurrence_element_start_line ON occurrence(element_id, start_line);
CREATE INDEX IF NOT EXISTS idx_occurrence_file ON occurrence(file_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_file ON edge(file_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_scope_unresolved
     ON edge(kind, resolved_target_node_id, source_node_id, file_node_id);
CREATE INDEX IF NOT EXISTS idx_node_file ON node(file_node_id);
CREATE INDEX IF NOT EXISTS idx_node_file_kind_line ON node(file_node_id, kind, start_line);
CREATE INDEX IF NOT EXISTS idx_node_file_kind_name ON node(file_node_id, kind, qualified_name, serialized_name);
CREATE INDEX IF NOT EXISTS idx_node_canonical_id ON node(canonical_id);
CREATE INDEX IF NOT EXISTS idx_node_qualified_name ON node(qualified_name);
CREATE INDEX IF NOT EXISTS idx_bookmark_node_category ON bookmark_node(category_id);
CREATE INDEX IF NOT EXISTS idx_bookmark_node_node ON bookmark_node(node_id);
CREATE INDEX IF NOT EXISTS idx_node_kind_serialized_name ON node(kind, serialized_name);
CREATE INDEX IF NOT EXISTS idx_edge_kind_source ON edge(kind, source_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_kind_target ON edge(kind, target_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_kind_resolved_target ON edge(kind, resolved_target_node_id);
CREATE INDEX IF NOT EXISTS idx_edge_line ON edge(line);
CREATE INDEX IF NOT EXISTS idx_edge_callsite_identity ON edge(callsite_identity);
CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_file_node ON llm_symbol_doc(file_node_id);
CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_kind ON llm_symbol_doc(kind);
CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_updated_at ON llm_symbol_doc(updated_at_epoch_ms);
CREATE INDEX IF NOT EXISTS idx_llm_symbol_doc_policy_reason
     ON llm_symbol_doc(semantic_policy_version, dense_reason);
CREATE INDEX IF NOT EXISTS idx_dense_anchor_input_file_node
     ON dense_anchor_input(file_node_id);
CREATE INDEX IF NOT EXISTS idx_dense_anchor_input_reuse
     ON dense_anchor_input(policy_version, document_hash);
CREATE INDEX IF NOT EXISTS idx_dense_anchor_input_source
     ON dense_anchor_input(source_identity);
CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_file_node ON symbol_search_doc(file_node_id);
CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_kind ON symbol_search_doc(kind);
CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_policy ON symbol_search_doc(policy_version);
CREATE INDEX IF NOT EXISTS idx_symbol_search_doc_hash ON symbol_search_doc(doc_version, doc_hash);
CREATE INDEX IF NOT EXISTS idx_search_symbol_projection_display_name
     ON search_symbol_projection(display_name);
CREATE INDEX IF NOT EXISTS idx_callable_projection_state_node_id ON callable_projection_state(node_id);
CREATE INDEX IF NOT EXISTS idx_callable_projection_state_normalized_signature
        ON callable_projection_state(normalized_signature);
CREATE INDEX IF NOT EXISTS idx_callable_projection_state_file_node ON callable_projection_state(file_id, node_id);
CREATE INDEX IF NOT EXISTS idx_index_artifact_cache_key
     ON index_artifact_cache(cache_key);
CREATE INDEX IF NOT EXISTS idx_structural_text_unit_file
     ON structural_text_unit(file_id);
CREATE INDEX IF NOT EXISTS idx_structural_text_unit_content
     ON structural_text_unit(content_hash);
CREATE INDEX IF NOT EXISTS idx_structural_text_artifact_cache_key
     ON structural_text_artifact_cache(cache_key);
CREATE INDEX IF NOT EXISTS idx_structural_text_artifact_cache_file
     ON structural_text_artifact_cache(file_id);
CREATE INDEX IF NOT EXISTS idx_retrieval_index_manifest_built_at
     ON retrieval_index_manifest(built_at_epoch_ms);
CREATE INDEX IF NOT EXISTS idx_grounding_file_snapshot_path ON grounding_file_snapshot(path);
CREATE INDEX IF NOT EXISTS idx_grounding_file_snapshot_rank
     ON grounding_file_snapshot(best_node_rank, symbol_count DESC, path);
CREATE INDEX IF NOT EXISTS idx_grounding_node_snapshot_file_rank
     ON grounding_node_snapshot(file_node_id, file_symbol_rank, node_id);
CREATE INDEX IF NOT EXISTS idx_grounding_node_snapshot_root_rank
     ON grounding_node_snapshot(is_root, node_rank, sort_start_line, display_name, node_id);
PRAGMA user_version = 31;
