use crate::index_commit::{IndexWriterGuard, index_publication_dto};
use crate::index_freshness::{
    CachedIndexFreshness, index_freshness_cache_ttl_secs, index_freshness_from_storage_with_policy,
    open_storage_for_read, storage_fingerprint, workspace_member_index_summaries,
    workspace_member_storage_summaries,
};
use crate::index_full::index_full_for_runtime;
use crate::index_incremental::{
    ensure_incremental_refresh_compatible, index_incremental_for_runtime,
};
use crate::index_timings::IndexingRunSummary;
#[cfg(test)]
use crate::publication::{
    PublicationTestBoundary, publication_test_checkpoint,
    run_activation_search_before_revalidate_hook,
};
use crate::route_coverage::{
    framework_route_coverage_matrix, language_support_summary_for_language,
};
use crate::search_intent::indexed_file_matches_language_filter;
use crate::search_publication::{
    load_persisted_search_state_for_runtime, retrieval_state_from_storage_for_runtime,
};
use crate::search_state_cache::{
    indexing_cancelled_error, publish_prepared_search_state,
    rebuild_search_state_from_storage_for_runtime, refresh_caches, workspace_refresh_inputs,
};
use crate::semantic_projection::{
    CacheRefreshStats, SEMANTIC_POLICY_VERSION, SemanticProjectionRepublishOutcome,
    apply_cache_refresh_stats, summarize_symbol_doc,
};
use crate::semantic_republish::semantic_projection_republish_for_runtime;
use crate::support::{clamp_i64_to_u32, clamp_u128_to_u32};
use crate::workspace_state::runtime_workspace_manifest;
use crate::{
    AppController, Storage, clear_search_engine, current_epoch_ms, file_coverage_detail,
    file_coverage_reason, file_coverage_retryable, full_refresh_execution_plan_with_coverage,
    indexed_file_role, no_project_error, normalize_path_key, path_role_from_key,
    publish_search_engine, runtime_relative_path, validate_source_policy_exclusions,
    validate_structural_text_units,
};
use codestory_contracts::api::{
    ApiError, AppEventPayload, FileCoverageDiagnosticDto, IndexDryRunDto, IndexFreshnessDto,
    IndexMode, IndexPublicationDto, IndexedFileDto, IndexedFileIncompleteReasonCountDto,
    IndexedFileLanguageCountDto, IndexedFilesDto, IndexedFilesRequest, IndexedFilesSummaryDto,
    IndexingPhaseTimings, OpenProjectRequest, ProjectSummary, SourcePolicyExclusionDto,
    StartIndexingRequest, StorageStatsDto, SummaryGenerationDto,
};
use codestory_contracts::graph::FileCoverageReason;
use codestory_indexer::CancellationToken;
use codestory_store::{CURRENT_SCHEMA_VERSION, IndexPublicationRecord, Store, SymbolSummaryRecord};
use codestory_workspace::{RefreshInputs, WorkspaceManifest};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

impl AppController {
    pub(crate) fn project_summary_from_storage(
        &self,
        root: &Path,
        storage_path: &Path,
        storage: &Storage,
    ) -> Result<ProjectSummary, ApiError> {
        let stats = storage
            .get_stats()
            .map_err(|e| ApiError::internal(format!("Failed to query stats: {e}")))?;
        let derived_file_count = if stats.file_count > 0 {
            stats.file_count
        } else {
            storage
                .get_file_node_count()
                .map_err(|e| ApiError::internal(format!("Failed to query file nodes: {e}")))?
        };
        let dto_stats = StorageStatsDto {
            node_count: clamp_i64_to_u32(stats.node_count),
            edge_count: clamp_i64_to_u32(stats.edge_count),
            file_count: clamp_i64_to_u32(derived_file_count),
            error_count: clamp_i64_to_u32(stats.error_count),
            fatal_error_count: clamp_i64_to_u32(stats.fatal_error_count),
        };
        let workspace = runtime_workspace_manifest(root, storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let members = workspace_member_storage_summaries(root, &workspace, storage)?;
        let freshness =
            self.cached_index_freshness_from_storage(root, storage_path, &workspace, storage);
        let publication = storage
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read complete index publication: {error}"
                ))
            })?
            .map(index_publication_dto);

        Ok(ProjectSummary {
            root: root.to_string_lossy().to_string(),
            stats: dto_stats,
            members,
            retrieval: Some(retrieval_state_from_storage_for_runtime(
                storage,
                &self.runtime_config,
            )?),
            freshness: Some(freshness),
            publication,
        })
    }

    pub fn complete_index_publication_at(
        &self,
        storage_path: &Path,
    ) -> Result<Option<IndexPublicationDto>, ApiError> {
        if !storage_path.is_file() {
            return Ok(None);
        }
        Store::open_observational(storage_path)
            .and_then(|storage| storage.get_complete_index_publication())
            .map(|publication| publication.map(index_publication_dto))
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to observe complete index publication: {error}"
                ))
            })
    }

    fn open_project_summary_with_storage_inner(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        let storage = open_storage_for_read(&storage_path)?;
        let snapshot = storage.read_snapshot().map_err(|error| {
            ApiError::internal(format!("Failed to begin project summary snapshot: {error}"))
        })?;
        let summary =
            self.project_summary_from_storage(&root, &storage_path, snapshot.storage())?;
        snapshot.finish().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finish project summary snapshot: {error}"
            ))
        })?;

        {
            let mut s = self.state.lock();
            s.project_root = Some(root);
            s.storage_path = Some(storage_path);
            s.node_names.clear();
            clear_search_engine(&mut s);
        }
        self.sidecar_query_cache.lock().clear();

        Ok(summary)
    }

    fn open_project_with_storage_inner(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        let mut storage = open_storage_for_read(&storage_path)?;
        let loaded = load_persisted_search_state_for_runtime(
            &mut storage,
            &storage_path,
            &self.runtime_config,
        )?;
        let mut summary = self.project_summary_from_storage(&root, &storage_path, &storage)?;
        summary.retrieval = Some(retrieval_state_from_storage_for_runtime(
            &storage,
            &self.runtime_config,
        )?);

        {
            let mut s = self.state.lock();
            s.project_root = Some(root);
            s.storage_path = Some(storage_path);
            s.node_names = loaded.node_names;
            publish_search_engine(&mut s, loaded.engine, loaded.publication);
        }
        self.sidecar_query_cache.lock().clear();

        let _ = self.events_tx.send(AppEventPayload::StatusUpdate {
            message: "Project opened.".to_string(),
        });

        Ok(summary)
    }

    pub fn open_project(&self, req: OpenProjectRequest) -> Result<ProjectSummary, ApiError> {
        let root = PathBuf::from(req.path);
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }

        let storage_path = root.join("codestory.db");
        self.open_project_with_storage_path(root, storage_path)
    }

    pub fn open_project_with_storage_path(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }
        if let Some(parent) = storage_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ApiError::internal(format!(
                    "Failed to create storage directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        self.open_project_with_storage_inner(root, storage_path)
    }

    pub fn open_project_summary_with_storage_path(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }
        if let Some(parent) = storage_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ApiError::internal(format!(
                    "Failed to create storage directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        self.open_project_summary_with_storage_inner(root, storage_path)
    }

    pub fn inspect_project_summary_with_storage_path(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<Option<ProjectSummary>, ApiError> {
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }
        if !storage_path.is_file() {
            return Ok(None);
        }
        let storage = Storage::open_observational(&storage_path).map_err(|error| {
            ApiError::internal(format!("Failed to open storage observationally: {error}"))
        })?;
        let snapshot = storage.read_snapshot().map_err(|error| {
            ApiError::internal(format!("Failed to begin project summary snapshot: {error}"))
        })?;
        let summary =
            self.project_summary_from_storage(&root, &storage_path, snapshot.storage())?;
        snapshot.finish().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finish project summary snapshot: {error}"
            ))
        })?;
        let changed = {
            let mut state = self.state.lock();
            let changed =
                state.project_root.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &root)
                }) || state.storage_path.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &storage_path)
                });
            if changed {
                state.node_names.clear();
                clear_search_engine(&mut state);
            }
            state.project_root = Some(root);
            state.storage_path = Some(storage_path);
            changed
        };
        if changed {
            self.sidecar_query_cache.lock().clear();
        }
        Ok(Some(summary))
    }

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        let (root, storage_path) = {
            let s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            let root = s.project_root.clone().ok_or_else(|| {
                ApiError::invalid_argument("No project open. Call open_project first.")
            })?;
            let storage_path = s
                .storage_path
                .clone()
                .unwrap_or_else(|| root.join("codestory.db"));
            (root, storage_path)
        };
        if req.mode == IndexMode::Incremental {
            ensure_incremental_refresh_compatible(&root, &storage_path)?;
        }
        {
            let mut s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            s.is_indexing = true;
            s.index_freshness_cache = None;
        }

        let events_tx = self.events_tx.clone();
        let controller = self.clone();

        // Use a dedicated thread so callers can keep their runtime responsive.
        std::thread::spawn(move || {
            let indexing_started = std::time::Instant::now();
            let result = match IndexWriterGuard::try_acquire(&storage_path) {
                Ok(_writer_guard) => {
                    let result = match req.mode {
                        IndexMode::Full => index_full_for_runtime(
                            &root,
                            &storage_path,
                            &events_tx,
                            None,
                            &controller.runtime_config,
                            &controller.source_index_policy,
                        ),
                        IndexMode::Incremental => index_incremental_for_runtime(
                            &root,
                            &storage_path,
                            &events_tx,
                            None,
                            &controller.runtime_config,
                            &controller.source_index_policy,
                        ),
                    };
                    result.and_then(|summary| {
                        controller.finish_successful_indexing(summary, &storage_path, true, None)
                    })
                }
                Err(error) => Err(error),
            };

            match result {
                Ok(phase_timings) => {
                    controller.state.lock().is_indexing = false;
                    let _ = events_tx.send(AppEventPayload::IndexingComplete {
                        duration_ms: clamp_u128_to_u32(indexing_started.elapsed().as_millis()),
                        phase_timings,
                    });
                }
                Err(err) => {
                    let _ = events_tx.send(AppEventPayload::IndexingFailed { error: err.message });
                    controller.recover_failed_indexing(&storage_path, true);
                }
            }
        });

        Ok(())
    }

    fn run_indexing_blocking_inner(
        &self,
        mode: IndexMode,
        refresh_runtime_caches: bool,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        let (root, storage_path) = {
            let s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            let root = s.project_root.clone().ok_or_else(no_project_error)?;
            let storage_path = s
                .storage_path
                .clone()
                .unwrap_or_else(|| root.join("codestory.db"));
            (root, storage_path)
        };
        if mode == IndexMode::Incremental {
            ensure_incremental_refresh_compatible(&root, &storage_path)?;
        }
        {
            let mut s = self.state.lock();
            if s.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            s.is_indexing = true;
            s.index_freshness_cache = None;
        }

        let _writer_guard = match IndexWriterGuard::try_acquire(&storage_path) {
            Ok(guard) => guard,
            Err(error) => {
                self.state.lock().is_indexing = false;
                return Err(error);
            }
        };

        let result = match mode {
            IndexMode::Full => index_full_for_runtime(
                &root,
                &storage_path,
                &self.events_tx,
                cancel_token,
                &self.runtime_config,
                &self.source_index_policy,
            ),
            IndexMode::Incremental => index_incremental_for_runtime(
                &root,
                &storage_path,
                &self.events_tx,
                cancel_token,
                &self.runtime_config,
                &self.source_index_policy,
            ),
        };

        match result {
            Ok(summary) => self.finish_successful_indexing(
                summary,
                &storage_path,
                refresh_runtime_caches,
                cancel_token,
            ),
            Err(error) => {
                self.recover_failed_indexing(&storage_path, refresh_runtime_caches);
                Err(error)
            }
        }
    }

    pub(crate) fn finish_successful_indexing(
        &self,
        mut summary: IndexingRunSummary,
        storage_path: &Path,
        refresh_runtime_caches: bool,
        _cancel_token: Option<&CancellationToken>,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        if refresh_runtime_caches {
            #[cfg(test)]
            let boundary_result =
                publication_test_checkpoint(PublicationTestBoundary::RuntimeCache, _cancel_token);
            #[cfg(not(test))]
            let boundary_result: Result<(), ApiError> = Ok(());
            if let Err(error) = boundary_result {
                tracing::warn!(
                    error = %error.message,
                    "Runtime cache publication fault occurred after durable database commit; completing from the prepared generation"
                );
            }
        }
        let cache_refresh_started = Instant::now();
        let cache_stats_result = if let Some(prepared) = summary.prepared_search_state.take() {
            if refresh_runtime_caches {
                Ok(publish_prepared_search_state(self, prepared))
            } else {
                self.clear_search_state();
                self.state.lock().is_indexing = false;
                Ok(CacheRefreshStats {
                    search_stats: prepared.search_stats,
                    semantic_stats: prepared.semantic_stats,
                    runtime_cache_publish_ms: None,
                })
            }
        } else if refresh_runtime_caches {
            (|| {
                let mut storage = Storage::open(storage_path)
                    .map_err(|e| ApiError::internal(format!("Failed to reopen storage: {e}")))?;
                refresh_caches(
                    self,
                    &mut storage,
                    storage_path,
                    summary.llm_refresh_scope.as_ref(),
                )
            })()
        } else {
            self.finalize_indexing_without_runtime_refresh_with(
                storage_path,
                summary.llm_refresh_scope.as_ref(),
                |storage, llm_refresh_scope| {
                    rebuild_search_state_from_storage_for_runtime(
                        storage,
                        storage_path,
                        llm_refresh_scope,
                        false,
                        &self.runtime_config,
                        None,
                        None,
                    )
                    .map(|result| CacheRefreshStats {
                        search_stats: result.search_stats,
                        semantic_stats: result.semantic_stats,
                        runtime_cache_publish_ms: None,
                    })
                },
            )
        };
        let mut cache_stats = match cache_stats_result {
            Ok(cache_stats) => cache_stats,
            Err(error) => {
                self.clear_search_state();
                self.state.lock().is_indexing = false;
                return Err(error);
            }
        };
        summary.phase_timings.cache_refresh_ms = Some(clamp_u128_to_u32(
            cache_refresh_started.elapsed().as_millis(),
        ));
        if summary.staged_semantic_stats.reported {
            summary.staged_semantic_stats.reload_ms = cache_stats.semantic_stats.reload_ms;
            cache_stats.semantic_stats = summary.staged_semantic_stats;
        }
        apply_cache_refresh_stats(&mut summary.phase_timings, cache_stats);
        Ok(summary.phase_timings)
    }

    fn recover_failed_indexing(&self, storage_path: &Path, refresh_runtime_caches: bool) {
        if refresh_runtime_caches && let Ok(mut storage) = Storage::open(storage_path) {
            let incomplete = storage.has_incomplete_incremental_run().unwrap_or(true);
            if !incomplete {
                self.clear_search_state();
                let _ = refresh_caches(self, &mut storage, storage_path, None);
                return;
            }
        }
        self.clear_search_state();
        let mut state = self.state.lock();
        state.index_freshness_cache = None;
        state.is_indexing = false;
    }

    pub(crate) fn prepare_search_state_for_activation(
        &self,
        cancel_token: &CancellationToken,
    ) -> Result<(), ApiError> {
        let storage_path = self
            .state
            .lock()
            .storage_path
            .clone()
            .ok_or_else(no_project_error)?;
        let _writer_guard = IndexWriterGuard::try_acquire(&storage_path)?;
        if cancel_token.is_cancelled() {
            return Err(indexing_cancelled_error());
        }

        let mut storage = Storage::open(&storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to open core storage for search preparation: {error}"
            ))
        })?;
        let expected_publication = storage
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read the complete core publication before search preparation: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "publication_changed",
                    "The complete core publication disappeared before search preparation.",
                )
            })?;
        let mut validate_before_completion =
            |prepared_publication: &IndexPublicationRecord| -> Result<(), ApiError> {
                if cancel_token.is_cancelled() {
                    return Err(indexing_cancelled_error());
                }

                #[cfg(test)]
                run_activation_search_before_revalidate_hook(&storage_path);

                let live_publication = Store::database_index_publication(&storage_path).map_err(
                    |error| {
                        ApiError::internal(format!(
                            "Failed to revalidate the core publication before search promotion: {error}"
                        ))
                    },
                )?;
                if prepared_publication != &expected_publication
                    || live_publication.as_ref() != Some(&expected_publication)
                {
                    return Err(ApiError::new(
                        "publication_changed",
                        "The core publication changed while its search generation was being prepared.",
                    ));
                }
                Ok(())
            };
        let prepared = rebuild_search_state_from_storage_for_runtime(
            &mut storage,
            &storage_path,
            None,
            false,
            &self.runtime_config,
            Some(cancel_token),
            Some(&mut validate_before_completion),
        )?;
        if cancel_token.is_cancelled() {
            return Err(indexing_cancelled_error());
        }

        let live_publication =
            Store::database_index_publication(&storage_path).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to revalidate the core publication after search preparation: {error}"
                ))
            })?;
        if prepared.publication.as_ref() != Some(&expected_publication)
            || live_publication.as_ref() != Some(&expected_publication)
        {
            drop(prepared);
            return Err(ApiError::new(
                "publication_changed",
                "The core publication changed while its search generation was being prepared.",
            ));
        }

        publish_prepared_search_state(self, prepared);
        Ok(())
    }

    pub(crate) fn complete_core_requires_publication_repair(
        &self,
        storage_path: &Path,
    ) -> Result<bool, ApiError> {
        if !storage_path.is_file() {
            return Ok(false);
        }
        let storage = Store::open_read_only(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect dense-anchor publication readiness: {error}"
            ))
        })?;
        let Some(publication) = storage.get_complete_index_publication().map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect dense-anchor core publication: {error}"
            ))
        })?
        else {
            return Ok(false);
        };
        if storage
            .validate_dense_anchor_publication(&publication)
            .is_err()
            || storage
                .validate_structural_text_unit_publication(&publication)
                .is_err()
        {
            return Ok(true);
        }
        let root = self.require_project_root()?;
        Ok(validate_source_policy_exclusions(
            &storage,
            &root,
            &publication,
            &self.source_index_policy,
        )
        .is_err())
    }

    pub fn ensure_incremental_refresh_compatible(&self) -> Result<(), ApiError> {
        let state = self.state.lock();
        let root = state.project_root.as_deref().ok_or_else(no_project_error)?;
        let storage_path = state.storage_path.as_deref().ok_or_else(no_project_error)?;
        ensure_incremental_refresh_compatible(root, storage_path)
    }

    pub fn ensure_incremental_refresh_compatible_at(
        &self,
        root: &Path,
        storage_path: &Path,
    ) -> Result<(), ApiError> {
        ensure_incremental_refresh_compatible(root, storage_path)
    }

    pub fn run_indexing_blocking(&self, mode: IndexMode) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, true, None)
    }

    pub fn run_indexing_blocking_with_cancel(
        &self,
        mode: IndexMode,
        cancel_token: &CancellationToken,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, true, Some(cancel_token))
    }

    pub fn run_indexing_blocking_without_runtime_refresh(
        &self,
        mode: IndexMode,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, false, None)
    }

    pub fn run_indexing_blocking_without_runtime_refresh_with_cancel(
        &self,
        mode: IndexMode,
        cancel_token: &CancellationToken,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, false, Some(cancel_token))
    }

    pub fn republish_semantic_projections_blocking(
        &self,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        self.republish_semantic_projections_blocking_inner(None)
    }

    pub fn republish_semantic_projections_blocking_with_cancel(
        &self,
        cancel_token: &CancellationToken,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        self.republish_semantic_projections_blocking_inner(Some(cancel_token))
    }

    fn republish_semantic_projections_blocking_inner(
        &self,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        let (root, storage_path) = {
            let state = self.state.lock();
            let root = state.project_root.clone().ok_or_else(no_project_error)?;
            let storage_path = state
                .storage_path
                .clone()
                .unwrap_or_else(|| root.join("codestory.db"));
            (root, storage_path)
        };
        self.republish_semantic_projections_at_blocking_inner(root, storage_path, cancel_token)
    }

    pub fn republish_semantic_projections_at_blocking(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        self.republish_semantic_projections_at_blocking_inner(root, storage_path, None)
    }

    fn republish_semantic_projections_at_blocking_inner(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<SemanticProjectionRepublishOutcome, ApiError> {
        if !root.is_dir() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist or is not a directory: {}",
                root.display()
            )));
        }
        {
            let mut state = self.state.lock();
            if state.is_indexing {
                return Err(ApiError::invalid_argument(
                    "Indexing already in progress for this controller.",
                ));
            }
            let changed =
                state.project_root.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &root)
                }) || state.storage_path.as_ref().is_none_or(|current| {
                    !codestory_workspace::same_workspace_path(current, &storage_path)
                });
            if changed {
                state.node_names.clear();
                clear_search_engine(&mut state);
            }
            state.project_root = Some(root.clone());
            state.storage_path = Some(storage_path.clone());
            state.is_indexing = true;
            state.index_freshness_cache = None;
        }
        let _writer_guard = match IndexWriterGuard::try_acquire(&storage_path) {
            Ok(guard) => guard,
            Err(error) => {
                self.state.lock().is_indexing = false;
                return Err(error);
            }
        };
        let result = semantic_projection_republish_for_runtime(
            &root,
            &storage_path,
            cancel_token,
            &self.runtime_config,
            &self.source_index_policy,
        );
        match result {
            Ok((
                summary,
                previous_publication,
                publication,
                symbol_document_count,
                dense_anchor_count,
            )) => {
                let phase_timings =
                    self.finish_successful_indexing(summary, &storage_path, true, cancel_token)?;
                Ok(SemanticProjectionRepublishOutcome {
                    previous_publication,
                    publication,
                    semantic_policy_version: SEMANTIC_POLICY_VERSION.to_string(),
                    symbol_document_count,
                    dense_anchor_count,
                    phase_timings,
                })
            }
            Err(error) => {
                let mut state = self.state.lock();
                state.is_indexing = false;
                state.index_freshness_cache = None;
                Err(error)
            }
        }
    }

    pub fn dry_run_index(&self, mode: IndexMode) -> Result<IndexDryRunDto, ApiError> {
        let root = self.require_project_root()?;
        let storage_path = self.require_storage_path()?;
        if mode == IndexMode::Incremental {
            ensure_incremental_refresh_compatible(&root, &storage_path)?;
        }
        let workspace = runtime_workspace_manifest(&root, &storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let refresh_inputs = if storage_path.exists() {
            let schema_version = Store::database_schema_version_observational(&storage_path)
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to inspect dry-run storage without recovery: {error}"
                    ))
                })?;
            if schema_version < CURRENT_SCHEMA_VERSION {
                RefreshInputs::default()
            } else {
                let store =
                    Store::open_freshness_observational(&storage_path).map_err(|error| {
                        ApiError::internal(format!(
                            "Failed to inspect dry-run storage without mutation: {error}"
                        ))
                    })?;
                workspace_refresh_inputs(&store)?
            }
        } else {
            RefreshInputs::default()
        };
        let execution_plan = match mode {
            IndexMode::Full => {
                full_refresh_execution_plan_with_coverage(
                    &root,
                    &workspace,
                    &self.source_index_policy,
                )?
                .0
            }
            IndexMode::Incremental => {
                workspace
                    .build_execution_outcome_with_policy(&refresh_inputs, &self.source_index_policy)
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "Failed to generate incremental refresh plan: {e}"
                        ))
                    })?
                    .refresh
                    .plan
            }
        };
        let members =
            workspace_member_index_summaries(&root, &workspace, &refresh_inputs, &execution_plan);
        Ok(IndexDryRunDto {
            root: root.to_string_lossy().to_string(),
            storage_path: storage_path.to_string_lossy().to_string(),
            refresh: mode,
            files_to_index: execution_plan.files_to_index.len().min(u32::MAX as usize) as u32,
            files_to_remove: execution_plan.files_to_remove.len().min(u32::MAX as usize) as u32,
            sample_files_to_index: execution_plan
                .files_to_index
                .iter()
                .take(12)
                .map(|path| runtime_relative_path(&root, path))
                .collect(),
            sample_file_ids_to_remove: execution_plan
                .files_to_remove
                .iter()
                .take(12)
                .copied()
                .collect(),
            members,
        })
    }

    pub fn summarize_symbols_blocking(&self) -> Result<SummaryGenerationDto, ApiError> {
        let endpoint = self
            .runtime_config
            .summary
            .endpoint
            .clone()
            .ok_or_else(|| {
                ApiError::invalid_argument(
                    "--summarize requires CODESTORY_SUMMARY_ENDPOINT to be configured.",
                )
            })?;
        let model = self.runtime_config.summary.model.clone();
        let storage_path = self.require_storage_path()?;
        let mut storage = Store::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;
        let docs = storage
            .get_all_llm_symbol_docs()
            .map_err(|e| ApiError::internal(format!("Failed to load symbol docs: {e}")))?;
        let current_summaries = storage
            .get_all_current_symbol_summaries()
            .map_err(|e| ApiError::internal(format!("Failed to load symbol summaries: {e}")))?;

        let mut generated = 0u32;
        let mut reused = 0u32;
        let mut skipped = 0u32;
        let mut pending = Vec::new();
        for doc in docs {
            if current_summaries.contains_key(&doc.node_id) {
                reused = reused.saturating_add(1);
                continue;
            }
            if doc.doc_text.trim().is_empty() {
                skipped = skipped.saturating_add(1);
                continue;
            }
            let summary =
                summarize_symbol_doc(&endpoint, &model, &doc, &self.runtime_config.summary)?;
            pending.push(SymbolSummaryRecord {
                node_id: doc.node_id,
                content_hash: doc.doc_hash,
                summary,
                model: model.clone(),
                updated_at_epoch_ms: current_epoch_ms(),
            });
            generated = generated.saturating_add(1);

            if pending.len() >= 32 {
                storage
                    .upsert_symbol_summaries_batch(&pending)
                    .map_err(|e| {
                        ApiError::internal(format!("Failed to store symbol summaries: {e}"))
                    })?;
                pending.clear();
            }
        }
        storage
            .upsert_symbol_summaries_batch(&pending)
            .map_err(|e| ApiError::internal(format!("Failed to store symbol summaries: {e}")))?;

        Ok(SummaryGenerationDto {
            generated,
            reused,
            skipped,
            endpoint,
        })
    }

    pub(crate) fn finalize_indexing_without_runtime_refresh_with<F>(
        &self,
        storage_path: &Path,
        llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
        rebuild: F,
    ) -> Result<CacheRefreshStats, ApiError>
    where
        F: FnOnce(
            &mut Storage,
            Option<&HashSet<codestory_contracts::graph::NodeId>>,
        ) -> Result<CacheRefreshStats, ApiError>,
    {
        let result = (|| {
            let mut storage = Storage::open(storage_path)
                .map_err(|e| ApiError::internal(format!("Failed to reopen storage: {e}")))?;
            rebuild(&mut storage, llm_refresh_scope)
        })();

        self.clear_search_state();
        self.state.lock().is_indexing = false;

        result
    }

    pub fn indexed_files(&self, req: IndexedFilesRequest) -> Result<IndexedFilesDto, ApiError> {
        self.ensure_consistent_read_state("Files")?;
        let root = self.require_project_root()?;
        let storage = self.open_storage_read_only()?;
        let publication = storage
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read source policy exclusion publication identity: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "source_verification_failed",
                    "Indexed-file coverage requires a complete core publication.",
                )
            })?;
        validate_source_policy_exclusions(
            &storage,
            &root,
            &publication,
            &self.source_index_policy,
        )?;
        validate_structural_text_units(&storage, &publication)?;
        let source_policy_exclusions = storage.get_source_policy_exclusions().map_err(|error| {
            ApiError::internal(format!("Failed to load source policy exclusions: {error}"))
        })?;
        let mut files = storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load indexed files: {e}")))?;
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let errors = storage
            .get_errors(None)
            .map_err(|e| ApiError::internal(format!("Failed to load index errors: {e}")))?;
        let verified_file_ids = storage
            .files()
            .inventory()
            .map_err(|e| ApiError::internal(format!("Failed to load file inventory: {e}")))?
            .into_iter()
            .filter_map(|file| file.content_hash.map(|_| file.id))
            .collect::<HashSet<_>>();
        let mut errors_by_file = HashMap::<i64, u32>::new();
        let mut coverage_reasons_by_file = HashMap::<i64, Vec<FileCoverageReason>>::new();
        for error in errors {
            if let Some(file_id) = error.file_id {
                *errors_by_file.entry(file_id.0).or_default() += 1;
                coverage_reasons_by_file.entry(file_id.0).or_default().push(
                    error
                        .coverage_reason
                        .unwrap_or(FileCoverageReason::CollectorFailure),
                );
            }
        }

        let mut language_counts = BTreeMap::<String, u32>::new();
        let mut incomplete_reason_counts = BTreeMap::<String, (u32, String)>::new();
        let mut indexed_file_count = 0_u32;
        let mut incomplete_file_count = 0_u32;
        let mut error_file_count = 0_u32;
        for file in &files {
            *language_counts.entry(file.language.clone()).or_default() += 1;
            indexed_file_count += u32::from(file.indexed);
            incomplete_file_count += u32::from(!file.complete);
            error_file_count += u32::from(errors_by_file.contains_key(&file.id));
            if let Some(reason) = file_coverage_reason(
                file,
                &coverage_reasons_by_file,
                verified_file_ids.contains(&file.id),
            ) {
                let entry = incomplete_reason_counts
                    .entry(reason.as_str().to_string())
                    .or_insert_with(|| (0, file_coverage_detail(reason).to_string()));
                entry.0 += 1;
            }
        }
        let coverage_gaps = files
            .iter()
            .filter_map(|file| {
                let verified_source = verified_file_ids.contains(&file.id);
                file_coverage_reason(file, &coverage_reasons_by_file, verified_source).map(
                    |reason| FileCoverageDiagnosticDto {
                        path: runtime_relative_path(&root, &file.path),
                        reason,
                        retryable: file_coverage_retryable(reason),
                        verified_source,
                        projection_available: file.indexed && verified_source,
                    },
                )
            })
            .collect::<Vec<_>>();

        let path_filter = req.path_contains.as_deref().map(normalize_path_key);
        let language_filter = req.language.as_deref().map(str::to_ascii_lowercase);
        let policy_exclusion_count = source_policy_exclusions.len().min(u32::MAX as usize) as u32;
        let mut policy_exclusions = source_policy_exclusions
            .into_iter()
            .filter(|entry| {
                let role = path_role_from_key(&normalize_path_key(&entry.normalized_path));
                req.role.is_none_or(|requested| requested == role)
                    && path_filter.as_deref().is_none_or(|needle| {
                        normalize_path_key(&entry.normalized_path).contains(needle)
                    })
                    && language_filter.as_deref().is_none_or(|language| {
                        indexed_file_matches_language_filter(
                            "unknown",
                            Path::new(&entry.normalized_path),
                            language,
                        )
                    })
            })
            .map(|entry| SourcePolicyExclusionDto {
                role: path_role_from_key(&normalize_path_key(&entry.normalized_path)),
                path: entry.normalized_path,
                content_hash: entry.content_hash,
                observed_size: entry.observed_size,
                observed_unit_count: entry.observed_unit_count,
                policy_version: entry.policy_version,
                byte_cap: entry.byte_cap,
                structural_unit_cap: entry.structural_unit_cap,
                project_id: entry.project_id,
                workspace_id: entry.workspace_id,
                core_generation_id: entry.core_generation_id,
                core_run_id: entry.core_run_id,
                graph_coverage: false,
                semantic_coverage: false,
            })
            .collect::<Vec<_>>();
        policy_exclusions.truncate(5_000);
        let mut visible = files
            .into_iter()
            .filter(|file| {
                let role = indexed_file_role(&file.path);
                req.role.is_none_or(|requested| requested == role)
                    && path_filter.as_deref().is_none_or(|needle| {
                        normalize_path_key(&runtime_relative_path(&root, &file.path))
                            .contains(needle)
                    })
                    && language_filter.as_deref().is_none_or(|language| {
                        indexed_file_matches_language_filter(&file.language, &file.path, language)
                    })
            })
            .map(|file| IndexedFileDto {
                path: runtime_relative_path(&root, &file.path),
                language: file.language,
                indexed: file.indexed,
                complete: file.complete,
                line_count: file.line_count,
                role: indexed_file_role(&file.path),
                error_count: errors_by_file.get(&file.id).copied().unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let limit = req.limit.unwrap_or(500).clamp(1, 5000) as usize;
        let filtered_file_count = visible.len().min(u32::MAX as usize) as u32;
        let truncated = visible.len() > limit;
        visible.truncate(limit);
        let visible_file_count = visible.len().min(u32::MAX as usize) as u32;

        let mut coverage_notes = Vec::new();
        if incomplete_file_count > 0 || error_file_count > 0 {
            coverage_notes.push(format!(
                "index usable with {incomplete_file_count} incomplete files and {error_file_count} files carrying index errors"
            ));
        } else {
            coverage_notes.push("index usable; no file-level index errors recorded".to_string());
        }
        if policy_exclusion_count > 0 {
            coverage_notes.push(format!(
                "{policy_exclusion_count} verified source policy exclusions have no parser-backed graph or semantic coverage"
            ));
        }
        let language_counts = language_counts
            .into_iter()
            .map(|(language, file_count)| {
                let support = language_support_summary_for_language(&language);
                IndexedFileLanguageCountDto {
                    language,
                    file_count,
                    support_mode: support.support_mode,
                    evidence_tier: support.evidence_tier,
                    claim_label: support.claim_label,
                }
            })
            .collect::<Vec<_>>();
        let incomplete_reason_counts = incomplete_reason_counts
            .into_iter()
            .map(
                |(reason, (file_count, detail))| IndexedFileIncompleteReasonCountDto {
                    reason,
                    file_count,
                    detail,
                },
            )
            .collect::<Vec<_>>();
        let file_count = language_counts
            .iter()
            .map(|entry| entry.file_count)
            .sum::<u32>();

        Ok(IndexedFilesDto {
            project_root: root.to_string_lossy().to_string(),
            usable: indexed_file_count > 0,
            summary: IndexedFilesSummaryDto {
                file_count,
                indexed_file_count,
                filtered_file_count,
                visible_file_count,
                incomplete_file_count,
                error_file_count,
                policy_exclusion_count,
                incomplete_reason_counts,
                truncated,
                language_counts,
                framework_route_coverage: framework_route_coverage_matrix(),
                coverage_notes,
            },
            coverage_gaps,
            policy_exclusions,
            files: visible,
        })
    }

    pub(crate) fn index_freshness(&self) -> Result<IndexFreshnessDto, ApiError> {
        let root = self.require_project_root()?;
        let storage_path = self.require_storage_path()?;
        let storage = self.open_storage_for_freshness()?;
        let workspace = runtime_workspace_manifest(&root, &storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let freshness =
            self.cached_index_freshness_from_storage(&root, &storage_path, &workspace, &storage);
        Ok(freshness)
    }

    /// Return the durable identity of the core database generation at the live path.
    pub fn index_publication(&self) -> Result<Option<IndexPublicationRecord>, ApiError> {
        let storage_path = self.require_storage_path()?;
        Store::database_index_publication(&storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to read index publication identity: {error}"
            ))
        })
    }

    /// Return the durable publication only when the live database is not fenced
    /// by an incomplete legacy incremental run.
    pub fn complete_index_publication(&self) -> Result<Option<IndexPublicationRecord>, ApiError> {
        let storage_path = self.require_storage_path()?;
        Store::database_complete_index_publication(&storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to read complete index publication: {error}"
            ))
        })
    }

    fn cached_index_freshness_from_storage(
        &self,
        root: &Path,
        storage_path: &Path,
        workspace: &WorkspaceManifest,
        storage: &Storage,
    ) -> IndexFreshnessDto {
        if !matches!(storage.has_incomplete_incremental_run(), Ok(false)) {
            self.state.lock().index_freshness_cache = None;
            return index_freshness_from_storage_with_policy(
                root,
                workspace,
                storage,
                &self.source_index_policy,
            );
        }
        let ttl = Duration::from_secs(index_freshness_cache_ttl_secs());
        let storage_fingerprint = storage_fingerprint(storage_path);
        {
            let state = self.state.lock();
            if let Some(cached) = state.index_freshness_cache.as_ref()
                && cached.root == root
                && cached.storage_path == storage_path
                && cached.storage_fingerprint == storage_fingerprint
                && cached.cached_at.elapsed() < ttl
            {
                return cached.value.clone();
            }
        }

        let freshness = index_freshness_from_storage_with_policy(
            root,
            workspace,
            storage,
            &self.source_index_policy,
        );
        let mut state = self.state.lock();
        state.index_freshness_cache = Some(CachedIndexFreshness {
            root: root.to_path_buf(),
            storage_path: storage_path.to_path_buf(),
            storage_fingerprint,
            value: freshness.clone(),
            cached_at: Instant::now(),
        });
        freshness
    }
}
