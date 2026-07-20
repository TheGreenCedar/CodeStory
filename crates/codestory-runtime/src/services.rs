use codestory_contracts::api::{
    AffectedAnalysisDto, AffectedAnalysisRequest, AgentAnswerDto, AgentAskRequest,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, ApiError, ApiErrorDetails,
    BookmarkCategoryDto, BookmarkDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest,
    EmbeddingCapacityPressureDto, EmbeddingRetryStateDto, EmbeddingVectorPublicationIdentityDto,
    GroundingBudgetDto, GroundingSnapshotDto, IndexDryRunDto, IndexFreshnessStatusDto, IndexMode,
    IndexPublicationDto, IndexedFilesDto, IndexedFilesRequest, IndexingPhaseTimings,
    ListChildrenSymbolsRequest, ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest, NodeId,
    OpenDefinitionRequest, OpenProjectRequest, ProjectSummary, RetrievalStateDto, SearchHit,
    SearchRequest, SearchResultsDto, SnippetContextDto, SourceOccurrenceDto, StartIndexingRequest,
    SummaryGenerationDto, SymbolContextDto, SymbolSummaryDto, SystemActionResponse, TrailConfigDto,
    TrailContextDto,
};

use crate::AppController;
use codestory_indexer::CancellationToken;
use codestory_store::{IndexPublicationRecord, Store};
use serde::Serialize;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_ACTIVATION_FOREGROUND_BUDGET: Duration = Duration::from_secs(5);
const ACTIVATION_WAIT_SLICE: Duration = Duration::from_millis(25);

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static BEFORE_RETRIEVAL_PIN_TEST_HOOK: RefCell<Option<Box<dyn FnOnce()>>> =
        RefCell::new(None);
}

/// Install a one-shot hostile publication hook for deterministic pinning tests.
#[cfg(any(test, feature = "test-support"))]
pub fn set_before_retrieval_pin_test_hook(hook: impl FnOnce() + 'static) {
    BEFORE_RETRIEVAL_PIN_TEST_HOOK.with(|slot| slot.replace(Some(Box::new(hook))));
}

#[cfg(any(test, feature = "test-support"))]
fn run_before_retrieval_pin_test_hook() {
    BEFORE_RETRIEVAL_PIN_TEST_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(any(test, feature = "test-support")))]
fn run_before_retrieval_pin_test_hook() {}

thread_local! {
    static ACTIVE_PUBLIC_OPERATION_CANCELLATION: RefCell<Option<Arc<AtomicBool>>> =
        const { RefCell::new(None) };
}

struct ActivePublicOperationCancellationGuard {
    previous: Option<Arc<AtomicBool>>,
}

impl Drop for ActivePublicOperationCancellationGuard {
    fn drop(&mut self) {
        ACTIVE_PUBLIC_OPERATION_CANCELLATION.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

fn with_public_operation_cancellation<T>(
    cancelled: Arc<AtomicBool>,
    build: impl FnOnce() -> T,
) -> T {
    let previous =
        ACTIVE_PUBLIC_OPERATION_CANCELLATION.with(|active| active.replace(Some(cancelled)));
    let _guard = ActivePublicOperationCancellationGuard { previous };
    build()
}

pub(crate) fn active_public_operation_cancellation() -> Option<Arc<AtomicBool>> {
    ACTIVE_PUBLIC_OPERATION_CANCELLATION.with(|active| active.borrow().clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationStage {
    Discovery,
    CoreFreshness,
    SearchPreparation,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_capacity: Option<EmbeddingCapacityPressureDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_retry: Option<EmbeddingRetryStateDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    pub failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_details: Option<Box<ApiErrorDetails>>,
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
    project_id: String,
    workspace_id: String,
    storage_path: PathBuf,
}

impl ActivationTarget {
    fn new(project_root: &Path, storage_path: &Path) -> Self {
        let project = codestory_workspace::project_identity_v3(project_root);
        Self {
            project_id: project.project_id,
            workspace_id: project.workspace_id,
            storage_path: storage_path
                .canonicalize()
                .unwrap_or_else(|_| storage_path.to_path_buf()),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.project_id == other.project_id
            && self.workspace_id == other.workspace_id
            && (self.storage_path == other.storage_path
                || codestory_workspace::same_workspace_path(
                    &self.storage_path,
                    &other.storage_path,
                ))
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

enum CompleteCoreAdmission {
    Complete,
    Cold,
    Fenced,
    Corrupt(ApiError),
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

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn set_snapshot_for_test(&self, snapshot: Option<ActivationSnapshot>) {
        let mut state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        state.current = snapshot;
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

    /// Configure the controller around an existing complete core publication
    /// without repairing source freshness. This admission path is for
    /// operations that explain drift from that publication. Cold or partial
    /// state still runs normal activation; corrupt observational reads fail
    /// directly and are never reclassified as a cold cache.
    pub fn ensure_complete_core_for_observation(
        &self,
        project_root: &Path,
        storage_path: &Path,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), ApiError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "cancelled",
                "request cancelled before observational activation",
            ));
        }
        match self.classify_complete_core_admission(project_root, storage_path) {
            CompleteCoreAdmission::Complete => return Ok(()),
            CompleteCoreAdmission::Corrupt(error) => return Err(error),
            CompleteCoreAdmission::Cold | CompleteCoreAdmission::Fenced => {}
        }

        match self.activate_project(project_root, storage_path, cancelled) {
            Ok(_) => Ok(()),
            Err(error)
                if error.code != "cancelled"
                    && self.snapshot().is_some_and(|snapshot| {
                        snapshot.capabilities.local_navigation == ActivationCapabilityState::Ready
                    }) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn classify_complete_core_admission(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> CompleteCoreAdmission {
        if !storage_path.is_file() {
            return CompleteCoreAdmission::Cold;
        }
        let freshness = match Store::open_freshness_observational(storage_path) {
            Ok(storage) => storage,
            Err(error) => {
                return CompleteCoreAdmission::Corrupt(ApiError::internal(format!(
                    "Failed to inspect storage admission state: {error}"
                )));
            }
        };
        match freshness.has_incomplete_incremental_run() {
            Ok(true) => return CompleteCoreAdmission::Fenced,
            Ok(false) => {}
            Err(error) => {
                return CompleteCoreAdmission::Corrupt(ApiError::internal(format!(
                    "Failed to inspect incomplete-run admission fence: {error}"
                )));
            }
        }
        drop(freshness);

        match self.controller.inspect_project_summary_with_storage_path(
            project_root.to_path_buf(),
            storage_path.to_path_buf(),
        ) {
            Ok(Some(summary)) if summary.publication.is_some() => CompleteCoreAdmission::Complete,
            Ok(_) => CompleteCoreAdmission::Cold,
            Err(error) => CompleteCoreAdmission::Corrupt(error),
        }
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
                snapshot.failure_code = None;
                snapshot.failure_details = None;
                snapshot.embedding_capacity = None;
                snapshot.embedding_retry = None;
                snapshot.retry_after_ms = Some(250);
                snapshot.state = ActivationState::Preparing;
                snapshot.stage = ActivationStage::Discovery;
                snapshot.operation_id.clone()
            } else {
                let project_scope = target
                    .project_id
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .take(12)
                    .collect::<String>();
                let operation_id = format!(
                    "activation-{project_scope}-{}",
                    self.coordinator.next_id.fetch_add(1, Ordering::Relaxed) + 1
                );
                state.current = Some(ActivationSnapshot {
                    operation_id: operation_id.clone(),
                    state: ActivationState::Preparing,
                    stage: ActivationStage::Discovery,
                    attempt: 1,
                    retry_after_ms: Some(250),
                    embedding_capacity: None,
                    embedding_retry: None,
                    failure_code: None,
                    failure: None,
                    failure_details: None,
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
                    let attempt = worker_operation.attempt();
                    let result = worker_service
                        .activate_once(&worker_operation, worker_project_root, worker_storage_path)
                        .map_err(|error| classify_activation_api_error_for_attempt(error, attempt));
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
            .open_project_summary_with_storage_path(project_root.clone(), storage_path.clone())?;

        operation.set_stage(ActivationStage::CoreFreshness);
        let core_stale = summary.publication.is_none()
            || summary.stats.node_count == 0
            || self
                .controller
                .complete_core_requires_publication_repair(&storage_path)?
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
            summary = self.controller.open_project_summary_with_storage_path(
                project_root.clone(),
                storage_path.clone(),
            )?;
        }
        let local_ready = summary.publication.is_some()
            && summary.stats.node_count > 0
            && summary.stats.fatal_error_count == 0
            && !self
                .controller
                .complete_core_requires_publication_repair(&storage_path)?
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

        operation.ensure_not_cancelled("search preparation")?;
        operation.set_stage(ActivationStage::SearchPreparation);
        let token = CancellationToken::from_shared_flag(Arc::clone(&operation.cancelled));
        self.controller
            .prepare_search_state_for_activation(&token)?;

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
        _ => snapshot
            .failure_code
            .as_deref()
            .unwrap_or("project_unavailable"),
    };
    let mut error = activation_api_error(
        code,
        snapshot.failure.clone().unwrap_or_else(|| {
            "project activation did not provide the requested capability".into()
        }),
        snapshot.embedding_retry.clone(),
        snapshot.embedding_capacity.clone(),
    );
    if let Some(details) = snapshot.failure_details.as_ref() {
        error.details = Some(details.clone());
    }
    error
}

fn activation_preparing_error(snapshot: &ActivationSnapshot) -> ApiError {
    activation_api_error(
        "activation_preparing",
        format!(
            "project activation {} is still {:?} at {:?}; retry after {}ms",
            snapshot.operation_id,
            snapshot.state,
            snapshot.stage,
            snapshot.retry_after_ms.unwrap_or(250)
        ),
        snapshot.embedding_retry.clone(),
        snapshot.embedding_capacity.clone(),
    )
}

fn map_activation_error(error: anyhow::Error) -> ApiError {
    if let Some(error) = embedding_api_error(&error) {
        return classify_activation_api_error(error);
    }
    if codestory_retrieval::is_retrieval_index_cancelled(&error) {
        return ApiError::new("cancelled", error.to_string());
    }
    if codestory_retrieval::is_retrieval_publication_changed(&error)
        || codestory_retrieval::is_sidecar_input_changed(&error)
    {
        return classify_activation_api_error(ApiError::new(
            "publication_changed",
            error.to_string(),
        ));
    }
    classify_activation_api_error(ApiError::new("project_unavailable", error.to_string()))
}

fn classify_activation_api_error(mut error: ApiError) -> ApiError {
    match error.code.as_str() {
        "embedding_capacity"
        | "embedding_retryable"
        | "cache_busy"
        | "publication_changed"
        | "source_changed" => {
            let cause_code = error.code.clone();
            match error.details.as_mut() {
                Some(details) => {
                    details.cause_code.get_or_insert(cause_code);
                }
                None => {
                    error.details = Some(Box::new(ApiErrorDetails::cause(cause_code)));
                }
            }
            error.code = "activation_retryable".into();
            error
        }
        "cancelled" | "activation_preparing" | "activation_retryable" => error,
        "source_unreadable"
        | "source_malformed"
        | "source_binary"
        | "source_oversized"
        | "source_discovery_incomplete"
        | "source_collector_failure"
        | "source_verification_failed" => error,
        _ => {
            error.code = "project_unavailable".into();
            error
        }
    }
}

fn classify_activation_api_error_for_attempt(mut error: ApiError, attempt: u32) -> ApiError {
    if error.code == "source_changed" && attempt > 1 {
        if let Some(details) = error.details.as_mut() {
            for gap in &mut details.coverage_gaps {
                if gap.reason == codestory_contracts::graph::FileCoverageReason::SourceChanged {
                    gap.retryable = false;
                }
            }
        }
        return error;
    }
    classify_activation_api_error(error)
}

fn activation_api_error(
    code: &str,
    message: String,
    retry: Option<EmbeddingRetryStateDto>,
    pressure: Option<EmbeddingCapacityPressureDto>,
) -> ApiError {
    if let Some(retry) = retry {
        return ApiError::embedding_retry(code, message, retry);
    }
    let Some(pressure) = pressure else {
        return ApiError::new(code, message);
    };
    let mut error = ApiError::embedding_capacity(message, pressure);
    error.code = code.into();
    error
}

pub fn embedding_api_error(error: &anyhow::Error) -> Option<ApiError> {
    codestory_retrieval::embedding_retry_state(error).map(embedding_retry_api_error)
}

fn embedding_retry_api_error(retry: codestory_retrieval::EmbeddingRetryStateWire) -> ApiError {
    let capacity = retry.capacity.map(embedding_capacity_dto);
    let cause_code = retry.code.clone();
    let public_code = if retry.code.contains("cancelled") {
        "cancelled"
    } else if capacity.is_some() {
        "embedding_capacity"
    } else if retry.code == "native_model_not_embedded" {
        "project_unavailable"
    } else if matches!(
        retry.retry_class.as_str(),
        "after_capacity_change"
            | "after_delay"
            | "after_owner_idle"
            | "after_server_change"
            | "same_rpc_once"
    ) {
        "embedding_retryable"
    } else {
        "project_unavailable"
    };
    let mut error = ApiError::embedding_retry(
        public_code,
        retry.message,
        EmbeddingRetryStateDto {
            code: retry.code,
            retry_class: retry.retry_class,
            retry_after_ms: retry.retry_after_ms,
            retry_condition: retry.retry_condition,
            capacity,
        },
    );
    if public_code == "project_unavailable"
        && let Some(details) = error.details.as_mut()
    {
        details.cause_code = Some(cause_code);
    }
    error
}

fn embedding_capacity_dto(
    pressure: codestory_retrieval::EmbeddingCapacityPressureWire,
) -> EmbeddingCapacityPressureDto {
    EmbeddingCapacityPressureDto {
        reason: pressure.reason,
        queue_class: pressure.queue_class,
        capacity: pressure.capacity,
        depth: pressure.depth,
        retry_after_ms: pressure.retry_after_ms,
        retry_condition: pressure.retry_condition,
        owner_state: pressure.owner_state,
        active_scope_id: pressure.active_scope_id,
        active_request_id: pressure.active_request_id,
        active_request_class: pressure.active_request_class,
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

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn retrieval_primary_enabled_for_test(&self) -> bool {
        crate::agent::retrieval_primary::sidecar_retrieval_primary_enabled(&self.controller)
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

    /// Read the project summary from the core snapshot pinned by the current
    /// public operation. This deliberately rejects calls outside a pin so a
    /// response cannot mix a pre-operation summary with pinned graph reads.
    pub fn active_project_summary(&self) -> Result<ProjectSummary, ApiError> {
        self.controller.active_project_summary()
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
                    let value =
                        with_public_operation_cancellation(Arc::clone(&cancelled), &mut build)?;
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
                    run_before_retrieval_pin_test_hook();
                    crate::agent::retrieval_primary::with_pinned_retrieval_publication_value(
                        &self.controller,
                        &publication.generation_id,
                        &publication.run_id,
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
                    let value =
                        with_public_operation_cancellation(Arc::clone(&cancelled), &mut build)?;
                    if cancelled.load(Ordering::Acquire) {
                        return Err(ApiError::new(
                            "cancelled",
                            format!("request cancelled during {operation}"),
                        ));
                    }
                    Ok(value)
                };
                let (value, retrieval_publication) = if operation_requires_retrieval(operation) {
                    run_before_retrieval_pin_test_hook();
                    crate::agent::retrieval_primary::with_pinned_retrieval_publication_value(
                        &self.controller,
                        &publication.generation_id,
                        &publication.run_id,
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
    fn attempt(&self) -> u32 {
        self.service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned")
            .current
            .as_ref()
            .filter(|snapshot| snapshot.operation_id == self.operation_id)
            .map_or(1, |snapshot| snapshot.attempt)
    }

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
                "activation_retryable"
                | "embedding_capacity"
                | "cache_busy"
                | "publication_changed" => ActivationCapabilityState::Retryable,
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
            snapshot.embedding_capacity = error
                .details
                .as_deref()
                .and_then(|details| details.embedding_capacity.clone());
            snapshot.embedding_retry = error
                .details
                .as_deref()
                .and_then(|details| details.embedding_retry.clone());
            snapshot.failure_code = Some(error.code.clone());
            snapshot.failure_details = error.details.clone();
            snapshot.retry_after_ms =
                (capability == ActivationCapabilityState::Retryable).then(|| {
                    snapshot.embedding_retry.as_ref().map_or_else(
                        || {
                            snapshot
                                .embedding_capacity
                                .as_ref()
                                .map_or(250, |pressure| pressure.retry_after_ms)
                        },
                        |retry| retry.retry_after_ms,
                    )
                });
            snapshot.failure = Some(error.message.clone());
        } else {
            snapshot.state = ActivationState::Ready;
            snapshot.stage = ActivationStage::Ready;
            snapshot.retry_after_ms = None;
            snapshot.embedding_capacity = None;
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
    use std::process::Command;

    fn git(project: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .status()
            .expect("run git fixture command");
        assert!(status.success(), "git fixture command failed: {args:?}");
    }

    fn initialize_identifiable_git_project(project: &Path) {
        git(project, &["init", "-q"]);
        git(
            project,
            &["config", "user.email", "codestory-tests@example.com"],
        );
        git(project, &["config", "user.name", "CodeStory Tests"]);
        fs::write(project.join("fixture.rs"), "pub fn clean_fixture() {}\n")
            .expect("write clean fixture");
        git(project, &["add", "fixture.rs"]);
        git(project, &["commit", "-qm", "fixture"]);
        git(
            project,
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/codestory/fixture.git",
            ],
        );
    }

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
    fn activation_target_exact_storage_path_does_not_reobserve_filesystem_identity() {
        let project = tempfile::tempdir().expect("project");
        let storage = project.path().join("codestory\0.db");

        let target = ActivationTarget::new(project.path(), &storage);
        let same_target = target.clone();

        assert!(target.matches(&same_target));
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
    fn activation_target_ignores_mutable_artifact_eligibility() {
        let project = tempfile::tempdir().expect("project");
        initialize_identifiable_git_project(project.path());
        let storage = project.path().join("cache").join("codestory.db");
        let clean_identity = codestory_workspace::project_identity_v3(project.path());
        let clean = ActivationTarget::new(project.path(), &storage);

        fs::write(
            project.path().join("fixture.rs"),
            "pub fn dirty_fixture() {}\n",
        )
        .expect("make fixture dirty");
        let dirty_identity = codestory_workspace::project_identity_v3(project.path());
        let dirty = ActivationTarget::new(project.path(), &storage);

        assert_ne!(
            clean_identity.artifact_scope_id,
            dirty_identity.artifact_scope_id
        );
        assert_ne!(
            clean_identity.portable_reuse_eligible,
            dirty_identity.portable_reuse_eligible
        );
        assert_eq!(clean_identity.project_id, dirty_identity.project_id);
        assert_eq!(clean_identity.workspace_id, dirty_identity.workspace_id);
        assert!(clean.matches(&dirty));
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
    fn activation_repairs_complete_core_without_a_search_generation() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let storage_path = cache.path().join("codestory.db");
        fs::write(
            project.path().join("fixture.rs"),
            "// migrated core fixture\n",
        )
        .expect("write fixture");

        let seeding_runtime = Runtime::new();
        seeding_runtime
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        seeding_runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish complete core");
        let publication = Store::database_index_publication(&storage_path)
            .expect("read core publication")
            .expect("complete core publication");
        let search_path =
            crate::search_index_path_for_publication(&storage_path, Some(&publication))
                .expect("search generation path");
        fs::remove_dir_all(&search_path).expect("remove completed search generation");

        let runtime = Runtime::new();
        let error = runtime
            .activation_service()
            .activate_project(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect_err("the unit-test runtime has no managed embedding server");

        assert_eq!(error.code, "project_unavailable");
        let snapshot = runtime.activation_service().snapshot().expect("snapshot");
        assert_eq!(
            snapshot.capabilities.local_navigation,
            ActivationCapabilityState::Ready
        );
        assert!(
            crate::read_search_generation_completion(&search_path, &publication.generation_id,)
                .is_some(),
            "activation must publish a completion marker for the repaired generation"
        );
        runtime
            .project_service()
            .open_project_with_storage_path(project.path().to_path_buf(), storage_path)
            .expect("the strict reader must admit the repaired generation");
    }

    #[test]
    fn activation_republishes_a_migrated_core_missing_dense_and_search_state() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let storage_path = cache.path().join("codestory.db");
        fs::write(
            project.path().join("fixture.rs"),
            "// migrated core fixture\n",
        )
        .expect("write fixture");

        let seeding_runtime = Runtime::new();
        seeding_runtime
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        seeding_runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish complete core");
        let previous = Store::database_index_publication(&storage_path)
            .expect("read core publication")
            .expect("complete core publication");
        let previous_search =
            crate::search_index_path_for_publication(&storage_path, Some(&previous))
                .expect("search generation path");
        fs::remove_dir_all(previous_search).expect("remove completed search generation");
        Store::open(&storage_path)
            .expect("open migrated core")
            .get_connection()
            .execute("DELETE FROM dense_anchor_publication", [])
            .expect("remove dense-anchor publication marker");

        let runtime = Runtime::new();
        runtime
            .activation_service()
            .activate_project(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect_err("the unit-test runtime has no managed embedding server");

        let current = Store::database_index_publication(&storage_path)
            .expect("read repaired publication")
            .expect("repaired complete publication");
        assert_eq!(current.generation, previous.generation + 1);
        assert_eq!(
            current.mode,
            codestory_store::IndexPublicationMode::Incremental
        );
        let current_search =
            crate::search_index_path_for_publication(&storage_path, Some(&current))
                .expect("repaired search generation path");
        assert!(
            crate::read_search_generation_completion(&current_search, &current.generation_id)
                .is_some(),
            "incremental migration must publish the exact new search generation"
        );
        let storage = Store::open_read_only(&storage_path).expect("open repaired core");
        storage
            .validate_dense_anchor_publication(&current)
            .expect("incremental migration must republish dense anchors");
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
    fn concurrent_cold_projects_keep_independent_activation_operations() {
        let projects = (0..3)
            .map(|index| {
                let project = tempfile::tempdir().expect("project");
                fs::write(
                    project.path().join(format!("fixture-{index}.rs")),
                    format!("pub fn project_{index}() {{}}\n"),
                )
                .expect("write project fixture");
                project
            })
            .collect::<Vec<_>>();
        let caches = (0..3)
            .map(|_| tempfile::tempdir().expect("cache"))
            .collect::<Vec<_>>();
        let runtimes = (0..3).map(|_| Runtime::new()).collect::<Vec<_>>();
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let workers = (0..3)
            .map(|index| {
                let runtime = runtimes[index].clone();
                let project_root = projects[index].path().to_path_buf();
                let storage_path = caches[index].path().join("codestory.db");
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let _ = runtime
                        .activation_service()
                        .activate_project_with_foreground_budget(
                            &project_root,
                            &storage_path,
                            Arc::new(AtomicBool::new(false)),
                            Duration::ZERO,
                        );
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().expect("join cold activation");
        }

        let operation_ids = runtimes
            .iter()
            .map(|runtime| {
                runtime
                    .activation_service()
                    .snapshot()
                    .expect("activation snapshot")
                    .operation_id
            })
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(operation_ids.len(), 3);

        runtimes[0].activation_service().cancel_and_wait();
        for runtime in runtimes.iter().skip(1) {
            assert_ne!(
                runtime
                    .activation_service()
                    .snapshot()
                    .expect("independent activation snapshot")
                    .state,
                ActivationState::Cancelled,
                "cancelling one project must not cancel another project"
            );
            runtime.activation_service().cancel_and_wait();
        }
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
    fn observational_admission_preserves_an_existing_stale_complete_publication() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cache").join("codestory.db");
        let source = project.path().join("fixture.rs");
        fs::write(&source, "pub fn fixture() -> u32 { 1 }\n").expect("write fixture");
        let runtime = Runtime::new();
        runtime
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project");
        runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish complete core");
        let before = runtime
            .project_service()
            .complete_index_publication_at(&storage_path)
            .expect("read publication")
            .expect("complete publication");

        fs::write(&source, "pub fn fixture() -> u32 { 2 }\n").expect("make source stale");
        runtime
            .activation_service()
            .ensure_complete_core_for_observation(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect("admit stale complete publication");

        let summary = runtime
            .project_service()
            .inspect_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("inspect stale publication")
            .expect("existing project summary");
        assert_eq!(summary.publication.as_ref(), Some(&before));
        assert_eq!(
            summary.freshness.as_ref().map(|freshness| freshness.status),
            Some(IndexFreshnessStatusDto::Stale)
        );
        assert!(
            runtime.activation_service().snapshot().is_none(),
            "existing complete state must not start managed activation"
        );
    }

    #[test]
    fn observational_admission_routes_a_durable_incomplete_fence_to_recovery() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cache").join("codestory.db");
        fs::write(project.path().join("fixture.rs"), "pub fn fixture() {}\n")
            .expect("write fixture");
        let runtime = Runtime::new();
        runtime
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project");
        runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish complete core");
        {
            let storage = Store::open(&storage_path).expect("open published storage");
            storage
                .begin_incremental_run()
                .expect("install durable incomplete fence");
        }

        runtime
            .activation_service()
            .ensure_complete_core_for_observation(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect("the fenced sentinel must enter and complete managed core recovery");
        let snapshot = runtime
            .activation_service()
            .snapshot()
            .expect("fenced admission must attempt managed recovery");
        assert_eq!(
            snapshot.capabilities.local_navigation,
            ActivationCapabilityState::Ready
        );
        assert!(
            !Store::database_has_incomplete_incremental_run(&storage_path)
                .expect("inspect recovered storage"),
            "managed core recovery must clear the durable incomplete fence"
        );
        runtime.activation_service().cancel_and_wait();
    }

    #[test]
    fn observational_admission_propagates_corrupt_storage_instead_of_treating_it_as_cold() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cache").join("codestory.db");
        fs::create_dir_all(storage_path.parent().expect("cache parent")).expect("create cache");
        fs::write(&storage_path, b"not a sqlite database").expect("write corrupt storage");
        let runtime = Runtime::new();

        let error = runtime
            .activation_service()
            .ensure_complete_core_for_observation(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect_err("corrupt storage must fail observational admission");

        assert_eq!(error.code, "internal");
        assert!(runtime.activation_service().snapshot().is_none());
        assert_eq!(
            fs::read(&storage_path).expect("read corrupt storage"),
            b"not a sqlite database"
        );
    }

    #[test]
    fn pre_cancelled_observational_admission_does_not_create_cold_storage() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cache").join("codestory.db");
        let runtime = Runtime::new();

        let error = runtime
            .activation_service()
            .ensure_complete_core_for_observation(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(true)),
            )
            .expect_err("pre-cancelled admission must fail");

        assert_eq!(error.code, "cancelled");
        assert!(!storage_path.exists());
        assert!(runtime.activation_service().snapshot().is_none());
    }

    #[test]
    fn embedding_capacity_stays_typed_and_never_suggests_repair() {
        let source = anyhow::Error::new(codestory_retrieval::PerUserEmbeddingError {
            code: "embedding_capacity".into(),
            message: "embedding query capacity is unavailable".into(),
            retry_class: "after_capacity_change".into(),
            retry_after_ms: 25,
            retry_condition: "a query slot becomes available".into(),
            capacity: Some(codestory_retrieval::EmbeddingCapacityPressureWire {
                reason: "queue_full".into(),
                queue_class: "query".into(),
                capacity: 64,
                depth: 64,
                retry_after_ms: 25,
                retry_condition: "a query slot becomes available".into(),
                owner_state: "ready".into(),
                active_scope_id: Some("opaque-scope".into()),
                active_request_id: Some("opaque-request".into()),
                active_request_class: Some("bulk".into()),
            }),
        });
        let error = embedding_api_error(&source).expect("typed capacity error");
        let classified = classify_activation_api_error(error);
        let details = classified.details.as_deref().expect("capacity details");

        assert_eq!(classified.code, "activation_retryable");
        assert!(details.project.is_none());
        assert!(details.next_commands.is_empty());
        assert!(details.minimum_next.is_empty());
        assert!(details.full_repair.is_empty());
        assert_eq!(
            details
                .embedding_capacity
                .as_ref()
                .map(|pressure| pressure.retry_condition.as_str()),
            Some("a query slot becomes available")
        );
    }

    #[test]
    fn activation_classification_uses_codes_instead_of_message_text() {
        let diagnostic = codestory_contracts::api::FileCoverageDiagnosticDto {
            path: "src/lib.rs".to_string(),
            reason: codestory_contracts::graph::FileCoverageReason::SourceChanged,
            retryable: true,
            verified_source: false,
            projection_available: false,
        };
        let retryable = classify_activation_api_error(ApiError::with_details(
            "cache_busy",
            "another writer owns the project cache",
            codestory_contracts::api::ApiErrorDetails::source_coverage(vec![diagnostic.clone()]),
        ));
        assert_eq!(retryable.code, "activation_retryable");
        assert_eq!(
            retryable
                .details
                .as_ref()
                .and_then(|details| details.cause_code.as_deref()),
            Some("cache_busy")
        );
        assert_eq!(
            retryable
                .details
                .as_ref()
                .map(|details| details.coverage_gaps.as_slice()),
            Some([diagnostic].as_slice())
        );

        let drift = classify_activation_api_error(ApiError::new(
            "publication_changed",
            "the core identity changed during promotion",
        ));
        assert_eq!(drift.code, "activation_retryable");
        assert_eq!(
            drift
                .details
                .as_ref()
                .and_then(|details| details.cause_code.as_deref()),
            Some("publication_changed")
        );

        let persistent_source_drift = classify_activation_api_error_for_attempt(
            ApiError::source_coverage_failure(
                "source_changed",
                "source changed again while indexing",
                vec![codestory_contracts::api::FileCoverageDiagnosticDto {
                    path: "src/lib.rs".to_string(),
                    reason: codestory_contracts::graph::FileCoverageReason::SourceChanged,
                    retryable: true,
                    verified_source: false,
                    projection_available: false,
                }],
            ),
            2,
        );
        assert_eq!(persistent_source_drift.code, "source_changed");
        assert!(
            persistent_source_drift
                .details
                .as_ref()
                .is_some_and(|details| {
                    details
                        .coverage_gaps
                        .iter()
                        .all(|diagnostic| !diagnostic.retryable)
                })
        );

        let terminal = classify_activation_api_error(ApiError::new(
            "internal",
            "cache_busy publication changed cancellation",
        ));
        assert_eq!(terminal.code, "project_unavailable");
    }

    #[test]
    fn retrieval_cancellation_remains_typed_through_activation_mapping() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let storage_path = cache.path().join("codestory.db");
        let cancelled = AtomicBool::new(true);
        let error = codestory_retrieval::finalize_index_for_runtime_with_cancel(
            project.path(),
            &storage_path,
            &codestory_retrieval::SidecarRuntimeConfig::local(),
            &cancelled,
        )
        .expect_err("pre-cancelled retrieval preparation");

        let mapped = map_activation_error(error);

        assert_eq!(mapped.code, "cancelled");
    }

    #[test]
    fn terminal_source_failure_survives_activation_snapshot_round_trip() {
        let diagnostic = codestory_contracts::api::FileCoverageDiagnosticDto {
            path: "src/large.ts".to_string(),
            reason: codestory_contracts::graph::FileCoverageReason::Oversized,
            retryable: false,
            verified_source: false,
            projection_available: false,
        };
        let snapshot = ActivationSnapshot {
            operation_id: "activation-source-failure".to_string(),
            state: ActivationState::Unavailable,
            stage: ActivationStage::CoreFreshness,
            attempt: 1,
            retry_after_ms: None,
            embedding_capacity: None,
            embedding_retry: None,
            failure_code: Some("source_oversized".to_string()),
            failure: Some("source exceeds the indexing limit".to_string()),
            failure_details: Some(Box::new(ApiErrorDetails::source_coverage(vec![
                diagnostic.clone(),
            ]))),
            capabilities: ActivationCapabilities {
                local_navigation: ActivationCapabilityState::Unavailable,
                broad_search: ActivationCapabilityState::Unavailable,
            },
        };

        let error = snapshot_error(&snapshot);

        assert_eq!(error.code, "source_oversized");
        assert_eq!(
            error
                .details
                .as_ref()
                .map(|details| details.coverage_gaps.as_slice()),
            Some([diagnostic].as_slice())
        );
    }

    #[test]
    fn owner_idle_retry_metadata_survives_central_runtime_mapping() {
        let source = anyhow::Error::new(codestory_retrieval::PerUserEmbeddingError {
            code: "embedding_server_incompatible_active_owner".into(),
            message: "the live owner is incompatible".into(),
            retry_class: "after_owner_idle".into(),
            retry_after_ms: 0,
            retry_condition: "the incompatible server exits while fully idle".into(),
            capacity: None,
        });

        let mapped = embedding_api_error(&source).expect("typed embedding error");
        let retry = mapped
            .details
            .as_deref()
            .and_then(|details| details.embedding_retry.as_ref())
            .expect("retry details");
        assert_eq!(mapped.code, "embedding_retryable");
        assert_eq!(retry.code, "embedding_server_incompatible_active_owner");
        assert_eq!(retry.retry_class, "after_owner_idle");
        assert_eq!(
            retry.retry_condition,
            "the incompatible server exits while fully idle"
        );
        assert!(retry.capacity.is_none());
    }

    #[test]
    fn activation_classification_preserves_embedding_retry_details() {
        let source = anyhow::Error::new(codestory_retrieval::PerUserEmbeddingError {
            code: "embedding_server_owner_unresponsive".into(),
            message: "the owner did not respond".into(),
            retry_class: "after_server_change".into(),
            retry_after_ms: 25,
            retry_condition: "the lifetime authority changes".into(),
            capacity: None,
        });

        let mapped = map_activation_error(source);
        assert_eq!(mapped.code, "activation_retryable");
        let retry = mapped
            .details
            .as_deref()
            .and_then(|details| details.embedding_retry.as_ref())
            .expect("retry details");
        assert_eq!(retry.code, "embedding_server_owner_unresponsive");
        assert_eq!(retry.retry_class, "after_server_change");
        assert_eq!(retry.retry_after_ms, 25);
        assert_eq!(retry.retry_condition, "the lifetime authority changes");
        assert!(retry.capacity.is_none());
    }

    #[test]
    fn terminal_embedding_error_remains_unavailable_with_typed_diagnostics() {
        let source = anyhow::Error::new(codestory_retrieval::PerUserEmbeddingError {
            code: "embedding_server_protocol_mismatch".into(),
            message: "the protocol changed".into(),
            retry_class: "terminal".into(),
            retry_after_ms: 0,
            retry_condition: "the request or compatible executable changes".into(),
            capacity: None,
        });

        let mapped = embedding_api_error(&source).expect("typed embedding error");
        assert_eq!(mapped.code, "project_unavailable");
        assert_eq!(
            mapped
                .details
                .as_deref()
                .and_then(|details| details.embedding_retry.as_ref())
                .map(|retry| retry.retry_class.as_str()),
            Some("terminal")
        );
    }

    #[test]
    fn executable_without_an_embedded_model_is_terminal_for_activation() {
        let source = anyhow::Error::new(codestory_retrieval::PerUserEmbeddingError {
            code: "native_model_not_embedded".into(),
            message: "the executable has no embedded model".into(),
            retry_class: "after_server_change".into(),
            retry_after_ms: 0,
            retry_condition: "the server instance changes".into(),
            capacity: None,
        });

        let mapped = map_activation_error(source);

        assert_eq!(mapped.code, "project_unavailable");
        assert_eq!(
            mapped
                .details
                .as_deref()
                .and_then(|details| details.cause_code.as_deref()),
            Some("native_model_not_embedded")
        );
    }

    #[test]
    fn failed_broad_activation_never_becomes_ready_but_can_preserve_local_capability() {
        let snapshot = ActivationSnapshot {
            operation_id: "activation-1".into(),
            state: ActivationState::Unavailable,
            stage: ActivationStage::Validation,
            attempt: 1,
            retry_after_ms: None,
            embedding_capacity: None,
            embedding_retry: None,
            failure_code: Some("project_unavailable".into()),
            failure: Some("embedding backend unavailable".into()),
            failure_details: None,
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
