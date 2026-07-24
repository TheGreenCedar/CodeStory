use super::super::{
    ActiveServerRequest, AwakeMonotonicClock, EmbeddingQualificationWatchdogMarker,
    EmbeddingRequestClass, EmbeddingServerBudgets, WatchdogClassProgress,
    embedding_qualification_watchdog_marker_filename, publish_watchdog_fail_stop_marker,
    spawn_server_watchdog,
};
use super::{
    TestClock, WatchdogTransport, test_qualification_control, test_server_state, test_snapshot,
};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

#[test]
fn watchdog_progress_isolated_by_request_class() {
    let clock = TestClock::new();
    let timeout = Duration::from_millis(5);
    let mut query = WatchdogClassProgress::new(clock.now_ns());
    let mut bulk = WatchdogClassProgress::new(clock.now_ns());

    clock.sleep(Duration::from_millis(3));
    assert!(query.observe(true, 1, clock.as_ref(), timeout).is_none());
    assert!(bulk.observe(true, 0, clock.as_ref(), timeout).is_none());
    clock.sleep(Duration::from_millis(6));
    assert!(query.observe(true, 2, clock.as_ref(), timeout).is_none());
    let stalled = bulk
        .observe(true, 0, clock.as_ref(), timeout)
        .expect("query progress must not mask a stalled bulk class");

    assert_eq!(stalled.sequence, 0);
    assert_eq!(stalled.last_progress_ns, 3_000_001);
}

#[test]
fn watchdog_class_activation_starts_a_fresh_deadline() {
    let clock = TestClock::new();
    let timeout = Duration::from_millis(5);
    let mut progress = WatchdogClassProgress::new(clock.now_ns());

    clock.sleep(Duration::from_millis(20));
    assert!(
        progress
            .observe(false, 0, clock.as_ref(), timeout)
            .is_none()
    );
    clock.sleep(Duration::from_millis(20));
    assert!(
        progress.observe(true, 0, clock.as_ref(), timeout).is_none(),
        "inactive time must not be charged to a newly active class"
    );
    clock.sleep(timeout);
    assert!(progress.observe(true, 0, clock.as_ref(), timeout).is_some());
}

#[test]
fn inactive_watchdog_class_never_trips() {
    let clock = TestClock::new();
    let timeout = Duration::from_millis(1);
    let mut progress = WatchdogClassProgress::new(clock.now_ns());

    for sequence in 0..4 {
        clock.sleep(Duration::from_millis(10));
        assert!(
            progress
                .observe(false, sequence, clock.as_ref(), timeout)
                .is_none()
        );
    }
}

#[cfg(unix)]
#[test]
fn watchdog_marker_is_private_durable_and_never_reuses_stale_evidence() {
    use std::os::unix::fs::MetadataExt;

    let (temporary, control) = test_qualification_control();
    let marker_path = control.directory.join(
        embedding_qualification_watchdog_marker_filename(
            &control.nonce_sha256,
            &test_snapshot().process.server_instance_id,
        )
        .expect("marker filename"),
    );
    let state = test_server_state();
    publish_watchdog_fail_stop_marker(
        &control,
        &state,
        EmbeddingServerBudgets {
            idle_timeout: Duration::from_secs(60),
            native_no_progress: Duration::from_millis(4),
            watchdog_poll: Duration::from_millis(1),
        },
        7,
        1,
    )
    .expect("publish marker");
    let metadata = fs::symlink_metadata(&marker_path).expect("marker metadata");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.mode() & 0o077, 0);
    let marker: EmbeddingQualificationWatchdogMarker =
        serde_json::from_slice(&fs::read(&marker_path).expect("read marker"))
            .expect("parse marker");
    assert_eq!(marker.reason, "embedding_engine_stalled");
    assert_eq!(marker.nonce_sha256, control.nonce_sha256);
    assert_eq!(marker.progress_sequence, 7);
    assert!(
        publish_watchdog_fail_stop_marker(
            &control,
            &state,
            EmbeddingServerBudgets {
                idle_timeout: Duration::from_secs(60),
                native_no_progress: Duration::from_millis(4),
                watchdog_poll: Duration::from_millis(1),
            },
            8,
            1,
        )
        .expect_err("stale marker is rejected")
        .to_string()
        .contains("embedding_qualification_watchdog_marker_exists")
    );
    drop(temporary);
}

#[test]
fn shutdown_with_stuck_initialization_keeps_watchdog_fail_stop_armed() {
    let state = test_server_state();
    state.active.lock().expect("active state").insert(
        "connection:request".into(),
        ActiveServerRequest {
            request_id: "request".into(),
            scope_id: "scope".into(),
            request_class: EmbeddingRequestClass::Bulk,
            phase: "native_execution".into(),
            started_ns: state.clock.now_ns(),
        },
    );
    state.draining.store(true, Ordering::Release);
    let transport = Arc::new(WatchdogTransport {
        clock: TestClock::new(),
        fail_stops: AtomicUsize::new(0),
    });
    let _engine_lock = state.engine.lock().expect("simulate stuck initializer");
    let watchdog = spawn_server_watchdog(
        Arc::clone(&state),
        transport.clone(),
        EmbeddingServerBudgets {
            idle_timeout: Duration::from_secs(60),
            native_no_progress: Duration::from_millis(2),
            watchdog_poll: Duration::from_millis(1),
        },
    )
    .expect("watchdog");
    watchdog.join().expect("watchdog completion");
    assert_eq!(transport.fail_stops.load(Ordering::Acquire), 1);
    assert!(state.stopped.load(Ordering::Acquire));
}

#[test]
fn background_engine_cleanup_marks_normal_shutdown_complete() {
    let state = test_server_state();
    state.draining.store(true, Ordering::Release);
    let state_for_cleanup = Arc::clone(&state);
    let cleanup = thread::spawn(move || {
        state_for_cleanup.shutdown_engine();
        state_for_cleanup.stopped.store(true, Ordering::Release);
    });

    cleanup.join().expect("cleanup completion");

    assert!(state.stopped.load(Ordering::Acquire));
}
