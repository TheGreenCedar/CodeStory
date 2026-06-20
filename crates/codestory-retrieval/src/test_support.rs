use codestory_store::RetrievalIndexManifest;

pub fn retrieval_manifest_fixture(
    project_id: &str,
    sidecar_input_hash: &str,
) -> RetrievalIndexManifest {
    RetrievalIndexManifest {
        project_id: project_id.into(),
        zoekt_version: "zoekt-real-v1".into(),
        qdrant_collection: crate::generation::sidecar_qdrant_collection(
            project_id,
            sidecar_input_hash,
        ),
        scip_revision: Some("graph-test".into()),
        built_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
        disk_bytes: None,
        degraded_modes_json: "[]".into(),
        embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
        embedding_dim: Some(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32),
        sidecar_schema_version: Some(crate::generation::SIDECAR_SCHEMA_VERSION),
        sidecar_input_hash: Some(sidecar_input_hash.into()),
        sidecar_generation: Some(crate::generation::sidecar_generation_id(
            project_id,
            sidecar_input_hash,
        )),
        projection_count: Some(0),
        symbol_doc_count: Some(0),
        dense_projection_count: Some(0),
        semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
        graph_artifact_hash: Some("graph-test-hash".into()),
        dense_reason_counts_json: Some("{}".into()),
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    }
}
