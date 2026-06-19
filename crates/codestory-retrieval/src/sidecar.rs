use crate::config::SidecarLayout;
use crate::generation::{
    SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED, manifest_has_current_sidecar_contract,
    manifest_staleness_reason, manifest_unavailable_reason,
};
use crate::health::{RetrievalStatusReport, attach_manifest_contract, probe_sidecar_health};
use crate::index::{compute_sidecar_input_fingerprint, sidecar_project_id_for_root};
use anyhow::{Context, Result};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext,
};
use codestory_store::Store;
use codestory_workspace::{RefreshInputs, StoredFileState, WorkspaceManifest};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarStateFile {
    pub zoekt_http_port: u16,
    pub qdrant_http_port: u16,
    pub qdrant_grpc_port: u16,
    pub zoekt_data_dir: String,
    pub qdrant_data_dir: String,
    pub scip_artifacts_root: String,
    pub started_at_epoch_ms: i64,
}

pub fn sidecar_up() -> Result<SidecarStateFile> {
    let layout = SidecarLayout::from_env();
    layout.ensure_data_dirs()?;
    let state = SidecarStateFile {
        zoekt_http_port: layout.zoekt_http_port,
        qdrant_http_port: layout.qdrant_http_port,
        qdrant_grpc_port: layout.qdrant_grpc_port,
        zoekt_data_dir: layout.zoekt_data_dir.display().to_string(),
        qdrant_data_dir: layout.qdrant_data_dir.display().to_string(),
        scip_artifacts_root: layout.scip_artifacts_root.display().to_string(),
        started_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
    };
    let json = serde_json::to_string_pretty(&state).context("serialize sidecar state")?;
    std::fs::write(&layout.state_file, json).context("write retrieval-sidecars.json")?;
    Ok(state)
}

pub fn sidecar_down() -> Result<()> {
    let layout = SidecarLayout::from_env();
    if layout.state_file.exists() {
        std::fs::remove_file(&layout.state_file).context("remove retrieval-sidecars.json")?;
    }
    Ok(())
}

pub fn sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner(project_root, storage_path, false)
}

pub fn strict_sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    sidecar_status_inner(project_root, storage_path, true)
}

fn sidecar_status_inner(
    project_root: &Path,
    storage_path: Option<&Path>,
    strict: bool,
) -> Result<RetrievalStatusReport> {
    let layout = SidecarLayout::from_env();
    let project_id = sidecar_project_id_for_root(project_root);
    let manifest = if let Some(path) = storage_path.filter(|path| path.exists()) {
        let storage = Store::open(path).context("open storage for manifest")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
        if strict
            && let Some(manifest) = manifest.as_ref()
            && let Some(reason) = strict_readiness_unavailable_reason(
                project_root,
                path,
                &storage,
                &project_id,
                manifest,
            )
            .context("check strict sidecar readiness")?
        {
            return Ok(enrich_status_with_semantic_doc_stats(
                attach_manifest_contract(
                    crate::health::unavailable_status_report(
                        format!("sidecar_manifest_stale: {reason}"),
                        Some(manifest.clone()),
                    ),
                    project_root,
                ),
                &storage,
            ));
        }
        if let Some(manifest) = manifest.as_ref()
            && let Some(reason) = manifest_unavailable_reason(&project_id, &storage, manifest)
        {
            return Ok(enrich_status_with_semantic_doc_stats(
                attach_manifest_contract(
                    crate::health::unavailable_status_report(reason, Some(manifest.clone())),
                    project_root,
                ),
                &storage,
            ));
        }
        let report = probe_sidecar_health(&layout, &project_id, manifest);
        return Ok(enrich_status_with_semantic_doc_stats(
            attach_manifest_contract(report, project_root),
            &storage,
        ));
    } else {
        None
    };
    Ok(attach_manifest_contract(
        probe_sidecar_health(&layout, &project_id, manifest),
        project_root,
    ))
}

fn enrich_status_with_semantic_doc_stats(
    mut report: RetrievalStatusReport,
    storage: &Store,
) -> RetrievalStatusReport {
    if let Ok(stats) = storage.get_llm_symbol_doc_stats() {
        report.stored_doc_vector_producer_backend = stats.embedding_backend;
        report.stored_doc_vector_dim = stats.embedding_dim;
        report.stored_doc_vector_mixed_backends = Some(stats.mixed_embedding_backends);
    }
    report
}

pub(crate) fn validate_strict_sidecar_readiness(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
) -> Result<()> {
    let project_id = sidecar_project_id_for_root(project_root);
    let Some(manifest) = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load retrieval manifest for strict readiness")?
    else {
        return Ok(());
    };
    if let Some(reason) = strict_readiness_unavailable_reason(
        project_root,
        storage_path,
        storage,
        &project_id,
        &manifest,
    )? {
        anyhow::bail!("sidecar_manifest_stale: {reason}");
    }
    Ok(())
}

fn strict_readiness_unavailable_reason(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
    project_id: &str,
    manifest: &codestory_store::RetrievalIndexManifest,
) -> Result<Option<String>> {
    if !manifest_has_current_sidecar_contract(project_id, manifest) {
        return Ok(None);
    }
    if let Some(reason) = manifest_staleness_reason(storage, manifest)
        && manifest_contract_drift_should_win(&reason)
    {
        return Ok(None);
    }

    let embedding_backend = crate::embeddings::embedding_runtime_id();
    let embedding_dim = i32::try_from(crate::embeddings::qdrant_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    let current_input = compute_sidecar_input_fingerprint(
        storage,
        storage_path,
        project_root,
        project_id,
        &embedding_backend,
        embedding_dim,
    )
    .context("compute strict sidecar input fingerprint")?;
    if manifest.sidecar_input_hash.as_deref() == Some(current_input.hash.as_str())
        && manifest.projection_count == Some(current_input.projection_count)
        && manifest.symbol_doc_count == Some(current_input.symbol_doc_count)
        && manifest.dense_projection_count == Some(current_input.dense_projection_count)
        && manifest.semantic_policy_version == current_input.semantic_policy_version
        && manifest.graph_artifact_hash.as_deref()
            == Some(current_input.graph_artifact_hash.as_str())
        && manifest.dense_reason_counts_json.as_deref()
            == Some(current_input.dense_reason_counts_json.as_str())
    {
        return Ok(None);
    }

    let workspace = WorkspaceManifest::open(project_root.to_path_buf())
        .context("open workspace manifest for strict sidecar readiness")?;
    let files = storage.files().get_files().context("load indexed files")?;
    let refresh_inputs = RefreshInputs {
        stored_files: files
            .into_iter()
            .map(|file| StoredFileState {
                id: file.id,
                path: file.path,
                modification_time: file.modification_time,
                indexed: file.indexed,
            })
            .collect(),
        inventory: Default::default(),
    };
    let plan = workspace
        .build_execution_plan(&refresh_inputs)
        .context("build strict sidecar freshness plan")?;
    if let Some(path) = plan
        .files_to_index
        .iter()
        .find(|path| graph_indexed_source_path(path))
    {
        return Ok(Some(format!(
            "indexable_file_added_or_changed_after_sidecar_manifest: {}",
            path.display()
        )));
    }
    if let Some(file_id) = plan.files_to_remove.first() {
        return Ok(Some(format!(
            "indexed_file_removed_after_sidecar_manifest: file_id={file_id}"
        )));
    }
    Ok(Some(format!(
        "sidecar_input_hash_changed: manifest={} current={}; symbol_doc_count manifest={} current={}; dense_projection_count manifest={} current={}; projection_count manifest={} current={}",
        manifest
            .sidecar_input_hash
            .as_deref()
            .unwrap_or("<missing>"),
        current_input.hash,
        manifest
            .symbol_doc_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.symbol_doc_count,
        manifest
            .dense_projection_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.dense_projection_count,
        manifest
            .projection_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "<missing>".into()),
        current_input.projection_count
    )))
}

fn graph_indexed_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(language_support_profile_for_ext)
        .is_some_and(|profile| profile.support_mode == LanguageSupportMode::ParserBackedGraph)
}

fn manifest_contract_drift_should_win(reason: &str) -> bool {
    reason.contains("sidecar_embedding_backend_changed")
        || reason.contains("sidecar_embedding_dim_changed")
        || reason == SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{
        SIDECAR_SCHEMA_VERSION, sidecar_generation_id, sidecar_qdrant_collection,
    };
    use crate::index::{compute_sidecar_input_fingerprint, project_id_for_root};
    use crate::test_support::retrieval_manifest_fixture;
    use codestory_store::{FileInfo, FileRole};
    use tempfile::TempDir;

    #[test]
    fn status_rejects_stale_manifest_before_component_probes() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let project_id = project_id_for_root(project.path());
        let hash = "deadbeefcafebabe";
        {
            let mut storage = Store::open(&storage_path).expect("open db");
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.projection_count = Some(10);
            manifest.symbol_doc_count = Some(10);
            manifest.dense_projection_count = Some(10);
            manifest.dense_reason_counts_json = Some("{\"public_api\":10}".into());
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        let report = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("sidecar status report");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar_manifest_stale")
        );
    }

    #[test]
    fn status_rejects_manifest_when_live_indexed_file_changes_or_is_removed() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "feedfacecafebeef";
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
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&source_path, "pub fn indexed() -> usize { 1 }\n").expect("mutate source");
        let changed = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("changed sidecar status");
        assert_eq!(changed.retrieval_mode, "unavailable");
        assert!(
            changed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "changed indexed file should make sidecar status fail closed: {changed:?}"
        );

        std::fs::remove_file(&source_path).expect("remove source");
        let removed = strict_sidecar_status(project.path(), Some(&storage_path))
            .expect("removed sidecar status");
        assert_eq!(removed.retrieval_mode, "unavailable");
        assert!(
            removed
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexed_file_removed_after_sidecar_manifest"),
            "removed indexed file should make sidecar status fail closed: {removed:?}"
        );
    }

    #[test]
    fn lightweight_status_does_not_scan_live_indexable_inventory() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "1ead1e55cafebeef";
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
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let lightweight =
            sidecar_status(project.path(), Some(&storage_path)).expect("lightweight status");
        let strict =
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert!(
            !lightweight
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "lightweight status should leave live inventory scans to strict callers: {lightweight:?}"
        );
        assert!(
            strict
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("sidecar_manifest_stale"),
            "strict status should fail closed on new indexable files: {strict:?}"
        );
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_indexable_file_is_added() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "ba5eba11cafebeef";
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
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("new_module.rs"),
            "pub fn newly_added() {}\n",
        )
        .expect("write new source");

        let report =
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new indexable file should make strict status fail closed: {report:?}"
        );
    }

    #[test]
    fn strict_status_rejects_manifest_when_new_parser_backed_language_file_is_added() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());
        let hash = "ba5eba11feedface";
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
            let mut manifest = retrieval_manifest_fixture(&project_id, hash);
            manifest.built_at_epoch_ms = indexed_mtime;
            storage
                .upsert_retrieval_index_manifest(&manifest)
                .expect("manifest");
        }

        std::fs::write(
            project.path().join("src").join("Routes.kt"),
            "fun routeUsers() = Unit\n",
        )
        .expect("write kotlin source");

        let report =
            strict_sidecar_status(project.path(), Some(&storage_path)).expect("strict status");

        assert_eq!(report.retrieval_mode, "unavailable");
        assert!(
            report
                .degraded_reason
                .as_deref()
                .unwrap_or_default()
                .contains("indexable_file_added_or_changed_after_sidecar_manifest"),
            "new registry-backed parser file should make strict status fail closed: {report:?}"
        );
    }

    #[test]
    fn strict_readiness_accepts_markdown_covered_by_sidecar_fingerprint() {
        let project = TempDir::new().expect("project");
        let storage_dir = TempDir::new().expect("storage");
        let storage_path = storage_dir.path().join("codestory.db");
        let source_path = project.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("source parent"))
            .expect("create source parent");
        std::fs::write(&source_path, "pub fn indexed() {}\n").expect("write source");
        std::fs::write(project.path().join("AGENTS.md"), "# Agent guidance\n")
            .expect("write markdown");
        let indexed_mtime = live_mtime_millis(&source_path);
        let project_id = project_id_for_root(project.path());

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
        let input = compute_sidecar_input_fingerprint(
            &storage,
            &storage_path,
            project.path(),
            &project_id,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32,
        )
        .expect("sidecar input");
        storage
            .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                project_id: project_id.clone(),
                zoekt_version: "zoekt-real-v1".into(),
                qdrant_collection: sidecar_qdrant_collection(&project_id, &input.hash),
                scip_revision: Some("graph-test".into()),
                built_at_epoch_ms: indexed_mtime,
                disk_bytes: None,
                degraded_modes_json: "[]".into(),
                embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                embedding_dim: Some(768),
                sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
                sidecar_input_hash: Some(input.hash.clone()),
                sidecar_generation: Some(sidecar_generation_id(&project_id, &input.hash)),
                projection_count: Some(input.projection_count),
                symbol_doc_count: Some(input.symbol_doc_count),
                dense_projection_count: Some(input.dense_projection_count),
                semantic_policy_version: input.semantic_policy_version.clone(),
                graph_artifact_hash: Some(input.graph_artifact_hash.clone()),
                dense_reason_counts_json: Some(input.dense_reason_counts_json.clone()),
            })
            .expect("manifest");

        validate_strict_sidecar_readiness(project.path(), &storage_path, &storage)
            .expect("markdown already covered by sidecar input should not look stale");

        std::fs::write(project.path().join("README.md"), "# New docs\n").expect("write new docs");
        let stale = validate_strict_sidecar_readiness(project.path(), &storage_path, &storage)
            .expect_err("new sidecar-only docs should stale the manifest");
        assert!(
            stale.to_string().contains("sidecar_input_hash_changed"),
            "docs-only sidecar drift should report input-hash drift, got: {stale:?}"
        );
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
