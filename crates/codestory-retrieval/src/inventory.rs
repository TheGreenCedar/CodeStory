use crate::config::{SidecarRuntimeConfig, user_cache_root};
use crate::generation::{
    manifest_has_current_sidecar_contract, manifest_unavailable_reason_for_runtime,
};
use crate::retention::{
    FsGenerationRemover, GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport,
    GenerationRetentionLock, GenerationRetentionPlan, GenerationRetentionState,
    apply_generation_retention, global_generation_gc_state_file,
    plan_generation_retention_with_unrooted_state, scan_retention_protection,
};
use anyhow::{Context, Result};
use codestory_store::Store;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Read-only inventory of immutable retrieval generations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarInventoryReport {
    pub dry_run: bool,
    pub cache_root: String,
    pub generation_retention: Option<GenerationRetentionPlan>,
}

/// Result of applying the bounded generation-retention plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarGcReport {
    pub dry_run: bool,
    pub cache_root: String,
    pub generation_retention: Option<GenerationRetentionApplyReport>,
}

pub fn sidecar_inventory_with_storage(
    project_root: &Path,
    storage_path: &Path,
) -> Result<SidecarInventoryReport> {
    let cache_root = user_cache_root();
    Ok(SidecarInventoryReport {
        dry_run: true,
        cache_root: cache_root.display().to_string(),
        generation_retention: Some(generation_retention_plan_for_storage(
            project_root,
            storage_path,
            &cache_root,
        )?),
    })
}

pub fn sidecar_gc_apply_with_storage(
    project_root: &Path,
    storage_path: &Path,
) -> Result<SidecarGcReport> {
    let cache_root = user_cache_root();
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    let global_gc_state_file = global_generation_gc_state_file(&runtime);
    let _global_gc_lock =
        GenerationRetentionLock::acquire(&global_gc_state_file, GLOBAL_GENERATION_GC_LOCK_SCOPE)
            .context("coordinate retrieval cleanup with generation publication")?;
    Ok(SidecarGcReport {
        dry_run: false,
        cache_root: cache_root.display().to_string(),
        generation_retention: Some(apply_generation_retention_for_storage(
            project_root,
            storage_path,
            &cache_root,
        )?),
    })
}

fn generation_retention_plan_for_storage(
    project_root: &Path,
    storage_path: &Path,
    cache_root: &Path,
) -> Result<GenerationRetentionPlan> {
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    let project_id = crate::index::sidecar_project_id_for_runtime(project_root, &runtime)?;
    let (_lock, unrooted_state) = inventory_retention_view(&runtime, &project_id)?;
    Ok(build_generation_retention_plan(
        storage_path,
        cache_root,
        &runtime,
        &project_id,
        unrooted_state,
    ))
}

fn inventory_retention_view(
    runtime: &SidecarRuntimeConfig,
    project_id: &str,
) -> Result<(Option<GenerationRetentionLock>, GenerationRetentionState)> {
    let lock = GenerationRetentionLock::try_acquire_shared(&runtime.layout.state_file, project_id)
        .context("observe retrieval generation inventory lock")?;
    let state = if lock.is_some() {
        GenerationRetentionState::Reclaimable
    } else {
        GenerationRetentionState::Building
    };
    Ok((lock, state))
}

fn apply_generation_retention_for_storage(
    project_root: &Path,
    storage_path: &Path,
    cache_root: &Path,
) -> Result<GenerationRetentionApplyReport> {
    let runtime = SidecarRuntimeConfig::for_project_auto(project_root);
    let project_id = crate::index::sidecar_project_id_for_runtime(project_root, &runtime)?;
    let _lock = GenerationRetentionLock::acquire(&runtime.layout.state_file, &project_id)
        .context("lock retrieval generation retention apply")?;
    let plan = build_generation_retention_plan(
        storage_path,
        cache_root,
        &runtime,
        &project_id,
        GenerationRetentionState::Reclaimable,
    );
    let mut remover = FsGenerationRemover::new(&runtime.layout)?;
    Ok(apply_generation_retention(&plan, &mut remover))
}

fn build_generation_retention_plan(
    storage_path: &Path,
    cache_root: &Path,
    runtime: &SidecarRuntimeConfig,
    project_id: &str,
    unrooted_state: GenerationRetentionState,
) -> GenerationRetentionPlan {
    let layout = &runtime.layout;
    let mut protection =
        scan_retention_protection(cache_root, Some(storage_path), &layout.state_file);
    let manifest = if storage_path.is_file() {
        match Store::open(storage_path) {
            Ok(store) => match store.get_retrieval_index_manifest(project_id) {
                Ok(Some(manifest)) => {
                    record_manifest_freshness(
                        &store,
                        project_id,
                        &manifest,
                        runtime,
                        &mut protection.errors,
                    );
                    Some(manifest)
                }
                Ok(None) => None,
                Err(error) => {
                    protection
                        .errors
                        .push(format!("load active manifest for retention: {error:#}"));
                    None
                }
            },
            Err(error) => {
                protection
                    .errors
                    .push(format!("open active storage for retention: {error:#}"));
                None
            }
        }
    } else {
        None
    };

    match manifest {
        Some(manifest) if manifest_has_current_sidecar_contract(project_id, &manifest) => {
            let embedding_device =
                crate::embeddings::embedding_device_readiness_for_runtime(runtime);
            let health = crate::health::probe_sidecar_health_for_runtime(
                layout,
                project_id,
                Some(manifest),
                &embedding_device,
                runtime,
            );
            if health.retrieval_mode != "full" {
                protection.errors.push(format!(
                    "active generation is not verified full; pruning suppressed: mode={} reason={}",
                    health.retrieval_mode,
                    health.degraded_reason.as_deref().unwrap_or("unknown")
                ));
            }
        }
        Some(_) => protection.errors.push(
            "active retrieval manifest does not satisfy the current generation contract; pruning suppressed"
                .into(),
        ),
        None => protection
            .errors
            .push("active retrieval manifest is unavailable; pruning suppressed".into()),
    }
    plan_generation_retention_with_unrooted_state(layout, project_id, &protection, unrooted_state)
}

fn record_manifest_freshness(
    store: &Store,
    project_id: &str,
    manifest: &codestory_store::RetrievalIndexManifest,
    runtime: &SidecarRuntimeConfig,
    errors: &mut Vec<String>,
) {
    if let Some(reason) =
        manifest_unavailable_reason_for_runtime(project_id, store, manifest, runtime)
    {
        errors.push(format!(
            "active retrieval manifest is stale; pruning suppressed: {reason}"
        ));
    }
}
