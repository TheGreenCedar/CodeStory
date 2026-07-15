use codestory_store::RetrievalIndexManifest;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn env_lock() -> MutexGuard<'static, ()> {
    // Environment guards restore their variables while unwinding. Recover the
    // mutex after a failed assertion so one primary failure does not obscure
    // the rest of the suite with poison errors.
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn retrieval_manifest_fixture(
    project_id: &str,
    sidecar_input_hash: &str,
) -> RetrievalIndexManifest {
    RetrievalIndexManifest {
        project_id: project_id.into(),
        lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
        semantic_generation: crate::generation::sidecar_vector_generation(
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
