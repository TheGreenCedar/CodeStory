use super::super::{
    AwakeMonotonicClock, EmbeddingCompatibility, EmbeddingConnectIntent, EmbeddingOperation,
    EmbeddingResult, PerUserEmbeddingClient, PerUserEmbeddingError, RETRIEVAL_EMBEDDING_DIM,
    embedding_capacity_pressure, embedding_retry_state, embedding_scope_id, is_server_loss,
};
use super::{
    BlockingScriptState, BootstrapConnectOutcome, BootstrapTestTransport, ClientTestTransport,
    ControlledCancelTestTransport, DeadlineBudgetTransport, ExplicitDeadlineTransport,
    ScriptOutcome, ScriptStream, test_cancel_token, test_client,
};
use crate::config::SidecarRuntimeConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
fn windows_pipe_closing_write_loss_is_classified_and_replayed_exactly_once() {
    let transport = BootstrapTestTransport::new(
        [
            BootstrapConnectOutcome::WriteDisconnect,
            BootstrapConnectOutcome::NoOwner,
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

    let (vector, attempts) = client
        .embed_query_with_qualification_attempts("x")
        .expect("a classified pipe-closing write loss earns exactly one replay");

    assert_eq!(vector.len(), RETRIEVAL_EMBEDDING_DIM);
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, "server_loss");
    assert_eq!(
        attempts[0].loss_code.as_deref(),
        Some("embedding_server_connection_lost"),
        "the recorded loss must be the classified transport disconnect, \
         not an unresponsive-owner timeout"
    );
    assert_eq!(attempts[1].outcome, "completed");
    assert_eq!(attempts[1].loss_code, None);
    assert_ne!(attempts[0].request_id, attempts[1].request_id);
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 3);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
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
fn pure_rpc_replay_rejects_the_same_dying_server_before_replacement() {
    let transport = BootstrapTestTransport::new(
        [
            BootstrapConnectOutcome::Loss,
            BootstrapConnectOutcome::Connected,
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
        .expect("the replay waits for a distinct server instance");

    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, "server_loss");
    assert_eq!(attempts[0].server_instance_id, "server-1");
    assert_eq!(attempts[1].outcome, "completed");
    assert_eq!(attempts[1].server_instance_id, "server-2");
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 5);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
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
    {
        let state = transport
            .blocking_state
            .lock()
            .expect("scripted blocking state");
        assert!(
            state.active_request_id.is_some() && state.active_cancel_token.is_some(),
            "the scripted server must have recorded the admitted request's credentials"
        );
        assert_eq!(
            state.rejected_cancels, 0,
            "the client-sent cancel must authenticate against the recorded request"
        );
    }
    assert!(
        transport.connect_count.load(Ordering::Acquire) >= 2,
        "the watcher must use a separate authenticated control connection"
    );

    // Credential discrimination on the scripted server itself: a cancel naming
    // the wrong request, or carrying a well-formed foreign token, is Released
    // rather than Cancelled — matching server/connection.rs — and a malformed
    // token fails closed as embedding_server_cancel_token_invalid.
    let scripted_cancelled = Arc::new(AtomicBool::new(false));
    let scripted_state = Arc::new(Mutex::new(BlockingScriptState::default()));
    let mut scripted = ScriptStream::new(
        ScriptOutcome::Blocking {
            request_started: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::clone(&scripted_cancelled),
            state: Arc::clone(&scripted_state),
        },
        EmbeddingCompatibility::current(true),
    );
    let compatibility = EmbeddingCompatibility::current(true);
    scripted
        .response_for_request(super::super::exchange::request(
            "request-active",
            compatibility.clone(),
            EmbeddingOperation::EmbedQuery {
                scope_id: "test-scope".into(),
                deadline_ms: 1000,
                retry_after_ms: 0,
                cancel_token: Some(test_cancel_token()),
                input: "x".into(),
            },
        ))
        .expect("admit the scripted request");

    let wrong_token = "00000000-0000-4000-8000-000000000001";
    for (target_request_id, cancel_token, label) in [
        ("request-active", wrong_token, "foreign token"),
        (
            "request-other",
            test_cancel_token().as_str(),
            "wrong target request",
        ),
        (
            "request-active",
            test_cancel_token().as_str(),
            "correct credentials",
        ),
    ] {
        let (response, _) = scripted
            .response_for_request(super::super::exchange::request(
                &format!("cancel-{label}"),
                compatibility.clone(),
                EmbeddingOperation::Cancel {
                    target_request_id: target_request_id.into(),
                    cancel_token: cancel_token.into(),
                },
            ))
            .expect("scripted cancel response")
            .expect("a cancel always earns a response");
        let expected_cancelled = label == "correct credentials";
        match &response.result {
            Some(EmbeddingResult::Cancelled) => {
                assert!(expected_cancelled, "{label} cancelled without credentials")
            }
            Some(EmbeddingResult::Released) => {
                assert!(!expected_cancelled, "{label} released valid credentials")
            }
            other => panic!("{label}: unexpected cancel result {other:?}"),
        }
        assert_eq!(
            scripted_cancelled.load(Ordering::Acquire),
            expected_cancelled,
            "{label} must {}the scripted request",
            if expected_cancelled {
                "end "
            } else {
                "not end "
            }
        );
    }
    assert_eq!(scripted_state.lock().expect("state").rejected_cancels, 2);

    let (response, _) = scripted
        .response_for_request(super::super::exchange::request(
            "cancel-malformed",
            compatibility,
            EmbeddingOperation::Cancel {
                target_request_id: "request-active".into(),
                cancel_token: "not-a-token".into(),
            },
        ))
        .expect("scripted cancel response")
        .expect("a cancel always earns a response");
    assert_eq!(
        response.error.map(|error| error.code).as_deref(),
        Some("embedding_server_cancel_token_invalid")
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
fn transient_handshake_capacity_retries_within_one_connect_budget() {
    let transport = BootstrapTestTransport::with_connect_budget(
        [
            BootstrapConnectOutcome::HelloCapacity,
            BootstrapConnectOutcome::HelloHandlerCapacity,
            BootstrapConnectOutcome::Connected,
        ],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    let (_, attempts) = client
        .embed_query_with_qualification_attempts("x")
        .expect("both bounded connection-capacity reasons recover without an RPC replay");

    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].outcome, "completed");
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 3);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
}

#[test]
fn persistent_handshake_capacity_remains_typed_and_bounded() {
    let transport = BootstrapTestTransport::with_connect_budget(
        [],
        BootstrapConnectOutcome::HelloCapacity,
        Duration::from_millis(3),
        Duration::from_millis(10),
    );
    let client = PerUserEmbeddingClient {
        transport: transport.clone(),
        compatibility: EmbeddingCompatibility::current(true),
        scope_id: "test-scope".into(),
    };

    let error = client
        .embed_query("x")
        .expect_err("connection admission cannot retry beyond the connect budget");
    let pressure = embedding_capacity_pressure(&error).expect("typed connection pressure");

    assert_eq!(pressure.reason, "pre_request_full");
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 3);
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

#[test]
fn idle_drain_preserves_compatible_connect_and_hello_budgets() {
    let transport = BootstrapTestTransport::with_connect_budget(
        [BootstrapConnectOutcome::SlowCompatibleHello],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(5),
        Duration::from_millis(20),
    );
    let client = test_client(transport.clone());
    let started = transport.clock.now_ns();
    client
        .connect(EmbeddingConnectIntent::Activate, true)
        .expect("compatible owner retains separate existing transport/Hello bounds");
    assert_eq!(
        Duration::from_nanos(transport.clock.now_ns() - started),
        Duration::from_millis(6),
        "4ms connect plus2ms Hello each fit the existing5ms per-stage bound"
    );
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
}

#[test]
fn idle_drain_activation_converges_to_one_authenticated_replacement_rpc() {
    let transport = BootstrapTestTransport::with_connect_budget(
        [
            BootstrapConnectOutcome::HelloIdleDraining,
            BootstrapConnectOutcome::NoOwner,
            BootstrapConnectOutcome::OwnerUnresponsive,
            BootstrapConnectOutcome::Connected,
        ],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(10),
        Duration::from_millis(20),
    );
    let client = test_client(transport.clone());
    let (vector, attempts) = client
        .embed_query_with_qualification_attempts("x")
        .expect("one activation must converge after the idle owner commits to drain");
    assert_eq!(vector.len(), RETRIEVAL_EMBEDDING_DIM);
    assert_eq!(vector[0], 1.0);
    assert_eq!(
        attempts.len(),
        1,
        "Hello convergence must not replay an embedding RPC"
    );
    assert_eq!(attempts[0].outcome, "completed");
    assert_eq!(attempts[0].server_instance_id, "server-2");
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 4);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
    let observations = transport.connect_observations.lock().unwrap();
    assert_eq!(
        observations
            .iter()
            .map(|(_, _, spawned)| *spawned)
            .collect::<Vec<_>>(),
        [false, false, true, true],
        "spawn authority is obtained only after NoOwner"
    );
}

#[test]
fn idle_drain_and_post_spawn_wait_share_one_absolute_convergence_budget() {
    let budget = Duration::from_millis(5);
    for (outcomes, fallback, expected_spawns) in [
        (
            vec![BootstrapConnectOutcome::HelloIdleDrainingAfterConnectDelay],
            BootstrapConnectOutcome::HelloIdleDraining,
            0,
        ),
        (
            vec![
                BootstrapConnectOutcome::HelloIdleDrainingAfterConnectDelay,
                BootstrapConnectOutcome::HelloIdleDraining,
                BootstrapConnectOutcome::NoOwner,
            ],
            BootstrapConnectOutcome::OwnerUnresponsive,
            1,
        ),
    ] {
        let transport = BootstrapTestTransport::with_connect_budget(
            outcomes,
            fallback,
            budget,
            Duration::from_millis(20),
        );
        let client = test_client(transport.clone());
        let started = transport.clock.now_ns();
        let error = match client.connect(EmbeddingConnectIntent::Activate, true) {
            Ok(_) => panic!("persistent drain/startup must be bounded"),
            Err(error) => error,
        };
        assert!(
            embedding_retry_state(&error).is_some(),
            "bounded refusal remains typed: {error:#}"
        );
        let elapsed = Duration::from_nanos(transport.clock.now_ns() - started);
        assert!(
            elapsed >= Duration::from_millis(3),
            "must wait beyond the first2ms connect/Hello cost, rather than immediately returning: {error:#}"
        );
        assert!(
            elapsed <= budget,
            "NoOwner/spawn cannot reset the original drain deadline: {elapsed:?}"
        );
        assert_eq!(
            transport.spawn_count.load(Ordering::Acquire),
            expected_spawns
        );
        assert!(
            transport.connect_count.load(Ordering::Acquire) >= 2,
            "committed drain must actually reconnect before its budget expires"
        );
        assert!(
            transport.connect_count.load(Ordering::Acquire) <= 8,
            "no hot reconnect loop"
        );
        for (connect_budget, at_ns, _) in transport
            .connect_observations
            .lock()
            .unwrap()
            .iter()
            .skip(1)
        {
            let remaining = budget.saturating_sub(Duration::from_nanos(*at_ns - started));
            assert!(
                !connect_budget.is_zero() && *connect_budget <= remaining,
                "each reconnect/Hello shares the original remaining convergence budget: requested={connect_budget:?}, remaining={remaining:?}"
            );
        }
    }
}

#[test]
fn idle_drain_wait_honors_caller_cancellation_before_reconnect_or_spawn() {
    let transport = BootstrapTestTransport::new(
        [
            BootstrapConnectOutcome::HelloIdleDraining,
            BootstrapConnectOutcome::NoOwner,
        ],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(10),
    );
    let client = test_client(transport.clone());
    let error = client
        .embed_query_with_control("x", None, &|| {
            transport.connect_count.load(Ordering::Acquire) != 0
        })
        .expect_err("caller cancellation during drain must stop bootstrap");
    assert_eq!(
        embedding_retry_state(&error)
            .expect("typed cancellation")
            .code,
        "embedding_cancelled"
    );
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
}

#[test]
fn idle_drain_reconnect_hello_keeps_the_original_caller_deadline() {
    let transport = ExplicitDeadlineTransport::after_idle_drain();
    let client = test_client(transport.clone());
    let started = Instant::now();
    let error = client
        .embed_query_with_control("x", Some(Duration::from_millis(50)), &|| false)
        .expect_err("the pending replacement Hello is bounded by the original caller deadline");
    assert_eq!(
        embedding_retry_state(&error).expect("typed deadline").code,
        "embedding_deadline_exceeded"
    );
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "drain cannot reset the500ms local budget"
    );
    assert_eq!(transport.connect_count.load(Ordering::Acquire), 2);
    let timeout = transport
        .observed_read_timeout
        .lock()
        .unwrap()
        .expect("replacement Hello has a bounded read timeout");
    assert!(timeout <= Duration::from_millis(50));
    assert!(
        transport
            .observed_connect_budget
            .lock()
            .unwrap()
            .expect("reconnect budget")
            <= Duration::from_millis(50)
    );
}

#[test]
fn idle_drain_convergence_requires_activating_spawn_authority_and_exact_retry_state() {
    for (intent, may_spawn, outcome, expected_code) in [
        (
            EmbeddingConnectIntent::Observe,
            false,
            BootstrapConnectOutcome::HelloIdleDraining,
            "embedding_server_draining",
        ),
        (
            EmbeddingConnectIntent::Observe,
            true,
            BootstrapConnectOutcome::HelloIdleDraining,
            "embedding_server_draining",
        ),
        (
            EmbeddingConnectIntent::Activate,
            false,
            BootstrapConnectOutcome::HelloIdleDraining,
            "embedding_server_draining",
        ),
        (
            EmbeddingConnectIntent::Activate,
            true,
            BootstrapConnectOutcome::HelloActiveOwner,
            "embedding_server_incompatible_active_owner",
        ),
        (
            EmbeddingConnectIntent::Activate,
            true,
            BootstrapConnectOutcome::HelloDrainingAfterDelay,
            "embedding_server_draining",
        ),
        (
            EmbeddingConnectIntent::Activate,
            true,
            BootstrapConnectOutcome::HelloIdleDrainingUnverifiedPeer,
            "embedding_server_draining",
        ),
    ] {
        let transport = BootstrapTestTransport::new(
            [outcome, BootstrapConnectOutcome::NoOwner],
            BootstrapConnectOutcome::Connected,
            Duration::from_millis(5),
        );
        let client = test_client(transport.clone());
        let started = transport.clock.now_ns();
        let error = match client.connect(intent, may_spawn) {
            Ok(_) => panic!("observe/non-spawning/other owner failures must not converge"),
            Err(error) => error,
        };
        assert_eq!(
            embedding_retry_state(&error)
                .expect("typed owner refusal")
                .code,
            expected_code
        );
        assert_eq!(transport.connect_count.load(Ordering::Acquire), 1);
        assert_eq!(transport.spawn_count.load(Ordering::Acquire), 0);
        assert_eq!(
            transport.clock.now_ns(),
            started,
            "passive/active-owner failures must not wait"
        );
    }
}

#[test]
fn idle_drain_replacement_still_requires_authenticated_native_identity() {
    let transport = BootstrapTestTransport::with_connect_budget(
        [
            BootstrapConnectOutcome::HelloIdleDraining,
            BootstrapConnectOutcome::NoOwner,
            BootstrapConnectOutcome::HelloWrongIdentity,
            BootstrapConnectOutcome::Connected,
        ],
        BootstrapConnectOutcome::Connected,
        Duration::from_millis(10),
        Duration::from_millis(20),
    );
    let client = test_client(transport.clone());
    let error = match client.connect(EmbeddingConnectIntent::Activate, true) {
        Ok(_) => panic!("an unverified replacement peer cannot be admitted"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("embedding_server_peer_identity_mismatch"),
        "unexpected identity error: {error:#}"
    );
    assert_eq!(
        transport.connect_count.load(Ordering::Acquire),
        3,
        "hard identity refusal must not reconnect to a later good peer"
    );
    assert_eq!(transport.spawn_count.load(Ordering::Acquire), 1);
}
