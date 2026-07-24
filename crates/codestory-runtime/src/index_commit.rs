#[cfg(test)]
use crate::publication::run_source_policy_before_revalidate_hook;
#[cfg(test)]
use crate::publication::{PublicationTestBoundary, publication_test_checkpoint};
use crate::search_publication::{
    SearchGenerationCatalogGuard, discard_unpublished_search_generation,
};
use crate::search_state_cache::ensure_indexing_active;
use crate::semantic_projection::{SEMANTIC_POLICY_VERSION, SearchStateBuildResult};
use crate::{
    current_epoch_ms, publish_source_policy_exclusions, revalidate_source_policy_exclusions,
};
use codestory_contracts::api::{ApiError, IndexPublicationDto, IndexPublicationModeDto};
use codestory_indexer::CancellationToken;
use codestory_store::{
    IndexPublicationMode, IndexPublicationRecord, StagedSnapshot, StagedSnapshotPublishStats,
};
use codestory_workspace::{
    OversizedSourceExclusionCandidate, SourceIndexPolicy, WorkspaceManifest,
};
use fs4::fs_std::FileExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub(super) fn next_index_publication(
    previous: Option<&IndexPublicationRecord>,
    mode: IndexPublicationMode,
    run_id: &str,
) -> Result<IndexPublicationRecord, ApiError> {
    let generation = previous
        .map(|publication| publication.generation)
        .unwrap_or_default()
        .checked_add(1)
        .ok_or_else(|| ApiError::internal("Index publication generation overflow"))?;
    Ok(IndexPublicationRecord {
        generation,
        generation_id: Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        mode,
        published_at_epoch_ms: current_epoch_ms(),
    })
}

pub(super) fn index_publication_dto(publication: IndexPublicationRecord) -> IndexPublicationDto {
    IndexPublicationDto {
        generation: publication.generation,
        generation_id: publication.generation_id,
        run_id: publication.run_id,
        mode: match publication.mode {
            IndexPublicationMode::Full => IndexPublicationModeDto::Full,
            IndexPublicationMode::Incremental => IndexPublicationModeDto::Incremental,
            IndexPublicationMode::SemanticProjection => IndexPublicationModeDto::SemanticProjection,
        },
        published_at_epoch_ms: publication.published_at_epoch_ms,
    }
}

pub(super) struct IndexWriterGuard {
    file: std::fs::File,
    path: PathBuf,
}

impl IndexWriterGuard {
    pub(super) fn try_acquire(storage_path: &Path) -> Result<Self, ApiError> {
        let path = storage_path.with_extension("index-writer.lock");
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create index writer lock directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to open index writer lock {}: {error}",
                    path.display()
                ))
            })?;
        if !FileExt::try_lock_exclusive(&file).map_err(|error| {
            ApiError::internal(format!(
                "Failed to acquire index writer lock {}: {error}",
                path.display()
            ))
        })? {
            return Err(ApiError::new(
                "cache_busy",
                format!(
                    "Another indexing run owns the writer lock at {}. Wait for it to finish and retry.",
                    path.display()
                ),
            ));
        }
        Ok(Self { file, path })
    }
}

impl Drop for IndexWriterGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                "Failed to unlock index writer lock: {error}"
            );
        }
    }
}

pub(super) fn stage_core_publication_identity(
    staged: &mut StagedSnapshot,
    root: &Path,
    workspace: &WorkspaceManifest,
    publication: &IndexPublicationRecord,
    policy_exclusions: &[OversizedSourceExclusionCandidate],
    source_index_policy: &SourceIndexPolicy,
    cancel_token: Option<&CancellationToken>,
) -> Result<(), ApiError> {
    ensure_indexing_active(cancel_token)?;
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::Identity, cancel_token)?;
    staged
        .store_mut()
        .publish_dense_anchor_generation(publication, SEMANTIC_POLICY_VERSION)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish complete dense anchor inputs: {error}"
            ))
        })?;
    #[cfg(test)]
    run_source_policy_before_revalidate_hook();
    let exclusions =
        revalidate_source_policy_exclusions(workspace, policy_exclusions, source_index_policy)?;
    publish_source_policy_exclusions(
        staged.store_mut(),
        root,
        publication,
        &exclusions,
        source_index_policy,
    )?;
    staged
        .store_mut()
        .publish_structural_text_unit_generation(publication)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish complete structural text units: {error}"
            ))
        })?;
    let mode = match publication.mode {
        IndexPublicationMode::Full => "full",
        IndexPublicationMode::Incremental => "incremental",
        IndexPublicationMode::SemanticProjection => "semantic projection",
    };
    staged
        .store_mut()
        .put_index_publication(publication)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to persist staged {mode} publication identity: {error}"
            ))
        })
}

#[derive(Clone, Copy)]
pub(super) enum CoreCommitMode {
    Full { finish_recovery_marker: bool },
    Incremental,
}

pub(super) struct PreparedCoreCommit {
    staged: Option<StagedSnapshot>,
    search_state: Option<SearchStateBuildResult>,
    storage_path: PathBuf,
    publication: IndexPublicationRecord,
    committed: bool,
}

impl PreparedCoreCommit {
    pub(super) fn new(
        staged: StagedSnapshot,
        search_state: SearchStateBuildResult,
        storage_path: &Path,
        publication: &IndexPublicationRecord,
    ) -> Self {
        Self {
            staged: Some(staged),
            search_state: Some(search_state),
            storage_path: storage_path.to_path_buf(),
            publication: publication.clone(),
            committed: false,
        }
    }

    fn staged_mut(&mut self) -> &mut StagedSnapshot {
        self.staged
            .as_mut()
            .expect("prepared core commit must own staged storage")
    }

    pub(super) fn commit(
        mut self,
        mode: CoreCommitMode,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<(SearchStateBuildResult, StagedSnapshotPublishStats, Duration), ApiError> {
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::CatalogLock, cancel_token)?;
        let _catalog_guard = SearchGenerationCatalogGuard::acquire(&self.storage_path)?;
        ensure_indexing_active(cancel_token)?;
        let finish_marker = match mode {
            CoreCommitMode::Full {
                finish_recovery_marker,
            } => finish_recovery_marker,
            CoreCommitMode::Incremental => true,
        };
        if finish_marker {
            #[cfg(test)]
            publication_test_checkpoint(PublicationTestBoundary::MarkerCompletion, cancel_token)?;
            ensure_indexing_active(cancel_token)?;
            let marker = match mode {
                CoreCommitMode::Full { .. } => "full-recovery",
                CoreCommitMode::Incremental => "incremental",
            };
            self.staged_mut()
                .store_mut()
                .finish_incremental_run()
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to complete staged {marker} marker: {error}"
                    ))
                })?;
        }
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::DatabaseReplacement, cancel_token)?;
        ensure_indexing_active(cancel_token)?;
        let staged_path = self.staged_mut().path().to_path_buf();
        let staged = self
            .staged
            .take()
            .expect("prepared core commit must own staged storage");
        let publish_started = Instant::now();
        let publish_stats = staged
            .publish_with_stats(&self.storage_path)
            .map_err(|error| {
                let publication = match mode {
                    CoreCommitMode::Full { .. } => "storage",
                    CoreCommitMode::Incremental => "incremental storage",
                };
                ApiError::internal(format!(
                    "Failed to publish staged {publication}: {error}. Preserved staged snapshot at {}",
                    staged_path.display()
                ))
            })?;
        let search_state = self
            .search_state
            .take()
            .expect("prepared core commit must own search state");
        self.committed = true;
        Ok((search_state, publish_stats, publish_started.elapsed()))
    }
}

impl Drop for PreparedCoreCommit {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        drop(self.search_state.take());
        if let Some(staged) = self.staged.take() {
            let _ = staged.discard();
        }
        discard_unpublished_search_generation(&self.storage_path, &self.publication);
    }
}

pub(super) struct StagedPreparation {
    staged: Option<StagedSnapshot>,
}

impl StagedPreparation {
    pub(super) fn new(staged: StagedSnapshot) -> Self {
        Self {
            staged: Some(staged),
        }
    }

    pub(super) fn staged_mut(&mut self) -> &mut StagedSnapshot {
        self.staged
            .as_mut()
            .expect("staged preparation must own staged storage")
    }

    pub(super) fn release(mut self) -> StagedSnapshot {
        self.staged
            .take()
            .expect("staged preparation must own staged storage")
    }
}

impl Drop for StagedPreparation {
    fn drop(&mut self) {
        if let Some(staged) = self.staged.take() {
            let _ = staged.discard();
        }
    }
}
