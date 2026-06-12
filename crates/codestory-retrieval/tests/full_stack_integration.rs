//! Phase 2 exit: fixture index → `full` mode → SQLite-resolvable hits.

use codestory_contracts::graph::{Node, NodeId, NodeKind};
use codestory_retrieval::{
    QdrantClient, QueryRequest, SidecarLayout, execute_retrieval_query, finalize_index,
    probe_sidecar_health, project_id_for_root, qdrant_enabled,
};
use codestory_store::{FileInfo, FileRole, SearchSymbolProjection, Store};
use std::path::Path;
use tempfile::TempDir;

fn seed_fixture_graph(
    storage: &mut Store,
    project_root: &Path,
) -> codestory_contracts::graph::NodeId {
    let file_path = project_root.join("lib.rs");
    let file_node_id = 1_001_i64;
    storage
        .insert_file(&FileInfo {
            id: file_node_id,
            path: file_path.clone(),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 3,
            file_role: FileRole::Entrypoint,
        })
        .expect("insert file");
    let file_node = Node {
        id: NodeId(file_node_id),
        kind: NodeKind::FILE,
        serialized_name: "lib.rs".to_string(),
        qualified_name: None,
        canonical_id: None,
        file_node_id: None,
        start_line: Some(1),
        start_col: Some(0),
        end_line: Some(3),
        end_col: Some(0),
    };
    storage.insert_nodes_batch(&[file_node]).expect("file node");
    let func_node = Node {
        id: NodeId(2_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "extension_service".to_string(),
        qualified_name: Some("extension_service".to_string()),
        canonical_id: None,
        file_node_id: Some(NodeId(file_node_id)),
        start_line: Some(1),
        start_col: Some(0),
        end_line: Some(1),
        end_col: Some(30),
    };
    storage
        .insert_nodes_batch(std::slice::from_ref(&func_node))
        .expect("func node");
    storage
        .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
            node_id: func_node.id,
            display_name: "extension_service".to_string(),
        }])
        .expect("projection");
    func_node.id
}

#[test]
#[ignore = "requires live mandatory retrieval sidecars"]
fn full_mode_fixture_produces_resolvable_hits() {
    // SAFETY: serialized env mutation for this test only.
    unsafe {
        std::env::set_var("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        std::env::set_var("CODESTORY_EMBED_BACKEND", "llamacpp");
    }

    let project = TempDir::new().expect("project");
    std::fs::write(
        project.path().join("lib.rs"),
        "pub fn extension_service() {}\n",
    )
    .expect("write lib.rs");

    let storage_dir = TempDir::new().expect("storage");
    let storage_path = storage_dir.path().join("codestory.db");
    {
        let mut storage = Store::open(&storage_path).expect("open db");
        seed_fixture_graph(&mut storage, project.path());
    }

    finalize_index(project.path(), &storage_path).expect("finalize index");

    let layout = SidecarLayout::from_env();
    let project_id = project_id_for_root(project.path());
    if qdrant_enabled() {
        let qdrant = QdrantClient::new(&layout);
        if qdrant.list_collections_probe().reachable {
            let collection = QdrantClient::collection_name(&project_id);
            let _ = qdrant.ensure_collection(&collection);
            let storage = Store::open(&storage_path).expect("reopen for qdrant");
            if storage.get_search_symbol_projection_count().unwrap_or(0) > 0 {
                let points = vec![codestory_retrieval::QdrantUpsertPoint {
                    id: 2_001,
                    display_name: "extension_service".to_string(),
                    node_id: "2001".to_string(),
                    file_path: Some("lib.rs".to_string()),
                    file_role: Some(FileRole::Entrypoint),
                    dense_reason: Some("entrypoint".to_string()),
                    vector: None,
                }];
                if qdrant.upsert_points(&collection, &points).is_ok() {
                    let stub = QdrantClient::stub_marker_path(&layout.qdrant_data_dir, &collection);
                    if stub.is_file() {
                        let _ = std::fs::remove_file(stub);
                    }
                }
            }
        }
    }

    let manifest = Store::open(&storage_path)
        .expect("reopen")
        .get_retrieval_index_manifest(&project_id)
        .expect("load manifest")
        .expect("manifest row");
    let status = probe_sidecar_health(&layout, &project_id, Some(manifest));
    assert_eq!(
        status.retrieval_mode, "full",
        "expected full mode, got {} ({:?})",
        status.retrieval_mode, status.degraded_reason
    );
    assert!(status.zoekt.capabilities.lexical);
    assert!(status.qdrant.capabilities.semantic);
    assert!(status.scip.capabilities.graph);

    let result = execute_retrieval_query(QueryRequest {
        project_root: project.path(),
        storage_path: &storage_path,
        query: "extension_service",
        budget_ms: Some(1_000),
        cancelled: None,
    })
    .expect("query");
    assert!(!result.hits.is_empty(), "expected sidecar hits");
    assert!(
        result
            .hits
            .iter()
            .any(|hit| hit.file_path.contains("lib.rs") && !hit.file_path.starts_with("zoekt:")),
        "expected repo-relative zoekt/scip hit, got {:?}",
        result.hits
    );

    // SAFETY: test env cleanup.
    unsafe {
        std::env::remove_var("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS");
        std::env::remove_var("CODESTORY_EMBED_BACKEND");
    }
}
