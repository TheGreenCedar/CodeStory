//! Bootstrap Qdrant storage repair contracts (mixed cache layouts, prune suppression).

use codestory_retrieval::{
    BootstrapStorageScope, PRUNE_SUPPRESSED_PROTECTION_SCAN_ERROR, SidecarLayout,
    repair_qdrant_storage,
};
use codestory_store::{RetrievalIndexManifest, Store};
use std::fs;
use tempfile::tempdir;

fn touch_collection(collections_dir: &std::path::Path, name: &str) {
    let dir = collections_dir.join(name);
    fs::create_dir_all(&dir).expect("collection dir");
    fs::write(dir.join("config.json"), "{}").expect("config");
}

fn test_layout(qdrant_data: &tempfile::TempDir) -> SidecarLayout {
    SidecarLayout {
        qdrant_http_port: 1,
        qdrant_grpc_port: 1,
        lexical_data_dir: qdrant_data.path().join("lexical"),
        qdrant_data_dir: qdrant_data.path().to_path_buf(),
        scip_artifacts_root: qdrant_data.path().join("scip"),
        state_file: qdrant_data.path().join("state.json"),
    }
}

#[test]
fn mixed_flat_and_hashed_cache_protects_both_manifest_collections() {
    let cache_root = tempdir().expect("cache");
    let flat_db = cache_root.path().join("codestory.db");
    let mut flat_storage = Store::open(&flat_db).expect("open flat");
    flat_storage
        .upsert_retrieval_index_manifest(&RetrievalIndexManifest {
            project_id: "flat".into(),
            lexical_version: "v1".into(),
            qdrant_collection: "codestory_contract_flat".into(),
            scip_revision: None,
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: None,
            embedding_dim: None,
            sidecar_schema_version: None,
            sidecar_input_hash: None,
            sidecar_generation: None,
            projection_count: None,
            symbol_doc_count: None,
            dense_projection_count: None,
            semantic_policy_version: None,
            graph_artifact_hash: None,
            dense_reason_counts_json: None,
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        })
        .expect("flat manifest");

    let hashed_dir = cache_root.path().join("cccccccccccccccc");
    fs::create_dir_all(&hashed_dir).expect("hashed dir");
    let mut hashed_storage = Store::open(hashed_dir.join("codestory.db")).expect("open hashed");
    hashed_storage
        .upsert_retrieval_index_manifest(&RetrievalIndexManifest {
            project_id: "hashed".into(),
            lexical_version: "v1".into(),
            qdrant_collection: "codestory_contract_hashed".into(),
            scip_revision: None,
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: None,
            embedding_dim: None,
            sidecar_schema_version: None,
            sidecar_input_hash: None,
            sidecar_generation: None,
            projection_count: None,
            symbol_doc_count: None,
            dense_projection_count: None,
            semantic_policy_version: None,
            graph_artifact_hash: None,
            dense_reason_counts_json: None,
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        })
        .expect("hashed manifest");

    let qdrant_data = tempdir().expect("qdrant");
    let collections = qdrant_data.path().join("collections");
    fs::create_dir_all(&collections).expect("collections dir");
    touch_collection(&collections, "codestory_contract_flat");
    touch_collection(&collections, "codestory_contract_hashed");
    for index in 0..65 {
        touch_collection(
            &collections,
            &format!("codestory_contract_extra_{index:02}"),
        );
    }

    let scope = BootstrapStorageScope {
        repo_root: None,
        active_storage_path: None,
        active_cache_root: None,
        global_cache_root: cache_root.path().to_path_buf(),
    };
    let report = repair_qdrant_storage(&test_layout(&qdrant_data), &scope, 64).expect("repair");
    assert!(report.prune_suppressed_reason.is_none());
    assert!(report.pruned_collections > 0);
    assert!(collections.join("codestory_contract_flat").exists());
    assert!(collections.join("codestory_contract_hashed").exists());
}

#[test]
fn corrupt_active_cache_suppresses_retention_deletes() {
    let cache_root = tempdir().expect("cache");
    let corrupt_db = cache_root.path().join("codestory.db");
    fs::write(&corrupt_db, b"not sqlite").expect("corrupt db");

    let qdrant_data = tempdir().expect("qdrant");
    let collections = qdrant_data.path().join("collections");
    fs::create_dir_all(&collections).expect("collections dir");
    for index in 0..65 {
        touch_collection(&collections, &format!("codestory_suppressed_{index:02}"));
    }

    let scope = BootstrapStorageScope {
        repo_root: None,
        active_storage_path: Some(corrupt_db),
        active_cache_root: Some(cache_root.path().to_path_buf()),
        global_cache_root: cache_root.path().to_path_buf(),
    };
    let report = repair_qdrant_storage(&test_layout(&qdrant_data), &scope, 64).expect("repair");
    assert!(!report.scan_errors.is_empty());
    assert_eq!(
        report.prune_suppressed_reason.as_deref(),
        Some(PRUNE_SUPPRESSED_PROTECTION_SCAN_ERROR)
    );
    assert_eq!(report.pruned_collections, 0);
    assert_eq!(report.prune_candidates, 1);
}
