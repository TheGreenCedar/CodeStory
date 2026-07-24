use super::super::{
    EmbeddingCompatibility, EmbeddingConnectIntent, PerUserEmbeddingClient, PerUserEmbeddingError,
    RETRIEVAL_EMBEDDING_DIM, embedding_capacity_pressure, embedding_retry_state,
    embedding_scope_id, is_server_loss,
};
use super::{
    BootstrapConnectOutcome, BootstrapTestTransport, ClientTestTransport,
    ControlledCancelTestTransport, DeadlineBudgetTransport, ExplicitDeadlineTransport, test_client,
};
use crate::config::SidecarRuntimeConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn project_runtime_identity_selects_a_distinct_embedding_scope() {
    let first_root = tempfile::tempdir().expect("first project root");
    let second_root = tempfile::tempdir().expect("second project root");
    let first = SidecarRuntimeConfig::for_project_profile(
        Some(first_root.path()),
        crate::config::SidecarProfile::Local,
    );
    let second = SidecarRuntimeConfig::for_project_profile(
        Some(second_root.path()),
        crate::config::SidecarProfile::Local,
    );

    assert_ne!(first.project_identity, second.project_identity);
    assert_ne!(embedding_scope_id(&first), embedding_scope_id(&second));
    assert_eq!(
        embedding_scope_id(&first),
        embedding_scope_id(&first.clone())
    );
}

#[test]
fn pure_rpc_replays_once_and_only_once_on_typed_loss() {
    let transport = ClientTestTransport::new(1, false);
    let client = test_client(transport.clone());
    let (vector, attempts) = client
        .embed_query_with_qualification_attempts("x")
        .expect("one replay succeeds");
    assert_eq!(vector.len(), RETRIEVAL_EMBEDDING_DIM);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 2);
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].ordinal, 1);
    assert_eq!(attempts[0].outcome, "server_loss");
    assert_eq!(attempts[1].ordinal, 2);
    assert_eq!(attempts[1].outcome, "completed");
    assert_ne!(attempts[0].request_id, attempts[1].request_id);

    let transport = ClientTestTransport::new(usize::MAX, false);
    let client = test_client(transport.clone());
    let error = client
        .embed_query("x")
        .expect_err("second loss is terminal");
    assert!(is_server_loss(&error));
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 2);
}

#[test]
fn pure_rpc_replay_waits_for_a_fail_stopped_owner_to_release_authority() {
    let transport = BootstrapTestTransport::new(
        [
            BootstrapConnectOutcome::Loss,
            BootstrapConnectOutcome::OwnerUnresponsive,
            BootstrapConnectOutcome::NoOwner,
            BootstrapConnectOutcome::OwnerUnresponsive,
            BootstrapConnectOutcome::Connected,
        ],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(5),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    let (_, attempts) = client
        .embed_query_with_qualification_attempts("x")
        .expect("one replay converges after the fail-stopped owner releases authority");

    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, "server_loss");
    assert_eq!(attempts[1].outcome, "completed");
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 5);
}

#[test]
fn pure_rpc_replay_converges_after_recovery_hello_loss() {
    let transport = BootstrapTestTransport::new(
        [
            BootstrapConnectOutcome::Loss,
            BootstrapConnectOutcome::HelloLoss,
            BootstrapConnectOutcome::OwnerUnresponsive,
            BootstrapConnectOutcome::NoOwner,
            BootstrapConnectOutcome::OwnerUnresponsive,
            BootstrapConnectOutcome::Connected,
        ],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(6),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    let (_, attempts) = client
        .embed_query_with_qualification_attempts("x")
        .expect("recovery hello loss converges before the replay RPC is sent");

    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, "server_loss");
    assert_eq!(attempts[1].outcome, "completed");
    assert_ne!(attempts[0].request_id, attempts[1].request_id);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 6);
}

#[test]
fn bulk_replay_budget_preserves_the_full_replay_window_after_initial_bootstrap() {
    // The frozen bulk deadline is the sum of the stalled-native window,
    // replacement convergence, and replay-success budget. Keep those
    // phases comfortably inside the 400 ms test deadline, then add 200 ms
    // of initial Hello work that the frozen formula does not account for.
    // The old accounting takes about 475 ms and the repaired accounting
    // about 275 ms, leaving wide real-time margins around the same 400 ms
    // deadline despite ordinary scheduler jitter.
    let transport = DeadlineBudgetTransport::new();
    let client = test_client(transport.clone());
    let result = client.embed_documents_with_qualification_attempts(&["x".into()]);
    let observed = result
        .as_ref()
        .err()
        .and_then(embedding_retry_state)
        .map(|retry| retry.code);

    assert!(
        result.is_ok(),
        "a contract-sized recovery must retain its full replay window; observed_code={observed:?}, result={result:?}"
    );
    let (_, attempts) = result.expect("successful replay");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, "server_loss");
    assert_eq!(attempts[1].outcome, "completed");
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
}

#[test]
fn explicit_caller_deadline_bounds_initial_hello() {
    let transport = ExplicitDeadlineTransport::new();
    let client = test_client(transport.clone());
    let started = Instant::now();
    let error = client
        .embed_query_with_control("x", Some(Duration::from_millis(50)), &|| false)
        .expect_err("the explicit caller deadline must bound initial Hello");
    let elapsed = started.elapsed();
    let retry = embedding_retry_state(&error).expect("typed caller deadline");
    let connect_budget = transport
        .observed_connect_budget
        .lock()
        .expect("observed connect budget")
        .expect("connect budget");
    let read_timeout = transport
        .observed_read_timeout
        .lock()
        .expect("observed Hello read timeout")
        .expect("Hello read timeout");

    assert_eq!(retry.code, "embedding_deadline_exceeded");
    assert!(connect_budget <= Duration::from_millis(50));
    assert!(read_timeout <= Duration::from_millis(50));
    assert!(
        elapsed < Duration::from_millis(250),
        "explicit deadline took {elapsed:?}, approaching the 500 ms connect budget"
    );
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
}

#[test]
fn pure_rpc_does_not_wait_for_a_preexisting_frozen_owner() {
    let transport = BootstrapTestTransport::new(
        [BootstrapConnectOutcome::OwnerUnresponsive],
        BootstrapConnectOutcome::OwnerUnresponsive,
        Duration::from_millis(5),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    let error = client
        .embed_query("x")
        .expect_err("a pre-existing frozen owner remains a typed failure");
    let typed = error
        .downcast_ref::<PerUserEmbeddingError>()
        .expect("typed frozen-owner state");

    assert_eq!(typed.code, "embedding_server_owner_unresponsive");
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 2);
}

#[test]
fn caller_cancellation_interrupts_active_rpc_over_authenticated_control_connection() {
    let transport = ControlledCancelTestTransport::new();
    let client = test_client(transport.clone());
    let caller_cancelled = AtomicBool::new(false);

    let error = thread::scope(|scope| {
        let request = scope.spawn(|| {
            client.embed_query_with_control("x", Some(Duration::from_secs(1)), &|| {
                caller_cancelled.load(Ordering::Acquire)
            })
        });
        while !transport.request_started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        caller_cancelled.store(true, Ordering::Release);
        request
            .join()
            .expect("controlled request thread")
            .expect_err("caller cancellation must win")
    });

    let retry = embedding_retry_state(&error).expect("typed cancellation");
    assert_eq!(retry.code, "embedding_cancelled");
    assert_eq!(retry.retry_class, "none");
    assert!(transport.server_cancelled.load(Ordering::Acquire));
    assert!(
        transport.connect_count.load(Ordering::Acquire) >= 2,
        "the watcher must use a separate authenticated control connection"
    );
}

#[test]
fn cancellation_wins_before_connection_loss_can_replay() {
    let transport = ClientTestTransport::new(usize::MAX, false);
    let client = test_client(transport.clone());
    let error = client
        .embed_query_with_control("x", Some(Duration::from_secs(1)), &|| {
            transport.connect_count.load(Ordering::Acquire) > 0
        })
        .expect_err("cancellation after connect must suppress pure replay");

    assert_eq!(
        embedding_retry_state(&error)
            .expect("typed cancellation")
            .code,
        "embedding_cancelled"
    );
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
}

#[test]
fn typed_capacity_does_not_spawn_or_replay() {
    let transport = ClientTestTransport::new(0, true);
    let client = test_client(transport.clone());
    let error = client.embed_query("x").expect_err("capacity is surfaced");
    let pressure = embedding_capacity_pressure(&error).expect("typed pressure");
    assert_eq!(pressure.reason, "queue_full");
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
}

#[test]
fn post_spawn_authority_without_endpoint_converges_within_spawn_budget() {
    let transport = BootstrapTestTransport::new(
        [
            BootstrapConnectOutcome::NoOwner,
            BootstrapConnectOutcome::OwnerUnresponsive,
            BootstrapConnectOutcome::Connected,
        ],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(5),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    client
        .connect(EmbeddingConnectIntent::Activate, true)
        .expect("a spawned owner may hold authority before publishing its endpoint");

    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 3);
}

#[test]
fn preexisting_frozen_authority_remains_typed_and_does_not_spawn() {
    let transport = BootstrapTestTransport::new(
        [BootstrapConnectOutcome::OwnerUnresponsive],
        BootstrapConnectOutcome::OwnerUnresponsive,
        Duration::from_millis(5),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    let error = match client.connect(EmbeddingConnectIntent::Activate, true) {
        Ok(_) => panic!("an owner present before spawn is terminal"),
        Err(error) => error,
    };
    let typed = error
        .downcast_ref::<PerUserEmbeddingError>()
        .expect("typed owner state");

    assert_eq!(typed.code, "embedding_server_owner_unresponsive");
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
}

#[test]
fn post_spawn_owner_convergence_is_hard_bounded() {
    let transport = BootstrapTestTransport::new(
        [BootstrapConnectOutcome::NoOwner],
        BootstrapConnectOutcome::OwnerUnresponsive,
        Duration::from_millis(2),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    let error = match client.connect(EmbeddingConnectIntent::Activate, true) {
        Ok(_) => panic!("a spawned owner must publish within the convergence budget"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("embedding_server_start_timeout"));
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 4);
}
