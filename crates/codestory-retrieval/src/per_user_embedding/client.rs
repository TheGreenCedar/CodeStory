//! Per-process embedding client, replay boundary, and residency lease.

use super::protocol::validate_raw_inputs;
use super::qualification_control::{
    EmbeddingQualificationAttemptExchange, EmbeddingQualificationAttemptResult,
};
use super::{
    CONNECTION_POLL, EmbeddingClientBudgets, EmbeddingClientTransport, EmbeddingCompatibility,
    EmbeddingConnectIntent, EmbeddingConnectOutcome, EmbeddingEngineIdentity,
    EmbeddingEngineLeaseIdentity, EmbeddingExecutableIdentity, EmbeddingOperation, EmbeddingResult,
    EmbeddingServerSnapshot, EmbeddingServerStream, PerUserEmbeddingError,
    configure_exchange_timeout, decode_vectors, duration_ms, elapsed_since, embedding_scope_id,
    exchange, hello, is_server_loss, positive_duration_ms, request, response_result,
    validate_engine_identity, validate_engine_server_identity, validate_lease_server_identity,
    validate_same_server, validate_server_snapshot, vectors_result,
};
use crate::config::SidecarRuntimeConfig;
use crate::embedding_contract::normalize_and_validate_vectors;
use anyhow::{Result, anyhow, bail};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

static CLIENT_TRANSPORT: OnceLock<Arc<dyn EmbeddingClientTransport>> = OnceLock::new();

pub fn install_embedding_client_transport(
    transport: Arc<dyn EmbeddingClientTransport>,
) -> Result<()> {
    CLIENT_TRANSPORT
        .set(transport)
        .map_err(|_| anyhow!("embedding_client_transport_already_installed"))
}

#[derive(Clone)]
pub struct PerUserEmbeddingClient {
    pub(super) transport: Arc<dyn EmbeddingClientTransport>,
    pub(super) compatibility: EmbeddingCompatibility,
    pub(super) scope_id: String,
}

struct EmbeddingCallControl<'a> {
    operation_timeout: Duration,
    outer_deadline: Option<Instant>,
    operation_deadline: OnceLock<Instant>,
    cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl<'a> EmbeddingCallControl<'a> {
    fn new(
        operation_timeout: Duration,
        outer_timeout: Option<Duration>,
        cancelled: &'a (dyn Fn() -> bool + Sync),
    ) -> Result<Self> {
        if operation_timeout.is_zero() || outer_timeout.is_some_and(|timeout| timeout.is_zero()) {
            bail!("embedding_server_deadline_invalid");
        }
        let outer_deadline = outer_timeout
            .map(|timeout| {
                Instant::now()
                    .checked_add(timeout)
                    .ok_or_else(|| anyhow!("embedding_server_deadline_invalid"))
            })
            .transpose()?;
        let control = Self {
            operation_timeout,
            outer_deadline,
            operation_deadline: OnceLock::new(),
            cancelled,
        };
        control.check()?;
        Ok(control)
    }

    fn arm(&self) -> Result<()> {
        if self.operation_deadline.get().is_none() {
            let deadline = Instant::now()
                .checked_add(self.operation_timeout)
                .ok_or_else(|| anyhow!("embedding_server_deadline_invalid"))?;
            let _ = self.operation_deadline.set(deadline);
        }
        self.check()
    }

    fn active_deadline(&self) -> Option<Instant> {
        match (self.outer_deadline, self.operation_deadline.get().copied()) {
            (Some(outer), Some(operation)) => Some(outer.min(operation)),
            (Some(outer), None) => Some(outer),
            (None, Some(operation)) => Some(operation),
            (None, None) => None,
        }
    }

    fn triggered(&self) -> bool {
        (self.cancelled)()
            || self
                .active_deadline()
                .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn check(&self) -> Result<()> {
        if (self.cancelled)() {
            return Err(PerUserEmbeddingError {
                code: "embedding_cancelled".into(),
                message: "the caller cancelled the embedding request".into(),
                retry_class: "none".into(),
                retry_after_ms: 0,
                retry_condition: "the caller starts a new request".into(),
                capacity: None,
            }
            .into());
        }
        if self
            .active_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(PerUserEmbeddingError {
                code: "embedding_deadline_exceeded".into(),
                message: "the caller deadline elapsed during the embedding request".into(),
                retry_class: "after_delay".into(),
                retry_after_ms: 0,
                retry_condition: "the caller starts a new request with a fresh deadline".into(),
                capacity: None,
            }
            .into());
        }
        Ok(())
    }

    fn remaining(&self, maximum: Duration) -> Result<Duration> {
        self.check()?;
        let remaining = self.active_deadline().map_or(maximum, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(maximum)
        });
        if remaining.is_zero() {
            self.check()?;
            bail!("embedding_server_deadline_invalid");
        }
        Ok(remaining)
    }
}

pub(super) struct ValidatedEmbeddingConnection {
    stream: Box<dyn EmbeddingServerStream>,
    snapshot: EmbeddingServerSnapshot,
}

impl fmt::Debug for PerUserEmbeddingClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingClient")
            .field("scope_id", &self.scope_id)
            .field("compatibility", &self.compatibility)
            .finish_non_exhaustive()
    }
}

impl PerUserEmbeddingClient {
    pub fn for_runtime(runtime: &SidecarRuntimeConfig) -> Result<Self> {
        let transport = CLIENT_TRANSPORT
            .get()
            .cloned()
            .ok_or_else(|| anyhow!("embedding_server_transport_unavailable"))?;
        Ok(Self {
            transport,
            compatibility: EmbeddingCompatibility::current(runtime.embedding.allow_cpu),
            scope_id: embedding_scope_id(runtime),
        })
    }

    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_query_with_control(text, None, &|| false)
    }

    pub fn embed_query_with_control(
        &self,
        text: &str,
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<f32>> {
        self.embed_query_with_control_and_attempts(text, maximum_timeout, cancelled)
            .map(|(vector, _)| vector)
    }

    pub(super) fn embed_query_with_qualification_attempts(
        &self,
        text: &str,
    ) -> Result<(Vec<f32>, Vec<EmbeddingQualificationAttemptResult>)> {
        self.embed_query_with_control_and_attempts(text, None, &|| false)
    }

    fn embed_query_with_control_and_attempts(
        &self,
        text: &str,
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(Vec<f32>, Vec<EmbeddingQualificationAttemptResult>)> {
        validate_raw_inputs(std::slice::from_ref(&text.to_string()))?;
        let budgets = self.transport.budgets();
        let (result, attempts) = self.call_pure_with_replay_controlled_and_attempts(
            budgets.query_request,
            maximum_timeout,
            cancelled,
            |deadline_ms, token| EmbeddingOperation::EmbedQuery {
                scope_id: self.scope_id.clone(),
                deadline_ms,
                retry_after_ms: duration_ms(budgets.retry_after),
                cancel_token: Some(token),
                input: text.to_string(),
            },
        )?;
        let (rows, columns, identity, payload) = vectors_result(result)?;
        if rows != 1 {
            bail!("embedding_vector_row_count_mismatch: expected=1 observed={rows}");
        }
        let mut vectors = decode_vectors(rows, columns, &payload)?;
        validate_engine_identity(&identity, &self.compatibility)?;
        let vector = normalize_and_validate_vectors(std::mem::take(&mut vectors))?
            .pop()
            .ok_or_else(|| anyhow!("embedding_vector_missing"))?;
        Ok((vector, attempts))
    }

    pub fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_documents_with_control(texts, None, &|| false)
    }

    pub fn embed_documents_with_control(
        &self,
        texts: &[String],
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<Vec<f32>>> {
        self.embed_documents_with_control_and_attempts(texts, maximum_timeout, cancelled)
            .map(|(vectors, _)| vectors)
    }

    pub(super) fn embed_documents_with_qualification_attempts(
        &self,
        texts: &[String],
    ) -> Result<(Vec<Vec<f32>>, Vec<EmbeddingQualificationAttemptResult>)> {
        self.embed_documents_with_control_and_attempts(texts, None, &|| false)
    }

    fn embed_documents_with_control_and_attempts(
        &self,
        texts: &[String],
        maximum_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(Vec<Vec<f32>>, Vec<EmbeddingQualificationAttemptResult>)> {
        if texts.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        validate_raw_inputs(texts)?;
        let budgets = self.transport.budgets();
        let (result, attempts) = self.call_pure_with_replay_controlled_and_attempts(
            budgets.bulk_request,
            maximum_timeout,
            cancelled,
            |deadline_ms, token| EmbeddingOperation::EmbedDocuments {
                scope_id: self.scope_id.clone(),
                deadline_ms,
                retry_after_ms: duration_ms(budgets.retry_after),
                cancel_token: Some(token),
                inputs: texts.to_vec(),
            },
        )?;
        let (rows, columns, identity, payload) = vectors_result(result)?;
        if rows as usize != texts.len() {
            bail!(
                "embedding_vector_row_count_mismatch: expected={} observed={rows}",
                texts.len()
            );
        }
        validate_engine_identity(&identity, &self.compatibility)?;
        Ok((
            normalize_and_validate_vectors(decode_vectors(rows, columns, &payload)?)?,
            attempts,
        ))
    }

    pub fn ensure_resident(&self) -> Result<EmbeddingEngineIdentity> {
        let budgets = self.transport.budgets();
        let mut connection = self.connect(EmbeddingConnectIntent::Activate, true)?;
        configure_exchange_timeout(&*connection.stream, budgets.bulk_request)?;
        let request_id = Uuid::new_v4().to_string();
        let operation = EmbeddingOperation::EnsureResident {
            scope_id: self.scope_id.clone(),
            deadline_ms: duration_ms(budgets.bulk_request),
            retry_after_ms: duration_ms(budgets.retry_after),
        };
        let (response, _) = exchange(
            &mut *connection.stream,
            request(&request_id, self.compatibility.clone(), operation),
        )?;
        let EmbeddingResult::Identity { identity } = response_result(response)? else {
            bail!("embedding_server_protocol_mismatch: expected identity");
        };
        validate_engine_identity(&identity, &self.compatibility)?;
        validate_engine_server_identity(&identity, &connection.snapshot)?;
        Ok(*identity)
    }

    pub fn acquire_residency_lease(&self) -> Result<PerUserEmbeddingResidencyLease> {
        let budgets = self.transport.budgets();
        let mut connection = self.connect(EmbeddingConnectIntent::Activate, true)?;
        configure_exchange_timeout(&*connection.stream, budgets.bulk_request)?;
        let request_id = Uuid::new_v4().to_string();
        let operation = EmbeddingOperation::AcquireLease {
            scope_id: self.scope_id.clone(),
            deadline_ms: duration_ms(budgets.bulk_request),
            retry_after_ms: duration_ms(budgets.retry_after),
        };
        let (response, _) = exchange(
            &mut *connection.stream,
            request(&request_id, self.compatibility.clone(), operation),
        )?;
        let EmbeddingResult::Lease { lease, identity } = response_result(response)? else {
            bail!("embedding_server_protocol_mismatch: expected lease");
        };
        validate_engine_identity(&identity, &self.compatibility)?;
        validate_engine_server_identity(&identity, &connection.snapshot)?;
        validate_lease_server_identity(&lease, &identity, &connection.snapshot)?;
        Ok(PerUserEmbeddingResidencyLease {
            stream: Some(connection.stream),
            compatibility: self.compatibility.clone(),
            lease,
            identity: *identity,
            server: connection.snapshot,
            budgets,
        })
    }

    pub fn observe(&self) -> Result<Option<EmbeddingServerSnapshot>> {
        Ok(self
            .observe_with_identity()?
            .map(|(snapshot, _identity)| snapshot))
    }

    pub(crate) fn observe_with_identity(
        &self,
    ) -> Result<Option<(EmbeddingServerSnapshot, Option<EmbeddingEngineIdentity>)>> {
        let mut connection = match self.connect(EmbeddingConnectIntent::Observe, false) {
            Ok(connected) => connected,
            Err(error) if error.to_string().contains("embedding_server_absent") => return Ok(None),
            Err(error) => return Err(error),
        };
        configure_exchange_timeout(&*connection.stream, self.transport.budgets().connect)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut *connection.stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::Snapshot,
            ),
        )?;
        let EmbeddingResult::Snapshot {
            snapshot, identity, ..
        } = response_result(response)?
        else {
            bail!("embedding_server_protocol_mismatch: expected snapshot");
        };
        validate_server_snapshot(
            &snapshot,
            connection.stream.transport_identity(),
            &self.transport.executable_identity(),
        )?;
        validate_same_server(&snapshot, &connection.snapshot)?;
        if let Some(identity) = identity.as_deref() {
            validate_engine_identity(identity, &self.compatibility)?;
            validate_engine_server_identity(identity, &snapshot)?;
        }
        Ok(Some((*snapshot, identity.map(|identity| *identity))))
    }

    fn call_pure_with_replay_controlled_and_attempts<B>(
        &self,
        operation_timeout: Duration,
        outer_timeout: Option<Duration>,
        cancelled: &(dyn Fn() -> bool + Sync),
        operation: B,
    ) -> Result<EmbeddingQualificationAttemptExchange>
    where
        B: Fn(u64, String) -> EmbeddingOperation,
    {
        let control = EmbeddingCallControl::new(operation_timeout, outer_timeout, cancelled)?;
        let clock = self.transport.clock();
        let mut replayed = false;
        let mut recover_after_inflight_loss = false;
        let mut attempts = Vec::with_capacity(2);
        loop {
            control.check()?;
            let mut connection = match self.connect_with_control(
                EmbeddingConnectIntent::Activate,
                true,
                Some(&control),
                recover_after_inflight_loss,
            ) {
                Ok(connection) => connection,
                Err(error) if !replayed && is_server_loss(&error) => {
                    control.check()?;
                    replayed = true;
                    continue;
                }
                Err(error) => return Err(error),
            };
            control.arm()?;
            let request_id = Uuid::new_v4().to_string();
            let cancel_token = Uuid::new_v4().to_string();
            let remaining = control.remaining(operation_timeout)?;
            let request_operation =
                operation(positive_duration_ms(remaining), cancel_token.clone());
            configure_exchange_timeout(&*connection.stream, remaining)?;
            let server_instance_id = connection.snapshot.process.server_instance_id.clone();
            let submitted_ns = clock.now_ns();
            let completed = AtomicBool::new(false);
            let exchange_result = thread::scope(|scope| {
                scope.spawn(|| {
                    self.watch_controlled_cancellation(
                        &control,
                        &completed,
                        &request_id,
                        &cancel_token,
                    );
                });
                let result = exchange(
                    &mut *connection.stream,
                    request(&request_id, self.compatibility.clone(), request_operation),
                );
                completed.store(true, Ordering::Release);
                result
            });
            let call = (|| {
                control.check()?;
                let (response, payload) = exchange_result?;
                let result = response_result(response)?;
                if let EmbeddingResult::Vectors { identity, .. } = &result {
                    validate_engine_server_identity(identity, &connection.snapshot)?;
                }
                Ok::<_, anyhow::Error>((result, payload))
            })();
            let completed_ns = clock.now_ns();
            let outcome = match &call {
                Ok(_) => "completed",
                Err(error) if is_server_loss(error) => "server_loss",
                Err(_) => "failed",
            };
            attempts.push(EmbeddingQualificationAttemptResult {
                ordinal: attempts.len() as u32 + 1,
                request_id,
                server_instance_id,
                submitted_ns,
                completed_ns,
                outcome: outcome.into(),
            });
            match call {
                Ok(result) => return Ok((result, attempts)),
                Err(error) if !replayed && is_server_loss(&error) => {
                    control.check()?;
                    replayed = true;
                    recover_after_inflight_loss = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn watch_controlled_cancellation(
        &self,
        control: &EmbeddingCallControl<'_>,
        completed: &AtomicBool,
        request_id: &str,
        cancel_token: &str,
    ) {
        while !completed.load(Ordering::Acquire) && !control.triggered() {
            thread::sleep(CONNECTION_POLL);
        }
        if !completed.load(Ordering::Acquire) {
            // The server has the same finite request deadline, so cancellation
            // is best effort rather than a retry loop. Retrying every poll
            // after a full handler admission would turn timed-out callers into
            // an unbounded control-connection storm.
            let _ = self.send_cancel(request_id, cancel_token);
        }
    }

    fn send_cancel(&self, target_request_id: &str, cancel_token: &str) -> Result<bool> {
        let mut connection = self.connect(EmbeddingConnectIntent::Activate, false)?;
        configure_exchange_timeout(&*connection.stream, self.transport.budgets().connect)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut *connection.stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::Cancel {
                    target_request_id: target_request_id.into(),
                    cancel_token: cancel_token.into(),
                },
            ),
        )?;
        match response_result(response)? {
            EmbeddingResult::Cancelled => Ok(true),
            EmbeddingResult::Released => Ok(false),
            _ => bail!("embedding_server_protocol_mismatch: expected cancellation result"),
        }
    }

    pub(super) fn connect(
        &self,
        intent: EmbeddingConnectIntent,
        may_spawn: bool,
    ) -> Result<ValidatedEmbeddingConnection> {
        self.connect_with_control(intent, may_spawn, None, false)
    }

    fn connect_with_control(
        &self,
        intent: EmbeddingConnectIntent,
        may_spawn: bool,
        control: Option<&EmbeddingCallControl<'_>>,
        recover_after_server_loss: bool,
    ) -> Result<ValidatedEmbeddingConnection> {
        let budgets = self.transport.budgets();
        let mut spawned_at_ns = None;
        let mut owner_recovery_started_at_ns = None;
        let mut spawn_attempt = None;
        let wait_for_convergence = |started_at_ns| -> Result<()> {
            if let Some(control) = control {
                control.check()?;
            }
            let elapsed = elapsed_since(self.transport.clock().as_ref(), started_at_ns);
            let remaining = budgets.spawn.saturating_sub(elapsed);
            if remaining.is_zero() {
                bail!("embedding_server_start_timeout");
            }
            let remaining = control
                .map(|control| control.remaining(remaining))
                .transpose()?
                .unwrap_or(remaining);
            self.transport
                .clock()
                .sleep(budgets.retry_after.min(remaining));
            Ok(())
        };
        loop {
            if let Some(control) = control {
                control.check()?;
            }
            let connect_budget = control
                .map(|control| control.remaining(budgets.connect))
                .transpose()?
                .unwrap_or(budgets.connect);
            match self
                .transport
                .connect(intent, connect_budget, spawn_attempt.as_ref())
                .map_err(anyhow::Error::new)?
            {
                EmbeddingConnectOutcome::Connected(mut stream) => {
                    configure_exchange_timeout(&*stream, connect_budget)?;
                    let transport_identity = stream.transport_identity().clone();
                    let executable = self.transport.executable_identity();
                    let snapshot = match hello(
                        &mut *stream,
                        intent,
                        self.compatibility.clone(),
                        &transport_identity,
                        &executable,
                    ) {
                        Ok(snapshot) => snapshot,
                        Err(error) if recover_after_server_loss && is_server_loss(&error) => {
                            let recovery_started_at_ns = owner_recovery_started_at_ns
                                .get_or_insert_with(|| self.transport.clock().now_ns());
                            wait_for_convergence(*recovery_started_at_ns)?;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    if let Some(control) = control {
                        control.check()?;
                    }
                    return Ok(ValidatedEmbeddingConnection { stream, snapshot });
                }
                EmbeddingConnectOutcome::NoOwner if may_spawn && spawned_at_ns.is_none() => {
                    spawn_attempt = Some(
                        self.transport
                            .spawn_exact_current_exe()
                            .map_err(anyhow::Error::new)?,
                    );
                    spawned_at_ns = Some(self.transport.clock().now_ns());
                }
                EmbeddingConnectOutcome::NoOwner if !may_spawn => {
                    bail!("embedding_server_absent");
                }
                EmbeddingConnectOutcome::NoOwner => {
                    let spawned_at_ns =
                        spawned_at_ns.expect("an activating retry follows an exact-exe spawn");
                    wait_for_convergence(spawned_at_ns)?;
                }
                EmbeddingConnectOutcome::OwnerUnresponsive(error) => {
                    if let Some(spawned_at_ns) = spawned_at_ns {
                        wait_for_convergence(spawned_at_ns)?;
                        continue;
                    }
                    if recover_after_server_loss {
                        let recovery_started_at_ns = owner_recovery_started_at_ns
                            .get_or_insert_with(|| self.transport.clock().now_ns());
                        wait_for_convergence(*recovery_started_at_ns)?;
                        continue;
                    }
                    return Err(PerUserEmbeddingError {
                        code: "embedding_server_owner_unresponsive".into(),
                        message: error.message,
                        retry_class: "after_server_change".into(),
                        retry_after_ms: duration_ms(budgets.retry_after),
                        retry_condition: "the lifetime authority changes".into(),
                        capacity: None,
                    }
                    .into());
                }
            }
        }
    }
}

pub struct PerUserEmbeddingResidencyLease {
    stream: Option<Box<dyn EmbeddingServerStream>>,
    compatibility: EmbeddingCompatibility,
    lease: EmbeddingEngineLeaseIdentity,
    identity: EmbeddingEngineIdentity,
    server: EmbeddingServerSnapshot,
    budgets: EmbeddingClientBudgets,
}

impl fmt::Debug for PerUserEmbeddingResidencyLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerUserEmbeddingResidencyLease")
            .field("lease", &self.lease)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl PerUserEmbeddingResidencyLease {
    pub fn identity(&self) -> &EmbeddingEngineIdentity {
        &self.identity
    }

    pub fn lease_identity(&self) -> &EmbeddingEngineLeaseIdentity {
        &self.lease
    }

    pub fn revalidate(&mut self) -> Result<EmbeddingEngineIdentity> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("embedding_publication_lease_released"))?;
        configure_exchange_timeout(&**stream, self.budgets.bulk_request)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut **stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::Snapshot,
            ),
        )?;
        let EmbeddingResult::Snapshot {
            snapshot,
            lease: Some(lease),
            identity: Some(identity),
        } = response_result(response)?
        else {
            bail!("embedding_server_protocol_mismatch: expected lease revalidation");
        };
        if lease != self.lease || identity.server_instance_id != self.identity.server_instance_id {
            bail!("embedding_publication_lease_changed");
        }
        validate_server_snapshot(
            &snapshot,
            stream.transport_identity(),
            &EmbeddingExecutableIdentity {
                pid: self.server.process.pid,
                process_start_id: self.server.process.process_start_id.clone(),
                executable_sha256: self.server.process.executable_sha256.clone(),
                executable_version: self.server.process.executable_version.clone(),
            },
        )?;
        validate_same_server(&snapshot, &self.server)?;
        validate_lease_server_identity(&lease, &identity, &snapshot)?;
        validate_engine_identity(&identity, &self.compatibility)?;
        self.identity = *identity;
        Ok(self.identity.clone())
    }

    pub fn release(mut self) -> Result<()> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> Result<()> {
        let Some(mut stream) = self.stream.take() else {
            return Ok(());
        };
        configure_exchange_timeout(&*stream, self.budgets.connect)?;
        let request_id = Uuid::new_v4().to_string();
        let (response, _) = exchange(
            &mut *stream,
            request(
                &request_id,
                self.compatibility.clone(),
                EmbeddingOperation::ReleaseLease {
                    lease_token: self.lease.lease_token.clone(),
                },
            ),
        )?;
        let EmbeddingResult::Released = response_result(response)? else {
            bail!("embedding_server_protocol_mismatch: expected lease release");
        };
        Ok(())
    }
}

impl Drop for PerUserEmbeddingResidencyLease {
    fn drop(&mut self) {
        let _ = self.release_inner();
    }
}
