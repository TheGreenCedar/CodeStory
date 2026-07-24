use super::super::{
    EmbeddingClientBudgets, EmbeddingCompatibility, EmbeddingEngineLeaseIdentity,
    EmbeddingOperation, EmbeddingProtocolRequest, EmbeddingResult, IncrementalProtocolFrameReader,
    PER_USER_EMBEDDING_MAX_METADATA_BYTES, PER_USER_EMBEDDING_PROTOCOL_SHA256,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS, PER_USER_EMBEDDING_SERVER_PROOF_MARKER,
    PerUserEmbeddingServerState, ProtocolFramePoll, ServerLeaseActivity,
    configure_server_operation_timeout, elapsed_since, embedding_retry_state, exchange,
    exchange_raw_os_error, hex_sha256, map_bounded_exchange_error, read_frame, request,
    serve_embedding_connection, success_response, validate_lease_server_identity,
    validate_same_server, validate_server_snapshot,
};
use super::{
    MemoryStream, PollingStream, encode_test_frame, test_engine_identity, test_executable,
    test_hello_operation, test_server_state, test_snapshot, test_transport_identity,
};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn response_correlation_and_protocol_hashes_are_enforced() {
    let response = success_response("wrong", EmbeddingResult::Released);
    let (mut stream, _) = MemoryStream::new(encode_test_frame(&response, &[]), true);
    let error = exchange(
        &mut stream,
        request(
            "expected",
            EmbeddingCompatibility::current(true),
            EmbeddingOperation::Snapshot,
        ),
    )
    .expect_err("wrong response id");
    assert!(
        error
            .to_string()
            .contains("embedding_server_response_request_id_mismatch")
    );

    validate_server_snapshot(
        &test_snapshot(),
        &test_transport_identity(),
        &test_executable(),
    )
    .expect("same exact executable digest is compatible");

    let mut snapshot = test_snapshot();
    snapshot.protocol.protocol_sha256 = "wrong".into();
    assert!(
        validate_server_snapshot(&snapshot, &test_transport_identity(), &test_executable(),)
            .is_err()
    );

    let mut snapshot = test_snapshot();
    snapshot.process.executable_sha256 = "b".repeat(64);
    let error = validate_server_snapshot(&snapshot, &test_transport_identity(), &test_executable())
        .expect_err("snapshot executable digest mismatch");
    assert!(
        error
            .to_string()
            .contains("embedding_server_executable_identity_mismatch")
    );
}

#[test]
fn checked_in_protocol_hash_flows_into_the_build_marker() {
    let expected = hex_sha256(include_bytes!(
        "../../../../../docs/testing/per-user-embedding-server-protocol.json"
    ));
    assert_eq!(PER_USER_EMBEDDING_PROTOCOL_SHA256, expected);

    let marker = std::str::from_utf8(PER_USER_EMBEDDING_SERVER_PROOF_MARKER).expect("UTF-8 marker");
    assert!(
        marker.contains(&format!("protocol_sha256={expected}|")),
        "build marker did not bind the checked-in protocol hash: {marker}"
    );
}

#[test]
fn transport_identity_contains_no_peer_image_hash() {
    let identity =
        serde_json::to_value(test_transport_identity()).expect("serialize transport identity");
    assert!(identity.get("peer_executable_sha256").is_none());
    assert_eq!(identity["peer_pid"], 42);
    assert_eq!(identity["peer_process_start_id"], "server-start");
}

#[test]
fn hello_process_start_claim_must_match_authenticated_transport() {
    let mut operation = test_hello_operation("observe");
    let EmbeddingOperation::Hello {
        client_process_start_id,
        ..
    } = &mut operation
    else {
        unreachable!("test helper always builds hello");
    };
    *client_process_start_id = "stale-start".into();
    let hello = request(
        "stale-client",
        EmbeddingCompatibility::current(true),
        operation,
    );
    let fixture = MemoryStream::with_delivery_tracking(encode_test_frame(&hello, &[]), true);
    let error = serve_embedding_connection(test_server_state(), Box::new(fixture.stream))
        .expect_err("stale client identity must fail");
    assert!(
        error
            .to_string()
            .contains("embedding_server_peer_identity_mismatch")
    );
    assert_eq!(
        fixture.finished_deliveries.load(Ordering::Acquire),
        0,
        "an uncorrelated protocol failure must not pretend response delivery completed"
    );
}

#[test]
fn bounded_frames_reject_oversized_lengths_before_allocation() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&((PER_USER_EMBEDDING_MAX_METADATA_BYTES + 1) as u32).to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    let (mut stream, _) = MemoryStream::new(bytes, true);
    let error = read_frame::<serde_json::Value>(&mut stream).expect_err("oversized frame");
    assert!(
        error
            .to_string()
            .contains("embedding_server_frame_too_large")
    );
}

#[test]
fn server_response_write_timeout_cannot_exceed_the_frozen_query_budget() {
    let fixture = MemoryStream::with_delivery_tracking(Vec::new(), true);

    configure_server_operation_timeout(&fixture.stream, 24 * 60 * 60 * 1_000)
        .expect("configure peer-selected exchange deadline");

    let expected = Some(EmbeddingClientBudgets::current().query_request);
    assert_eq!(
        fixture
            .read_timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .copied()
            .flatten(),
        expected,
        "a peer-selected deadline must not retain a server read beyond the frozen cap"
    );
    assert_eq!(
        fixture
            .write_timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .copied()
            .flatten(),
        expected,
        "a non-reading peer must not retain a response writer beyond the frozen cap"
    );
}

#[test]
fn bounded_exchange_maps_normalized_disconnect_and_retains_peer_evidence() {
    let raw_error = io::Error::from_raw_os_error(233);
    let normalized = io::Error::new(io::ErrorKind::NotConnected, raw_error);
    let source = anyhow::Error::new(normalized).context("read embedding control length");
    let (stream, _) = MemoryStream::new(Vec::new(), true);

    let error = map_bounded_exchange_error(source, &stream);
    let retry = embedding_retry_state(&error).expect("typed connection loss");

    assert_eq!(retry.code, "embedding_server_connection_lost");
    assert_eq!(retry.retry_class, "same_rpc_once");
    assert!(retry.message.contains("raw_os_error=233"));
    assert!(retry.message.contains("peer_pid=42"));
    assert!(retry.message.contains("peer_process_start_id=server-start"));
    assert!(retry.message.contains("peer_state=running"));
    assert!(retry.message.contains("peer_exit_code=none"));
    assert!(
        retry
            .message
            .contains("source=read embedding control length")
    );
    assert_eq!(exchange_raw_os_error(&error), Some(233));
}

#[test]
fn bounded_exchange_does_not_type_unrelated_io_errors() {
    let source = anyhow::Error::new(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unrelated denial",
    ))
    .context("read embedding control length");
    let (stream, _) = MemoryStream::new(Vec::new(), false);

    let error = map_bounded_exchange_error(source, &stream);

    assert!(embedding_retry_state(&error).is_none());
    assert_eq!(error.to_string(), "read embedding control length");
}

#[test]
fn bounded_exchange_reprobes_exit_code_after_liveness_observes_exit() {
    let raw_error = io::Error::from_raw_os_error(233);
    let normalized = io::Error::new(io::ErrorKind::NotConnected, raw_error);
    let source = anyhow::Error::new(normalized).context("read embedding control length");
    let (mut stream, _) = MemoryStream::new(Vec::new(), false);
    stream.exit_codes = Mutex::new(vec![None, Some(0xc000_0005)]);

    let error = map_bounded_exchange_error(source, &stream);
    let retry = embedding_retry_state(&error).expect("typed connection loss");

    assert!(retry.message.contains("peer_state=exited"));
    assert!(retry.message.contains("peer_exit_code=3221225477"));
}

#[test]
fn held_lease_reader_survives_repeated_timeouts_then_decodes() {
    let frame = encode_test_frame(
        &request(
            "lease-snapshot",
            EmbeddingCompatibility::current(true),
            EmbeddingOperation::Snapshot,
        ),
        &[],
    );
    let (inner, _) = MemoryStream::new(frame, true);
    let mut stream = PollingStream {
        inner,
        pending_reads: 4,
    };
    let mut reader = IncrementalProtocolFrameReader::default();
    for _ in 0..4 {
        assert!(matches!(
            reader
                .poll::<EmbeddingProtocolRequest>(&mut stream)
                .expect("bounded poll"),
            ProtocolFramePoll::Pending
        ));
    }
    assert!(matches!(
        reader
            .poll::<EmbeddingProtocolRequest>(&mut stream)
            .expect("eventual frame"),
        ProtocolFramePoll::Ready((
            EmbeddingProtocolRequest {
                operation: EmbeddingOperation::Snapshot,
                ..
            },
            _
        ))
    ));
}

#[test]
fn lease_and_server_identity_drift_fail_closed() {
    let snapshot = test_snapshot();
    let identity = test_engine_identity();
    let mut lease = EmbeddingEngineLeaseIdentity {
        lease_token: "lease".into(),
        server_instance_id: snapshot.process.server_instance_id.clone(),
        load_generation: identity.load_generation,
        compatibility_sha256: "compat".into(),
    };
    assert!(validate_lease_server_identity(&lease, &identity, &snapshot).is_ok());
    lease.load_generation += 1;
    assert!(validate_lease_server_identity(&lease, &identity, &snapshot).is_err());
    let mut changed = snapshot.clone();
    changed.process.server_instance_id = "other".into();
    assert!(validate_same_server(&changed, &snapshot).is_err());
}

#[test]
fn lease_end_restarts_the_full_true_idle_window_before_native_release() {
    struct LeaseDropProbe {
        state: Arc<PerUserEmbeddingServerState>,
        observed_idle_start: Arc<AtomicU64>,
    }

    impl Drop for LeaseDropProbe {
        fn drop(&mut self) {
            self.observed_idle_start.store(
                self.state.last_work_ended_ns.load(Ordering::Acquire),
                Ordering::Release,
            );
        }
    }

    let state = test_server_state();
    let observed_idle_start = Arc::new(AtomicU64::new(0));
    let lease = ServerLeaseActivity::new(
        &state,
        LeaseDropProbe {
            state: Arc::clone(&state),
            observed_idle_start: Arc::clone(&observed_idle_start),
        },
    );
    state.clock.sleep(Duration::from_secs(75));

    drop(lease);

    let idle_start = state.last_work_ended_ns.load(Ordering::Acquire);
    assert_eq!(idle_start, state.clock.now_ns());
    assert_eq!(
        observed_idle_start.load(Ordering::Acquire),
        idle_start,
        "the idle clock must reset before the wrapped native lease is released"
    );
    state.clock.sleep(Duration::from_millis(
        PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS - 1,
    ));
    assert!(
        elapsed_since(state.clock.as_ref(), idle_start)
            < Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
    );
    state.clock.sleep(Duration::from_millis(1));
    assert_eq!(
        elapsed_since(state.clock.as_ref(), idle_start),
        Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
    );
}
