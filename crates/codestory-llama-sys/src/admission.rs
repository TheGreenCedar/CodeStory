use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const EMBEDDING_QUERY_QUEUE_CAPACITY: usize = 64;
pub const EMBEDDING_BULK_QUEUE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingRequestClass {
    Query,
    Bulk,
}

impl EmbeddingRequestClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Bulk => "bulk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingOwnerState {
    Unloaded,
    Waking,
    Ready,
    Sleeping,
    Draining,
}

impl EmbeddingOwnerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unloaded => "unloaded",
            Self::Waking => "waking",
            Self::Ready => "ready",
            Self::Sleeping => "sleeping",
            Self::Draining => "draining",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingCapacityReason {
    QueueFull,
    DeadlineElapsed,
    OwnerDraining,
    OwnerUnresponsive,
}

impl EmbeddingCapacityReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::QueueFull => "queue_full",
            Self::DeadlineElapsed => "deadline_elapsed",
            Self::OwnerDraining => "owner_draining",
            Self::OwnerUnresponsive => "owner_unresponsive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingCapacityPressure {
    pub reason: EmbeddingCapacityReason,
    pub request_class: EmbeddingRequestClass,
    pub capacity: usize,
    pub depth: usize,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    pub owner_state: EmbeddingOwnerState,
    pub active_scope_id: Option<String>,
    pub active_request_id: Option<String>,
    pub active_request_class: Option<EmbeddingRequestClass>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingActiveRequestSnapshot {
    pub request_id: String,
    pub scope_id: String,
    pub request_class: EmbeddingRequestClass,
    pub phase: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingAdmissionSnapshot {
    pub owner_state: EmbeddingOwnerState,
    pub query_capacity: usize,
    pub query_depth: usize,
    pub bulk_capacity: usize,
    pub bulk_depth: usize,
    pub active_request_count: usize,
    pub lease_count: usize,
    pub active_request: Option<EmbeddingActiveRequestSnapshot>,
    pub event_sequence: u64,
    pub progress_sequence: u64,
    pub query_progress_sequence: u64,
    pub bulk_progress_sequence: u64,
    pub completed_request_count: u64,
    pub cancelled_request_count: u64,
    pub failed_request_count: u64,
}

#[derive(Debug, Clone)]
pub struct EmbeddingRequestContext {
    request_id: String,
    scope_id: String,
    retry_after_ms: u64,
    lifecycle: Arc<AtomicU8>,
    completed_tokens: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum RequestLifecycle {
    Queued = 0,
    Active = 1,
    Committed = 2,
    Cancelled = 3,
}

impl EmbeddingRequestContext {
    pub fn new(
        request_id: impl Into<String>,
        scope_id: impl Into<String>,
        retry_after_ms: u64,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            scope_id: scope_id.into(),
            retry_after_ms,
            lifecycle: Arc::new(AtomicU8::new(RequestLifecycle::Queued as u8)),
            completed_tokens: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn retry_after_ms(&self) -> u64 {
        self.retry_after_ms
    }

    pub fn cancel(&self) -> bool {
        let mut current = self.lifecycle.load(Ordering::Acquire);
        loop {
            if current == RequestLifecycle::Committed as u8
                || current == RequestLifecycle::Cancelled as u8
            {
                return false;
            }
            match self.lifecycle.compare_exchange_weak(
                current,
                RequestLifecycle::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) == RequestLifecycle::Cancelled as u8
    }

    pub fn phase(&self) -> &'static str {
        match self.lifecycle.load(Ordering::Acquire) {
            value if value == RequestLifecycle::Queued as u8 => "queued",
            value if value == RequestLifecycle::Active as u8 => "native_execution",
            value if value == RequestLifecycle::Committed as u8 => "committed",
            value if value == RequestLifecycle::Cancelled as u8 => "cancelled",
            _ => "invalid",
        }
    }

    pub fn completed_tokens(&self) -> u64 {
        self.completed_tokens.load(Ordering::Acquire)
    }

    pub(crate) fn record_completed_tokens(&self, completed_tokens: usize) {
        self.completed_tokens
            .store(completed_tokens as u64, Ordering::Release);
    }

    pub(crate) fn activate(&self) -> bool {
        self.lifecycle
            .compare_exchange(
                RequestLifecycle::Queued as u8,
                RequestLifecycle::Active as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(crate) fn commit(&self) -> bool {
        self.lifecycle
            .compare_exchange(
                RequestLifecycle::Active as u8,
                RequestLifecycle::Committed as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

#[derive(Debug, Clone)]
struct ActiveRequest {
    context: EmbeddingRequestContext,
    request_class: EmbeddingRequestClass,
    phase: &'static str,
}

#[derive(Debug)]
struct AdmissionState {
    owner_state: EmbeddingOwnerState,
    active: Vec<ActiveRequest>,
    lease_count: usize,
    event_sequence: u64,
    progress_sequence: u64,
    query_progress_sequence: u64,
    bulk_progress_sequence: u64,
    completed_request_count: u64,
    cancelled_request_count: u64,
    failed_request_count: u64,
}

impl Default for AdmissionState {
    fn default() -> Self {
        Self {
            owner_state: EmbeddingOwnerState::Waking,
            active: Vec::new(),
            lease_count: 0,
            event_sequence: 0,
            progress_sequence: 0,
            query_progress_sequence: 0,
            bulk_progress_sequence: 0,
            completed_request_count: 0,
            cancelled_request_count: 0,
            failed_request_count: 0,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct EmbeddingAdmissionTracker {
    state: Mutex<AdmissionState>,
}

impl EmbeddingAdmissionTracker {
    pub(crate) fn set_owner_state(&self, owner_state: EmbeddingOwnerState) {
        if let Ok(mut state) = self.state.lock() {
            state.owner_state = owner_state;
            state.event_sequence = state.event_sequence.saturating_add(1);
        }
    }

    pub(crate) fn begin(
        &self,
        context: &EmbeddingRequestContext,
        request_class: EmbeddingRequestClass,
    ) -> bool {
        if !context.activate() {
            self.cancelled();
            return false;
        }
        if let Ok(mut state) = self.state.lock() {
            state.active.push(ActiveRequest {
                context: context.clone(),
                request_class,
                phase: "native_execution",
            });
            state.event_sequence = state.event_sequence.saturating_add(1);
            record_progress(&mut state, request_class);
        }
        true
    }

    pub(crate) fn progress(&self, request_class: EmbeddingRequestClass) {
        if let Ok(mut state) = self.state.lock() {
            state.event_sequence = state.event_sequence.saturating_add(1);
            record_progress(&mut state, request_class);
        }
    }

    pub(crate) fn finish(
        &self,
        context: &EmbeddingRequestContext,
        request_class: EmbeddingRequestClass,
        succeeded: bool,
        cancelled: bool,
    ) -> u64 {
        if let Ok(mut state) = self.state.lock() {
            if let Some(index) = state
                .active
                .iter()
                .rposition(|active| active.context.request_id() == context.request_id())
            {
                state.active.remove(index);
            }
            if cancelled {
                state.cancelled_request_count = state.cancelled_request_count.saturating_add(1);
            } else if succeeded {
                state.completed_request_count = state.completed_request_count.saturating_add(1);
            } else {
                state.failed_request_count = state.failed_request_count.saturating_add(1);
            }
            state.event_sequence = state.event_sequence.saturating_add(1);
            record_progress(&mut state, request_class);
            return state.completed_request_count;
        }
        0
    }

    pub(crate) fn cancelled(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.cancelled_request_count = state.cancelled_request_count.saturating_add(1);
            state.event_sequence = state.event_sequence.saturating_add(1);
        }
    }

    pub(crate) fn lease_acquired(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.lease_count = state.lease_count.saturating_add(1);
            state.event_sequence = state.event_sequence.saturating_add(1);
        }
    }

    pub(crate) fn lease_released(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.lease_count = state.lease_count.saturating_sub(1);
            state.event_sequence = state.event_sequence.saturating_add(1);
        }
    }

    pub(crate) fn pressure(
        &self,
        reason: EmbeddingCapacityReason,
        request_class: EmbeddingRequestClass,
        query_depth: usize,
        bulk_depth: usize,
        retry_after_ms: u64,
        retry_condition: impl Into<String>,
    ) -> EmbeddingCapacityPressure {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active = state.active.last();
        let (capacity, depth) = match request_class {
            EmbeddingRequestClass::Query => (EMBEDDING_QUERY_QUEUE_CAPACITY, query_depth),
            EmbeddingRequestClass::Bulk => (EMBEDDING_BULK_QUEUE_CAPACITY, bulk_depth),
        };
        EmbeddingCapacityPressure {
            reason,
            request_class,
            capacity,
            depth,
            retry_after_ms,
            retry_condition: retry_condition.into(),
            owner_state: state.owner_state,
            active_scope_id: active.map(|active| active.context.scope_id().to_string()),
            active_request_id: active.map(|active| active.context.request_id().to_string()),
            active_request_class: active.map(|active| active.request_class),
        }
    }

    pub(crate) fn snapshot(
        &self,
        query_depth: usize,
        bulk_depth: usize,
    ) -> EmbeddingAdmissionSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let active_request = state
            .active
            .last()
            .map(|active| EmbeddingActiveRequestSnapshot {
                request_id: active.context.request_id().to_string(),
                scope_id: active.context.scope_id().to_string(),
                request_class: active.request_class,
                phase: active.phase,
            });
        EmbeddingAdmissionSnapshot {
            owner_state: state.owner_state,
            query_capacity: EMBEDDING_QUERY_QUEUE_CAPACITY,
            query_depth,
            bulk_capacity: EMBEDDING_BULK_QUEUE_CAPACITY,
            bulk_depth,
            active_request_count: state.active.len(),
            lease_count: state.lease_count,
            active_request,
            event_sequence: state.event_sequence,
            progress_sequence: state.progress_sequence,
            query_progress_sequence: state.query_progress_sequence,
            bulk_progress_sequence: state.bulk_progress_sequence,
            completed_request_count: state.completed_request_count,
            cancelled_request_count: state.cancelled_request_count,
            failed_request_count: state.failed_request_count,
        }
    }
}

fn record_progress(state: &mut AdmissionState, request_class: EmbeddingRequestClass) {
    state.progress_sequence = state.progress_sequence.saturating_add(1);
    let class_progress = match request_class {
        EmbeddingRequestClass::Query => &mut state.query_progress_sequence,
        EmbeddingRequestClass::Bulk => &mut state.bulk_progress_sequence,
    };
    *class_progress = class_progress.saturating_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_commit_have_one_winner() {
        let context = EmbeddingRequestContext::new("request", "scope", 25);
        assert!(context.activate());
        assert!(context.cancel());
        assert!(!context.commit());
        assert!(context.is_cancelled());

        let committed = EmbeddingRequestContext::new("request-2", "scope", 25);
        assert!(committed.activate());
        assert!(committed.commit());
        assert!(!committed.cancel());
    }

    #[test]
    fn snapshot_preserves_fifo_queue_capacities_and_opaque_active_scope() {
        let tracker = EmbeddingAdmissionTracker::default();
        tracker.set_owner_state(EmbeddingOwnerState::Ready);
        let context = EmbeddingRequestContext::new("request-1", "scope-opaque", 25);
        assert!(tracker.begin(&context, EmbeddingRequestClass::Query));
        let snapshot = tracker.snapshot(2, 3);
        assert_eq!(snapshot.query_capacity, 64);
        assert_eq!(snapshot.bulk_capacity, 64);
        assert_eq!(snapshot.query_depth, 2);
        assert_eq!(snapshot.bulk_depth, 3);
        assert_eq!(
            snapshot
                .active_request
                .as_ref()
                .map(|request| request.scope_id.as_str()),
            Some("scope-opaque")
        );
    }

    #[test]
    fn successful_finishes_receive_monotonic_native_completion_sequences() {
        let tracker = EmbeddingAdmissionTracker::default();
        let first = EmbeddingRequestContext::new("first", "scope", 25);
        let failed = EmbeddingRequestContext::new("failed", "scope", 25);
        let second = EmbeddingRequestContext::new("second", "scope", 25);
        assert!(tracker.begin(&first, EmbeddingRequestClass::Query));
        assert!(tracker.begin(&failed, EmbeddingRequestClass::Bulk));
        assert!(tracker.begin(&second, EmbeddingRequestClass::Query));

        assert_eq!(
            tracker.finish(&first, EmbeddingRequestClass::Query, true, false),
            1
        );
        assert_eq!(
            tracker.finish(&failed, EmbeddingRequestClass::Bulk, false, false),
            1
        );
        assert_eq!(
            tracker.finish(&second, EmbeddingRequestClass::Query, true, false),
            2
        );
    }

    #[test]
    fn progress_sequences_advance_independently_by_request_class() {
        let tracker = EmbeddingAdmissionTracker::default();
        let query = EmbeddingRequestContext::new("query", "scope", 25);
        let bulk = EmbeddingRequestContext::new("bulk", "scope", 25);
        assert!(tracker.begin(&query, EmbeddingRequestClass::Query));
        assert!(tracker.begin(&bulk, EmbeddingRequestClass::Bulk));

        tracker.progress(EmbeddingRequestClass::Bulk);
        let after_bulk = tracker.snapshot(0, 0);
        assert_eq!(after_bulk.progress_sequence, 3);
        assert_eq!(after_bulk.query_progress_sequence, 1);
        assert_eq!(after_bulk.bulk_progress_sequence, 2);

        tracker.progress(EmbeddingRequestClass::Query);
        let after_query = tracker.snapshot(0, 0);
        assert_eq!(after_query.progress_sequence, 4);
        assert_eq!(after_query.query_progress_sequence, 2);
        assert_eq!(after_query.bulk_progress_sequence, 2);
    }
}
