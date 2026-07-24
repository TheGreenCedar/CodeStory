//! Bounded wire exchange, frame encoding, identity validation, and replay classification.

use super::protocol::hex_sha256;
use super::{
    AwakeMonotonicClock, EmbeddingClientBudgets, EmbeddingCompatibility, EmbeddingConnectIntent,
    EmbeddingEngineIdentity, EmbeddingEngineLeaseIdentity, EmbeddingExecutableIdentity,
    EmbeddingOperation, EmbeddingProtocolRequest, EmbeddingProtocolResponse, EmbeddingResult,
    EmbeddingServerProtocolSnapshot, EmbeddingServerSnapshot, EmbeddingServerStream,
    EmbeddingTransportIdentity, PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT,
    PER_USER_EMBEDDING_MAX_METADATA_BYTES, PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES,
    PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION, PER_USER_EMBEDDING_PROTOCOL_V1,
    PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION, PerUserEmbeddingError,
};
use crate::config::SidecarRuntimeConfig;
use crate::embedding_contract::RETRIEVAL_EMBEDDING_DIM;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::io;
use std::time::Duration;
use uuid::Uuid;

pub(super) fn request(
    request_id: &str,
    compatibility: EmbeddingCompatibility,
    operation: EmbeddingOperation,
) -> EmbeddingProtocolRequest {
    EmbeddingProtocolRequest {
        protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
        schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
        request_id: request_id.into(),
        compatibility,
        operation,
    }
}

pub(super) fn hello(
    stream: &mut dyn EmbeddingServerStream,
    intent: EmbeddingConnectIntent,
    compatibility: EmbeddingCompatibility,
    transport_identity: &EmbeddingTransportIdentity,
    executable: &EmbeddingExecutableIdentity,
) -> Result<EmbeddingServerSnapshot> {
    let request_id = Uuid::new_v4().to_string();
    let intent = match intent {
        EmbeddingConnectIntent::Activate => "activate",
        EmbeddingConnectIntent::Observe => "observe",
    };
    let (response, _) = exchange(
        stream,
        request(
            &request_id,
            compatibility.clone(),
            EmbeddingOperation::Hello {
                intent: intent.into(),
                client_pid: executable.pid,
                client_process_start_id: executable.process_start_id.clone(),
                client_executable_sha256: executable.executable_sha256.clone(),
                client_executable_version: executable.executable_version.clone(),
            },
        ),
    )?;
    let EmbeddingResult::Hello {
        compatibility_sha256,
        snapshot,
    } = response_result(response)?
    else {
        bail!("embedding_server_protocol_mismatch: expected hello");
    };
    if compatibility_sha256 != compatibility.digest()? {
        bail!("embedding_server_incompatible_active_owner");
    }
    validate_server_snapshot(&snapshot, transport_identity, executable)?;
    Ok(*snapshot)
}

pub(super) fn exchange(
    stream: &mut dyn EmbeddingServerStream,
    request: EmbeddingProtocolRequest,
) -> Result<(EmbeddingProtocolResponse, Vec<u8>)> {
    let request_id = request.request_id.clone();
    write_frame(stream, &request, &[])
        .map_err(|error| map_bounded_exchange_error(error, stream))?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(stream).map_err(|error| map_bounded_exchange_error(error, stream))?;
    if response.request_id != request_id {
        bail!("embedding_server_response_request_id_mismatch");
    }
    if response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
    {
        bail!("embedding_server_protocol_mismatch");
    }
    Ok((response, payload))
}

pub(super) fn map_bounded_exchange_error(
    error: anyhow::Error,
    stream: &dyn EmbeddingServerStream,
) -> anyhow::Error {
    let io_kind = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<io::Error>().map(io::Error::kind));
    if matches!(
        io_kind,
        Some(io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
    ) {
        return error.context(PerUserEmbeddingError {
            code: "embedding_server_owner_unresponsive".into(),
            message: "the embedding server did not complete a bounded exchange".into(),
            retry_class: "after_server_change".into(),
            retry_after_ms: duration_ms(EmbeddingClientBudgets::current().retry_after),
            retry_condition: "the lifetime authority or server instance changes".into(),
            capacity: None,
        });
    }
    if matches!(
        io_kind,
        Some(
            io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::NotConnected
                | io::ErrorKind::UnexpectedEof
        )
    ) {
        let raw_os_error = exchange_raw_os_error(&error);
        let identity = stream.transport_identity();
        let (peer_state, peer_exit_code) = match stream.peer_exit_code() {
            Ok(Some(exit_code)) => ("exited".to_string(), Some(exit_code)),
            Ok(None) => match stream.peer_is_alive() {
                Ok(true) => ("running".to_string(), None),
                Ok(false) => match stream.peer_exit_code() {
                    Ok(exit_code) => ("exited".to_string(), exit_code),
                    Err(probe_error) => (
                        format!("exited (exit-code probe failed: {probe_error})"),
                        None,
                    ),
                },
                Err(probe_error) => (format!("unknown ({probe_error})"), None),
            },
            Err(probe_error) => (format!("unknown ({probe_error})"), None),
        };
        let source_chain = format!("{error:#}");
        let message = format!(
            "the authenticated embedding server connection was lost; raw_os_error={}; \
             peer_pid={}; peer_process_start_id={}; peer_state={peer_state}; \
             peer_exit_code={}; source={source_chain}",
            raw_os_error.map_or_else(|| "none".into(), |code| code.to_string()),
            identity
                .peer_pid
                .map_or_else(|| "unknown".into(), |pid| pid.to_string()),
            identity
                .peer_process_start_id
                .as_deref()
                .unwrap_or("unknown"),
            peer_exit_code.map_or_else(|| "none".into(), |code| code.to_string()),
        );
        return error.context(PerUserEmbeddingError {
            code: "embedding_server_connection_lost".into(),
            message,
            retry_class: "same_rpc_once".into(),
            retry_after_ms: 0,
            retry_condition: "the server instance changes".into(),
            capacity: None,
        });
    }
    error
}

pub(super) fn exchange_raw_os_error(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .and_then(nested_io_raw_os_error)
    })
}

pub(super) fn nested_io_raw_os_error(error: &io::Error) -> Option<i32> {
    error.raw_os_error().or_else(|| {
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .and_then(nested_io_raw_os_error)
    })
}

pub(super) fn response_result(response: EmbeddingProtocolResponse) -> Result<EmbeddingResult> {
    if response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
    {
        bail!("embedding_server_protocol_mismatch");
    }
    if let Some(result) = response.result {
        return Ok(result);
    }
    let error = response
        .error
        .ok_or_else(|| anyhow!("embedding server returned neither result nor error"))?;
    Err(PerUserEmbeddingError {
        code: error.code,
        message: error.message,
        retry_class: error.retry_class,
        retry_after_ms: error.retry_after_ms,
        retry_condition: error.retry_condition,
        capacity: error.capacity,
    }
    .into())
}

pub(super) fn vectors_result(
    result: (EmbeddingResult, Vec<u8>),
) -> Result<(u32, u32, EmbeddingEngineIdentity, Vec<u8>)> {
    let (
        EmbeddingResult::Vectors {
            rows,
            columns,
            encoding,
            identity,
        },
        payload,
    ) = result
    else {
        bail!("embedding_server_protocol_mismatch: expected vectors");
    };
    if encoding != "f32_le" {
        bail!("embedding_vector_encoding_mismatch");
    }
    Ok((rows, columns, *identity, payload))
}

pub(super) fn write_frame<T: Serialize>(
    stream: &mut dyn EmbeddingServerStream,
    control: &T,
    payload: &[u8],
) -> Result<()> {
    let control = serde_json::to_vec(control).context("serialize embedding protocol frame")?;
    if control.len() > PER_USER_EMBEDDING_MAX_METADATA_BYTES
        || payload.len() > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
    {
        bail!("embedding_server_frame_too_large");
    }
    stream
        .write_all(&(control.len() as u32).to_be_bytes())
        .context("write embedding control length")?;
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .context("write embedding payload length")?;
    stream
        .write_all(&control)
        .context("write embedding control frame")?;
    stream
        .write_all(payload)
        .context("write embedding payload frame")?;
    stream.flush().context("flush embedding protocol frame")
}

pub(super) fn read_frame<T: for<'de> Deserialize<'de>>(
    stream: &mut dyn EmbeddingServerStream,
) -> Result<(T, Vec<u8>)> {
    let mut control_len = [0_u8; 4];
    let mut payload_len = [0_u8; 4];
    stream
        .read_exact(&mut control_len)
        .context("read embedding control length")?;
    stream
        .read_exact(&mut payload_len)
        .context("read embedding payload length")?;
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
    stream
        .read_exact(&mut control)
        .context("read embedding control frame")?;
    stream
        .read_exact(&mut payload)
        .context("read embedding payload frame")?;
    let control =
        serde_json::from_slice(&control).context("decode embedding protocol control frame")?;
    Ok((control, payload))
}

pub(super) fn encode_vectors(vectors: &[Vec<f32>]) -> Result<Vec<u8>> {
    if vectors.len() > PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT
        || vectors
            .iter()
            .any(|vector| vector.len() != RETRIEVAL_EMBEDDING_DIM)
    {
        bail!("embedding_vector_shape_invalid");
    }
    let bytes = vectors
        .len()
        .checked_mul(RETRIEVAL_EMBEDDING_DIM)
        .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| anyhow!("embedding_vector_payload_overflow"))?;
    if bytes > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES {
        bail!("embedding_vector_payload_too_large");
    }
    let mut payload = Vec::with_capacity(bytes);
    for vector in vectors {
        for value in vector {
            payload.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(payload)
}

pub(super) fn decode_vectors(rows: u32, columns: u32, payload: &[u8]) -> Result<Vec<Vec<f32>>> {
    if columns as usize != RETRIEVAL_EMBEDDING_DIM {
        bail!(
            "embedding_vector_dimension_mismatch: expected={} observed={columns}",
            RETRIEVAL_EMBEDDING_DIM
        );
    }
    let expected = (rows as usize)
        .checked_mul(columns as usize)
        .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| anyhow!("embedding_vector_payload_overflow"))?;
    if payload.len() != expected {
        bail!(
            "embedding_vector_payload_length_mismatch: expected={expected} observed={}",
            payload.len()
        );
    }
    let mut vectors = Vec::with_capacity(rows as usize);
    for row in payload.chunks_exact(columns as usize * std::mem::size_of::<f32>()) {
        let vector = row
            .chunks_exact(std::mem::size_of::<f32>())
            .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte f32 chunk")))
            .collect();
        vectors.push(vector);
    }
    Ok(vectors)
}

pub(super) fn validate_engine_identity(
    identity: &EmbeddingEngineIdentity,
    compatibility: &EmbeddingCompatibility,
) -> Result<()> {
    if !identity.worker_alive || identity.load_error.is_some() {
        bail!("embedding_server_engine_unavailable");
    }
    if identity.model_digest != compatibility.model_sha256
        || identity.ggml_build_identity != compatibility.ggml_build_identity
        || identity.policy != compatibility.policy
        || identity.materialized_model_sha256 != compatibility.model_sha256
        || identity.load_generation == 0
    {
        bail!("embedding_server_engine_identity_mismatch");
    }
    Ok(())
}

pub(super) fn validate_engine_server_identity(
    identity: &EmbeddingEngineIdentity,
    server: &EmbeddingServerSnapshot,
) -> Result<()> {
    if identity.server_instance_id != server.process.server_instance_id {
        bail!("embedding_server_instance_changed");
    }
    Ok(())
}

pub(super) fn validate_lease_server_identity(
    lease: &EmbeddingEngineLeaseIdentity,
    identity: &EmbeddingEngineIdentity,
    server: &EmbeddingServerSnapshot,
) -> Result<()> {
    if lease.server_instance_id != server.process.server_instance_id
        || lease.server_instance_id != identity.server_instance_id
        || lease.load_generation != identity.load_generation
        || lease.lease_token.trim().is_empty()
    {
        bail!("embedding_publication_lease_changed");
    }
    Ok(())
}

pub(super) fn validate_same_server(
    observed: &EmbeddingServerSnapshot,
    accepted: &EmbeddingServerSnapshot,
) -> Result<()> {
    if observed.process != accepted.process
        || observed.protocol != accepted.protocol
        || observed.authority != accepted.authority
    {
        bail!("embedding_server_instance_changed");
    }
    Ok(())
}

pub(super) fn validate_server_snapshot(
    snapshot: &EmbeddingServerSnapshot,
    transport: &EmbeddingTransportIdentity,
    executable: &EmbeddingExecutableIdentity,
) -> Result<()> {
    if snapshot.schema_version != PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION
        || snapshot.protocol != EmbeddingServerProtocolSnapshot::current()
        || snapshot.process.server_instance_id.trim().is_empty()
        || snapshot.process.pid == 0
        || snapshot.process.process_start_id.trim().is_empty()
        || snapshot.process.executable_sha256.trim().is_empty()
        || snapshot.process.executable_version.trim().is_empty()
    {
        bail!("embedding_server_snapshot_contract_mismatch");
    }
    if !transport.peer_verified
        || !snapshot.authority.peer_verified
        || snapshot.authority.endpoint_namespace_id != transport.endpoint_namespace_id
        || snapshot.authority.lifetime_authority_id != transport.lifetime_authority_id
        || snapshot.authority.listener_id != transport.listener_id
        || transport.peer_pid != Some(snapshot.process.pid)
        || transport.peer_process_start_id.as_deref()
            != Some(snapshot.process.process_start_id.as_str())
    {
        bail!("embedding_server_peer_identity_mismatch");
    }
    if snapshot.process.executable_sha256 != executable.executable_sha256
        || snapshot.process.executable_version != executable.executable_version
    {
        bail!("embedding_server_executable_identity_mismatch");
    }
    Ok(())
}

pub(super) fn configure_exchange_timeout(
    stream: &dyn EmbeddingServerStream,
    timeout: Duration,
) -> Result<()> {
    if timeout.is_zero() {
        bail!("embedding_server_timeout_invalid");
    }
    stream
        .set_read_timeout(Some(timeout))
        .map_err(exchange_timeout_configuration_error)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(exchange_timeout_configuration_error)?;
    Ok(())
}

pub(super) fn exchange_timeout_configuration_error(error: io::Error) -> anyhow::Error {
    PerUserEmbeddingError {
        code: "embedding_server_owner_unresponsive".into(),
        message: format!("could not bound the embedding server exchange: {error}"),
        retry_class: "after_server_change".into(),
        retry_after_ms: duration_ms(EmbeddingClientBudgets::current().retry_after),
        retry_condition: "the lifetime authority or server instance changes".into(),
        capacity: None,
    }
    .into()
}

pub(super) fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn embedding_scope_id(runtime: &SidecarRuntimeConfig) -> String {
    let scope_seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    hex_sha256(scope_seed.as_bytes())
}

pub(super) fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

pub(super) fn positive_duration_ms(duration: Duration) -> u64 {
    duration_ms(duration).max(1)
}

pub(super) fn elapsed_since(clock: &dyn AwakeMonotonicClock, started_ns: u64) -> Duration {
    Duration::from_nanos(clock.now_ns().saturating_sub(started_ns))
}

pub(super) fn is_server_loss(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<PerUserEmbeddingError>()
        .is_some_and(|error| {
            matches!(
                error.code.as_str(),
                "embedding_server_owner_unresponsive" | "embedding_server_connection_lost"
            )
        })
}
