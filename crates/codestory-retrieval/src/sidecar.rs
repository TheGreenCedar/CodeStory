use crate::config::{SidecarProfile, SidecarRuntimeConfig};
use crate::generation::{
    SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED, manifest_has_current_sidecar_contract,
    manifest_staleness_reason_for_runtime, manifest_unavailable_reason_for_runtime,
};
use crate::health::{
    RetrievalStatusReport, attach_manifest_contract, probe_sidecar_health_for_runtime,
    unavailable_status_report_with_embedding_device,
};
use crate::index::{compute_sidecar_input_fingerprint, sidecar_project_id_for_runtime};
use anyhow::{Context, Result};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext,
};
use codestory_store::Store;
use codestory_workspace::{RefreshInputs, WorkspaceManifest};
use std::path::Path;

/// Observe retrieval-generation health for the selected project.
pub fn sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    status_with_runtime(
        project_root,
        storage_path,
        false,
        SidecarRuntimeConfig::for_project_auto(project_root),
    )
}

/// Validate generation identity and managed per-user embedding readiness.
pub fn strict_sidecar_status(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<RetrievalStatusReport> {
    status_with_runtime(
        project_root,
        storage_path,
        true,
        SidecarRuntimeConfig::for_project_auto(project_root),
    )
}

pub fn strict_sidecar_status_for_profile(
    project_root: &Path,
    storage_path: Option<&Path>,
    profile: SidecarProfile,
) -> Result<RetrievalStatusReport> {
    strict_sidecar_status_for_runtime(
        project_root,
        storage_path,
        SidecarRuntimeConfig::for_project_profile(Some(project_root), profile),
    )
}

pub fn strict_sidecar_status_for_runtime(
    project_root: &Path,
    storage_path: Option<&Path>,
    runtime: SidecarRuntimeConfig,
) -> Result<RetrievalStatusReport> {
    status_with_runtime(project_root, storage_path, true, runtime)
}

fn status_with_runtime(
    project_root: &Path,
    storage_path: Option<&Path>,
    strict: bool,
    runtime: SidecarRuntimeConfig,
) -> Result<RetrievalStatusReport> {
    let layout = runtime.layout.clone();
    let embedding_snapshot = crate::embeddings::embedding_engine_snapshot_for_runtime(&runtime);
    let producer_compatibility_identity =
        crate::embedded_vector::vector_producer_compatibility_identity(
            &embedding_snapshot.device,
            embedding_snapshot.identity.as_ref(),
            u32::try_from(crate::embeddings::semantic_vector_dim())
                .context("embedding dimension exceeds evidence contract")?,
        )?;
    let embedding_probe = strict.then_some(embedding_snapshot.probe);
    let embedding_device = embedding_snapshot.device;
    let project_id = sidecar_project_id_for_runtime(project_root, &runtime)?;

    if let Some(path) = storage_path.filter(|path| path.exists()) {
        let storage = Store::open_observational(path)
            .context("open storage observationally for retrieval manifest")?;
        let manifest = storage
            .get_retrieval_index_manifest(&project_id)
            .context("load retrieval manifest")?;
        if let Some(manifest) = manifest.as_ref()
            && embedding_probe
                .as_ref()
                .is_some_and(|probe| !probe.reachable)
        {
            return Ok(enrich_stored_status(
                unavailable_status_report_with_embedding_device(
                    format!(
                        "embedding_runtime_unavailable: {}",
                        embedding_probe
                            .as_ref()
                            .map(|probe| probe.detail.as_str())
                            .unwrap_or("unknown")
                    ),
                    Some(manifest.clone()),
                    &embedding_device,
                ),
                project_root,
                &storage,
                &runtime,
            ));
        }
        if strict
            && let Some(manifest) = manifest.as_ref()
            && let Some(reason) = strict_readiness_unavailable_reason_for_runtime(
                project_root,
                path,
                &storage,
                &project_id,
                manifest,
                &runtime,
                &producer_compatibility_identity,
            )
            .context("check strict retrieval readiness")?
        {
            return Ok(enrich_stored_status(
                unavailable_status_report_with_embedding_device(
                    format!("retrieval_manifest_stale: {reason}"),
                    Some(manifest.clone()),
                    &embedding_device,
                ),
                project_root,
                &storage,
                &runtime,
            ));
        }
        if let Some(manifest) = manifest.as_ref()
            && let Some(reason) =
                manifest_unavailable_reason_for_runtime(&project_id, &storage, manifest, &runtime)
        {
            return Ok(enrich_stored_status(
                unavailable_status_report_with_embedding_device(
                    reason,
                    Some(manifest.clone()),
                    &embedding_device,
                ),
                project_root,
                &storage,
                &runtime,
            ));
        }
        if let Some(manifest) = manifest.as_ref() {
            let evidence = storage
                .get_complete_index_publication()
                .context("load core publication for retrieval evidence status")?
                .context("retrieval evidence status requires a complete core publication")
                .and_then(|publication| {
                    crate::embedded_vector::validate_generation_evidence_for_publication(
                        &layout,
                        &storage,
                        manifest,
                        &publication,
                        &runtime,
                        &embedding_device,
                        embedding_snapshot.identity.as_ref(),
                    )
                    .map(|_| ())
                });
            if let Err(error) = evidence {
                return Ok(enrich_stored_status(
                    unavailable_status_report_with_embedding_device(
                        format!("retrieval_vector_evidence_unavailable: {error:#}"),
                        Some(manifest.clone()),
                        &embedding_device,
                    ),
                    project_root,
                    &storage,
                    &runtime,
                ));
            }
        }
        return Ok(enrich_stored_status(
            probe_sidecar_health_for_runtime(
                &layout,
                &project_id,
                manifest,
                &embedding_device,
                &runtime,
            ),
            project_root,
            &storage,
            &runtime,
        ));
    }

    Ok(enrich_status(
        attach_manifest_contract(
            probe_sidecar_health_for_runtime(
                &layout,
                &project_id,
                None,
                &embedding_device,
                &runtime,
            ),
            project_root,
        ),
        &runtime,
    ))
}

fn enrich_stored_status(
    report: RetrievalStatusReport,
    project_root: &Path,
    storage: &Store,
    runtime: &SidecarRuntimeConfig,
) -> RetrievalStatusReport {
    let mut report = enrich_status(attach_manifest_contract(report, project_root), runtime);
    if let Ok(stats) = storage.get_llm_symbol_doc_stats() {
        report.stored_doc_vector_producer_backend = stats.embedding_backend;
        report.stored_doc_vector_dim = stats.embedding_dim;
        report.stored_doc_vector_mixed_backends = Some(stats.mixed_embedding_backends);
    }
    report
}

fn enrich_status(
    mut report: RetrievalStatusReport,
    runtime: &SidecarRuntimeConfig,
) -> RetrievalStatusReport {
    report.query_embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    report
}

pub(crate) fn validate_strict_sidecar_readiness_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
    runtime: &SidecarRuntimeConfig,
    producer_compatibility_identity: &str,
) -> Result<()> {
    crate::embeddings::ensure_product_embedding_backend_for_runtime(runtime)
        .context("connect managed per-user embedding server")?;
    let project_id = sidecar_project_id_for_runtime(project_root, runtime)?;
    let Some(manifest) = storage
        .get_retrieval_index_manifest(&project_id)
        .context("load retrieval manifest for strict readiness")?
    else {
        return Ok(());
    };
    if let Some(reason) = strict_readiness_unavailable_reason_for_runtime(
        project_root,
        storage_path,
        storage,
        &project_id,
        &manifest,
        runtime,
        producer_compatibility_identity,
    )? {
        anyhow::bail!("retrieval_manifest_stale: {reason}");
    }
    Ok(())
}

fn strict_readiness_unavailable_reason_for_runtime(
    project_root: &Path,
    storage_path: &Path,
    storage: &Store,
    project_id: &str,
    manifest: &codestory_store::RetrievalIndexManifest,
    runtime: &SidecarRuntimeConfig,
    producer_compatibility_identity: &str,
) -> Result<Option<String>> {
    if storage
        .has_incomplete_incremental_run()
        .context("inspect incomplete incremental index marker")?
    {
        return Ok(Some("incomplete_incremental_index_run".into()));
    }
    if !manifest_has_current_sidecar_contract(project_id, manifest) {
        return Ok(None);
    }
    if let Some(reason) = manifest_staleness_reason_for_runtime(storage, manifest, runtime)
        && manifest_contract_drift_should_win(&reason)
    {
        return Ok(None);
    }

    let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let expected_doc_backend = crate::embeddings::embedding_backend_label_for_runtime(runtime);
    if let Ok(stats) = storage.get_llm_symbol_doc_stats() {
        if stats.mixed_embedding_backends {
            return Ok(Some("sidecar_symbol_docs_mixed_embedding_backends".into()));
        }
        if stats
            .embedding_backend
            .as_deref()
            .is_some_and(|backend| backend != expected_doc_backend)
        {
            return Ok(Some(format!(
                "sidecar_symbol_doc_embedding_backend_changed: stored={} current={}",
                stats.embedding_backend.as_deref().unwrap_or("<missing>"),
                expected_doc_backend
            )));
        }
    }

    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    let current_input = compute_sidecar_input_fingerprint(
        storage,
        project_root,
        storage_path,
        project_id,
        &embedding_backend,
        embedding_dim,
        producer_compatibility_identity,
    )
    .context("compute strict retrieval input fingerprint")?;
    let stored_files = storage
        .files()
        .inventory()
        .context("load indexed file inventory")?;
    if let Some(file) = stored_files.iter().find(|file| file.retry_required) {
        return Ok(Some(format!(
            "indexed_file_error_retry_required: {}",
            file.path.display()
        )));
    }
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

    let workspace = WorkspaceManifest::open_with_storage_owned_exclusions(
        project_root.to_path_buf(),
        storage_path,
    )
    .context("open workspace manifest for strict retrieval readiness")?;
    let plan = workspace
        .build_execution_plan(&RefreshInputs {
            stored_files,
            policy_exclusions: Vec::new(),
            inventory: Default::default(),
        })
        .context("build strict retrieval freshness plan")?;
    if let Some(path) = plan
        .files_to_index
        .iter()
        .find(|path| graph_indexed_source_path(path))
    {
        return Ok(Some(format!(
            "indexable_file_added_or_changed_after_retrieval_manifest: {}",
            path.display()
        )));
    }
    if let Some(file_id) = plan.files_to_remove.first() {
        return Ok(Some(format!(
            "indexed_file_removed_after_retrieval_manifest: file_id={file_id}"
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
        .and_then(|extension| extension.to_str())
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
    use tempfile::TempDir;

    #[test]
    fn strict_readiness_ignores_storage_owned_search_metadata() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("cache").join("custom-core.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("storage parent");
        let storage = Store::open(&storage_path).expect("store");
        let runtime = SidecarRuntimeConfig::local();
        let project_id =
            sidecar_project_id_for_runtime(project.path(), &runtime).expect("project id");
        let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(&runtime);
        let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
            .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
        let producer_compatibility_identity = "strict-readiness-test-producer";
        let input = compute_sidecar_input_fingerprint(
            &storage,
            project.path(),
            &storage_path,
            &project_id,
            &embedding_backend,
            embedding_dim,
            producer_compatibility_identity,
        )
        .expect("sidecar input");
        let mut manifest =
            crate::test_support::retrieval_manifest_fixture(&project_id, &input.hash);
        manifest.embedding_backend = Some(embedding_backend);
        manifest.embedding_dim = Some(embedding_dim);
        manifest.projection_count = Some(input.projection_count);
        manifest.symbol_doc_count = Some(input.symbol_doc_count);
        manifest.dense_projection_count = Some(input.dense_projection_count);
        manifest.semantic_policy_version = input.semantic_policy_version.clone();
        manifest.graph_artifact_hash = Some(input.graph_artifact_hash.clone());
        manifest.dense_reason_counts_json = Some(input.dense_reason_counts_json.clone());
        assert!(manifest_has_current_sidecar_contract(
            &project_id,
            &manifest
        ));

        let generations =
            codestory_workspace::search_generation_directory_for_storage(&storage_path);
        std::fs::create_dir_all(generations.join("generation-1")).expect("generation directory");
        std::fs::write(
            generations.join("generation-1").join("meta.json"),
            "{\"generated\":true}\n",
        )
        .expect("generation metadata");

        manifest.graph_artifact_hash = Some("stale-graph-artifact-hash".into());
        let reason = strict_readiness_unavailable_reason_for_runtime(
            project.path(),
            &storage_path,
            &storage,
            &project_id,
            &manifest,
            &runtime,
            producer_compatibility_identity,
        )
        .expect("strict readiness")
        .expect("stale sidecar input");
        assert!(reason.starts_with("sidecar_input_hash_changed:"));
        assert!(!reason.contains("indexable_file_added_or_changed"));
        assert!(!reason.contains("meta.json"));
    }
}
