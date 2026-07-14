use crate::cache::RetrievalCache;
#[cfg(test)]
use crate::cache::RetrievalCacheKey;
use crate::candidate::CandidateHit;
use crate::health::{
    probe_sidecar_health, probe_sidecar_health_for_runtime,
    probe_sidecar_health_with_embedding_device,
};
use crate::index::query_fingerprint;
use crate::mode::{RetrievalDegradedMode, derive_degraded_mode};
use crate::planner::{PlannedStage, RetrievalStageKind};
use crate::query_features::{QueryFeatures, classify_query};
use crate::ranker::rank_candidates;
use crate::sidecar_search::{SearchExecutionContext, SidecarSearch};
use anyhow::{Result, bail};
use codestory_store::RetrievalIndexManifest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

const STAGE_WORKER_LIMIT: usize = 16;
const STAGE_WAIT_POLL: Duration = Duration::from_millis(5);
const MAX_RETRIEVAL_BUDGET_MS: u64 = 120_000;
static STAGE_WORKER_POOL: OnceLock<StageWorkerPool> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Trace for one retrieval stage.
///
/// `degraded` and `stub_reason` are diagnostic fields. A stage trace does not make partial
/// sidecar output eligible for packet/search primary results.
pub struct StageTrace {
    pub stage: RetrievalStageKind,
    pub budget_ms: u64,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub admission_wait_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_ms: Option<u64>,
    pub candidates_added: usize,
    pub marginal_gain: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    pub cache_hit: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub degraded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stub_reason: Option<String>,
    #[serde(default)]
    pub completion_status: StageCompletionStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageCompletionStatus {
    #[default]
    Completed,
    PendingAfterDeadline,
    CancelledBeforeStart,
    CompletedLate,
    Skipped,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Trace for a complete sidecar query.
///
/// `retrieval_mode="full"` is the only product-ready mode. `degraded_reason`, cancellation, and
/// cache-hit fields explain why a query could not provide fresh full-mode evidence.
pub struct QueryTrace {
    pub retrieval_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
    pub total_budget_ms: u64,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    pub cache_hit: bool,
    pub stages: Vec<StageTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalPublicationIdentity {
    pub core_generation_id: String,
    pub core_run_id: String,
    pub sidecar_generation: String,
    pub sidecar_input_hash: String,
    pub qdrant_collection: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Result of executing one retrieval query against the sidecar stack.
///
/// Hits may include lexical, graph, or dense-anchor candidates. Runtime packet code must still
/// resolve candidates to indexed symbols before treating them as answer support.
pub struct QueryResult {
    #[serde(skip)]
    pub publication_identity: Option<RetrievalPublicationIdentity>,
    pub query: String,
    pub features: QueryFeatures,
    pub hits: Vec<CandidateHit>,
    pub trace: QueryTrace,
}

impl QueryResult {
    pub(crate) fn with_publication_identity(
        mut self,
        identity: &RetrievalPublicationIdentity,
    ) -> Self {
        self.publication_identity = Some(identity.clone());
        self
    }
}

/// Executes sidecar retrieval stages with manifest-scoped caching.
///
/// The executor is fail-closed: live degraded modes return an error instead of serving partial
/// results. Tests may use `mode_override`, but product callers should rely on live health probes.
pub struct QueryExecutor<'a> {
    pub sidecars: Arc<dyn SidecarSearch>,
    pub cache: &'a mut RetrievalCache,
    pub manifest: Option<RetrievalIndexManifest>,
    pub file_roles: Arc<HashMap<String, codestory_store::FileRole>>,
    pub cancelled: Arc<AtomicBool>,
    /// When set (tests), skips live health probing.
    pub mode_override: Option<RetrievalDegradedMode>,
}

impl<'a> QueryExecutor<'a> {
    /// Run one query within the provided total budget.
    ///
    /// `total_budget_ms` caps retrieval work only; it does not include runtime candidate
    /// resolution, packet sufficiency checks, or answer composition.
    pub fn execute(&mut self, query: &str, total_budget_ms: Option<u64>) -> Result<QueryResult> {
        let request_started = Instant::now();
        let features = classify_query(query);
        let fingerprint = query_fingerprint(&features.raw_query);

        let (mode, degraded_reason) = self.resolve_mode();
        if mode != RetrievalDegradedMode::Full {
            bail!(
                "retrieval sidecar is mandatory; project is not in full mode (mode={}, reason={})",
                mode.as_str(),
                degraded_reason.as_deref().unwrap_or("unknown")
            );
        }

        if !self.cancelled.load(Ordering::Acquire)
            && let Some(manifest) = self.manifest.as_ref()
        {
            let key = self.cache.key_for_manifest(manifest, fingerprint.clone());
            if let Some(cached) = self.cache.get(&key) {
                let cached = cached.to_vec();
                if self.cancelled.load(Ordering::Acquire) {
                    return Ok(cancelled_query_result(
                        features,
                        mode,
                        request_started,
                        "cancelled",
                    ));
                }
                return Ok(QueryResult {
                    publication_identity: None,
                    query: features.raw_query.clone(),
                    features,
                    hits: cached,
                    trace: QueryTrace {
                        retrieval_mode: mode.as_str().into(),
                        degraded_reason: None,
                        total_budget_ms: 0,
                        elapsed_ms: request_started.elapsed().as_millis() as u64,
                        cancel_reason: None,
                        cache_hit: true,
                        stages: Vec::new(),
                    },
                });
            }
        }

        let mut plan = crate::planner::plan_query(&features, mode);
        let planned_budget_ms = plan.stages.iter().map(|stage| stage.budget_ms).sum::<u64>();
        if let Some(budget) = total_budget_ms {
            if is_broad_query(features.shape) && budget < planned_budget_ms {
                scale_stage_budgets(&mut plan.stages, budget);
            }
            plan.total_budget_ms = budget.min(MAX_RETRIEVAL_BUDGET_MS);
        }
        plan.total_budget_ms = plan.total_budget_ms.min(MAX_RETRIEVAL_BUDGET_MS);

        let started = Instant::now();
        let deadline = started
            .checked_add(Duration::from_millis(plan.total_budget_ms))
            .unwrap_or(started);
        let mut candidates = Vec::new();
        let mut stage_traces = Vec::new();

        let mut cancel_reason = self.run_stage_sequence(
            &features,
            &plan.stages,
            &mut candidates,
            &mut stage_traces,
            deadline,
            StageSequenceOptions {
                stop_marginal_gain_threshold: Some(plan.stop_marginal_gain_threshold),
                stop_after_low_gain_streak: plan.stop_after_low_gain_streak,
            },
        )?;

        enrich_candidates_with_file_roles(&mut candidates, &self.file_roles);
        let ranked = rank_candidates(&features, candidates);
        let hits = ranked;

        if self.cancelled.load(Ordering::Acquire) {
            cancel_reason = Some("cancelled".into());
        } else if Instant::now() >= deadline && cancel_reason.is_none() {
            cancel_reason = Some("deadline".into());
        }

        if cancel_reason.is_none()
            && !self.cancelled.load(Ordering::Acquire)
            && Instant::now() < deadline
            && let Some(manifest) = self.manifest.as_ref()
        {
            let key = self.cache.key_for_manifest(manifest, fingerprint);
            if !self.cancelled.load(Ordering::Acquire) {
                self.cache.insert(key.clone(), hits.clone());
                if self.cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                    self.cache.remove(&key);
                    cancel_reason = Some(cancellation_reason(&self.cancelled).into());
                }
            }
        }

        Ok(QueryResult {
            publication_identity: None,
            query: features.raw_query.clone(),
            features,
            hits,
            trace: QueryTrace {
                retrieval_mode: mode.as_str().into(),
                degraded_reason,
                total_budget_ms: plan.total_budget_ms,
                elapsed_ms: request_started.elapsed().as_millis() as u64,
                cancel_reason,
                cache_hit: false,
                stages: stage_traces,
            },
        })
    }

    fn resolve_mode(&self) -> (RetrievalDegradedMode, Option<String>) {
        if let Some(mode) = self.mode_override {
            return (mode, None);
        }
        if let Some(manifest) = self.manifest.as_ref() {
            let Some(layout) = self.sidecars.layout() else {
                return (
                    RetrievalDegradedMode::Unavailable,
                    Some("sidecar_layout_missing".into()),
                );
            };
            let report = if let (Some(embedding_device), Some(runtime)) = (
                self.sidecars.embedding_device_readiness(),
                self.sidecars.runtime_config(),
            ) {
                probe_sidecar_health_for_runtime(
                    layout,
                    &manifest.project_id,
                    Some(manifest.clone()),
                    embedding_device,
                    runtime,
                )
            } else if let Some(embedding_device) = self.sidecars.embedding_device_readiness() {
                probe_sidecar_health_with_embedding_device(
                    layout,
                    &manifest.project_id,
                    Some(manifest.clone()),
                    embedding_device,
                )
            } else {
                probe_sidecar_health(layout, &manifest.project_id, Some(manifest.clone()))
            };
            return derive_degraded_mode(&report.lexical, &report.qdrant, &report.scip);
        }
        (
            RetrievalDegradedMode::LexicalOnly,
            Some("manifest_missing".into()),
        )
    }

    fn run_stage(
        sidecars: &dyn SidecarSearch,
        stage: &PlannedStage,
        features: &QueryFeatures,
        anchors: &[CandidateHit],
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        let query = &features.raw_query;
        match stage.kind {
            RetrievalStageKind::Stage0ScipAnchor => {
                sidecars.scip_anchor_with_context(query, stage.top_k, context)
            }
            RetrievalStageKind::Stage1Lexical => {
                sidecars.lexical_search_with_context(query, stage.top_k, context)
            }
            RetrievalStageKind::Stage1bQdrantSemantic => {
                sidecars.qdrant_search_with_context(query, stage.top_k, context)
            }
            RetrievalStageKind::Stage2ScipExpand => {
                sidecars.scip_expand_with_context(anchors, stage.top_k, context)
            }
            RetrievalStageKind::Stage3RepoTextFallback => {
                bail!("repo-text diagnostic stage is unsupported in mandatory sidecar retrieval")
            }
        }
    }

    fn run_stage_bounded(
        &self,
        stage: &PlannedStage,
        features: &QueryFeatures,
        anchors: &[CandidateHit],
        request_deadline: Instant,
    ) -> Result<StageRun> {
        let stage = stage.clone();
        let stage_deadline = request_deadline.min(
            Instant::now()
                .checked_add(Duration::from_millis(stage.budget_ms.max(1)))
                .unwrap_or(request_deadline),
        );
        let features = features.clone();
        let anchors = anchors.to_vec();
        let sidecars = Arc::clone(&self.sidecars);
        let stage_cancelled = Arc::new(AtomicBool::new(false));
        let context = SearchExecutionContext::new(
            stage_deadline,
            Arc::clone(&self.cancelled),
            Arc::clone(&stage_cancelled),
        );
        let admission_started = Instant::now();
        let Some((permit, admission_wait_ms)) =
            stage_worker_pool().acquire(stage_deadline, self.cancelled.as_ref())
        else {
            stage_cancelled.store(true, Ordering::Release);
            return Ok(StageRun::Cancelled {
                reason: cancellation_reason(&self.cancelled),
                admission_wait_ms: admission_started.elapsed().as_millis() as u64,
                queue_wait_ms: None,
                execution_ms: None,
                completion_status: StageCompletionStatus::CancelledBeforeStart,
            });
        };
        let state = Arc::new(StageJobState::default());
        let (sender, receiver) = mpsc::channel();
        let queued_at = Instant::now();
        let job = StageJob {
            queued_at,
            state: Arc::clone(&state),
            task: Box::new(move || {
                Self::run_stage(sidecars.as_ref(), &stage, &features, &anchors, &context)
            }),
            sender,
            _permit: permit,
        };
        stage_worker_pool()
            .sender
            .send(job)
            .map_err(|_| anyhow::anyhow!("retrieval stage worker pool disconnected"))?;

        loop {
            if self.cancelled.load(Ordering::Acquire) || Instant::now() >= stage_deadline {
                stage_cancelled.store(true, Ordering::Release);
                return Ok(cancelled_stage_run(
                    cancellation_reason(&self.cancelled),
                    admission_wait_ms,
                    &state,
                ));
            }
            let remaining = stage_deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining.min(STAGE_WAIT_POLL)) {
                Ok(completion) => {
                    let late = completion.finished_at >= stage_deadline
                        || self.cancelled.load(Ordering::Acquire);
                    if late {
                        stage_cancelled.store(true, Ordering::Release);
                        return Ok(StageRun::Cancelled {
                            reason: cancellation_reason(&self.cancelled),
                            admission_wait_ms,
                            queue_wait_ms: Some(completion.queue_wait_ms),
                            execution_ms: Some(completion.execution_ms),
                            completion_status: StageCompletionStatus::CompletedLate,
                        });
                    }
                    return completion.result.map(|hits| StageRun::Completed {
                        hits,
                        admission_wait_ms,
                        queue_wait_ms: Some(completion.queue_wait_ms),
                        execution_ms: Some(completion.execution_ms),
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("sidecar stage worker disconnected")
                }
            }
        }
    }

    fn run_stage_sequence(
        &self,
        features: &QueryFeatures,
        stages: &[PlannedStage],
        candidates: &mut Vec<CandidateHit>,
        stage_traces: &mut Vec<StageTrace>,
        deadline: Instant,
        options: StageSequenceOptions,
    ) -> Result<Option<String>> {
        let mut low_gain_streak = 0u32;
        let mut cancel_reason = None;
        for (index, stage) in stages.iter().enumerate() {
            if self.cancelled.load(Ordering::Relaxed) {
                return Ok(Some("cancelled".into()));
            }
            if Instant::now() >= deadline {
                return Ok(Some(cancel_reason.unwrap_or_else(|| "deadline".into())));
            }

            if should_skip_after_exact_symbol_anchor(stage, features, candidates) {
                let mut trace = stage_trace(
                    stage,
                    0,
                    0,
                    0.0,
                    Some("exact_symbol_anchor".into()),
                    false,
                    None,
                );
                trace.completion_status = StageCompletionStatus::Skipped;
                stage_traces.push(trace);
                continue;
            }
            if should_skip_zero_dense_stage(stage, self.manifest.as_ref()) {
                let mut trace = stage_trace(
                    stage,
                    0,
                    0,
                    0.0,
                    Some("zero_dense_anchors".into()),
                    false,
                    None,
                );
                trace.completion_status = StageCompletionStatus::Skipped;
                stage_traces.push(trace);
                continue;
            }

            let mut stage = stage.clone();
            let remaining_budget_ms = u64::try_from(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis(),
            )
            .unwrap_or(u64::MAX)
            .max(1);
            let later_stage_reserve_ms = stages[index + 1..]
                .iter()
                .map(|stage| stage.budget_ms)
                .sum::<u64>();
            stage.budget_ms = stage
                .budget_ms
                .max(remaining_budget_ms.saturating_sub(later_stage_reserve_ms))
                .min(remaining_budget_ms);

            let stage_started = Instant::now();
            let before_score = candidate_mass(candidates);
            let (mut stage_hits, admission_wait_ms, queue_wait_ms, execution_ms) =
                match self.run_stage_bounded(&stage, features, candidates, deadline)? {
                    StageRun::Completed {
                        hits,
                        admission_wait_ms,
                        queue_wait_ms,
                        execution_ms,
                    } => (hits, admission_wait_ms, queue_wait_ms, execution_ms),
                    StageRun::Cancelled {
                        reason,
                        admission_wait_ms,
                        queue_wait_ms,
                        execution_ms,
                        completion_status,
                    } => {
                        let mut trace = stage_trace(
                            &stage,
                            stage_started.elapsed().as_millis() as u64,
                            0,
                            0.0,
                            Some(reason.into()),
                            false,
                            None,
                        );
                        trace.admission_wait_ms = admission_wait_ms;
                        trace.queue_wait_ms = queue_wait_ms;
                        trace.execution_ms = execution_ms;
                        trace.completion_status = completion_status;
                        stage_traces.push(trace);
                        cancel_reason.get_or_insert_with(|| reason.into());
                        continue;
                    }
                };
            annotate_stage_provenance(&stage, &mut stage_hits);
            let (stub_reason, stage_degraded) = stage_stub_metadata(&stage_hits);
            let added = merge_candidates(candidates, stage_hits);
            let after_score = candidate_mass(candidates);
            let marginal_gain = if before_score <= 0.0 {
                after_score
            } else {
                ((after_score - before_score) / before_score).max(0.0)
            };

            let mut trace = stage_trace(
                &stage,
                stage_started.elapsed().as_millis() as u64,
                added,
                marginal_gain,
                None,
                stage_degraded,
                stub_reason,
            );
            trace.admission_wait_ms = admission_wait_ms;
            trace.queue_wait_ms = queue_wait_ms;
            trace.execution_ms = execution_ms;
            stage_traces.push(trace);

            if let Some(threshold) = options.stop_marginal_gain_threshold {
                if marginal_gain < threshold && !candidates.is_empty() {
                    low_gain_streak += 1;
                    if low_gain_streak >= options.stop_after_low_gain_streak {
                        return Ok(Some(
                            cancel_reason.unwrap_or_else(|| "marginal_gain".into()),
                        ));
                    }
                } else {
                    low_gain_streak = 0;
                }
            }
        }
        Ok(cancel_reason)
    }
}

fn cancelled_query_result(
    features: QueryFeatures,
    mode: RetrievalDegradedMode,
    started: Instant,
    reason: &str,
) -> QueryResult {
    QueryResult {
        publication_identity: None,
        query: features.raw_query.clone(),
        features,
        hits: Vec::new(),
        trace: QueryTrace {
            retrieval_mode: mode.as_str().into(),
            degraded_reason: None,
            total_budget_ms: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
            cancel_reason: Some(reason.into()),
            cache_hit: false,
            stages: Vec::new(),
        },
    }
}

enum StageRun {
    Completed {
        hits: Vec<CandidateHit>,
        admission_wait_ms: u64,
        queue_wait_ms: Option<u64>,
        execution_ms: Option<u64>,
    },
    Cancelled {
        reason: &'static str,
        admission_wait_ms: u64,
        queue_wait_ms: Option<u64>,
        execution_ms: Option<u64>,
        completion_status: StageCompletionStatus,
    },
}

#[derive(Default)]
struct StageJobState {
    started: AtomicBool,
    queue_wait_ms: AtomicU64,
}

struct StageCompletion {
    result: Result<Vec<CandidateHit>>,
    queue_wait_ms: u64,
    execution_ms: u64,
    finished_at: Instant,
}

struct StageJob {
    queued_at: Instant,
    state: Arc<StageJobState>,
    task: Box<dyn FnOnce() -> Result<Vec<CandidateHit>> + Send>,
    sender: mpsc::Sender<StageCompletion>,
    _permit: StageAdmissionPermit,
}

struct StageWorkerPool {
    sender: mpsc::SyncSender<StageJob>,
    admission: Arc<StageAdmission>,
}

struct StageAdmission {
    in_flight: Mutex<usize>,
    available: Condvar,
    capacity: usize,
}

struct StageAdmissionPermit {
    admission: Arc<StageAdmission>,
}

impl Drop for StageAdmissionPermit {
    fn drop(&mut self) {
        let mut in_flight = self
            .admission
            .in_flight
            .lock()
            .expect("stage admission lock");
        *in_flight = in_flight.saturating_sub(1);
        self.admission.available.notify_one();
    }
}

impl StageWorkerPool {
    fn new(worker_limit: usize, queue_capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<StageJob>(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..worker_limit {
            let receiver = Arc::clone(&receiver);
            std::thread::Builder::new()
                .name(format!("codestory-retrieval-{index}"))
                .spawn(move || stage_worker_loop(&receiver))
                .expect("spawn bounded retrieval stage worker");
        }
        Self {
            sender,
            admission: Arc::new(StageAdmission {
                in_flight: Mutex::new(0),
                available: Condvar::new(),
                capacity: worker_limit.saturating_add(queue_capacity),
            }),
        }
    }

    fn acquire(
        &self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Option<(StageAdmissionPermit, u64)> {
        let started = Instant::now();
        let mut in_flight = self
            .admission
            .in_flight
            .lock()
            .expect("stage admission lock");
        loop {
            if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                return None;
            }
            if *in_flight < self.admission.capacity {
                *in_flight += 1;
                return Some((
                    StageAdmissionPermit {
                        admission: Arc::clone(&self.admission),
                    },
                    started.elapsed().as_millis() as u64,
                ));
            }
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(STAGE_WAIT_POLL);
            let (guard, _) = self
                .admission
                .available
                .wait_timeout(in_flight, wait)
                .expect("stage admission wait");
            in_flight = guard;
        }
    }
}

fn stage_worker_pool() -> &'static StageWorkerPool {
    STAGE_WORKER_POOL.get_or_init(|| StageWorkerPool::new(STAGE_WORKER_LIMIT, STAGE_WORKER_LIMIT))
}

fn stage_worker_loop(receiver: &Mutex<mpsc::Receiver<StageJob>>) {
    loop {
        let job = match receiver.lock().expect("stage queue lock").recv() {
            Ok(job) => job,
            Err(_) => return,
        };
        let queue_wait_ms = job.queued_at.elapsed().as_millis() as u64;
        job.state
            .queue_wait_ms
            .store(queue_wait_ms, Ordering::Release);
        job.state.started.store(true, Ordering::Release);
        let started = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(job.task))
            .map_err(|_| anyhow::anyhow!("retrieval stage worker panicked"))
            .and_then(|result| result);
        let completion = StageCompletion {
            result,
            queue_wait_ms,
            execution_ms: started.elapsed().as_millis() as u64,
            finished_at: Instant::now(),
        };
        if job.sender.send(completion).is_err() {
            tracing::debug!(
                late_completion = true,
                "discarded late retrieval stage result"
            );
        }
    }
}

fn cancellation_reason(cancelled: &AtomicBool) -> &'static str {
    if cancelled.load(Ordering::Acquire) {
        "cancelled"
    } else {
        "stage_deadline"
    }
}

fn cancelled_stage_run(
    reason: &'static str,
    admission_wait_ms: u64,
    state: &StageJobState,
) -> StageRun {
    let started = state.started.load(Ordering::Acquire);
    StageRun::Cancelled {
        reason,
        admission_wait_ms,
        queue_wait_ms: if started {
            Some(state.queue_wait_ms.load(Ordering::Acquire))
        } else {
            None
        },
        execution_ms: None,
        completion_status: if started {
            StageCompletionStatus::PendingAfterDeadline
        } else {
            StageCompletionStatus::CancelledBeforeStart
        },
    }
}

#[derive(Debug, Clone, Copy)]
struct StageSequenceOptions {
    stop_marginal_gain_threshold: Option<f32>,
    stop_after_low_gain_streak: u32,
}

fn is_broad_query(shape: crate::query_features::QueryShape) -> bool {
    matches!(
        shape,
        crate::query_features::QueryShape::NaturalLanguage
            | crate::query_features::QueryShape::Mixed
    )
}

fn scale_stage_budgets(stages: &mut [PlannedStage], total_budget_ms: u64) {
    let planned_total = stages.iter().map(|stage| stage.budget_ms).sum::<u64>();
    if stages.is_empty() || planned_total == 0 {
        return;
    }

    let mut remaining = total_budget_ms.max(stages.len() as u64);
    let last = stages.len() - 1;
    for (index, stage) in stages.iter_mut().enumerate() {
        if index == last {
            stage.budget_ms = remaining.max(1);
            break;
        }
        let stages_left = (last - index) as u64;
        let scaled = stage.budget_ms.saturating_mul(total_budget_ms) / planned_total;
        let budget = scaled.max(1).min(remaining.saturating_sub(stages_left));
        stage.budget_ms = budget;
        remaining = remaining.saturating_sub(budget);
    }
}

fn stage_trace(
    stage: &PlannedStage,
    elapsed_ms: u64,
    candidates_added: usize,
    marginal_gain: f32,
    cancel_reason: Option<String>,
    degraded: bool,
    stub_reason: Option<String>,
) -> StageTrace {
    StageTrace {
        stage: stage.kind,
        budget_ms: stage.budget_ms,
        elapsed_ms,
        admission_wait_ms: 0,
        queue_wait_ms: None,
        execution_ms: None,
        candidates_added,
        marginal_gain,
        cancel_reason,
        cache_hit: false,
        degraded,
        stub_reason,
        completion_status: StageCompletionStatus::Completed,
    }
}

fn should_skip_after_exact_symbol_anchor(
    stage: &PlannedStage,
    features: &QueryFeatures,
    candidates: &[CandidateHit],
) -> bool {
    if !matches!(
        features.shape,
        crate::query_features::QueryShape::SymbolLike
    ) {
        return false;
    }
    if !matches!(
        stage.kind,
        RetrievalStageKind::Stage1bQdrantSemantic | RetrievalStageKind::Stage2ScipExpand
    ) {
        return false;
    }
    candidates
        .iter()
        .any(|candidate| candidate_is_exact_symbol_anchor(&features.raw_query, candidate))
}

fn should_skip_zero_dense_stage(
    stage: &PlannedStage,
    manifest: Option<&RetrievalIndexManifest>,
) -> bool {
    if !matches!(stage.kind, RetrievalStageKind::Stage1bQdrantSemantic) {
        return false;
    }
    let dense_count = manifest
        .and_then(|manifest| {
            manifest
                .dense_projection_count
                .or(manifest.projection_count)
        })
        .unwrap_or(0);
    dense_count <= 0
}

fn annotate_stage_provenance(stage: &PlannedStage, hits: &mut [CandidateHit]) {
    if let Some(label) = stage.kind.provenance_label() {
        for hit in hits {
            hit.add_provenance(label);
        }
    }
}

fn candidate_is_exact_symbol_anchor(query: &str, candidate: &CandidateHit) -> bool {
    if matches!(
        candidate.source,
        crate::candidate::CandidateSource::Qdrant | crate::candidate::CandidateSource::Legacy
    ) {
        return false;
    }
    let Some(symbol) = candidate.symbol_name.as_deref() else {
        return false;
    };
    let query_lower = query.trim().to_ascii_lowercase();
    if query_lower.is_empty() {
        return false;
    }
    let symbol_lower = symbol.trim().to_ascii_lowercase();
    if symbol_lower == query_lower {
        return true;
    }
    let symbol_tail = symbol_lower
        .rsplit("::")
        .next()
        .unwrap_or(&symbol_lower)
        .rsplit('.')
        .next()
        .unwrap_or(&symbol_lower);
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|token| token.len() >= 2)
        .map(|token| token.to_ascii_lowercase())
        .any(|token| token == symbol_tail)
}

fn candidate_mass(candidates: &[CandidateHit]) -> f32 {
    candidates.iter().map(|hit| hit.score.max(0.01)).sum()
}

fn stage_stub_metadata(hits: &[CandidateHit]) -> (Option<String>, bool) {
    if hits.is_empty() {
        return (None, false);
    }
    if crate::candidate::phantom_sidecar_candidates_only(hits) {
        return (Some("phantom_stub_hits".into()), true);
    }
    (None, false)
}

fn merge_candidates(acc: &mut Vec<CandidateHit>, incoming: Vec<CandidateHit>) -> usize {
    let mut added = 0usize;
    for hit in incoming {
        let duplicate = acc.iter_mut().find(|existing| {
            existing.file_path == hit.file_path && existing.symbol_name == hit.symbol_name
        });
        if let Some(existing) = duplicate {
            existing.score = existing.score.max(hit.score);
            if existing.node_id.is_none() {
                existing.node_id = hit.node_id.clone();
            }
            if existing.start_line.is_none() {
                existing.start_line = hit.start_line;
            }
            if existing.file_role.is_none() {
                existing.file_role = hit.file_role;
            }
            if existing.scip_hop_distance.is_none() {
                existing.scip_hop_distance = hit.scip_hop_distance;
            }
            for label in hit.provenance {
                existing.add_provenance(label);
            }
            continue;
        }
        acc.push(hit);
        added += 1;
    }
    added
}

pub fn cancellation_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn enrich_candidates_with_file_roles(
    candidates: &mut [CandidateHit],
    file_roles: &HashMap<String, codestory_store::FileRole>,
) {
    for candidate in candidates {
        if candidate.file_role.is_some() {
            continue;
        }
        candidate.file_role = Some(
            lookup_file_role(file_roles, &candidate.file_path).unwrap_or_else(|| {
                codestory_store::FileRole::classify_path(Path::new(&candidate.file_path))
            }),
        );
    }
}

fn lookup_file_role(
    file_roles: &HashMap<String, codestory_store::FileRole>,
    file_path: &str,
) -> Option<codestory_store::FileRole> {
    file_roles.get(file_path).copied().or_else(|| {
        let normalized = file_path.replace('\\', "/");
        file_roles.get(&normalized).copied()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::RetrievalCache;
    use crate::candidate::{CandidateHit, CandidateSource};
    use crate::config::SidecarLayout;
    use crate::sidecar_search::{SidecarSearch, mock::MockSidecarSearch};
    use crate::test_support::retrieval_manifest_fixture;
    use codestory_store::RetrievalIndexManifest;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn sample_manifest() -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: "testproj".into(),
            lexical_version: "v1".into(),
            qdrant_collection: "codestory_testproj".into(),
            scip_revision: Some("rev1".into()),
            built_at_epoch_ms: 0,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: None,
            embedding_dim: None,
            sidecar_schema_version: None,
            sidecar_input_hash: None,
            sidecar_generation: None,
            projection_count: Some(10),
            symbol_doc_count: Some(20),
            dense_projection_count: Some(10),
            semantic_policy_version: None,
            graph_artifact_hash: None,
            dense_reason_counts_json: None,
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        }
    }

    #[test]
    fn executor_runs_stages_with_mock_sidecars() {
        let mock = MockSidecarSearch {
            lexical: Mutex::new(HashMap::from([(
                "ExtensionService".into(),
                vec![CandidateHit::with_source(
                    "src/service.rs",
                    Some("ExtensionService".into()),
                    0.9,
                    CandidateSource::Lexical,
                )],
            )])),
            qdrant: Mutex::new(HashMap::from([(
                "ExtensionService".into(),
                vec![CandidateHit::with_source(
                    "src/service_semantic.rs",
                    Some("ExtensionService".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )],
            )])),
            scip_anchor: Mutex::new(HashMap::new()),
            scip_expand: Mutex::new(Vec::new()),
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("ExtensionService", Some(800))
            .expect("query succeeds");
        assert!(!result.hits.is_empty());
        assert!(!result.trace.stages.is_empty());
        assert!(!result.trace.cache_hit);
    }

    #[test]
    fn executor_uses_cache_on_second_query() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        let key = RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("cached-query"));
        cache.insert(key, vec![CandidateHit::lexical_stub("cached.rs", 1.0)]);

        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("cached-query", None).expect("cache hit");
        assert!(result.trace.cache_hit);
        assert_eq!(result.hits[0].file_path, "cached.rs");
    }

    #[test]
    fn executor_caches_only_complete_query_results() {
        let mock = Arc::new(MockSidecarSearch {
            lexical: Mutex::new(HashMap::from([(
                "startup".into(),
                vec![CandidateHit::with_source(
                    "src/startup.rs",
                    Some("startup".into()),
                    0.9,
                    CandidateSource::Lexical,
                )],
            )])),
            ..Default::default()
        });
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        {
            let mut executor = QueryExecutor {
                sidecars: mock.clone(),
                cache: &mut cache,
                manifest: Some(manifest.clone()),
                file_roles: Arc::new(HashMap::new()),
                cancelled: cancellation_flag(),
                mode_override: Some(RetrievalDegradedMode::Full),
            };
            let result = executor.execute("startup", Some(800)).expect("query");
            assert_eq!(result.trace.cancel_reason, None);
        }
        let key = RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("startup"));
        assert!(cache.get(&key).is_some());

        let mut cancelled_cache = RetrievalCache::new();
        let cancelled_key =
            RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("startup"));
        cancelled_cache.insert(
            cancelled_key,
            vec![CandidateHit::lexical_stub("stale-cancelled.rs", 1.0)],
        );
        let cancelled = cancellation_flag();
        cancelled.store(true, Ordering::Relaxed);
        let mut executor = QueryExecutor {
            sidecars: mock.clone(),
            cache: &mut cancelled_cache,
            manifest: Some(manifest.clone()),
            file_roles: Arc::new(HashMap::new()),
            cancelled,
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("startup", Some(800)).expect("query");
        assert_eq!(result.trace.cancel_reason.as_deref(), Some("cancelled"));
        assert!(
            result.hits.is_empty(),
            "cancelled requests must not serve cache"
        );
        assert_eq!(cancelled_cache.len(), 1);
    }

    #[test]
    fn executor_skips_semantic_and_expand_after_exact_symbol_anchor() {
        let mock = MockSidecarSearch {
            scip_anchor: Mutex::new(HashMap::from([(
                "EventProcessor".into(),
                vec![CandidateHit::with_source(
                    "src/event_processor.rs",
                    Some("EventProcessor".into()),
                    0.95,
                    CandidateSource::Scip,
                )],
            )])),
            qdrant: Mutex::new(HashMap::from([(
                "EventProcessor".into(),
                vec![CandidateHit::with_source(
                    "docs/event-output.md",
                    Some("event output".into()),
                    0.99,
                    CandidateSource::Qdrant,
                )],
            )])),
            scip_expand: Mutex::new(vec![CandidateHit::with_source(
                "src/neighbor.rs",
                Some("Neighbor".into()),
                0.80,
                CandidateSource::Scip,
            )]),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("EventProcessor", Some(800))
            .expect("query succeeds");
        assert_eq!(
            result.hits.first().map(|hit| hit.file_path.as_str()),
            Some("src/event_processor.rs")
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "docs/event-output.md")
        );
        let skipped: Vec<_> = result
            .trace
            .stages
            .iter()
            .filter(|stage| stage.cancel_reason.as_deref() == Some("exact_symbol_anchor"))
            .map(|stage| stage.stage)
            .collect();
        assert!(skipped.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(skipped.contains(&RetrievalStageKind::Stage2ScipExpand));
        let skipped_final = result
            .trace
            .stages
            .iter()
            .find(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic)
            .expect("skipped final stage");
        assert_eq!(skipped_final.budget_ms, 120);
        assert_eq!(
            skipped_final.completion_status,
            StageCompletionStatus::Skipped
        );
    }

    #[test]
    fn executor_rejects_non_full_modes() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::NoSemantic),
        };
        let error = executor
            .execute("ExtensionService", Some(800))
            .expect_err("non-full modes must fail closed");
        assert!(error.to_string().contains("retrieval sidecar is mandatory"));
    }

    #[test]
    fn executor_resolve_mode_probes_live_sidecar_layout_instead_of_env_default() {
        let layout = SidecarLayout::from_env();
        let manifest = retrieval_manifest_fixture("testproj", "cafebabedeadbeef");
        let sidecars = Arc::new(TrackingSidecars {
            layout,
            layout_calls: AtomicUsize::new(0),
        });
        let mut cache = RetrievalCache::new();
        let executor = QueryExecutor {
            sidecars: sidecars.clone(),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: None,
        };

        let (mode, reason) = executor.resolve_mode();

        assert_eq!(sidecars.layout_calls.load(Ordering::Relaxed), 1);
        assert_eq!(mode, RetrievalDegradedMode::Unavailable);
        assert_eq!(reason.as_deref(), Some("lexical_shard_unavailable"));
    }

    #[test]
    fn executor_rejects_cached_hits_when_mode_is_not_full() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let manifest = sample_manifest();
        let key = RetrievalCacheKey::from_manifest(&manifest, query_fingerprint("cached-query"));
        cache.insert(key, vec![CandidateHit::lexical_stub("cached.rs", 1.0)]);

        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::NoSemantic),
        };
        let error = executor
            .execute("cached-query", None)
            .expect_err("cache must not bypass mandatory full sidecar mode");
        assert!(error.to_string().contains("retrieval sidecar is mandatory"));
    }

    #[test]
    fn executor_reaches_semantic_stage_after_empty_lexical_stages() {
        let mock = MockSidecarSearch {
            qdrant: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/semantic.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("how does startup sequence work", Some(800))
            .expect("query");
        assert!(
            result
                .trace
                .stages
                .iter()
                .any(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic),
            "semantic stage should run after empty SCIP/Lexical stages: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/semantic.rs"),
            "expected semantic hit after empty lexical stages: {:?}",
            result.hits
        );
    }

    #[test]
    fn broad_query_stage_deadline_preserves_later_sidecar_contribution() {
        struct SlowLexicalSidecars;

        impl SidecarSearch for SlowLexicalSidecars {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(200));
                Ok(vec![CandidateHit::with_source(
                    "src/slow_lexical.rs",
                    Some("SlowLexical".into()),
                    0.99,
                    CandidateSource::Lexical,
                )])
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/semantic.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )])
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }
        }

        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(SlowLexicalSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let started = Instant::now();
        let result = executor
            .execute(
                "LiveSidecarSearch qdrant_search retrieval_mode full sidecar unavailable",
                Some(120),
            )
            .expect("query");

        assert!(
            started.elapsed() < Duration::from_millis(190),
            "slow Lexical must not consume the whole broad-query path: {:?}",
            result.trace.stages
        );
        assert_eq!(
            result.trace.cancel_reason.as_deref(),
            Some("stage_deadline")
        );
        assert!(
            result.trace.stages.iter().any(|stage| {
                stage.stage == RetrievalStageKind::Stage1Lexical
                    && stage.cancel_reason.as_deref() == Some("stage_deadline")
            }),
            "Lexical overrun should be explicit in stage provenance: {:?}",
            result.trace.stages
        );
        assert!(
            result.trace.stages.iter().any(|stage| {
                stage.stage == RetrievalStageKind::Stage1bQdrantSemantic
                    && stage.candidates_added > 0
            }),
            "Qdrant must still contribute after Lexical overrun: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/semantic.rs"),
            "semantic fallback should be rankable after lexical overrun: {:?}",
            result.hits
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "src/slow_lexical.rs"),
            "timed-out Lexical hits must not merge late into this query: {:?}",
            result.hits
        );
    }

    #[test]
    fn broad_query_slow_scip_expand_still_allows_qdrant_contribution() {
        struct SlowScipExpandSidecars;

        impl SidecarSearch for SlowScipExpandSidecars {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/lexical.rs",
                    Some("LexicalAnchor".into()),
                    0.7,
                    CandidateSource::Lexical,
                )])
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/semantic.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )])
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(200));
                Ok(vec![CandidateHit::with_source(
                    "src/slow_graph.rs",
                    Some("SlowGraph".into()),
                    0.95,
                    CandidateSource::Scip,
                )])
            }
        }

        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(SlowScipExpandSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute(
                "LiveSidecarSearch qdrant_search retrieval_mode full sidecar unavailable",
                Some(120),
            )
            .expect("query");

        assert_eq!(
            result.trace.cancel_reason.as_deref(),
            Some("stage_deadline")
        );
        assert!(
            result.trace.stages.iter().any(|stage| {
                stage.stage == RetrievalStageKind::Stage2ScipExpand
                    && stage.cancel_reason.as_deref() == Some("stage_deadline")
            }),
            "SCIP expand overrun should be explicit in stage provenance: {:?}",
            result.trace.stages
        );
        assert!(
            result.trace.stages.iter().any(|stage| {
                stage.stage == RetrievalStageKind::Stage1bQdrantSemantic
                    && stage.candidates_added > 0
            }),
            "Qdrant must still contribute after SCIP expand overrun: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/semantic.rs"),
            "semantic fallback should be rankable after graph overrun: {:?}",
            result.hits
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "src/slow_graph.rs"),
            "timed-out graph hits must not merge late into this query: {:?}",
            result.hits
        );
    }

    #[test]
    fn graph_stage_uses_remaining_total_budget_across_query_orders() {
        struct LateUsefulGraphSidecars;

        impl SidecarSearch for LateUsefulGraphSidecars {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(110));
                Ok(vec![CandidateHit::with_source(
                    "src/lexical.rs",
                    Some("LiveSidecarSearch".into()),
                    0.7,
                    CandidateSource::Lexical,
                )])
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/dense.rs",
                    Some("ReadinessInputs".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )])
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(70));
                Ok(vec![CandidateHit::with_source(
                    "src/scip_anchor.rs",
                    Some("RuntimeAnchor".into()),
                    0.75,
                    CandidateSource::Scip,
                )])
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(220));
                Ok(vec![CandidateHit::with_source(
                    "src/graph_neighbor.rs",
                    Some("PackagedAgentReadiness".into()),
                    0.9,
                    CandidateSource::Scip,
                )])
            }
        }

        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(LateUsefulGraphSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute(
                "Explain how LiveSidecarSearch validates packaged agent readiness",
                Some(1_000),
            )
            .expect("query");

        assert_eq!(
            result.features.shape,
            crate::query_features::QueryShape::Mixed
        );
        assert_eq!(result.trace.cancel_reason, None);
        for kind in [
            RetrievalStageKind::Stage1Lexical,
            RetrievalStageKind::Stage1bQdrantSemantic,
        ] {
            let stage = result
                .trace
                .stages
                .iter()
                .find(|stage| stage.stage == kind)
                .expect("completed anchor stage");
            assert_eq!(stage.completion_status, StageCompletionStatus::Completed);
            assert!(stage.candidates_added > 0);
        }
        let graph = result
            .trace
            .stages
            .iter()
            .find(|stage| stage.stage == RetrievalStageKind::Stage2ScipExpand)
            .expect("graph stage");
        assert_eq!(graph.completion_status, StageCompletionStatus::Completed);
        assert!(
            graph.budget_ms > 180,
            "graph stage should receive prior stage slack: {graph:?}"
        );
        assert!(result.hits.iter().any(|hit| {
            hit.file_path == "src/graph_neighbor.rs"
                && hit.provenance.iter().any(|value| value == "graph_neighbor")
        }));

        let result = executor
            .execute("RuntimeContext", Some(1_000))
            .expect("query");
        assert_eq!(
            result.features.shape,
            crate::query_features::QueryShape::SymbolLike
        );
        assert_eq!(result.trace.cancel_reason, None);
        for (kind, static_budget) in [
            (RetrievalStageKind::Stage0ScipAnchor, 40),
            (RetrievalStageKind::Stage1Lexical, 80),
            (RetrievalStageKind::Stage2ScipExpand, 180),
        ] {
            let stage = result
                .trace
                .stages
                .iter()
                .find(|stage| stage.stage == kind)
                .expect("reserve-expanded stage");
            assert_eq!(stage.completion_status, StageCompletionStatus::Completed);
            assert!(
                stage.budget_ms > static_budget,
                "stage should borrow slack while preserving later reserves: {stage:?}"
            );
        }
        let graph_index = result
            .trace
            .stages
            .iter()
            .position(|stage| stage.stage == RetrievalStageKind::Stage2ScipExpand)
            .expect("graph stage");
        let dense_index = result
            .trace
            .stages
            .iter()
            .position(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic)
            .expect("dense stage");
        assert!(graph_index < dense_index);
        let graph = &result.trace.stages[graph_index];
        assert_eq!(graph.completion_status, StageCompletionStatus::Completed);
        assert!(
            graph.budget_ms > 180,
            "graph stage should borrow slack while reserving dense time: {graph:?}"
        );
        assert_eq!(
            result.trace.stages[dense_index].completion_status,
            StageCompletionStatus::Completed
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/graph_neighbor.rs")
        );
        let cached = executor
            .execute("RuntimeContext", Some(1_000))
            .expect("cache");
        assert!(cached.trace.cache_hit);
        assert!(cached.trace.stages.is_empty());
    }

    #[test]
    fn broad_query_expands_dense_anchors_before_ranking_window() {
        struct DenseAnchorExpandSidecars;

        impl SidecarSearch for DenseAnchorExpandSidecars {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/semantic_anchor.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )])
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_expand(
                &self,
                anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                if anchors
                    .iter()
                    .any(|hit| hit.source == CandidateSource::Qdrant)
                {
                    return Ok(vec![CandidateHit::with_source(
                        "src/expanded_from_dense.rs",
                        Some("ExpandedFromDense".into()),
                        0.9,
                        CandidateSource::Scip,
                    )]);
                }
                Ok(Vec::new())
            }
        }

        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(DenseAnchorExpandSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("packet search output evidence retrieval shadow", Some(800))
            .expect("query");

        let qdrant_index = result
            .trace
            .stages
            .iter()
            .position(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic)
            .expect("qdrant stage");
        let expand_index = result
            .trace
            .stages
            .iter()
            .position(|stage| stage.stage == RetrievalStageKind::Stage2ScipExpand)
            .expect("scip expand stage");
        assert!(
            qdrant_index < expand_index,
            "dense anchors should be available before graph expansion: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/expanded_from_dense.rs"),
            "dense-anchor graph expansion should enter the rank window: {:?}",
            result.hits
        );
    }

    #[test]
    fn broad_query_lexical_budget_admits_source_anchor_before_dense_distractors() {
        struct SlowLexicalUsefulSidecars;

        impl SidecarSearch for SlowLexicalUsefulSidecars {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(350));
                Ok(vec![CandidateHit::with_source(
                    "crates/codestory-cli/src/output.rs",
                    Some("append_search_evidence_packet".into()),
                    0.92,
                    CandidateSource::Lexical,
                )])
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "crates/codestory-contracts/src/api/dto.rs",
                    Some("PacketRetrievalTraceSummaryDto".into()),
                    0.99,
                    CandidateSource::Qdrant,
                )])
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }
        }

        let mut cache = RetrievalCache::new();
        let mut roles = HashMap::new();
        roles.insert(
            "crates/codestory-cli/src/output.rs".to_string(),
            codestory_store::FileRole::Source,
        );
        roles.insert(
            "crates/codestory-contracts/src/api/dto.rs".to_string(),
            codestory_store::FileRole::Source,
        );
        let mut executor = QueryExecutor {
            sidecars: Arc::new(SlowLexicalUsefulSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(roles),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute(
                "packet search output evidence packet indexed symbol hits retrieval shadow",
                Some(1_000),
            )
            .expect("query");

        assert_eq!(result.trace.cancel_reason, None);
        assert!(
            result.trace.stages.iter().any(|stage| {
                stage.stage == RetrievalStageKind::Stage1Lexical
                    && stage.candidates_added == 1
                    && stage.cancel_reason.is_none()
            }),
            "broad lexical source anchor should be admitted before ranking: {:?}",
            result.trace.stages
        );
        assert_eq!(
            result.hits.first().map(|hit| hit.file_path.as_str()),
            Some("crates/codestory-cli/src/output.rs")
        );
    }

    #[test]
    fn non_broad_stage_deadline_remains_blocking_after_later_stages() {
        struct SlowScipAnchorSidecars;

        impl SidecarSearch for SlowScipAnchorSidecars {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/late_lexical.rs",
                    Some("LateLexical".into()),
                    0.99,
                    CandidateSource::Lexical,
                )])
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/anchor.rs",
                    Some("OtherAnchor".into()),
                    0.7,
                    CandidateSource::Scip,
                )])
            }

            fn scip_anchor_with_context(
                &self,
                query: &str,
                limit: usize,
                context: &SearchExecutionContext,
            ) -> Result<Vec<CandidateHit>> {
                while !context.is_cancelled() {
                    std::hint::spin_loop();
                }
                self.scip_anchor(query, limit)
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }
        }

        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(SlowScipAnchorSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("EventProcessor", Some(10)).expect("query");

        assert!(
            result.trace.cancel_reason.as_deref() == Some("stage_deadline"),
            "request deadline should remain fail-closed: {:?}",
            result.trace
        );
        assert!(
            result.trace.stages.is_empty()
                || result
                    .trace
                    .stages
                    .iter()
                    .any(|stage| stage.stage == RetrievalStageKind::Stage0ScipAnchor),
            "a started non-broad stage should be bounded by the same stage deadline as broad work: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "src/anchor.rs"),
            "timed-out anchor hit must not merge late into the partial query: {:?}",
            result.hits
        );
        drop(executor);
        assert!(cache.is_empty(), "partial query must remain uncached");
    }

    #[test]
    fn symbol_like_queries_expand_scip_before_slow_qdrant_can_consume_window() {
        struct SlowDenseSymbolSidecars;

        impl SidecarSearch for SlowDenseSymbolSidecars {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                std::thread::sleep(Duration::from_millis(80));
                Ok(vec![CandidateHit::with_source(
                    "src/dense_distractor.rs",
                    Some("DenseDistractor".into()),
                    0.99,
                    CandidateSource::Qdrant,
                )])
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(vec![CandidateHit::with_source(
                    "src/nearby_anchor.rs",
                    Some("NearbyAnchor".into()),
                    0.7,
                    CandidateSource::Scip,
                )])
            }

            fn scip_expand(
                &self,
                anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                if anchors
                    .iter()
                    .any(|hit| hit.symbol_name.as_deref() == Some("NearbyAnchor"))
                {
                    return Ok(vec![CandidateHit::with_source(
                        "src/expanded_neighbor.rs",
                        Some("ExpandedNeighbor".into()),
                        0.9,
                        CandidateSource::Scip,
                    )]);
                }
                Ok(Vec::new())
            }
        }

        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(SlowDenseSymbolSidecars),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("MissingAnchor", Some(50)).expect("query");

        let expand_index = result
            .trace
            .stages
            .iter()
            .position(|stage| stage.stage == RetrievalStageKind::Stage2ScipExpand)
            .expect("scip expand stage");
        let qdrant_index = result
            .trace
            .stages
            .iter()
            .position(|stage| stage.stage == RetrievalStageKind::Stage1bQdrantSemantic)
            .expect("qdrant stage");
        assert!(
            expand_index < qdrant_index,
            "symbol-like graph expansion must preserve pre-dense ordering: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .any(|hit| hit.file_path == "src/expanded_neighbor.rs"),
            "symbol-like SCIP expansion should not be starved by slow dense search: {:?}",
            result.hits
        );
    }

    #[test]
    fn executor_skips_qdrant_when_policy_selects_zero_dense_anchors() {
        let mock = MockSidecarSearch {
            qdrant: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/semantic.rs",
                    Some("SemanticAnchor".into()),
                    0.8,
                    CandidateSource::Qdrant,
                )],
            )])),
            lexical: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/lexical.rs",
                    Some("LexicalAnchor".into()),
                    0.7,
                    CandidateSource::Lexical,
                )],
            )])),
            ..Default::default()
        };
        let mut manifest = sample_manifest();
        manifest.projection_count = Some(0);
        manifest.dense_projection_count = Some(0);
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(manifest),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("how does startup sequence work", Some(800))
            .expect("query");
        assert!(
            result.trace.stages.iter().any(|stage| stage.stage
                == RetrievalStageKind::Stage1bQdrantSemantic
                && stage.cancel_reason.as_deref() == Some("zero_dense_anchors")),
            "zero dense policy should skip qdrant explicitly: {:?}",
            result.trace.stages
        );
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "src/semantic.rs"),
            "qdrant hits must not be recalled when dense count is zero: {:?}",
            result.hits
        );
    }

    #[test]
    fn executor_merges_duplicate_candidate_provenance() {
        let query = "how extension service starts";
        let mut graph_hit = CandidateHit::with_source(
            "src/service.rs",
            Some("ExtensionService".into()),
            0.75,
            CandidateSource::Scip,
        );
        graph_hit.scip_hop_distance = Some(1);
        let mock = MockSidecarSearch {
            lexical: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/service.rs",
                    Some("ExtensionService".into()),
                    0.70,
                    CandidateSource::Lexical,
                )],
            )])),
            qdrant: Mutex::new(HashMap::from([(
                query.into(),
                vec![CandidateHit::with_source(
                    "src/service.rs",
                    Some("ExtensionService".into()),
                    0.85,
                    CandidateSource::Qdrant,
                )],
            )])),
            scip_expand: Mutex::new(vec![graph_hit]),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute(query, Some(800)).expect("query");
        let hit = result
            .hits
            .iter()
            .find(|hit| hit.file_path == "src/service.rs")
            .expect("merged candidate");
        assert!(
            hit.score > 0.70,
            "merged candidate should keep ranker-adjusted score above lexical-only input: {hit:?}"
        );
        assert!(hit.provenance.iter().any(|label| label == "lexical_source"));
        assert!(hit.provenance.iter().any(|label| label == "graph_neighbor"));
        assert!(hit.provenance.iter().any(|label| label == "dense_anchor"));
        let rank_features = hit.rank_features.as_ref().expect("rank features");
        assert!(rank_features.lexical >= 0.85);
        assert!(rank_features.semantic >= 0.85);
        assert_eq!(rank_features.scip_distance, 0.5);
    }

    #[test]
    fn executor_respects_cancellation() {
        let mock = MockSidecarSearch::default();
        let mut cache = RetrievalCache::new();
        let cancelled = cancellation_flag();
        cancelled.store(true, Ordering::Relaxed);
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled,
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("anything", Some(500)).expect("partial ok");
        assert_eq!(result.trace.cancel_reason.as_deref(), Some("cancelled"));
    }

    #[test]
    fn executor_caps_untrusted_total_budget_before_instant_arithmetic() {
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(MockSidecarSearch::default()),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };

        let result = executor
            .execute("EventProcessor", Some(u64::MAX))
            .expect("huge budget should be capped");

        assert_eq!(result.trace.total_budget_ms, MAX_RETRIEVAL_BUDGET_MS);
    }

    #[test]
    fn cancellation_after_last_stage_marks_result_and_suppresses_cache_write() {
        struct CancelOnReturn {
            cancelled: Arc<AtomicBool>,
        }

        impl SidecarSearch for CancelOnReturn {
            fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }

            fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
                self.cancelled.store(true, Ordering::Release);
                Ok(vec![CandidateHit::with_source(
                    "src/late.rs",
                    Some("EventProcessor".into()),
                    1.0,
                    CandidateSource::Scip,
                )])
            }

            fn scip_expand(
                &self,
                _anchors: &[CandidateHit],
                _limit: usize,
            ) -> Result<Vec<CandidateHit>> {
                Ok(Vec::new())
            }
        }

        let cancelled = cancellation_flag();
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(CancelOnReturn {
                cancelled: Arc::clone(&cancelled),
            }),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled,
            mode_override: Some(RetrievalDegradedMode::Full),
        };

        let result = executor
            .execute("EventProcessor", Some(500))
            .expect("query");

        assert_eq!(result.trace.cancel_reason.as_deref(), Some("cancelled"));
        assert!(
            result.hits.is_empty(),
            "cancelled stage hits must be discarded"
        );
        assert!(cache.is_empty());
    }

    struct CooperativeCancellationSidecars {
        started: mpsc::SyncSender<()>,
    }

    impl CooperativeCancellationSidecars {
        fn wait_for_cancellation(
            &self,
            context: &SearchExecutionContext,
        ) -> Result<Vec<CandidateHit>> {
            self.started.send(()).expect("signal active stage");
            while !context.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            context.check_cancelled()?;
            unreachable!("cancelled contexts return an error")
        }
    }

    impl SidecarSearch for CooperativeCancellationSidecars {
        fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn scip_expand(
            &self,
            _anchors: &[CandidateHit],
            _limit: usize,
        ) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn scip_anchor_with_context(
            &self,
            _query: &str,
            _limit: usize,
            context: &SearchExecutionContext,
        ) -> Result<Vec<CandidateHit>> {
            self.wait_for_cancellation(context)
        }
    }

    #[test]
    fn active_stage_cancellation_is_traced_and_not_cached() {
        let cancelled = cancellation_flag();
        let request_cancelled = Arc::clone(&cancelled);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let handle = std::thread::spawn(move || {
            let mut cache = RetrievalCache::new();
            let mut executor = QueryExecutor {
                sidecars: Arc::new(CooperativeCancellationSidecars {
                    started: started_tx,
                }),
                cache: &mut cache,
                manifest: Some(sample_manifest()),
                file_roles: Arc::new(HashMap::new()),
                cancelled: request_cancelled,
                mode_override: Some(RetrievalDegradedMode::Full),
            };
            let result = executor
                .execute("EventProcessor", Some(500))
                .expect("cancelled work remains diagnostic");
            (result, cache.len())
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("active stage started");
        cancelled.store(true, Ordering::Release);
        let (result, cache_len) = handle.join().expect("query worker");

        assert_eq!(result.trace.cancel_reason.as_deref(), Some("cancelled"));
        assert_eq!(cache_len, 0, "cancelled results must not be cached");
        assert!(result.trace.stages.iter().any(|stage| {
            stage.cancel_reason.as_deref() == Some("cancelled")
                && match stage.completion_status {
                    StageCompletionStatus::PendingAfterDeadline => stage.execution_ms.is_none(),
                    StageCompletionStatus::CompletedLate => stage.execution_ms.is_some(),
                    _ => false,
                }
        }));
    }

    #[test]
    fn timeout_storm_cannot_exceed_worker_and_queue_capacity() {
        let pool = StageWorkerPool::new(1, 1);
        let cancelled = AtomicBool::new(false);
        let (first, _) = pool
            .acquire(Instant::now() + Duration::from_secs(1), &cancelled)
            .expect("first admission");
        let (second, _) = pool
            .acquire(Instant::now() + Duration::from_secs(1), &cancelled)
            .expect("queued admission");
        for _ in 0..32 {
            assert!(
                pool.acquire(Instant::now() + Duration::from_millis(1), &cancelled)
                    .is_none(),
                "one active worker plus one queued job must bound caller admission"
            );
        }
        drop(first);
        assert!(
            pool.acquire(Instant::now() + Duration::from_secs(1), &cancelled)
                .is_some()
        );
        drop(second);
    }

    #[test]
    fn incomplete_stage_metrics_do_not_invent_queue_or_execution_duration() {
        let queued = StageJobState::default();
        let before_start = cancelled_stage_run("stage_deadline", 7, &queued);
        assert!(matches!(
            before_start,
            StageRun::Cancelled {
                admission_wait_ms: 7,
                queue_wait_ms: None,
                execution_ms: None,
                completion_status: StageCompletionStatus::CancelledBeforeStart,
                ..
            }
        ));

        queued.queue_wait_ms.store(11, Ordering::Release);
        queued.started.store(true, Ordering::Release);
        let executing = cancelled_stage_run("stage_deadline", 3, &queued);
        assert!(matches!(
            executing,
            StageRun::Cancelled {
                admission_wait_ms: 3,
                queue_wait_ms: Some(11),
                execution_ms: None,
                completion_status: StageCompletionStatus::PendingAfterDeadline,
                ..
            }
        ));
    }

    #[test]
    fn stage_worker_survives_panicking_job() {
        let pool = StageWorkerPool::new(1, 1);
        let cancelled = AtomicBool::new(false);
        for should_panic in [true, false] {
            let (permit, _) = pool
                .acquire(Instant::now() + Duration::from_secs(1), &cancelled)
                .expect("job admission");
            let (sender, receiver) = mpsc::channel();
            pool.sender
                .send(StageJob {
                    queued_at: Instant::now(),
                    state: Arc::new(StageJobState::default()),
                    task: Box::new(move || {
                        assert!(!should_panic, "intentional worker panic");
                        Ok(Vec::new())
                    }),
                    sender,
                    _permit: permit,
                })
                .expect("submit job");
            let completion = receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("worker completion");
            assert_eq!(completion.result.is_err(), should_panic);
        }
    }

    #[test]
    fn executor_enriches_file_role_before_ranking() {
        let mock = MockSidecarSearch {
            lexical: Mutex::new(HashMap::from([(
                "startup".into(),
                vec![
                    CandidateHit::with_source("src/main.rs", None, 0.55, CandidateSource::Lexical),
                    CandidateHit::with_source(
                        "src\\boot_test.rs",
                        None,
                        0.80,
                        CandidateSource::Lexical,
                    ),
                ],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut roles = HashMap::new();
        roles.insert(
            "src/main.rs".to_string(),
            codestory_store::FileRole::Entrypoint,
        );
        roles.insert(
            "src/boot_test.rs".to_string(),
            codestory_store::FileRole::Test,
        );
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(roles),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("startup", Some(500)).expect("query");
        let role_by_path = result
            .hits
            .iter()
            .map(|hit| (hit.file_path.as_str(), hit.file_role))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            role_by_path.get("src/main.rs").copied().flatten(),
            Some(codestory_store::FileRole::Entrypoint)
        );
        assert_eq!(
            role_by_path.get("src\\boot_test.rs").copied().flatten(),
            Some(codestory_store::FileRole::Test)
        );
    }

    #[test]
    fn executor_infers_file_role_when_storage_lookup_misses() {
        let mock = MockSidecarSearch {
            lexical: Mutex::new(HashMap::from([(
                "startup".into(),
                vec![CandidateHit::with_source(
                    "fixtures/generated/boot_test.rs",
                    None,
                    0.80,
                    CandidateSource::Lexical,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor.execute("startup", Some(500)).expect("query");
        assert_eq!(
            result.hits.first().and_then(|hit| hit.file_role),
            Some(codestory_store::FileRole::Generated)
        );
    }

    struct TrackingSidecars {
        layout: SidecarLayout,
        layout_calls: AtomicUsize,
    }

    impl SidecarSearch for TrackingSidecars {
        fn layout(&self) -> Option<&SidecarLayout> {
            self.layout_calls.fetch_add(1, Ordering::Relaxed);
            Some(&self.layout)
        }

        fn lexical_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn qdrant_search(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn scip_anchor(&self, _query: &str, _limit: usize) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }

        fn scip_expand(
            &self,
            _anchors: &[CandidateHit],
            _limit: usize,
        ) -> Result<Vec<CandidateHit>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn executor_does_not_use_repo_text_diagnostic_for_natural_language_queries() {
        let mock = MockSidecarSearch {
            lexical: Mutex::new(HashMap::from([(
                "how does startup sequence work".into(),
                vec![CandidateHit::with_source(
                    "src/main.rs",
                    None,
                    0.7,
                    CandidateSource::Lexical,
                )],
            )])),
            ..Default::default()
        };
        let mut cache = RetrievalCache::new();
        let mut executor = QueryExecutor {
            sidecars: Arc::new(mock),
            cache: &mut cache,
            manifest: Some(sample_manifest()),
            file_roles: Arc::new(HashMap::new()),
            cancelled: cancellation_flag(),
            mode_override: Some(RetrievalDegradedMode::Full),
        };
        let result = executor
            .execute("how does startup sequence work", Some(500))
            .expect("query");
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.file_path != "docs/startup.md")
        );
        assert!(
            result
                .trace
                .stages
                .iter()
                .all(|stage| stage.stage != RetrievalStageKind::Stage3RepoTextFallback)
        );
    }
}
