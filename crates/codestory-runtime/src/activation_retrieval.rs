use std::fs::File;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::{fs, io};

use codestory_retrieval::{
    GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionLock, global_generation_gc_state_file,
};
use codestory_store::{CORE_DATABASE_FILE, CORE_LEASE_FILE, StorageError};
use codestory_workspace::owned_deletion::OwnedDeletionRoot;

use crate::{
    ActivationService, FinalizeIndexOutcome, QueryResult, RetainedRollbackObservation,
    RollbackActivationError, RollbackActivationOutcome, RuntimeRetrievalConfig, SidecarGcReport,
    SidecarInventoryReport,
};

impl ActivationService {
    /// Observe the retained rollback pointer without validating or mutating it.
    pub fn observe_retained_rollback_generation(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> anyhow::Result<Option<RetainedRollbackObservation>> {
        codestory_retrieval::observe_retained_rollback_generation(
            project_root,
            storage_path,
            self.controller.runtime_config.as_ref(),
        )
    }

    /// Validate and optionally activate the retained rollback generation.
    pub fn activate_retained_rollback_generation(
        &self,
        project_root: &Path,
        storage_path: &Path,
        apply: bool,
    ) -> Result<RollbackActivationOutcome, RollbackActivationError> {
        codestory_retrieval::activate_retained_rollback_generation(
            project_root,
            storage_path,
            self.controller.runtime_config.as_ref(),
            apply,
        )
    }

    /// Observe immutable retrieval generations and the current retention plan.
    pub fn retrieval_inventory(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> anyhow::Result<SidecarInventoryReport> {
        codestory_retrieval::sidecar_inventory_with_storage(project_root, storage_path)
    }

    /// Apply the retrieval owner's bounded generation-retention plan.
    pub fn apply_retrieval_gc(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> anyhow::Result<SidecarGcReport> {
        let report =
            codestory_retrieval::sidecar_gc_apply_with_storage(project_root, storage_path)?;
        apply_core_gc_for_runtime(
            self.controller.runtime_config.as_ref(),
            storage_path,
            &|| false,
        )?;
        Ok(report)
    }

    /// Execute one query with a fresh caller-isolated retrieval cache.
    pub fn execute_retrieval_query(
        &self,
        project_root: &Path,
        storage_path: &Path,
        query: &str,
        budget_ms: Option<u64>,
    ) -> anyhow::Result<QueryResult> {
        codestory_retrieval::execute_retrieval_query_with_cache_for_runtime(
            codestory_retrieval::QueryRequest {
                project_root,
                storage_path,
                query,
                budget_ms,
                cancelled: None,
            },
            &mut codestory_retrieval::RetrievalCache::new(),
            self.controller.runtime_config.as_ref(),
        )
    }

    /// Finalize retrieval artifacts for the adapter-selected runtime profile.
    pub fn finalize_retrieval_index_with_cancel(
        &self,
        project_root: &Path,
        storage_path: &Path,
        config: &RuntimeRetrievalConfig,
        cancelled: &AtomicBool,
    ) -> anyhow::Result<FinalizeIndexOutcome> {
        codestory_retrieval::finalize_index_for_runtime_with_cancel(
            project_root,
            storage_path,
            config.as_inner(),
            cancelled,
        )
    }
}

pub(crate) fn apply_core_gc_after_publication(
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    storage_path: &Path,
    cancel_token: Option<&codestory_indexer::CancellationToken>,
) {
    if let Err(error) = apply_core_gc_for_runtime(runtime, storage_path, &|| {
        cancel_token.is_some_and(codestory_indexer::CancellationToken::is_cancelled)
    }) {
        // The core is already published. A retention failure cannot turn the
        // successful commit into an apparent failed write or invite a retry.
        tracing::warn!("Core retention after publication deferred: {error}");
    }
}

fn apply_core_gc_for_runtime(
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    storage_path: &Path,
    cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<()> {
    if cancelled() {
        return Ok(());
    }
    let Some(_global) = GenerationRetentionLock::try_acquire(
        &global_generation_gc_state_file(runtime),
        GLOBAL_GENERATION_GC_LOCK_SCOPE,
    )?
    else {
        return Ok(());
    };
    let core =
        codestory_store::apply_core_retention(storage_path, cancelled, remove_owned_core_image)?;
    for error in core.errors {
        tracing::warn!("Core retention deferred candidate: {error}");
    }
    Ok(())
}

fn remove_owned_core_image(
    generations_root: &Path,
    generation_id: &str,
    validated_directory: &File,
    validated_database: &File,
) -> Result<bool, StorageError> {
    let root = OwnedDeletionRoot::open(generations_root)
        .map_err(|error| core_deletion_error("open core generation root", error))?;
    let relative = Path::new(generation_id);
    let generation = root
        .open_child_directory(relative)
        .map_err(|error| core_deletion_error("pin core generation", error))?;
    if !generation
        .matches_path(&generations_root.join(relative))
        .map_err(|error| core_deletion_error("verify core generation identity", error))?
        || !generation
            .matches_open_directory(validated_directory)
            .map_err(|error| core_deletion_error("verify validated generation handle", error))?
        || generation
            .known_regular_file_bytes(&[CORE_DATABASE_FILE, CORE_LEASE_FILE])
            .map_err(|error| core_deletion_error("verify owned core files", error))?
            .is_none()
    {
        return Ok(false);
    }
    let Some(opened_database) = generation
        .open_regular_file(Path::new(CORE_DATABASE_FILE))
        .map_err(|error| core_deletion_error("pin validated core image", error))?
    else {
        return Ok(false);
    };
    if codestory_workspace::workspace_file_identity(&opened_database)
        .map_err(|error| core_deletion_error("identify pinned core image", error))?
        != codestory_workspace::workspace_file_identity(validated_database)
            .map_err(|error| core_deletion_error("identify validated core image", error))?
    {
        return Ok(false);
    }
    let removed = generation
        .remove(Path::new(CORE_DATABASE_FILE))
        .map_err(|error| core_deletion_error("remove obsolete core image", error))?;
    if !removed {
        return Ok(false);
    }
    match fs::symlink_metadata(generations_root.join(relative).join(CORE_DATABASE_FILE)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(StorageError::Other(
                "Obsolete core image remains after removal".into(),
            ));
        }
        Err(error) => return Err(core_deletion_error("verify obsolete core removal", error)),
    }
    generation
        .remove(Path::new(CORE_LEASE_FILE))
        .map_err(|error| core_deletion_error("remove retired core lease", error))?;
    drop(generation);
    // On Unix an empty directory is deliberately retained because its final
    // pathname removal cannot be tied to the already-pinned directory handle.
    root.remove_empty_directory(relative)
        .map_err(|error| core_deletion_error("remove empty core directory", error))?;
    Ok(true)
}

fn core_deletion_error(action: &str, error: io::Error) -> StorageError {
    StorageError::Other(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Runtime, RuntimeProcessConfig};
    use codestory_contracts::workspace::SourceIndexPolicy;
    use codestory_store::{
        CorePublicationLayout, IndexPublicationMode, IndexPublicationRecord,
        RetrievalIndexManifest, SnapshotStore, SourcePolicyExclusionPolicyIdentity, Store,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::tempdir;

    fn error_chain(error: &anyhow::Error) -> Vec<String> {
        error.chain().map(ToString::to_string).collect()
    }

    fn publish_owned_core_generations(project: &Path, storage_path: &Path, count: u64) {
        for generation in 1..=count {
            publish_owned_core_generation(project, storage_path, generation);
        }
    }

    fn publish_owned_core_generation(project: &Path, storage_path: &Path, generation: u64) {
        let identity = codestory_workspace::project_identity_v3(project);
        let policy = SourceIndexPolicy::default();
        let staged_path = SnapshotStore::staged_path(storage_path).expect("stage path");
        let mut stage = Store::open_build(&staged_path).expect("open stage");
        let publication = IndexPublicationRecord {
            generation,
            generation_id: format!("owned-core-{generation}"),
            run_id: format!("run-{generation}"),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: generation as i64,
        };
        stage
            .publish_structural_text_unit_generation(&publication)
            .expect("publish structural identity");
        stage
            .publish_source_policy_exclusion_generation(
                &publication,
                &identity.project_id,
                &identity.workspace_id,
                SourcePolicyExclusionPolicyIdentity::new(
                    &policy.policy_version,
                    policy.byte_cap,
                    policy.structural_unit_cap,
                ),
                &[],
            )
            .expect("publish source-policy identity");
        stage
            .put_index_publication(&publication)
            .expect("publish core identity");
        stage.finalize_staged_snapshot().expect("finalize stage");
        drop(stage);
        Store::promote_staged_snapshot(&staged_path, storage_path).expect("promote complete core");
    }

    fn retained_manifest(project_id: &str) -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: project_id.into(),
            lexical_version: "lexical-v1".into(),
            semantic_generation: "semantic-one".into(),
            scip_revision: None,
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: None,
            embedding_dim: None,
            sidecar_schema_version: Some(2),
            sidecar_input_hash: Some("retained-input-hash".into()),
            sidecar_generation: Some("retrieval-one".into()),
            projection_count: Some(0),
            symbol_doc_count: Some(0),
            dense_projection_count: Some(0),
            semantic_policy_version: Some("graph_first_v1".into()),
            graph_artifact_hash: Some("graph".into()),
            dense_reason_counts_json: Some("{}".into()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        }
    }

    #[test]
    fn facade_preserves_observation_query_and_finalize_error_contracts() {
        let project = tempdir().expect("operation project");
        let storage = tempdir().expect("operation storage");
        let storage_path = storage.path().join("codestory.db");
        let raw = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
            Some(project.path()),
            codestory_retrieval::SidecarProfile::Local,
        );
        let runtime = Runtime::new_with_process_config(RuntimeProcessConfig::new(
            raw.clone(),
            SourceIndexPolicy::default(),
        ));
        let facade = runtime.activation_service();

        let direct_observation = codestory_retrieval::observe_retained_rollback_generation(
            project.path(),
            &storage_path,
            &raw,
        )
        .expect("direct observation");
        let facade_observation = facade
            .observe_retained_rollback_generation(project.path(), &storage_path)
            .expect("facade observation");
        assert_eq!(facade_observation, direct_observation);

        let direct_query = codestory_retrieval::execute_retrieval_query_with_cache_for_runtime(
            codestory_retrieval::QueryRequest {
                project_root: project.path(),
                storage_path: &storage_path,
                query: "missing",
                budget_ms: Some(37),
                cancelled: None,
            },
            &mut codestory_retrieval::RetrievalCache::new(),
            &raw,
        )
        .expect_err("missing storage must fail direct query");
        let facade_query = facade
            .execute_retrieval_query(project.path(), &storage_path, "missing", Some(37))
            .expect_err("missing storage must fail facade query");
        assert_eq!(error_chain(&facade_query), error_chain(&direct_query));

        let selected: RuntimeRetrievalConfig = raw.clone().into();
        let cancelled = AtomicBool::new(true);
        let direct_finalize = codestory_retrieval::finalize_index_for_runtime_with_cancel(
            project.path(),
            &storage_path,
            &raw,
            &cancelled,
        )
        .expect_err("cancelled direct finalize must fail");
        let facade_finalize = facade
            .finalize_retrieval_index_with_cancel(
                project.path(),
                &storage_path,
                &selected,
                &cancelled,
            )
            .expect_err("cancelled facade finalize must fail");
        assert_eq!(error_chain(&facade_finalize), error_chain(&direct_finalize));
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn facade_preserves_rollback_refusal_and_inventory_results() {
        let cache = tempdir().expect("operation cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("operation project");
            let storage = tempdir().expect("operation storage");
            let storage_path = storage.path().join("codestory.db");
            let raw = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            );
            let runtime = Runtime::new_with_process_config(RuntimeProcessConfig::new(
                raw.clone(),
                SourceIndexPolicy::default(),
            ));
            let facade = runtime.activation_service();

            let direct_refusal = codestory_retrieval::activate_retained_rollback_generation(
                project.path(),
                &storage_path,
                &raw,
                false,
            )
            .expect_err("missing publication must refuse direct activation");
            let facade_refusal = facade
                .activate_retained_rollback_generation(project.path(), &storage_path, false)
                .expect_err("missing publication must refuse facade activation");
            assert_eq!(facade_refusal.code(), direct_refusal.code());
            assert_eq!(facade_refusal.to_string(), direct_refusal.to_string());

            let direct_inventory =
                codestory_retrieval::sidecar_inventory_with_storage(project.path(), &storage_path)
                    .expect("direct inventory");
            let facade_inventory = facade
                .retrieval_inventory(project.path(), &storage_path)
                .expect("facade inventory");
            assert_eq!(facade_inventory, direct_inventory);

            let direct_gc =
                codestory_retrieval::sidecar_gc_apply_with_storage(project.path(), &storage_path)
                    .expect("direct gc");
            let facade_gc = facade
                .apply_retrieval_gc(project.path(), &storage_path)
                .expect("facade gc");
            assert_eq!(facade_gc, direct_gc);
        });
    }

    #[test]
    fn retrieval_gc_reclaims_an_obsolete_owned_core_after_real_publications() {
        let cache = tempdir().expect("isolated cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("project");
            let storage_path = cache.path().join("codestory.db");
            let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
            let policy = SourceIndexPolicy::default();
            publish_owned_core_generations(project.path(), &storage_path, 3);

            let pointer = layout.read_pointer().expect("pointer").expect("published");
            assert_eq!(pointer.active.generation_id, "owned-core-3");
            assert_eq!(
                pointer
                    .rollback
                    .as_ref()
                    .map(|value| value.generation_id.as_str()),
                Some("owned-core-2")
            );
            let obsolete = layout
                .generation_database_path("owned-core-1")
                .expect("obsolete core path");
            assert!(
                obsolete.is_file(),
                "baseline must contain an obsolete image"
            );

            let runtime = Runtime::new_with_process_config(RuntimeProcessConfig::new(
                codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                    Some(project.path()),
                    codestory_retrieval::SidecarProfile::Local,
                ),
                policy,
            ));
            runtime
                .activation_service()
                .apply_retrieval_gc(project.path(), &storage_path)
                .expect("apply the existing production GC entrypoint");

            assert!(
                !obsolete.is_file(),
                "obsolete core image remains after the production GC entrypoint"
            );
            assert!(
                layout
                    .resolve_generation_database("owned-core-2")
                    .expect("rollback still readable")
                    .is_file()
            );
            assert!(
                layout
                    .resolve_generation_database("owned-core-3")
                    .expect("active still readable")
                    .is_file()
            );
        });
    }

    #[test]
    fn core_gc_reclaims_unpinned_old_image_while_another_old_reader_is_alive() {
        let cache = tempdir().expect("isolated cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("project");
            let storage_path = cache.path().join("codestory.db");
            publish_owned_core_generations(project.path(), &storage_path, 4);
            let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
            let old_one = layout
                .generation_database_path("owned-core-1")
                .expect("old one");
            let old_two = layout
                .generation_database_path("owned-core-2")
                .expect("old two");
            let pinned = Store::open_immutable_generation(&old_two).expect("pin old reader");
            let runtime = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            );
            apply_core_gc_for_runtime(&runtime, &storage_path, &|| false)
                .expect("first retention pass");
            assert!(!old_one.exists(), "unrelated old image should be reclaimed");
            assert!(old_two.is_file(), "pinned old image must survive");
            assert!(
                pinned
                    .get_complete_index_publication()
                    .expect("pinned read")
                    .is_some()
            );
            drop(pinned);
            apply_core_gc_for_runtime(&runtime, &storage_path, &|| false)
                .expect("retry retention pass");
            assert!(!old_two.exists(), "released old image should be reclaimed");
            assert!(
                layout
                    .generation_database_path("owned-core-3")
                    .unwrap()
                    .is_file(),
                "rollback must remain"
            );
            assert!(
                layout
                    .generation_database_path("owned-core-4")
                    .unwrap()
                    .is_file(),
                "active must remain"
            );
        });
    }

    #[test]
    fn observational_old_core_open_does_not_provision_retention_locks() {
        let cache = tempdir().expect("isolated cache");
        let project = tempdir().expect("project");
        let storage_path = cache.path().join("codestory.db");
        publish_owned_core_generation(project.path(), &storage_path, 1);
        let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
        let image = layout.generation_database_path("owned-core-1").unwrap();
        let acquisition = layout.root().join("acquisition.lock");
        let lease = image.parent().unwrap().join(CORE_LEASE_FILE);
        fs::remove_file(&acquisition).expect("simulate old unprovisioned cache");
        fs::remove_file(&lease).expect("simulate old unprovisioned generation");
        let before = fs::read(&image).expect("core bytes");
        let opened = Store::open_observational(&storage_path).expect("observe old core");
        assert!(opened.get_complete_index_publication().unwrap().is_some());
        drop(opened);
        assert_eq!(fs::read(&image).unwrap(), before);
        assert!(
            !acquisition.exists(),
            "observation cannot create coordination state"
        );
        assert!(!lease.exists(), "observation cannot provision an old image");
        assert!(!image.with_file_name("codestory.db-shm").exists());
        assert!(!image.with_file_name("codestory.db-wal").exists());
    }

    #[test]
    fn core_gc_retains_retrieval_bound_predecessor_but_reclaims_other_old_core() {
        let cache = tempdir().expect("isolated cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("project");
            let storage_path = cache.path().join("codestory.db");
            publish_owned_core_generation(project.path(), &storage_path, 1);
            let project_id = codestory_workspace::project_identity_v3(project.path()).project_id;
            Store::open(&storage_path)
                .expect("open bound core")
                .upsert_retrieval_index_manifest(&retained_manifest(&project_id))
                .expect("bind retrieval to first core");
            for generation in 2..=4 {
                publish_owned_core_generation(project.path(), &storage_path, generation);
            }
            let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
            let retained = layout.generation_database_path("owned-core-1").unwrap();
            let unbound = layout.generation_database_path("owned-core-2").unwrap();
            let runtime = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            );
            apply_core_gc_for_runtime(&runtime, &storage_path, &|| false).expect("retention pass");
            assert!(
                retained.is_file(),
                "retrieval-bound predecessor must survive"
            );
            assert!(!unbound.exists(), "unbound old core should be reclaimed");
            assert!(
                layout
                    .generation_database_path("owned-core-3")
                    .unwrap()
                    .is_file()
            );
            assert!(
                layout
                    .generation_database_path("owned-core-4")
                    .unwrap()
                    .is_file()
            );
        });
    }

    #[test]
    fn core_gc_preserves_unknown_neighbors_and_refuses_malformed_retrieval_roots() {
        let cache = tempdir().expect("isolated cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("project");
            let storage_path = cache.path().join("codestory.db");
            publish_owned_core_generations(project.path(), &storage_path, 3);
            let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
            let old = layout
                .generation_database_path("owned-core-1")
                .expect("old image");
            let neighbor = old.parent().unwrap().join("unknown.txt");
            fs::write(&neighbor, b"keep").expect("unknown neighbor");
            let runtime = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            );
            apply_core_gc_for_runtime(&runtime, &storage_path, &|| false)
                .expect("unknown neighbor pass");
            assert!(old.is_file());
            assert_eq!(fs::read(&neighbor).unwrap(), b"keep");
            fs::remove_file(&neighbor).expect("remove test neighbor");
            let pointer_path = layout.publication_path();
            let valid_pointer = fs::read(&pointer_path).expect("saved pointer");
            fs::write(&pointer_path, b"not a core pointer").expect("malformed pointer");
            assert!(apply_core_gc_for_runtime(&runtime, &storage_path, &|| false).is_err());
            assert!(
                old.is_file(),
                "malformed core pointer must suppress pruning"
            );
            fs::write(&pointer_path, valid_pointer).expect("restore pointer");
            fs::write(layout.retrieval_publication_path(), b"not sqlite")
                .expect("malformed retained root");
            assert!(apply_core_gc_for_runtime(&runtime, &storage_path, &|| false).is_err());
            assert!(
                old.is_file(),
                "unknown retrieval roots must suppress pruning"
            );
        });
    }

    #[test]
    fn core_gc_rejects_same_name_replacement_after_validation() {
        let cache = tempdir().expect("isolated cache");
        let project = tempdir().expect("project");
        let storage_path = cache.path().join("codestory.db");
        publish_owned_core_generations(project.path(), &storage_path, 3);
        let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
        let old_dir = layout
            .generation_directory("owned-core-1")
            .expect("old directory");
        let moved = layout.generations_root().join("moved-original");
        let old = old_dir.join(CORE_DATABASE_FILE);
        let report = codestory_store::apply_core_retention(
            &storage_path,
            &|| false,
            |root, generation, expected_dir, expected_db| {
                fs::rename(&old_dir, &moved).expect("replace old directory at callback seam");
                fs::create_dir(&old_dir).expect("replacement directory");
                fs::copy(moved.join(CORE_DATABASE_FILE), &old).expect("replacement image");
                fs::write(old_dir.join(CORE_LEASE_FILE), b"").expect("replacement marker");
                remove_owned_core_image(root, generation, expected_dir, expected_db)
            },
        )
        .expect("retention pass");
        assert_eq!(report.reclaimed_images, 0);
        assert!(old.is_file(), "same-name replacement must survive");
        assert!(moved.join(CORE_DATABASE_FILE).is_file());
    }

    #[test]
    fn core_gc_cancellation_and_unprovisioned_legacy_image_leave_bytes_then_retry() {
        let cache = tempdir().expect("isolated cache");
        let project = tempdir().expect("project");
        let storage_path = cache.path().join("codestory.db");
        publish_owned_core_generations(project.path(), &storage_path, 3);
        let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
        let old = layout.generation_database_path("owned-core-1").unwrap();
        let original = fs::read(&old).expect("old core bytes");
        let cancelled =
            codestory_store::apply_core_retention(&storage_path, &|| true, |_, _, _, _| {
                panic!("cancelled retention cannot remove an image")
            })
            .expect("cancelled pass");
        assert_eq!(cancelled.reclaimed_images, 0);
        assert_eq!(fs::read(&old).unwrap(), original);
        let cancelled_publication = codestory_indexer::CancellationToken::new();
        cancelled_publication.cancel();
        let runtime = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
            Some(project.path()),
            codestory_retrieval::SidecarProfile::Local,
        );
        apply_core_gc_after_publication(&runtime, &storage_path, Some(&cancelled_publication));
        assert_eq!(
            fs::read(&old).unwrap(),
            original,
            "postcommit cancellation should defer cleanup without changing the publication"
        );
        fs::remove_file(old.parent().unwrap().join(CORE_LEASE_FILE))
            .expect("simulate pre-provisioned legacy generation");
        let unprovisioned =
            codestory_store::apply_core_retention(&storage_path, &|| false, |_, _, _, _| {
                panic!("unprovisioned generation cannot be removed")
            })
            .expect("legacy pass");
        assert_eq!(unprovisioned.unprovisioned_generations, 1);
        assert_eq!(fs::read(&old).unwrap(), original);
        fs::write(old.parent().unwrap().join(CORE_LEASE_FILE), b"")
            .expect("restore owned lease for retry");
        codestory_store::apply_core_retention(&storage_path, &|| false, remove_owned_core_image)
            .expect("retry after restored ownership");
        assert!(!old.exists());
    }

    #[cfg(unix)]
    #[test]
    fn core_gc_preserves_symlink_replacement_and_outside_target() {
        use std::os::unix::fs::symlink;

        let cache = tempdir().expect("isolated cache");
        let project = tempdir().expect("project");
        let storage_path = cache.path().join("codestory.db");
        publish_owned_core_generations(project.path(), &storage_path, 3);
        let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
        let old = layout.generation_database_path("owned-core-1").unwrap();
        let outside = project.path().join("outside-sentinel");
        fs::write(&outside, b"keep-outside").expect("outside target");
        fs::remove_file(&old).expect("replace old core file");
        symlink(&outside, &old).expect("symlink replacement");
        let report =
            codestory_store::apply_core_retention(&storage_path, &|| false, |_, _, _, _| {
                panic!("symlink candidate cannot reach deletion callback")
            })
            .expect("safe refusal");
        assert_eq!(report.reclaimed_images, 0);
        assert!(fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&outside).unwrap(), b"keep-outside");
    }

    #[test]
    fn core_gc_defers_behind_a_live_retrieval_global_reader_then_retries() {
        let cache = tempdir().expect("isolated cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("project");
            let storage_path = cache.path().join("codestory.db");
            publish_owned_core_generations(project.path(), &storage_path, 3);
            let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
            let old = layout.generation_database_path("owned-core-1").unwrap();
            let runtime = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            );
            let state = global_generation_gc_state_file(&runtime);
            let reader =
                GenerationRetentionLock::acquire_shared(&state, GLOBAL_GENERATION_GC_LOCK_SCOPE)
                    .expect("retrieval reader");
            apply_core_gc_for_runtime(&runtime, &storage_path, &|| false).expect("deferred pass");
            assert!(old.is_file());
            drop(reader);
            apply_core_gc_for_runtime(&runtime, &storage_path, &|| false).expect("retry pass");
            assert!(!old.is_file());
        });
    }

    #[test]
    fn ordinary_full_publication_schedules_core_retention_after_commit() {
        let cache = tempdir().expect("isolated cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("project");
            fs::write(
                project.path().join("lib.rs"),
                "pub fn answer() -> u32 { 42 }\n",
            )
            .expect("fixture source");
            let storage_path = cache.path().join("codestory.db");
            let controller = crate::AppController::new_with_config(
                codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                    Some(project.path()),
                    codestory_retrieval::SidecarProfile::Local,
                ),
            );
            controller
                .open_project_summary_with_storage_path(
                    project.path().to_path_buf(),
                    storage_path.clone(),
                )
                .expect("open project");
            let layout = CorePublicationLayout::from_storage_path(&storage_path).expect("layout");
            let mut first = None;
            let mut first_image = None;
            for run in 0..3 {
                controller
                    .run_indexing_blocking_without_runtime_refresh(
                        codestory_contracts::api::IndexMode::Full,
                    )
                    .expect("full core publication");
                if run == 0 {
                    first = layout
                        .read_pointer()
                        .unwrap()
                        .map(|pointer| pointer.active.generation_id);
                    first_image = Some(
                        fs::read(
                            layout
                                .generation_database_path(first.as_deref().unwrap())
                                .unwrap(),
                        )
                        .expect("first immutable image"),
                    );
                } else if run == 1 {
                    let first_path = layout
                        .generation_database_path(first.as_deref().unwrap())
                        .unwrap();
                    assert_eq!(
                        fs::read(&first_path).expect("predecessor image"),
                        first_image.as_ref().unwrap().as_slice(),
                        "copy-forward must not mutate the published predecessor"
                    );
                    for suffix in ["-wal", "-shm"] {
                        assert!(
                            fs::symlink_metadata(format!("{}{suffix}", first_path.display()))
                                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound),
                            "copy-forward must not create predecessor SQLite sidecars"
                        );
                    }
                }
            }
            let first = first.expect("first core generation");
            let pointer = layout.read_pointer().unwrap().expect("latest publication");
            assert_ne!(pointer.active.generation_id, first);
            assert_ne!(pointer.rollback.unwrap().generation_id, first);
            assert!(
                !layout.generation_database_path(&first).unwrap().exists(),
                "ordinary publication must trigger a best-effort retention pass"
            );
        });
    }
}
