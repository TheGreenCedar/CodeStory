use super::super::{
    ActiveServerRequest, AwakeMonotonicClock, EMBEDDING_BULK_QUEUE_CAPACITY,
    EMBEDDING_QUERY_QUEUE_CAPACITY, EmbeddingClientBudgets, EmbeddingCompatibility,
    EmbeddingOperation, EmbeddingProtocolResponse, EmbeddingRequestClass, EmbeddingRequestContext,
    SERVER_CONNECTION_HANDLER_CAPACITY, SERVER_CONTROL_CONNECTION_RESERVE, ServerRequestAdmission,
    ServerRequestAdmissionDepths, ServerRequestDeadline, ServerRequestRegistration,
    cancel_if_peer_dead, read_frame, reap_finished_connection_handlers, request,
    serve_embedding_connection, serve_embedding_connection_at_handler_capacity,
};
use super::{
    MemoryStream, TestClock, begin_test_request, encode_test_frame, serve_mismatched_peer_hello,
    test_cancel_token, test_executable, test_hello_operation, test_server_state,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

#[test]
fn request_deadline_covers_pre_engine_work_and_cancels_abandoned_context() {
    let clock = TestClock::new();
    let context = EmbeddingRequestContext::new("deadline", "scope", 0);
    let deadline = ServerRequestDeadline::start(clock.as_ref(), 10);

    clock.sleep(Duration::from_millis(9));
    assert!(!deadline.cancel_if_elapsed(clock.as_ref(), &context));
    assert!(!context.is_cancelled());

    // This elapsed time represents admission plus cold engine
    // initialization before a native request handle exists.
    clock.sleep(Duration::from_millis(1));
    assert!(deadline.cancel_if_elapsed(clock.as_ref(), &context));
    assert!(context.is_cancelled());
}

#[test]
fn idle_admission_closes_before_a_new_request_can_enter() {
    let state = test_server_state();
    assert!(state.begin_draining_if_idle());
    let context = EmbeddingRequestContext::new("late", "scope", 0);
    let admission = state
        .try_admit_request(EmbeddingRequestClass::Query, 0)
        .expect("front admission remains independently bounded");
    assert!(
        state
            .begin_request(ServerRequestRegistration {
                connection_id: "connection",
                request_id: "late",
                scope_id: "scope",
                request_class: EmbeddingRequestClass::Query,
                phase: "queued",
                context,
                admission,
                cancellation_auth: None,
            })
            .is_err()
    );
    assert!(state.engine.lock().expect("engine state").is_none());
}

#[test]
fn dead_authenticated_peer_cancels_queued_context() {
    let (stream, _) = MemoryStream::new(Vec::new(), false);
    let context = EmbeddingRequestContext::new("dead", "scope", 0);
    assert!(cancel_if_peer_dead(&stream, &context).expect("liveness probe"));
    assert!(context.is_cancelled());
}

#[test]
fn observe_intent_rejects_activation_without_initializing_or_resetting_idle() {
    let compatibility = EmbeddingCompatibility::current(true);
    let hello = request(
        "hello",
        compatibility.clone(),
        test_hello_operation("observe"),
    );
    let activate = request(
        "activate",
        compatibility,
        EmbeddingOperation::EnsureResident {
            scope_id: "scope".into(),
            deadline_ms: 100,
            retry_after_ms: 1,
        },
    );
    let mut input = encode_test_frame(&hello, &[]);
    input.extend_from_slice(&encode_test_frame(&activate, &[]));
    let fixture = MemoryStream::with_delivery_tracking(input, true);
    let state = test_server_state();
    let idle_before = state.last_work_ended_ns.load(Ordering::Acquire);
    serve_embedding_connection(Arc::clone(&state), Box::new(fixture.stream))
        .expect("observe rejection is correlated");
    assert_eq!(
        fixture.finished_deliveries.load(Ordering::Acquire),
        1,
        "a correlated final response must finish transport delivery before teardown"
    );
    assert_eq!(
        fixture
            .read_timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .copied()
            .flatten(),
        Some(EmbeddingClientBudgets::current().query_request),
        "final delivery must replace the peer-selected timeout with the server-owned cap"
    );
    assert!(state.engine.lock().expect("engine state").is_none());
    assert_eq!(
        state.last_work_ended_ns.load(Ordering::Acquire),
        idle_before
    );
    let bytes = fixture
        .output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let (mut output_stream, _) = MemoryStream::new(bytes, true);
    let _: (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(&mut output_stream).expect("hello response");
    let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(&mut output_stream).expect("observe rejection");
    assert_eq!(
        response.error.expect("terminal error").code,
        "embedding_server_observe_operation_forbidden"
    );
}

#[test]
fn incompatible_observe_reports_without_draining_or_resetting_idle() {
    let mut compatibility = EmbeddingCompatibility::current(true);
    compatibility.config_sha256 = "incompatible-observer".into();
    let hello = request("hello", compatibility, test_hello_operation("observe"));
    let (stream, output) = MemoryStream::new(encode_test_frame(&hello, &[]), true);
    let state = test_server_state();
    let idle_before = state.last_work_ended_ns.load(Ordering::Acquire);
    let event_before = state.event_sequence.load(Ordering::Acquire);

    serve_embedding_connection(Arc::clone(&state), Box::new(stream))
        .expect("incompatible observation is correlated");

    assert!(!state.draining.load(Ordering::Acquire));
    assert!(state.engine.lock().expect("engine state").is_none());
    assert_eq!(
        state.last_work_ended_ns.load(Ordering::Acquire),
        idle_before
    );
    assert_eq!(state.event_sequence.load(Ordering::Acquire), event_before);
    let bytes = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let (mut output_stream, _) = MemoryStream::new(bytes, true);
    let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(&mut output_stream).expect("incompatible response");
    assert!(response.result.is_none());
    let error = response.error.expect("terminal incompatibility");
    assert_eq!(error.code, "embedding_server_incompatible_active_owner");
}

#[test]
fn same_user_hello_executable_mismatch_uses_typed_upgrade_handshake() {
    let observed = test_server_state();
    let observe_error = serve_mismatched_peer_hello(&observed, "observe");
    assert_eq!(
        observe_error.code,
        "embedding_server_incompatible_active_owner"
    );
    assert!(!observed.draining.load(Ordering::Acquire));

    let active = test_server_state();
    active.active.lock().expect("active state").insert(
        "existing:request".into(),
        ActiveServerRequest {
            request_id: "request".into(),
            scope_id: "scope".into(),
            request_class: EmbeddingRequestClass::Query,
            phase: "native_execution".into(),
            started_ns: active.clock.now_ns(),
        },
    );
    let active_error = serve_mismatched_peer_hello(&active, "activate");
    assert_eq!(
        active_error.code,
        "embedding_server_incompatible_active_owner"
    );
    assert!(!active.draining.load(Ordering::Acquire));

    let idle = test_server_state();
    let idle_error = serve_mismatched_peer_hello(&idle, "activate");
    assert_eq!(idle_error.code, "embedding_server_draining");
    assert!(idle.draining.load(Ordering::Acquire));
}

#[test]
fn cold_initialization_admission_is_bounded_per_class_and_cancel_reclaims_capacity() {
    let state = test_server_state();
    let _cold_initialization = state.engine.lock().expect("hold cold engine state");
    let mut query_guards = Vec::new();
    let mut bulk_guards = Vec::new();

    for index in 0..EMBEDDING_QUERY_QUEUE_CAPACITY {
        let parsed_request = state
            .try_begin_pre_request()
            .expect("bounded request parser slot");
        drop(parsed_request);
        query_guards.push(begin_test_request(
            &state,
            EmbeddingRequestClass::Query,
            &format!("query-{index}"),
        ));
    }
    for index in 0..EMBEDDING_BULK_QUEUE_CAPACITY {
        let parsed_request = state
            .try_begin_pre_request()
            .expect("bounded request parser slot");
        drop(parsed_request);
        bulk_guards.push(begin_test_request(
            &state,
            EmbeddingRequestClass::Bulk,
            &format!("bulk-{index}"),
        ));
    }

    let query_error = state
        .try_admit_request(EmbeddingRequestClass::Query, 17)
        .expect_err("the 65th cold query must receive typed capacity");
    let bulk_error = state
        .try_admit_request(EmbeddingRequestClass::Bulk, 19)
        .expect_err("the 65th cold bulk request must receive typed capacity");
    for (error, class, retry_after_ms) in [(query_error, "query", 17), (bulk_error, "bulk", 19)] {
        assert_eq!(error.code, "embedding_capacity");
        let pressure = error.capacity.expect("typed capacity details");
        assert_eq!(pressure.reason, "queue_full");
        assert_eq!(pressure.queue_class, class);
        assert_eq!(pressure.capacity, 64);
        assert_eq!(pressure.depth, 64);
        assert_eq!(pressure.owner_state, "waking");
        assert_eq!(pressure.retry_after_ms, retry_after_ms);
    }
    assert_eq!(
        state.request_admission.snapshot(),
        ServerRequestAdmissionDepths {
            query: EMBEDDING_QUERY_QUEUE_CAPACITY,
            bulk: EMBEDDING_BULK_QUEUE_CAPACITY,
        }
    );
    assert_eq!(
        state.active.lock().expect("active state").len(),
        EMBEDDING_QUERY_QUEUE_CAPACITY + EMBEDDING_BULK_QUEUE_CAPACITY
    );

    assert!(!state.cancel(
        "query-0",
        "00000000-0000-0000-0000-000000000000",
        test_executable().pid,
        &test_executable().process_start_id,
    ));
    assert!(!state.cancel(
        "query-0",
        &test_cancel_token(),
        test_executable().pid + 1,
        &test_executable().process_start_id,
    ));
    assert!(state.cancel(
        "query-0",
        &test_cancel_token(),
        test_executable().pid,
        &test_executable().process_start_id,
    ));
    assert_eq!(
        state.request_admission.snapshot().query,
        EMBEDDING_QUERY_QUEUE_CAPACITY - 1
    );
    let replacement = state
        .try_admit_request(EmbeddingRequestClass::Query, 23)
        .expect("cancellation immediately reclaims the class permit");
    drop(replacement);
    drop(query_guards.remove(0));
    assert_eq!(
        state.active.lock().expect("active state").len(),
        EMBEDDING_QUERY_QUEUE_CAPACITY + EMBEDDING_BULK_QUEUE_CAPACITY - 1
    );

    drop(query_guards);
    drop(bulk_guards);
    assert_eq!(
        state.request_admission.snapshot(),
        ServerRequestAdmissionDepths::default()
    );
    assert!(state.active.lock().expect("active state").is_empty());
}

#[test]
fn front_admission_reserves_the_documented_queue_behind_one_active_request() {
    let admission = Arc::new(ServerRequestAdmission::default());
    let permits = (0..=EMBEDDING_BULK_QUEUE_CAPACITY)
        .map(|_| {
            admission
                .try_acquire(EmbeddingRequestClass::Bulk, true)
                .expect("one active request plus the full queue remains bounded")
        })
        .collect::<Vec<_>>();
    assert_eq!(admission.snapshot().bulk, EMBEDDING_BULK_QUEUE_CAPACITY + 1);
    assert!(
        admission
            .try_acquire(EmbeddingRequestClass::Bulk, true)
            .is_err(),
        "the request after the active slot and full queue must be rejected"
    );
    drop(permits);
    assert_eq!(
        admission.snapshot(),
        ServerRequestAdmissionDepths::default()
    );
}

#[test]
fn hostile_idle_connections_are_bounded_and_product_rejection_is_correlated() {
    let state = test_server_state();
    let idle_before = state.last_work_ended_ns.load(Ordering::Acquire);
    let mut permits = (0..SERVER_CONNECTION_HANDLER_CAPACITY)
        .map(|_| {
            state
                .try_begin_connection()
                .expect("connection permit within hard bound")
        })
        .collect::<Vec<_>>();
    assert!(state.try_begin_connection().is_none());
    let idle_permits = (0..SERVER_CONTROL_CONNECTION_RESERVE)
        .map(|_| {
            state
                .try_begin_pre_request()
                .expect("idle handshake within the pre-request bound")
        })
        .collect::<Vec<_>>();
    assert!(
        state.try_begin_pre_request().is_none(),
        "at most eight connections may remain between Hello and a classified request"
    );
    assert!(
        state.true_idle(),
        "idle handshakes must not extend the native owner's true-idle lifetime"
    );
    assert_eq!(
        state.last_work_ended_ns.load(Ordering::Acquire),
        idle_before
    );

    let product_hello = request(
        "product-pre-request-capacity",
        EmbeddingCompatibility::current(true),
        test_hello_operation("activate"),
    );
    let (stream, output) = MemoryStream::new(encode_test_frame(&product_hello, &[]), true);
    serve_embedding_connection(Arc::clone(&state), Box::new(stream))
        .expect("pre-request rejection is correlated");
    let bytes = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let (mut output_stream, _) = MemoryStream::new(bytes, true);
    let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(&mut output_stream).expect("typed pre-request rejection");
    let pressure = response
        .error
        .and_then(|error| error.capacity)
        .expect("pre-request pressure");
    assert_eq!(pressure.reason, "pre_request_full");
    assert_eq!(pressure.capacity, SERVER_CONTROL_CONNECTION_RESERVE as u64);

    let rejection_guard = state
        .try_begin_rejection_connection()
        .expect("dedicated rejection reserve remains available");
    let hello = request(
        "product-hello",
        EmbeddingCompatibility::current(true),
        test_hello_operation("activate"),
    );
    let (stream, output) = MemoryStream::new(encode_test_frame(&hello, &[]), true);
    serve_embedding_connection_at_handler_capacity(Arc::clone(&state), Box::new(stream))
        .expect("hard-cap rejection is correlated");
    let bytes = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let (mut output_stream, _) = MemoryStream::new(bytes, true);
    let (response, _): (EmbeddingProtocolResponse, Vec<u8>) =
        read_frame(&mut output_stream).expect("typed product rejection");
    let error = response.error.expect("capacity response");
    assert_eq!(error.code, "embedding_capacity");
    let pressure = error.capacity.expect("connection pressure");
    assert_eq!(pressure.reason, "connection_handler_full");
    assert_eq!(pressure.queue_class, "connection");
    assert_eq!(pressure.capacity, SERVER_CONNECTION_HANDLER_CAPACITY as u64);
    assert!(pressure.depth >= pressure.capacity);

    assert_eq!(
        state.connections.load(Ordering::Acquire),
        SERVER_CONNECTION_HANDLER_CAPACITY + 1
    );
    drop(rejection_guard);
    drop(idle_permits);
    drop(permits.pop());
    let replacement = state
        .try_begin_connection()
        .expect("dropped handler permit is immediately reusable");
    drop(replacement);
    drop(permits);
    assert_eq!(state.connections.load(Ordering::Acquire), 0);
}

#[test]
fn finished_connection_handlers_are_reaped_under_high_churn() {
    let mut retained = Vec::new();
    for _ in 0..256 {
        retained.push(thread::spawn(|| {}));
        while retained
            .last()
            .is_some_and(|connection| !connection.is_finished())
        {
            thread::yield_now();
        }
        reap_finished_connection_handlers(&mut retained);
        assert!(
            retained.is_empty(),
            "completed JoinHandles must not accumulate between accepts"
        );
    }
}
