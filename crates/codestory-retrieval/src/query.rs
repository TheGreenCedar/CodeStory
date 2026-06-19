use crate::cache::RetrievalCache;
use crate::config::SidecarLayout;
use crate::executor::{QueryExecutor, QueryResult, cancellation_flag};
use crate::generation::manifest_unavailable_reason;
use crate::index::sidecar_project_id_for_root;
use crate::sidecar::validate_strict_sidecar_readiness;
use crate::sidecar_search::LiveSidecarSearch;
use anyhow::{Context, Result, bail};
use codestory_store::Store;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone)]
pub struct QueryRequest<'a> {
    pub project_root: &'a Path,
    pub storage_path: &'a Path,
    pub query: &'a str,
    pub budget_ms: Option<u64>,
    pub cancelled: Option<Arc<AtomicBool>>,
}

pub fn execute_retrieval_query(request: QueryRequest<'_>) -> Result<QueryResult> {
    let mut cache = RetrievalCache::new();
    execute_retrieval_query_with_cache(request, &mut cache)
}

pub fn execute_retrieval_query_with_cache(
    request: QueryRequest<'_>,
    cache: &mut RetrievalCache,
) -> Result<QueryResult> {
    let layout = SidecarLayout::from_env();
    let project_id = sidecar_project_id_for_root(request.project_root);
    let (manifest, file_roles) = if request.storage_path.exists() {
        let storage = Store::open(request.storage_path).context("open storage for query")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
        if let Some(manifest) = manifest.as_ref() {
            if let Err(error) = validate_strict_sidecar_readiness(
                request.project_root,
                request.storage_path,
                &storage,
            ) {
                bail!(
                    "retrieval sidecar manifest is unavailable ({error}); run retrieval index for project {project_id}"
                );
            }
            if let Some(reason) = manifest_unavailable_reason(&project_id, &storage, manifest) {
                bail!(
                    "retrieval sidecar manifest is unavailable ({reason}); run retrieval index for project {project_id}"
                );
            }
        } else {
            bail!(
                "retrieval sidecar manifest is missing; run retrieval index for project {project_id}"
            );
        }
        let roles = storage
            .get_files()
            .map(|files| {
                files
                    .into_iter()
                    .map(|file| (file.path.to_string_lossy().to_string(), file.file_role))
                    .collect()
            })
            .unwrap_or_default();
        (manifest, roles)
    } else {
        bail!("retrieval sidecar storage is missing; run retrieval index for project {project_id}");
    };

    let sidecars = LiveSidecarSearch::new(layout, project_id, manifest.as_ref());
    let cancelled = request.cancelled.unwrap_or_else(cancellation_flag);
    let mut executor = QueryExecutor {
        sidecars: &sidecars,
        cache,
        manifest,
        file_roles,
        cancelled,
        mode_override: None,
    };
    executor.execute(request.query, request.budget_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::finalize_index;
    use crate::test_support::retrieval_manifest_fixture;
    use crate::{QdrantClient, SidecarLayout, ZoektClient};
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{FileInfo, FileRole, LlmSymbolDoc, SearchSymbolProjection};
    use tempfile::TempDir;

    fn manifest_for(
        project_id: &str,
        hash: &str,
        projection_count: i64,
    ) -> codestory_store::RetrievalIndexManifest {
        let mut manifest = retrieval_manifest_fixture(project_id, hash);
        manifest.projection_count = Some(projection_count);
        manifest.symbol_doc_count = Some(projection_count);
        manifest.dense_projection_count = Some(projection_count);
        manifest.dense_reason_counts_json = Some(format!("{{\"public_api\":{projection_count}}}"));
        manifest
    }

    #[test]
    #[ignore = "requires live Qdrant, Zoekt, and embedding sidecars; run explicitly with cargo test -p codestory-retrieval integration_query_against_fixture_manifest -- --ignored --nocapture"]
    fn integration_query_against_fixture_manifest() {
        let layout = SidecarLayout::from_env();
        if !QdrantClient::new(&layout)
            .list_collections_probe()
            .reachable
            || !ZoektClient::new(&layout).health_probe().reachable
        {
            return;
        }
        if crate::embeddings::embed_query("function").is_err() {
            return;
        }

        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("lib.rs"),
            "pub fn extension_service() {}",
        )
        .expect("write");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            let file_id = 10_i64;
            let source_path = project.path().join("lib.rs");
            storage
                .insert_file(&FileInfo {
                    id: file_id,
                    path: source_path.clone(),
                    language: "rust".to_string(),
                    modification_time: live_mtime_millis(&source_path),
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Entrypoint,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: NodeId(file_id),
                        kind: NodeKind::FILE,
                        serialized_name: "lib.rs".to_string(),
                        qualified_name: None,
                        canonical_id: None,
                        file_node_id: None,
                        start_line: Some(1),
                        start_col: Some(0),
                        end_line: Some(1),
                        end_col: Some(0),
                    },
                    Node {
                        id: NodeId(11),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "extension_service".to_string(),
                        qualified_name: Some("extension_service".to_string()),
                        canonical_id: None,
                        file_node_id: Some(NodeId(file_id)),
                        start_line: Some(1),
                        start_col: Some(0),
                        end_line: Some(1),
                        end_col: Some(30),
                    },
                ])
                .expect("insert nodes");
            storage
                .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
                    node_id: NodeId(11),
                    display_name: "extension_service".to_string(),
                }])
                .expect("projection");
            storage
                .upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
                    node_id: NodeId(11),
                    file_node_id: Some(NodeId(file_id)),
                    kind: NodeKind::FUNCTION,
                    display_name: "extension_service".to_string(),
                    qualified_name: Some("extension_service".to_string()),
                    file_path: Some(project.path().join("lib.rs").display().to_string()),
                    start_line: Some(1),
                    doc_text:
                        "semantic_doc_version: 4\nsymbol_kind: FUNCTION\nname: extension_service"
                            .to_string(),
                    doc_version: 4,
                    doc_hash: "extension-service-doc".to_string(),
                    embedding_profile: Some("bge-base-en-v1.5".to_string()),
                    embedding_model: "BAAI/bge-base-en-v1.5-local|backend=onnx".to_string(),
                    embedding_backend: Some("onnx".to_string()),
                    embedding_dim: 768,
                    doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".to_string()),
                    semantic_policy_version: Some(
                        crate::generation::SEMANTIC_POLICY_VERSION.into(),
                    ),
                    dense_reason: Some("public_api".into()),
                    embedding: vec![0.01; 768],
                    updated_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
                }])
                .expect("semantic doc");
        }
        if let Err(error) = finalize_index(project.path(), &storage_path) {
            eprintln!(
                "skipping live retrieval query fixture because sidecar indexing failed: {error:#}"
            );
            return;
        }

        let result = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "extension",
            budget_ms: Some(500),
            cancelled: None,
        })
        .expect("query");

        assert_eq!(result.trace.retrieval_mode, "full");
        assert!(!result.hits.is_empty() || !result.trace.stages.is_empty());
    }

    #[test]
    fn query_rejects_legacy_manifest_before_sidecar_access() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                    project_id,
                    zoekt_version: "zoekt-real-v1".into(),
                    qdrant_collection: "codestory_legacy".into(),
                    scip_revision: Some("graph-test".into()),
                    built_at_epoch_ms: 1,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: Some("hash-projection:768".into()),
                    embedding_dim: Some(768),
                    sidecar_schema_version: None,
                    sidecar_input_hash: None,
                    sidecar_generation: None,
                    projection_count: None,
                    symbol_doc_count: None,
                    dense_projection_count: None,
                    semantic_policy_version: None,
                    graph_artifact_hash: None,
                    dense_reason_counts_json: None,
                })
                .expect("manifest");
        }

        let error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "ExtensionHostManager",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("legacy manifests must fail closed");

        assert!(error.to_string().contains("generation_contract_missing"));
    }

    #[test]
    fn query_rejects_manifest_with_stale_projection_count() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .upsert_retrieval_index_manifest(&manifest_for(&project_id, "deadbeefcafebabe", 10))
                .expect("manifest");
        }

        let error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "ExtensionHostManager",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("stale manifests must fail closed");

        assert!(error.to_string().contains("sidecar_manifest_stale"));
    }

    #[test]
    fn query_rejects_manifest_when_indexed_file_changes_or_is_removed() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            storage
                .upsert_retrieval_index_manifest(&manifest_for(
                    &project_id,
                    "changedfeedcafebeef",
                    0,
                ))
                .expect("manifest");
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&source_path, "pub fn indexed() -> usize { 1 }\n").expect("mutate source");
        let changed_error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "indexed",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("changed indexed file must fail closed");
        assert!(changed_error.to_string().contains("sidecar_manifest_stale"));

        std::fs::remove_file(&source_path).expect("remove source");
        let removed_error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "indexed",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("removed indexed file must fail closed");
        assert!(removed_error.to_string().contains("sidecar_manifest_stale"));
    }

    #[test]
    fn query_rejects_manifest_when_new_indexable_file_is_added() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = crate::index::project_id_for_root(project.path());
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path.clone(),
                    language: "rust".into(),
                    modification_time: indexed_mtime,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                })
                .expect("insert indexed file");
            storage
                .upsert_retrieval_index_manifest(&manifest_for(
                    &project_id,
                    "newfilefeedcafebeef",
                    0,
                ))
                .expect("manifest");
        }
        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let error = execute_retrieval_query(QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "newly_added",
            budget_ms: Some(100),
            cancelled: None,
        })
        .expect_err("new indexable file must fail closed");

        assert!(error.to_string().contains("sidecar_manifest_stale"));
    }

    fn live_mtime_millis(path: &Path) -> i64 {
        std::fs::metadata(path)
            .expect("metadata")
            .modified()
            .expect("modified")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("mtime since epoch")
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
}
