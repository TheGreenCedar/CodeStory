use crate::config::{SidecarRuntimeConfig, user_cache_root};
use crate::generation::{
    manifest_has_current_sidecar_contract, manifest_unavailable_reason_for_runtime,
};
use crate::retention::{
    FsGenerationRemover, GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionApplyReport,
    GenerationRetentionLock, GenerationRetentionPlan, GenerationRetentionState,
    ObservedRetentionLock, apply_generation_retention, global_generation_gc_state_file,
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
) -> Result<(ObservedRetentionLock, GenerationRetentionState)> {
    // Observation only: a dry-run inventory must not be the reason the
    // retention directory or its lock file first exists.
    let lock = GenerationRetentionLock::observe_shared(&runtime.layout.state_file, project_id)
        .context("observe retrieval generation inventory lock")?;
    let state = if lock.is_quiescent() {
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
        // Planning retention is observation, including of the caller's own
        // store: a plan must never be the thing that migrates or recovers the
        // database it is reasoning about.
        match Store::open_observational(storage_path) {
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
                protection.protection_incomplete = true;
                protection
                    .errors
                    .push(format!("observe active storage for retention: {error:#}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::with_test_cache_root;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Every path below `root`, mapped to the exact bytes it holds. A missing
    /// entry, an added entry, or one changed byte all show up as an inequality.
    fn snapshot_tree(root: &Path) -> BTreeMap<String, String> {
        let mut entries = BTreeMap::new();
        collect_tree(root, root, &mut entries);
        entries
    }

    fn collect_tree(root: &Path, dir: &Path, entries: &mut BTreeMap<String, String>) {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("entry is below the snapshot root")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("inspect snapshot entry");
            if metadata.is_dir() {
                entries.insert(format!("{relative}/"), "<dir>".to_string());
                collect_tree(root, &path, entries);
            } else {
                let bytes = std::fs::read(&path).expect("read snapshot entry");
                let digest = Sha256::digest(&bytes);
                entries.insert(relative, format!("{}:{digest:x}", bytes.len()));
            }
        }
    }

    fn create_store(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("storage parent"))
            .expect("create storage parent");
        let store = Store::open(path).expect("create store");
        drop(store);
    }

    fn schema_version(path: &Path) -> i64 {
        let connection = rusqlite::Connection::open(path).expect("open sqlite");
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read user_version");
        drop(connection);
        version
    }

    fn set_schema_version(path: &Path, version: i64) {
        let connection = rusqlite::Connection::open(path).expect("open sqlite");
        connection
            .pragma_update(None, "user_version", version)
            .expect("write user_version");
        drop(connection);
    }

    /// A version-skewed neighbour: another project's cache left at the schema
    /// an older binary wrote. The activating open path would migrate it in
    /// place; an inventory has no right to.
    fn skewed_neighbour_cache(cache_root: &Path, name: &str) -> PathBuf {
        let storage = cache_root.join(name).join("codestory.db");
        create_store(&storage);
        let current = schema_version(&storage);
        set_schema_version(&storage, current - 1);
        storage
    }

    #[test]
    fn dry_run_inventory_leaves_the_cache_tree_byte_identical() {
        let cache = tempdir().expect("cache root");
        let project = tempdir().expect("project root");
        let workspace_id = codestory_workspace::workspace_id_v3_for_root(project.path());
        let active_storage = cache.path().join(&workspace_id).join("codestory.db");
        create_store(&active_storage);
        let neighbour = skewed_neighbour_cache(cache.path(), "00112233445566aa");
        let active_schema = schema_version(&active_storage);

        let before = snapshot_tree(cache.path());
        let report = with_test_cache_root(cache.path(), || {
            sidecar_inventory_with_storage(project.path(), &active_storage)
                .expect("dry-run inventory")
        });
        let after = snapshot_tree(cache.path());

        assert_eq!(
            before, after,
            "a dry-run inventory must not add, remove, or rewrite one byte of the cache tree"
        );
        assert_eq!(
            schema_version(&neighbour),
            active_schema - 1,
            "the neighbour cache must still be at the schema its owner left it on"
        );
        let plan = report
            .generation_retention
            .as_ref()
            .expect("inventory reports a retention plan");
        assert!(
            plan.pruning_suppressed,
            "a cache holding an unobservable neighbour must not prune"
        );
        assert!(
            plan.errors
                .iter()
                .any(|error| error.contains("observe retrieval manifests in")
                    && error.contains("00112233445566aa")),
            "the skewed neighbour must be reported as unobservable, got {:?}",
            plan.errors
        );
    }

    #[test]
    fn dry_run_inventory_does_not_create_the_retention_lock_it_observes() {
        let cache = tempdir().expect("cache root");
        let project = tempdir().expect("project root");
        let workspace_id = codestory_workspace::workspace_id_v3_for_root(project.path());
        let active_storage = cache.path().join(&workspace_id).join("codestory.db");
        create_store(&active_storage);
        let retention_dir = cache.path().join("retention");

        with_test_cache_root(cache.path(), || {
            sidecar_inventory_with_storage(project.path(), &active_storage)
                .expect("dry-run inventory")
        });

        assert!(
            !retention_dir.exists(),
            "observing the retention lock must not be what creates {}",
            retention_dir.display()
        );
    }
}
