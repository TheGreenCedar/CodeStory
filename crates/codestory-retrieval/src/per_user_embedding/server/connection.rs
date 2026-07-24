//! Authenticated connection handling and protocol dispatch.

use super::super::protocol::validate_raw_inputs;
use super::super::{
    EmbeddingClientBudgets, EmbeddingCompatibility, EmbeddingOperation, EmbeddingProtocolError,
    EmbeddingProtocolRequest, EmbeddingRequestClass, EmbeddingRequestContext, EmbeddingResult,
    EmbeddingServerStream, SERVER_CONNECTION_HANDLER_CAPACITY, SERVER_CONTROL_CONNECTION_RESERVE,
    elapsed_since, is_sha256, read_frame,
};
use super::operation::{
    ServerEmbeddingRequest, ServerLeaseRequest, serve_embedding_request, serve_lease_connection,
};
use super::response::{
    configure_server_operation_timeout, engine_error, engine_identity, failure_response,
    protocol_error, success_response, valid_cancel_token, validate_protocol_request,
    write_anyhow_failure, write_deadline_exceeded, write_deadline_invalid, write_protocol_response,
};
use super::state::{PerUserEmbeddingServerState, ServerRequestRegistration};
use crate::embedding_contract::{CODERANK_DOCUMENT_PREFIX, CODERANK_QUERY_PREFIX};
use anyhow::{Context, Result, anyhow, bail};
use codestory_llama_sys::NativeDeviceClass;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use uuid::Uuid;

pub(in crate::per_user_embedding) fn serve_embedding_connection(
    state: Arc<PerUserEmbeddingServerState>,
    mut stream: Box<dyn EmbeddingServerStream>,
) -> Result<()> {
    let result = serve_embedding_connection_inner(state, &mut *stream, false);
    finish_embedding_response_delivery(&*stream, result)
}

pub(in crate::per_user_embedding) fn serve_embedding_connection_at_handler_capacity(
    state: Arc<PerUserEmbeddingServerState>,
    mut stream: Box<dyn EmbeddingServerStream>,
) -> Result<()> {
    let result = serve_embedding_connection_inner(state, &mut *stream, true);
    finish_embedding_response_delivery(&*stream, result)
}

fn finish_embedding_response_delivery(
    stream: &dyn EmbeddingServerStream,
    result: Result<()>,
) -> Result<()> {
    result?;
    // Do not inherit the wire deadline here: an authenticated same-user client
    // may choose any positive value and could otherwise retain every bounded
    // handler after receiving its response. The frozen query request deadline
    // is the smallest server-owned product budget and physically covers a
    // response larger than the Windows pipe buffer.
    stream
        .set_read_timeout(Some(EmbeddingClientBudgets::current().query_request))
        .context("bound embedding final response delivery")?;
    stream
        .finish_response_delivery()
        .context("finish embedding final response delivery")
}

fn serve_embedding_connection_inner(
    state: Arc<PerUserEmbeddingServerState>,
    stream: &mut dyn EmbeddingServerStream,
    handler_capacity_limited: bool,
) -> Result<()> {
    let pre_request_guard = (!handler_capacity_limited)
        .then(|| state.try_begin_pre_request())
        .flatten();
    if !stream.transport_identity().peer_verified {
        bail!("embedding_server_peer_unverified");
    }
    let transport_peer_pid = stream
        .transport_identity()
        .peer_pid
        .filter(|pid| *pid != 0)
        .ok_or_else(|| anyhow!("embedding_server_peer_process_identity_missing"))?;
    let transport_peer_process_start_id = stream
        .transport_identity()
        .peer_process_start_id
        .as_deref()
        .filter(|start_id| !start_id.is_empty())
        .ok_or_else(|| anyhow!("embedding_server_peer_process_identity_missing"))?
        .to_owned();
    stream
        .set_read_timeout(Some(EmbeddingClientBudgets::current().connect))
        .context("bound embedding server handshake read")?;
    stream
        .set_write_timeout(Some(EmbeddingClientBudgets::current().connect))
        .context("bound embedding server handshake write")?;
    let connection_id = Uuid::new_v4().to_string();
    let (hello_request, hello_payload): (EmbeddingProtocolRequest, Vec<u8>) = read_frame(stream)?;
    if !hello_payload.is_empty() {
        bail!("embedding_server_protocol_hello_required");
    }
    validate_protocol_request(&hello_request)?;
    let EmbeddingOperation::Hello {
        intent,
        client_pid,
        client_process_start_id,
        client_executable_sha256,
        client_executable_version,
    } = &hello_request.operation
    else {
        bail!("embedding_server_protocol_hello_required");
    };
    if *client_pid != transport_peer_pid
        || client_process_start_id != &transport_peer_process_start_id
    {
        bail!("embedding_server_peer_identity_mismatch");
    }
    if !is_sha256(client_executable_sha256) || client_executable_version.trim().is_empty() {
        bail!("embedding_server_peer_executable_identity_invalid");
    }
    let peer_executable_mismatch = client_executable_sha256 != &state.process.executable_sha256
        || client_executable_version != &state.process.executable_version;
    if !matches!(intent.as_str(), "activate" | "observe") {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &hello_request.request_id,
                protocol_error(
                    "embedding_server_intent_invalid",
                    "hello intent must be activate or observe",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if handler_capacity_limited || pre_request_guard.is_none() {
        let (reason, capacity, depth) = if handler_capacity_limited {
            (
                "connection_handler_full",
                SERVER_CONNECTION_HANDLER_CAPACITY,
                state.connections.load(Ordering::Acquire),
            )
        } else {
            (
                "pre_request_full",
                SERVER_CONTROL_CONNECTION_RESERVE,
                SERVER_CONTROL_CONNECTION_RESERVE,
            )
        };
        write_protocol_response(
            &mut *stream,
            failure_response(
                &hello_request.request_id,
                state.connection_capacity_error(reason, capacity, depth),
            ),
            &[],
        )?;
        return Ok(());
    }
    let observe_only = intent == "observe";
    let expected = EmbeddingCompatibility::current(
        state.engine_config.backend.device_class == NativeDeviceClass::Cpu,
    );
    let compatible = hello_request.compatibility == expected
        && !peer_executable_mismatch
        && !state
            .qualification
            .as_ref()
            .is_some_and(|control| control.force_incompatible.load(Ordering::Acquire));
    if !compatible {
        // Observe is a read-only contract. An incompatible status/doctor
        // process may report the live owner, but it must never transfer
        // authority or make that owner drain.
        let idle = !observe_only && state.begin_draining_if_idle();
        let error = EmbeddingProtocolError {
            code: if idle {
                "embedding_server_draining"
            } else {
                "embedding_server_incompatible_active_owner"
            }
            .into(),
            message: "the live per-user embedding server has an incompatible engine contract"
                .into(),
            retry_class: "after_owner_idle".into(),
            retry_after_ms: 0,
            retry_condition: "the incompatible server exits while fully idle".into(),
            capacity: None,
        };
        write_protocol_response(
            &mut *stream,
            failure_response(&hello_request.request_id, error),
            &[],
        )?;
        return Ok(());
    }
    write_protocol_response(
        &mut *stream,
        success_response(
            &hello_request.request_id,
            EmbeddingResult::Hello {
                compatibility_sha256: expected.digest()?,
                snapshot: Box::new(state.snapshot()),
            },
        ),
        &[],
    )?;

    let (request, payload): (EmbeddingProtocolRequest, Vec<u8>) = read_frame(&mut *stream)?;
    if let Err(error) = validate_protocol_request(&request) {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_protocol_mismatch",
                    &format!("embedding request protocol was rejected: {error}"),
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if !payload.is_empty() {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_request_payload_forbidden",
                    "request payload bytes are not accepted",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if request.compatibility != expected {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_compatibility_changed",
                    "request compatibility changed after hello",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    if observe_only && !matches!(request.operation, EmbeddingOperation::Snapshot) {
        write_protocol_response(
            &mut *stream,
            failure_response(
                &request.request_id,
                protocol_error(
                    "embedding_server_observe_operation_forbidden",
                    "observe connections may only request a snapshot",
                ),
            ),
            &[],
        )?;
        return Ok(());
    }
    drop(pre_request_guard);
    match request.operation.clone() {
        EmbeddingOperation::Snapshot => {
            let identity = state
                .try_initialized_engine()
                .and_then(|engine| engine.snapshot().ok())
                .and_then(|snapshot| {
                    engine_identity(&state.process.server_instance_id, &snapshot).ok()
                });
            write_protocol_response(
                &mut *stream,
                success_response(
                    &request.request_id,
                    EmbeddingResult::Snapshot {
                        snapshot: Box::new(state.snapshot()),
                        lease: None,
                        identity: identity.map(Box::new),
                    },
                ),
                &[],
            )?;
        }
        EmbeddingOperation::EnsureResident {
            scope_id,
            deadline_ms,
            retry_after_ms,
        } => {
            if deadline_ms == 0 {
                return write_deadline_invalid(&mut *stream, &request.request_id);
            }
            configure_server_operation_timeout(&*stream, deadline_ms)?;
            let started_ns = state.clock.now_ns();
            let context =
                EmbeddingRequestContext::new(&request.request_id, &scope_id, retry_after_ms);
            let admission =
                match state.try_admit_request(EmbeddingRequestClass::Bulk, retry_after_ms) {
                    Ok(admission) => admission,
                    Err(error) => {
                        return write_protocol_response(
                            &mut *stream,
                            failure_response(&request.request_id, *error),
                            &[],
                        );
                    }
                };
            let guard = state.begin_request(ServerRequestRegistration {
                connection_id: &connection_id,
                request_id: &request.request_id,
                scope_id: &scope_id,
                request_class: EmbeddingRequestClass::Bulk,
                phase: "ensure_resident",
                context,
                admission,
                cancellation_auth: None,
            })?;
            guard.update_phase("native_execution");
            let result = state
                .engine()
                .and_then(|engine| engine.ensure_resident().map_err(engine_error))
                .and_then(|snapshot| engine_identity(&state.process.server_instance_id, &snapshot));
            if elapsed_since(state.clock.as_ref(), started_ns) >= Duration::from_millis(deadline_ms)
            {
                return write_deadline_exceeded(
                    &mut *stream,
                    &request.request_id,
                    retry_after_ms,
                    EmbeddingRequestClass::Bulk,
                    state.initialized_engine().as_ref(),
                );
            }
            guard.update_phase("response");
            match result {
                Ok(identity) => write_protocol_response(
                    &mut *stream,
                    success_response(
                        &request.request_id,
                        EmbeddingResult::Identity {
                            identity: Box::new(identity),
                        },
                    ),
                    &[],
                )?,
                Err(error) => write_anyhow_failure(&mut *stream, &request.request_id, error)?,
            }
        }
        EmbeddingOperation::AcquireLease {
            scope_id,
            deadline_ms,
            retry_after_ms,
        } => {
            if deadline_ms == 0 {
                return write_deadline_invalid(&mut *stream, &request.request_id);
            }
            configure_server_operation_timeout(&*stream, deadline_ms)?;
            serve_lease_connection(
                &state,
                &connection_id,
                &mut *stream,
                ServerLeaseRequest {
                    request,
                    scope_id,
                    deadline_ms,
                    retry_after_ms,
                    compatibility: expected,
                },
            )?;
        }
        EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms,
            retry_after_ms,
            cancel_token,
            input,
        } => {
            if let Err(error) = validate_raw_inputs(std::slice::from_ref(&input)) {
                return write_protocol_response(
                    &mut *stream,
                    failure_response(
                        &request.request_id,
                        protocol_error(
                            "embedding_server_input_invalid",
                            &format!("embedding query input was rejected: {error}"),
                        ),
                    ),
                    &[],
                );
            }
            serve_embedding_request(
                &state,
                &connection_id,
                &mut *stream,
                ServerEmbeddingRequest {
                    request_id: &request.request_id,
                    scope_id,
                    request_class: EmbeddingRequestClass::Query,
                    deadline_ms,
                    retry_after_ms,
                    cancel_token,
                    client_pid: transport_peer_pid,
                    client_process_start_id: &transport_peer_process_start_id,
                    inputs: vec![format!("{CODERANK_QUERY_PREFIX}{input}")],
                },
            )?;
        }
        EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms,
            retry_after_ms,
            cancel_token,
            inputs,
        } => {
            if let Err(error) = validate_raw_inputs(&inputs) {
                return write_protocol_response(
                    &mut *stream,
                    failure_response(
                        &request.request_id,
                        protocol_error(
                            "embedding_server_input_invalid",
                            &format!("embedding document inputs were rejected: {error}"),
                        ),
                    ),
                    &[],
                );
            }
            let inputs = inputs
                .into_iter()
                .map(|input| format!("{CODERANK_DOCUMENT_PREFIX}{input}"))
                .collect();
            serve_embedding_request(
                &state,
                &connection_id,
                &mut *stream,
                ServerEmbeddingRequest {
                    request_id: &request.request_id,
                    scope_id,
                    request_class: EmbeddingRequestClass::Bulk,
                    deadline_ms,
                    retry_after_ms,
                    cancel_token,
                    client_pid: transport_peer_pid,
                    client_process_start_id: &transport_peer_process_start_id,
                    inputs,
                },
            )?;
        }
        EmbeddingOperation::Cancel {
            target_request_id,
            cancel_token,
        } => {
            if !valid_cancel_token(&cancel_token) {
                return write_protocol_response(
                    &mut *stream,
                    failure_response(
                        &request.request_id,
                        protocol_error(
                            "embedding_server_cancel_token_invalid",
                            "embedding cancellation requires an unguessable token",
                        ),
                    ),
                    &[],
                );
            }
            let cancelled = state.cancel(
                &target_request_id,
                &cancel_token,
                transport_peer_pid,
                &transport_peer_process_start_id,
            );
            write_protocol_response(
                &mut *stream,
                success_response(
                    &request.request_id,
                    if cancelled {
                        EmbeddingResult::Cancelled
                    } else {
                        EmbeddingResult::Released
                    },
                ),
                &[],
            )?;
        }
        EmbeddingOperation::Hello { .. } | EmbeddingOperation::ReleaseLease { .. } => {
            write_protocol_response(
                &mut *stream,
                failure_response(
                    &request.request_id,
                    protocol_error(
                        "embedding_server_operation_invalid",
                        "operation is invalid outside its connection state",
                    ),
                ),
                &[],
            )?;
        }
    }
    Ok(())
}
