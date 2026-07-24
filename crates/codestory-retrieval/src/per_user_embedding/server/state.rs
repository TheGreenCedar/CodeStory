//! Server state, ownership, admission guards, and idle tracking.

use super::super::admission::{
    ServerRequestAdmission, ServerRequestAdmissionDepths, ServerRequestAdmissionPermit,
};
use super::super::qualification_control::ServerQualificationControl;
use super::super::scheduler::{ActiveServerRequest, active_request_snapshot, scheduler_snapshot};
use super::super::{
    AwakeMonotonicClock, EmbeddingCapacityPressureWire, EmbeddingClientBudgets,
    EmbeddingProtocolError, EmbeddingServerAuthoritySnapshot, EmbeddingServerEngineSnapshot,
    EmbeddingServerFailureSnapshot, EmbeddingServerProcessSnapshot,
    EmbeddingServerProtocolSnapshot, EmbeddingServerSchedulerSnapshot, EmbeddingServerSnapshot,
    PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION, SERVER_CONNECTION_HANDLER_CAPACITY,
    SERVER_CONTROL_CONNECTION_RESERVE, SERVER_TOTAL_CONNECTION_HANDLER_CAPACITY, duration_ms,
};
use super::response::{engine_error, request_key};
use anyhow::{Result, anyhow, bail};
use codestory_llama_sys::{
    EMBEDDING_BULK_QUEUE_CAPACITY, EMBEDDING_QUERY_QUEUE_CAPACITY, EmbeddingCapacityReason,
    EmbeddingEngine, EmbeddingEngineConfig, EmbeddingOwnerState, EmbeddingRequestClass,
    EmbeddingRequestContext,
};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub(in crate::per_user_embedding) struct ServerCancellation {
    pub(in crate::per_user_embedding) context: EmbeddingRequestContext,
    pub(in crate::per_user_embedding) admission: ServerRequestAdmissionPermit,
    pub(in crate::per_user_embedding) auth: Option<ServerCancellationAuth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::per_user_embedding) struct ServerCancellationAuth {
    pub(in crate::per_user_embedding) token: String,
    pub(in crate::per_user_embedding) client_pid: u32,
    pub(in crate::per_user_embedding) client_process_start_id: String,
}

pub(in crate::per_user_embedding) struct ServerRequestRegistration<'a> {
    pub(in crate::per_user_embedding) connection_id: &'a str,
    pub(in crate::per_user_embedding) request_id: &'a str,
    pub(in crate::per_user_embedding) scope_id: &'a str,
    pub(in crate::per_user_embedding) request_class: EmbeddingRequestClass,
    pub(in crate::per_user_embedding) phase: &'a str,
    pub(in crate::per_user_embedding) context: EmbeddingRequestContext,
    pub(in crate::per_user_embedding) admission: ServerRequestAdmissionPermit,
    pub(in crate::per_user_embedding) cancellation_auth: Option<ServerCancellationAuth>,
}

pub(in crate::per_user_embedding) struct PerUserEmbeddingServerState {
    pub(in crate::per_user_embedding) clock: Arc<dyn AwakeMonotonicClock>,
    pub(in crate::per_user_embedding) engine_cache_root: PathBuf,
    pub(in crate::per_user_embedding) engine_config: EmbeddingEngineConfig,
    pub(in crate::per_user_embedding) engine: Mutex<Option<EmbeddingEngine>>,
    pub(in crate::per_user_embedding) process: EmbeddingServerProcessSnapshot,
    pub(in crate::per_user_embedding) protocol: EmbeddingServerProtocolSnapshot,
    pub(in crate::per_user_embedding) authority: EmbeddingServerAuthoritySnapshot,
    pub(in crate::per_user_embedding) connections: AtomicUsize,
    pub(in crate::per_user_embedding) pre_request_connections: AtomicUsize,
    pub(in crate::per_user_embedding) admission_gate: Mutex<()>,
    pub(in crate::per_user_embedding) request_admission: Arc<ServerRequestAdmission>,
    pub(in crate::per_user_embedding) active: Mutex<BTreeMap<String, ActiveServerRequest>>,
    pub(in crate::per_user_embedding) cancellations: Mutex<BTreeMap<String, ServerCancellation>>,
    pub(in crate::per_user_embedding) draining: AtomicBool,
    pub(in crate::per_user_embedding) stopped: AtomicBool,
    pub(in crate::per_user_embedding) last_work_ended_ns: AtomicU64,
    pub(in crate::per_user_embedding) event_sequence: AtomicU64,
    pub(in crate::per_user_embedding) last_failure: Mutex<Option<EmbeddingServerFailureSnapshot>>,
    pub(in crate::per_user_embedding) qualification: Option<ServerQualificationControl>,
}

impl fmt::Debug for PerUserEmbeddingServerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingServerState")
            .field("process", &self.process)
            .field("protocol", &self.protocol)
            .field("authority", &self.authority)
            .field("connections", &self.connections.load(Ordering::Acquire))
            .field("draining", &self.draining.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl PerUserEmbeddingServerState {
    pub(in crate::per_user_embedding) fn engine(&self) -> Result<EmbeddingEngine> {
        let _admission = self
            .admission_gate
            .lock()
            .map_err(|_| anyhow!("embedding_server_admission_gate_poisoned"))?;
        if self.draining.load(Ordering::Acquire) {
            bail!("embedding_server_draining");
        }
        let mut slot = self
            .engine
            .lock()
            .map_err(|_| anyhow!("embedding_server_engine_state_poisoned"))?;
        if slot.is_none() {
            *slot = Some(
                EmbeddingEngine::initialize(&self.engine_cache_root, self.engine_config.clone())
                    .map_err(engine_error)?,
            );
            self.bump_event();
        }
        Ok(slot
            .as_ref()
            .expect("embedding engine initialized above")
            .clone())
    }

    pub(in crate::per_user_embedding) fn initialized_engine(&self) -> Option<EmbeddingEngine> {
        self.engine.lock().ok().and_then(|engine| engine.clone())
    }

    pub(in crate::per_user_embedding) fn try_initialized_engine(&self) -> Option<EmbeddingEngine> {
        self.engine
            .try_lock()
            .ok()
            .and_then(|engine| engine.clone())
    }

    pub(in crate::per_user_embedding) fn try_admit_request(
        &self,
        request_class: EmbeddingRequestClass,
        retry_after_ms: u64,
    ) -> std::result::Result<ServerRequestAdmissionPermit, Box<EmbeddingProtocolError>> {
        let active_execution = self
            .try_initialized_engine()
            .and_then(|engine| engine.admission_snapshot().active_request)
            .is_some_and(|active| active.request_class == request_class);
        self.request_admission
            .try_acquire(request_class, active_execution)
            .map_err(|depths| {
                let active = self
                    .active
                    .lock()
                    .ok()
                    .and_then(|active| active.values().next().cloned());
                let owner_state = self
                    .try_initialized_engine()
                    .map(|engine| engine.admission_snapshot().owner_state)
                    .unwrap_or(EmbeddingOwnerState::Waking);
                let pressure = EmbeddingCapacityPressureWire {
                    reason: EmbeddingCapacityReason::QueueFull.as_str().into(),
                    queue_class: request_class.as_str().into(),
                    capacity: ServerRequestAdmissionDepths::capacity(request_class) as u64,
                    depth: depths
                        .depth(request_class)
                        .saturating_sub(usize::from(active_execution))
                        as u64,
                    retry_after_ms,
                    retry_condition: "an admitted request completes or is cancelled".into(),
                    owner_state: owner_state.as_str().into(),
                    active_scope_id: active.as_ref().map(|active| active.scope_id.clone()),
                    active_request_id: active.as_ref().map(|active| active.request_id.clone()),
                    active_request_class: active
                        .as_ref()
                        .map(|active| active.request_class.as_str().into()),
                };
                Box::new(EmbeddingProtocolError {
                    code: "embedding_capacity".into(),
                    message: format!("{} request admission is full", request_class.as_str()),
                    retry_class: "after_capacity_change".into(),
                    retry_after_ms,
                    retry_condition: pressure.retry_condition.clone(),
                    capacity: Some(pressure),
                })
            })
    }

    pub(in crate::per_user_embedding) fn connection_capacity_error(
        &self,
        reason: &str,
        capacity: usize,
        depth: usize,
    ) -> EmbeddingProtocolError {
        let active = self
            .active
            .lock()
            .ok()
            .and_then(|active| active.values().next().cloned());
        let owner_state = self
            .try_initialized_engine()
            .map(|engine| engine.admission_snapshot().owner_state)
            .unwrap_or(EmbeddingOwnerState::Waking);
        let pressure = EmbeddingCapacityPressureWire {
            reason: reason.into(),
            queue_class: "connection".into(),
            capacity: capacity as u64,
            depth: depth as u64,
            retry_after_ms: duration_ms(EmbeddingClientBudgets::current().retry_after),
            retry_condition: "an authenticated connection handler completes".into(),
            owner_state: owner_state.as_str().into(),
            active_scope_id: active.as_ref().map(|active| active.scope_id.clone()),
            active_request_id: active.as_ref().map(|active| active.request_id.clone()),
            active_request_class: active
                .as_ref()
                .map(|active| active.request_class.as_str().into()),
        };
        EmbeddingProtocolError {
            code: "embedding_capacity".into(),
            message: "embedding connection admission is full".into(),
            retry_class: "after_capacity_change".into(),
            retry_after_ms: pressure.retry_after_ms,
            retry_condition: pressure.retry_condition.clone(),
            capacity: Some(pressure),
        }
    }

    pub(in crate::per_user_embedding) fn try_begin_connection(
        self: &Arc<Self>,
    ) -> Option<ServerConnectionGuard> {
        self.connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |connections| {
                (connections < SERVER_CONNECTION_HANDLER_CAPACITY).then_some(connections + 1)
            })
            .ok()?;
        self.bump_event();
        Some(ServerConnectionGuard {
            state: Arc::clone(self),
        })
    }

    pub(in crate::per_user_embedding) fn try_begin_rejection_connection(
        self: &Arc<Self>,
    ) -> Option<ServerConnectionGuard> {
        self.connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |connections| {
                (connections < SERVER_TOTAL_CONNECTION_HANDLER_CAPACITY).then_some(connections + 1)
            })
            .ok()?;
        self.bump_event();
        Some(ServerConnectionGuard {
            state: Arc::clone(self),
        })
    }

    pub(in crate::per_user_embedding) fn try_begin_pre_request(
        self: &Arc<Self>,
    ) -> Option<ServerPreRequestGuard> {
        self.pre_request_connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |connections| {
                (connections < SERVER_CONTROL_CONNECTION_RESERVE).then_some(connections + 1)
            })
            .ok()?;
        Some(ServerPreRequestGuard {
            state: Arc::clone(self),
        })
    }

    pub(in crate::per_user_embedding) fn begin_request(
        self: &Arc<Self>,
        registration: ServerRequestRegistration<'_>,
    ) -> Result<ServerRequestGuard> {
        let ServerRequestRegistration {
            connection_id,
            request_id,
            scope_id,
            request_class,
            phase,
            context,
            admission,
            cancellation_auth,
        } = registration;
        let _admission = self
            .admission_gate
            .lock()
            .map_err(|_| anyhow!("embedding_server_admission_gate_poisoned"))?;
        if self.draining.load(Ordering::Acquire) {
            bail!("embedding_server_draining");
        }
        let key = request_key(connection_id, request_id);
        let mut active = self
            .active
            .lock()
            .map_err(|_| anyhow!("embedding_server_active_state_poisoned"))?;
        let mut cancellations = self
            .cancellations
            .lock()
            .map_err(|_| anyhow!("embedding_server_cancellation_state_poisoned"))?;
        if active.contains_key(&key) {
            bail!("embedding_server_duplicate_request_id");
        }
        active.insert(
            key.clone(),
            ActiveServerRequest {
                request_id: request_id.into(),
                scope_id: scope_id.into(),
                request_class,
                phase: phase.into(),
                started_ns: self.clock.now_ns(),
            },
        );
        cancellations.insert(
            key.clone(),
            ServerCancellation {
                context,
                admission: admission.clone(),
                auth: cancellation_auth,
            },
        );
        drop(cancellations);
        drop(active);
        self.bump_event();
        Ok(ServerRequestGuard {
            state: Arc::clone(self),
            key: Some(key),
            _admission: admission,
        })
    }

    pub(in crate::per_user_embedding) fn cancel(
        &self,
        request_id: &str,
        cancel_token: &str,
        client_pid: u32,
        client_process_start_id: &str,
    ) -> bool {
        self.cancellations.lock().ok().is_some_and(|requests| {
            let suffix = format!(":{request_id}");
            let mut matches = requests
                .iter()
                .filter(|(key, cancellation)| {
                    key.ends_with(&suffix)
                        && cancellation.auth.as_ref().is_some_and(|auth| {
                            auth.token == cancel_token
                                && auth.client_pid == client_pid
                                && auth.client_process_start_id == client_process_start_id
                        })
                })
                .map(|(_, context)| context);
            let first = matches.next();
            if matches.next().is_some() {
                return false;
            }
            first.is_some_and(|cancellation| {
                let cancelled = cancellation.context.cancel();
                if cancelled {
                    cancellation.admission.release();
                }
                cancelled
            })
        })
    }

    pub(in crate::per_user_embedding) fn update_request_phase(&self, key: &str, phase: &str) {
        if let Ok(mut active) = self.active.lock()
            && let Some(request) = active.get_mut(key)
            && request.phase != phase
        {
            request.phase = phase.into();
            self.bump_event();
        }
    }

    pub(in crate::per_user_embedding) fn finish_request(&self, key: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(key);
        }
        if let Ok(mut cancellations) = self.cancellations.lock() {
            cancellations.remove(key);
        }
        self.restart_idle_window();
        self.bump_event();
    }

    pub(in crate::per_user_embedding) fn restart_idle_window(&self) {
        self.last_work_ended_ns
            .store(self.clock.now_ns(), Ordering::Release);
    }

    pub(in crate::per_user_embedding) fn true_idle(&self) -> bool {
        if self.active.lock().map_or(true, |active| !active.is_empty()) {
            return false;
        }
        if self.request_admission.snapshot() != ServerRequestAdmissionDepths::default() {
            return false;
        }
        self.initialized_engine().is_none_or(|engine| {
            let admission = engine.admission_snapshot();
            admission.query_depth == 0
                && admission.bulk_depth == 0
                && admission.active_request_count == 0
                && admission.lease_count == 0
        })
    }

    pub(in crate::per_user_embedding) fn begin_draining_if_idle(&self) -> bool {
        let Ok(_admission) = self.admission_gate.lock() else {
            return false;
        };
        if self
            .draining
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return self.true_idle();
        }
        if !self.true_idle() {
            self.draining.store(false, Ordering::Release);
            return false;
        }
        if let Some(engine) = self.initialized_engine()
            && !engine.begin_draining_if_idle()
        {
            self.draining.store(false, Ordering::Release);
            return false;
        }
        self.bump_event();
        true
    }

    pub(in crate::per_user_embedding) fn record_failure(
        &self,
        failure: EmbeddingServerFailureSnapshot,
    ) {
        if let Ok(mut last_failure) = self.last_failure.lock() {
            *last_failure = Some(failure);
        }
        self.bump_event();
    }

    pub(in crate::per_user_embedding) fn bump_event(&self) {
        self.event_sequence.fetch_add(1, Ordering::AcqRel);
    }

    pub(in crate::per_user_embedding) fn snapshot(&self) -> EmbeddingServerSnapshot {
        // Status and every Hello must remain bounded while another request is
        // performing the cold native load under the engine mutex.
        let engine = self.try_initialized_engine();
        let lifecycle = engine.as_ref().and_then(|engine| engine.snapshot().ok());
        let admission = engine.as_ref().map(EmbeddingEngine::admission_snapshot);
        let front_admission = self.request_admission.snapshot();
        let active = self.active.lock().ok().and_then(|active| {
            admission
                .as_ref()
                .and_then(|admission| admission.active_request.as_ref())
                .and_then(|native| {
                    active
                        .values()
                        .find(|candidate| {
                            candidate.request_id == native.request_id
                                && candidate.scope_id == native.scope_id
                                && candidate.request_class == native.request_class
                        })
                        .cloned()
                })
                .or_else(|| {
                    admission
                        .is_none()
                        .then(|| active.values().next().cloned())
                        .flatten()
                })
        });
        let scheduler = match admission.as_ref() {
            Some(admission) => scheduler_snapshot(
                admission,
                self.connections.load(Ordering::Acquire),
                active.as_ref(),
                self.clock.as_ref(),
            ),
            None => EmbeddingServerSchedulerSnapshot {
                query_capacity: EMBEDDING_QUERY_QUEUE_CAPACITY as u64,
                query_depth: front_admission.query as u64,
                bulk_capacity: EMBEDDING_BULK_QUEUE_CAPACITY as u64,
                bulk_depth: front_admission.bulk as u64,
                connection_count: self.connections.load(Ordering::Acquire) as u64,
                active_request_count: self.active.lock().map_or(0, |active| active.len() as u64),
                lease_count: 0,
                active_request: active
                    .as_ref()
                    .map(|active| active_request_snapshot(active, self.clock.as_ref())),
            },
        };
        let engine_snapshot = lifecycle
            .as_ref()
            .map(|lifecycle| EmbeddingServerEngineSnapshot {
                engine_owner_id: format!("{}:engine-owner", self.process.server_instance_id),
                native_worker_id: format!(
                    "{}:native-worker:{}",
                    self.process.server_instance_id, lifecycle.load_generation
                ),
                load_generation: lifecycle.load_generation,
                model_load_count: lifecycle.model_load_count,
                successful_encode_count: lifecycle.identity.encode_count,
            });
        let lifecycle_name = if self.draining.load(Ordering::Acquire) {
            "draining"
        } else {
            lifecycle
                .as_ref()
                .map_or("listening", |lifecycle| lifecycle.residency.as_str())
        };
        EmbeddingServerSnapshot {
            schema_version: PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
            event_sequence: self.event_sequence.load(Ordering::Acquire),
            lifecycle: lifecycle_name.into(),
            clock: self.clock.snapshot(),
            protocol: self.protocol.clone(),
            authority: self.authority.clone(),
            process: self.process.clone(),
            scheduler,
            engine: engine_snapshot,
            failure: self
                .last_failure
                .lock()
                .ok()
                .and_then(|failure| failure.clone()),
        }
    }

    pub(in crate::per_user_embedding) fn shutdown_engine(&self) {
        match self.engine.lock() {
            Ok(mut engine) => {
                engine.take();
            }
            Err(poisoned) => {
                poisoned.into_inner().take();
            }
        }
    }
}

pub(in crate::per_user_embedding) struct ServerLeaseActivity<L> {
    state: Arc<PerUserEmbeddingServerState>,
    lease: Option<L>,
}

impl<L> ServerLeaseActivity<L> {
    pub(in crate::per_user_embedding) fn new(
        state: &Arc<PerUserEmbeddingServerState>,
        lease: L,
    ) -> Self {
        Self {
            state: Arc::clone(state),
            lease: Some(lease),
        }
    }

    pub(in crate::per_user_embedding) fn lease(&self) -> &L {
        self.lease
            .as_ref()
            .expect("server lease activity remains live until drop")
    }
}

impl<L> Drop for ServerLeaseActivity<L> {
    fn drop(&mut self) {
        // Reset the idle clock before the native lease count becomes zero, so
        // the accept loop can never observe true idle with the old timestamp.
        self.state.restart_idle_window();
        self.lease.take();
        self.state.bump_event();
    }
}

pub(in crate::per_user_embedding) struct ServerRequestGuard {
    state: Arc<PerUserEmbeddingServerState>,
    key: Option<String>,
    _admission: ServerRequestAdmissionPermit,
}

impl Drop for ServerRequestGuard {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.state.finish_request(&key);
        }
    }
}

impl ServerRequestGuard {
    pub(in crate::per_user_embedding) fn update_phase(&self, phase: &str) {
        if let Some(key) = self.key.as_deref() {
            self.state.update_request_phase(key, phase);
        }
    }
}

pub(in crate::per_user_embedding) struct ServerConnectionGuard {
    state: Arc<PerUserEmbeddingServerState>,
}

impl Drop for ServerConnectionGuard {
    fn drop(&mut self) {
        self.state.connections.fetch_sub(1, Ordering::AcqRel);
        self.state.bump_event();
    }
}

pub(in crate::per_user_embedding) struct ServerPreRequestGuard {
    state: Arc<PerUserEmbeddingServerState>,
}

impl Drop for ServerPreRequestGuard {
    fn drop(&mut self) {
        self.state
            .pre_request_connections
            .fetch_sub(1, Ordering::AcqRel);
    }
}
