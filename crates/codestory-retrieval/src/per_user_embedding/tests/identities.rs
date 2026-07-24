use super::super::{
    AwakeMonotonicClock, EmbeddingCapacityPressureWire, EmbeddingClientTransport,
    EmbeddingCompatibility, EmbeddingEngineIdentity, EmbeddingExecutableIdentity,
    EmbeddingOperation, EmbeddingProtocolError, EmbeddingProtocolResponse, EmbeddingRequestClass,
    EmbeddingRequestContext, EmbeddingServerAuthoritySnapshot, EmbeddingServerProcessSnapshot,
    EmbeddingServerProtocolSnapshot, EmbeddingServerSchedulerSnapshot, EmbeddingServerSnapshot,
    EmbeddingTransportIdentity, PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
    PerUserEmbeddingClient, PerUserEmbeddingServerState, ServerCancellationAuth,
    ServerQualificationControl, ServerQualificationEvent, ServerQualificationEventClock,
    ServerRequestAdmission, ServerRequestGuard, ServerRequestRegistration, read_frame, request,
    serve_embedding_connection, server_qualification_control_from_values,
};
use super::transport_fixtures::{MemoryStream, TestClock};
use crate::embedding_contract::{EMBEDDING_MODEL_SHA256, native_engine_config};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};

pub(super) fn test_client<T>(transport: Arc<T>) -> PerUserEmbeddingClient
where
    T: EmbeddingClientTransport + 'static,
{
    PerUserEmbeddingClient {
        transport,
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    }
}

pub(super) fn test_server_state() -> Arc<PerUserEmbeddingServerState> {
    let clock = TestClock::new();
    Arc::new(PerUserEmbeddingServerState {
        clock,
        engine_cache_root: PathBuf::from("test-cache"),
        engine_config: native_engine_config(true).expect("CPU engine config"),
        engine: Mutex::new(None),
        process: test_snapshot().process,
        protocol: EmbeddingServerProtocolSnapshot::current(),
        authority: test_snapshot().authority,
        connections: AtomicUsize::new(0),
        pre_request_connections: AtomicUsize::new(0),
        admission_gate: Mutex::new(()),
        request_admission: Arc::new(ServerRequestAdmission::default()),
        active: Mutex::new(std::collections::BTreeMap::new()),
        cancellations: Mutex::new(std::collections::BTreeMap::new()),
        draining: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        last_work_ended_ns: AtomicU64::new(1),
        event_sequence: AtomicU64::new(1),
        last_failure: Mutex::new(None),
        qualification: None,
    })
}

#[cfg(any(unix, windows))]
pub(super) fn test_qualification_control() -> (tempfile::TempDir, ServerQualificationControl) {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("temporary qualification root");
    let directory = temporary.path().join("qualification");
    fs::create_dir(&directory).expect("qualification directory");
    #[cfg(unix)]
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .expect("private qualification directory");
    let canonical = fs::canonicalize(&directory).expect("canonical qualification directory");
    let control = server_qualification_control_from_values(
        Some(canonical.into_os_string()),
        Some("test-nonce".into()),
    )
    .expect("valid qualification control")
    .expect("qualification control is enabled");
    (temporary, control)
}

#[cfg(any(unix, windows))]
pub(super) fn test_qualification_event() -> ServerQualificationEvent {
    ServerQualificationEvent {
        schema_version: 1,
        sequence: 1,
        action: "snapshot".into(),
        status: "completed".into(),
        server_event_sequence: 1,
        clock: ServerQualificationEventClock {
            domain: "awake_monotonic".into(),
            api: "test".into(),
            boot_id: "test-boot".into(),
            observed_ns: 1,
        },
        snapshot: None,
        details: None,
    }
}

pub(super) fn begin_test_request(
    state: &Arc<PerUserEmbeddingServerState>,
    request_class: EmbeddingRequestClass,
    request_id: &str,
) -> ServerRequestGuard {
    let admission = state
        .try_admit_request(request_class, 11)
        .expect("request is within the class bound");
    let connection_id = format!("connection-{request_id}");
    let scope_id = format!("scope-{request_id}");
    state
        .begin_request(ServerRequestRegistration {
            connection_id: &connection_id,
            request_id,
            scope_id: &scope_id,
            request_class,
            phase: "queued",
            context: EmbeddingRequestContext::new(request_id, &scope_id, 11),
            admission,
            cancellation_auth: Some(ServerCancellationAuth {
                token: test_cancel_token(),
                client_pid: test_executable().pid,
                client_process_start_id: test_executable().process_start_id,
            }),
        })
        .expect("admitted request enters bounded active state")
}

pub(super) fn test_cancel_token() -> String {
    "b9236f3d-c1f4-4af0-8c73-6d6574c40c5e".into()
}

pub(super) fn serve_mismatched_peer_hello(
    state: &Arc<PerUserEmbeddingServerState>,
    intent: &str,
) -> EmbeddingProtocolError {
    let mut operation = test_hello_operation(intent);
    let EmbeddingOperation::Hello {
        client_executable_sha256,
        ..
    } = &mut operation
    else {
        unreachable!("test helper always builds hello");
    };
    *client_executable_sha256 = "b".repeat(64);
    let hello = request(
        "upgrade-hello",
        EmbeddingCompatibility::current(true),
        operation,
    );
    let (stream, output) = MemoryStream::new(encode_test_frame(&hello, &[]), true);
    serve_embedding_connection(Arc::clone(state), Box::new(stream))
        .expect("upgrade incompatibility is correlated");
    let bytes = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let (mut output_stream, _) = MemoryStream::new(bytes, true);
    let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(&mut output_stream).expect("upgrade response");
    response.error.expect("typed upgrade incompatibility")
}

pub(super) fn test_executable() -> EmbeddingExecutableIdentity {
    EmbeddingExecutableIdentity {
        pid: 42,
        process_start_id: "server-start".into(),
        executable_sha256: "a".repeat(64),
        executable_version: "0.16.0".into(),
    }
}

pub(super) fn test_hello_operation(intent: &str) -> EmbeddingOperation {
    EmbeddingOperation::Hello {
        intent: intent.into(),
        client_pid: 42,
        client_process_start_id: "server-start".into(),
        client_executable_sha256: "a".repeat(64),
        client_executable_version: "0.16.0".into(),
    }
}

pub(super) fn test_transport_identity() -> EmbeddingTransportIdentity {
    EmbeddingTransportIdentity {
        endpoint_namespace_id: "endpoint".into(),
        lifetime_authority_id: "authority".into(),
        listener_id: "listener".into(),
        peer_verified: true,
        peer_pid: Some(42),
        peer_process_start_id: Some("server-start".into()),
    }
}

pub(super) fn test_snapshot() -> EmbeddingServerSnapshot {
    EmbeddingServerSnapshot {
        schema_version: PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
        event_sequence: 1,
        lifecycle: "listening".into(),
        clock: TestClock::new().snapshot(),
        protocol: EmbeddingServerProtocolSnapshot::current(),
        authority: EmbeddingServerAuthoritySnapshot {
            endpoint_namespace_id: "endpoint".into(),
            lifetime_authority_id: "authority".into(),
            listener_id: "listener".into(),
            peer_verified: true,
        },
        process: EmbeddingServerProcessSnapshot {
            server_instance_id: "server".into(),
            pid: 42,
            process_start_id: "server-start".into(),
            executable_sha256: "a".repeat(64),
            executable_version: "0.16.0".into(),
        },
        scheduler: EmbeddingServerSchedulerSnapshot {
            query_capacity: 64,
            query_depth: 0,
            bulk_capacity: 64,
            bulk_depth: 0,
            connection_count: 1,
            active_request_count: 0,
            lease_count: 0,
            active_request: None,
        },
        engine: None,
        failure: None,
    }
}

pub(super) fn test_engine_identity() -> EmbeddingEngineIdentity {
    EmbeddingEngineIdentity {
        server_instance_id: "server".into(),
        load_generation: 1,
        model_load_count: 1,
        residency: "resident".into(),
        worker_alive: true,
        load_error: None,
        model_digest: EMBEDDING_MODEL_SHA256.into(),
        ggml_build_identity: codestory_llama_sys::GGML_BUILD_IDENTITY.into(),
        backend: "CPU".into(),
        adapter_name: "CPU".into(),
        adapter_description: "test".into(),
        policy: "cpu_explicit".into(),
        embedded_model: true,
        materialized_model_sha256: EMBEDDING_MODEL_SHA256.into(),
        materialized_reused: true,
        initialization_ms: 1,
        smoke_ms: 1,
        adapter_memory_total: 1024,
        adapter_memory_used_by_load: 512,
        execution_device_names: Vec::new(),
        execution_backend_names: Vec::new(),
        execution_observation_source: "ggml_eval_callback".into(),
        encode_count: 1,
        execution_node_count: 0,
        resident_accelerator_tensor_count: 0,
        resident_accelerator_tensor_bytes: 0,
        model_layer_count: 13,
        offloaded_layer_count: 0,
        accelerator_execution_verified: false,
    }
}

pub(super) fn test_capacity() -> EmbeddingCapacityPressureWire {
    EmbeddingCapacityPressureWire {
        reason: "queue_full".into(),
        queue_class: "query".into(),
        capacity: 64,
        depth: 64,
        retry_after_ms: 10,
        retry_condition: "a live request completes".into(),
        owner_state: "ready".into(),
        active_scope_id: None,
        active_request_id: None,
        active_request_class: None,
    }
}

pub(super) fn encode_test_frame<T: Serialize>(value: &T, payload: &[u8]) -> Vec<u8> {
    let control = serde_json::to_vec(value).expect("test frame JSON");
    let mut frame = Vec::with_capacity(8 + control.len() + payload.len());
    frame.extend_from_slice(&(control.len() as u32).to_be_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&control);
    frame.extend_from_slice(payload);
    frame
}

pub(super) fn decode_test_frame<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    if bytes.len() < 8 {
        bail!("test frame is incomplete");
    }
    let control_len =
        u32::from_be_bytes(bytes[0..4].try_into().expect("four-byte frame length")) as usize;
    serde_json::from_slice(&bytes[8..8 + control_len]).context("decode test frame")
}
