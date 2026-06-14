use codestory_contracts::graph::NodeKind;
use codestory_store::{LlmSymbolDoc, RetrievalIndexManifest, Store};
use std::collections::BTreeMap;

pub const SIDECAR_SCHEMA_VERSION: i32 = 1;
pub const SEMANTIC_POLICY_VERSION: &str = "graph_first_v1";
pub const SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED: &str =
    "sidecar_semantic_doc_embedding_contract_changed";
const STALENESS_DOC_BATCH_SIZE: usize = 1024;

pub fn sidecar_generation_id(project_id: &str, sidecar_input_hash: &str) -> String {
    let suffix = sidecar_input_hash.chars().take(16).collect::<String>();
    format!("{project_id}-{suffix}")
}

pub fn sidecar_qdrant_collection(project_id: &str, sidecar_input_hash: &str) -> String {
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
    let expected_collection = sidecar_qdrant_collection(project_id, hash);
    !hash.trim().is_empty()
        && manifest.sidecar_schema_version == Some(SIDECAR_SCHEMA_VERSION)
        && manifest.sidecar_generation.as_deref() == Some(expected_generation.as_str())
        && manifest.qdrant_collection == expected_collection
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

pub fn manifest_staleness_reason(
    storage: &Store,
    manifest: &RetrievalIndexManifest,
) -> Option<String> {
    let embedding_backend = crate::embeddings::embedding_runtime_id();
    if manifest.embedding_backend.as_deref() != Some(embedding_backend.as_str()) {
        return Some(format!(
            "sidecar_embedding_backend_changed: manifest={} current={embedding_backend}",
            manifest.embedding_backend.as_deref().unwrap_or("<missing>")
        ));
    }

    let embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
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
        match collect_sidecar_semantic_doc_stats(storage) {
            Ok(stats) => {
                if expected_count > 0 && stats.doc_count == 0 {
                    return Some(
                        "sidecar_semantic_doc_count_unavailable: no sidecar-eligible stored docs"
                            .into(),
                    );
                }
                if expected_count > 0
                    && (stats.mixed_embedding_profiles
                        || stats.mixed_embedding_models
                        || stats.mixed_embedding_backends
                        || stats.mixed_dimensions
                        || stats.mixed_doc_shapes
                        || stats.mixed_semantic_policy_versions
                        || stats.semantic_policy_version.as_deref()
                            != Some(SEMANTIC_POLICY_VERSION)
                        || stats.embedding_profile.as_deref() != Some("bge-base-en-v1.5")
                        || stats.embedding_dim
                            != Some(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32)
                        || !stats
                            .embedding_model
                            .as_deref()
                            .is_some_and(|model| model.contains("bge-base-en-v1.5")))
                {
                    return Some(SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED.into());
                }
                if i64::from(stats.doc_count) != expected_count {
                    return Some(format!(
                        "sidecar_semantic_doc_count_changed: manifest={expected_count} current={}",
                        stats.doc_count
                    ));
                }
                if let Some(expected_reasons) = manifest.dense_reason_counts_json.as_deref() {
                    let actual_reasons = serde_json::to_string(&stats.dense_reason_counts)
                        .unwrap_or_else(|_| "{}".into());
                    if actual_reasons != expected_reasons {
                        return Some(format!(
                            "sidecar_dense_reason_counts_changed: manifest={expected_reasons} current={actual_reasons}"
                        ));
                    }
                }
            }
            Err(error) => {
                return Some(format!("sidecar_semantic_doc_count_unavailable: {error}"));
            }
        }
    }

    match storage.max_indexed_file_modification_time() {
        Ok(Some(max_mtime)) if max_mtime > manifest.built_at_epoch_ms => Some(format!(
            "indexed_file_newer_than_sidecar_manifest: file_mtime={max_mtime} manifest_built_at={}",
            manifest.built_at_epoch_ms
        )),
        Err(error) => Some(format!("indexed_file_mtime_unavailable: {error}")),
        _ => None,
    }
}

pub fn manifest_unavailable_reason(
    project_id: &str,
    storage: &Store,
    manifest: &RetrievalIndexManifest,
) -> Option<String> {
    if !manifest_has_current_sidecar_contract(project_id, manifest) {
        return Some("sidecar_manifest_generation_contract_missing".into());
    }
    manifest_staleness_reason(storage, manifest)
        .map(|reason| format!("sidecar_manifest_stale: {reason}"))
}

pub fn manifest_sidecar_generation(manifest: &RetrievalIndexManifest) -> &str {
    manifest
        .sidecar_generation
        .as_deref()
        .expect("validated sidecar manifest has a generation id")
}

pub(crate) fn sidecar_semantic_doc_is_product_eligible(doc: &LlmSymbolDoc) -> bool {
    (sidecar_semantic_node_kind(doc.kind)
        || doc.dense_reason.as_deref() == Some("component_report"))
        && sidecar_stored_embedding_is_product_compatible(doc)
}

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

fn sidecar_stored_embedding_is_product_compatible(doc: &LlmSymbolDoc) -> bool {
    if doc.embedding_dim as usize != crate::embeddings::RETRIEVAL_EMBEDDING_DIM
        || doc.embedding.len() != crate::embeddings::RETRIEVAL_EMBEDDING_DIM
        || doc.embedding.iter().any(|value| !value.is_finite())
    {
        return false;
    }
    if doc.embedding_profile.as_deref() != Some("bge-base-en-v1.5") {
        return false;
    }
    if !doc.embedding_model.contains("bge-base-en-v1.5") {
        return false;
    }
    matches!(
        doc.embedding_backend
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("onnx" | "llamacpp" | "llama_cpp")
    )
}

#[derive(Default)]
struct SidecarSemanticDocStats {
    doc_count: u32,
    embedding_profile: Option<String>,
    embedding_model: Option<String>,
    embedding_backend: Option<String>,
    embedding_dim: Option<u32>,
    doc_shape: Option<String>,
    semantic_policy_version: Option<String>,
    dense_reason_counts: BTreeMap<String, u32>,
    mixed_embedding_profiles: bool,
    mixed_embedding_models: bool,
    mixed_embedding_backends: bool,
    mixed_dimensions: bool,
    mixed_doc_shapes: bool,
    mixed_semantic_policy_versions: bool,
}

fn collect_sidecar_semantic_doc_stats(storage: &Store) -> Result<SidecarSemanticDocStats, String> {
    let mut stats = SidecarSemanticDocStats::default();
    let mut first_profile: Option<Option<String>> = None;
    let mut first_model: Option<Option<String>> = None;
    let mut first_backend: Option<Option<String>> = None;
    let mut first_dim: Option<Option<u32>> = None;
    let mut first_shape: Option<Option<String>> = None;
    let mut first_policy: Option<Option<String>> = None;
    let mut after = None;

    loop {
        let docs = storage
            .get_llm_symbol_docs_batch_after(after, STALENESS_DOC_BATCH_SIZE)
            .map_err(|error| error.to_string())?;
        if docs.is_empty() {
            break;
        }
        after = docs.last().map(|doc| doc.node_id);
        for doc in docs
            .into_iter()
            .filter(sidecar_semantic_doc_is_product_eligible)
        {
            stats.doc_count = stats.doc_count.saturating_add(1);
            observe_optional_string(
                &mut first_profile,
                &mut stats.embedding_profile,
                &mut stats.mixed_embedding_profiles,
                doc.embedding_profile.as_deref(),
            );
            observe_optional_string(
                &mut first_model,
                &mut stats.embedding_model,
                &mut stats.mixed_embedding_models,
                Some(&doc.embedding_model),
            );
            observe_optional_string(
                &mut first_backend,
                &mut stats.embedding_backend,
                &mut stats.mixed_embedding_backends,
                doc.embedding_backend.as_deref(),
            );
            observe_optional_u32(
                &mut first_dim,
                &mut stats.embedding_dim,
                &mut stats.mixed_dimensions,
                Some(doc.embedding_dim),
            );
            observe_optional_string(
                &mut first_shape,
                &mut stats.doc_shape,
                &mut stats.mixed_doc_shapes,
                doc.doc_shape.as_deref(),
            );
            observe_optional_string(
                &mut first_policy,
                &mut stats.semantic_policy_version,
                &mut stats.mixed_semantic_policy_versions,
                doc.semantic_policy_version.as_deref(),
            );
            let reason = doc.dense_reason.unwrap_or_else(|| "unknown".into());
            *stats.dense_reason_counts.entry(reason).or_insert(0) += 1;
        }
    }

    Ok(stats)
}

fn observe_optional_string(
    first: &mut Option<Option<String>>,
    value: &mut Option<String>,
    mixed: &mut bool,
    current: Option<&str>,
) {
    let current = current.map(str::to_string);
    match first {
        Some(first) if first != &current => *mixed = true,
        Some(_) => {}
        None => {
            *value = current.clone();
            *first = Some(current);
        }
    }
}

fn observe_optional_u32(
    first: &mut Option<Option<u32>>,
    value: &mut Option<u32>,
    mixed: &mut bool,
    current: Option<u32>,
) {
    match first {
        Some(first) if first != &current => *mixed = true,
        Some(_) => {}
        None => {
            *value = current;
            *first = Some(current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use tempfile::TempDir;

    fn manifest(project_id: &str, hash: &str) -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: project_id.into(),
            zoekt_version: "zoekt-real-v1".into(),
            qdrant_collection: sidecar_qdrant_collection(project_id, hash),
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
        stale_collection.qdrant_collection = "codestory_proj_old".into();
        assert!(!manifest_has_current_sidecar_contract(
            project_id,
            &stale_collection
        ));

        let mut legacy = current;
        legacy.sidecar_schema_version = None;
        assert!(!manifest_has_current_sidecar_contract(project_id, &legacy));
    }

    #[test]
    fn manifest_staleness_rejects_embedding_contract_drift() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("codestory.db");
        let storage = Store::open(&storage_path).expect("open store");
        let mut manifest = manifest("proj", "deadbeefcafebabe1234");
        manifest.embedding_backend = Some("other-backend:768".into());

        let reason = manifest_staleness_reason(&storage, &manifest)
            .expect("embedding backend drift should stale manifest");

        assert!(reason.contains("sidecar_embedding_backend_changed"));
    }

    #[test]
    fn manifest_staleness_rejects_mixed_semantic_doc_shapes() {
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
            .upsert_llm_symbol_docs_batch(&[
                semantic_doc(1, "semantic_doc_version=4;scope=durable_symbols"),
                semantic_doc(
                    2,
                    "semantic_doc_version=4;scope=durable_symbols;alias_mode=alias_variant",
                ),
            ])
            .expect("docs");
        let mut manifest = manifest("proj", "deadbeefcafebabe1234");
        manifest.embedding_backend = Some(crate::embeddings::embedding_runtime_id());
        manifest.embedding_dim = Some(crate::embeddings::qdrant_vector_dim() as i32);
        manifest.dense_reason_counts_json = Some("{\"public_api\":2}".into());

        let reason =
            manifest_staleness_reason(&storage, &manifest).expect("mixed shapes should stale");

        assert_eq!(reason, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED);
    }

    #[test]
    fn manifest_staleness_counts_only_sidecar_eligible_semantic_docs() {
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
            .upsert_llm_symbol_docs_batch(&[
                semantic_doc(1, "semantic_doc_version=4;scope=durable_symbols"),
                LlmSymbolDoc {
                    kind: NodeKind::VARIABLE,
                    doc_shape: Some(
                        "semantic_doc_version=4;scope=local_symbol;variable=true".into(),
                    ),
                    ..semantic_doc(2, "semantic_doc_version=4;scope=local_symbol")
                },
            ])
            .expect("docs");
        let mut manifest = manifest("proj", "deadbeefcafebabe1234");
        manifest.embedding_backend = Some(crate::embeddings::embedding_runtime_id());
        manifest.embedding_dim = Some(crate::embeddings::qdrant_vector_dim() as i32);
        manifest.projection_count = Some(1);
        manifest.dense_projection_count = Some(1);
        manifest.dense_reason_counts_json = Some("{\"public_api\":1}".into());

        let reason = manifest_staleness_reason(&storage, &manifest);

        assert_eq!(reason, None);
    }

    fn semantic_doc(node_id: i64, doc_shape: &str) -> LlmSymbolDoc {
        LlmSymbolDoc {
            node_id: NodeId(node_id),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: format!("do_work_{node_id}"),
            qualified_name: Some(format!("pkg::do_work_{node_id}")),
            file_path: Some("src/lib.rs".into()),
            start_line: Some(1),
            doc_text: "semantic_doc_version: 4\nsymbol_kind: FUNCTION\nname: do_work".into(),
            doc_version: 4,
            doc_hash: format!("hash-{node_id}"),
            embedding_profile: Some("bge-base-en-v1.5".into()),
            embedding_model: "BAAI/bge-base-en-v1.5-local|backend=onnx".into(),
            embedding_backend: Some("onnx".into()),
            embedding_dim: crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            doc_shape: Some(doc_shape.into()),
            semantic_policy_version: Some(SEMANTIC_POLICY_VERSION.into()),
            dense_reason: Some("public_api".into()),
            embedding: vec![0.01; crate::embeddings::RETRIEVAL_EMBEDDING_DIM],
            updated_at_epoch_ms: 123,
        }
    }
}
