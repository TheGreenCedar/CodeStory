use crate::index_commit::next_index_publication;
use crate::index_timings::{
    IndexingRunSummary, core_promotion_timings, database_snapshot_copy_timings,
};
#[cfg(test)]
use crate::publication::{
    PublicationTestBoundary, publication_test_checkpoint,
    run_semantic_projection_before_revalidate_hook,
};
use crate::search_publication::{
    SearchGenerationCatalogGuard, discard_unpublished_search_generation,
};
use crate::search_state_cache::{
    ensure_indexing_active, rebuild_search_state_from_storage_for_runtime,
};
use crate::semantic_projection::{
    LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION, SEMANTIC_POLICY_VERSION, SearchStateBuildResult,
    SemanticProjectionDocumentSource, SemanticProjectionSourcePolicyCompatibility,
    SemanticProjectionStats, apply_semantic_projection_stats,
    finalize_staged_semantic_docs_for_runtime, semantic_projection_source_policy_compatibility,
};
use crate::{clamp_u128_to_u32, source_policy_exclusion_candidate};
use codestory_contracts::api::{ApiError, IndexingPhaseTimings};
use codestory_indexer::CancellationToken;
use codestory_store::{
    IndexPublicationMode, IndexPublicationRecord, SnapshotStore,
    SourcePolicyExclusionPolicyIdentity, SourcePolicyExclusionRecord, StagedSnapshot,
    StagedSnapshotFinalizeStats, StagedSnapshotPublishStats, Store,
    StructuralTextPublicationCompatibility,
};
use codestory_workspace::{SourceIndexPolicy, project_identity_v3};
use std::path::Path;
use std::time::{Duration, Instant};
use uuid::Uuid;

fn validate_semantic_projection_core(
    staged: &mut StagedSnapshot,
    root: &Path,
    expected_schema_version: u32,
    expected_publication: &IndexPublicationRecord,
    source_index_policy: &SourceIndexPolicy,
) -> Result<Vec<SourcePolicyExclusionRecord>, ApiError> {
    let staged_publication = staged
        .store_mut()
        .get_complete_index_publication()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to validate the staged core publication: {error}"
            ))
        })?
        .ok_or_else(|| {
            ApiError::new(
                "semantic_projection_core_incomplete",
                "The staged core publication is incomplete.",
            )
        })?;
    if staged_publication != *expected_publication {
        return Err(ApiError::new(
            "publication_changed",
            "The cloned core does not match the pinned live publication.",
        ));
    }
    staged
        .store_mut()
        .validate_dense_anchor_publication(expected_publication)
        .map_err(|error| {
            ApiError::new(
                "semantic_projection_migration_required",
                format!("Pinned dense-anchor publication is not complete: {error}"),
            )
        })?;
    let structural_compatibility = staged
        .store_mut()
        .validate_structural_text_unit_publication_or_legacy_empty(expected_publication)
        .map_err(|error| {
            ApiError::new(
                "semantic_projection_migration_required",
                format!("Pinned structural state is not compatible: {error}"),
            )
        })?;
    if structural_compatibility == StructuralTextPublicationCompatibility::LegacyEmpty
        && expected_schema_version != LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION
    {
        return Err(ApiError::new(
            "semantic_projection_migration_required",
            "A missing structural publication is compatible only with a schema-29 retained core whose structural stores are empty.",
        ));
    }
    let source_manifest = staged
        .store_mut()
        .get_source_policy_exclusion_manifest()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to load the pinned source-policy manifest: {error}"
            ))
        })?
        .ok_or_else(|| {
            ApiError::new(
                "semantic_projection_migration_required",
                "Pinned source-policy publication is missing.",
            )
        })?;
    let recorded_source_policy = SourcePolicyExclusionPolicyIdentity::new(
        &source_manifest.policy_version,
        source_manifest.byte_cap,
        source_manifest.structural_unit_cap,
    );
    let selected_identity = project_identity_v3(root);
    if source_manifest.project_id != selected_identity.project_id
        || source_manifest.workspace_id != selected_identity.workspace_id
    {
        return Err(ApiError::new(
            "semantic_projection_project_mismatch",
            "The selected project root does not own the cached core publication.",
        ));
    }
    let compatibility = semantic_projection_source_policy_compatibility(
        recorded_source_policy,
        source_index_policy,
        expected_schema_version,
        structural_compatibility == StructuralTextPublicationCompatibility::LegacyEmpty,
    )
    .ok_or_else(|| {
        ApiError::new(
            "semantic_projection_migration_required",
            "Pinned source-policy identity differs from the current runtime policy; run a source refresh before republishing semantic projections.",
        )
    })?;
    let validation = match compatibility {
        SemanticProjectionSourcePolicyCompatibility::Exact => staged
            .store_mut()
            .validate_source_policy_exclusion_publication(
                expected_publication,
                &source_manifest.project_id,
                &source_manifest.workspace_id,
                recorded_source_policy,
            ),
        SemanticProjectionSourcePolicyCompatibility::LegacyPredecessor => staged
            .store_mut()
            .validate_legacy_v1_source_policy_exclusion_publication(
                expected_publication,
                &source_manifest.project_id,
                &source_manifest.workspace_id,
                recorded_source_policy,
            ),
    };
    validation.map_err(|error| {
        ApiError::new(
            "semantic_projection_migration_required",
            format!("Pinned source-policy publication is not complete: {error}"),
        )
    })?;
    staged
        .store_mut()
        .get_source_policy_exclusions()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to load pinned source-policy exclusions: {error}"
            ))
        })
}

struct PreparedSemanticProjection {
    stats: SemanticProjectionStats,
    finalize_stats: StagedSnapshotFinalizeStats,
    detail_snapshot_ms: u32,
}

fn prepare_semantic_projection(
    staged: &mut StagedSnapshot,
    publication: &IndexPublicationRecord,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<PreparedSemanticProjection, ApiError> {
    let source_identity = format!("core:{}:{}", publication.generation_id, publication.run_id);
    staged
        .store_mut()
        .begin_incremental_run()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to fence the staged semantic projection writer: {error}"
            ))
        })?;
    staged
        .store_mut()
        .invalidate_grounding_snapshots()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to invalidate staged derived snapshots: {error}"
            ))
        })?;

    let stats = finalize_staged_semantic_docs_for_runtime(
        staged.store_mut(),
        None,
        None,
        &source_identity,
        cancel_token,
        runtime,
        SemanticProjectionDocumentSource::StoredCore,
    )?;
    ensure_indexing_active(cancel_token)?;
    let finalize_stats = staged.snapshots().finalize_staged().map_err(|error| {
        ApiError::internal(format!(
            "Failed to finalize staged semantic projection snapshots: {error}"
        ))
    })?;
    #[cfg(test)]
    publication_test_checkpoint(
        PublicationTestBoundary::ProjectionSnapshotFinalize,
        cancel_token,
    )?;
    ensure_indexing_active(cancel_token)?;
    let detail_started = Instant::now();
    staged.snapshots().refresh_detail().map_err(|error| {
        ApiError::internal(format!(
            "Failed to refresh staged grounding detail snapshot: {error}"
        ))
    })?;
    #[cfg(test)]
    publication_test_checkpoint(
        PublicationTestBoundary::ProjectionSnapshotDetail,
        cancel_token,
    )?;
    ensure_indexing_active(cancel_token)?;
    Ok(PreparedSemanticProjection {
        stats,
        finalize_stats,
        detail_snapshot_ms: clamp_u128_to_u32(detail_started.elapsed().as_millis()),
    })
}

fn stage_semantic_projection_publication(
    staged: &mut StagedSnapshot,
    root: &Path,
    publication: &IndexPublicationRecord,
    source_exclusions: &[SourcePolicyExclusionRecord],
    source_index_policy: &SourceIndexPolicy,
    cancel_token: Option<&CancellationToken>,
) -> Result<u64, ApiError> {
    #[cfg(test)]
    publication_test_checkpoint(
        PublicationTestBoundary::ProjectionManifestIdentity,
        cancel_token,
    )?;
    ensure_indexing_active(cancel_token)?;
    let dense_manifest = staged
        .store_mut()
        .publish_dense_anchor_generation(publication, SEMANTIC_POLICY_VERSION)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish semantic dense-anchor inputs: {error}"
            ))
        })?;
    let selected_identity = project_identity_v3(root);
    let current_source_policy = SourcePolicyExclusionPolicyIdentity::new(
        &source_index_policy.policy_version,
        source_index_policy.byte_cap,
        source_index_policy.structural_unit_cap,
    );
    let source_candidates = source_exclusions
        .iter()
        .map(|record| {
            let mut candidate = source_policy_exclusion_candidate(record);
            candidate.policy_version = source_index_policy.policy_version.clone();
            candidate.byte_cap = source_index_policy.byte_cap;
            candidate.structural_unit_cap = source_index_policy.structural_unit_cap;
            candidate
        })
        .collect::<Vec<_>>();
    staged
        .store_mut()
        .publish_source_policy_exclusion_generation(
            publication,
            &selected_identity.project_id,
            &selected_identity.workspace_id,
            current_source_policy,
            &source_candidates,
        )
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to rebind pinned source-policy exclusions: {error}"
            ))
        })?;
    staged
        .store_mut()
        .publish_structural_text_unit_generation(publication)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to rebind pinned structural publication: {error}"
            ))
        })?;
    staged
        .store_mut()
        .put_index_publication(publication)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to persist staged semantic projection identity: {error}"
            ))
        })?;
    Ok(dense_manifest.anchor_count)
}

fn commit_semantic_projection(
    mut staged: StagedSnapshot,
    storage_path: &Path,
    expected_publication: &IndexPublicationRecord,
    publication: &IndexPublicationRecord,
    prepared_search_state: SearchStateBuildResult,
    cancel_token: Option<&CancellationToken>,
) -> Result<(SearchStateBuildResult, StagedSnapshotPublishStats, Duration), ApiError> {
    let staged_path = staged.path().to_path_buf();
    let result = (|| {
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::CatalogLock, cancel_token)?;
        ensure_indexing_active(cancel_token)?;
        #[cfg(test)]
        run_semantic_projection_before_revalidate_hook(storage_path);
        let _catalog_guard = SearchGenerationCatalogGuard::acquire(storage_path)?;
        let live_publication =
            Store::database_complete_index_publication(storage_path).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to revalidate the pinned core before promotion: {error}"
                ))
            })?;
        if live_publication.as_ref() != Some(expected_publication) {
            return Err(ApiError::new(
                "publication_changed",
                "The live core changed while semantic projections were being rebuilt.",
            ));
        }
        ensure_indexing_active(cancel_token)?;
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::MarkerCompletion, cancel_token)?;
        ensure_indexing_active(cancel_token)?;
        staged
            .store_mut()
            .finish_incremental_run()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to complete the staged semantic projection marker: {error}"
                ))
            })?;
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::DatabaseReplacement, cancel_token)?;
        ensure_indexing_active(cancel_token)?;
        let publish_started = Instant::now();
        let publish_stats = staged.publish_with_stats(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish staged semantic projections: {error}. Preserved staged snapshot at {}",
                staged_path.display()
            ))
        })?;
        Ok((
            prepared_search_state,
            publish_stats,
            publish_started.elapsed(),
        ))
    })();
    if result.is_err() {
        discard_unpublished_search_generation(storage_path, publication);
    }
    result
}

fn semantic_projection_phase_timings(
    prepared: &PreparedSemanticProjection,
    publish_stats: StagedSnapshotPublishStats,
    publish_duration: Duration,
) -> IndexingPhaseTimings {
    let mut phase_timings = IndexingPhaseTimings {
        deferred_indexes_ms: Some(
            prepared
                .finalize_stats
                .deferred_indexes_ms
                .saturating_add(prepared.stats.semantic_context_index_ms),
        ),
        summary_snapshot_ms: Some(prepared.finalize_stats.summary_snapshot_ms),
        detail_snapshot_ms: Some(prepared.detail_snapshot_ms),
        publish_ms: Some(clamp_u128_to_u32(publish_duration.as_millis())),
        staged_sqlite_wal_autocheckpoint_bytes: publish_stats.sqlite_wal_autocheckpoint_bytes,
        staged_sqlite_checkpoint_ms: publish_stats.sqlite_checkpoint_ms,
        staged_sqlite_sync_ms: publish_stats.sqlite_sync_ms,
        staged_snapshot_copy: publish_stats
            .snapshot_copy
            .map(database_snapshot_copy_timings),
        core_promotion: Some(core_promotion_timings(publish_stats.core_promotion)),
        ..Default::default()
    };
    apply_semantic_projection_stats(&mut phase_timings, prepared.stats);
    phase_timings
}

pub(super) fn semantic_projection_republish_for_runtime(
    root: &Path,
    storage_path: &Path,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<
    (
        IndexingRunSummary,
        IndexPublicationRecord,
        IndexPublicationRecord,
        u32,
        u64,
    ),
    ApiError,
> {
    ensure_indexing_active(cancel_token)?;
    if !storage_path.is_file() {
        return Err(ApiError::new(
            "semantic_projection_core_missing",
            "Semantic projection republish requires an existing complete core publication.",
        ));
    }

    let expected_schema_version =
        Store::database_schema_version(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to pin the stored core schema version: {error}"
            ))
        })?;

    let expected_publication = Store::database_complete_index_publication(storage_path)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to pin the complete core publication: {error}"
            ))
        })?
        .ok_or_else(|| {
            ApiError::new(
                "semantic_projection_core_incomplete",
                "Semantic projection republish requires a complete core publication.",
            )
        })?;
    let mut staged = SnapshotStore::clone_live_to_staged(storage_path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to clone the pinned core for semantic projection republish: {error}"
        ))
    })?;
    let cleanup_staged_path = staged.path().to_path_buf();
    let result = (|| {
        let source_exclusions = validate_semantic_projection_core(
            &mut staged,
            root,
            expected_schema_version,
            &expected_publication,
            source_index_policy,
        )?;
        let publication = next_index_publication(
            Some(&expected_publication),
            IndexPublicationMode::SemanticProjection,
            &Uuid::new_v4().to_string(),
        )?;
        let prepared =
            prepare_semantic_projection(&mut staged, &publication, cancel_token, runtime)?;
        let dense_anchor_count = stage_semantic_projection_publication(
            &mut staged,
            root,
            &publication,
            &source_exclusions,
            source_index_policy,
            cancel_token,
        )?;
        let prepared_search_state = match rebuild_search_state_from_storage_for_runtime(
            staged.store_mut(),
            storage_path,
            None,
            false,
            runtime,
            cancel_token,
            None,
        ) {
            Ok(state) => state,
            Err(error) => {
                discard_unpublished_search_generation(storage_path, &publication);
                return Err(error);
            }
        };
        let (prepared_search_state, publish_stats, publish_duration) = commit_semantic_projection(
            staged,
            storage_path,
            &expected_publication,
            &publication,
            prepared_search_state,
            cancel_token,
        )?;
        let phase_timings =
            semantic_projection_phase_timings(&prepared, publish_stats, publish_duration);
        Ok((
            IndexingRunSummary {
                phase_timings,
                staged_semantic_stats: prepared.stats,
                llm_refresh_scope: None,
                #[cfg(test)]
                publication: publication.clone(),
                prepared_search_state: Some(prepared_search_state),
            },
            publication,
            prepared.stats.symbol_search_docs_written,
            dense_anchor_count,
        ))
    })();

    match result {
        Ok((summary, publication, symbol_document_count, dense_anchor_count)) => Ok((
            summary,
            expected_publication,
            publication,
            symbol_document_count,
            dense_anchor_count,
        )),
        Err(error) => {
            let _ = SnapshotStore::discard_staged(&cleanup_staged_path);
            Err(error)
        }
    }
}
