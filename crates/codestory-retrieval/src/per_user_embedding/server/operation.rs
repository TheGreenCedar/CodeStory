//! Engine request execution, leases, cancellation, and qualification completion.

use super::super::qualification_control::{
    ServerQualificationEvent, ServerQualificationEventClock, write_server_qualification_event,
};
use super::super::{
    AwakeMonotonicClock, CONNECTION_POLL, EmbeddingClientBudgets, EmbeddingCompatibility,
    EmbeddingEngineLeaseIdentity, EmbeddingOperation, EmbeddingProtocolRequest,
    EmbeddingRequestClass, EmbeddingRequestContext, EmbeddingResult, EmbeddingServerStream,
    RETRIEVAL_EMBEDDING_DIM, elapsed_since, encode_vectors,
};
use super::frame::{IncrementalProtocolFrameReader, ProtocolFramePoll};
use super::response::{
    configure_server_operation_timeout, engine_error, engine_identity, failure_response,
    protocol_error, success_response, valid_cancel_token, validate_protocol_request,
    write_anyhow_failure, write_deadline_exceeded, write_engine_failure, write_protocol_response,
};
use super::state::{
    PerUserEmbeddingServerState, ServerCancellationAuth, ServerLeaseActivity,
    ServerRequestRegistration,
};
use crate::embedding_contract::normalize_and_validate_vectors;
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use uuid::Uuid;

pub(super) struct ServerEmbeddingRequest<'a> {
    pub(super) request_id: &'a str,
    pub(super) scope_id: String,
    pub(super) request_class: EmbeddingRequestClass,
    pub(super) deadline_ms: u64,
    pub(super) retry_after_ms: u64,
    pub(super) cancel_token: Option<String>,
    pub(super) client_pid: u32,
    pub(super) client_process_start_id: &'a str,
    pub(super) inputs: Vec<String>,
}

pub(in crate::per_user_embedding) fn serve_embedding_request(
    state: &Arc<PerUserEmbeddingServerState>,
    connection_id: &str,
    stream: &mut dyn EmbeddingServerStream,
    request: ServerEmbeddingRequest<'_>,
) -> Result<()> {
    let ServerEmbeddingRequest {
        request_id,
        scope_id,
        request_class,
        deadline_ms,
        retry_after_ms,
        cancel_token,
        client_pid,
        client_process_start_id,
        inputs,
    } = request;
    if deadline_ms == 0 {
        return write_protocol_response(
            stream,
            failure_response(
                request_id,
                protocol_error(
                    "embedding_server_deadline_invalid",
                    "embedding request deadline must be finite and positive",
                ),
            ),
            &[],
        );
    }
    let deadline = ServerRequestDeadline::start(state.clock.as_ref(), deadline_ms);
    let context = EmbeddingRequestContext::new(request_id, &scope_id, retry_after_ms);
    let cancellation_auth = match cancel_token {
        Some(token) if valid_cancel_token(&token) => Some(ServerCancellationAuth {
            token,
            client_pid,
            client_process_start_id: client_process_start_id.into(),
        }),
        Some(_) => {
            return write_protocol_response(
                stream,
                failure_response(
                    request_id,
                    protocol_error(
                        "embedding_server_cancel_token_invalid",
                        "embedding cancellation requires an unguessable token",
                    ),
                ),
                &[],
            );
        }
        None => None,
    };
    configure_server_operation_timeout(stream, deadline_ms)?;
    let admission = state.try_admit_request(request_class, retry_after_ms);
    if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
        return write_deadline_exceeded(stream, request_id, retry_after_ms, request_class, None);
    }
    let admission = match admission {
        Ok(admission) => admission,
        Err(error) => {
            return write_protocol_response(stream, failure_response(request_id, *error), &[]);
        }
    };
    let guard = state.begin_request(ServerRequestRegistration {
        connection_id,
        request_id,
        scope_id: &scope_id,
        request_class,
        phase: "queued",
        context: context.clone(),
        admission,
        cancellation_auth,
    });
    if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
        return write_deadline_exceeded(stream, request_id, retry_after_ms, request_class, None);
    }
    let guard = guard?;
    let engine = state.engine();
    if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
        return write_deadline_exceeded(
            stream,
            request_id,
            retry_after_ms,
            request_class,
            engine.as_ref().ok(),
        );
    }
    let engine = match engine {
        Ok(engine) => engine,
        Err(error) => return write_anyhow_failure(stream, request_id, error),
    };
    if context.is_cancelled() || cancel_if_peer_dead(stream, &context)? {
        return Ok(());
    }
    let handle = match request_class {
        EmbeddingRequestClass::Query => {
            engine.submit_query_prepared(context.clone(), inputs[0].clone())
        }
        EmbeddingRequestClass::Bulk => engine.submit_documents_prepared(context.clone(), inputs),
    };
    let handle = match handle {
        Ok(handle) => handle,
        Err(error) => return write_engine_failure(stream, request_id, error),
    };
    loop {
        guard.update_phase(context.phase());
        if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
            let _ = handle.cancel();
            return write_deadline_exceeded(
                stream,
                request_id,
                retry_after_ms,
                request_class,
                Some(&engine),
            );
        }
        if cancel_if_peer_dead(stream, &context)? {
            let _ = handle.cancel();
            return Ok(());
        }
        match handle.try_recv_with_completion() {
            Ok(Some(Ok(completion))) => {
                let native_completion_sequence = completion.completion_sequence;
                let vectors = normalize_and_validate_vectors(completion.vectors)?;
                let payload = encode_vectors(&vectors)?;
                let snapshot = engine.snapshot().map_err(engine_error)?;
                let identity = engine_identity(&state.process.server_instance_id, &snapshot)?;
                if deadline.cancel_if_elapsed(state.clock.as_ref(), &context) {
                    let _ = handle.cancel();
                    return write_deadline_exceeded(
                        stream,
                        request_id,
                        retry_after_ms,
                        request_class,
                        Some(&engine),
                    );
                }
                record_qualification_completed_tokens(
                    state,
                    request_id,
                    context.completed_tokens(),
                    native_completion_sequence,
                )?;
                guard.update_phase("response");
                return write_protocol_response(
                    stream,
                    success_response(
                        request_id,
                        EmbeddingResult::Vectors {
                            rows: vectors.len() as u32,
                            columns: RETRIEVAL_EMBEDDING_DIM as u32,
                            encoding: "f32_le".into(),
                            identity: Box::new(identity),
                        },
                    ),
                    &payload,
                );
            }
            Ok(Some(Err(error))) => return write_engine_failure(stream, request_id, error),
            Ok(None) => {}
            Err(error) => return write_engine_failure(stream, request_id, error),
        }
        state.clock.sleep(CONNECTION_POLL);
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::per_user_embedding) struct ServerRequestDeadline {
    started_ns: u64,
    timeout: Duration,
}

impl ServerRequestDeadline {
    pub(in crate::per_user_embedding) fn start(
        clock: &dyn AwakeMonotonicClock,
        deadline_ms: u64,
    ) -> Self {
        Self {
            started_ns: clock.now_ns(),
            timeout: Duration::from_millis(deadline_ms),
        }
    }

    pub(in crate::per_user_embedding) fn cancel_if_elapsed(
        self,
        clock: &dyn AwakeMonotonicClock,
        context: &EmbeddingRequestContext,
    ) -> bool {
        if elapsed_since(clock, self.started_ns) < self.timeout {
            return false;
        }
        context.cancel();
        true
    }
}

fn record_qualification_completed_tokens(
    state: &PerUserEmbeddingServerState,
    request_id: &str,
    completed_tokens: u64,
    native_completion_sequence: u64,
) -> Result<()> {
    let Some(control) = state.qualification.as_ref() else {
        return Ok(());
    };
    if completed_tokens == 0 {
        bail!("embedding_qualification_completed_token_count_missing");
    }
    if native_completion_sequence == 0 {
        bail!("embedding_qualification_native_completion_sequence_missing");
    }
    let clock = state.clock.snapshot();
    write_server_qualification_event(
        control,
        state,
        ServerQualificationEvent {
            schema_version: 1,
            sequence: 0,
            action: "completed_tokens".into(),
            status: "completed".into(),
            server_event_sequence: state.event_sequence.load(Ordering::Acquire),
            clock: ServerQualificationEventClock {
                domain: clock.domain,
                api: clock.api,
                boot_id: clock.boot_id,
                observed_ns: state.clock.now_ns(),
            },
            snapshot: None,
            details: Some(
                [
                    ("request_id".into(), request_id.into()),
                    ("completed_tokens".into(), completed_tokens.to_string()),
                    (
                        "native_completion_sequence".into(),
                        native_completion_sequence.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
        },
    )
}

pub(in crate::per_user_embedding) fn cancel_if_peer_dead(
    stream: &dyn EmbeddingServerStream,
    context: &EmbeddingRequestContext,
) -> Result<bool> {
    if stream
        .peer_is_alive()
        .context("probe embedding client liveness")?
    {
        return Ok(false);
    }
    context.cancel();
    Ok(true)
}

pub(super) struct ServerLeaseRequest {
    pub(super) request: EmbeddingProtocolRequest,
    pub(super) scope_id: String,
    pub(super) deadline_ms: u64,
    pub(super) retry_after_ms: u64,
    pub(super) compatibility: EmbeddingCompatibility,
}

pub(in crate::per_user_embedding) fn serve_lease_connection(
    state: &Arc<PerUserEmbeddingServerState>,
    connection_id: &str,
    stream: &mut dyn EmbeddingServerStream,
    lease_request: ServerLeaseRequest,
) -> Result<()> {
    let ServerLeaseRequest {
        request,
        scope_id,
        deadline_ms,
        retry_after_ms,
        compatibility,
    } = lease_request;
    let started_ns = state.clock.now_ns();
    let context = EmbeddingRequestContext::new(&request.request_id, &scope_id, retry_after_ms);
    let admission = match state.try_admit_request(EmbeddingRequestClass::Bulk, retry_after_ms) {
        Ok(admission) => admission,
        Err(error) => {
            return write_protocol_response(
                stream,
                failure_response(&request.request_id, *error),
                &[],
            );
        }
    };
    let guard = state.begin_request(ServerRequestRegistration {
        connection_id,
        request_id: &request.request_id,
        scope_id: &scope_id,
        request_class: EmbeddingRequestClass::Bulk,
        phase: "acquire_lease",
        context,
        admission,
        cancellation_auth: None,
    })?;
    let engine = match state.engine() {
        Ok(engine) => engine,
        Err(error) => return write_anyhow_failure(stream, &request.request_id, error),
    };
    let native_lease = match engine.acquire_residency_lease() {
        Ok(lease) => ServerLeaseActivity::new(state, lease),
        Err(error) => return write_engine_failure(stream, &request.request_id, error),
    };
    if elapsed_since(state.clock.as_ref(), started_ns) >= Duration::from_millis(deadline_ms) {
        drop(native_lease);
        return write_deadline_exceeded(
            stream,
            &request.request_id,
            retry_after_ms,
            EmbeddingRequestClass::Bulk,
            Some(&engine),
        );
    }
    if !stream
        .peer_is_alive()
        .context("probe lease client liveness")?
    {
        drop(native_lease);
        return Ok(());
    }
    guard.update_phase("response");
    let identity = engine_identity(
        &state.process.server_instance_id,
        native_lease.lease().snapshot(),
    )?;
    let lease = EmbeddingEngineLeaseIdentity {
        lease_token: Uuid::new_v4().to_string(),
        server_instance_id: state.process.server_instance_id.clone(),
        load_generation: identity.load_generation,
        compatibility_sha256: compatibility.digest()?,
    };
    write_protocol_response(
        stream,
        success_response(
            &request.request_id,
            EmbeddingResult::Lease {
                lease: lease.clone(),
                identity: Box::new(identity.clone()),
            },
        ),
        &[],
    )?;
    drop(guard);
    stream
        .set_read_timeout(Some(CONNECTION_POLL))
        .context("bound held embedding lease liveness poll")?;
    stream
        .set_write_timeout(Some(EmbeddingClientBudgets::current().connect))
        .context("bound held embedding lease response")?;
    let mut frame_reader = IncrementalProtocolFrameReader::default();

    loop {
        let (next, payload): (EmbeddingProtocolRequest, Vec<u8>) =
            match frame_reader.poll(stream)? {
                ProtocolFramePoll::Pending => {
                    if !stream
                        .peer_is_alive()
                        .context("probe held lease client liveness")?
                    {
                        return Ok(());
                    }
                    continue;
                }
                ProtocolFramePoll::Closed => return Ok(()),
                ProtocolFramePoll::Ready(next) => next,
            };
        if !payload.is_empty() {
            return Ok(());
        }
        validate_protocol_request(&next)?;
        if next.compatibility != compatibility {
            return Ok(());
        }
        match next.operation {
            EmbeddingOperation::Snapshot => {
                let current = engine.snapshot().map_err(engine_error)?;
                let current_identity =
                    engine_identity(&state.process.server_instance_id, &current)?;
                if current_identity.server_instance_id != lease.server_instance_id
                    || current_identity.load_generation != lease.load_generation
                {
                    write_protocol_response(
                        stream,
                        failure_response(
                            &next.request_id,
                            protocol_error(
                                "embedding_publication_lease_changed",
                                "embedding lease load identity changed before publication",
                            ),
                        ),
                        &[],
                    )?;
                    return Ok(());
                }
                write_protocol_response(
                    stream,
                    success_response(
                        &next.request_id,
                        EmbeddingResult::Snapshot {
                            snapshot: Box::new(state.snapshot()),
                            lease: Some(lease.clone()),
                            identity: Some(Box::new(current_identity)),
                        },
                    ),
                    &[],
                )?;
            }
            EmbeddingOperation::ReleaseLease { lease_token }
                if lease_token == lease.lease_token =>
            {
                drop(native_lease);
                write_protocol_response(
                    stream,
                    success_response(&next.request_id, EmbeddingResult::Released),
                    &[],
                )?;
                return Ok(());
            }
            _ => {
                write_protocol_response(
                    stream,
                    failure_response(
                        &next.request_id,
                        protocol_error(
                            "embedding_publication_lease_operation_invalid",
                            "only snapshot or release is valid on a lease connection",
                        ),
                    ),
                    &[],
                )?;
                return Ok(());
            }
        }
    }
}
