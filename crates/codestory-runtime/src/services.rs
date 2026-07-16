use codestory_contracts::api::{
    AffectedAnalysisDto, AffectedAnalysisRequest, AgentAnswerDto, AgentAskRequest,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, ApiError, BookmarkCategoryDto,
    BookmarkDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest,
    EmbeddingVectorPublicationIdentityDto, GroundingBudgetDto, GroundingSnapshotDto,
    IndexDryRunDto, IndexFreshnessStatusDto, IndexMode, IndexPublicationDto, IndexedFilesDto,
    IndexedFilesRequest, IndexingPhaseTimings, ListChildrenSymbolsRequest, ListRootSymbolsRequest,
    NodeDetailsDto, NodeDetailsRequest, NodeId, OpenDefinitionRequest, OpenProjectRequest,
    ProjectSummary, RetrievalStateDto, SearchHit, SearchRequest, SearchResultsDto,
    SnippetContextDto, SourceOccurrenceDto, StartIndexingRequest, SummaryGenerationDto,
    SymbolContextDto, SymbolSummaryDto, SystemActionResponse, TrailConfigDto, TrailContextDto,
};

use crate::AppController;
use codestory_indexer::CancellationToken;
use codestory_store::IndexPublicationRecord;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_ACTIVATION_FOREGROUND_BUDGET: Duration = Duration::from_secs(5);
const ACTIVATION_WAIT_SLICE: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationStage {
    Discovery,
    CoreFreshness,
    DensePreparation,
    Validation,
    Publication,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationState {
    Preparing,
    Updating,
    Ready,
    Retryable,
    Unavailable,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationCapabilityState {
    Ready,
    Retryable,
    Unavailable,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivationCapabilities {
    pub local_navigation: ActivationCapabilityState,
    pub broad_search: ActivationCapabilityState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivationSnapshot {
    pub operation_id: String,
    pub state: ActivationState,
    pub stage: ActivationStage,
    pub attempt: u32,
    pub retry_after_ms: Option<u64>,
    pub failure: Option<String>,
    pub capabilities: ActivationCapabilities,
}

impl ActivationSnapshot {
    pub fn allows_operation(&self, operation: &str) -> bool {
        if operation_requires_retrieval(operation) {
            self.capabilities.broad_search == ActivationCapabilityState::Ready
        } else {
            self.capabilities.local_navigation == ActivationCapabilityState::Ready
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivationRun {
    pub snapshot: ActivationSnapshot,
    pub joined: bool,
}

#[derive(Default)]
struct ActivationCoordinatorState {
    target: Option<ActivationTarget>,
    current: Option<ActivationSnapshot>,
    running: bool,
    current_cancel: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone)]
struct ActivationTarget {
    project: codestory_workspace::ProjectIdentityV3,
    storage_path: PathBuf,
}

impl ActivationTarget {
    fn new(project_root: &Path, storage_path: &Path) -> Self {
        Self {
            project: codestory_workspace::project_identity_v3(project_root),
            storage_path: storage_path
                .canonicalize()
                .unwrap_or_else(|_| storage_path.to_path_buf()),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.project == other.project
            && codestory_workspace::same_workspace_path(&self.storage_path, &other.storage_path)
    }
}

#[derive(Default)]
struct ActivationCoordinator {
    state: Mutex<ActivationCoordinatorState>,
    changed: Condvar,
    next_id: AtomicU64,
}

/// Runtime-owned single-flight activation for one logical project, core store,
/// and immutable runtime configuration. The configuration is fixed by the
/// controller owned by this service.
#[derive(Clone)]
pub struct ActivationService {
    coordinator: Arc<ActivationCoordinator>,
    controller: AppController,
}

impl ActivationService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self {
            coordinator: Arc::new(ActivationCoordinator::default()),
            controller,
        }
    }

    pub fn snapshot(&self) -> Option<ActivationSnapshot> {
        self.coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned")
            .current
            .clone()
    }

    pub fn activate_project(
        &self,
        project_root: &Path,
        storage_path: &Path,
        cancelled: Arc<AtomicBool>,
    ) -> Result<ActivationRun, ApiError> {
        self.activate_project_with_foreground_budget(
            project_root,
            storage_path,
            cancelled,
            DEFAULT_ACTIVATION_FOREGROUND_BUDGET,
        )
    }

    pub fn activate_project_with_foreground_budget(
        &self,
        project_root: &Path,
        storage_path: &Path,
        request_cancelled: Arc<AtomicBool>,
        foreground_budget: Duration,
    ) -> Result<ActivationRun, ApiError> {
        if request_cancelled.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "cancelled",
                "request cancelled before project activation",
            ));
        }
        let target = ActivationTarget::new(project_root, storage_path);
        let mut state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        let joined = state.running;
        let operation_id = if joined {
            if !state
                .target
                .as_ref()
                .is_some_and(|current| current.matches(&target))
            {
                return Err(ApiError::new(
                    "project_unavailable",
                    "a different logical project is already activating in this runtime context",
                ));
            }
            let operation_id = state
                .current
                .as_ref()
                .expect("running activation has a snapshot")
                .operation_id
                .clone();
            drop(state);
            operation_id
        } else {
            if !state
                .target
                .as_ref()
                .is_some_and(|current| current.matches(&target))
            {
                state.target = Some(target.clone());
                state.current = None;
            }

            let operation_id = if let Some(snapshot) = state
                .current
                .as_mut()
                .filter(|snapshot| snapshot.state == ActivationState::Retryable)
            {
                snapshot.attempt += 1;
                snapshot.failure = None;
                snapshot.retry_after_ms = Some(250);
                snapshot.state = ActivationState::Preparing;
                snapshot.stage = ActivationStage::Discovery;
                snapshot.operation_id.clone()
            } else {
                let operation_id = format!(
                    "activation-{}",
                    self.coordinator.next_id.fetch_add(1, Ordering::Relaxed) + 1
                );
                state.current = Some(ActivationSnapshot {
                    operation_id: operation_id.clone(),
                    state: ActivationState::Preparing,
                    stage: ActivationStage::Discovery,
                    attempt: 1,
                    retry_after_ms: Some(250),
                    failure: None,
                    capabilities: ActivationCapabilities {
                        local_navigation: ActivationCapabilityState::Unavailable,
                        broad_search: ActivationCapabilityState::Unavailable,
                    },
                });
                operation_id
            };
            let activation_cancelled = Arc::new(AtomicBool::new(false));
            state.running = true;
            state.current_cancel = Some(Arc::clone(&activation_cancelled));
            drop(state);

            let operation = ActivationOperation {
                service: self.clone(),
                operation_id: operation_id.clone(),
                cancelled: activation_cancelled,
            };
            let worker_operation = operation.clone();
            let worker_service = self.clone();
            let worker_project_root = project_root.to_path_buf();
            let worker_storage_path = storage_path.to_path_buf();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("codestory-{operation_id}"))
                .spawn(move || {
                    let result = worker_service
                        .activate_once(&worker_operation, worker_project_root, worker_storage_path)
                        .map_err(classify_activation_api_error);
                    worker_operation.finish(result.as_ref().err());
                })
            {
                let error = ApiError::new(
                    "project_unavailable",
                    format!("failed to start project activation worker: {error}"),
                );
                operation.finish(Some(&error));
                return Err(error);
            }
            operation_id
        };

        self.wait_for_activation(
            &target,
            &operation_id,
            joined,
            request_cancelled.as_ref(),
            foreground_budget,
        )
    }

    fn wait_for_activation(
        &self,
        target: &ActivationTarget,
        operation_id: &str,
        joined: bool,
        request_cancelled: &AtomicBool,
        foreground_budget: Duration,
    ) -> Result<ActivationRun, ApiError> {
        let deadline = Instant::now()
            .checked_add(foreground_budget)
            .unwrap_or_else(Instant::now);
        let mut state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        loop {
            if request_cancelled.load(Ordering::Acquire) {
                return Err(ApiError::new(
                    "cancelled",
                    "request cancelled while waiting for shared project activation",
                ));
            }
            if !state
                .target
                .as_ref()
                .is_some_and(|current| current.matches(target))
            {
                return Err(ApiError::new(
                    "project_unavailable",
                    "the project activation target changed while the request was waiting",
                ));
            }
            let snapshot = state
                .current
                .clone()
                .filter(|snapshot| snapshot.operation_id == operation_id)
                .ok_or_else(|| {
                    ApiError::new(
                        "project_unavailable",
                        "the shared project activation operation changed while the request was waiting",
                    )
                })?;
            if !state.running {
                return if snapshot_allows(&snapshot) {
                    Ok(ActivationRun { snapshot, joined })
                } else {
                    Err(snapshot_error(&snapshot))
                };
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(activation_preparing_error(&snapshot));
            }
            let remaining = deadline.saturating_duration_since(now);
            state = self
                .coordinator
                .changed
                .wait_timeout(state, remaining.min(ACTIVATION_WAIT_SLICE))
                .expect("activation coordinator poisoned")
                .0;
        }
    }

    pub fn cancel_and_wait(&self) {
        let mut state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        if let Some(cancelled) = state.current_cancel.as_ref() {
            cancelled.store(true, Ordering::Release);
        }
        while state.running {
            state = self
                .coordinator
                .changed
                .wait(state)
                .expect("activation coordinator poisoned");
        }
    }

    fn activate_once(
        &self,
        operation: &ActivationOperation,
        project_root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<(), ApiError> {
        operation.ensure_not_cancelled("project discovery")?;
        let mut summary = self
            .controller
            .open_project_with_storage_path(project_root.clone(), storage_path.clone())?;

        operation.set_stage(ActivationStage::CoreFreshness);
        let core_stale = summary.publication.is_none()
            || summary.stats.node_count == 0
            || summary
                .freshness
                .as_ref()
                .is_none_or(|freshness| freshness.status != IndexFreshnessStatusDto::Fresh);
        if core_stale {
            let mode = if summary.publication.is_none() || summary.stats.node_count == 0 {
                IndexMode::Full
            } else {
                IndexMode::Incremental
            };
            let token = CancellationToken::from_shared_flag(Arc::clone(&operation.cancelled));
            self.controller
                .run_indexing_blocking_with_cancel(mode, &token)?;
            operation.ensure_not_cancelled("core publication validation")?;
            summary = self
                .controller
                .open_project_with_storage_path(project_root.clone(), storage_path.clone())?;
        }
        let local_ready = summary.publication.is_some()
            && summary.stats.node_count > 0
            && summary.stats.fatal_error_count == 0
            && summary
                .freshness
                .as_ref()
                .is_some_and(|freshness| freshness.status == IndexFreshnessStatusDto::Fresh);
        if !local_ready {
            return Err(ApiError::new(
                "project_unavailable",
                "activation did not produce a fresh complete core publication",
            ));
        }
        operation.set_capability(false, ActivationCapabilityState::Ready);

        operation.ensure_not_cancelled("dense preparation")?;
        operation.set_stage(ActivationStage::DensePreparation);
        codestory_retrieval::ensure_product_embedding_backend_for_runtime(
            &self.controller.runtime_config,
        )
        .map_err(map_activation_error)?;
        operation.ensure_not_cancelled("retrieval publication")?;
        operation.set_stage(ActivationStage::Publication);
        codestory_retrieval::finalize_index_for_runtime_with_cancel(
            &project_root,
            &storage_path,
            &self.controller.runtime_config,
            operation.cancelled.as_ref(),
        )
        .map_err(map_activation_error)?;
        operation.ensure_not_cancelled("retrieval validation")?;
        operation.set_stage(ActivationStage::Validation);
        let status = codestory_retrieval::strict_sidecar_status_for_runtime(
            &project_root,
            Some(&storage_path),
            self.controller.runtime_config.as_ref().clone(),
        )
        .map_err(map_activation_error)?;
        if !status.is_live_ready() {
            return Err(ApiError::new(
                "project_unavailable",
                "retrieval publication is not live-ready after activation",
            ));
        }
        operation.set_capability(true, ActivationCapabilityState::Ready);
        Ok(())
    }
}

fn operation_requires_retrieval(operation: &str) -> bool {
    matches!(
        operation,
        "packet" | "search" | "context" | "drill" | "resolution" | "graph_assisted"
    )
}

fn snapshot_allows(snapshot: &ActivationSnapshot) -> bool {
    snapshot.allows_operation("packet")
}

fn snapshot_error(snapshot: &ActivationSnapshot) -> ApiError {
    let code = match snapshot.state {
        ActivationState::Cancelled => "cancelled",
        ActivationState::Retryable => "activation_retryable",
        _ => "project_unavailable",
    };
    ApiError::new(
        code,
        snapshot.failure.clone().unwrap_or_else(|| {
            "project activation did not provide the requested capability".into()
        }),
    )
}

fn activation_preparing_error(snapshot: &ActivationSnapshot) -> ApiError {
    ApiError::new(
        "activation_preparing",
        format!(
            "project activation {} is still {:?} at {:?}; retry after {}ms",
            snapshot.operation_id,
            snapshot.state,
            snapshot.stage,
            snapshot.retry_after_ms.unwrap_or(250)
        ),
    )
}

fn map_activation_error(error: anyhow::Error) -> ApiError {
    classify_activation_api_error(ApiError::new("project_unavailable", error.to_string()))
}

fn classify_activation_api_error(error: ApiError) -> ApiError {
    if matches!(
        error.code.as_str(),
        "cancelled" | "activation_preparing" | "activation_retryable"
    ) {
        return error;
    }
    let normalized = error.message.to_ascii_lowercase();
    if normalized.contains("cancel") {
        ApiError::new("cancelled", error.message)
    } else if [
        "cache_busy",
        "database is locked",
        "database table is locked",
        "writer lock",
        "publication changed",
        "input changed",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        ApiError::new("activation_retryable", error.message)
    } else {
        ApiError::new("project_unavailable", error.message)
    }
}

#[derive(Clone)]
pub struct ActivationOperation {
    service: ActivationService,
    operation_id: String,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct PublicOperation<T> {
    pub value: T,
    pub core_publication: Option<IndexPublicationDto>,
    pub retrieval_publication: Option<EmbeddingVectorPublicationIdentityDto>,
    pub operation_id: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePublicOperationPublication {
    pub core_publication: IndexPublicationDto,
    pub retrieval_publication: Option<EmbeddingVectorPublicationIdentityDto>,
}

#[derive(Clone)]
pub struct PublicOperationService {
    controller: AppController,
    next_id: Arc<AtomicU64>,
}

impl PublicOperationService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self {
            controller,
            next_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Return the exact publications pinned by the currently executing public
    /// operation. Product caches use this inside the response builder instead
    /// of inferring identity from file metadata or partial sidecar status.
    pub fn active_publication(&self) -> Option<ActivePublicOperationPublication> {
        let core_publication = self
            .controller
            .active_core_publication()
            .map(crate::index_publication_dto)?;
        let retrieval_publication =
            crate::agent::retrieval_primary::active_pinned_retrieval_publication(&self.controller);
        Some(ActivePublicOperationPublication {
            core_publication,
            retrieval_publication,
        })
    }

    /// Run one complete public response under the runtime's retrieval pin and
    /// single bounded publication retry. Host cancellation is checked before
    /// and after every attempt, so adapters do not add a second retry loop.
    pub fn run_with_cancel<T>(
        &self,
        operation: &str,
        cancelled: Arc<AtomicBool>,
        mut build: impl FnMut() -> Result<T, ApiError>,
    ) -> Result<PublicOperation<T>, ApiError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "cancelled",
                format!("request cancelled before {operation}"),
            ));
        }
        let operation_id = format!(
            "public-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        );
        for attempt in 1..=2 {
            let result = self.controller.with_complete_core_snapshot(|publication| {
                let freshness = self.controller.index_freshness_uncached()?;
                if freshness.status != IndexFreshnessStatusDto::Fresh {
                    return Err(ApiError::new(
                        "project_unavailable",
                        format!("{operation} requires a fresh complete core publication"),
                    ));
                }
                let mut run = || {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(ApiError::new(
                            "cancelled",
                            format!("request cancelled before {operation}"),
                        ));
                    }
                    let value = build()?;
                    if cancelled.load(Ordering::Acquire) {
                        return Err(ApiError::new(
                            "cancelled",
                            format!("request cancelled during {operation}"),
                        ));
                    }
                    let after = self.controller.index_freshness_uncached()?;
                    if after.status != IndexFreshnessStatusDto::Fresh {
                        return Err(ApiError::new(
                            "publication_changed",
                            format!("source inputs changed while running {operation}"),
                        ));
                    }
                    Ok(value)
                };
                let (value, retrieval_publication) = if operation_requires_retrieval(operation) {
                    crate::agent::retrieval_primary::with_pinned_retrieval_publication_value(
                        &self.controller,
                        run,
                    )?
                } else {
                    (run()?, None)
                };
                Ok((
                    value,
                    crate::index_publication_dto(publication.clone()),
                    retrieval_publication,
                ))
            });
            match result {
                Ok((value, core_publication, retrieval_publication)) => {
                    return Ok(PublicOperation {
                        value,
                        core_publication: Some(core_publication),
                        retrieval_publication,
                        operation_id,
                        attempt,
                    });
                }
                Err(error)
                    if attempt == 1
                        && matches!(error.code.as_str(), "publication_changed" | "cache_busy") =>
                {
                    tracing::debug!(operation, "retrying pinned public operation");
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("bounded public operation attempts always return")
    }

    pub fn run_observational_with_cancel<T>(
        &self,
        operation: &str,
        cancelled: Arc<AtomicBool>,
        mut build: impl FnMut() -> Result<T, ApiError>,
    ) -> Result<PublicOperation<T>, ApiError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "cancelled",
                format!("request cancelled before {operation}"),
            ));
        }
        let operation_id = format!(
            "resource-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        );
        for attempt in 1..=2 {
            let result = self.controller.with_complete_core_snapshot(|publication| {
                if cancelled.load(Ordering::Acquire) {
                    return Err(ApiError::new(
                        "cancelled",
                        format!("request cancelled before {operation}"),
                    ));
                }
                let mut run = || {
                    let value = build()?;
                    if cancelled.load(Ordering::Acquire) {
                        return Err(ApiError::new(
                            "cancelled",
                            format!("request cancelled during {operation}"),
                        ));
                    }
                    Ok(value)
                };
                let (value, retrieval_publication) = if operation_requires_retrieval(operation) {
                    crate::agent::retrieval_primary::with_pinned_retrieval_publication_value(
                        &self.controller,
                        run,
                    )?
                } else {
                    (run()?, None)
                };
                Ok((
                    value,
                    crate::index_publication_dto(publication.clone()),
                    retrieval_publication,
                ))
            });
            match result {
                Ok((value, core_publication, retrieval_publication)) => {
                    return Ok(PublicOperation {
                        value,
                        core_publication: Some(core_publication),
                        retrieval_publication,
                        operation_id,
                        attempt,
                    });
                }
                Err(error) if attempt == 1 && error.code == "publication_changed" => continue,
                Err(error) => return Err(error),
            }
        }
        unreachable!("bounded observational operation attempts always return")
    }
}

impl ActivationOperation {
    pub fn ensure_not_cancelled(&self, boundary: &str) -> Result<(), ApiError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "cancelled",
                format!("request cancelled before {boundary}"),
            ));
        }
        Ok(())
    }

    pub fn set_stage(&self, stage: ActivationStage) {
        let mut state = self
            .service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        if let Some(snapshot) = state
            .current
            .as_mut()
            .filter(|snapshot| snapshot.operation_id == self.operation_id)
        {
            snapshot.stage = stage;
            snapshot.state = ActivationState::Updating;
        }
        self.service.coordinator.changed.notify_all();
    }

    fn set_capability(&self, broad: bool, capability: ActivationCapabilityState) {
        let mut state = self
            .service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        if let Some(snapshot) = state
            .current
            .as_mut()
            .filter(|snapshot| snapshot.operation_id == self.operation_id)
        {
            if broad {
                snapshot.capabilities.broad_search = capability;
            } else {
                snapshot.capabilities.local_navigation = capability;
            }
        }
        self.service.coordinator.changed.notify_all();
    }

    fn finish(&self, error: Option<&ApiError>) -> ActivationSnapshot {
        let mut state = self
            .service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        let snapshot = state
            .current
            .as_mut()
            .filter(|snapshot| snapshot.operation_id == self.operation_id)
            .expect("activation operation owns current snapshot");
        if let Some(error) = error {
            let capability = match error.code.as_str() {
                "cancelled" => ActivationCapabilityState::Cancelled,
                "activation_retryable" | "cache_busy" | "publication_changed" => {
                    ActivationCapabilityState::Retryable
                }
                _ => ActivationCapabilityState::Unavailable,
            };
            if snapshot.capabilities.local_navigation != ActivationCapabilityState::Ready {
                snapshot.capabilities.local_navigation = capability;
            }
            if snapshot.capabilities.broad_search != ActivationCapabilityState::Ready {
                snapshot.capabilities.broad_search = capability;
            }
            snapshot.state = match capability {
                ActivationCapabilityState::Retryable => ActivationState::Retryable,
                ActivationCapabilityState::Unavailable => ActivationState::Unavailable,
                ActivationCapabilityState::Cancelled => ActivationState::Cancelled,
                ActivationCapabilityState::Ready => ActivationState::Ready,
            };
            snapshot.retry_after_ms =
                (capability == ActivationCapabilityState::Retryable).then_some(250);
            snapshot.failure = Some(error.message.clone());
        } else {
            snapshot.state = ActivationState::Ready;
            snapshot.stage = ActivationStage::Ready;
            snapshot.retry_after_ms = None;
            snapshot.failure = None;
        }
        let snapshot = snapshot.clone();
        state.running = false;
        state.current_cancel = None;
        self.service.coordinator.changed.notify_all();
        snapshot
    }
}

#[derive(Clone)]
pub struct ProjectService {
    controller: AppController,
}

impl ProjectService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn open_project(&self, req: OpenProjectRequest) -> Result<ProjectSummary, ApiError> {
        self.controller.open_project(req)
    }

    pub fn open_project_with_storage_path(
        &self,
        root: std::path::PathBuf,
        storage_path: std::path::PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        self.controller
            .open_project_with_storage_path(root, storage_path)
    }

    pub fn open_project_summary_with_storage_path(
        &self,
        root: std::path::PathBuf,
        storage_path: std::path::PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        self.controller
            .open_project_summary_with_storage_path(root, storage_path)
    }

    /// Observe an existing project store without creating directories,
    /// initializing a database, migrating schema, or binding controller state.
    pub fn inspect_project_summary_with_storage_path(
        &self,
        root: std::path::PathBuf,
        storage_path: std::path::PathBuf,
    ) -> Result<Option<ProjectSummary>, ApiError> {
        self.controller
            .inspect_project_summary_with_storage_path(root, storage_path)
    }

    pub fn complete_index_publication_at(
        &self,
        storage_path: &std::path::Path,
    ) -> Result<Option<IndexPublicationDto>, ApiError> {
        self.controller.complete_index_publication_at(storage_path)
    }

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        self.controller.start_indexing(req)
    }

    pub fn run_indexing_blocking(&self, mode: IndexMode) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller.run_indexing_blocking(mode)
    }

    pub fn run_indexing_blocking_without_runtime_refresh(
        &self,
        mode: IndexMode,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller
            .run_indexing_blocking_without_runtime_refresh(mode)
    }

    pub fn dry_run_index(&self, mode: IndexMode) -> Result<IndexDryRunDto, ApiError> {
        self.controller.dry_run_index(mode)
    }

    pub fn summarize_symbols_blocking(&self) -> Result<SummaryGenerationDto, ApiError> {
        self.controller.summarize_symbols_blocking()
    }
}

#[derive(Clone)]
pub struct IndexService {
    controller: AppController,
}

impl IndexService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        self.controller.start_indexing(req)
    }

    pub fn run_indexing_blocking(&self, mode: IndexMode) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller.run_indexing_blocking(mode)
    }

    pub fn run_indexing_blocking_with_cancel(
        &self,
        mode: IndexMode,
        cancel_token: &CancellationToken,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller
            .run_indexing_blocking_with_cancel(mode, cancel_token)
    }

    /// Run indexing with a host-owned cancellation flag.
    ///
    /// This keeps the indexer's cancellation token behind the runtime service
    /// boundary while allowing transports to share their request lifecycle.
    pub fn run_indexing_blocking_with_cancel_flag(
        &self,
        mode: IndexMode,
        cancelled: Arc<AtomicBool>,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        let cancel_token = CancellationToken::from_shared_flag(cancelled);
        self.controller
            .run_indexing_blocking_with_cancel(mode, &cancel_token)
    }

    pub fn run_indexing_blocking_without_runtime_refresh(
        &self,
        mode: IndexMode,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller
            .run_indexing_blocking_without_runtime_refresh(mode)
    }

    pub fn run_indexing_blocking_without_runtime_refresh_with_cancel(
        &self,
        mode: IndexMode,
        cancel_token: &CancellationToken,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.controller
            .run_indexing_blocking_without_runtime_refresh_with_cancel(mode, cancel_token)
    }

    pub fn complete_index_publication(&self) -> Result<Option<IndexPublicationRecord>, ApiError> {
        self.controller.complete_index_publication()
    }

    pub fn dry_run_index(&self, mode: IndexMode) -> Result<IndexDryRunDto, ApiError> {
        self.controller.dry_run_index(mode)
    }

    pub fn summarize_symbols_blocking(&self) -> Result<SummaryGenerationDto, ApiError> {
        self.controller.summarize_symbols_blocking()
    }
}

#[derive(Clone)]
pub struct SearchService {
    controller: AppController,
}

impl SearchService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn retrieval_state(&self) -> Result<RetrievalStateDto, ApiError> {
        self.controller.retrieval_state()
    }

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        self.controller.search(req)
    }

    pub fn search_results(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        self.controller.search_results(req)
    }

    pub fn indexed_files(&self, req: IndexedFilesRequest) -> Result<IndexedFilesDto, ApiError> {
        self.controller.indexed_files(req)
    }

    pub fn affected_analysis(
        &self,
        req: AffectedAnalysisRequest,
    ) -> Result<AffectedAnalysisDto, ApiError> {
        self.controller.affected_analysis(req)
    }

    pub fn search_hybrid(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: Option<u32>,
        hybrid_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<Vec<SearchHit>, ApiError> {
        self.controller
            .search_hybrid(req, focus_node_id, max_results, hybrid_weights)
    }
}

#[derive(Clone)]
pub struct GroundingService {
    controller: AppController,
}

impl GroundingService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn grounding_snapshot(
        &self,
        budget: GroundingBudgetDto,
    ) -> Result<GroundingSnapshotDto, ApiError> {
        self.controller.grounding_snapshot(budget)
    }

    pub fn symbol_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        self.controller.symbol_context(node_id)
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        self.controller.trail_context(req)
    }

    pub fn snippet_context(
        &self,
        node_id: NodeId,
        context: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        self.controller.snippet_context(node_id, context)
    }

    pub fn snippet_function_body_context(
        &self,
        node_id: NodeId,
        context: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        self.controller
            .snippet_function_body_context(node_id, context)
    }

    pub fn node_details(&self, req: NodeDetailsRequest) -> Result<NodeDetailsDto, ApiError> {
        self.controller.node_details(req)
    }

    pub fn node_occurrences(
        &self,
        req: codestory_contracts::api::NodeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        self.controller.node_occurrences(req)
    }

    pub fn list_root_symbols(
        &self,
        req: ListRootSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.controller.list_root_symbols(req)
    }

    pub fn list_children_symbols(
        &self,
        req: ListChildrenSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        self.controller.list_children_symbols(req)
    }

    pub fn open_definition(
        &self,
        req: OpenDefinitionRequest,
    ) -> Result<SystemActionResponse, ApiError> {
        self.controller.open_definition(req)
    }
}

#[derive(Clone)]
pub struct TrailService {
    controller: AppController,
}

impl TrailService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        self.controller.trail_context(req)
    }
}

#[derive(Clone)]
pub struct AgentService {
    controller: AppController,
}

impl AgentService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn ask(&self, req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        self.controller.agent_ask(req)
    }

    pub fn packet(&self, req: AgentPacketRequestDto) -> Result<AgentPacketDto, ApiError> {
        self.controller.agent_packet(req)
    }
}

#[derive(Clone)]
pub struct BookmarkService {
    controller: AppController,
}

impl BookmarkService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self { controller }
    }

    pub fn list_categories(&self) -> Result<Vec<BookmarkCategoryDto>, ApiError> {
        self.controller.list_bookmark_categories()
    }

    pub fn create_category(
        &self,
        req: CreateBookmarkCategoryRequest,
    ) -> Result<BookmarkCategoryDto, ApiError> {
        self.controller.create_bookmark_category(req)
    }

    pub fn list_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<BookmarkDto>, ApiError> {
        self.controller.list_bookmarks(category_id)
    }

    pub fn create_bookmark(&self, req: CreateBookmarkRequest) -> Result<BookmarkDto, ApiError> {
        self.controller.create_bookmark(req)
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<(), ApiError> {
        self.controller.delete_bookmark(id)
    }
}

#[cfg(test)]
mod activation_tests {
    use super::*;
    use crate::Runtime;
    use std::fs;

    #[test]
    fn activation_target_matches_existing_storage_by_native_identity() {
        let project = tempfile::tempdir().expect("project");
        let storage = project.path().join("codestory.db");
        let alias = project.path().join("codestory-alias.db");
        fs::write(&storage, b"storage").expect("write storage");
        fs::hard_link(&storage, &alias).expect("create storage hard link");

        let target = ActivationTarget::new(project.path(), &storage);
        let aliased = ActivationTarget::new(project.path(), &alias);

        assert!(target.matches(&aliased));
    }

    #[test]
    fn activation_target_uses_lexical_identity_for_missing_storage() {
        let project = tempfile::tempdir().expect("project");
        let storage = project.path().join("cache").join("codestory.db");
        let dotted = project.path().join("cache").join(".").join("codestory.db");

        let target = ActivationTarget::new(project.path(), &storage);
        let aliased = ActivationTarget::new(project.path(), &dotted);

        assert!(target.matches(&aliased));
    }

    #[test]
    fn pre_cancelled_activation_does_not_start_shared_work() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cache").join("codestory.db");
        let runtime = Runtime::new();
        let cancelled = Arc::new(AtomicBool::new(true));

        let error = runtime
            .activation_service()
            .activate_project(project.path(), &storage_path, cancelled)
            .expect_err("pre-cancelled activation must fail");

        assert_eq!(error.code, "cancelled");
        assert!(runtime.activation_service().snapshot().is_none());
        assert!(!storage_path.exists());
    }

    #[test]
    fn foreground_budget_returns_progress_while_one_shared_activation_continues() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cache").join("codestory.db");
        fs::write(
            project.path().join("fixture.rs"),
            "pub fn foreground_activation_fixture() {}\n",
        )
        .expect("write fixture");
        let service = Runtime::new().activation_service();

        let first = service
            .activate_project_with_foreground_budget(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
                Duration::ZERO,
            )
            .expect_err("zero foreground budget must return typed progress");
        assert_eq!(first.code, "activation_preparing");
        let first_snapshot = service.snapshot().expect("running snapshot");
        assert!(matches!(
            first_snapshot.state,
            ActivationState::Preparing | ActivationState::Updating
        ));
        assert_eq!(first_snapshot.attempt, 1);

        let second = service
            .activate_project_with_foreground_budget(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
                Duration::ZERO,
            )
            .expect_err("joining caller must observe the same running operation");
        assert_eq!(second.code, "activation_preparing");
        let joined_snapshot = service.snapshot().expect("joined snapshot");
        assert_eq!(joined_snapshot.operation_id, first_snapshot.operation_id);
        assert_eq!(joined_snapshot.attempt, 1);

        service.cancel_and_wait();
        let terminal = service.snapshot().expect("terminal snapshot");
        assert_ne!(terminal.state, ActivationState::Ready);
    }

    #[test]
    fn activation_error_is_unavailable_instead_of_ready() {
        let project = tempfile::tempdir().expect("project");
        let missing = project.path().join("missing");
        let storage_path = project.path().join("cache").join("codestory.db");
        let runtime = Runtime::new();

        let error = runtime
            .activation_service()
            .activate_project(&missing, &storage_path, Arc::new(AtomicBool::new(false)))
            .expect_err("missing project must fail");
        let snapshot = runtime.activation_service().snapshot().expect("snapshot");

        assert_eq!(error.code, "project_unavailable");
        assert_eq!(snapshot.state, ActivationState::Unavailable);
        assert_ne!(
            snapshot.capabilities.local_navigation,
            ActivationCapabilityState::Ready
        );
    }

    #[test]
    fn activation_state_is_not_reused_across_project_targets() {
        let project_a = tempfile::tempdir().expect("project a");
        let project_b = tempfile::tempdir().expect("project b");
        let service = Runtime::new().activation_service();

        service
            .activate_project_with_foreground_budget(
                project_a.path(),
                &project_a.path().join("codestory.db"),
                Arc::new(AtomicBool::new(false)),
                Duration::ZERO,
            )
            .expect_err("project a should continue outside the foreground budget");
        let first = service.snapshot().expect("first state");
        service.cancel_and_wait();

        service
            .activate_project_with_foreground_budget(
                project_b.path(),
                &project_b.path().join("codestory.db"),
                Arc::new(AtomicBool::new(false)),
                Duration::ZERO,
            )
            .expect_err("project b should continue outside the foreground budget");
        let second = service.snapshot().expect("second state");
        service.cancel_and_wait();

        assert_ne!(first.operation_id, second.operation_id);
        assert_eq!(second.attempt, 1);
        assert!(matches!(
            second.state,
            ActivationState::Preparing | ActivationState::Updating
        ));
    }

    #[test]
    fn observational_summary_does_not_create_storage_parent() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cold-cache").join("codestory.db");
        let runtime = Runtime::new();

        let summary = runtime
            .project_service()
            .inspect_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("cold observation");

        assert!(summary.is_none());
        assert!(!storage_path.parent().expect("storage parent").exists());
    }

    #[test]
    fn cancelled_public_operation_never_enters_response_builder() {
        let runtime = Runtime::new();
        let cancelled = Arc::new(AtomicBool::new(true));
        let mut entered = false;

        let error = runtime
            .public_operation_service()
            .run_observational_with_cancel("cancelled test", cancelled, || {
                entered = true;
                Ok(())
            })
            .expect_err("pre-cancelled operation must fail");

        assert_eq!(error.code, "cancelled");
        assert!(!entered);
    }

    #[test]
    fn failed_broad_activation_never_becomes_ready_but_can_preserve_local_capability() {
        let snapshot = ActivationSnapshot {
            operation_id: "activation-1".into(),
            state: ActivationState::Unavailable,
            stage: ActivationStage::Validation,
            attempt: 1,
            retry_after_ms: None,
            failure: Some("embedding backend unavailable".into()),
            capabilities: ActivationCapabilities {
                local_navigation: ActivationCapabilityState::Ready,
                broad_search: ActivationCapabilityState::Unavailable,
            },
        };

        assert!(snapshot.allows_operation("ground"));
        assert!(!snapshot.allows_operation("packet"));
        assert_ne!(snapshot.state, ActivationState::Ready);
    }
}
