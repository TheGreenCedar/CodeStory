//! Publication location is producer-owned; request policy stays consumer-owned.
//! Legacy resolution is finite and metadata-only. Full payload/GPU validation
//! remains at the existing readiness/admission boundary.
use crate::config::SidecarRuntimeConfig;
use crate::embedded_vector::EmbeddedVectorIndex;
use crate::retention::{
    GLOBAL_GENERATION_GC_LOCK_SCOPE, GenerationRetentionLock, ObservedRetentionLock,
    global_generation_gc_state_file, read_retention_marker,
};
use anyhow::{Context, Result, bail};
use codestory_store::{RetrievalArtifactScope, RetrievalIndexManifest, Store};
use std::path::Path;

pub(crate) fn resolve_publication_runtime(
    project_root: &Path,
    storage: &Store,
    manifest: &RetrievalIndexManifest,
    consumer: &SidecarRuntimeConfig,
) -> Result<SidecarRuntimeConfig> {
    if let Some(scope) = manifest.artifact_scope.as_ref() {
        return consumer.with_publication_artifact_scope(scope);
    }
    resolve_legacy_runtime(project_root, storage, manifest, consumer, false)
}

fn resolve_legacy_runtime(
    project_root: &Path,
    storage: &Store,
    manifest: &RetrievalIndexManifest,
    consumer: &SidecarRuntimeConfig,
    writer: bool,
) -> Result<SidecarRuntimeConfig> {
    // An owner registration authenticates location even when reusable bytes
    // are damaged. It authorizes a writer to rebuild, never a reader to admit
    // those damaged artifacts. No arbitrary namespace discovery is allowed.
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    let selected = if writer {
        authenticate_registered_owner(project_root, manifest, consumer)
            .or_else(|_| authenticate_locator(storage, manifest, consumer))
    } else {
        authenticate_locator(storage, manifest, consumer)
    };
    match selected {
        Ok(()) => candidates.push(consumer.clone()),
        Err(error) => errors.push(format!("selected: {error:#}")),
    }
    let local = consumer.with_publication_artifact_scope(&RetrievalArtifactScope::Local)?;
    if !same_location(consumer, &local) {
        let validation =
            authenticate_registered_owner(project_root, manifest, &local).and_then(|()| {
                if writer {
                    Ok(())
                } else {
                    authenticate_locator(storage, manifest, &local)
                }
            });
        match validation {
            Ok(()) => candidates.push(local),
            Err(error) => errors.push(format!("local: {error:#}")),
        }
    }
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => bail!(
            "retrieval_publication_origin_unavailable: {}",
            errors.join("; ")
        ),
        _ => bail!(
            "retrieval_publication_origin_ambiguous: multiple authenticated legacy artifact scopes"
        ),
    }
}

fn same_location(left: &SidecarRuntimeConfig, right: &SidecarRuntimeConfig) -> bool {
    codestory_workspace::same_workspace_path(
        &left.layout.semantic_data_dir,
        &right.layout.semantic_data_dir,
    )
}

fn authenticate_registered_owner(
    project_root: &Path,
    manifest: &RetrievalIndexManifest,
    runtime: &SidecarRuntimeConfig,
) -> Result<()> {
    let identity = runtime.validated_project_identity(project_root)?;
    let marker = read_retention_marker(&runtime.layout.state_file, &identity.workspace_id)?
        .context("legacy local publication has no owner marker")?;
    let owner = marker
        .owner
        .as_ref()
        .context("legacy local publication has no registered owner")?;
    if marker.schema_version != crate::retention::RETENTION_MARKER_SCHEMA_V2
        || marker.workspace_id != identity.workspace_id
        || marker.project_id != manifest.project_id
        || owner.workspace_id != identity.workspace_id
        || !codestory_workspace::same_workspace_path(Path::new(&owner.project_root), project_root)
        || !(marker.active == *manifest
            || marker
                .rollback
                .as_ref()
                .is_some_and(|r| r.manifest == *manifest))
    {
        bail!("legacy local publication owner/manifest mismatch");
    }
    Ok(())
}

fn authenticate_locator(
    storage: &Store,
    manifest: &RetrievalIndexManifest,
    runtime: &SidecarRuntimeConfig,
) -> Result<()> {
    let generation = manifest
        .sidecar_generation
        .as_deref()
        .context("publication generation missing")?;
    let input = manifest
        .sidecar_input_hash
        .as_deref()
        .context("publication input hash missing")?;
    for component in [generation, manifest.semantic_generation.as_str()] {
        if component.is_empty()
            || component.len() > 256
            || !component
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("unsafe publication artifact identity");
        }
    }
    let path = runtime
        .layout
        .semantic_data_dir
        .join("collections")
        .join(&manifest.semantic_generation)
        .join("vector-generation-manifest.json");
    // Reject links at the artifact boundary; metadata cannot authorize an
    // arbitrary path outside the process-captured cache root.
    let relative = path
        .strip_prefix(&runtime.cache_root)
        .context("publication artifact is outside captured cache root")?;
    let mut current = runtime.cache_root.clone();
    for component in relative.components() {
        current.push(component);
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            bail!("publication artifact path contains a symlink");
        }
    }
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        bail!("publication locator is not a regular file");
    }
    let vector = EmbeddedVectorIndex::load_generation_manifest(
        &runtime.layout,
        &manifest.semantic_generation,
    )?;
    let core = storage
        .get_complete_index_publication()?
        .context("publication locator requires complete Core")?;
    let evidence = &vector.evidence.publication;
    let count = manifest
        .dense_projection_count
        .or(manifest.projection_count)
        .and_then(|n| u64::try_from(n).ok())
        .context("publication dense count is invalid")?;
    if evidence.core_generation_id != core.generation_id
        || evidence.core_run_id != core.run_id
        || evidence.retrieval_generation != generation
        || evidence.retrieval_input_hash != input
        || evidence.semantic_generation != manifest.semantic_generation
        || vector.vectors.generation != generation
        || vector.vectors.input_hash != input
        || vector.vectors.embedding_backend
            != manifest.embedding_backend.as_deref().unwrap_or_default()
        || Some(i32::try_from(vector.vectors.embedding_dim)?) != manifest.embedding_dim
        || vector.vectors.point_count != count
    {
        bail!("publication locator does not authenticate exact Core/vector generation");
    }
    Ok(())
}

/// Observational readers hold existing fences without creating or repairing state.
pub(crate) fn observe_publication_global(
    runtime: &SidecarRuntimeConfig,
) -> Result<ObservedRetentionLock> {
    observe(
        &global_generation_gc_state_file(runtime),
        GLOBAL_GENERATION_GC_LOCK_SCOPE,
    )
}
pub(crate) fn observe_publication_origin(
    runtime: &SidecarRuntimeConfig,
    project_id: &str,
) -> Result<ObservedRetentionLock> {
    observe(&runtime.layout.state_file, project_id)
}
fn observe(state: &Path, scope: &str) -> Result<ObservedRetentionLock> {
    let lock = GenerationRetentionLock::observe_shared(state, scope)?;
    if !lock.is_quiescent() {
        bail!("retrieval publication retention is contended");
    }
    Ok(lock)
}

pub(crate) fn runtime_for_stored_publication(
    project_root: &Path,
    storage_path: &Path,
    consumer: &SidecarRuntimeConfig,
) -> Result<SidecarRuntimeConfig> {
    if !codestory_store::core_database_exists(storage_path)? {
        return Ok(consumer.clone());
    }
    let storage = Store::open_observational(storage_path)?;
    let project_id = crate::index::sidecar_project_id_for_runtime(project_root, consumer)?;
    match storage.get_retrieval_index_manifest(&project_id)? {
        Some(manifest) => resolve_publication_runtime(project_root, &storage, &manifest, consumer),
        None => Ok(consumer.clone()),
    }
}

/// Finalizers select the stored producer while holding the captured-root
/// global GC fence and the project writer fence. They retain that binding for
/// reuse, new artifacts, pointer publication, marker updates and cleanup.
pub(crate) fn runtime_for_publication_writer(
    project_root: &Path,
    storage_path: &Path,
    consumer: &SidecarRuntimeConfig,
) -> Result<(SidecarRuntimeConfig, RetrievalArtifactScope)> {
    if !codestory_store::core_database_exists(storage_path)?
        || crate::index::external_retrieval_publication_is_physically_absent(storage_path)?
    {
        return Ok((consumer.clone(), consumer.publication_artifact_scope()?));
    }
    let pointer = Store::open_observational(storage_path)?;
    let project_id = crate::index::sidecar_project_id_for_runtime(project_root, consumer)?;
    let Some(bound) = pointer
        .get_bound_retrieval_index_manifest(&project_id)
        .context("load previous retrieval_index_manifest")?
    else {
        return Ok((consumer.clone(), consumer.publication_artifact_scope()?));
    };
    // A newer Core may already be staged/published. Authenticate the stored
    // predecessor against its own Core, never substitute the current Core.
    let core_path = codestory_store::resolve_core_generation_database_path(
        storage_path,
        &bound.core.generation_id,
    )?;
    let core = Store::open_read_only(&core_path)?;
    let complete = core
        .get_complete_index_publication()?
        .context("writer predecessor Core is incomplete")?;
    if complete.generation_id != bound.core.generation_id || complete.run_id != bound.core.run_id {
        bail!("writer predecessor Core binding does not match complete publication");
    }
    let resolved = match bound.manifest.artifact_scope.as_ref() {
        Some(scope) => consumer.with_publication_artifact_scope(scope)?,
        None => resolve_legacy_runtime(project_root, &core, &bound.manifest, consumer, true)?,
    };
    let scope = match bound.manifest.artifact_scope {
        Some(scope) => scope,
        None => {
            let local = consumer.with_publication_artifact_scope(&RetrievalArtifactScope::Local)?;
            if same_location(&resolved, &local) {
                RetrievalArtifactScope::Local
            } else {
                consumer.publication_artifact_scope()?
            }
        }
    };
    Ok((resolved, scope))
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::retention::{
        GenerationRetentionLease, GenerationRetentionMarker, write_retention_marker,
    };
    use crate::test_support::{
        env_lock, publish_empty_complete_core_fixture, publish_zero_dense_pinned_query_fixture,
    };
    use crate::{PinnedQuerySession, SidecarProfile};
    use tempfile::TempDir;

    struct Fixture {
        project: TempDir,
        cache: TempDir,
        _core: TempDir,
        path: std::path::PathBuf,
        producer: SidecarRuntimeConfig,
        consumer: SidecarRuntimeConfig,
        manifest: RetrievalIndexManifest,
    }
    fn fixture(profile: SidecarProfile) -> Fixture {
        let project = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let core = TempDir::new().unwrap();
        std::fs::write(project.path().join("lib.rs"), "pub fn hello() {}\n").unwrap();
        let path = core.path().join("codestory.db");
        publish_empty_complete_core_fixture(project.path(), &path).unwrap();
        let producer = crate::config::with_test_cache_root(cache.path(), || {
            SidecarRuntimeConfig::for_project_profile_with_run_id(
                Some(project.path()),
                profile,
                Some("producer-run"),
            )
        });
        let consumer = producer.with_profile_and_run_id(
            Some(project.path()),
            SidecarProfile::Agent,
            Some("consumer-run"),
        );
        let manifest =
            publish_zero_dense_pinned_query_fixture(project.path(), &path, &producer).unwrap();
        Fixture {
            project,
            cache,
            _core: core,
            path,
            producer,
            consumer,
            manifest,
        }
    }
    fn publish_scope(f: &mut Fixture) {
        f.manifest.artifact_scope = Some(f.producer.publication_artifact_scope().unwrap());
        Store::open(&f.path)
            .unwrap()
            .upsert_retrieval_index_manifest(&f.manifest)
            .unwrap();
    }
    fn register_local(f: &Fixture) {
        let id = f.producer.project_identity.as_ref().unwrap();
        let marker = GenerationRetentionMarker::next(
            &id.workspace_id,
            f.project.path(),
            f.manifest.clone(),
            None,
            1,
        )
        .unwrap();
        write_retention_marker(&f.producer.layout.state_file, &marker).unwrap();
    }
    #[test]
    fn local_producer_is_consumed_by_agent_without_moving_artifacts() {
        let _env = env_lock();
        let mut f = fixture(SidecarProfile::Local);
        publish_scope(&mut f);
        let resolved = resolve_publication_runtime(
            f.project.path(),
            &Store::open_read_only(&f.path).unwrap(),
            &f.manifest,
            &f.consumer,
        )
        .unwrap();
        assert_eq!(
            resolved.layout.semantic_data_dir,
            f.producer.layout.semantic_data_dir
        );
        assert_eq!(resolved.profile, SidecarProfile::Agent);
        assert_eq!(resolved.run_id, f.consumer.run_id);
        assert_eq!(resolved.namespace, f.consumer.namespace);
        let session = PinnedQuerySession::begin(f.project.path(), &f.path, &f.consumer).unwrap();
        assert_eq!(
            session.publication_identity().artifact_scope,
            Some(RetrievalArtifactScope::Local)
        );
        assert!(!f.consumer.layout.semantic_data_dir.exists());
        drop(session);
        assert!(f.producer.layout.semantic_data_dir.exists());
    }
    #[test]
    fn agent_producer_run_is_independent_of_consumer_run() {
        let _env = env_lock();
        let mut f = fixture(SidecarProfile::Agent);
        publish_scope(&mut f);
        let session = PinnedQuerySession::begin(f.project.path(), &f.path, &f.consumer).unwrap();
        assert_eq!(
            session.publication_identity().artifact_scope,
            f.manifest.artifact_scope
        );
        assert!(!f.consumer.layout.semantic_data_dir.exists());
    }
    #[test]
    fn authenticated_legacy_local_owner_supports_agent_read_without_backfill() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        register_local(&f);
        let before = std::fs::read(&f.path).unwrap();
        let session = PinnedQuerySession::begin(f.project.path(), &f.path, &f.consumer).unwrap();
        assert_eq!(session.publication_identity().artifact_scope, None);
        drop(session);
        assert_eq!(std::fs::read(&f.path).unwrap(), before);
        assert_eq!(
            Store::open_read_only(&f.path)
                .unwrap()
                .get_retrieval_index_manifest(&f.manifest.project_id)
                .unwrap()
                .unwrap()
                .artifact_scope,
            None
        );
    }
    #[test]
    fn unregistered_legacy_local_scope_fails_closed() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        let error = resolve_publication_runtime(
            f.project.path(),
            &Store::open_read_only(&f.path).unwrap(),
            &f.manifest,
            &f.consumer,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("origin_unavailable"));
        assert!(!f.consumer.layout.semantic_data_dir.exists());
    }
    #[test]
    fn legacy_local_owner_cannot_authenticate_other_core_generation() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        register_local(&f);
        let mut forged = f.manifest.clone();
        forged.sidecar_input_hash = Some("wrong".into());
        assert!(
            authenticate_locator(
                &Store::open_read_only(&f.path).unwrap(),
                &forged,
                &f.producer
            )
            .is_err()
        );
    }
    #[test]
    fn two_authenticated_legacy_origins_are_ambiguous() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        register_local(&f);
        let source = f
            .producer
            .layout
            .semantic_data_dir
            .join("collections")
            .join(&f.manifest.semantic_generation)
            .join("vector-generation-manifest.json");
        let target = f
            .consumer
            .layout
            .semantic_data_dir
            .join("collections")
            .join(&f.manifest.semantic_generation);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::copy(source, target.join("vector-generation-manifest.json")).unwrap();
        let error = resolve_publication_runtime(
            f.project.path(),
            &Store::open_read_only(&f.path).unwrap(),
            &f.manifest,
            &f.consumer,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("origin_ambiguous"));
    }
    #[test]
    fn foreign_workspace_and_noncanonical_run_are_rejected() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        for scope in [
            RetrievalArtifactScope::Agent {
                workspace_id: "foreign".into(),
                run_id: "run".into(),
            },
            RetrievalArtifactScope::Agent {
                workspace_id: f
                    .consumer
                    .project_identity
                    .as_ref()
                    .unwrap()
                    .workspace_id
                    .clone(),
                run_id: "../escape".into(),
            },
        ] {
            assert!(f.consumer.with_publication_artifact_scope(&scope).is_err());
        }
    }
    #[test]
    fn producer_origin_lease_blocks_its_cleanup_and_uses_captured_global_root() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        let mut lease =
            GenerationRetentionLease::acquire_for_query(&f.consumer, &f.manifest.project_id)
                .unwrap();
        lease
            .retain_origin(&f.consumer, &f.producer, &f.manifest.project_id)
            .unwrap();
        assert_eq!(
            global_generation_gc_state_file(&f.consumer),
            f.cache
                .path()
                .join("generation-retention-coordination.state")
        );
        assert!(
            GenerationRetentionLock::try_acquire(
                &f.producer.layout.state_file,
                &f.manifest.project_id
            )
            .unwrap()
            .is_none()
        );
        assert!(
            GenerationRetentionLock::try_acquire(
                &global_generation_gc_state_file(&f.consumer),
                GLOBAL_GENERATION_GC_LOCK_SCOPE
            )
            .unwrap()
            .is_none()
        );
        drop(lease);
        assert!(
            GenerationRetentionLock::try_acquire(
                &f.producer.layout.state_file,
                &f.manifest.project_id
            )
            .unwrap()
            .is_some()
        );
    }
    #[test]
    fn origin_observation_creates_no_retention_files() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        assert!(matches!(
            observe_publication_global(&f.consumer).unwrap(),
            ObservedRetentionLock::Absent
        ));
        assert!(matches!(
            observe_publication_origin(&f.producer, &f.manifest.project_id).unwrap(),
            ObservedRetentionLock::Absent
        ));
        assert!(!f.cache.path().join("retention").exists());
    }
    #[test]
    fn registered_legacy_owner_allows_writer_rebuild_when_vector_manifest_is_damaged() {
        let _env = env_lock();
        let f = fixture(SidecarProfile::Local);
        register_local(&f);
        let vector = f
            .producer
            .layout
            .semantic_data_dir
            .join("collections")
            .join(&f.manifest.semantic_generation)
            .join("vector-generation-manifest.json");
        for bytes in [b"corrupt".as_slice(), b"".as_slice()] {
            std::fs::write(&vector, bytes).unwrap();
            let (runtime, scope) =
                runtime_for_publication_writer(f.project.path(), &f.path, &f.consumer).unwrap();
            assert_eq!(
                runtime.layout.semantic_data_dir,
                f.producer.layout.semantic_data_dir
            );
            assert_eq!(scope, RetrievalArtifactScope::Local);
            assert!(PinnedQuerySession::begin(f.project.path(), &f.path, &f.consumer).is_err());
        }
        std::fs::remove_file(vector).unwrap();
        assert!(runtime_for_publication_writer(f.project.path(), &f.path, &f.consumer).is_ok());
    }
    #[test]
    fn writer_and_gc_resolve_same_agent_producer_across_request_runs() {
        let _env = env_lock();
        let mut f = fixture(SidecarProfile::Agent);
        publish_scope(&mut f);
        let (writer, scope) =
            runtime_for_publication_writer(f.project.path(), &f.path, &f.consumer).unwrap();
        let inventory =
            runtime_for_stored_publication(f.project.path(), &f.path, &f.consumer).unwrap();
        assert_eq!(
            writer.layout.semantic_data_dir,
            f.producer.layout.semantic_data_dir
        );
        assert_eq!(
            inventory.layout.semantic_data_dir,
            writer.layout.semantic_data_dir
        );
        assert_eq!(Some(scope), f.manifest.artifact_scope);
    }
    #[test]
    fn changing_artifact_scope_invalidates_a_pinned_query() {
        let _env = env_lock();
        let mut f = fixture(SidecarProfile::Local);
        publish_scope(&mut f);
        let session = PinnedQuerySession::begin(f.project.path(), &f.path, &f.consumer).unwrap();
        f.manifest.artifact_scope = Some(f.consumer.publication_artifact_scope().unwrap());
        Store::open(&f.path)
            .unwrap()
            .upsert_retrieval_index_manifest(&f.manifest)
            .unwrap();
        let error = session.revalidate().unwrap_err();
        assert!(format!("{error:#}").contains("publication_changed"));
    }
}
