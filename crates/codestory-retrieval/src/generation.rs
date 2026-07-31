#[cfg(test)]
use codestory_contracts::graph::NodeKind;
use codestory_store::{RetrievalIndexManifest, Store};

pub const SIDECAR_SCHEMA_VERSION: i32 = 6;
pub const SEMANTIC_POLICY_VERSION: &str = "graph_first_v2";
pub const SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED: &str =
    "sidecar_semantic_doc_embedding_contract_changed";

pub fn sidecar_generation_id(project_id: &str, sidecar_input_hash: &str) -> String {
    let suffix = sidecar_input_hash.chars().take(16).collect::<String>();
    format!("{project_id}-{suffix}")
}

pub fn sidecar_vector_generation(project_id: &str, sidecar_input_hash: &str) -> String {
    let suffix = sidecar_input_hash.chars().take(16).collect::<String>();
    format!("codestory_{project_id}_{suffix}")
}

pub fn manifest_has_current_sidecar_contract(
    project_id: &str,
    manifest: &RetrievalIndexManifest,
) -> bool {
    let Some(hash) = manifest.sidecar_input_hash.as_deref() else {
        return false;
    };
    let expected_generation = sidecar_generation_id(project_id, hash);
    let expected_collection = sidecar_vector_generation(project_id, hash);
    !hash.trim().is_empty()
        && manifest.lexical_version == crate::lexical_index::LEXICAL_INDEX_VERSION
        && manifest.sidecar_schema_version == Some(SIDECAR_SCHEMA_VERSION)
        && manifest.sidecar_generation.as_deref() == Some(expected_generation.as_str())
        && manifest.semantic_generation == expected_collection
        && manifest.projection_count.is_some_and(|count| count >= 0)
        && manifest.symbol_doc_count.is_some_and(|count| count >= 0)
        && manifest
            .dense_projection_count
            .is_some_and(|count| count >= 0)
        && manifest.dense_projection_count == manifest.projection_count
        && manifest.semantic_policy_version.as_deref() == Some(SEMANTIC_POLICY_VERSION)
        && manifest
            .graph_artifact_hash
            .as_deref()
            .is_some_and(|hash| !hash.trim().is_empty())
        && manifest.dense_reason_counts_json.is_some()
}

#[cfg(test)]
pub fn manifest_staleness_reason(
    storage: &Store,
    manifest: &RetrievalIndexManifest,
) -> Option<String> {
    manifest_staleness_reason_for_runtime(
        storage,
        manifest,
        &crate::config::SidecarRuntimeConfig::local(),
    )
}

pub fn manifest_staleness_reason_for_runtime(
    storage: &Store,
    manifest: &RetrievalIndexManifest,
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<String> {
    let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    if manifest.embedding_backend.as_deref() != Some(embedding_backend.as_str()) {
        return Some(format!(
            "sidecar_embedding_backend_changed: manifest={} current={embedding_backend}",
            manifest.embedding_backend.as_deref().unwrap_or("<missing>")
        ));
    }

    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    if manifest.embedding_dim != Some(embedding_dim) {
        return Some(format!(
            "sidecar_embedding_dim_changed: manifest={} current={embedding_dim}",
            manifest
                .embedding_dim
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<missing>".into())
        ));
    }

    if manifest.semantic_policy_version.as_deref() != Some(SEMANTIC_POLICY_VERSION) {
        return Some(format!(
            "sidecar_semantic_policy_changed: manifest={} current={SEMANTIC_POLICY_VERSION}",
            manifest
                .semantic_policy_version
                .as_deref()
                .unwrap_or("<missing>")
        ));
    }

    if let Some(expected_symbol_doc_count) = manifest.symbol_doc_count {
        match storage.get_symbol_search_doc_count() {
            Ok(actual) if i64::from(actual) == expected_symbol_doc_count => {}
            Ok(actual) => {
                return Some(format!(
                    "sidecar_symbol_doc_count_changed: manifest={expected_symbol_doc_count} current={actual}"
                ));
            }
            Err(error) => {
                return Some(format!("sidecar_symbol_doc_count_unavailable: {error}"));
            }
        }
    }

    if let Some(expected_count) = manifest
        .dense_projection_count
        .or(manifest.projection_count)
    {
        // Aggregate, never paged rows: this scan is on every observational
        // readiness and status call, and paging `DenseAnchorInput`s here
        // materialized every anchor's `document_text` just to count them.
        match storage.dense_anchor_input_stats() {
            Ok(stats) => {
                if expected_count > 0 && stats.doc_count == 0 {
                    return Some(
                        "sidecar_dense_anchor_count_unavailable: no published dense anchors".into(),
                    );
                }
                if expected_count > 0
                    && (stats.mixed_policy_versions
                        || stats.policy_version.as_deref() != Some(SEMANTIC_POLICY_VERSION))
                {
                    return Some(SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED.into());
                }
                if i64::from(stats.doc_count) != expected_count {
                    return Some(format!(
                        "sidecar_dense_anchor_count_changed: manifest={expected_count} current={}",
                        stats.doc_count
                    ));
                }
                if let Some(expected_reasons) = manifest.dense_reason_counts_json.as_deref() {
                    let actual_reasons = serde_json::to_string(&stats.selection_reason_counts)
                        .unwrap_or_else(|_| "{}".into());
                    if actual_reasons != expected_reasons {
                        return Some(format!(
                            "sidecar_dense_reason_counts_changed: manifest={expected_reasons} current={actual_reasons}"
                        ));
                    }
                }
            }
            Err(error) => {
                return Some(format!("sidecar_dense_anchor_count_unavailable: {error}"));
            }
        }
    }

    match storage.max_indexed_file_modification_time() {
        Ok(Some(max_mtime)) if max_mtime > manifest.built_at_epoch_ms => Some(format!(
            "indexed_file_newer_than_retrieval_manifest: file_mtime={max_mtime} manifest_built_at={}",
            manifest.built_at_epoch_ms
        )),
        Err(error) => Some(format!("indexed_file_mtime_unavailable: {error}")),
        _ => None,
    }
}

pub fn manifest_unavailable_reason_for_runtime(
    project_id: &str,
    storage: &Store,
    manifest: &RetrievalIndexManifest,
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<String> {
    if !manifest_has_current_sidecar_contract(project_id, manifest) {
        return Some("retrieval_manifest_generation_contract_missing".into());
    }
    manifest_staleness_reason_for_runtime(storage, manifest, runtime)
        .map(|reason| format!("retrieval_manifest_stale: {reason}"))
}

/// The incomplete-incremental-run refusal, named once.
///
/// Strict sidecar admission checks this before anything else and unconditionally
/// — before the manifest-contract early return — so an interrupted incremental
/// index refuses even while the manifest, counts, and mtimes still agree. It is
/// re-exported through `storage_admission_refusal_reason_for_runtime` so a
/// `&Store`-only caller outside this crate gates on the same marker rather than
/// re-deriving one of its own.
pub(crate) fn incomplete_incremental_run_reason(storage: &Store) -> Option<String> {
    match storage.has_incomplete_incremental_run() {
        Ok(true) => Some("incomplete_incremental_index_run".into()),
        Ok(false) => None,
        Err(error) => Some(format!(
            "incomplete_incremental_index_marker_unavailable: {error}"
        )),
    }
}

/// Every sidecar-admission refusal a caller holding only `&Store` can re-derive.
///
/// This is deliberately **not** all of admission. `SidecarQuery::begin` refuses
/// on two independent gates: this one, and
/// `validate_strict_sidecar_readiness_for_runtime`, which is `pub(crate)` here
/// and structurally unreachable from other crates because it additionally needs
/// the storage path, the project root's workspace manifest, and a producer
/// compatibility identity. The reasons only that second gate can raise are
/// `sidecar_symbol_docs_mixed_embedding_backends`,
/// `sidecar_symbol_doc_embedding_backend_changed`,
/// `indexed_file_error_retry_required`,
/// `indexable_file_added_or_changed_after_retrieval_manifest`,
/// `indexed_file_removed_after_retrieval_manifest`, and
/// `sidecar_input_hash_changed`. A caller of this function is therefore
/// *conservative-agreeing* with admission, not equal to it: everything this
/// refuses, admission also refuses, but admission can still refuse more.
/// Callers that must not over-claim readiness need that direction; callers
/// needing exact parity have to go through `retrieval status`, which runs the
/// strict gate.
pub fn storage_admission_refusal_reason_for_runtime(
    project_id: &str,
    storage: &Store,
    manifest: &RetrievalIndexManifest,
    runtime: &crate::config::SidecarRuntimeConfig,
) -> Option<String> {
    if let Some(reason) = incomplete_incremental_run_reason(storage) {
        return Some(format!("retrieval_manifest_stale: {reason}"));
    }
    manifest_unavailable_reason_for_runtime(project_id, storage, manifest, runtime)
}

pub fn manifest_sidecar_generation(manifest: &RetrievalIndexManifest) -> &str {
    manifest
        .sidecar_generation
        .as_deref()
        .expect("validated sidecar manifest has a generation id")
}

#[cfg(test)]
pub(crate) fn sidecar_semantic_node_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ANNOTATION
            | NodeKind::UNION
            | NodeKind::ENUM
            | NodeKind::TYPEDEF
            | NodeKind::FUNCTION
            | NodeKind::METHOD
            | NodeKind::MACRO
            | NodeKind::GLOBAL_VARIABLE
            | NodeKind::CONSTANT
            | NodeKind::ENUM_CONSTANT
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{DenseAnchorInput, FileRole};
    use tempfile::TempDir;

    fn manifest(project_id: &str, hash: &str) -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: project_id.into(),
            lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
            semantic_generation: sidecar_vector_generation(project_id, hash),
            scip_revision: Some("graph-test".into()),
            built_at_epoch_ms: 123,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
            embedding_dim: Some(768),
            sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
            sidecar_input_hash: Some(hash.into()),
            sidecar_generation: Some(sidecar_generation_id(project_id, hash)),
            projection_count: Some(7),
            symbol_doc_count: Some(0),
            dense_projection_count: Some(7),
            semantic_policy_version: Some(SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: Some("graph-test-hash".into()),
            dense_reason_counts_json: Some("{\"public_api\":7}".into()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        }
    }

    #[test]
    fn current_sidecar_contract_requires_derived_generation_and_collection() {
        let project_id = "proj";
        let hash = "deadbeefcafebabe1234";
        let current = manifest(project_id, hash);
        assert!(manifest_has_current_sidecar_contract(project_id, &current));

        let mut stale_generation = current.clone();
        stale_generation.sidecar_generation = Some("proj-old".into());
        assert!(!manifest_has_current_sidecar_contract(
            project_id,
            &stale_generation
        ));

        let mut stale_collection = current.clone();
        stale_collection.semantic_generation = "codestory_proj_old".into();
        assert!(!manifest_has_current_sidecar_contract(
            project_id,
            &stale_collection
        ));

        let mut legacy = current;
        legacy.sidecar_schema_version = Some(SIDECAR_SCHEMA_VERSION - 1);
        assert!(!manifest_has_current_sidecar_contract(project_id, &legacy));

        let mut legacy = manifest(project_id, hash);
        legacy.sidecar_schema_version = None;
        assert!(!manifest_has_current_sidecar_contract(project_id, &legacy));

        let mut legacy_policy = manifest(project_id, hash);
        legacy_policy.semantic_policy_version = Some("graph_first_v1".into());
        assert!(!manifest_has_current_sidecar_contract(
            project_id,
            &legacy_policy
        ));
    }

    #[test]
    fn manifest_staleness_rejects_previous_dense_anchor_policy() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("codestory.db");
        let storage = Store::open(&storage_path).expect("open store");
        let mut manifest = manifest("proj", "deadbeefcafebabe1234");
        manifest.embedding_backend = Some(crate::embeddings::embedding_runtime_id());
        manifest.embedding_dim = Some(crate::embeddings::semantic_vector_dim() as i32);
        manifest.semantic_policy_version = Some("graph_first_v1".into());

        let reason = manifest_staleness_reason(&storage, &manifest)
            .expect("previous dense-anchor policy should stale");

        assert!(reason.contains("sidecar_semantic_policy_changed"));
        assert!(reason.contains("manifest=graph_first_v1"));
        assert!(reason.contains(SEMANTIC_POLICY_VERSION));
    }

    #[test]
    fn manifest_staleness_rejects_wrong_embedding_contract() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("codestory.db");
        let storage = Store::open(&storage_path).expect("open store");
        let mut manifest = manifest("proj", "deadbeefcafebabe1234");
        manifest.embedding_backend = Some("inprocess:unexpected-model:q8_0".into());

        let reason = manifest_staleness_reason(&storage, &manifest)
            .expect("embedding backend drift should stale manifest");

        assert!(reason.contains("sidecar_embedding_backend_changed"));
        assert!(reason.contains("manifest=inprocess:unexpected-model:q8_0"));
    }

    #[test]
    fn manifest_staleness_rejects_mixed_dense_anchor_policies() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: NodeId(1),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "do_work_1".into(),
                    ..Default::default()
                },
                Node {
                    id: NodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "do_work_2".into(),
                    ..Default::default()
                },
            ])
            .expect("nodes");
        storage
            .upsert_dense_anchor_inputs_batch(&[
                dense_anchor(1, NodeKind::FUNCTION, SEMANTIC_POLICY_VERSION),
                dense_anchor(2, NodeKind::FUNCTION, "graph_first_v0"),
            ])
            .expect("dense anchors");
        let mut manifest = manifest("proj", "deadbeefcafebabe1234");
        manifest.embedding_backend = Some(crate::embeddings::embedding_runtime_id());
        manifest.embedding_dim = Some(crate::embeddings::semantic_vector_dim() as i32);
        manifest.projection_count = Some(2);
        manifest.dense_projection_count = Some(2);
        manifest.dense_reason_counts_json = Some("{\"public_api\":2}".into());

        let reason = manifest_staleness_reason(&storage, &manifest)
            .expect("mixed dense-anchor policies should stale");

        assert_eq!(reason, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED);
    }

    #[test]
    fn manifest_staleness_uses_dense_anchor_inputs_not_legacy_vector_rows() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: NodeId(1),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "do_work".into(),
                    ..Default::default()
                },
                Node {
                    id: NodeId(2),
                    kind: NodeKind::VARIABLE,
                    serialized_name: "current".into(),
                    ..Default::default()
                },
            ])
            .expect("nodes");
        storage
            .upsert_dense_anchor_inputs_batch(&[dense_anchor(
                2,
                NodeKind::VARIABLE,
                SEMANTIC_POLICY_VERSION,
            )])
            .expect("dense anchor");
        let mut manifest = manifest("proj", "deadbeefcafebabe1234");
        manifest.embedding_backend = Some(crate::embeddings::embedding_runtime_id());
        manifest.embedding_dim = Some(crate::embeddings::semantic_vector_dim() as i32);
        manifest.projection_count = Some(1);
        manifest.dense_projection_count = Some(1);
        manifest.dense_reason_counts_json = Some("{\"public_api\":1}".into());

        let reason = manifest_staleness_reason(&storage, &manifest);

        assert_eq!(reason, None);
    }

    fn dense_anchor(node_id: i64, kind: NodeKind, policy_version: &str) -> DenseAnchorInput {
        DenseAnchorInput {
            node_id: NodeId(node_id),
            file_node_id: None,
            kind,
            display_name: format!("do_work_{node_id}"),
            qualified_name: Some(format!("pkg::do_work_{node_id}")),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            end_line: Some(2),
            file_role: FileRole::Source,
            source_provenance: "extracted".into(),
            text: "semantic_doc_version: 6\nname: do_work".into(),
            document_hash: format!("hash-{node_id}"),
            selection_reason: "public_api".into(),
            policy_version: policy_version.into(),
            source_identity: "core:generation:run".into(),
            updated_at_epoch_ms: 123,
        }
    }
}
