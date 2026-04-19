use super::*;
use codestory_contracts::graph::{
    AccessKind, Edge, EdgeId, EdgeKind, NodeId, NodeKind, ResolutionCertainty, SourceLocation,
    TrailConfig, TrailDirection,
};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_db_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "codestory-store-{label}-{}-{stamp}.sqlite",
        std::process::id()
    ))
}

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
fn test_resolution_indexes_are_created() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    let mut node_stmt = storage.conn.prepare("PRAGMA index_list('node')")?;
    let node_indexes = node_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        node_indexes
            .iter()
            .any(|name| name == "idx_node_kind_serialized_name")
    );

    let mut edge_stmt = storage.conn.prepare("PRAGMA index_list('edge')")?;
    let edge_indexes = edge_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        edge_indexes
            .iter()
            .any(|name| name == "idx_edge_kind_resolved_target")
    );

    let mut callable_state_stmt = storage
        .conn
        .prepare("PRAGMA index_list('callable_projection_state')")?;
    let callable_state_indexes = callable_state_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        callable_state_indexes
            .iter()
            .any(|name| name == "idx_callable_projection_state_file_node")
    );

    Ok(())
}

#[test]
fn test_index_artifact_cache_round_trip() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let payload = br#"{"cached":true}"#;

    storage.upsert_index_artifact_cache(Path::new("src/lib.rs"), "cache-key", payload)?;

    assert_eq!(
        storage.get_index_artifact_cache(Path::new("src/lib.rs"), "cache-key")?,
        Some(payload.to_vec())
    );
    assert_eq!(
        storage.get_index_artifact_cache(Path::new("src/lib.rs"), "other-key")?,
        None
    );

    Ok(())
}

#[test]
fn test_resolution_support_snapshot_round_trip_and_invalidation() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let payload = br#"{"support":1}"#;

    assert!(!storage.has_ready_resolution_support_snapshot(1)?);

    storage.put_resolution_support_snapshot(1, payload)?;

    assert!(storage.has_ready_resolution_support_snapshot(1)?);
    assert_eq!(
        storage.get_resolution_support_snapshot(1)?,
        Some(payload.to_vec())
    );

    storage.invalidate_resolution_support_snapshot()?;

    assert!(!storage.has_ready_resolution_support_snapshot(1)?);
    assert_eq!(storage.get_resolution_support_snapshot(1)?, None);

    Ok(())
}

#[test]
fn test_update_file_metadata_preserves_resolution_support_snapshot() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    storage.insert_file(&FileInfo {
        id: 11,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;
    storage.put_resolution_support_snapshot(1, br#"{"hot":true}"#)?;

    storage.update_file_metadata(&FileInfo {
        id: 11,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 2,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;

    assert!(storage.has_ready_resolution_support_snapshot(1)?);
    Ok(())
}

#[test]
fn test_present_kind_queries() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(1),
            kind: NodeKind::CLASS,
            serialized_name: "A".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::METHOD,
            serialized_name: "A::run".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(2),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
    ])?;

    let node_kinds = storage.get_present_node_kinds()?;
    let edge_kinds = storage.get_present_edge_kinds()?;
    assert!(node_kinds.contains(&NodeKind::CLASS));
    assert!(node_kinds.contains(&NodeKind::METHOD));
    assert!(edge_kinds.contains(&EdgeKind::MEMBER));
    assert!(edge_kinds.contains(&EdgeKind::CALL));
    Ok(())
}

#[test]
fn test_component_access_round_trip() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(41),
            kind: NodeKind::METHOD,
            serialized_name: "run".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(42),
            kind: NodeKind::FIELD,
            serialized_name: "state".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_component_access_batch(&[
        (NodeId(41), AccessKind::Protected),
        (NodeId(42), AccessKind::Private),
    ])?;

    assert_eq!(
        storage.get_component_access(NodeId(41))?,
        Some(AccessKind::Protected)
    );
    let map = storage.get_component_access_map_for_nodes(&[NodeId(41), NodeId(42)])?;
    assert_eq!(map.get(&NodeId(42)).copied(), Some(AccessKind::Private));
    Ok(())
}

#[test]
fn test_llm_symbol_doc_round_trip() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "do_work".to_string(),
        ..Default::default()
    }])?;

    storage.upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
        node_id: NodeId(501),
        file_node_id: None,
        kind: NodeKind::FUNCTION,
        display_name: "pkg::do_work".to_string(),
        qualified_name: Some("pkg::do_work".to_string()),
        file_path: Some("src/lib.rs".to_string()),
        start_line: Some(12),
        doc_text: "function pkg::do_work in src/lib.rs line 12".to_string(),
        doc_version: 2,
        doc_hash: "semantic-hash-501".to_string(),
        embedding_model: "local-hash-384".to_string(),
        embedding_dim: 384,
        embedding: vec![0.25_f32; 384],
        updated_at_epoch_ms: 123,
    }])?;

    let docs = storage.get_llm_symbol_docs_by_node_ids(&[NodeId(501)])?;
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].node_id, NodeId(501));
    assert_eq!(docs[0].doc_version, 2);
    assert_eq!(docs[0].doc_hash, "semantic-hash-501");
    assert_eq!(docs[0].embedding_dim, 384);
    assert_eq!(docs[0].embedding.len(), 384);
    Ok(())
}

#[test]
fn test_llm_symbol_doc_copy_forward_preserves_reuse_metadata() -> Result<(), StorageError> {
    let live_path = unique_temp_db_path("llm-copy-source");
    let _ = cleanup_sqlite_sidecars(&live_path);

    {
        let mut live = Storage::open(&live_path)?;
        live.insert_nodes_batch(&[Node {
            id: NodeId(501),
            kind: NodeKind::FUNCTION,
            serialized_name: "do_work".to_string(),
            ..Default::default()
        }])?;
        live.upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
            node_id: NodeId(501),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "pkg::do_work".to_string(),
            qualified_name: Some("pkg::do_work".to_string()),
            file_path: Some("src/lib.rs".to_string()),
            start_line: Some(12),
            doc_text: "function pkg::do_work in src/lib.rs line 12".to_string(),
            doc_version: 2,
            doc_hash: "semantic-hash-501".to_string(),
            embedding_model: "local-hash-384".to_string(),
            embedding_dim: 384,
            embedding: vec![0.25_f32; 384],
            updated_at_epoch_ms: 123,
        }])?;
    }

    let mut staged = Storage::new_in_memory()?;
    staged.insert_nodes_batch(&[Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "do_work".to_string(),
        ..Default::default()
    }])?;

    assert_eq!(staged.copy_llm_symbol_docs_from(&live_path)?, 1);
    let metadata = staged.get_llm_symbol_doc_reuse_metadata()?;
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].node_id, NodeId(501));
    assert_eq!(metadata[0].doc_version, 2);
    assert_eq!(metadata[0].doc_hash, "semantic-hash-501");

    assert_eq!(staged.prune_llm_symbol_docs_to_node_ids(&[NodeId(501)])?, 0);
    assert_eq!(staged.prune_llm_symbol_docs_to_node_ids(&[])?, 1);
    assert!(staged.get_all_llm_symbol_docs()?.is_empty());

    cleanup_sqlite_sidecars(&live_path)?;
    Ok(())
}

#[test]
fn test_search_symbol_projection_round_trip_and_backfill() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(700),
            kind: NodeKind::FUNCTION,
            serialized_name: "short_name".to_string(),
            qualified_name: Some("pkg::short_name".to_string()),
            ..Default::default()
        },
        Node {
            id: NodeId(701),
            kind: NodeKind::METHOD,
            serialized_name: "secondary".to_string(),
            ..Default::default()
        },
    ])?;

    storage.upsert_search_symbol_projection_batch(&[
        SearchSymbolProjection {
            node_id: NodeId(700),
            display_name: "pkg::short_name".to_string(),
        },
        SearchSymbolProjection {
            node_id: NodeId(701),
            display_name: "secondary".to_string(),
        },
    ])?;
    assert_eq!(storage.get_search_symbol_projection_count()?, 2);
    let projection = storage.get_search_symbol_projection_batch_after(None, 10)?;
    assert_eq!(projection.len(), 2);
    assert_eq!(projection[0].display_name, "pkg::short_name");

    storage.clear_search_symbol_projection()?;
    assert_eq!(storage.get_search_symbol_projection_count()?, 0);

    let rebuilt = storage.rebuild_search_symbol_projection_from_node_table()?;
    assert_eq!(rebuilt, 2);
    let projection = storage.get_search_symbol_projection_batch_after(None, 10)?;
    assert_eq!(projection.len(), 2);
    assert_eq!(projection[0].display_name, "pkg::short_name");
    assert_eq!(projection[1].display_name, "secondary");
    Ok(())
}

#[test]
fn test_clear_removes_fk_dependents_and_cache() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_node = Node {
        id: NodeId(500),
        kind: NodeKind::FILE,
        serialized_name: "src/main.rs".to_string(),
        ..Default::default()
    };
    let function_node = Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "main".to_string(),
        file_node_id: Some(file_node.id),
        ..Default::default()
    };

    storage.insert_file(&FileInfo {
        id: file_node.id.0,
        path: PathBuf::from("src/main.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;
    storage.insert_nodes_batch(&[file_node.clone(), function_node.clone()])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(700),
        source: function_node.id,
        target: function_node.id,
        kind: EdgeKind::CALL,
        file_node_id: Some(file_node.id),
        ..Default::default()
    }])?;
    storage.insert_occurrences_batch(&[Occurrence {
        element_id: function_node.id.0,
        kind: codestory_contracts::graph::OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_node.id,
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 4,
        },
    }])?;
    storage.insert_component_access_batch(&[(function_node.id, AccessKind::Public)])?;
    storage.upsert_callable_projection_states(&[CallableProjectionState {
        file_id: file_node.id.0,
        symbol_key: "src/main.rs::main:FUNCTION".to_string(),
        node_id: function_node.id,
        signature_hash: 101,
        body_hash: 202,
        start_line: 1,
        end_line: 1,
    }])?;
    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "test".to_string(),
        file_id: Some(file_node.id),
        line: Some(1),
        column: Some(1),
        is_fatal: false,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
    })?;
    storage.conn.execute(
        "INSERT INTO local_symbol (id, name, file_id) VALUES (?1, ?2, ?3)",
        params![1_i64, "main", file_node.id.0],
    )?;

    let category_id = storage.create_bookmark_category("Favorites")?;
    let _ = storage.add_bookmark(category_id, function_node.id, Some("keep"))?;

    // Ensure cache is warm before clear.
    assert!(storage.get_node(function_node.id)?.is_some());

    storage.clear()?;

    for table in [
        "occurrence",
        "edge",
        "llm_symbol_doc",
        "callable_projection_state",
        "component_access",
        "bookmark_node",
        "local_symbol",
        "error",
        "node",
        "file",
    ] {
        let count: i64 =
            storage
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
        assert_eq!(count, 0, "expected {table} to be empty after clear");
    }

    // Categories are user-managed metadata; clear only removes node-linked data.
    assert_eq!(storage.get_bookmark_categories()?.len(), 1);
    assert!(storage.get_node(function_node.id)?.is_none());
    Ok(())
}

#[test]
fn test_callable_projection_state_round_trip() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_file(&FileInfo {
        id: 11,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 40,
    })?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(11),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
        Node {
            id: NodeId(102),
            kind: NodeKind::FUNCTION,
            serialized_name: "helper".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
    ])?;
    storage.upsert_callable_projection_states(&[
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
            node_id: NodeId(101),
            signature_hash: 111,
            body_hash: 211,
            start_line: 10,
            end_line: 20,
        },
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::helper:FUNCTION".to_string(),
            node_id: NodeId(102),
            signature_hash: 112,
            body_hash: 212,
            start_line: 30,
            end_line: 35,
        },
    ])?;

    let stored = storage.get_callable_projection_states_for_file(11)?;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].symbol_key, "src/lib.rs::run:FUNCTION");

    storage.upsert_callable_projection_states(&[CallableProjectionState {
        file_id: 11,
        symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
        node_id: NodeId(101),
        signature_hash: 111,
        body_hash: 299,
        start_line: 12,
        end_line: 22,
    }])?;
    let updated = storage.get_callable_projection_states_for_file(11)?;
    assert_eq!(updated.len(), 2);
    let run_state = updated
        .iter()
        .find(|state| state.symbol_key == "src/lib.rs::run:FUNCTION")
        .expect("updated run state");
    assert_eq!(run_state.body_hash, 299);
    assert_eq!(run_state.start_line, 12);
    Ok(())
}

#[test]
fn test_delete_callable_projection_states_for_file() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_file(&FileInfo {
        id: 11,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 40,
    })?;
    storage.insert_file(&FileInfo {
        id: 12,
        path: PathBuf::from("src/other.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(11),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(12),
            kind: NodeKind::FILE,
            serialized_name: "src/other.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
        Node {
            id: NodeId(102),
            kind: NodeKind::FUNCTION,
            serialized_name: "helper".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
        Node {
            id: NodeId(201),
            kind: NodeKind::FUNCTION,
            serialized_name: "keep".to_string(),
            file_node_id: Some(NodeId(12)),
            ..Default::default()
        },
    ])?;
    storage.upsert_callable_projection_states(&[
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
            node_id: NodeId(101),
            signature_hash: 111,
            body_hash: 211,
            start_line: 10,
            end_line: 20,
        },
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::helper:FUNCTION".to_string(),
            node_id: NodeId(102),
            signature_hash: 112,
            body_hash: 212,
            start_line: 30,
            end_line: 35,
        },
        CallableProjectionState {
            file_id: 12,
            symbol_key: "src/other.rs::keep:FUNCTION".to_string(),
            node_id: NodeId(201),
            signature_hash: 311,
            body_hash: 411,
            start_line: 1,
            end_line: 5,
        },
    ])?;

    let removed = storage.delete_callable_projection_states_for_file(11)?;
    assert_eq!(removed, 2);
    assert!(
        storage
            .get_callable_projection_states_for_file(11)?
            .is_empty()
    );
    assert_eq!(
        storage.get_callable_projection_states_for_file(12)?.len(),
        1
    );
    Ok(())
}

#[test]
fn test_delete_projection_for_callers_removes_callable_scoped_data() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_id = 9_i64;
    let file_node = Node {
        id: NodeId(file_id),
        kind: NodeKind::FILE,
        serialized_name: "src/lib.rs".to_string(),
        ..Default::default()
    };
    let caller_a = Node {
        id: NodeId(901),
        kind: NodeKind::FUNCTION,
        serialized_name: "run".to_string(),
        file_node_id: Some(file_node.id),
        ..Default::default()
    };
    let caller_b = Node {
        id: NodeId(902),
        kind: NodeKind::FUNCTION,
        serialized_name: "keep".to_string(),
        file_node_id: Some(file_node.id),
        ..Default::default()
    };

    storage.insert_file(&FileInfo {
        id: file_id,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 50,
    })?;
    storage.insert_nodes_batch(&[
        file_node.clone(),
        caller_a.clone(),
        caller_b.clone(),
        Node {
            id: NodeId(903),
            kind: NodeKind::FUNCTION,
            serialized_name: "callee".to_string(),
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: caller_a.id,
            target: NodeId(903),
            kind: EdgeKind::CALL,
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: caller_b.id,
            target: NodeId(903),
            kind: EdgeKind::CALL,
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
        Edge {
            id: EdgeId(3),
            source: caller_a.id,
            target: NodeId(903),
            kind: EdgeKind::USAGE,
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
    ])?;
    storage.insert_occurrences_batch(&[
        Occurrence {
            element_id: caller_a.id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 1,
                start_col: 0,
                end_line: 3,
                end_col: 1,
            },
        },
        Occurrence {
            element_id: caller_b.id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 10,
                start_col: 0,
                end_line: 12,
                end_col: 1,
            },
        },
        Occurrence {
            element_id: NodeId(903).0,
            kind: OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 2,
                start_col: 4,
                end_line: 2,
                end_col: 10,
            },
        },
        Occurrence {
            element_id: NodeId(903).0,
            kind: OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 11,
                start_col: 4,
                end_line: 11,
                end_col: 10,
            },
        },
    ])?;
    storage.upsert_callable_projection_states(&[
        CallableProjectionState {
            file_id,
            symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
            node_id: caller_a.id,
            signature_hash: 111,
            body_hash: 211,
            start_line: 1,
            end_line: 3,
        },
        CallableProjectionState {
            file_id,
            symbol_key: "src/lib.rs::keep:FUNCTION".to_string(),
            node_id: caller_b.id,
            signature_hash: 112,
            body_hash: 212,
            start_line: 10,
            end_line: 12,
        },
    ])?;

    let summary = storage.delete_projection_for_callers(file_id, &[caller_a.id])?;
    assert_eq!(summary.removed_edge_count, 2);
    assert_eq!(summary.removed_occurrence_count, 2);
    assert_eq!(summary.removed_callable_projection_state_count, 1);

    let remaining_edges = storage.get_edges()?;
    assert_eq!(remaining_edges.len(), 1);
    assert_eq!(remaining_edges[0].source, caller_b.id);

    let remaining_occurrences = storage.get_occurrences()?;
    assert_eq!(remaining_occurrences.len(), 2);
    assert!(
        remaining_occurrences
            .iter()
            .any(|occurrence| occurrence.element_id == caller_b.id.0)
    );
    assert!(
        remaining_occurrences
            .iter()
            .any(|occurrence| occurrence.element_id == NodeId(903).0)
    );

    let remaining_states = storage.get_callable_projection_states_for_file(file_id)?;
    assert_eq!(remaining_states.len(), 1);
    assert_eq!(remaining_states[0].node_id, caller_b.id);
    Ok(())
}

#[test]
fn test_opening_v3_db_resets_projection_state() -> Result<(), StorageError> {
    let db_path = std::env::temp_dir().join(format!(
        "codestory-store-v3-migration-{}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&db_path);
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        schema::create_tables(&conn)?;
        schema::create_indexes(&conn, StorageOpenMode::Live)?;
        conn.pragma_update(None, "user_version", 3)?;
        conn.execute(
            "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count)
             VALUES (1, 'src/lib.rs', 'rust', 1, 1, 1, 10)",
            [],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name) VALUES (?1, ?2, ?3)",
            params![1_i64, NodeKind::FILE as i32, "src/lib.rs"],
        )?;
        conn.execute(
            "INSERT INTO callable_projection_state (file_id, symbol_key, node_id, signature_hash, body_hash, start_line, end_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![1_i64, "sym", 1_i64, 11_i64, 22_i64, 1_i64, 2_i64],
        )?;
        conn.execute(
            "INSERT INTO bookmark_category (id, name) VALUES (1, 'Favorites')",
            [],
        )?;
        conn.execute(
            "INSERT INTO bookmark_node (id, category_id, node_id, comment) VALUES (1, 1, 1, 'saved')",
            [],
        )?;
    }

    let storage = Storage::open(&db_path)?;
    assert!(storage.get_files()?.is_empty());
    assert!(storage.get_nodes()?.is_empty());
    assert!(
        storage
            .get_callable_projection_states_for_file(1)?
            .is_empty()
    );
    assert!(storage.get_bookmarks(None)?.is_empty());
    assert!(storage.get_bookmark_categories()?.is_empty());
    drop(storage);
    let _ = std::fs::remove_file(&db_path);
    Ok(())
}

#[test]
fn test_promote_staged_snapshot_replaces_live_db_while_live_reader_is_open()
-> Result<(), StorageError> {
    let live_path = unique_temp_db_path("live");
    let staged_path = unique_temp_db_path("staged");
    let backup_path = live_path.with_extension("sqlite.backup");
    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&staged_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);

    {
        let mut live = Storage::open(&live_path)?;
        live.insert_files_batch(&[FileInfo {
            id: 1,
            path: PathBuf::from("live.rs"),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 10,
        }])?;

        {
            let mut staged = Storage::open_build(&staged_path)?;
            staged.insert_files_batch(&[FileInfo {
                id: 2,
                path: PathBuf::from("staged.rs"),
                language: "rust".to_string(),
                modification_time: 2,
                indexed: true,
                complete: true,
                line_count: 20,
            }])?;
            staged.finalize_staged_snapshot()?;
        }

        Storage::promote_staged_snapshot(&staged_path, &live_path)?;

        let live_reader_files = live.get_files()?;
        assert_eq!(live_reader_files.len(), 1);
    }

    let promoted = Storage::open(&live_path)?;
    let promoted_files = promoted.get_files()?;
    assert_eq!(promoted_files.len(), 1);
    assert_eq!(promoted_files[0].id, 2);
    assert_eq!(promoted_files[0].path, PathBuf::from("staged.rs"));
    drop(promoted);

    assert!(!staged_path.exists());
    assert!(!PathBuf::from(format!("{}-wal", staged_path.display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", staged_path.display())).exists());

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&staged_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);
    Ok(())
}

#[test]
fn test_resolution_query_plan_prefers_new_indexes() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    let mut node_plan_stmt = storage.conn.prepare(
            "EXPLAIN QUERY PLAN SELECT id FROM node WHERE kind IN (3, 11, 12) AND serialized_name = 'foo' LIMIT 1",
        )?;
    let node_plan = node_plan_stmt
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        node_plan
            .iter()
            .any(|line| line.contains("idx_node_kind_serialized_name"))
    );

    let mut edge_plan_stmt = storage.conn.prepare(
            "EXPLAIN QUERY PLAN SELECT COUNT(*) FROM edge WHERE kind = 3 AND resolved_target_node_id IS NULL",
        )?;
    let edge_plan = edge_plan_stmt
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        edge_plan
            .iter()
            .any(|line| line.contains("idx_edge_kind_resolved_target"))
    );

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
    let error = codestory_contracts::graph::ErrorInfo {
        message: "Syntax error".to_string(),
        file_id: Some(NodeId(1)),
        line: Some(10),
        column: Some(5),
        is_fatal: true,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
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
fn test_delete_file_projection() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_node_id = 1_234_i64;
    let file_node = Node {
        id: NodeId(file_node_id),
        kind: NodeKind::FILE,
        serialized_name: "src/main.rs".to_string(),
        start_line: Some(1),
        start_col: Some(1),
        end_line: Some(3),
        end_col: Some(1),
        ..Default::default()
    };
    let func_node = Node {
        id: NodeId(2_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "foo".to_string(),
        file_node_id: Some(NodeId(file_node_id)),
        start_line: Some(1),
        start_col: Some(1),
        end_line: Some(1),
        end_col: Some(20),
        ..Default::default()
    };
    storage.insert_file(&FileInfo {
        id: file_node_id,
        path: PathBuf::from("src/main.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;
    storage.insert_nodes_batch(&[file_node.clone(), func_node.clone()])?;

    storage.insert_edges_batch(&[Edge {
        id: EdgeId(9_001),
        source: file_node.id,
        target: func_node.id,
        kind: EdgeKind::MEMBER,
        file_node_id: Some(file_node.id),
        ..Default::default()
    }])?;

    storage.insert_occurrences_batch(&[Occurrence {
        element_id: func_node.id.0,
        kind: codestory_contracts::graph::OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_node.id,
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 3,
        },
    }])?;

    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "test".to_string(),
        file_id: Some(file_node.id),
        line: Some(1),
        column: None,
        is_fatal: false,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
    })?;
    storage.upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
        node_id: func_node.id,
        file_node_id: Some(file_node.id),
        kind: NodeKind::FUNCTION,
        display_name: "foo".to_string(),
        qualified_name: None,
        file_path: Some("src/main.rs".to_string()),
        start_line: Some(1),
        doc_text: "foo symbol".to_string(),
        doc_version: 2,
        doc_hash: "semantic-hash-foo".to_string(),
        embedding_model: "local-hash-384".to_string(),
        embedding_dim: 384,
        embedding: vec![0.1_f32; 384],
        updated_at_epoch_ms: 1,
    }])?;
    storage.upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
        node_id: func_node.id,
        display_name: "foo".to_string(),
    }])?;
    storage.upsert_callable_projection_states(&[CallableProjectionState {
        file_id: file_node_id,
        symbol_key: "src/main.rs::foo:FUNCTION".to_string(),
        node_id: func_node.id,
        signature_hash: 111,
        body_hash: 211,
        start_line: 1,
        end_line: 1,
    }])?;

    let category_id = storage.create_bookmark_category("Cat")?;
    let _ = storage.add_bookmark(category_id, func_node.id, Some("test"))?;

    let summary = storage.delete_file_projection(file_node_id)?;
    assert_eq!(summary.canonical_file_node_id, file_node_id);
    assert_eq!(summary.removed_node_count, 2);
    assert_eq!(summary.removed_edge_count, 1);
    assert_eq!(summary.removed_occurrence_count, 1);
    assert_eq!(summary.removed_error_count, 1);
    assert_eq!(summary.removed_file_row_count, 1);
    assert_eq!(summary.removed_callable_projection_state_count, 1);

    assert!(storage.get_nodes()?.is_empty());
    assert!(storage.get_edges()?.is_empty());
    assert!(storage.get_occurrences()?.is_empty());
    assert!(storage.get_all_llm_symbol_docs()?.is_empty());
    assert_eq!(storage.get_search_symbol_projection_count()?, 0);
    assert!(
        storage
            .get_callable_projection_states_for_file(file_node_id)?
            .is_empty()
    );
    assert!(storage.get_errors(None)?.is_empty());
    assert!(storage.get_bookmarks(Some(category_id))?.is_empty());

    let cache = storage.cache.nodes.read();
    assert!(!cache.contains_key(&NodeId(file_node_id)));
    assert!(!cache.contains_key(&NodeId(2_001)));

    Ok(())
}

#[test]
fn test_delete_file_projection_preserves_cross_file_edges_and_clears_resolution()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_a_id = 1_001_i64;
    let file_b_id = 2_001_i64;

    storage.insert_file(&FileInfo {
        id: file_a_id,
        path: PathBuf::from("src/a.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;
    storage.insert_file(&FileInfo {
        id: file_b_id,
        path: PathBuf::from("src/b.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;

    let file_a = Node {
        id: NodeId(file_a_id),
        kind: NodeKind::FILE,
        serialized_name: "src/a.rs".to_string(),
        ..Default::default()
    };
    let file_b = Node {
        id: NodeId(file_b_id),
        kind: NodeKind::FILE,
        serialized_name: "src/b.rs".to_string(),
        ..Default::default()
    };
    let caller_in_a = Node {
        id: NodeId(10_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "caller".to_string(),
        file_node_id: Some(file_a.id),
        ..Default::default()
    };
    let unresolved_in_a = Node {
        id: NodeId(10_002),
        kind: NodeKind::FUNCTION,
        serialized_name: "callee".to_string(),
        file_node_id: Some(file_a.id),
        ..Default::default()
    };
    let callee_in_b = Node {
        id: NodeId(20_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "callee".to_string(),
        file_node_id: Some(file_b.id),
        ..Default::default()
    };
    storage.insert_nodes_batch(&[
        file_a.clone(),
        file_b.clone(),
        caller_in_a.clone(),
        unresolved_in_a.clone(),
        callee_in_b.clone(),
    ])?;

    storage.insert_edges_batch(&[Edge {
        id: EdgeId(30_001),
        source: caller_in_a.id,
        target: unresolved_in_a.id,
        kind: EdgeKind::CALL,
        file_node_id: Some(file_a.id),
        resolved_target: Some(callee_in_b.id),
        confidence: Some(0.91),
        certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
        candidate_targets: vec![callee_in_b.id],
        ..Default::default()
    }])?;

    let summary = storage.delete_file_projection(file_b_id)?;
    assert_eq!(summary.canonical_file_node_id, file_b_id);
    assert_eq!(summary.removed_node_count, 2);
    assert_eq!(summary.removed_edge_count, 0);

    let edges = storage.get_edges()?;
    assert_eq!(edges.len(), 1);
    let edge = &edges[0];
    assert_eq!(edge.source, caller_in_a.id);
    assert_eq!(edge.target, unresolved_in_a.id);
    assert_eq!(edge.file_node_id, Some(file_a.id));
    assert_eq!(edge.resolved_target, None);
    assert_eq!(edge.confidence, None);
    assert_eq!(edge.certainty, None);
    assert!(edge.candidate_targets.is_empty());

    assert!(storage.get_node(file_b.id)?.is_none());
    assert!(storage.get_node(callee_in_b.id)?.is_none());
    assert!(storage.get_node(caller_in_a.id)?.is_some());

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
fn test_update_bookmark_tri_state_comment_patch() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    let category_id = storage.create_bookmark_category("General")?;
    storage.insert_node(&Node {
        id: NodeId(300),
        kind: NodeKind::FUNCTION,
        serialized_name: "tri_state_target".to_string(),
        ..Default::default()
    })?;
    let bookmark_id = storage.add_bookmark(category_id, NodeId(300), Some("initial"))?;

    // Omitted comment keeps existing value.
    storage.update_bookmark(bookmark_id, None, None)?;
    let mut bookmarks = storage.get_bookmarks(Some(category_id))?;
    assert_eq!(bookmarks.remove(0).comment.as_deref(), Some("initial"));

    // Explicit null clears the comment.
    storage.update_bookmark(bookmark_id, None, Some(None))?;
    let mut bookmarks = storage.get_bookmarks(Some(category_id))?;
    assert_eq!(bookmarks.remove(0).comment, None);

    // Explicit value sets the comment.
    storage.update_bookmark(bookmark_id, None, Some(Some("updated")))?;
    let mut bookmarks = storage.get_bookmarks(Some(category_id))?;
    assert_eq!(bookmarks.remove(0).comment.as_deref(), Some("updated"));

    Ok(())
}

#[test]
fn test_get_errors() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    // Insert errors
    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "Fatal error".to_string(),
        file_id: None,
        line: Some(10),
        column: None,
        is_fatal: true,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
    })?;
    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "Warning".to_string(),
        file_id: None,
        line: Some(20),
        column: None,
        is_fatal: false,
        index_step: codestory_contracts::graph::IndexStep::Collection,
    })?;

    // Get all errors
    let errors = storage.get_errors(None)?;
    assert_eq!(errors.len(), 2);

    // Get fatal errors only
    let filter = codestory_contracts::graph::ErrorFilter {
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
            id: codestory_contracts::graph::EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
        Edge {
            id: codestory_contracts::graph::EdgeId(2),
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
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
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
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
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
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
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
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
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
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;

    assert!(result.edges.is_empty());
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].id, resolved.id);

    Ok(())
}

#[test]
fn test_trail_production_scope_excludes_test_callers() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let file_prod = Node {
        id: NodeId(100),
        kind: NodeKind::FILE,
        serialized_name: "src/lib.rs".to_string(),
        ..Default::default()
    };
    let file_test = Node {
        id: NodeId(101),
        kind: NodeKind::FILE,
        serialized_name: "tests/integration.rs".to_string(),
        ..Default::default()
    };
    let prod_target = Node {
        id: NodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "target".to_string(),
        file_node_id: Some(file_prod.id),
        ..Default::default()
    };
    let test_caller = Node {
        id: NodeId(2),
        kind: NodeKind::FUNCTION,
        serialized_name: "test_caller".to_string(),
        file_node_id: Some(file_test.id),
        ..Default::default()
    };
    let unresolved_target = Node {
        id: NodeId(3),
        kind: NodeKind::UNKNOWN,
        serialized_name: "target".to_string(),
        file_node_id: Some(file_test.id),
        ..Default::default()
    };

    storage.insert_nodes_batch(&[
        file_prod,
        file_test,
        prod_target,
        test_caller,
        unresolved_target,
    ])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(1),
        source: NodeId(2),
        target: NodeId(3),
        kind: EdgeKind::CALL,
        resolved_target: Some(NodeId(1)),
        file_node_id: Some(NodeId(101)),
        ..Default::default()
    }])?;

    let production_only = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Incoming,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert!(production_only.edges.is_empty());

    let include_tests = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Incoming,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert_eq!(include_tests.edges.len(), 1);

    Ok(())
}

#[test]
fn test_trail_can_hide_utility_calls() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let caller = Node {
        id: NodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "caller".to_string(),
        ..Default::default()
    };
    let utility_symbol = Node {
        id: NodeId(2),
        kind: NodeKind::UNKNOWN,
        serialized_name: "len".to_string(),
        ..Default::default()
    };

    storage.insert_nodes_batch(&[caller, utility_symbol])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(10),
        source: NodeId(1),
        target: NodeId(2),
        kind: EdgeKind::CALL,
        ..Default::default()
    }])?;

    let hidden = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: false,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert!(hidden.edges.is_empty());

    let shown = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert_eq!(shown.edges.len(), 1);

    Ok(())
}

#[test]
fn test_helper_calls_are_not_suppressed_as_ambiguous() {
    assert!(!should_ignore_call_resolution(
        "Self::flush_projection_batch",
        Some(ResolutionCertainty::Uncertain),
        Some(0.40)
    ));
    assert!(!should_ignore_call_resolution(
        "WorkspaceIndexer::seed_symbol_table",
        Some(ResolutionCertainty::Probable),
        Some(0.70)
    ));
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
        id: codestory_contracts::graph::EdgeId(1),
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

#[test]
fn test_grounding_queries_rank_symbols_and_roots() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    storage.insert_file(&FileInfo {
        id: 100,
        path: PathBuf::from("src/a.rs"),
        language: "rust".to_string(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;
    storage.insert_file(&FileInfo {
        id: 200,
        path: PathBuf::from("src/b.rs"),
        language: "rust".to_string(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 10,
    })?;
    storage.insert_nodes_batch(&[
        codestory_contracts::graph::Node {
            id: NodeId(100),
            kind: NodeKind::FILE,
            serialized_name: "src/a.rs".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(200),
            kind: NodeKind::FILE,
            serialized_name: "src/b.rs".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "zeta".to_string(),
            file_node_id: Some(NodeId(100)),
            start_line: Some(8),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(102),
            kind: NodeKind::STRUCT,
            serialized_name: "Alpha".to_string(),
            file_node_id: Some(NodeId(100)),
            start_line: Some(2),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(201),
            kind: NodeKind::MODULE,
            serialized_name: "\"./types\"".to_string(),
            file_node_id: Some(NodeId(200)),
            start_line: Some(1),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(202),
            kind: NodeKind::CLASS,
            serialized_name: "Widget".to_string(),
            file_node_id: Some(NodeId(200)),
            start_line: Some(2),
            ..Default::default()
        },
    ])?;

    let summaries = storage.get_grounding_file_summaries()?;
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].file.id, 100);
    assert_eq!(summaries[0].symbol_count, 2);
    assert_eq!(summaries[0].best_node_rank, 0);

    let top = storage.get_grounding_top_symbols_for_files(&[100, 200], 1)?;
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].node.id, NodeId(102));
    assert_eq!(top[1].node.id, NodeId(202));

    let roots = storage.get_grounding_root_symbol_candidates(2, 0)?;
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].node.id, NodeId(102));
    assert_eq!(roots[1].node.id, NodeId(202));

    Ok(())
}

#[test]
fn test_grounding_member_counts_and_occurrence_lines() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    storage.insert_nodes_batch(&[
        codestory_contracts::graph::Node {
            id: NodeId(1),
            kind: NodeKind::STRUCT,
            serialized_name: "Widget".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(2),
            kind: NodeKind::FIELD,
            serialized_name: "title".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(3),
            kind: NodeKind::FIELD,
            serialized_name: "count".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(10),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(11),
            kind: NodeKind::FUNCTION,
            serialized_name: "render".to_string(),
            file_node_id: Some(NodeId(10)),
            start_line: None,
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(1),
            target: NodeId(3),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
    ])?;
    storage.insert_occurrences_batch(&[
        codestory_contracts::graph::Occurrence {
            element_id: 11,
            kind: codestory_contracts::graph::OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: NodeId(10),
                start_line: 20,
                start_col: 1,
                end_line: 20,
                end_col: 5,
            },
        },
        codestory_contracts::graph::Occurrence {
            element_id: 11,
            kind: codestory_contracts::graph::OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: NodeId(10),
                start_line: 5,
                start_col: 1,
                end_line: 5,
                end_col: 5,
            },
        },
    ])?;

    let member_counts = storage.get_grounding_member_counts(&[NodeId(1)])?;
    assert_eq!(member_counts.get(&NodeId(1)), Some(&2));

    let fallback_lines = storage.get_grounding_min_occurrence_lines(&[NodeId(11)])?;
    assert_eq!(fallback_lines.get(&NodeId(11)), Some(&20));

    Ok(())
}

#[test]
fn test_grounding_edge_digests_ignore_ambiguous_resolved_targets() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    storage.insert_nodes_batch(&[
        codestory_contracts::graph::Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(2),
            kind: NodeKind::UNKNOWN,
            serialized_name: "len".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "Vec::len".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(10),
        source: NodeId(1),
        target: NodeId(2),
        kind: EdgeKind::CALL,
        resolved_target: Some(NodeId(3)),
        certainty: Some(ResolutionCertainty::Uncertain),
        ..Default::default()
    }])?;

    let counts = storage.get_grounding_edge_digest_counts(&[NodeId(1), NodeId(2), NodeId(3)])?;
    assert!(counts.iter().any(|entry| {
        entry.node_id == NodeId(1) && entry.kind == EdgeKind::CALL && entry.count == 1
    }));
    assert!(counts.iter().any(|entry| {
        entry.node_id == NodeId(2) && entry.kind == EdgeKind::CALL && entry.count == 1
    }));
    assert!(!counts.iter().any(|entry| entry.node_id == NodeId(3)));

    Ok(())
}
