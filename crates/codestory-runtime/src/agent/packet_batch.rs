//! Packet batch retrieval orchestration for missing material proof.
#![allow(clippy::items_after_test_module)]

use super::packet_candidate::PacketSearchHit;
use super::packet_plan::packet_plan_query_is_typed_free_query;
use super::packet_scoring::{
    normalize_identifier, packet_stage_citation_carry_limit, packet_subquery_hit_limit,
};
use super::packet_trace::merge_packet_fused_subquery_batch;
use crate::{AppController, clamp_u128_to_u32};
use codestory_contracts::api::{
    AgentAnswerDto, AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, ApiError,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto,
    PacketSidecarQueryDiagnosticDto, RetrievalAnnotationDto,
};
use std::cell::Cell;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Instant;

const DEFAULT_SLA_TARGET_MS: u32 = 18_000;
const MIN_PACKET_HANDOFF_MS: u128 = 1_000;
#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketLatencyBudget {
    pub(crate) started_at: Instant,
    pub(crate) target_ms: u128,
}

thread_local! {
    static ACTIVE_PACKET_LATENCY_BUDGET: Cell<Option<PacketLatencyBudget>> = const {
        Cell::new(None)
    };
    static ACTIVE_PACKET_ENTRY_OBSERVATION: Cell<Option<PacketEntryObservation>> = const {
        Cell::new(None)
    };
    static PACKET_PUBLIC_OPERATION_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PACKET_PUBLIC_OPERATION_OWNER_DEPTH: Cell<u32> = const { Cell::new(0) };
}

static NEXT_PACKET_ENTRY_OBSERVATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, Default)]
struct PacketEntryObservation {
    id: u64,
    target_ms: u64,
    phase_mask: u64,
    activation_branch_mask: u64,
    activation_join_count: u64,
    activation_join_ms: u64,
    ready_probe_count: u64,
    ready_probe_total_ms: u64,
    post_probe_join_count: u64,
    project_selection_started_ms: u64,
    project_selection_completed_ms: u64,
    activation_started_ms: u64,
    ready_probe_started_ms: u64,
    ready_probe_configuration_ms: u64,
    ready_probe_retrieval_ms: u64,
    ready_probe_core_ms: u64,
    ready_probe_source_ms: u64,
    ready_probe_completed_ms: u64,
    post_probe_join_ms: u64,
    activation_returned_ms: u64,
    source_scope_started_ms: u64,
    source_scope_completed_ms: u64,
    public_admission_check_ms: u64,
    public_admission_reached_count: u64,
    public_admission_passed_count: u64,
    public_admission_refused_count: u64,
    last_public_admission_ms: u64,
    attempt_started_count: u64,
    retry_publication_changed_count: u64,
    retry_cache_busy_count: u64,
    complete_core_snapshot: PacketOperationSpanObservation,
    uncached_freshness: PacketOperationSpanObservation,
    retrieval_pin: PacketOperationSpanObservation,
    build_callback: PacketOperationSpanObservation,
    post_build_freshness: PacketOperationSpanObservation,
    pin_begin: PacketOperationSpanObservation,
    pin_revalidation: PacketOperationSpanObservation,
}

#[derive(Debug, Clone, Copy, Default)]
struct PacketOperationSpanObservation {
    started_count: u64,
    succeeded_count: u64,
    total_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PacketOperationObservationSpan {
    CompleteCoreSnapshot,
    UncachedFreshness,
    RetrievalPin,
    BuildCallback,
    PostBuildFreshness,
    PinBegin,
    PinRevalidation,
}

fn packet_operation_span_mut(
    observation: &mut PacketEntryObservation,
    span: PacketOperationObservationSpan,
) -> &mut PacketOperationSpanObservation {
    match span {
        PacketOperationObservationSpan::CompleteCoreSnapshot => {
            &mut observation.complete_core_snapshot
        }
        PacketOperationObservationSpan::UncachedFreshness => &mut observation.uncached_freshness,
        PacketOperationObservationSpan::RetrievalPin => &mut observation.retrieval_pin,
        PacketOperationObservationSpan::BuildCallback => &mut observation.build_callback,
        PacketOperationObservationSpan::PostBuildFreshness => &mut observation.post_build_freshness,
        PacketOperationObservationSpan::PinBegin => &mut observation.pin_begin,
        PacketOperationObservationSpan::PinRevalidation => &mut observation.pin_revalidation,
    }
}

fn packet_operation_observation_is_owned() -> bool {
    PACKET_PUBLIC_OPERATION_DEPTH.with(|depth| {
        PACKET_PUBLIC_OPERATION_OWNER_DEPTH.with(|owner| {
            let depth = depth.get();
            depth != 0 && owner.get() == depth
        })
    })
}

fn update_packet_operation_observation(update: impl FnOnce(&mut PacketEntryObservation)) {
    if !packet_operation_observation_is_owned() {
        return;
    }
    ACTIVE_PACKET_ENTRY_OBSERVATION.with(|active| {
        if let Some(mut observation) = active.get() {
            update(&mut observation);
            active.set(Some(observation));
        }
    });
}

pub(crate) struct PacketPublicOperationObservationGuard {
    previous_depth: u32,
    previous_owner_depth: u32,
    active: bool,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Drop for PacketPublicOperationObservationGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        PACKET_PUBLIC_OPERATION_OWNER_DEPTH.with(|owner| owner.set(self.previous_owner_depth));
        PACKET_PUBLIC_OPERATION_DEPTH.with(|depth| depth.set(self.previous_depth));
    }
}

pub(crate) fn enter_packet_public_operation_observation(
    operation: &str,
) -> PacketPublicOperationObservationGuard {
    let active = ACTIVE_PACKET_ENTRY_OBSERVATION.with(|observation| observation.get().is_some());
    if !active {
        return PacketPublicOperationObservationGuard {
            previous_depth: 0,
            previous_owner_depth: 0,
            active: false,
            _thread_bound: PhantomData,
        };
    }
    let previous_depth = PACKET_PUBLIC_OPERATION_DEPTH.with(|depth| {
        let previous = depth.get();
        depth.set(previous.saturating_add(1));
        previous
    });
    let previous_owner_depth = PACKET_PUBLIC_OPERATION_OWNER_DEPTH.with(Cell::get);
    if operation == "packet" && previous_owner_depth == 0 {
        PACKET_PUBLIC_OPERATION_OWNER_DEPTH.with(|owner| owner.set(previous_depth + 1));
    }
    PacketPublicOperationObservationGuard {
        previous_depth,
        previous_owner_depth,
        active: true,
        _thread_bound: PhantomData,
    }
}

pub(crate) fn observe_packet_public_admission_reached() {
    let elapsed_ms = active_packet_latency_budget()
        .map(|budget| clamp_u128_to_u32(budget.started_at.elapsed().as_millis()) as u64)
        .unwrap_or(0);
    update_packet_operation_observation(|observation| {
        observation.public_admission_reached_count =
            observation.public_admission_reached_count.saturating_add(1);
        if observation.public_admission_reached_count == 1 {
            observation.public_admission_check_ms = elapsed_ms;
        }
        observation.last_public_admission_ms = elapsed_ms;
    });
}

pub(crate) fn observe_packet_public_admission_passed() {
    update_packet_operation_observation(|observation| {
        observation.public_admission_passed_count =
            observation.public_admission_passed_count.saturating_add(1);
    });
}

pub(crate) fn observe_packet_public_admission_refused() {
    update_packet_operation_observation(|observation| {
        observation.public_admission_refused_count =
            observation.public_admission_refused_count.saturating_add(1);
    });
}

pub(crate) fn observe_packet_attempt_started() {
    update_packet_operation_observation(|observation| {
        observation.attempt_started_count = observation.attempt_started_count.saturating_add(1);
    });
}

pub(crate) fn observe_packet_retry_cause(code: &str) {
    update_packet_operation_observation(|observation| match code {
        "publication_changed" => {
            observation.retry_publication_changed_count = observation
                .retry_publication_changed_count
                .saturating_add(1);
        }
        "cache_busy" => {
            observation.retry_cache_busy_count =
                observation.retry_cache_busy_count.saturating_add(1);
        }
        _ => {}
    });
}

pub(crate) struct PacketOperationObservationSpanGuard {
    span: PacketOperationObservationSpan,
    started_at: Option<Instant>,
    active: bool,
    _thread_bound: PhantomData<Rc<()>>,
}

impl PacketOperationObservationSpanGuard {
    pub(crate) fn finish_success(&mut self) {
        if self.active {
            self.finish(true);
        }
    }

    fn finish(&mut self, succeeded: bool) {
        let Some(started_at) = self.started_at.take() else {
            self.active = false;
            return;
        };
        let elapsed_ms = clamp_u128_to_u32(started_at.elapsed().as_millis()) as u64;
        update_packet_operation_observation(|observation| {
            let span = packet_operation_span_mut(observation, self.span);
            span.total_ms = span.total_ms.saturating_add(elapsed_ms);
            if succeeded {
                span.succeeded_count = span.succeeded_count.saturating_add(1);
            }
        });
        self.active = false;
    }
}

impl Drop for PacketOperationObservationSpanGuard {
    fn drop(&mut self) {
        if self.active {
            self.finish(false);
        }
    }
}

pub(crate) fn observe_packet_operation_span(
    span: PacketOperationObservationSpan,
) -> PacketOperationObservationSpanGuard {
    let active = packet_operation_observation_is_owned();
    if active {
        update_packet_operation_observation(|observation| {
            let span = packet_operation_span_mut(observation, span);
            span.started_count = span.started_count.saturating_add(1);
        });
    }
    PacketOperationObservationSpanGuard {
        span,
        started_at: active.then(Instant::now),
        active,
        _thread_bound: PhantomData,
    }
}

/// Fixed request-side boundaries retained by the active packet allowance.
///
/// The diagnostic receipt contains only numeric timings and branch flags. It
/// never records a project path, query, source text, or request payload.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum PacketEntryObservationPhase {
    ProjectSelectionStarted = 0,
    ProjectSelectionCompleted = 1,
    ActivationStarted = 2,
    ActivationJoinedRunning = 3,
    ActivationReadyProbeStarted = 4,
    ReadyProbeConfigurationCompleted = 5,
    ReadyProbeRetrievalCompleted = 6,
    ReadyProbeCoreCompleted = 7,
    ReadyProbeSourceCompleted = 8,
    ActivationReadyProbeCompleted = 9,
    ActivationPostProbeJoin = 10,
    ActivationStartedWorker = 11,
    ActivationReturned = 12,
    SourceScopeStarted = 13,
    SourceScopeCompleted = 14,
    PublicAdmissionCheck = 15,
}

/// Record one request-side boundary against the active packet allowance.
/// Non-packet callers have no active observation and pay only a TLS lookup.
#[doc(hidden)]
pub fn observe_packet_entry_phase(phase: PacketEntryObservationPhase) {
    let Some(packet_latency) = active_packet_latency_budget() else {
        return;
    };
    let elapsed_ms = clamp_u128_to_u32(packet_latency.started_at.elapsed().as_millis()) as u64;
    ACTIVE_PACKET_ENTRY_OBSERVATION.with(|active| {
        let Some(mut observation) = active.get() else {
            return;
        };
        let bit = 1_u64 << (phase as u8);
        let was_recorded = observation.phase_mask & bit != 0;
        observation.phase_mask |= bit;
        match phase {
            PacketEntryObservationPhase::ProjectSelectionStarted => {
                if !was_recorded {
                    observation.project_selection_started_ms = elapsed_ms;
                }
            }
            PacketEntryObservationPhase::ProjectSelectionCompleted => {
                if !was_recorded {
                    observation.project_selection_completed_ms = elapsed_ms;
                }
            }
            PacketEntryObservationPhase::ActivationStarted => {
                if !was_recorded {
                    observation.activation_started_ms = elapsed_ms;
                }
            }
            PacketEntryObservationPhase::ActivationJoinedRunning => {
                observation.activation_branch_mask |= 1;
                observation.activation_join_count =
                    observation.activation_join_count.saturating_add(1);
                if !was_recorded {
                    observation.activation_join_ms = elapsed_ms;
                }
            }
            PacketEntryObservationPhase::ActivationReadyProbeStarted => {
                observation.activation_branch_mask |= 2;
                observation.ready_probe_count = observation.ready_probe_count.saturating_add(1);
                observation.ready_probe_started_ms = elapsed_ms;
            }
            PacketEntryObservationPhase::ReadyProbeConfigurationCompleted => {
                observation.ready_probe_configuration_ms = elapsed_ms;
            }
            PacketEntryObservationPhase::ReadyProbeRetrievalCompleted => {
                observation.ready_probe_retrieval_ms = elapsed_ms;
            }
            PacketEntryObservationPhase::ReadyProbeCoreCompleted => {
                observation.ready_probe_core_ms = elapsed_ms;
            }
            PacketEntryObservationPhase::ReadyProbeSourceCompleted => {
                observation.ready_probe_source_ms = elapsed_ms;
            }
            PacketEntryObservationPhase::ActivationReadyProbeCompleted => {
                observation.ready_probe_total_ms = observation
                    .ready_probe_total_ms
                    .saturating_add(elapsed_ms.saturating_sub(observation.ready_probe_started_ms));
                observation.ready_probe_completed_ms = elapsed_ms;
            }
            PacketEntryObservationPhase::ActivationPostProbeJoin => {
                observation.activation_branch_mask |= 4;
                observation.post_probe_join_count =
                    observation.post_probe_join_count.saturating_add(1);
                observation.post_probe_join_ms = elapsed_ms;
            }
            PacketEntryObservationPhase::ActivationStartedWorker => {
                observation.activation_branch_mask |= 8;
            }
            PacketEntryObservationPhase::ActivationReturned => {
                if !was_recorded {
                    observation.activation_returned_ms = elapsed_ms;
                }
            }
            PacketEntryObservationPhase::SourceScopeStarted => {
                if !was_recorded {
                    observation.source_scope_started_ms = elapsed_ms;
                }
            }
            PacketEntryObservationPhase::SourceScopeCompleted => {
                if !was_recorded {
                    observation.source_scope_completed_ms = elapsed_ms;
                }
            }
            PacketEntryObservationPhase::PublicAdmissionCheck => {
                if !was_recorded {
                    observation.public_admission_check_ms = elapsed_ms;
                }
            }
        }
        active.set(Some(observation));
    });
}

/// Restores the packet allowance that was active before this synchronous scope.
///
/// The guard is deliberately thread-bound because it restores thread-local state.
#[doc(hidden)]
pub struct PacketLatencyScopeGuard {
    previous: Option<PacketLatencyBudget>,
    owns_observation: bool,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Drop for PacketLatencyScopeGuard {
    fn drop(&mut self) {
        let total_elapsed_ms = active_packet_latency_budget()
            .map(|budget| clamp_u128_to_u32(budget.started_at.elapsed().as_millis()) as u64)
            .unwrap_or(0);
        ACTIVE_PACKET_LATENCY_BUDGET.with(|active| active.set(self.previous));
        if self.owns_observation {
            let observation = ACTIVE_PACKET_ENTRY_OBSERVATION.with(|active| active.take());
            if let Some(observation) = observation {
                tracing::warn!(
                    packet_entry_observation_id = observation.id,
                    target_ms = observation.target_ms,
                    total_elapsed_ms,
                    phase_mask = observation.phase_mask,
                    activation_branch_mask = observation.activation_branch_mask,
                    activation_join_count = observation.activation_join_count,
                    activation_join_ms = observation.activation_join_ms,
                    ready_probe_count = observation.ready_probe_count,
                    ready_probe_total_ms = observation.ready_probe_total_ms,
                    post_probe_join_count = observation.post_probe_join_count,
                    project_selection_started_ms = observation.project_selection_started_ms,
                    project_selection_completed_ms = observation.project_selection_completed_ms,
                    activation_started_ms = observation.activation_started_ms,
                    ready_probe_started_ms = observation.ready_probe_started_ms,
                    ready_probe_configuration_ms = observation.ready_probe_configuration_ms,
                    ready_probe_retrieval_ms = observation.ready_probe_retrieval_ms,
                    ready_probe_core_ms = observation.ready_probe_core_ms,
                    ready_probe_source_ms = observation.ready_probe_source_ms,
                    ready_probe_completed_ms = observation.ready_probe_completed_ms,
                    post_probe_join_ms = observation.post_probe_join_ms,
                    activation_returned_ms = observation.activation_returned_ms,
                    source_scope_started_ms = observation.source_scope_started_ms,
                    source_scope_completed_ms = observation.source_scope_completed_ms,
                    public_admission_check_ms = observation.public_admission_check_ms,
                    last_public_admission_ms = observation.last_public_admission_ms,
                    public_admission_reached_count = observation.public_admission_reached_count,
                    public_admission_passed_count = observation.public_admission_passed_count,
                    public_admission_refused_count = observation.public_admission_refused_count,
                    attempt_started_count = observation.attempt_started_count,
                    retry_publication_changed_count = observation.retry_publication_changed_count,
                    retry_cache_busy_count = observation.retry_cache_busy_count,
                    complete_core_snapshot_started_count =
                        observation.complete_core_snapshot.started_count,
                    complete_core_snapshot_succeeded_count =
                        observation.complete_core_snapshot.succeeded_count,
                    complete_core_snapshot_ms = observation.complete_core_snapshot.total_ms,
                    uncached_freshness_started_count = observation.uncached_freshness.started_count,
                    uncached_freshness_succeeded_count =
                        observation.uncached_freshness.succeeded_count,
                    uncached_freshness_ms = observation.uncached_freshness.total_ms,
                    retrieval_pin_started_count = observation.retrieval_pin.started_count,
                    retrieval_pin_succeeded_count = observation.retrieval_pin.succeeded_count,
                    retrieval_pin_ms = observation.retrieval_pin.total_ms,
                    build_callback_started_count = observation.build_callback.started_count,
                    build_callback_succeeded_count = observation.build_callback.succeeded_count,
                    build_callback_ms = observation.build_callback.total_ms,
                    post_build_freshness_started_count =
                        observation.post_build_freshness.started_count,
                    post_build_freshness_succeeded_count =
                        observation.post_build_freshness.succeeded_count,
                    post_build_freshness_ms = observation.post_build_freshness.total_ms,
                    pin_begin_started_count = observation.pin_begin.started_count,
                    pin_begin_succeeded_count = observation.pin_begin.succeeded_count,
                    pin_begin_ms = observation.pin_begin.total_ms,
                    pin_revalidation_started_count = observation.pin_revalidation.started_count,
                    pin_revalidation_succeeded_count = observation.pin_revalidation.succeeded_count,
                    pin_revalidation_ms = observation.pin_revalidation.total_ms,
                    "packet entry observation"
                );
            }
        }
    }
}

/// Start one packet allowance unless an outer packet entry already owns it.
///
/// Nested public-operation wrappers inherit the outer start instant. This is a
/// runtime integration surface for adapters; the request DTO remains unchanged.
#[doc(hidden)]
pub fn enter_packet_latency_scope(requested_ms: Option<u32>) -> PacketLatencyScopeGuard {
    ACTIVE_PACKET_LATENCY_BUDGET.with(|active| {
        let previous = active.get();
        let budget = previous.unwrap_or_else(|| PacketLatencyBudget::new(requested_ms));
        active.set(Some(budget));
        let owns_observation = previous.is_none();
        if owns_observation {
            ACTIVE_PACKET_ENTRY_OBSERVATION.with(|observation| {
                observation.set(Some(PacketEntryObservation {
                    id: NEXT_PACKET_ENTRY_OBSERVATION_ID.fetch_add(1, AtomicOrdering::Relaxed),
                    target_ms: clamp_u128_to_u32(budget.target_ms) as u64,
                    ..PacketEntryObservation::default()
                }));
            });
        }
        PacketLatencyScopeGuard {
            previous,
            owns_observation,
            _thread_bound: PhantomData,
        }
    })
}

pub(crate) fn active_packet_latency_budget() -> Option<PacketLatencyBudget> {
    ACTIVE_PACKET_LATENCY_BUDGET.with(Cell::get)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PacketOperationObservationTestSnapshot {
    pub(crate) public_admission_reached_count: u64,
    pub(crate) public_admission_passed_count: u64,
    pub(crate) public_admission_refused_count: u64,
    pub(crate) first_public_admission_ms: u64,
    pub(crate) last_public_admission_ms: u64,
    pub(crate) attempt_started_count: u64,
    pub(crate) retry_publication_changed_count: u64,
    pub(crate) retry_cache_busy_count: u64,
    pub(crate) complete_core_snapshot_started_count: u64,
    pub(crate) complete_core_snapshot_succeeded_count: u64,
    pub(crate) uncached_freshness_started_count: u64,
    pub(crate) uncached_freshness_succeeded_count: u64,
    pub(crate) retrieval_pin_started_count: u64,
    pub(crate) retrieval_pin_succeeded_count: u64,
    pub(crate) build_callback_started_count: u64,
    pub(crate) build_callback_succeeded_count: u64,
    pub(crate) post_build_freshness_started_count: u64,
    pub(crate) post_build_freshness_succeeded_count: u64,
    pub(crate) pin_begin_started_count: u64,
    pub(crate) pin_begin_succeeded_count: u64,
    pub(crate) pin_revalidation_started_count: u64,
    pub(crate) pin_revalidation_succeeded_count: u64,
}

#[cfg(test)]
pub(crate) fn packet_operation_observation_for_test()
-> Option<PacketOperationObservationTestSnapshot> {
    ACTIVE_PACKET_ENTRY_OBSERVATION.with(|active| {
        active
            .get()
            .map(|observation| PacketOperationObservationTestSnapshot {
                public_admission_reached_count: observation.public_admission_reached_count,
                public_admission_passed_count: observation.public_admission_passed_count,
                public_admission_refused_count: observation.public_admission_refused_count,
                first_public_admission_ms: observation.public_admission_check_ms,
                last_public_admission_ms: observation.last_public_admission_ms,
                attempt_started_count: observation.attempt_started_count,
                retry_publication_changed_count: observation.retry_publication_changed_count,
                retry_cache_busy_count: observation.retry_cache_busy_count,
                complete_core_snapshot_started_count: observation
                    .complete_core_snapshot
                    .started_count,
                complete_core_snapshot_succeeded_count: observation
                    .complete_core_snapshot
                    .succeeded_count,
                uncached_freshness_started_count: observation.uncached_freshness.started_count,
                uncached_freshness_succeeded_count: observation.uncached_freshness.succeeded_count,
                retrieval_pin_started_count: observation.retrieval_pin.started_count,
                retrieval_pin_succeeded_count: observation.retrieval_pin.succeeded_count,
                build_callback_started_count: observation.build_callback.started_count,
                build_callback_succeeded_count: observation.build_callback.succeeded_count,
                post_build_freshness_started_count: observation.post_build_freshness.started_count,
                post_build_freshness_succeeded_count: observation
                    .post_build_freshness
                    .succeeded_count,
                pin_begin_started_count: observation.pin_begin.started_count,
                pin_begin_succeeded_count: observation.pin_begin.succeeded_count,
                pin_revalidation_started_count: observation.pin_revalidation.started_count,
                pin_revalidation_succeeded_count: observation.pin_revalidation.succeeded_count,
            })
    })
}

impl PacketLatencyBudget {
    pub(crate) fn new(requested_ms: Option<u32>) -> Self {
        Self {
            started_at: Instant::now(),
            target_ms: requested_ms
                .unwrap_or(DEFAULT_SLA_TARGET_MS)
                .clamp(1_000, 120_000) as u128,
        }
    }

    pub(crate) fn inherited_or_new(requested_ms: Option<u32>) -> Self {
        active_packet_latency_budget().unwrap_or_else(|| Self::new(requested_ms))
    }

    fn elapsed_ms(&self) -> u128 {
        self.started_at.elapsed().as_millis()
    }

    pub(crate) fn exhausted(&self) -> bool {
        self.elapsed_ms() >= self.target_ms
    }

    pub(crate) fn remaining_for_handoff(self) -> Option<u32> {
        let remaining_ms = self.target_ms.saturating_sub(self.elapsed_ms());
        (remaining_ms >= MIN_PACKET_HANDOFF_MS).then(|| clamp_u128_to_u32(remaining_ms))
    }

    pub(crate) fn apply_to_trace(self, answer: &mut AgentAnswerDto) {
        answer.retrieval_trace.sla_target_ms = Some(clamp_u128_to_u32(self.target_ms));
        if (answer.retrieval_trace.total_latency_ms as u128) > self.target_ms || self.exhausted() {
            answer.retrieval_trace.sla_missed = true;
        }
    }
}

#[cfg(test)]
mod packet_latency_budget_tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;

    #[test]
    fn packet_budget_handoff_charges_elapsed_work_and_refuses_an_exhausted_floor() {
        let partially_spent = PacketLatencyBudget {
            started_at: Instant::now()
                .checked_sub(Duration::from_millis(700))
                .expect("backdate packet start"),
            target_ms: 2_000,
        };
        let remaining = partially_spent
            .remaining_for_handoff()
            .expect("partially spent packet allowance");
        assert!(
            (1_250..=1_300).contains(&remaining),
            "descriptor elapsed time must reduce the downstream allowance: {remaining}"
        );

        let below_retrieval_minimum = PacketLatencyBudget {
            started_at: Instant::now()
                .checked_sub(Duration::from_millis(700))
                .expect("backdate sub-minimum packet start"),
            target_ms: 1_000,
        };
        assert_eq!(
            below_retrieval_minimum.remaining_for_handoff(),
            None,
            "a sub-minimum remainder must stop before downstream retrieval instead of granting a fresh 1000 ms phase"
        );

        let exhausted = PacketLatencyBudget {
            started_at: Instant::now()
                .checked_sub(Duration::from_millis(1_001))
                .expect("backdate exhausted packet start"),
            target_ms: 1_000,
        };
        assert_eq!(
            exhausted.remaining_for_handoff(),
            None,
            "an exhausted packet must stop before a downstream stage instead of renewing the 1000 ms floor"
        );
    }

    #[test]
    fn packet_latency_scope_inherits_outer_identity_and_restores_after_unwind() {
        assert!(active_packet_latency_budget().is_none());
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let _outer = enter_packet_latency_scope(Some(2_000));
            let outer = active_packet_latency_budget().expect("outer packet allowance");
            assert_eq!(outer.target_ms, 2_000);

            {
                let _inner = enter_packet_latency_scope(Some(120_000));
                let inherited = active_packet_latency_budget().expect("inherited allowance");
                assert_eq!(inherited.started_at, outer.started_at);
                assert_eq!(inherited.target_ms, outer.target_ms);
            }
            let restored = active_packet_latency_budget().expect("restored outer allowance");
            assert_eq!(restored.started_at, outer.started_at);
            assert_eq!(restored.target_ms, outer.target_ms);
            panic!("exercise packet allowance unwind cleanup");
        }));
        assert!(unwind.is_err());
        assert!(
            active_packet_latency_budget().is_none(),
            "unwind must restore the prior thread-local packet allowance"
        );

        let _independent = enter_packet_latency_scope(None);
        assert_eq!(
            active_packet_latency_budget()
                .expect("independent packet allowance")
                .target_ms,
            DEFAULT_SLA_TARGET_MS as u128
        );
    }

    #[test]
    fn packet_entry_observation_is_outer_scope_correlated_and_nonpacket_safe() {
        observe_packet_entry_phase(PacketEntryObservationPhase::ProjectSelectionStarted);
        observe_packet_public_admission_reached();
        observe_packet_public_admission_passed();
        observe_packet_attempt_started();
        observe_packet_retry_cause("publication_changed");
        drop(observe_packet_operation_span(
            PacketOperationObservationSpan::BuildCallback,
        ));
        assert!(ACTIVE_PACKET_ENTRY_OBSERVATION.with(Cell::get).is_none());

        {
            let _outer = enter_packet_latency_scope(Some(2_000));
            observe_packet_entry_phase(PacketEntryObservationPhase::ProjectSelectionStarted);
            let first = ACTIVE_PACKET_ENTRY_OBSERVATION
                .with(Cell::get)
                .expect("outer packet observation");
            assert_ne!(first.id, 0);

            {
                let _nested = enter_packet_latency_scope(Some(120_000));
                observe_packet_entry_phase(PacketEntryObservationPhase::ActivationJoinedRunning);
                let nested = ACTIVE_PACKET_ENTRY_OBSERVATION
                    .with(Cell::get)
                    .expect("nested packet observation");
                assert_eq!(nested.id, first.id);
                assert_eq!(nested.target_ms, 2_000);
                assert_eq!(nested.activation_join_count, 1);
            }

            assert_eq!(
                ACTIVE_PACKET_ENTRY_OBSERVATION
                    .with(Cell::get)
                    .expect("restored packet observation")
                    .id,
                first.id
            );
        }
        assert!(ACTIVE_PACKET_ENTRY_OBSERVATION.with(Cell::get).is_none());
    }

    #[test]
    fn packet_operation_observation_is_owned_by_the_outer_packet_operation() {
        let _latency = enter_packet_latency_scope(Some(2_000));
        let unwind = catch_unwind(AssertUnwindSafe(|| {
            let _outer = enter_packet_public_operation_observation("packet");
            observe_packet_public_admission_reached();
            observe_packet_public_admission_passed();
            observe_packet_attempt_started();
            {
                let _nested = enter_packet_public_operation_observation("packet");
                observe_packet_public_admission_reached();
                observe_packet_public_admission_refused();
                observe_packet_attempt_started();
                observe_packet_retry_cause("cache_busy");
                let mut nested =
                    observe_packet_operation_span(PacketOperationObservationSpan::BuildCallback);
                nested.finish_success();
            }
            let mut outer =
                observe_packet_operation_span(PacketOperationObservationSpan::BuildCallback);
            outer.finish_success();
            panic!("exercise packet operation observation unwind");
        }));
        assert!(unwind.is_err());

        let observation = packet_operation_observation_for_test()
            .expect("outer packet entry observation survives operation unwind");
        assert_eq!(observation.public_admission_reached_count, 1);
        assert_eq!(observation.public_admission_passed_count, 1);
        assert_eq!(observation.public_admission_refused_count, 0);
        assert_eq!(observation.attempt_started_count, 1);
        assert_eq!(observation.retry_cache_busy_count, 0);
        assert_eq!(observation.build_callback_started_count, 1);
        assert_eq!(observation.build_callback_succeeded_count, 1);

        {
            let _next = enter_packet_public_operation_observation("packet");
            observe_packet_public_admission_reached();
        }
        let observation = packet_operation_observation_for_test()
            .expect("same outer entry observation remains active");
        assert_eq!(observation.public_admission_reached_count, 2);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_packet_planned_subqueries(
    controller: &AppController,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    include_evidence: bool,
    packet_latency: PacketLatencyBudget,
    answer: &mut AgentAnswerDto,
) -> Result<(), ApiError> {
    let limit = packet_subquery_limit(budget);
    if limit == 0 {
        // Planned subqueries never ran, so their evidence is genuinely absent.
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::gap(
                "packet_subqueries skipped budget=tiny",
            ));
        return Ok(());
    }

    let adaptive_queries = packet_free_queries(plan, answer, limit);
    let pending = adaptive_queries
        .iter()
        .enumerate()
        .map(|(index, query)| (plan.queries.len().saturating_add(index), query))
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }
    let Some(remaining_ms) = packet_latency.remaining_for_handoff() else {
        answer.retrieval_trace.sla_missed = true;
        answer
            .retrieval_trace
            .annotations
            .push(RetrievalAnnotationDto::gap(format!(
                "packet_material_queries skipped reason=latency_budget_exhausted count={}",
                pending.len()
            )));
        return Ok(());
    };

    let per_query_limit = packet_subquery_hit_limit(limits);
    let stage_carry_limit = packet_stage_citation_carry_limit(limits);
    let batch = pending
        .iter()
        .map(|(_, query)| (query.query.clone(), per_query_limit))
        .collect::<Vec<_>>();
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(format!(
            "packet_material_queries fused_batch={} total={}",
            batch.len(),
            pending.len()
        )));

    let started_at = Instant::now();
    let outcome = match controller.search_packet_fused_batch(&batch, Some(remaining_ms)) {
        Ok(outcome) => outcome,
        Err(error) => {
            answer
                .retrieval_trace
                .annotations
                .push(RetrievalAnnotationDto::gap(format!(
                    "packet_fused_subquery_batch_failed error={error:?}"
                )));
            return Err(error);
        }
    };
    let duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
    answer.retrieval_trace.total_latency_ms = answer
        .retrieval_trace
        .total_latency_ms
        .saturating_add(duration_ms);
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .extend(outcome.sidecar_diagnostics.clone());
    annotate_packet_batch_timing(
        answer,
        "packet_fused_subquery_batch",
        duration_ms,
        &outcome.sidecar_diagnostics,
    );

    let mut total_duration_ms = duration_ms;
    let mut results = outcome.results;
    let mut effective_diagnostics = outcome.sidecar_diagnostics;
    let retry_pending = packet_fused_retry_pending(&pending, &outcome.retryable_queries);
    if !retry_pending.is_empty() {
        if !packet_fused_retry_is_live() {
            return Err(ApiError::new(
                "cancelled",
                "packet fused retry was cancelled before dispatch",
            ));
        }
        if let Some(remaining_ms) = packet_latency.remaining_for_handoff() {
            answer
                .retrieval_trace
                .annotations
                .push(RetrievalAnnotationDto::observation(format!(
                    "packet_fused_blocking_cancel_retry count={}",
                    retry_pending.len()
                )));
            let retry_batch = retry_pending
                .iter()
                .map(|(_, query)| (query.query.clone(), per_query_limit))
                .collect::<Vec<_>>();
            let retry_started_at = Instant::now();
            let retry_outcome = controller
                .search_packet_fused_batch(&retry_batch, Some(remaining_ms))
                .map_err(|error| {
                    answer
                        .retrieval_trace
                        .annotations
                        .push(RetrievalAnnotationDto::gap(format!(
                            "packet_fused_blocking_cancel_retry_failed error={error:?}"
                        )));
                    error
                })?;
            let retry_duration_ms = clamp_u128_to_u32(retry_started_at.elapsed().as_millis());
            total_duration_ms = total_duration_ms.saturating_add(retry_duration_ms);
            answer.retrieval_trace.total_latency_ms = answer
                .retrieval_trace
                .total_latency_ms
                .saturating_add(retry_duration_ms);
            answer
                .retrieval_trace
                .packet_sidecar_diagnostics
                .extend(retry_outcome.sidecar_diagnostics.clone());
            annotate_packet_batch_timing(
                answer,
                "packet_fused_blocking_cancel_retry_batch",
                retry_duration_ms,
                &retry_outcome.sidecar_diagnostics,
            );
            replace_packet_fused_results(&mut results, retry_outcome.results);
            replace_packet_fused_diagnostics(
                &mut effective_diagnostics,
                retry_outcome.sidecar_diagnostics,
            );
            if !retry_outcome.retryable_queries.is_empty() {
                // Retries were exhausted with queries still unresolved: their evidence is missing.
                answer
                    .retrieval_trace
                    .annotations
                    .push(RetrievalAnnotationDto::gap(format!(
                        "packet_fused_blocking_cancel_retry exhausted count={}",
                        retry_outcome.retryable_queries.len()
                    )));
            }
        } else {
            // The retry never ran, so those queries contributed no evidence.
            answer
                .retrieval_trace
                .annotations
                .push(RetrievalAnnotationDto::gap(format!(
                    "packet_fused_blocking_cancel_retry skipped reason=latency_budget_exhausted count={}",
                    retry_pending.len()
                )));
        }
    }

    merge_packet_fused_subquery_batch(
        answer,
        &pending,
        &results,
        total_duration_ms,
        &effective_diagnostics,
        include_evidence,
        stage_carry_limit,
    );
    packet_latency.apply_to_trace(answer);
    Ok(())
}

fn packet_fused_retry_pending<'a>(
    pending: &[(usize, &'a PacketPlanQueryDto)],
    retryable_queries: &[String],
) -> Vec<(usize, &'a PacketPlanQueryDto)> {
    let retryable = retryable_queries
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    pending
        .iter()
        .copied()
        .filter(|(_, query)| retryable.contains(query.query.as_str()))
        .collect()
}

fn packet_fused_retry_is_live() -> bool {
    !crate::services::active_public_operation_cancellation()
        .is_some_and(|cancelled| cancelled.load(AtomicOrdering::Acquire))
}

fn replace_packet_fused_results(
    results: &mut [(String, Vec<PacketSearchHit>)],
    retry_results: Vec<(String, Vec<PacketSearchHit>)>,
) {
    for (retry_query, retry_hits) in retry_results {
        if let Some((_, hits)) = results.iter_mut().find(|(query, _)| *query == retry_query) {
            *hits = retry_hits;
        }
    }
}

fn replace_packet_fused_diagnostics(
    diagnostics: &mut [PacketSidecarQueryDiagnosticDto],
    retry_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
) {
    for retry in retry_diagnostics {
        if let Some(diagnostic) = diagnostics
            .iter_mut()
            .find(|diagnostic| diagnostic.query == retry.query)
        {
            *diagnostic = retry;
        }
    }
}

fn annotate_packet_batch_timing(
    answer: &mut AgentAnswerDto,
    label: &str,
    duration_ms: u32,
    diagnostics: &[PacketSidecarQueryDiagnosticDto],
) {
    let attributed_ms = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.total_elapsed_ms.or(diagnostic.sidecar_query_ms))
        .fold(0_u32, u32::saturating_add);
    let overhead_ms = duration_ms.saturating_sub(attributed_ms);
    let batch_query_wall_ms = diagnostics
        .iter()
        .find_map(|diagnostic| diagnostic.batch_query_wall_ms);
    let batch_wall_note = batch_query_wall_ms
        .map(|ms| format!(" batch_query_wall_ms={ms}"))
        .unwrap_or_default();
    let mut annotation = format!(
        "{label} total_ms={} attributed_query_ms={} overhead_ms={} queries={}",
        duration_ms,
        attributed_ms,
        overhead_ms,
        diagnostics.len()
    );
    annotation.push_str(&batch_wall_note);
    answer
        .retrieval_trace
        .annotations
        .push(RetrievalAnnotationDto::observation(annotation));
}

fn packet_subquery_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact
        | PacketBudgetModeDto::Standard
        | PacketBudgetModeDto::Deep => 16,
    }
}

fn packet_free_queries(
    plan: &PacketPlanDto,
    answer: &AgentAnswerDto,
    limit: usize,
) -> Vec<PacketPlanQueryDto> {
    let mut seen = HashSet::new();
    plan.queries
        .iter()
        .filter(|query| packet_plan_query_is_typed_free_query(query))
        .filter(|query| !packet_query_completed(answer, &query.query))
        .filter(|query| {
            let key = normalize_identifier(&query.query);
            key.is_empty() || seen.insert(key)
        })
        .take(limit)
        .cloned()
        .collect()
}

fn packet_query_completed(answer: &AgentAnswerDto, query: &str) -> bool {
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .iter()
        .rev()
        .find(|diagnostic| diagnostic.query == query)
        .is_some_and(|diagnostic| {
            diagnostic.completion == codestory_contracts::api::PacketQueryCompletionDto::Completed
        })
        || answer.retrieval_trace.steps.iter().rev().any(|step| {
            step.kind == AgentRetrievalStepKindDto::Search
                && step.status == AgentRetrievalStepStatusDto::Ok
                && step
                    .input
                    .iter()
                    .any(|field| field.key == "query" && field.value == query)
        })
}
