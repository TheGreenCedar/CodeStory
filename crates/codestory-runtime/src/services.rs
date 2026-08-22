use codestory_contracts::api::{
    AffectedAnalysisDto, AffectedAnalysisRequest, AgentAnswerDto, AgentAskRequest,
    AgentHybridWeightsDto, AgentPacketDto, AgentPacketRequestDto, ApiError, ApiErrorDetails,
    BookmarkCategoryDto, BookmarkDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest,
    EmbeddingCapacityPressureDto, EmbeddingRetryStateDto, EmbeddingVectorPublicationIdentityDto,
    GroundingBudgetDto, GroundingSnapshotDto, IndexDryRunDto, IndexFreshnessDto,
    IndexFreshnessNotCheckedCauseDto, IndexFreshnessStatusDto, IndexMode, IndexPublicationDto,
    IndexedFilesDto, IndexedFilesRequest, IndexingPhaseTimings, ListChildrenSymbolsRequest,
    ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest, NodeId, OpenProjectRequest,
    ProjectSummary, RetrievalStateDto, SearchHit, SearchRequest, SearchResultsDto,
    SnippetContextDto, SourceOccurrenceDto, StartIndexingRequest, SummaryGenerationDto,
    SymbolContextDto, SymbolSummaryDto, TrailConfigDto, TrailContextDto, UpdateBookmarkRequest,
};

use crate::AppController;
use crate::ObservedSourceEpoch;
use crate::index_freshness::FreshnessObservationPolicy;
use codestory_indexer::CancellationToken;
use codestory_store::{IndexPublicationRecord, Store};
use serde::Serialize;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

const DEFAULT_ACTIVATION_FOREGROUND_BUDGET: Duration = Duration::from_secs(5);
const ACTIVATION_WAIT_SLICE: Duration = Duration::from_millis(25);
/// Longest an eviction or shutdown waits for a cancelled activation worker to
/// reach a quiescent boundary before fail-stopping the process.
///
/// The worker checks cancellation between every preparation stage and inside
/// indexing, and [`run_activation_worker`] installs its cancellation flag as
/// the ambient one for every bounded lock acquisition on the worker thread. A
/// lock wait therefore never blocks the worker for its own budget — a peer's
/// whole publication, up to [`bounded_locks::PUBLICATION_LOCK_WAIT`] — but only
/// until the flag is observed, within
/// [`bounded_locks::MAX_CANCELLATION_LATENCY`]. Exceeding this budget means the
/// worker is wedged inside owned mutation, not merely waiting for a peer.
const ACTIVATION_QUIESCENCE_BUDGET: Duration = Duration::from_secs(20);

/// The invariant this whole path rests on: the budget an eviction uses before
/// aborting the process must exceed the longest a worker can stay inside a
/// bounded lock wait after its cancellation is raised. Without the ambient
/// scope that longest wait would instead be `PUBLICATION_LOCK_WAIT`, and an
/// ordinary slow activation would abort a healthy session.
const _: () = assert!(
    ACTIVATION_QUIESCENCE_BUDGET.as_millis()
        > codestory_contracts::bounded_locks::MAX_CANCELLATION_LATENCY.as_millis(),
    "the quiescence budget must exceed the bounded-lock cancellation latency"
);
/// Fail-stop reason recorded when a cancelled activation worker never reaches
/// a quiescent boundary.
pub const ACTIVATION_QUIESCENCE_FAIL_STOP: &str = "activation_quiescence_timeout";

/// Process-level fail-stop installed by the host binary. The runtime never
/// aborts on its own: an embedder or a test observes the verdict instead.
pub type ActivationFailStopHook = Arc<dyn Fn(&str) + Send + Sync>;

static ACTIVATION_FAIL_STOP: RwLock<Option<ActivationFailStopHook>> = RwLock::new(None);

/// Install (or clear) the process fail-stop used when a cancelled activation
/// worker cannot be proven quiescent. Returns the previous hook.
pub fn set_activation_fail_stop_hook(
    hook: Option<ActivationFailStopHook>,
) -> Option<ActivationFailStopHook> {
    let mut installed = ACTIVATION_FAIL_STOP
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::replace(&mut *installed, hook)
}

fn run_activation_fail_stop(reason_code: &str) {
    let hook = ACTIVATION_FAIL_STOP
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match hook {
        Some(hook) => hook(reason_code),
        None => tracing::error!(
            reason_code,
            "a cancelled activation worker never reached a quiescent boundary and no process fail-stop is installed"
        ),
    }
}

/// Whether a cancelled activation worker reached a boundary at which it
/// provably holds no publication or store lock and mutates no owned state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationQuiescence {
    Quiesced,
    FailStopRequired,
}

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
    let previous = ACTIVE_PUBLIC_OPERATION_CANCELLATION
        .with(|active| active.replace(Some(Arc::clone(&cancelled))));
    let _guard = ActivePublicOperationCancellationGuard { previous };
    // A request body reaches the same publication-class locks the activation
    // worker does, and waits behind a peer's whole publication pass. That is
    // only tolerable while the request's own cancellation can end the wait, so
    // the flag becomes the ambient one for every bounded acquisition below.
    codestory_contracts::bounded_locks::with_thread_cancellation(cancelled, build)
}

pub(crate) fn active_public_operation_cancellation() -> Option<Arc<AtomicBool>> {
    ACTIVE_PUBLIC_OPERATION_CANCELLATION.with(|active| active.borrow().clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationStage {
    Discovery,
    CoreFreshness,
    SearchPreparation,
    DensePreparation,
    Publication,
    Validation,
    Ready,
}

fn activation_stage_progress(stage: ActivationStage) -> u8 {
    match stage {
        ActivationStage::Discovery => 0,
        ActivationStage::CoreFreshness => 20,
        ActivationStage::SearchPreparation => 40,
        ActivationStage::DensePreparation => 60,
        ActivationStage::Publication => 75,
        ActivationStage::Validation => 90,
        ActivationStage::Ready => 100,
    }
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
    Retained,
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
    pub revision: u64,
    pub state: ActivationState,
    pub stage: ActivationStage,
    pub progress: u8,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained_core_publication: Option<IndexPublicationDto>,
    pub capabilities: ActivationCapabilities,
}

impl ActivationSnapshot {
    pub fn allows_operation(&self, operation: &str) -> bool {
        if operation_requires_retrieval(operation) {
            self.capabilities.broad_search == ActivationCapabilityState::Ready
        } else {
            matches!(
                self.capabilities.local_navigation,
                ActivationCapabilityState::Ready | ActivationCapabilityState::Retained
            )
        }
    }
}

/// Test-only handshake a spawned activation worker waits on before starting.
#[cfg(any(test, feature = "test-support"))]
type WorkerStartGate = Arc<(Mutex<bool>, Condvar)>;

#[derive(Debug, Clone)]
pub struct ActivationRun {
    pub snapshot: ActivationSnapshot,
    pub joined: bool,
}

#[derive(Default)]
struct ActivationCoordinatorState {
    target: Option<ActivationTarget>,
    current: Option<ActivationSnapshot>,
    ready_lease: Option<ReadyLease>,
    running: bool,
    current_cancel: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadySourceIdentity {
    status: IndexFreshnessStatusDto,
    not_checked_cause: Option<IndexFreshnessNotCheckedCauseDto>,
    changed_file_count: u32,
    new_file_count: u32,
    removed_file_count: u32,
    checked_file_count: u32,
    indexed_file_count: u32,
    gap: Option<String>,
}

impl From<&IndexFreshnessDto> for ReadySourceIdentity {
    fn from(freshness: &IndexFreshnessDto) -> Self {
        Self {
            status: freshness.status,
            not_checked_cause: freshness.not_checked_cause,
            changed_file_count: freshness.changed_file_count,
            new_file_count: freshness.new_file_count,
            removed_file_count: freshness.removed_file_count,
            checked_file_count: freshness.checked_file_count,
            indexed_file_count: freshness.indexed_file_count,
            gap: freshness.reason.clone(),
        }
    }
}

impl ReadySourceIdentity {
    fn is_admissible_snapshot(&self) -> bool {
        let no_observed_drift = self.changed_file_count == 0
            && self.new_file_count == 0
            && self.removed_file_count == 0;
        match self.status {
            IndexFreshnessStatusDto::Fresh => {
                no_observed_drift
                    && self.checked_file_count == self.indexed_file_count
                    && self.not_checked_cause.is_none()
                    && self.gap.is_none()
            }
            IndexFreshnessStatusDto::NotChecked => {
                no_observed_drift
                    && self.checked_file_count <= self.indexed_file_count
                    && self.not_checked_cause
                        == Some(IndexFreshnessNotCheckedCauseDto::BoundedInventory)
                    && self.gap.is_some()
            }
            IndexFreshnessStatusDto::Stale => false,
        }
    }

    fn admission_basis(&self) -> &'static str {
        match (self.status, self.not_checked_cause) {
            (IndexFreshnessStatusDto::Fresh, None) => "complete_source_observation",
            (
                IndexFreshnessStatusDto::NotChecked,
                Some(IndexFreshnessNotCheckedCauseDto::BoundedInventory),
            ) => "bounded_source_inventory",
            _ => "inadmissible_source_observation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadyLease {
    configuration_id: String,
    core_publication: IndexPublicationDto,
    retrieval: codestory_retrieval::ReadyRetrievalIdentity,
    source: ReadySourceIdentity,
    source_freshness_memo: codestory_workspace::SourceFreshnessMemo,
    /// The observer session and epoch the source snapshot was taken under.
    ///
    /// Everything else in this lease is a pointer that a source write does not move: the core
    /// publication is unchanged because the database is unchanged, the retrieval manifest is
    /// unchanged for the same reason, and `source` is a DTO frozen at one instant. That is the
    /// window EV-78 left open and the reason this package exists — so the lease has to carry the
    /// one value that *does* move when the working tree does.
    ///
    /// `None` on a host the observer cannot watch. The lease then means exactly what it meant
    /// before this package: EV-7's bounded scan, revalidated by the serving reads.
    source_observer: Option<ObservedSourceEpoch>,
}

#[derive(Debug, Clone)]
struct ActivationTarget {
    project_id: String,
    workspace_id: String,
    repository_instance: codestory_workspace::RepositoryInstanceIdentity,
    storage_path: PathBuf,
}

impl ActivationTarget {
    fn new(project_root: &Path, storage_path: &Path) -> Self {
        let project = codestory_workspace::observe_logical_project_identity_v3(project_root);
        Self {
            project_id: project.project_id,
            workspace_id: project.workspace_id,
            repository_instance: project.repository_instance,
            storage_path: storage_path
                .canonicalize()
                .unwrap_or_else(|_| storage_path.to_path_buf()),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.project_id == other.project_id
            && self.workspace_id == other.workspace_id
            && self.repository_instance == other.repository_instance
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
    #[cfg(any(test, feature = "test-support"))]
    worker_start_count: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    worker_start_gate: Mutex<Option<WorkerStartGate>>,
    #[cfg(any(test, feature = "test-support"))]
    native_preparation_count: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    retrieval_finalization_count: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    preparation_seams_armed: AtomicBool,
    #[cfg(any(test, feature = "test-support"))]
    native_preparation_limit: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    retrieval_finalization_limit: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    use_published_retrieval_fixture: AtomicBool,
}

#[derive(Clone, Copy)]
enum ActivationPreparationPhase {
    NativeEmbedding,
    RetrievalFinalization,
}

/// Runtime-owned single-flight activation for one logical project, core store,
/// and immutable runtime configuration. The configuration is fixed by the
/// controller owned by this service.
#[derive(Clone)]
pub struct ActivationService {
    coordinator: Arc<ActivationCoordinator>,
    /// Visible to `crate::activation_status`, which hangs the transport-status
    /// answers off this same service.
    pub(crate) controller: AppController,
}

enum CompleteCoreAdmission {
    Complete,
    Cold,
    Fenced,
    Corrupt(ApiError),
}

struct ReadyLeaseProbe {
    admissible: bool,
    retained_core_publication: Option<IndexPublicationDto>,
}

fn ready_retrieval_identity_matches(
    observed: Option<&codestory_retrieval::ReadyRetrievalIdentity>,
    retained: &codestory_retrieval::ReadyRetrievalIdentity,
) -> bool {
    observed == Some(retained)
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

    /// Report the evidence retained by the matching ready lease without
    /// probing, renewing, or otherwise touching that lease.
    pub(crate) fn ready_lease_evidence(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> crate::activation_status::ReadyLeaseEvidence {
        let requested = ActivationTarget::new(project_root, storage_path);
        let state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        let bound_storage_matches = self
            .controller
            .require_storage_path()
            .is_ok_and(|bound| bound == storage_path);
        let Some(lease) = state
            .target
            .as_ref()
            .filter(|target| {
                target.matches(&requested)
                    || (target.project_id == requested.project_id
                        && target.workspace_id == requested.workspace_id
                        && target.repository_instance == requested.repository_instance
                        && bound_storage_matches)
            })
            .and_then(|_| state.ready_lease.as_ref())
        else {
            return crate::activation_status::ReadyLeaseEvidence::absent();
        };
        let observer_epoch_coherence = match lease.source_observer.as_ref() {
            None => "unproven",
            Some(recorded) => {
                if self
                    .controller
                    .observed_source_epoch_if_armed(project_root)
                    .as_ref()
                    == Some(recorded)
                {
                    "coherent"
                } else {
                    "stale"
                }
            }
        };
        crate::activation_status::ReadyLeaseEvidence {
            ready_lease_present: true,
            ready_lease_admission_basis: lease.source.admission_basis().to_string(),
            ready_lease_observer_epoch_coherence: observer_epoch_coherence.to_string(),
            // The memo is lease-owned. Reporting its attachment is deliberately
            // structural: inspecting its private caches would itself couple
            // status to freshness implementation details.
            ready_lease_memo_holds_observations: true,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)]
    fn set_worker_start_gate_for_test(&self, gate: Option<Arc<(Mutex<bool>, Condvar)>>) {
        *self
            .coordinator
            .worker_start_gate
            .lock()
            .expect("activation worker gate poisoned") = gate;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)]
    fn worker_start_count_for_test(&self) -> u64 {
        self.coordinator.worker_start_count.load(Ordering::Acquire)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn preparation_counts_for_test(&self) -> (u64, u64) {
        (
            self.coordinator
                .native_preparation_count
                .load(Ordering::Acquire),
            self.coordinator
                .retrieval_finalization_count
                .load(Ordering::Acquire),
        )
    }

    /// Make any later native preparation or retrieval finalization fail the
    /// owning activation. Tests arm this only after the first activation has
    /// established the expected phase counts.
    #[cfg(any(test, feature = "test-support"))]
    pub fn arm_preparation_seams_for_test(&self) {
        let (native, finalization) = self.preparation_counts_for_test();
        self.coordinator
            .native_preparation_limit
            .store(native, Ordering::Release);
        self.coordinator
            .retrieval_finalization_limit
            .store(finalization, Ordering::Release);
        self.coordinator
            .preparation_seams_armed
            .store(true, Ordering::Release);
    }

    /// Use an already strict published retrieval fixture at the finalization
    /// seam. The activation worker still advances through and records the
    /// native-preparation/finalization boundaries, then performs the normal
    /// strict readiness and identity validation before publishing its lease.
    #[cfg(any(test, feature = "test-support"))]
    pub fn use_published_retrieval_fixture_for_test(&self) {
        self.coordinator
            .use_published_retrieval_fixture
            .store(true, Ordering::Release);
    }

    fn should_finalize_retrieval_for_activation(&self) -> bool {
        #[cfg(any(test, feature = "test-support"))]
        if self
            .coordinator
            .use_published_retrieval_fixture
            .load(Ordering::Acquire)
        {
            return false;
        }
        true
    }

    fn record_preparation_phase(&self, phase: ActivationPreparationPhase) -> Result<(), ApiError> {
        #[cfg(any(test, feature = "test-support"))]
        {
            let (count, limit, label) = match phase {
                ActivationPreparationPhase::NativeEmbedding => (
                    self.coordinator
                        .native_preparation_count
                        .fetch_add(1, Ordering::AcqRel)
                        + 1,
                    self.coordinator
                        .native_preparation_limit
                        .load(Ordering::Acquire),
                    "native embedding preparation",
                ),
                ActivationPreparationPhase::RetrievalFinalization => (
                    self.coordinator
                        .retrieval_finalization_count
                        .fetch_add(1, Ordering::AcqRel)
                        + 1,
                    self.coordinator
                        .retrieval_finalization_limit
                        .load(Ordering::Acquire),
                    "retrieval finalization",
                ),
            };
            if self
                .coordinator
                .preparation_seams_armed
                .load(Ordering::Acquire)
                && count > limit
            {
                return Err(ApiError::internal(format!(
                    "test preparation seam was invoked again: {label}"
                )));
            }
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = phase;
        Ok(())
    }

    fn snapshot_for_target(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> Option<ActivationSnapshot> {
        let requested = ActivationTarget::new(project_root, storage_path);
        let state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        state
            .target
            .as_ref()
            .is_some_and(|current| current.matches(&requested))
            .then(|| state.current.clone())
            .flatten()
    }

    fn source_freshness_memo_for_target(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> Option<codestory_workspace::SourceFreshnessMemo> {
        let requested = ActivationTarget::new(project_root, storage_path);
        let state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        (state
            .target
            .as_ref()
            .is_some_and(|current| current.matches(&requested))
            && state
                .current
                .as_ref()
                .is_some_and(|snapshot| snapshot.state == ActivationState::Ready))
        .then(|| {
            state
                .ready_lease
                .as_ref()
                .map(|lease| lease.source_freshness_memo.clone())
        })
        .flatten()
    }

    fn target_for_request(&self, project_root: &Path, storage_path: &Path) -> ActivationTarget {
        let requested = ActivationTarget::new(project_root, storage_path);
        if let Some(target) = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned")
            .target
            .as_ref()
            .filter(|target| target.matches(&requested))
            .cloned()
        {
            return target;
        }
        requested
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
        state.ready_lease = None;
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

    fn retained_core_publication(
        &self,
        storage_path: &Path,
    ) -> Result<Option<IndexPublicationDto>, ApiError> {
        if !storage_path.is_file() {
            return Ok(None);
        }
        let storage = Store::open_read_only(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to open retained core publication observationally: {error}"
            ))
        })?;
        let snapshot = storage.read_snapshot().map_err(|error| {
            ApiError::internal(format!(
                "Failed to pin retained core publication observation: {error}"
            ))
        })?;
        let retained = if snapshot
            .storage()
            .has_incomplete_incremental_run()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to inspect retained core publication fence: {error}"
                ))
            })? {
            None
        } else {
            snapshot
                .storage()
                .get_complete_index_publication()
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to inspect retained core publication: {error}"
                    ))
                })?
                .map(crate::index_publication_dto)
        };
        snapshot.finish().map_err(|error| {
            ApiError::internal(format!(
                "Failed to finish retained core publication observation: {error}"
            ))
        })?;
        Ok(retained)
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
        let target = self.target_for_request(project_root, storage_path);
        let (operation_id, activation_cancelled) = loop {
            let ready_candidate = {
                let mut state = self
                    .coordinator
                    .state
                    .lock()
                    .expect("activation coordinator poisoned");
                if state.running {
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
                    return self.wait_for_activation(
                        &target,
                        &operation_id,
                        true,
                        request_cancelled.as_ref(),
                        foreground_budget,
                    );
                }
                if !state
                    .target
                    .as_ref()
                    .is_some_and(|current| current.matches(&target))
                {
                    state.target = Some(target.clone());
                    state.current = None;
                    state.ready_lease = None;
                }
                match (state.current.as_ref(), state.ready_lease.as_ref()) {
                    (Some(snapshot), Some(lease)) if snapshot.state == ActivationState::Ready => {
                        Some((snapshot.clone(), lease.clone()))
                    }
                    _ => None,
                }
            };

            if let Some((candidate_snapshot, candidate_lease)) = ready_candidate {
                let probe = self.probe_ready_lease(storage_path, &candidate_lease);
                if request_cancelled.load(Ordering::Acquire) {
                    return Err(ApiError::new(
                        "cancelled",
                        "request cancelled while observing the ready project lease",
                    ));
                }
                let mut state = self
                    .coordinator
                    .state
                    .lock()
                    .expect("activation coordinator poisoned");
                if state.running {
                    if !state
                        .target
                        .as_ref()
                        .is_some_and(|current| current.matches(&target))
                    {
                        return Err(ApiError::new(
                            "project_unavailable",
                            "a different logical project started activation while the ready lease was being observed",
                        ));
                    }
                    let operation_id = state
                        .current
                        .as_ref()
                        .expect("running activation has a snapshot")
                        .operation_id
                        .clone();
                    drop(state);
                    return self.wait_for_activation(
                        &target,
                        &operation_id,
                        true,
                        request_cancelled.as_ref(),
                        foreground_budget,
                    );
                }
                let candidate_is_current = state
                    .target
                    .as_ref()
                    .is_some_and(|current| current.matches(&target))
                    && state.current.as_ref().is_some_and(|snapshot| {
                        snapshot.state == ActivationState::Ready
                            && snapshot.operation_id == candidate_snapshot.operation_id
                            && snapshot.revision == candidate_snapshot.revision
                    })
                    && state.ready_lease.as_ref() == Some(&candidate_lease);
                if !candidate_is_current {
                    drop(state);
                    continue;
                }
                if probe.admissible {
                    let snapshot = state
                        .current
                        .clone()
                        .expect("validated ready lease has a snapshot");
                    drop(state);
                    return Ok(ActivationRun {
                        snapshot,
                        joined: false,
                    });
                }
                break self.begin_activation_locked(
                    &mut state,
                    &target,
                    probe.retained_core_publication,
                );
            }

            let retained_core_publication =
                self.retained_core_publication(storage_path).unwrap_or(None);
            let mut state = self
                .coordinator
                .state
                .lock()
                .expect("activation coordinator poisoned");
            if state.running {
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
                return self.wait_for_activation(
                    &target,
                    &operation_id,
                    true,
                    request_cancelled.as_ref(),
                    foreground_budget,
                );
            }
            if state
                .current
                .as_ref()
                .is_some_and(|snapshot| snapshot.state == ActivationState::Ready)
                && state.ready_lease.is_some()
            {
                drop(state);
                continue;
            }
            break self.begin_activation_locked(&mut state, &target, retained_core_publication);
        };

        let operation = ActivationOperation {
            service: self.clone(),
            operation_id: operation_id.clone(),
            cancelled: activation_cancelled,
        };
        let worker_operation = operation.clone();
        let worker_service = self.clone();
        let worker_project_root = project_root.to_path_buf();
        let worker_storage_path = storage_path.to_path_buf();
        #[cfg(any(test, feature = "test-support"))]
        let worker_start_gate = self
            .coordinator
            .worker_start_gate
            .lock()
            .expect("activation worker gate poisoned")
            .clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("codestory-{operation_id}"))
            .spawn(move || {
                #[cfg(any(test, feature = "test-support"))]
                worker_service
                    .coordinator
                    .worker_start_count
                    .fetch_add(1, Ordering::AcqRel);
                #[cfg(any(test, feature = "test-support"))]
                if let Some(gate) = worker_start_gate {
                    let (released, changed) = gate.as_ref();
                    let mut released = released
                        .lock()
                        .expect("activation worker test gate poisoned");
                    while !*released {
                        released = changed
                            .wait(released)
                            .expect("activation worker test gate poisoned");
                    }
                }
                run_activation_worker(&worker_operation, || {
                    worker_service.activate_once(
                        &worker_operation,
                        worker_project_root,
                        worker_storage_path,
                    )
                });
            })
        {
            let error = ApiError::new(
                "project_unavailable",
                format!("failed to start project activation worker: {error}"),
            );
            let _ = operation.finish(Some(&error));
            return Err(error);
        }

        self.wait_for_activation(
            &target,
            &operation_id,
            false,
            request_cancelled.as_ref(),
            foreground_budget,
        )
    }

    fn probe_ready_lease(&self, storage_path: &Path, lease: &ReadyLease) -> ReadyLeaseProbe {
        let configuration_matches = self.controller.runtime_configuration_id().ok().as_ref()
            == Some(&lease.configuration_id);
        let retrieval_identity =
            codestory_retrieval::observe_ready_retrieval_identity_for_project_id(
                storage_path,
                &self.controller.runtime_config,
                &lease.retrieval.manifest.project_id,
            )
            .ok()
            .flatten();
        let retrieval_matches =
            ready_retrieval_identity_matches(retrieval_identity.as_ref(), &lease.retrieval);
        let retained_core_publication =
            self.retained_core_publication(storage_path).unwrap_or(None);
        let core_matches = retained_core_publication.as_ref() == Some(&lease.core_publication);
        ReadyLeaseProbe {
            admissible: configuration_matches
                && lease.source.is_admissible_snapshot()
                && self.ready_lease_source_observer_unchanged(lease.source_observer.as_ref())
                && retrieval_matches
                && core_matches,
            retained_core_publication,
        }
    }

    /// Whether the working tree has stood still since the lease recorded its source snapshot.
    ///
    /// The lease probe is the one readiness gate that never re-scans: it compares stored
    /// identities, and a source write moves none of them. Comparing observer epochs is what makes
    /// the stored snapshot falsifiable at all — the epoch advances the moment an admitted path is
    /// written, which is precisely the post-scan window the lease used to keep re-admitting.
    ///
    /// A lease minted without an observer compares nothing and stays admissible. Refusing those
    /// would re-activate on every request on WSL drive mounts and network shares, which is the
    /// availability cost the typed-unknown fallback exists to refuse.
    fn ready_lease_source_observer_unchanged(
        &self,
        recorded: Option<&ObservedSourceEpoch>,
    ) -> bool {
        let Some(recorded) = recorded else {
            return true;
        };
        let Ok(project_root) = self.controller.require_project_root() else {
            return false;
        };
        // A re-armed session, or one that has taken a sticky loss, cannot speak for the window
        // the lease was minted in. Refusing costs one re-activation and then converges: the next
        // lease is minted without an observer and falls back to the floor.
        self.controller
            .observed_source_epoch(&project_root)
            .is_some_and(|observed| observed == *recorded)
    }

    fn begin_activation_locked(
        &self,
        state: &mut ActivationCoordinatorState,
        target: &ActivationTarget,
        retained_core_publication: Option<IndexPublicationDto>,
    ) -> (String, Arc<AtomicBool>) {
        if !state
            .target
            .as_ref()
            .is_some_and(|current| current.matches(target))
        {
            state.target = Some(target.clone());
            state.current = None;
        }
        state.ready_lease = None;
        let operation_id = if let Some(snapshot) = state.current.as_mut() {
            let replacing_ready = snapshot.state == ActivationState::Ready;
            snapshot.attempt += 1;
            snapshot.revision += 1;
            snapshot.failure = None;
            snapshot.failure_code = None;
            snapshot.failure_details = None;
            snapshot.embedding_capacity = None;
            snapshot.embedding_retry = None;
            snapshot.retry_after_ms = Some(250);
            snapshot.state = ActivationState::Preparing;
            if replacing_ready {
                snapshot.stage = ActivationStage::Discovery;
                snapshot.progress = activation_stage_progress(ActivationStage::Discovery);
            }
            let retained_was_ready = snapshot.capabilities.local_navigation
                == ActivationCapabilityState::Ready
                && snapshot.retained_core_publication == retained_core_publication;
            snapshot.retained_core_publication = retained_core_publication;
            snapshot.capabilities.local_navigation = if retained_was_ready {
                ActivationCapabilityState::Ready
            } else if snapshot.retained_core_publication.is_some() {
                ActivationCapabilityState::Retained
            } else {
                ActivationCapabilityState::Unavailable
            };
            snapshot.capabilities.broad_search = ActivationCapabilityState::Unavailable;
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
                revision: 1,
                state: ActivationState::Preparing,
                stage: ActivationStage::Discovery,
                progress: activation_stage_progress(ActivationStage::Discovery),
                attempt: 1,
                retry_after_ms: Some(250),
                embedding_capacity: None,
                embedding_retry: None,
                failure_code: None,
                failure: None,
                failure_details: None,
                retained_core_publication: retained_core_publication.clone(),
                capabilities: ActivationCapabilities {
                    local_navigation: if retained_core_publication.is_some() {
                        ActivationCapabilityState::Retained
                    } else {
                        ActivationCapabilityState::Unavailable
                    },
                    broad_search: ActivationCapabilityState::Unavailable,
                },
            });
            operation_id
        };
        let activation_cancelled = Arc::new(AtomicBool::new(false));
        state.running = true;
        state.current_cancel = Some(Arc::clone(&activation_cancelled));
        (operation_id, activation_cancelled)
    }

    fn require_ready_retrieval_identity_unchanged(
        &self,
        project_root: &Path,
        storage_path: &Path,
        expected: &codestory_retrieval::ReadyRetrievalIdentity,
    ) -> Result<(), ApiError> {
        let current = codestory_retrieval::ready_retrieval_identity_for_runtime(
            project_root,
            storage_path,
            &self.controller.runtime_config,
        )
        .map_err(|error| {
            ApiError::new(
                "publication_changed",
                format!(
                    "failed to revalidate retrieval identity before ready-lease publication: {error}"
                ),
            )
        })?;
        if current.as_ref() != Some(expected) {
            return Err(ApiError::new(
                "publication_changed",
                "the retrieval publication or producer changed during ready-lease validation",
            ));
        }
        Ok(())
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

    /// Cancel the in-flight activation and wait for it to reach a quiescent
    /// boundary. Past the activation quiescence budget the worker is never
    /// detached: it may still hold a publication or store lock, so the process
    /// fail-stops with the recorded reason instead of continuing.
    pub fn cancel_and_wait(&self) -> ActivationQuiescence {
        self.cancel_and_wait_or_fail_stop(ACTIVATION_QUIESCENCE_BUDGET)
    }

    fn cancel_and_wait_or_fail_stop(&self, budget: Duration) -> ActivationQuiescence {
        let quiescence = self.cancel_and_wait_within(budget);
        if quiescence == ActivationQuiescence::FailStopRequired {
            run_activation_fail_stop(ACTIVATION_QUIESCENCE_FAIL_STOP);
        }
        quiescence
    }

    /// Bounded quiescence join without the fail-stop side effect, so callers
    /// (and deterministic tests) can observe the verdict directly.
    pub fn cancel_and_wait_within(&self, budget: Duration) -> ActivationQuiescence {
        let deadline = Instant::now()
            .checked_add(budget)
            .unwrap_or_else(Instant::now);
        let mut state = self
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        if let Some(cancelled) = state.current_cancel.as_ref() {
            cancelled.store(true, Ordering::Release);
        }
        while state.running {
            let now = Instant::now();
            if now >= deadline {
                return ActivationQuiescence::FailStopRequired;
            }
            let remaining = deadline.saturating_duration_since(now);
            state = self
                .coordinator
                .changed
                .wait_timeout(state, remaining.min(ACTIVATION_WAIT_SLICE))
                .expect("activation coordinator poisoned")
                .0;
        }
        ActivationQuiescence::Quiesced
    }

    /// Cancel the running activation and wait at most `budget` for it to stop.
    ///
    /// Returns `true` once no activation is running. Callers that are already
    /// recovering from a failure use this instead of [`Self::cancel_and_wait`]:
    /// an unbounded wait would hold the recovering request for as long as the
    /// activation takes, and a poisoned coordinator — exactly the state a
    /// panicking request leaves behind — would turn the recovery itself into a
    /// second panic. Poison is therefore read through rather than asserted on:
    /// the only fields touched are the cancellation flag and the `running`
    /// bit, and both stay meaningful after an unrelated unwind.
    pub fn cancel_and_wait_timeout(&self, budget: Duration) -> bool {
        let deadline = Instant::now()
            .checked_add(budget)
            .unwrap_or_else(Instant::now);
        let mut state = match self.coordinator.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(cancelled) = state.current_cancel.as_ref() {
            cancelled.store(true, Ordering::Release);
        }
        while state.running {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let remaining = deadline.saturating_duration_since(now);
            state = match self
                .coordinator
                .changed
                .wait_timeout(state, remaining.min(ACTIVATION_WAIT_SLICE))
            {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        true
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
        summary.freshness = Some(
            self.controller
                .index_freshness_uncached(FreshnessObservationPolicy::Unobserved)?,
        );

        operation.set_stage(ActivationStage::CoreFreshness);
        let core_stale = summary.publication.is_none()
            || summary.stats.node_count == 0
            || self
                .controller
                .complete_core_requires_publication_repair(&storage_path)?
            // A bounded freshness check cannot prove drift either way, so treating it as stale
            // would rebuild the whole index on every activation of a large repository and never
            // reach a different answer.
            || summary
                .freshness
                .as_ref()
                .is_none_or(|freshness| !index_freshness_admits_operation(freshness));
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
            summary.freshness = Some(
                self.controller
                    .index_freshness_uncached(FreshnessObservationPolicy::Unobserved)?,
            );
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
                .is_some_and(index_freshness_admits_operation);
        if !local_ready {
            if summary.stats.node_count > 0
                && summary.stats.fatal_error_count == 0
                && !self
                    .controller
                    .complete_core_requires_publication_repair(&storage_path)?
                && let Some(publication) = summary.publication.clone()
            {
                operation.set_retained_local_publication(publication);
            }
            return Err(ApiError::new(
                "project_unavailable",
                "activation did not produce a fresh complete core publication",
            ));
        }
        let local_publication = summary
            .publication
            .clone()
            .expect("fresh complete core has a publication identity");
        operation.set_local_publication(local_publication.clone());

        operation.ensure_not_cancelled("search preparation")?;
        operation.set_stage(ActivationStage::SearchPreparation);
        let token = CancellationToken::from_shared_flag(Arc::clone(&operation.cancelled));
        self.controller
            .prepare_search_state_for_activation(&token)?;

        operation.ensure_not_cancelled("dense preparation")?;
        operation.set_stage(ActivationStage::DensePreparation);
        self.record_preparation_phase(ActivationPreparationPhase::NativeEmbedding)?;
        codestory_retrieval::ensure_product_embedding_backend_for_runtime(
            &self.controller.runtime_config,
        )
        .map_err(map_activation_error)?;
        operation.ensure_not_cancelled("retrieval publication")?;
        operation.set_stage(ActivationStage::Publication);
        self.record_preparation_phase(ActivationPreparationPhase::RetrievalFinalization)?;
        if self.should_finalize_retrieval_for_activation() {
            codestory_retrieval::finalize_index_for_runtime_with_cancel(
                &project_root,
                &storage_path,
                &self.controller.runtime_config,
                operation.cancelled.as_ref(),
            )
            .map_err(map_activation_error)?;
        }
        operation.ensure_not_cancelled("retrieval validation")?;
        operation.set_stage(ActivationStage::Validation);
        let retrieval = codestory_retrieval::ready_retrieval_identity_for_runtime(
            &project_root,
            &storage_path,
            &self.controller.runtime_config,
        )
        .map_err(map_activation_error)?
        .ok_or_else(|| {
            ApiError::new(
                "project_unavailable",
                "retrieval identity became unavailable before strict ready-lease validation",
            )
        })?;
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
        // Read the epoch *before* the scan, not after: a mutation that lands while the scan runs
        // has to fall outside the lease's recorded epoch, or the lease would vouch for the very
        // window the observer just proved was contested.
        let source_observer = self.controller.observed_source_epoch(&project_root);
        let source_freshness = self
            .controller
            .index_freshness_uncached(FreshnessObservationPolicy::ObserveSourceRoot)?;
        if !index_freshness_admits_operation(&source_freshness) {
            return Err(ApiError::new(
                "publication_changed",
                index_freshness_block_message("activation", &source_freshness),
            ));
        }
        let core_publication = self
            .retained_core_publication(&storage_path)?
            .ok_or_else(|| {
                ApiError::new(
                    "publication_changed",
                    "the complete core publication disappeared before ready-lease publication",
                )
            })?;
        if core_publication != local_publication {
            return Err(ApiError::new(
                "publication_changed",
                "the complete core publication changed before ready-lease publication",
            ));
        }
        self.require_ready_retrieval_identity_unchanged(&project_root, &storage_path, &retrieval)?;
        let revalidated_core = self
            .retained_core_publication(&storage_path)?
            .ok_or_else(|| {
                ApiError::new(
                    "publication_changed",
                    "the complete core publication disappeared during ready-lease validation",
                )
            })?;
        if revalidated_core != core_publication {
            return Err(ApiError::new(
                "publication_changed",
                "the complete core publication changed during ready-lease validation",
            ));
        }
        if revalidated_core != local_publication {
            return Err(ApiError::new(
                "publication_changed",
                "the revalidated core publication differs from the activated core",
            ));
        }
        operation.set_ready_lease(ReadyLease {
            configuration_id: self.controller.runtime_configuration_id()?,
            core_publication: revalidated_core,
            retrieval,
            source: ReadySourceIdentity::from(&source_freshness),
            source_freshness_memo: codestory_workspace::SourceFreshnessMemo::default(),
            source_observer,
        });
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

/// Whether a freshness observation permits serving an operation from the current publication.
///
/// `Fresh` obviously admits and `Stale` obviously blocks. `NotChecked` splits: a check that could
/// not run establishes nothing and must fail closed, but one that stopped at a deliberate
/// discovery bound says only that drift is unknown. The publication itself is still complete, so
/// blocking there would permanently lock every repository past the bound out of packet and search
/// with no way to re-open it.
fn index_freshness_admits_operation(freshness: &IndexFreshnessDto) -> bool {
    match freshness.status {
        IndexFreshnessStatusDto::Fresh => true,
        IndexFreshnessStatusDto::Stale => false,
        IndexFreshnessStatusDto::NotChecked => matches!(
            freshness.not_checked_cause,
            Some(IndexFreshnessNotCheckedCauseDto::BoundedInventory)
        ),
    }
}

fn index_freshness_block_message(operation: &str, freshness: &IndexFreshnessDto) -> String {
    // The reason is the only thing that tells an operator what to change, so it must survive.
    match freshness.reason.as_deref() {
        Some(reason) => {
            format!("{operation} requires a fresh complete core publication: {reason}")
        }
        None => format!("{operation} requires a fresh complete core publication"),
    }
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
    activation: Option<ActivationService>,
    next_id: Arc<AtomicU64>,
}

impl PublicOperationService {
    pub(crate) fn new(controller: AppController) -> Self {
        Self {
            controller,
            activation: None,
            next_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn new_with_activation(
        controller: AppController,
        activation: ActivationService,
    ) -> Self {
        Self {
            controller,
            activation: Some(activation),
            next_id: Arc::new(AtomicU64::new(0)),
        }
    }

    fn source_freshness_scope(&self) -> codestory_workspace::SourceFreshnessScope {
        let memo = self.activation.as_ref().and_then(|activation| {
            let project_root = self.controller.require_project_root().ok()?;
            let storage_path = self.controller.require_storage_path().ok()?;
            activation.source_freshness_memo_for_target(&project_root, &storage_path)
        });
        memo.map_or_else(
            codestory_workspace::SourceFreshnessScope::enter,
            codestory_workspace::SourceFreshnessScope::enter_with_memo,
        )
    }

    fn retained_core_allows(&self, operation: &str, publication: &IndexPublicationRecord) -> bool {
        !operation_requires_retrieval(operation)
            && self.activation.as_ref().is_some_and(|activation| {
                let Some(project_root) = self.controller.require_project_root().ok() else {
                    return false;
                };
                let Some(storage_path) = self.controller.require_storage_path().ok() else {
                    return false;
                };
                activation
                    .snapshot_for_target(&project_root, &storage_path)
                    .is_some_and(|snapshot| {
                        matches!(
                            snapshot.capabilities.local_navigation,
                            ActivationCapabilityState::Ready | ActivationCapabilityState::Retained
                        ) && snapshot.retained_core_publication.as_ref()
                            == Some(&crate::index_publication_dto(publication.clone()))
                    })
            })
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

    #[cfg(any(
        test,
        feature = "test-support",
        feature = "v3-evidence-separation-support"
    ))]
    pub(crate) fn active_project_identity_v3(
        &self,
    ) -> Result<codestory_workspace::ProjectIdentityV3, ApiError> {
        let project_root = self.controller.require_project_root()?;
        Ok(codestory_workspace::project_identity_v3(&project_root))
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
        // The ready lease owns reusable freshness and readiness observations;
        // this scope owns only the operation's counters. The post-build check
        // below still drops stored-file verdicts and re-derives them from
        // content, so same-mtime drift and torn reads win over reuse.
        let _source_freshness_scope = self.source_freshness_scope();
        for attempt in 1..=2 {
            let result = self.controller.with_complete_core_snapshot(|publication| {
                let freshness = self
                    .controller
                    .index_freshness_uncached(FreshnessObservationPolicy::ObserveSourceRoot)?;
                if !index_freshness_admits_operation(&freshness) {
                    codestory_workspace::invalidate_lease_memoized_values();
                    if !self.retained_core_allows(operation, publication) {
                        return Err(ApiError::new(
                            "project_unavailable",
                            index_freshness_block_message(operation, &freshness),
                        ));
                    }
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
                    // Reverified, not `index_freshness_uncached`: this refusal
                    // exists to catch source that moved while the build ran,
                    // including a mutation that preserved both mtime and byte
                    // length. Only re-hashing content sees that, so the
                    // operation-scoped verdict memo must not answer here.
                    // Observed as well, because the re-read is still a scan with
                    // a window of its own: the memo drop makes the scan see
                    // drift that landed before it started, and the observer
                    // makes it refuse drift that lands while it runs.
                    let after = self.controller.index_freshness_reverified(
                        FreshnessObservationPolicy::ObserveSourceRoot,
                    )?;
                    if !index_freshness_admits_operation(&after) {
                        codestory_workspace::invalidate_lease_memoized_values();
                        if !self.retained_core_allows(operation, publication) {
                            return Err(ApiError::new(
                                "publication_changed",
                                format!("source inputs changed while running {operation}"),
                            ));
                        }
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
        // Observational operations borrow the same ready-lease memo so a nested
        // wrapper cannot reintroduce a content or readiness pass.
        let _source_freshness_scope = self.source_freshness_scope();
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

fn run_activation_worker(
    operation: &ActivationOperation,
    activate: impl FnOnce() -> Result<(), ApiError>,
) {
    // This thread is the one whose quiescence an eviction or shutdown joins
    // against ACTIVATION_QUIESCENCE_BUDGET, and past that budget the process
    // aborts. Its lock waits must therefore all be interruptible. The deep ones
    // — model materialization, promotion, the search index and generation
    // catalog guards, retention — are reached through APIs that carry no
    // cancellation flag, so the flag is installed on the thread instead and
    // every bounded acquisition below inherits it.
    let result = codestory_contracts::bounded_locks::with_thread_cancellation(
        Arc::clone(&operation.cancelled),
        || {
            catch_unwind(AssertUnwindSafe(|| {
                let attempt = operation.attempt();
                activate()
                    .map_err(|error| classify_activation_api_error_for_attempt(error, attempt))
            }))
            .unwrap_or_else(|_| {
                Err(ApiError::new(
                    "project_unavailable",
                    "project activation worker stopped unexpectedly",
                ))
            })
        },
    );
    let _ = operation.finish(result.as_ref().err());
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
            snapshot.stage = snapshot.stage.max(stage);
            snapshot.progress = snapshot.progress.max(activation_stage_progress(stage));
            snapshot.state = ActivationState::Updating;
            snapshot.revision += 1;
        }
        self.service.coordinator.changed.notify_all();
    }

    fn set_local_publication(&self, publication: IndexPublicationDto) {
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
            snapshot.retained_core_publication = Some(publication);
            snapshot.capabilities.local_navigation = ActivationCapabilityState::Ready;
            snapshot.revision += 1;
        }
        self.service.coordinator.changed.notify_all();
    }

    fn set_retained_local_publication(&self, publication: IndexPublicationDto) {
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
            snapshot.retained_core_publication = Some(publication);
            snapshot.capabilities.local_navigation = ActivationCapabilityState::Retained;
            snapshot.revision += 1;
        }
        self.service.coordinator.changed.notify_all();
    }

    fn set_ready_lease(&self, lease: ReadyLease) {
        let mut state = self
            .service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator poisoned");
        if state
            .current
            .as_ref()
            .is_some_and(|snapshot| snapshot.operation_id == self.operation_id)
        {
            state.ready_lease = Some(lease);
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
            snapshot.revision += 1;
        }
        self.service.coordinator.changed.notify_all();
    }

    fn finish(&self, error: Option<&ApiError>) -> Option<ActivationSnapshot> {
        let mut state = self
            .service
            .coordinator
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if error.is_some() {
            state.ready_lease = None;
        }
        let ready_lease_present = state.ready_lease.is_some();
        let Some(snapshot) = state
            .current
            .as_mut()
            .filter(|snapshot| snapshot.operation_id == self.operation_id)
        else {
            self.service.coordinator.changed.notify_all();
            return None;
        };
        if let Some(error) = error {
            let capability = match error.code.as_str() {
                "cancelled" => ActivationCapabilityState::Cancelled,
                "activation_retryable"
                | "embedding_capacity"
                | "cache_busy"
                | "publication_changed" => ActivationCapabilityState::Retryable,
                _ => ActivationCapabilityState::Unavailable,
            };
            if !matches!(
                snapshot.capabilities.local_navigation,
                ActivationCapabilityState::Ready | ActivationCapabilityState::Retained
            ) {
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
                ActivationCapabilityState::Retained => ActivationState::Updating,
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
            debug_assert!(
                ready_lease_present,
                "successful activation must publish its ready lease before completion"
            );
            snapshot.state = ActivationState::Ready;
            snapshot.stage = ActivationStage::Ready;
            snapshot.progress = activation_stage_progress(ActivationStage::Ready);
            snapshot.retry_after_ms = None;
            snapshot.embedding_capacity = None;
            snapshot.embedding_retry = None;
            snapshot.failure_code = None;
            snapshot.failure_details = None;
            snapshot.failure = None;
        }
        snapshot.revision += 1;
        let snapshot = snapshot.clone();
        state.running = false;
        state.current_cancel = None;
        self.service.coordinator.changed.notify_all();
        Some(snapshot)
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

    pub fn republish_semantic_projections_blocking(
        &self,
    ) -> Result<crate::SemanticProjectionRepublishOutcome, ApiError> {
        self.controller.republish_semantic_projections_blocking()
    }

    pub fn republish_semantic_projections_blocking_with_cancel(
        &self,
        cancel_token: &CancellationToken,
    ) -> Result<crate::SemanticProjectionRepublishOutcome, ApiError> {
        self.controller
            .republish_semantic_projections_blocking_with_cancel(cancel_token)
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

    pub fn republish_semantic_projections_blocking(
        &self,
    ) -> Result<crate::SemanticProjectionRepublishOutcome, ApiError> {
        self.controller.republish_semantic_projections_blocking()
    }

    pub fn republish_semantic_projections_blocking_with_cancel(
        &self,
        cancel_token: &CancellationToken,
    ) -> Result<crate::SemanticProjectionRepublishOutcome, ApiError> {
        self.controller
            .republish_semantic_projections_blocking_with_cancel(cancel_token)
    }

    /// Bind one explicit project/cache pair and acquire its writer lock before
    /// opening or migrating the stored core.
    pub fn republish_semantic_projections_at_blocking(
        &self,
        root: std::path::PathBuf,
        storage_path: std::path::PathBuf,
    ) -> Result<crate::SemanticProjectionRepublishOutcome, ApiError> {
        self.controller
            .republish_semantic_projections_at_blocking(root, storage_path)
    }

    pub fn complete_index_publication(&self) -> Result<Option<IndexPublicationRecord>, ApiError> {
        self.controller.complete_index_publication()
    }

    pub fn ensure_incremental_refresh_compatible(&self) -> Result<(), ApiError> {
        self.controller.ensure_incremental_refresh_compatible()
    }

    pub fn ensure_incremental_refresh_compatible_at(
        &self,
        root: &std::path::Path,
        storage_path: &std::path::Path,
    ) -> Result<(), ApiError> {
        self.controller
            .ensure_incremental_refresh_compatible_at(root, storage_path)
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

    pub fn update_bookmark(
        &self,
        id: &str,
        req: UpdateBookmarkRequest,
    ) -> Result<BookmarkDto, ApiError> {
        self.controller.update_bookmark(id, req)
    }

    pub fn delete_bookmark(&self, id: &str) -> Result<(), ApiError> {
        self.controller.delete_bookmark(id)
    }
}

#[cfg(test)]
mod embedding_start_classification_tests {
    use super::*;

    fn classify(code: &str, retry_class: &str) -> ApiError {
        let error = anyhow::Error::new(codestory_retrieval::PerUserEmbeddingError {
            code: code.into(),
            message: format!("{code} occurred"),
            retry_class: retry_class.into(),
            retry_after_ms: 250,
            retry_condition: "the server finishes starting".into(),
            capacity: None,
        });
        map_activation_error(error)
    }

    #[test]
    fn a_slow_cold_start_is_retryable_rather_than_terminal() {
        // The spawned server keeps converging past the client's budget, so the next request
        // usually connects. Reporting this as terminal turned an ordinary slow first use into a
        // failed ground with nothing the caller could do.
        for code in ["embedding_server_start_timeout", "embedding_server_absent"] {
            let error = classify(code, "after_delay");
            assert_eq!(
                error.code, "activation_retryable",
                "{code} must invite a retry"
            );
            assert_eq!(
                error
                    .details
                    .as_ref()
                    .and_then(|details| details.cause_code.as_deref()),
                Some("embedding_retryable"),
                "{code} must keep its cause visible"
            );
        }
    }

    #[test]
    fn an_unretryable_embedding_failure_still_fails_closed() {
        assert_eq!(
            classify("native_model_not_embedded", "none").code,
            "project_unavailable"
        );
    }
}

#[cfg(test)]
mod freshness_gate_tests {
    use super::*;

    #[test]
    fn dark_indexed_call_path_builder_remains_core_only() {
        assert!(!operation_requires_retrieval(
            codestory_agent::proof_qualification_test_support::PROOF_DOMAIN
        ));
        for operation in ["packet", "search", "context", "drill"] {
            assert!(operation_requires_retrieval(operation));
        }
    }

    fn freshness(
        status: IndexFreshnessStatusDto,
        cause: Option<IndexFreshnessNotCheckedCauseDto>,
        reason: Option<&str>,
    ) -> IndexFreshnessDto {
        IndexFreshnessDto {
            status,
            changed_file_count: 0,
            new_file_count: 0,
            removed_file_count: 0,
            checked_file_count: 0,
            indexed_file_count: 30_000,
            duration_ms: 0,
            reason: reason.map(str::to_string),
            not_checked_cause: cause,
            samples: Vec::new(),
        }
    }

    #[test]
    fn bounded_inventory_admits_broad_retrieval_while_stale_still_blocks() {
        // A repository past the discovery bound has a complete publication; only drift is unknown.
        // Blocking it would lock packet and search out permanently, with no command that reopens
        // them, because the bound is a property of repository size rather than of index state.
        assert!(index_freshness_admits_operation(&freshness(
            IndexFreshnessStatusDto::NotChecked,
            Some(IndexFreshnessNotCheckedCauseDto::BoundedInventory),
            Some("indexed file inventory exceeds bounded freshness cap (30000 > 25000)"),
        )));

        assert!(index_freshness_admits_operation(&freshness(
            IndexFreshnessStatusDto::Fresh,
            None,
            None,
        )));

        assert!(!index_freshness_admits_operation(&freshness(
            IndexFreshnessStatusDto::Stale,
            None,
            None,
        )));
    }

    #[test]
    fn a_check_that_could_not_run_still_fails_closed() {
        // Unlike a bound, this establishes nothing about the publication.
        assert!(!index_freshness_admits_operation(&freshness(
            IndexFreshnessStatusDto::NotChecked,
            Some(IndexFreshnessNotCheckedCauseDto::InventoryUnavailable),
            Some("failed to read indexed file inventory: disk error"),
        )));

        // A NotChecked with no recorded cause predates the distinction; fail closed.
        assert!(!index_freshness_admits_operation(&freshness(
            IndexFreshnessStatusDto::NotChecked,
            None,
            None,
        )));
    }

    #[test]
    fn blocking_reports_the_reason_instead_of_discarding_it() {
        let message = index_freshness_block_message(
            "packet",
            &freshness(
                IndexFreshnessStatusDto::NotChecked,
                Some(IndexFreshnessNotCheckedCauseDto::InventoryUnavailable),
                Some("failed to read indexed file inventory: disk error"),
            ),
        );
        assert!(
            message.contains("failed to read indexed file inventory"),
            "operator needs the cause to act on it, got: {message}"
        );

        assert_eq!(
            index_freshness_block_message(
                "search",
                &freshness(IndexFreshnessStatusDto::Stale, None, None)
            ),
            "search requires a fresh complete core publication"
        );
    }
}

#[cfg(test)]
pub(crate) mod activation_tests {
    use super::*;
    use crate::Runtime;
    use crate::search_publication::{
        read_search_generation_completion, search_index_path_for_publication,
    };
    use crate::test_support::git;
    use std::fs;

    pub(crate) struct ReadyActivationFixture {
        pub(crate) project: tempfile::TempDir,
        _cache: tempfile::TempDir,
        pub(crate) runtime: Runtime,
        storage_path: PathBuf,
        lease: ReadyLease,
        sidecar: codestory_retrieval::SidecarRuntimeConfig,
    }

    pub(crate) fn ready_activation_fixture() -> ReadyActivationFixture {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let storage_path = cache.path().join("codestory.db");
        fs::write(
            project.path().join("metadata.rs"),
            "// READY_LEASE_SOURCE_ANCHOR\n",
        )
        .expect("write zero-dense source fixture");
        let sidecar_cache = cache.path().join("sidecar");
        fs::create_dir_all(&sidecar_cache).expect("create sidecar cache");
        let mut sidecar = codestory_retrieval::with_test_cache_root(&sidecar_cache, || {
            codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Agent,
            )
        });
        sidecar.embedding.allow_cpu = true;
        let runtime = Runtime::new_with_config(sidecar.clone());
        runtime
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("bind ready-lease fixture");
        runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish ready-lease core");
        codestory_retrieval::test_support::publish_zero_dense_pinned_query_fixture(
            project.path(),
            &storage_path,
            &sidecar,
        )
        .expect("publish ready-lease retrieval fixture");

        let service = runtime.activation_service();
        let core_publication = service
            .retained_core_publication(&storage_path)
            .expect("read ready core")
            .expect("complete ready core");
        let source_observer = service
            .controller
            .observed_source_epoch(project.path())
            .expect("a local temporary project must be observable");
        let source_freshness = service
            .controller
            .index_freshness_uncached(FreshnessObservationPolicy::ObserveSourceRoot)
            .expect("verify ready source snapshot");
        assert!(index_freshness_admits_operation(&source_freshness));
        let retrieval = codestory_retrieval::ready_retrieval_identity_for_runtime(
            project.path(),
            &storage_path,
            &sidecar,
        )
        .expect("observe ready retrieval identity")
        .expect("ready retrieval identity");
        let lease = ReadyLease {
            configuration_id: service
                .controller
                .runtime_configuration_id()
                .expect("ready runtime configuration identity"),
            core_publication: core_publication.clone(),
            retrieval,
            source: ReadySourceIdentity::from(&source_freshness),
            source_freshness_memo: codestory_workspace::SourceFreshnessMemo::default(),
            source_observer: Some(source_observer),
        };
        assert!(lease.source.is_admissible_snapshot());
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.target = Some(ActivationTarget::new(project.path(), &storage_path));
            state.current = Some(ActivationSnapshot {
                operation_id: "activation-ready-lease-fixture".into(),
                revision: 17,
                state: ActivationState::Ready,
                stage: ActivationStage::Ready,
                progress: activation_stage_progress(ActivationStage::Ready),
                attempt: 3,
                retry_after_ms: None,
                embedding_capacity: None,
                embedding_retry: None,
                failure_code: None,
                failure: None,
                failure_details: None,
                retained_core_publication: Some(core_publication),
                capabilities: ActivationCapabilities {
                    local_navigation: ActivationCapabilityState::Ready,
                    broad_search: ActivationCapabilityState::Ready,
                },
            });
            state.ready_lease = Some(lease.clone());
            state.running = false;
            state.current_cancel = None;
        }

        ReadyActivationFixture {
            project,
            _cache: cache,
            runtime,
            storage_path,
            lease,
            sidecar,
        }
    }

    fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        fn visit(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
            let mut children = fs::read_dir(path)
                .expect("read snapshot directory")
                .map(|entry| entry.expect("read snapshot entry").path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                let relative = child
                    .strip_prefix(root)
                    .expect("snapshot path stays below root")
                    .to_path_buf();
                if child.is_dir() {
                    entries.push((relative, None));
                    visit(root, &child, entries);
                } else {
                    entries.push((
                        relative,
                        Some(fs::read(&child).expect("read snapshot file")),
                    ));
                }
            }
        }

        let mut entries = Vec::new();
        visit(root, root, &mut entries);
        entries
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CoordinatorSnapshot {
        target: Option<(String, String, PathBuf)>,
        current: Option<ActivationSnapshot>,
        ready_lease: Option<ReadyLease>,
        running: bool,
        has_cancel: bool,
    }

    fn coordinator_snapshot(service: &ActivationService) -> CoordinatorSnapshot {
        let state = service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator");
        CoordinatorSnapshot {
            target: state.target.as_ref().map(|target| {
                (
                    target.project_id.clone(),
                    target.workspace_id.clone(),
                    target.storage_path.clone(),
                )
            }),
            current: state.current.clone(),
            ready_lease: state.ready_lease.clone(),
            running: state.running,
            has_cancel: state.current_cancel.is_some(),
        }
    }

    /// Status and doctor must report hostile lease states without trying to
    /// improve any of them. The full cache snapshot catches SQLite, sidecar,
    /// and generation writes; the counters catch activation, refresh, and
    /// observer arming even when those attempts leave no durable residue.
    #[test]
    fn status_and_doctor_report_hostile_lease_evidence_without_mutation() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        let cache_root = fixture
            .storage_path
            .parent()
            .expect("fixture cache root")
            .to_path_buf();

        let assert_observational = |label: &str,
                                    expected: crate::activation_status::ReadyLeaseEvidence,
                                    expected_mode: &str| {
            let state_before = coordinator_snapshot(&service);
            let files_before = tree_snapshot(&cache_root);
            let observer_requests_before = service.controller.source_observer_requests_for_test();
            let workers_before = service.worker_start_count_for_test();
            let preparation_before = service.preparation_counts_for_test();

            let status = service
                .retrieval_status(fixture.project.path(), &fixture.storage_path)
                .unwrap_or_else(|error| panic!("{label} status failed: {error}"));
            assert_eq!(status.ready_lease(), &expected, "{label} status golden");
            assert_eq!(
                status.report().retrieval_mode,
                expected_mode,
                "{label} status mode"
            );

            let doctor = service
                .retrieval_engine_diagnostics(fixture.project.path(), &fixture.storage_path)
                .unwrap_or_else(|error| panic!("{label} doctor failed: {error}"));
            assert_eq!(doctor.ready_lease, expected, "{label} doctor golden");
            assert_eq!(doctor.retrieval_mode, expected_mode, "{label} doctor mode");

            assert_eq!(
                coordinator_snapshot(&service),
                state_before,
                "{label} status/doctor changed or extended the ready lease"
            );
            assert_eq!(
                service.controller.source_observer_requests_for_test(),
                observer_requests_before,
                "{label} status/doctor armed or requested a source observer"
            );
            assert_eq!(
                service.worker_start_count_for_test(),
                workers_before,
                "{label} status/doctor started activation or refresh work"
            );
            assert_eq!(
                service.preparation_counts_for_test(),
                preparation_before,
                "{label} status/doctor ran sidecar preparation"
            );
            assert_eq!(
                codestory_workspace::source_freshness_counts(),
                None,
                "{label} status/doctor armed a source scan scope"
            );
            assert_eq!(
                tree_snapshot(&cache_root),
                files_before,
                "{label} status/doctor wrote cache, SQLite, or sidecar state"
            );
        };

        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.ready_lease = None;
        }
        assert_observational(
            "no lease",
            crate::activation_status::ReadyLeaseEvidence::default(),
            "full",
        );

        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            let mut stale = fixture.lease.clone();
            stale.source.status = IndexFreshnessStatusDto::Stale;
            state.ready_lease = Some(stale);
        }
        assert_observational(
            "stale lease",
            crate::activation_status::ReadyLeaseEvidence {
                ready_lease_present: true,
                ready_lease_admission_basis: "inadmissible_source_observation".to_string(),
                ready_lease_observer_epoch_coherence: "coherent".to_string(),
                ready_lease_memo_holds_observations: true,
            },
            "full",
        );

        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            let mut unproven = fixture.lease.clone();
            unproven.source_observer = None;
            state.ready_lease = Some(unproven);
        }
        assert_observational(
            "unproven observer",
            crate::activation_status::ReadyLeaseEvidence {
                ready_lease_present: true,
                ready_lease_admission_basis: "complete_source_observation".to_string(),
                ready_lease_observer_epoch_coherence: "unproven".to_string(),
                ready_lease_memo_holds_observations: true,
            },
            "full",
        );

        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.ready_lease = Some(fixture.lease.clone());
        }
        assert!(
            service
                .ready_lease_evidence(fixture.project.path(), &fixture.storage_path)
                .ready_lease_present,
            "the restored lease must match before publication removal"
        );
        fs::remove_file(&fixture.storage_path).expect("remove publication for hostile state");
        assert_observational(
            "missing publication",
            crate::activation_status::ReadyLeaseEvidence {
                ready_lease_present: true,
                ready_lease_admission_basis: "complete_source_observation".to_string(),
                ready_lease_observer_epoch_coherence: "coherent".to_string(),
                ready_lease_memo_holds_observations: true,
            },
            "unavailable",
        );
    }

    #[test]
    fn status_and_doctor_do_not_resurrect_a_lease_after_observing_one() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();

        assert!(
            service
                .retrieval_status(fixture.project.path(), &fixture.storage_path)
                .expect("status with a ready lease")
                .ready_lease()
                .ready_lease_present,
            "the observational matrix must first see the lease it is about to remove"
        );
        assert!(
            service
                .retrieval_engine_diagnostics(fixture.project.path(), &fixture.storage_path)
                .expect("doctor with a ready lease")
                .ready_lease
                .ready_lease_present,
            "doctor must see the same initial lease"
        );

        service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator")
            .ready_lease = None;

        assert!(
            !service
                .retrieval_status(fixture.project.path(), &fixture.storage_path)
                .expect("status after lease removal")
                .ready_lease()
                .ready_lease_present,
            "status must report no lease after the coordinator drops it"
        );
        assert!(
            !service
                .retrieval_engine_diagnostics(fixture.project.path(), &fixture.storage_path)
                .expect("doctor after lease removal")
                .ready_lease
                .ready_lease_present,
            "doctor must not resurrect a previously observed lease"
        );
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
    fn unchanged_ready_lease_returns_without_starting_or_advancing_activation() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        let before = service.snapshot().expect("ready snapshot");

        let reused = service
            .activate_project(
                fixture.project.path(),
                &fixture.storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect("unchanged ready lease must be reused");

        assert!(!reused.joined);
        assert_eq!(reused.snapshot, before);
        assert_eq!(service.worker_start_count_for_test(), 0);
        let state = service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator");
        assert!(!state.running);
        assert_eq!(state.ready_lease.as_ref(), Some(&fixture.lease));
    }

    /// One public operation derives source freshness before and after the
    /// build, and the MCP transport wraps the same request in a second public
    /// operation. The pre-build derivations all ask about the same instant, so
    /// they share one content pass; every post-build derivation asks whether
    /// the source moved *since*, so it re-reads content. Four derivations
    /// therefore cost three passes over the indexed files, not four and not
    /// one.
    #[test]
    fn a_warm_public_operation_shares_one_pre_build_content_pass() {
        let fixture = ready_activation_fixture();
        let indexed_files = fixture
            .runtime
            .activation_service()
            .controller
            .index_freshness_uncached(FreshnessObservationPolicy::Unobserved)
            .expect("observe indexed inventory")
            .indexed_file_count;
        assert!(
            indexed_files > 0,
            "the fixture must publish at least one indexed file"
        );

        let service = fixture.runtime.public_operation_service();
        let mut observed = None;
        let mut observed_telemetry = None;
        service
            .run_with_cancel("ground", Arc::new(AtomicBool::new(false)), || {
                fixture.runtime.public_operation_service().run_with_cancel(
                    "ground",
                    Arc::new(AtomicBool::new(false)),
                    || Ok(()),
                )?;
                observed = codestory_workspace::source_freshness_counts();
                observed_telemetry = crate::source_freshness_telemetry_for_operation();
                Ok(())
            })
            .expect("warm public operation");

        // Sampled where a response is assembled, so three of the operation's
        // four freshness derivations have run: the outer pre-build check, the
        // nested operation's pre-build check, and the nested operation's
        // post-build check. The outer post-build check runs after the response
        // is built.
        let counts = observed.expect("a public operation arms the source freshness scope");
        assert_eq!(
            counts.content_hash_reads,
            u64::from(indexed_files) * 2,
            "the two pre-build derivations share one pass; the post-build check \
             re-reads content because it must see drift the pre-build pass could \
             not have seen"
        );
        assert_eq!(
            counts.verdict_reuses,
            u64::from(indexed_files),
            "the nested pre-build derivation must reuse the outer pre-build pass"
        );
        let telemetry = observed_telemetry.expect("the operation publishes its pass counters");
        assert_eq!(telemetry.content_hash_reads, indexed_files * 2);
        assert_eq!(telemetry.verdict_reuses, indexed_files);
    }

    /// Issue #1700 requires the operation-scoped freshness memo to leave
    /// same-mtime drift detection intact. The post-build "source inputs changed
    /// while running {operation}" refusal is the only mechanism that sees a
    /// mutation preserving both mtime and byte length — metadata alone cannot —
    /// so the memo must never answer it. Coarse-mtime filesystems, a `git
    /// checkout` restoring a same-length variant inside one mtime tick, and any
    /// mtime-preserving tool all produce exactly this shape.
    #[test]
    fn a_mid_build_same_mtime_same_length_edit_refuses_instead_of_serving() {
        let fixture = ready_activation_fixture();
        let source = fixture.project.path().join("metadata.rs");
        let original = fs::read(&source).expect("read the indexed source");
        let original_mtime = fs::metadata(&source)
            .expect("stat the indexed source")
            .modified()
            .expect("indexed source modification time");

        let mut builds = 0_usize;
        let refusal = fixture
            .runtime
            .public_operation_service()
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                builds += 1;
                let mut drifted = original.clone();
                let last_byte = drifted.len() - 2;
                drifted[last_byte] = b'X';
                assert_ne!(drifted, original, "the drift must change the file's bytes");
                fs::write(&source, &drifted).expect("apply the mid-build drift");
                fs::File::options()
                    .write(true)
                    .open(&source)
                    .expect("reopen the drifted source")
                    .set_modified(original_mtime)
                    .expect("restore the modification time");
                let observed = fs::metadata(&source).expect("stat the drifted source");
                assert_eq!(
                    observed.len(),
                    original.len() as u64,
                    "the drift must preserve the byte length to exercise the guard"
                );
                assert_eq!(
                    observed.modified().expect("drifted modification time"),
                    original_mtime,
                    "the drift must preserve the modification time to exercise the guard"
                );
                Ok(())
            })
            .expect_err("a source mutated mid-operation must not be served");

        assert_eq!(
            builds, 1,
            "the first attempt must have been admitted and entered the build, so \
             the refusal came from the post-build check rather than from pre-flight \
             admission"
        );
        assert_eq!(
            refusal.code, "project_unavailable",
            "the bounded retry re-derives freshness from content and now blocks the \
             second attempt up front: {}",
            refusal.message
        );
    }

    /// A second operation on one ready lease begins from the clean verdicts
    /// re-established by the first operation's post-build guard.
    #[test]
    fn the_next_public_operation_reuses_the_ready_lease_verdicts() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.public_operation_service();
        let mut first = None;
        service
            .run_with_cancel("ground", Arc::new(AtomicBool::new(false)), || {
                first = codestory_workspace::source_freshness_counts();
                Ok(())
            })
            .expect("first operation");
        let mut second = None;
        service
            .run_with_cancel("ground", Arc::new(AtomicBool::new(false)), || {
                second = codestory_workspace::source_freshness_counts();
                Ok(())
            })
            .expect("second operation");

        let first = first.expect("first scope");
        let second = second.expect("second scope");
        assert!(first.content_hash_reads > 0);
        assert_eq!(second.content_hash_reads, 0);
        assert_eq!(
            second.verdict_reuses, first.content_hash_reads,
            "the second operation must reuse the verdicts left by the first post-build guard"
        );
        assert_eq!(
            codestory_workspace::source_freshness_counts(),
            None,
            "no scope may outlive the operation that armed it"
        );
    }

    fn warm_packet_request() -> codestory_contracts::api::AgentPacketRequestDto {
        codestory_contracts::api::AgentPacketRequestDto {
            question: "how does the ready lease source anchor work".to_string(),
            budget: codestory_contracts::api::PacketBudgetModeDto::default(),
            task_class: None,
            probes: Vec::new(),
            extra_probes: Vec::new(),
            include_evidence: true,
            latency_budget_ms: Some(30_000),
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
        }
    }

    /// The first packet on a ready lease performs exactly one fingerprint pass.
    /// A later wrapper on that same lease reuses the opaque fingerprint, while
    /// replacing the lease memo forces exactly one new pass.
    #[test]
    fn a_warm_packet_performs_one_fingerprint_pass_per_ready_lease() {
        let fixture = ready_activation_fixture();
        let browser = fixture.runtime.browser_service();

        let flat = browser
            .packet(warm_packet_request())
            .expect("a warm packet over the ready fixture");
        let flat_telemetry = flat
            .answer
            .retrieval_trace
            .source_freshness_telemetry
            .expect("a packet built inside a public operation publishes its pass counters");
        assert_eq!(
            flat_telemetry.readiness_fingerprint_passes, 1,
            "the first strict packet on a ready lease must perform exactly one fingerprint pass"
        );
        assert!(
            flat_telemetry.content_hash_reads > 0,
            "a warm packet content-verifies its indexed source at least once"
        );

        let mut wrapped = None;
        fixture
            .runtime
            .public_operation_service()
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                wrapped = Some(browser.packet(warm_packet_request())?);
                Ok(())
            })
            .expect("the transport wrapper around a packet request");
        let wrapped_telemetry = wrapped
            .expect("wrapped packet")
            .answer
            .retrieval_trace
            .source_freshness_telemetry
            .expect("the wrapped packet publishes its pass counters too");

        assert_eq!(
            wrapped_telemetry.readiness_fingerprint_passes, 0,
            "a later operation on the same ready lease must reuse its fingerprint"
        );

        {
            let activation = fixture.runtime.activation_service();
            let mut state = activation
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state
                .ready_lease
                .as_mut()
                .expect("ready lease")
                .source_freshness_memo = codestory_workspace::SourceFreshnessMemo::default();
        }
        let next_lease = browser
            .packet(warm_packet_request())
            .expect("a warm packet on the replacement lease memo");
        assert_eq!(
            next_lease
                .answer
                .retrieval_trace
                .source_freshness_telemetry
                .expect("the replacement lease packet publishes counters")
                .readiness_fingerprint_passes,
            1,
            "a new ready lease must compute its own fingerprint"
        );
    }

    #[test]
    fn an_admission_refusal_drops_the_lease_fingerprint_memo() {
        let fixture = ready_activation_fixture();
        let browser = fixture.runtime.browser_service();
        let source = fixture.project.path().join("metadata.rs");
        let original = fs::read(&source).expect("read indexed source");

        let first = browser
            .packet(warm_packet_request())
            .expect("prime the ready lease fingerprint memo");
        assert_eq!(
            first
                .answer
                .retrieval_trace
                .source_freshness_telemetry
                .expect("priming packet telemetry")
                .readiness_fingerprint_passes,
            1,
            "the fixture must first populate the ready lease fingerprint memo"
        );

        fs::write(&source, "// ADMISSION_REFUSAL_DRIFT\n").expect("make source stale");
        let mut builds = 0;
        let refusal = fixture
            .runtime
            .public_operation_service()
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                builds += 1;
                Ok(())
            })
            .expect_err("stale source must refuse retrieval at admission");
        assert_eq!(refusal.code, "project_unavailable");
        assert_eq!(builds, 0, "a refused admission must not enter the build");
        fs::write(&source, &original).expect("restore source after the refusal");
        let _lease_scope = codestory_workspace::SourceFreshnessScope::enter_with_memo(
            fixture.lease.source_freshness_memo.clone(),
        );
        codestory_retrieval::strict_sidecar_status_for_runtime(
            fixture.project.path(),
            Some(&fixture.storage_path),
            fixture.sidecar.clone(),
        )
        .expect("readiness after the admission refusal");
        assert_eq!(
            codestory_workspace::source_freshness_counts()
                .expect("post-refusal readiness telemetry")
                .readiness_fingerprint_passes,
            1,
            "the next readiness pass must recompute after admission refused freshness"
        );
    }

    #[test]
    fn ready_lease_probe_rejects_each_bounded_identity_drift() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        assert!(
            service
                .probe_ready_lease(&fixture.storage_path, &fixture.lease)
                .admissible
        );

        let mut configuration_drift = fixture.lease.clone();
        configuration_drift.configuration_id.push_str("-changed");
        assert!(
            !service
                .probe_ready_lease(&fixture.storage_path, &configuration_drift)
                .admissible
        );

        let mut core_drift = fixture.lease.clone();
        core_drift.core_publication.generation_id = "changed-core-generation".into();
        assert!(
            !service
                .probe_ready_lease(&fixture.storage_path, &core_drift)
                .admissible
        );

        let mut manifest_drift = fixture.lease.clone();
        manifest_drift.retrieval.manifest.built_at_epoch_ms += 1;
        assert!(
            !service
                .probe_ready_lease(&fixture.storage_path, &manifest_drift)
                .admissible
        );

        let mut producer_drift = fixture.lease.clone();
        producer_drift
            .retrieval
            .producer_compatibility_identity
            .push_str("-changed");
        assert!(
            !service
                .probe_ready_lease(&fixture.storage_path, &producer_drift)
                .admissible
        );

        let synthetic_engine = codestory_retrieval::ReadyEmbeddingEngineIdentity {
            instance_id: "embedding-server-1".into(),
            load_generation: 7,
            model_load_count: 2,
            model_digest: "sha256:model-a".into(),
            ggml_build_identity: "llama.cpp-build-a".into(),
            backend: "metal".into(),
            adapter_name: "Apple M3 Max".into(),
            policy: "accelerator_required".into(),
            execution_device_names: vec!["Apple M3 Max".into()],
            execution_backend_names: vec!["Metal".into()],
            accelerator_execution_verified: true,
        };
        let mut engine_presence_drift = fixture.lease.clone();
        engine_presence_drift.retrieval.engine = Some(synthetic_engine.clone());
        assert!(
            !service
                .probe_ready_lease(&fixture.storage_path, &engine_presence_drift)
                .admissible,
            "a changed live engine identity must invalidate ready reuse"
        );

        let mut exact_engine_identity = fixture.lease.retrieval.clone();
        exact_engine_identity.engine = Some(synthetic_engine);
        assert!(ready_retrieval_identity_matches(
            Some(&exact_engine_identity),
            &exact_engine_identity,
        ));
        macro_rules! reject_engine_field_drift {
            ($field:ident, $changed:expr) => {{
                let mut observed = exact_engine_identity.clone();
                observed
                    .engine
                    .as_mut()
                    .expect("synthetic engine identity")
                    .$field = $changed;
                assert!(
                    !ready_retrieval_identity_matches(Some(&observed), &exact_engine_identity),
                    concat!(
                        "ready retrieval equality omitted engine field `",
                        stringify!($field),
                        "`"
                    )
                );
            }};
        }
        reject_engine_field_drift!(instance_id, "embedding-server-2".into());
        reject_engine_field_drift!(load_generation, 8);
        reject_engine_field_drift!(model_load_count, 3);
        reject_engine_field_drift!(model_digest, "sha256:model-b".into());
        reject_engine_field_drift!(ggml_build_identity, "llama.cpp-build-b".into());
        reject_engine_field_drift!(backend, "cuda".into());
        reject_engine_field_drift!(adapter_name, "NVIDIA RTX".into());
        reject_engine_field_drift!(policy, "cpu_explicit".into());
        reject_engine_field_drift!(execution_device_names, vec!["NVIDIA RTX".into()]);
        reject_engine_field_drift!(execution_backend_names, vec!["CUDA".into()]);
        reject_engine_field_drift!(accelerator_execution_verified, false);

        let mut invalid_source_snapshot = fixture.lease.clone();
        invalid_source_snapshot.source.status = IndexFreshnessStatusDto::Stale;
        assert!(
            !service
                .probe_ready_lease(&fixture.storage_path, &invalid_source_snapshot)
                .admissible
        );
    }

    /// An observer that escalates every window it seals, moving nothing on disk.
    ///
    /// The event is directory-scoped, so it names no file to rehash: the scan verdict stays
    /// `Fresh` on its own and only the escalation can refuse. Anything these tests turn red is
    /// therefore the consequence of the escalation and not the arming.
    fn install_escalating_observer(controller: &AppController) -> PathBuf {
        let root = controller
            .require_project_root()
            .expect("the fixture binds a project root");
        let scoped_root = root.clone();
        let session = crate::tests::freshness_observer_tests::scripted_session(&root, move |_| {
            vec![
                codestory_workspace::filesystem_observer::ObservedFilesystemEvent::Mutated {
                    path: scoped_root.clone(),
                    scope: codestory_workspace::filesystem_observer::MutationScope::Directory,
                },
            ]
        });
        controller.install_source_observer_for_test(&root, Arc::new(session));
        root
    }

    #[test]
    fn an_escalated_verdict_refuses_ready_lease_publication() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        service.use_published_retrieval_fixture_for_test();
        let project_root = install_escalating_observer(&service.controller);
        let operation = ActivationOperation {
            service: service.clone(),
            operation_id: "activation-observed-source-drift".to_string(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };

        let error = service
            .activate_once(&operation, project_root, fixture.storage_path.clone())
            .expect_err("a source tree that moved under the scan must not be leased as ready");

        assert_eq!(
            error.code, "publication_changed",
            "ready-lease validation is a serving read; an observed race has to refuse it"
        );
        let state = service
            .coordinator
            .state
            .lock()
            .expect("activation coordinator");
        assert_eq!(
            state.ready_lease.as_ref(),
            Some(&fixture.lease),
            "a refused validation must not replace the lease it refused to renew"
        );
    }

    #[test]
    fn a_source_mutation_after_the_lease_was_minted_refuses_ready_reuse() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        let root = service
            .controller
            .require_project_root()
            .expect("the fixture binds a project root");
        let quiet = Arc::new(AtomicBool::new(true));
        let gate = Arc::clone(&quiet);
        let written = root.join("metadata.rs");
        let session = crate::tests::freshness_observer_tests::scripted_session(&root, move |_| {
            if gate.load(Ordering::Acquire) {
                return Vec::new();
            }
            vec![
                codestory_workspace::filesystem_observer::ObservedFilesystemEvent::Mutated {
                    path: written.clone(),
                    scope: codestory_workspace::filesystem_observer::MutationScope::File,
                },
            ]
        });
        service
            .controller
            .install_source_observer_for_test(&root, Arc::new(session));

        let mut lease = fixture.lease.clone();
        lease.source_observer = service.controller.observed_source_epoch(&root);
        assert!(
            lease.source_observer.is_some(),
            "an armed session must be able to stamp the lease it authorises"
        );
        assert!(
            service
                .probe_ready_lease(&fixture.storage_path, &lease)
                .admissible,
            "a still working tree keeps the lease it earned"
        );

        // The write EV-78 could not see: the database does not move, so every other identity in
        // the lease still matches and the probe never re-scans.
        quiet.store(false, Ordering::Release);
        assert!(
            !service
                .probe_ready_lease(&fixture.storage_path, &lease)
                .admissible,
            "a source write after the lease was minted must end the reuse window"
        );
    }

    #[test]
    fn a_lease_minted_without_an_observer_keeps_the_unobserved_floor() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        let mut unobservable = fixture.lease.clone();
        unobservable.source_observer = None;
        assert!(
            service
                .probe_ready_lease(&fixture.storage_path, &unobservable)
                .admissible,
            "a host the observer cannot watch keeps exactly the EV-7 answer it had before"
        );
    }

    #[test]
    fn ready_lease_revalidation_rejects_manifest_change_after_initial_capture() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        Store::open(&fixture.storage_path)
            .expect("open fixture storage")
            .get_connection()
            .execute(
                "UPDATE retrieval_index_manifest \
                 SET built_at_epoch_ms = built_at_epoch_ms + 1",
                [],
            )
            .expect("mutate retrieval pointer after initial capture");

        let error = service
            .require_ready_retrieval_identity_unchanged(
                fixture.project.path(),
                &fixture.storage_path,
                &fixture.lease.retrieval,
            )
            .expect_err("changed retrieval pointer must reject ready-lease publication");

        assert_eq!(error.code, "publication_changed");
    }

    #[test]
    fn concurrent_missing_pointer_callers_start_one_fail_closed_replacement() {
        let fixture = ready_activation_fixture();
        let service = fixture.runtime.activation_service();
        let ready = service.snapshot().expect("ready snapshot");
        Store::open(&fixture.storage_path)
            .expect("open fixture storage")
            .get_connection()
            .execute("DELETE FROM retrieval_index_manifest", [])
            .expect("remove retrieval identity pointer");

        let worker_gate = Arc::new((Mutex::new(false), Condvar::new()));
        service.set_worker_start_gate_for_test(Some(Arc::clone(&worker_gate)));
        let caller_gate = Arc::new(std::sync::Barrier::new(7));
        let callers = (0..6)
            .map(|_| {
                let service = service.clone();
                let project_root = fixture.project.path().to_path_buf();
                let storage_path = fixture.storage_path.clone();
                let caller_gate = Arc::clone(&caller_gate);
                std::thread::spawn(move || {
                    caller_gate.wait();
                    let error = service
                        .activate_project_with_foreground_budget(
                            &project_root,
                            &storage_path,
                            Arc::new(AtomicBool::new(false)),
                            Duration::ZERO,
                        )
                        .expect_err("missing pointer must start replacement");
                    (error, service.snapshot().expect("replacement snapshot"))
                })
            })
            .collect::<Vec<_>>();
        caller_gate.wait();
        let observations = callers
            .into_iter()
            .map(|caller| caller.join().expect("join replacement caller"))
            .collect::<Vec<_>>();

        assert!(observations.iter().all(|(error, snapshot)| {
            error.code == "activation_preparing"
                && snapshot.operation_id == ready.operation_id
                && snapshot.attempt == ready.attempt + 1
                && snapshot.capabilities.broad_search == ActivationCapabilityState::Unavailable
        }));
        assert!(
            observations
                .iter()
                .all(|(_, snapshot)| snapshot.revision > ready.revision),
            "replacement must expose phase/progress revision evidence"
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        while service.worker_start_count_for_test() == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(service.worker_start_count_for_test(), 1);
        let replacement = service.snapshot().expect("replacement snapshot");
        assert_eq!(replacement.stage, ActivationStage::Discovery);
        assert_eq!(
            replacement.progress,
            activation_stage_progress(ActivationStage::Discovery)
        );

        service.set_worker_start_gate_for_test(None);
        let (released, changed) = worker_gate.as_ref();
        *released
            .lock()
            .expect("activation worker test gate poisoned") = true;
        changed.notify_all();
        service.cancel_and_wait();
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
    fn activation_target_reobserves_same_root_remote_change_and_no_remote_reinit() {
        let project = tempfile::tempdir().expect("project");
        initialize_identifiable_git_project(project.path());
        let storage = project.path().join("cache").join("codestory.db");
        let service = Runtime::new().activation_service();
        let remote_a = ActivationTarget::new(project.path(), &storage);
        let snapshot = ActivationSnapshot {
            operation_id: "activation-logical-target-fixture".into(),
            revision: 1,
            state: ActivationState::Ready,
            stage: ActivationStage::Ready,
            progress: activation_stage_progress(ActivationStage::Ready),
            attempt: 1,
            retry_after_ms: None,
            embedding_capacity: None,
            embedding_retry: None,
            failure_code: None,
            failure: None,
            failure_details: None,
            retained_core_publication: None,
            capabilities: ActivationCapabilities {
                local_navigation: ActivationCapabilityState::Ready,
                broad_search: ActivationCapabilityState::Ready,
            },
        };
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.target = Some(remote_a.clone());
            state.current = Some(snapshot.clone());
        }
        assert_eq!(
            service.snapshot_for_target(project.path(), &storage),
            Some(snapshot.clone())
        );

        git(
            project.path(),
            &[
                "remote",
                "set-url",
                "origin",
                "https://example.com/codestory/other-fixture.git",
            ],
        );
        let remote_b = ActivationTarget::new(project.path(), &storage);
        assert_eq!(remote_a.workspace_id, remote_b.workspace_id);
        assert_ne!(remote_a.project_id, remote_b.project_id);
        assert_eq!(remote_a.repository_instance, remote_b.repository_instance);
        assert!(!remote_a.matches(&remote_b));
        assert!(
            service
                .snapshot_for_target(project.path(), &storage)
                .is_none()
        );
        assert_eq!(
            service
                .target_for_request(project.path(), &storage)
                .project_id,
            remote_b.project_id
        );
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.target = Some(remote_b.clone());
            state.current = Some(snapshot);
        }

        fs::rename(
            project.path().join(".git"),
            project.path().join(".git-retired"),
        )
        .expect("retire original repository metadata");
        git(project.path(), &["init", "-q"]);
        let no_remote = ActivationTarget::new(project.path(), &storage);
        assert_eq!(remote_b.workspace_id, no_remote.workspace_id);
        assert_eq!(no_remote.project_id, no_remote.workspace_id);
        assert_ne!(remote_b.repository_instance, no_remote.repository_instance);
        assert!(!remote_b.matches(&no_remote));
        assert!(
            service
                .snapshot_for_target(project.path(), &storage)
                .is_none()
        );
        assert_eq!(
            service
                .target_for_request(project.path(), &storage)
                .project_id,
            no_remote.project_id
        );
    }

    #[test]
    fn activation_target_rejects_same_remote_metadata_recreation() {
        let project = tempfile::tempdir().expect("project");
        initialize_identifiable_git_project(project.path());
        let storage = project.path().join("cache").join("codestory.db");
        let original = ActivationTarget::new(project.path(), &storage);

        fs::rename(
            project.path().join(".git"),
            project.path().join(".git-retired"),
        )
        .expect("retire original repository metadata");
        git(project.path(), &["init", "-q"]);
        git(
            project.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/codestory/fixture.git",
            ],
        );
        let recreated = ActivationTarget::new(project.path(), &storage);

        assert_eq!(original.project_id, recreated.project_id);
        assert_eq!(original.workspace_id, recreated.workspace_id);
        assert_ne!(original.repository_instance, recreated.repository_instance);
        assert!(!original.matches(&recreated));
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
    fn bounded_cancel_returns_on_a_running_activation_it_cannot_stop() {
        let service = Runtime::new().activation_service();
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            // An activation that will not notice cancellation: the recovery
            // path must not be held by it.
            state.running = true;
            state.current_cancel = Some(Arc::clone(&cancelled));
        }

        // The wait runs off the test thread so a wait that never returns fails
        // the assertion instead of hanging the suite.
        let waiting = service.clone();
        let (report, waited_for) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let stopped = waiting.cancel_and_wait_timeout(Duration::from_millis(50));
            let _ = report.send(stopped);
        });
        let stopped = waited_for
            .recv_timeout(Duration::from_secs(10))
            .expect("the wait must end at its own deadline, not when the activation does");

        assert!(
            !stopped,
            "an activation still running at the deadline must be reported as still running"
        );
        assert!(
            cancelled.load(Ordering::Acquire),
            "the bounded wait must still ask the activation to stop"
        );

        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.running = false;
            state.current_cancel = None;
        }
        service.coordinator.changed.notify_all();
        assert!(
            service.cancel_and_wait_timeout(Duration::from_millis(50)),
            "a coordinator with nothing running reports the activation stopped"
        );
    }

    #[test]
    fn bounded_cancel_reads_through_a_poisoned_coordinator() {
        let service = Runtime::new().activation_service();
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.current_cancel = Some(Arc::clone(&cancelled));
        }
        // A request that panics while holding the coordinator leaves it
        // poisoned; panic recovery runs straight into that state.
        let poisoning = service.clone();
        std::thread::spawn(move || {
            let _state = poisoning.coordinator.state.lock().expect("coordinator");
            panic!("poison the activation coordinator");
        })
        .join()
        .expect_err("the poisoning thread must panic");
        assert!(service.coordinator.state.is_poisoned());

        assert!(
            service.cancel_and_wait_timeout(Duration::from_millis(50)),
            "a poisoned coordinator must not turn recovery into a second panic"
        );
        assert!(
            cancelled.load(Ordering::Acquire),
            "the cancellation flag stays meaningful across an unrelated unwind"
        );
    }

    #[test]
    fn serial_retries_keep_one_activation_identity_after_terminal_failure() {
        let project = tempfile::tempdir().expect("project");
        let missing = project.path().join("missing");
        let storage_path = project.path().join("cache").join("codestory.db");
        let service = Runtime::new().activation_service();

        let first = service
            .activate_project(&missing, &storage_path, Arc::new(AtomicBool::new(false)))
            .expect_err("missing project must fail activation");
        assert_eq!(first.code, "project_unavailable");
        let first_snapshot = service.snapshot().expect("first terminal snapshot");

        let second = service
            .activate_project(&missing, &storage_path, Arc::new(AtomicBool::new(false)))
            .expect_err("same missing project must fail activation again");
        assert_eq!(second.code, "project_unavailable");
        let second_snapshot = service.snapshot().expect("second terminal snapshot");

        assert_eq!(
            second_snapshot.operation_id, first_snapshot.operation_id,
            "serial retries for one project must retain one activation identity"
        );
        assert_eq!(second_snapshot.attempt, first_snapshot.attempt + 1);
        assert!(second_snapshot.revision > first_snapshot.revision);
        assert!(second_snapshot.stage >= first_snapshot.stage);
        assert!(second_snapshot.progress >= first_snapshot.progress);
    }

    #[test]
    fn cancelling_a_waiter_does_not_cancel_or_replace_shared_activation() {
        let project = tempfile::tempdir().expect("project");
        let storage_path = project.path().join("cache").join("codestory.db");
        fs::write(
            project.path().join("fixture.rs"),
            "pub fn shared_activation_fixture() {}\n",
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
            .expect_err("zero foreground budget must return shared progress");
        assert_eq!(first.code, "activation_preparing");
        let before = service.snapshot().expect("shared activation snapshot");

        let target = ActivationTarget::new(project.path(), &storage_path);
        let cancelled = service
            .wait_for_activation(
                &target,
                &before.operation_id,
                true,
                &AtomicBool::new(true),
                Duration::ZERO,
            )
            .expect_err("the cancelled waiter must return without joining");
        assert_eq!(cancelled.code, "cancelled");
        let after = service
            .snapshot()
            .expect("shared activation survives waiter");

        assert_eq!(after.operation_id, before.operation_id);
        assert_ne!(after.state, ActivationState::Cancelled);
        service.cancel_and_wait();
    }

    #[test]
    fn panicking_activation_worker_finishes_waiters_and_allows_retry() {
        let project = tempfile::tempdir().expect("project");
        let missing = project.path().join("missing");
        let storage_path = project.path().join("cache").join("codestory.db");
        let service = Runtime::new().activation_service();
        let target = ActivationTarget::new(&missing, &storage_path);
        let operation_id = "activation-panic-fixture".to_string();
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.target = Some(target.clone());
            state.current = Some(ActivationSnapshot {
                operation_id: operation_id.clone(),
                revision: 1,
                state: ActivationState::Preparing,
                stage: ActivationStage::Discovery,
                progress: activation_stage_progress(ActivationStage::Discovery),
                attempt: 1,
                retry_after_ms: Some(250),
                embedding_capacity: None,
                embedding_retry: None,
                failure_code: None,
                failure: None,
                failure_details: None,
                retained_core_publication: None,
                capabilities: ActivationCapabilities {
                    local_navigation: ActivationCapabilityState::Unavailable,
                    broad_search: ActivationCapabilityState::Unavailable,
                },
            });
            state.running = true;
            state.current_cancel = Some(Arc::clone(&cancelled));
        }
        let operation = ActivationOperation {
            service: service.clone(),
            operation_id: operation_id.clone(),
            cancelled,
        };

        run_activation_worker(&operation, || panic!("activation panic fixture"));

        let terminal_error = service
            .wait_for_activation(
                &target,
                &operation_id,
                false,
                &AtomicBool::new(false),
                Duration::from_secs(1),
            )
            .expect_err("worker panic must become a terminal activation error");
        assert_eq!(terminal_error.code, "project_unavailable");
        let terminal = service.snapshot().expect("terminal snapshot");
        assert_eq!(terminal.state, ActivationState::Unavailable);
        assert_eq!(
            terminal.failure.as_deref(),
            Some("project activation worker stopped unexpectedly")
        );

        service.cancel_and_wait();
        let retry = service
            .activate_project(&missing, &storage_path, Arc::new(AtomicBool::new(false)))
            .expect_err("the missing project still fails, without a wedged coordinator");
        assert_eq!(retry.code, "project_unavailable");
        let retried = service.snapshot().expect("retried terminal snapshot");
        assert_eq!(retried.operation_id, operation_id);
        assert_eq!(retried.attempt, 2);
    }

    #[test]
    fn failed_replacement_retains_exact_local_core_without_broad_capability() {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        let storage_path = cache.path().join("codestory.db");
        fs::create_dir_all(project.path().join("src")).expect("create source directory");
        fs::write(
            project.path().join("src/lib.rs"),
            "pub fn retained_fixture() {}\n",
        )
        .expect("write fixture");

        let seeding_runtime = Runtime::new();
        seeding_runtime
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open seed project");
        seeding_runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("publish retained core");
        let retained = Store::database_complete_index_publication(&storage_path)
            .expect("read retained publication")
            .expect("complete retained publication");

        fs::write(
            project.path().join("codestory_workspace.json"),
            r#"{"members":["src","missing"]}"#,
        )
        .expect("write incomplete synthetic workspace");
        let runtime = Runtime::new();
        assert_eq!(
            runtime
                .activation_service()
                .retained_core_publication(&storage_path)
                .expect("inspect retained core before activation")
                .as_ref(),
            Some(&crate::index_publication_dto(retained.clone()))
        );
        let error = runtime
            .activation_service()
            .activate_project(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect_err("incomplete replacement inventory must fail closed");
        assert_eq!(error.code, "source_discovery_incomplete");

        let snapshot = runtime
            .activation_service()
            .snapshot()
            .expect("failed replacement snapshot");
        assert_eq!(
            snapshot.capabilities.local_navigation,
            ActivationCapabilityState::Retained
        );
        assert_eq!(
            snapshot.retained_core_publication.as_ref(),
            Some(&crate::index_publication_dto(retained.clone()))
        );
        assert_eq!(
            snapshot.capabilities.broad_search,
            ActivationCapabilityState::Unavailable
        );
        assert!(!snapshot.allows_operation("packet"));

        runtime
            .activation_service()
            .ensure_complete_core_for_observation(
                project.path(),
                &storage_path,
                Arc::new(AtomicBool::new(false)),
            )
            .expect("retained complete core remains admissible for affected analysis");
        runtime
            .public_operation_service()
            .run_observational_with_cancel("affected", Arc::new(AtomicBool::new(false)), || Ok(()))
            .expect("affected analysis can pin the exact retained core");
        runtime
            .public_operation_service()
            .run_with_cancel("ground", Arc::new(AtomicBool::new(false)), || Ok(()))
            .expect("local grounding can pin the exact retained core");
        let mut entered_broad_response = false;
        let broad = runtime
            .public_operation_service()
            .run_with_cancel("packet", Arc::new(AtomicBool::new(false)), || {
                entered_broad_response = true;
                Ok(())
            })
            .expect_err("broad search must remain fail-closed on a retained core");
        assert_eq!(broad.code, "project_unavailable");
        assert!(!entered_broad_response);

        fs::remove_file(project.path().join("codestory_workspace.json"))
            .expect("remove incomplete workspace");
        fs::write(
            project.path().join("src/lib.rs"),
            "pub fn retained_fixture_after_publication() {}\n",
        )
        .expect("change source for replacement publication");
        seeding_runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .expect("advance complete core publication");
        let advanced = Store::database_complete_index_publication(&storage_path)
            .expect("read advanced publication")
            .expect("complete advanced publication");
        assert_ne!(advanced, retained);
        fs::write(
            project.path().join("codestory_workspace.json"),
            r#"{"members":["src","missing"]}"#,
        )
        .expect("restore incomplete workspace");

        let stale_identity = runtime
            .public_operation_service()
            .run_with_cancel("ground", Arc::new(AtomicBool::new(false)), || Ok(()))
            .expect_err("the old retained identity must not admit a newer live core");
        assert_eq!(stale_identity.code, "project_unavailable");

        ActivationOperation {
            service: runtime.activation_service(),
            operation_id: snapshot.operation_id,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
        .set_retained_local_publication(crate::index_publication_dto(advanced.clone()));
        let rebound = runtime
            .activation_service()
            .snapshot()
            .expect("rebound retained snapshot");
        assert_eq!(
            rebound.retained_core_publication.as_ref(),
            Some(&crate::index_publication_dto(advanced))
        );
        runtime
            .public_operation_service()
            .run_with_cancel("ground", Arc::new(AtomicBool::new(false)), || Ok(()))
            .expect("ground admits the exact post-publication retained core");
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
        let search_path = search_index_path_for_publication(&storage_path, Some(&publication))
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
            read_search_generation_completion(&search_path, &publication.generation_id).is_some(),
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
        let previous_search = search_index_path_for_publication(&storage_path, Some(&previous))
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
        let current_search = search_index_path_for_publication(&storage_path, Some(&current))
            .expect("repaired search generation path");
        assert!(
            read_search_generation_completion(&current_search, &current.generation_id).is_some(),
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
            revision: 3,
            state: ActivationState::Unavailable,
            stage: ActivationStage::CoreFreshness,
            progress: activation_stage_progress(ActivationStage::CoreFreshness),
            attempt: 1,
            retry_after_ms: None,
            embedding_capacity: None,
            embedding_retry: None,
            failure_code: Some("source_oversized".to_string()),
            failure: Some("source exceeds the indexing limit".to_string()),
            failure_details: Some(Box::new(ApiErrorDetails::source_coverage(vec![
                diagnostic.clone(),
            ]))),
            retained_core_publication: None,
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
            revision: 7,
            state: ActivationState::Unavailable,
            stage: ActivationStage::Validation,
            progress: activation_stage_progress(ActivationStage::Validation),
            attempt: 1,
            retry_after_ms: None,
            embedding_capacity: None,
            embedding_retry: None,
            failure_code: Some("project_unavailable".into()),
            failure: Some("embedding backend unavailable".into()),
            failure_details: None,
            retained_core_publication: None,
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

#[cfg(test)]
mod bounded_runtime_tests {
    use super::*;
    use crate::Runtime;
    use std::fs;

    struct BoundedRuntimeFixture {
        project: tempfile::TempDir,
        _cache: tempfile::TempDir,
        runtime: Runtime,
    }

    /// A runtime with no published core: every test here decides before any
    /// snapshot work, so publishing one would only slow the proof down.
    fn bounded_runtime_fixture() -> BoundedRuntimeFixture {
        let project = tempfile::tempdir().expect("project");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(project.path().join("anchor.rs"), "// BOUNDED_ANCHOR\n")
            .expect("write source anchor");
        let sidecar_cache = cache.path().join("sidecar");
        fs::create_dir_all(&sidecar_cache).expect("create sidecar cache");
        let mut sidecar = codestory_retrieval::with_test_cache_root(&sidecar_cache, || {
            codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Agent,
            )
        });
        sidecar.embedding.allow_cpu = true;
        let runtime = Runtime::new_with_config(sidecar);
        BoundedRuntimeFixture {
            project,
            _cache: cache,
            runtime,
        }
    }

    fn preparing_snapshot(operation_id: &str) -> ActivationSnapshot {
        ActivationSnapshot {
            operation_id: operation_id.to_string(),
            revision: 1,
            state: ActivationState::Preparing,
            stage: ActivationStage::Publication,
            progress: activation_stage_progress(ActivationStage::Publication),
            attempt: 1,
            retry_after_ms: Some(250),
            embedding_capacity: None,
            embedding_retry: None,
            failure_code: None,
            failure: None,
            failure_details: None,
            retained_core_publication: None,
            capabilities: ActivationCapabilities {
                local_navigation: ActivationCapabilityState::Unavailable,
                broad_search: ActivationCapabilityState::Unavailable,
            },
        }
    }

    /// The invariant the fail-stop budget rests on: the longest a worker can be
    /// stuck without observing cancellation must stay under
    /// `ACTIVATION_QUIESCENCE_BUDGET`.
    ///
    /// A peer holding a retention lock for a whole publication is ordinary, not
    /// a fault, and the waiter's own budget for it is
    /// `PUBLICATION_LOCK_WAIT` — six times the quiescence budget. Unless the
    /// wait ends on cancellation, an eviction or a shutdown during a slow first
    /// activation joins a worker that is merely waiting, calls it unquiesced,
    /// and aborts the process out from under a healthy session.
    ///
    /// `GenerationRetentionLock::acquire` is a real production call site that
    /// passes no cancellation flag of its own, exactly like the promotion,
    /// search index, generation catalog, and model materialization waits the
    /// worker also reaches. It must inherit the worker's.
    #[test]
    fn a_worker_waiting_on_a_held_publication_lock_quiesces_on_cancellation() {
        let fixture = bounded_runtime_fixture();
        let service = fixture.runtime.activation_service();
        let state_file = fixture.project.path().join("retrieval-sidecars.json");
        let holder = codestory_retrieval::GenerationRetentionLock::acquire(
            &state_file,
            "quiescence_contention",
        )
        .expect("a peer takes the retention lock for its publication pass");

        let cancelled = Arc::new(AtomicBool::new(false));
        let operation_id = "activation-quiescence-contention".to_string();
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.current = Some(preparing_snapshot(&operation_id));
            state.running = true;
            state.current_cancel = Some(Arc::clone(&cancelled));
        }
        let operation = ActivationOperation {
            service: service.clone(),
            operation_id,
            cancelled: Arc::clone(&cancelled),
        };
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let waited_state_file = state_file.clone();
        let worker = std::thread::spawn(move || {
            run_activation_worker(&operation, || {
                entered
                    .send(())
                    .expect("announce that the worker is running");
                // Either outcome ends the body; the worker always reports a
                // failure so completion takes the error path rather than the
                // ready-lease one this fixture never publishes.
                match codestory_retrieval::GenerationRetentionLock::acquire(
                    &waited_state_file,
                    "quiescence_contention",
                ) {
                    Ok(_) => Err(ApiError::new(
                        "activation_retryable",
                        "the wait outlived cancellation and ended by acquiring the lock",
                    )),
                    Err(error) => Err(ApiError::new("cancelled", error.to_string())),
                }
            });
        });
        entered_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("worker entered its body");

        let started = Instant::now();
        let quiescence = service.cancel_and_wait_within(ACTIVATION_QUIESCENCE_BUDGET);
        let waited = started.elapsed();

        // Released before the join so a regression fails in the worker's own
        // budget rather than parking this test behind PUBLICATION_LOCK_WAIT.
        drop(holder);
        worker.join().expect("activation worker thread");
        let failure_code = service
            .snapshot()
            .expect("the fixture seeded a snapshot")
            .failure_code;

        assert_eq!(
            quiescence,
            ActivationQuiescence::Quiesced,
            "a worker merely waiting behind a peer's publication was reported unquiesced after {waited:?}, which aborts the process"
        );
        assert!(
            waited < Duration::from_secs(5),
            "the cancelled worker took {waited:?} to leave a lock wait it should have left within {:?}",
            codestory_contracts::bounded_locks::MAX_CANCELLATION_LATENCY
        );
        // Not merely fast: the wait ended because the flag was raised, and not
        // because the peer happened to release first.
        assert_eq!(
            failure_code.as_deref(),
            Some("cancelled"),
            "the worker must leave the lock wait on its cancellation flag"
        );
    }

    #[test]
    fn an_unquiesced_activation_worker_fail_stops_instead_of_being_detached() {
        // A worker that has not reached a cancellation checkpoint may still
        // hold a publication or store lock. Returning to the caller would
        // detach it behind an evicted context, so the join reports a fail-stop
        // and the hosting process records evidence and aborts.
        let fixture = bounded_runtime_fixture();
        let service = fixture.runtime.activation_service();
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.running = true;
            state.current_cancel = Some(Arc::clone(&cancelled));
        }
        let recorded = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&recorded);
        let previous = set_activation_fail_stop_hook(Some(Arc::new(move |reason: &str| {
            sink.lock()
                .expect("fail-stop sink")
                .push(reason.to_string());
        })));

        let started = Instant::now();
        let quiescence = service.cancel_and_wait_or_fail_stop(Duration::from_millis(80));
        let waited = started.elapsed();
        set_activation_fail_stop_hook(previous);

        assert_eq!(quiescence, ActivationQuiescence::FailStopRequired);
        assert!(
            cancelled.load(Ordering::Acquire),
            "the join must raise the worker's cancellation flag first"
        );
        assert_eq!(
            recorded.lock().expect("fail-stop sink").as_slice(),
            [ACTIVATION_QUIESCENCE_FAIL_STOP.to_string()]
        );
        assert!(
            waited < Duration::from_secs(5),
            "the join waited {waited:?} instead of its 80 ms budget"
        );

        {
            let mut state = service
                .coordinator
                .state
                .lock()
                .expect("activation coordinator");
            state.running = false;
            state.current_cancel = None;
        }
        service.coordinator.changed.notify_all();
        assert_eq!(
            service.cancel_and_wait_within(Duration::from_millis(80)),
            ActivationQuiescence::Quiesced,
            "a worker at a quiescent boundary must join without a fail-stop"
        );
    }

    #[test]
    fn the_read_only_facade_inherits_the_active_public_operation_cancellation() {
        // The facade used to mint a fresh, permanently-false flag on entry,
        // which replaced the host's live cancellation for the whole tool body.
        let fixture = bounded_runtime_fixture();
        let browser = fixture.runtime.browser_service();
        let cancelled = Arc::new(AtomicBool::new(true));

        let error = with_public_operation_cancellation(Arc::clone(&cancelled), || {
            browser
                .search(SearchRequest {
                    query: "anchor".into(),
                    repo_text: codestory_contracts::api::SearchRepoTextMode::Off,
                    limit_per_source: 1,
                    expand_search_plan: false,
                    hybrid_weights: None,
                    hybrid_limits: None,
                })
                .expect_err("an already-cancelled host request must not run a tool body")
        });

        assert_eq!(error.code, "cancelled");
    }
}
