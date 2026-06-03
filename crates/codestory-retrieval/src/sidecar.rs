use crate::config::SidecarLayout;
use crate::generation::manifest_unavailable_reason;
use crate::health::{RetrievalStatusReport, probe_sidecar_health};
use crate::index::project_id_for_root;
use anyhow::{Context, Result};
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
    let project_id = project_id_for_root(project_root);
    let manifest = if let Some(path) = storage_path.filter(|path| path.exists()) {
        let storage = Store::open(path).context("open storage for manifest")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
        if strict
            && let Some(manifest) = manifest.as_ref()
            && let Some(reason) = strict_readiness_unavailable_reason(project_root, &storage)
                .context("check strict sidecar readiness")?
        {
            return Ok(crate::health::unavailable_status_report(
                format!("sidecar_manifest_stale: {reason}"),
                Some(manifest.clone()),
            ));
        }
        if let Some(manifest) = manifest.as_ref()
            && let Some(reason) = manifest_unavailable_reason(&project_id, &storage, manifest)
        {
            return Ok(crate::health::unavailable_status_report(
                reason,
                Some(manifest.clone()),
            ));
        }
        manifest
    } else {
        None
    };
    Ok(probe_sidecar_health(&layout, &project_id, manifest))
}

pub(crate) fn validate_strict_sidecar_readiness(
    project_root: &Path,
    storage: &Store,
) -> Result<()> {
    if let Some(reason) = strict_readiness_unavailable_reason(project_root, storage)? {
        anyhow::bail!("sidecar_manifest_stale: {reason}");
    }
    Ok(())
}

fn strict_readiness_unavailable_reason(
    project_root: &Path,
    storage: &Store,
) -> Result<Option<String>> {
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
    if let Some(path) = plan.files_to_index.first() {
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
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{
        SIDECAR_SCHEMA_VERSION, sidecar_generation_id, sidecar_qdrant_collection,
    };
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
            storage
                .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                    project_id: project_id.clone(),
                    zoekt_version: "zoekt-real-v1".into(),
                    qdrant_collection: sidecar_qdrant_collection(&project_id, hash),
                    scip_revision: Some("graph-test".into()),
                    built_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                    embedding_dim: Some(768),
                    sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
                    sidecar_input_hash: Some(hash.into()),
                    sidecar_generation: Some(sidecar_generation_id(&project_id, hash)),
                    projection_count: Some(10),
                })
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
            storage
                .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                    project_id: project_id.clone(),
                    zoekt_version: "zoekt-real-v1".into(),
                    qdrant_collection: sidecar_qdrant_collection(&project_id, hash),
                    scip_revision: Some("graph-test".into()),
                    built_at_epoch_ms: indexed_mtime,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                    embedding_dim: Some(768),
                    sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
                    sidecar_input_hash: Some(hash.into()),
                    sidecar_generation: Some(sidecar_generation_id(&project_id, hash)),
                    projection_count: Some(0),
                })
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
            storage
                .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                    project_id: project_id.clone(),
                    zoekt_version: "zoekt-real-v1".into(),
                    qdrant_collection: sidecar_qdrant_collection(&project_id, hash),
                    scip_revision: Some("graph-test".into()),
                    built_at_epoch_ms: indexed_mtime,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                    embedding_dim: Some(768),
                    sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
                    sidecar_input_hash: Some(hash.into()),
                    sidecar_generation: Some(sidecar_generation_id(&project_id, hash)),
                    projection_count: Some(0),
                })
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
            storage
                .upsert_retrieval_index_manifest(&codestory_store::RetrievalIndexManifest {
                    project_id: project_id.clone(),
                    zoekt_version: "zoekt-real-v1".into(),
                    qdrant_collection: sidecar_qdrant_collection(&project_id, hash),
                    scip_revision: Some("graph-test".into()),
                    built_at_epoch_ms: indexed_mtime,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
                    embedding_dim: Some(768),
                    sidecar_schema_version: Some(SIDECAR_SCHEMA_VERSION),
                    sidecar_input_hash: Some(hash.into()),
                    sidecar_generation: Some(sidecar_generation_id(&project_id, hash)),
                    projection_count: Some(0),
                })
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
