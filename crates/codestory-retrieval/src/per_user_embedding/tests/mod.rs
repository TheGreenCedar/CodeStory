mod client_replay;
mod client_transports;
mod identities;
mod protocol_transport;
mod qualification;
mod server_admission;
mod server_fixtures;
mod transport_fixtures;
mod watchdog;

use client_transports::{
    BootstrapConnectOutcome, BootstrapTestTransport, ClientTestTransport,
    ControlledCancelTestTransport, DeadlineBudgetTransport, ExplicitDeadlineTransport,
};
use identities::{
    begin_test_request, encode_test_frame, serve_mismatched_peer_hello, test_cancel_token,
    test_client, test_engine_identity, test_executable, test_hello_operation,
    test_qualification_control, test_qualification_event, test_server_state, test_snapshot,
    test_transport_identity,
};
use server_fixtures::{PollingStream, WatchdogTransport};
use transport_fixtures::{MemoryStream, TestClock};

mod lazy_transport {
    use super::super::client::LazyClientTransport;
    use super::client_transports::ClientTestTransport;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn transport() -> Arc<ClientTestTransport> {
        ClientTestTransport::new(0, false)
    }

    #[test]
    fn a_registered_factory_is_not_run_until_a_transport_is_needed() {
        // Capturing a transport hashes the whole executable, which in a release build carries the
        // embedded model. Registering must stay free so commands that never embed do not pay it.
        let builds = Arc::new(AtomicUsize::new(0));
        let lazy = LazyClientTransport::new();
        let counter = Arc::clone(&builds);
        lazy.install_factory(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(transport() as Arc<dyn super::super::EmbeddingClientTransport>)
        }))
        .expect("register factory");
        assert_eq!(
            builds.load(Ordering::SeqCst),
            0,
            "registering built nothing"
        );

        lazy.resolve().expect("first resolve builds");
        assert_eq!(builds.load(Ordering::SeqCst), 1);

        // Later callers share the captured transport rather than re-hashing.
        lazy.resolve().expect("second resolve reuses");
        lazy.resolve().expect("third resolve reuses");
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_capture_is_not_cached_for_the_life_of_the_process() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let lazy = LazyClientTransport::new();
        let counter = Arc::clone(&attempts);
        lazy.install_factory(Box::new(move || {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                anyhow::bail!("transient capture failure");
            }
            Ok(transport() as Arc<dyn super::super::EmbeddingClientTransport>)
        }))
        .expect("register factory");

        assert!(lazy.resolve().is_err(), "first capture fails");
        lazy.resolve().expect("a later caller retries the capture");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn resolving_without_a_registration_names_the_missing_transport() {
        let lazy = LazyClientTransport::new();
        let error = match lazy.resolve() {
            Ok(_) => panic!("nothing was registered, so resolve must fail"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("embedding_server_transport_unavailable"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn concurrent_first_callers_capture_one_transport() {
        let builds = Arc::new(AtomicUsize::new(0));
        let lazy = Arc::new(LazyClientTransport::new());
        let counter = Arc::clone(&builds);
        lazy.install_factory(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(transport() as Arc<dyn super::super::EmbeddingClientTransport>)
        }))
        .expect("register factory");

        let start = Arc::new(Barrier::new(9));
        let callers = (0..8)
            .map(|_| {
                let lazy = Arc::clone(&lazy);
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    lazy.resolve().expect("resolve shared transport")
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        for caller in callers {
            caller.join().expect("join first caller");
        }

        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn direct_and_factory_installation_are_mutually_exclusive() {
        let direct = LazyClientTransport::new();
        direct
            .install(transport())
            .expect("install direct transport");
        assert!(
            direct
                .install_factory(Box::new(|| {
                    Ok(transport() as Arc<dyn super::super::EmbeddingClientTransport>)
                }))
                .is_err()
        );

        let deferred = LazyClientTransport::new();
        deferred
            .install_factory(Box::new(|| {
                Ok(transport() as Arc<dyn super::super::EmbeddingClientTransport>)
            }))
            .expect("install factory");
        assert!(deferred.install(transport()).is_err());
    }
}
