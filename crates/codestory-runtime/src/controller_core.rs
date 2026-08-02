use crate::browser::ReadOnlyBrowserService;
use crate::index_freshness::{
    FreshnessObservation, FreshnessObservationPolicy, index_freshness_from_storage_with_policy,
    open_existing_storage_for_read, open_storage_for_read,
};
use crate::search_publication::{
    load_persisted_search_state_for_runtime, retrieval_state_from_storage_for_runtime,
};
use crate::services::{
    AgentService, BookmarkService, GroundingService, IndexService, ProjectService,
    PublicOperationService, SearchService, TrailService,
};
use crate::workspace_state::runtime_workspace_manifest;
use crate::{
    ACTIVE_CORE_READ, ActiveCoreRead, ActiveCoreReadGuard, AppController, AppState, ReadStorage,
    RuntimeProcessConfig, SidecarQueryCacheState, SourceObserverState, Storage,
    clear_search_engine, publish_search_engine,
};
use codestory_contracts::api::{
    ApiError, AppEventPayload, IndexFreshnessDto, ProjectSummary, RetrievalStateDto,
};
use codestory_store::{IndexPublicationRecord, Store};
use codestory_workspace::SourceIndexPolicy;
use crossbeam_channel::{Receiver, unbounded};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

pub(crate) fn no_project_error() -> ApiError {
    ApiError::invalid_argument("No project open. Call open_project first.")
}

impl AppController {
    pub fn new() -> Self {
        Self::new_with_process_config(RuntimeProcessConfig::local())
    }

    pub fn new_with_config(config: codestory_retrieval::SidecarRuntimeConfig) -> Self {
        Self::new_with_process_config(RuntimeProcessConfig::new(
            config,
            SourceIndexPolicy::default(),
        ))
    }

    pub(crate) fn new_with_process_config(config: RuntimeProcessConfig) -> Self {
        Self::new_with_source_index_policy(config.sidecar, config.source_index_policy)
    }

    pub(crate) fn new_with_source_index_policy(
        config: codestory_retrieval::SidecarRuntimeConfig,
        source_index_policy: SourceIndexPolicy,
    ) -> Self {
        let (events_tx, events_rx) = unbounded();
        Self {
            state: Arc::new(Mutex::new(AppState {
                project_root: None,
                storage_path: None,
                node_names: HashMap::new(),
                search_engine: None,
                search_publication: None,
                observed_core_publication: None,
                is_indexing: false,
                index_freshness_cache: None,
                #[cfg(test)]
                last_hybrid_instrumentation: None,
            })),
            sidecar_query_cache: Arc::new(Mutex::new(SidecarQueryCacheState::new())),
            canonical_symbol_names: Arc::new(Mutex::new(Default::default())),
            source_observer: Arc::new(Mutex::new(SourceObserverState::default())),
            events_tx,
            events_rx,
            runtime_config: Arc::new(config),
            source_index_policy: Arc::new(source_index_policy),
        }
    }

    pub fn project_service(&self) -> ProjectService {
        ProjectService::new(self.clone())
    }

    pub fn search_service(&self) -> SearchService {
        SearchService::new(self.clone())
    }

    pub fn grounding_service(&self) -> GroundingService {
        GroundingService::new(self.clone())
    }

    pub fn index_service(&self) -> IndexService {
        IndexService::new(self.clone())
    }

    pub fn trail_service(&self) -> TrailService {
        TrailService::new(self.clone())
    }

    pub fn agent_service(&self) -> AgentService {
        AgentService::new(self.clone())
    }

    pub fn bookmark_service(&self) -> BookmarkService {
        BookmarkService::new(self.clone())
    }

    pub fn browser_service(&self) -> ReadOnlyBrowserService {
        ReadOnlyBrowserService::new(self.clone(), PublicOperationService::new(self.clone()))
    }

    /// Subscribe to backend events. Intended to be consumed by a single pump
    /// that forwards to the active runtime.
    pub fn events(&self) -> Receiver<AppEventPayload> {
        self.events_rx.clone()
    }

    pub(crate) fn require_project_root(&self) -> Result<PathBuf, ApiError> {
        self.state
            .lock()
            .project_root
            .clone()
            .ok_or_else(no_project_error)
    }

    pub(crate) fn require_storage_path(&self) -> Result<PathBuf, ApiError> {
        self.state
            .lock()
            .storage_path
            .clone()
            .ok_or_else(no_project_error)
    }

    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.state) as usize
    }

    pub(crate) fn runtime_configuration_id(&self) -> Result<String, ApiError> {
        let storage_path = self.require_storage_path()?;
        let project_cache_root = storage_path.parent().unwrap_or(storage_path.as_path());
        Ok(RuntimeProcessConfig::new(
            self.runtime_config.as_ref().clone(),
            self.source_index_policy.as_ref().clone(),
        )
        .configuration_id(project_cache_root))
    }

    pub(crate) fn open_storage(&self) -> Result<Storage, ApiError> {
        let storage_path = self.require_storage_path()?;
        Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))
    }

    pub(crate) fn open_storage_read_only(&self) -> Result<ReadStorage, ApiError> {
        if let Some(storage) = ACTIVE_CORE_READ.with(|active| {
            active
                .borrow()
                .as_ref()
                .filter(|active| active.controller_identity == self.identity())
                .map(|active| Rc::clone(&active.storage))
        }) {
            return Ok(ReadStorage::Pinned(storage));
        }
        let storage_path = self.require_storage_path()?;
        open_existing_storage_for_read(&storage_path).map(ReadStorage::Owned)
    }

    pub(crate) fn open_storage_for_freshness(&self) -> Result<ReadStorage, ApiError> {
        if let Some(storage) = ACTIVE_CORE_READ.with(|active| {
            active
                .borrow()
                .as_ref()
                .filter(|active| active.controller_identity == self.identity())
                .map(|active| Rc::clone(&active.storage))
        }) {
            return Ok(ReadStorage::Pinned(storage));
        }
        let storage_path = self.require_storage_path()?;
        Storage::open_freshness_observational(&storage_path)
            .map(ReadStorage::Owned)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to open storage for freshness observation: {error}"
                ))
            })
    }

    pub(crate) fn active_core_publication(&self) -> Option<IndexPublicationRecord> {
        ACTIVE_CORE_READ.with(|active| {
            active
                .borrow()
                .as_ref()
                .filter(|active| active.controller_identity == self.identity())
                .map(|active| active.publication.clone())
        })
    }

    pub(crate) fn active_project_summary(&self) -> Result<ProjectSummary, ApiError> {
        if self.active_core_publication().is_none() {
            return Err(ApiError::internal(
                "Active project summary requires a pinned public operation",
            ));
        }
        let root = self.require_project_root()?;
        let storage_path = self.require_storage_path()?;
        let storage = self.open_storage_read_only()?;
        self.project_summary_from_storage(&root, &storage_path, &storage)
    }

    pub(crate) fn with_complete_core_snapshot<T>(
        &self,
        build: impl FnOnce(&IndexPublicationRecord) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        if let Some(publication) = self.active_core_publication() {
            return build(&publication);
        }
        let storage_path = self.require_storage_path()?;
        let storage = Rc::new(open_existing_storage_for_read(&storage_path)?);
        let installed_storage = Rc::clone(&storage);
        let snapshot = storage.read_snapshot().map_err(|error| {
            ApiError::internal(format!(
                "Failed to begin public operation snapshot: {error}"
            ))
        })?;
        let publication = snapshot
            .storage()
            .get_complete_index_publication()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read public operation publication: {error}"
                ))
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "project_unavailable",
                    "no complete core publication is available",
                )
            })?;
        let previous = ACTIVE_CORE_READ.with(|active| {
            active.replace(Some(ActiveCoreRead {
                controller_identity: self.identity(),
                storage: installed_storage,
                publication: publication.clone(),
            }))
        });
        let guard = ActiveCoreReadGuard { previous };
        let result = build(&publication);
        drop(guard);
        snapshot.finish().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finish public operation snapshot: {error}"
            ))
        })?;
        let live = Store::database_complete_index_publication(&storage_path).map_err(|error| {
            ApiError::internal(format!("Failed to revalidate public operation: {error}"))
        })?;
        if live.as_ref() != Some(&publication) {
            return Err(ApiError::new(
                "publication_changed",
                "the complete core publication changed during the public operation",
            ));
        }
        result
    }

    pub(crate) fn index_freshness_uncached(
        &self,
        observation: FreshnessObservationPolicy,
    ) -> Result<IndexFreshnessDto, ApiError> {
        let root = self.require_project_root()?;
        let storage = self.open_storage_for_freshness()?;
        let storage_path = self.require_storage_path()?;
        let workspace = runtime_workspace_manifest(&root, &storage_path)
            .map_err(|error| ApiError::internal(format!("Failed to open project: {error}")))?;
        let session = match observation {
            FreshnessObservationPolicy::Unobserved => None,
            FreshnessObservationPolicy::ObserveSourceRoot => self.source_observer_session(&root),
        };
        Ok(index_freshness_from_storage_with_policy(
            &root,
            &workspace,
            &storage,
            &self.source_index_policy,
            session.as_deref().map_or(
                FreshnessObservation::Unobserved,
                FreshnessObservation::Observed,
            ),
        ))
    }

    /// Derive source freshness from content, ignoring any verdict the current
    /// operation already memoized.
    ///
    /// `index_freshness_uncached` bypasses the *time-based* freshness cache but
    /// still consults `codestory_workspace`'s operation-scoped verdict memo, so
    /// on its own it reports what the operation observed at its first
    /// derivation. That is correct for a pre-flight admission check and wrong
    /// for any check that must see drift which happened since: a mutation that
    /// preserves both mtime and byte length is invisible to metadata, and the
    /// content hash is the only mechanism that catches it. Callers guarding the
    /// *end* of an operation use this entry point so the hash runs again.
    ///
    /// The observation policy is passed straight through: dropping the memo
    /// governs what the scan is allowed to remember, arming the observer
    /// governs what the scan is allowed to miss, and the end-of-operation
    /// refusal needs both.
    pub(crate) fn index_freshness_reverified(
        &self,
        observation: FreshnessObservationPolicy,
    ) -> Result<IndexFreshnessDto, ApiError> {
        codestory_workspace::reverify_source_freshness_from_content();
        self.index_freshness_uncached(observation)
    }

    /// Arm, or reuse, the filesystem observer over `root`.
    ///
    /// Arming is the expensive half — a recursive watch over the working tree — so one session
    /// serves every observed read of the same root. A session that has already lost coverage is
    /// kept rather than replaced: it seals indeterminate from then on, which is exactly the
    /// unobserved fallback, and re-arming on every read would pay the watch cost repeatedly on a
    /// host that is producing more churn than the notifier can carry.
    pub(crate) fn source_observer_session(
        &self,
        root: &Path,
    ) -> Option<Arc<codestory_workspace::filesystem_observer::FilesystemObserverSession>> {
        use codestory_workspace::filesystem_observer::FilesystemObserverSession;

        let mut state = self.source_observer.lock();
        if state.root.as_deref() != Some(root) {
            *state = SourceObserverState {
                root: Some(root.to_path_buf()),
                #[cfg(any(test, feature = "test-support"))]
                arm_requests: state.arm_requests,
                ..SourceObserverState::default()
            };
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            state.arm_requests = state.arm_requests.saturating_add(1);
        }
        if let Some(session) = state.session.as_ref() {
            return Some(Arc::clone(session));
        }
        if state.refused.is_some() {
            return None;
        }
        match FilesystemObserverSession::arm(root) {
            Ok(session) => {
                let session = Arc::new(session);
                state.session = Some(Arc::clone(&session));
                Some(session)
            }
            Err(gap) => {
                tracing::debug!(
                    root = %root.display(),
                    gap = gap.id(),
                    detail = %gap.detail(),
                    "filesystem observation unavailable; freshness falls back to the scan verdict"
                );
                state.refused = Some(gap);
                None
            }
        }
    }

    /// Install a caller-driven observer session so a test can script the racing window.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)]
    pub(crate) fn install_source_observer_for_test(
        &self,
        root: &Path,
        session: Arc<codestory_workspace::filesystem_observer::FilesystemObserverSession>,
    ) {
        let mut state = self.source_observer.lock();
        state.root = Some(root.to_path_buf());
        state.session = Some(session);
        state.refused = None;
    }

    /// How many times a read asked this controller for an observer session.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)]
    pub(crate) fn source_observer_requests_for_test(&self) -> u64 {
        self.source_observer.lock().arm_requests
    }

    pub(crate) fn clear_search_state(&self) {
        let mut s = self.state.lock();
        s.node_names.clear();
        clear_search_engine(&mut s);
        self.sidecar_query_cache.lock().clear();
        self.canonical_symbol_names.lock().clear();
    }

    pub(crate) fn ensure_consistent_read_state(&self, operation: &str) -> Result<(), ApiError> {
        if self.state.lock().is_indexing {
            return Err(ApiError::invalid_argument(format!(
                "{operation} is unavailable while indexing is in progress. Retry after indexing completes."
            )));
        }
        Ok(())
    }

    pub(crate) fn ensure_search_state(&self) -> Result<(), ApiError> {
        let pinned_publication = self.active_core_publication();
        if let Some(publication) = pinned_publication.as_ref() {
            let state = self.state.lock();
            if state.search_engine.is_some()
                && state.search_publication.as_ref() == Some(publication)
            {
                return Ok(());
            }
        }
        let storage_path = self.require_storage_path()?;
        let current_publication = Store::database_complete_index_publication(&storage_path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read current search publication: {error}"
                ))
            })?;
        if pinned_publication.as_ref() != current_publication.as_ref()
            && pinned_publication.is_some()
        {
            return Err(ApiError::new(
                "publication_changed",
                "the pinned core publication is no longer the current lexical search generation",
            ));
        }
        {
            let s = self.state.lock();
            if s.search_engine.is_some() && s.search_publication == current_publication {
                return Ok(());
            }
        }

        let mut attempts = 0;
        let loaded = loop {
            let mut storage = open_storage_for_read(&storage_path)?;
            match load_persisted_search_state_for_runtime(
                &mut storage,
                &storage_path,
                &self.runtime_config,
            ) {
                Ok(state) => break state,
                Err(error) if error.code == "cache_busy" && attempts == 0 => attempts += 1,
                Err(error) => return Err(error),
            }
        };
        if pinned_publication.as_ref() != loaded.publication.as_ref()
            && pinned_publication.is_some()
        {
            return Err(ApiError::new(
                "publication_changed",
                "the pinned core publication does not match the loaded lexical search generation",
            ));
        }

        let mut s = self.state.lock();
        if s.search_engine.is_none() || s.search_publication != loaded.publication {
            s.node_names = loaded.node_names;
            publish_search_engine(&mut s, loaded.engine, loaded.publication);
        }

        Ok(())
    }

    pub fn retrieval_state(&self) -> Result<RetrievalStateDto, ApiError> {
        let project_root = self.require_project_root()?;
        let storage = self.open_storage_read_only()?;
        retrieval_state_from_storage_for_runtime(&storage, &project_root, &self.runtime_config)
    }
}
