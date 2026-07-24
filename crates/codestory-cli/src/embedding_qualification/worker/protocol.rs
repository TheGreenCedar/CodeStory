use super::gate::{elapsed, error_head, qualification_request_id, sha256_bytes};
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingClientTransport, EmbeddingCompatibility, EmbeddingOperation,
    EmbeddingProtocolRequest, EmbeddingProtocolResponse,
    EmbeddingQualificationWorkerProtocolExchange as WorkerProtocolExchange, EmbeddingResult,
    EmbeddingServerSnapshot, EmbeddingServerStream, EmbeddingTransportIdentity,
    PER_USER_EMBEDDING_CONSTANT_SET_SHA256, PER_USER_EMBEDDING_MAX_METADATA_BYTES,
    PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES, PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256,
    PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION, PER_USER_EMBEDDING_PROTOCOL_SHA256,
    PER_USER_EMBEDDING_PROTOCOL_V1, SidecarRuntimeConfig,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(25);

pub(super) fn connect_until(
    transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
    clock: &dyn AwakeMonotonicClock,
    budget: Duration,
    spawn_attempt: Option<&codestory_retrieval::EmbeddingSpawnAttempt>,
) -> Result<crate::embedding_server_transport::NativeEmbeddingStream> {
    let started = clock.now_ns();
    loop {
        match transport.connect_for_attempt(
            budget.saturating_sub(elapsed(clock, started)),
            spawn_attempt,
        )? {
            crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => {
                return Ok(stream);
            }
            crate::embedding_server_transport::NativeConnectOutcome::NoOwner
            | crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {}
        }
        if elapsed(clock, started) >= budget {
            bail!("embedding_qualification_connect_timeout");
        }
        clock.sleep(POLL);
    }
}

pub(super) fn validated_hello(
    stream: &mut crate::embedding_server_transport::NativeEmbeddingStream,
    transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<EmbeddingServerSnapshot> {
    validated_hello_with_intent(stream, transport, runtime, clock, "activate")
}

pub(super) fn validated_hello_with_intent(
    stream: &mut crate::embedding_server_transport::NativeEmbeddingStream,
    transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    intent: &str,
) -> Result<EmbeddingServerSnapshot> {
    EmbeddingServerStream::set_read_timeout(stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(stream, Some(Duration::from_secs(2)))?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let request_id = qualification_request_id("measurement-hello", clock.now_ns());
    write_protocol_frame(
        stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.clone(),
            compatibility: compatibility.clone(),
            operation: client_hello_operation(intent, transport),
        },
        &[],
    )?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) = read_protocol_frame(stream)?;
    if !payload.is_empty()
        || response.request_id != request_id
        || response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || response.error.is_some()
    {
        bail!("embedding_qualification_measurement_hello_invalid");
    }
    let (compatibility_sha256, snapshot) = match response.result {
        Some(EmbeddingResult::Hello {
            compatibility_sha256,
            snapshot,
        }) => (compatibility_sha256, *snapshot),
        _ => bail!("embedding_qualification_measurement_hello_missing"),
    };
    if compatibility_sha256 != compatibility.digest()? {
        bail!("embedding_qualification_measurement_hello_incompatible");
    }
    authenticate_snapshot(&snapshot, stream.identity())?;
    Ok(snapshot)
}

pub(super) fn client_hello_operation(
    intent: &str,
    transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
) -> EmbeddingOperation {
    let executable = EmbeddingClientTransport::executable_identity(transport);
    EmbeddingOperation::Hello {
        intent: intent.into(),
        client_pid: executable.pid,
        client_process_start_id: executable.process_start_id,
        client_executable_sha256: executable.executable_sha256,
        client_executable_version: executable.executable_version,
    }
}
pub(super) fn run_raw_protocol_exchange(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    class: &str,
    deadline_ms: u64,
) -> Result<WorkerProtocolExchange> {
    run_raw_protocol_exchange_with_input(runtime, clock, class, deadline_ms, None)
}

pub(super) fn run_raw_protocol_exchange_with_input(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    class: &str,
    deadline_ms: u64,
    measured_input: Option<String>,
) -> Result<WorkerProtocolExchange> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let stream = match transport.connect(Duration::from_secs(2))? {
        crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => stream,
        crate::embedding_server_transport::NativeConnectOutcome::NoOwner => {
            bail!("embedding_server_absent")
        }
        crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {
            bail!("embedding_server_owner_unresponsive")
        }
    };
    run_protocol_exchange_on_stream(
        runtime,
        clock,
        class,
        deadline_ms,
        measured_input,
        &transport,
        stream,
    )
}

pub(super) fn run_protocol_exchange_on_stream(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    class: &str,
    deadline_ms: u64,
    measured_input: Option<String>,
    transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
    mut stream: crate::embedding_server_transport::NativeEmbeddingStream,
) -> Result<WorkerProtocolExchange> {
    let transport_identity = stream.identity().clone();
    if !transport_identity.peer_verified
        || transport_identity.peer_pid.is_none()
        || transport_identity
            .peer_process_start_id
            .as_deref()
            .is_none_or(str::is_empty)
    {
        bail!("embedding_qualification_stall_peer_unverified");
    }
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_secs(2)))?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let hello_id = qualification_request_id("stall-hello", clock.now_ns());
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: hello_id.clone(),
            compatibility: compatibility.clone(),
            operation: client_hello_operation("activate", transport),
        },
        &[],
    )?;
    let (hello, payload): (EmbeddingProtocolResponse, Vec<u8>) = read_protocol_frame(&mut stream)?;
    if !payload.is_empty()
        || hello.request_id != hello_id
        || hello.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || hello.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || hello.error.is_some()
    {
        bail!("embedding_qualification_stall_hello_invalid");
    }
    let hello_snapshot = match hello.result {
        Some(EmbeddingResult::Hello { snapshot, .. }) => *snapshot,
        _ => bail!("embedding_qualification_stall_hello_missing"),
    };
    authenticate_snapshot(&hello_snapshot, &transport_identity)?;
    let request_id = qualification_request_id(&format!("qualification-{class}"), clock.now_ns());
    let scope_seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    let submitted_ns = clock.now_ns();
    let scope_id = sha256_bytes(scope_seed.as_bytes());
    let operation = match class {
        "query" => EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            cancel_token: None,
            input: measured_input.unwrap_or_else(|| "qualification-long-query".into()),
        },
        "bulk" => EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            cancel_token: None,
            inputs: vec![measured_input.unwrap_or_else(|| "qualification-long-bulk".into())],
        },
        _ => bail!("embedding_qualification_protocol_class_invalid"),
    };
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.clone(),
            compatibility,
            operation,
        },
        &[],
    )?;
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    let (response, response_payload_bytes, terminal_transport_error) =
        match read_protocol_frame::<EmbeddingProtocolResponse>(&mut stream) {
            Ok((response, payload)) => (Some(response), payload.len() as u64, None),
            Err(error) => (None, 0, Some(error_head(&error))),
        };
    let final_snapshot = if terminal_transport_error.is_none()
        && response
            .as_ref()
            .is_some_and(|response| response.error.is_none())
    {
        drop(stream);
        let mut snapshot_stream = match transport.connect(Duration::from_secs(2))? {
            crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => stream,
            crate::embedding_server_transport::NativeConnectOutcome::NoOwner => {
                bail!("embedding_server_absent")
            }
            crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {
                bail!("embedding_server_owner_unresponsive")
            }
        };
        let snapshot = validated_hello(&mut snapshot_stream, transport, runtime, clock)?;
        if !same_server_authority(&hello_snapshot, &snapshot) {
            bail!("embedding_qualification_protocol_owner_changed");
        }
        Some(snapshot)
    } else {
        None
    };
    Ok(WorkerProtocolExchange {
        request_id,
        submitted_ns,
        finished_ns: clock.now_ns(),
        transport_identity,
        hello_snapshot,
        final_snapshot,
        response,
        response_payload_bytes,
        terminal_transport_error,
    })
}

pub(super) fn authenticate_snapshot(
    snapshot: &EmbeddingServerSnapshot,
    transport: &EmbeddingTransportIdentity,
) -> Result<()> {
    if !snapshot.authority.peer_verified
        || snapshot.process.pid != transport.peer_pid.unwrap_or_default()
        || Some(snapshot.process.process_start_id.as_str())
            != transport.peer_process_start_id.as_deref()
        || snapshot.authority.endpoint_namespace_id != transport.endpoint_namespace_id
        || snapshot.authority.lifetime_authority_id != transport.lifetime_authority_id
        || snapshot.authority.listener_id != transport.listener_id
        || snapshot.protocol.protocol_sha256 != PER_USER_EMBEDDING_PROTOCOL_SHA256
        || snapshot.protocol.constant_set_sha256 != PER_USER_EMBEDDING_CONSTANT_SET_SHA256
        || snapshot.protocol.measurement_protocol_sha256
            != PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256
    {
        bail!("embedding_qualification_snapshot_authentication_failed");
    }
    Ok(())
}

fn same_server_authority(
    first: &EmbeddingServerSnapshot,
    second: &EmbeddingServerSnapshot,
) -> bool {
    first.process.server_instance_id == second.process.server_instance_id
        && first.process.pid == second.process.pid
        && first.process.process_start_id == second.process.process_start_id
        && first.authority.lifetime_authority_id == second.authority.lifetime_authority_id
        && first.authority.listener_id == second.authority.listener_id
}

pub(super) fn write_protocol_frame<T: Serialize>(
    stream: &mut dyn EmbeddingServerStream,
    control: &T,
    payload: &[u8],
) -> Result<()> {
    let control = serde_json::to_vec(control).context("serialize qualification protocol frame")?;
    if control.len() > PER_USER_EMBEDDING_MAX_METADATA_BYTES
        || payload.len() > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
    {
        bail!("embedding_server_frame_too_large");
    }
    stream.write_all(&(control.len() as u32).to_be_bytes())?;
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(&control)?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

pub(super) fn read_protocol_frame<T: for<'de> Deserialize<'de>>(
    stream: &mut dyn EmbeddingServerStream,
) -> Result<(T, Vec<u8>)> {
    let mut control_len = [0_u8; 4];
    let mut payload_len = [0_u8; 4];
    stream.read_exact(&mut control_len)?;
    stream.read_exact(&mut payload_len)?;
    let control_len = u32::from_be_bytes(control_len) as usize;
    let payload_len = u32::from_be_bytes(payload_len) as usize;
    if control_len == 0
        || control_len > PER_USER_EMBEDDING_MAX_METADATA_BYTES
        || payload_len > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
    {
        bail!("embedding_server_frame_too_large");
    }
    let mut control = vec![0_u8; control_len];
    let mut payload = vec![0_u8; payload_len];
    stream.read_exact(&mut control)?;
    stream.read_exact(&mut payload)?;
    Ok((serde_json::from_slice(&control)?, payload))
}
