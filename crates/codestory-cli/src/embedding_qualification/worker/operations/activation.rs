use super::super::contracts::{WorkerError, WorkerProtocolExchange};
use super::super::gate::qualification_request_id;
use super::super::protocol::{
    client_hello_operation, connect_until, read_protocol_frame, run_protocol_exchange_on_stream,
    write_protocol_frame,
};
use super::ANTI_IDLE_PROTOCOL_DEADLINE_MS;
use anyhow::{Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingCompatibility, EmbeddingProtocolRequest,
    EmbeddingProtocolResponse, EmbeddingServerStream, PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
    PER_USER_EMBEDDING_PROTOCOL_V1, SidecarRuntimeConfig,
};
use std::time::Duration;

pub(in crate::embedding_qualification::worker) fn run_activate_probe(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<WorkerError> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let mut stream = match transport.connect(Duration::from_secs(2))? {
        crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => stream,
        crate::embedding_server_transport::NativeConnectOutcome::NoOwner => {
            bail!("embedding_server_absent")
        }
        crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {
            bail!("embedding_server_owner_unresponsive")
        }
    };
    let identity = stream.identity();
    if !identity.peer_verified
        || identity.peer_pid.is_none()
        || identity
            .peer_process_start_id
            .as_deref()
            .is_none_or(str::is_empty)
    {
        bail!("embedding_qualification_activate_peer_unverified");
    }
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_secs(2)))?;
    let request_id = qualification_request_id("activate-probe", clock.now_ns());
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.clone(),
            compatibility: EmbeddingCompatibility::current(runtime.embedding.allow_cpu),
            operation: client_hello_operation("activate", &transport),
        },
        &[],
    )?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_protocol_frame(&mut stream)?;
    if !payload.is_empty()
        || response.request_id != request_id
        || response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || response.result.is_some()
    {
        bail!("embedding_qualification_activate_response_invalid");
    }
    let error = response
        .error
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_activate_error_missing"))?;
    Ok(WorkerError {
        code: error.code,
        message_head: error.message.chars().take(128).collect(),
        retry_class: error.retry_class,
        retry_after_ms: error.retry_after_ms,
        retry_condition: error.retry_condition,
        capacity: error.capacity,
    })
}

pub(in crate::embedding_qualification::worker) fn run_cold_race_protocol_exchange(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<WorkerProtocolExchange> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let spawn_attempt = transport.spawn_exact_current_exe()?;
    let stream = connect_until(
        &transport,
        clock,
        Duration::from_secs(15),
        Some(&spawn_attempt),
    )?;
    run_protocol_exchange_on_stream(
        runtime,
        clock,
        "query",
        ANTI_IDLE_PROTOCOL_DEADLINE_MS,
        None,
        &transport,
        stream,
    )
}
