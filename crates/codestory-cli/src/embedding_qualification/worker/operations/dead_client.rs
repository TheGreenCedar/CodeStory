use super::super::protocol::run_raw_protocol_exchange_with_transport;
use super::ANTI_IDLE_PROTOCOL_DEADLINE_MS;
use anyhow::{Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingClientTransport, EmbeddingQualificationParameters,
    PerUserEmbeddingClient, SidecarRuntimeConfig,
};
use std::sync::Arc;
use std::time::Duration;

const CLIENT_DEATH_LEASE_HOLD_MS: u64 = 600_000;

pub(in crate::embedding_qualification::worker) fn run_dead_client_load(
    runtime: &SidecarRuntimeConfig,
    parameters: EmbeddingQualificationParameters,
    clock: &dyn AwakeMonotonicClock,
) -> Result<()> {
    if parameters.query_count == 0
        || parameters.bulk_count == 0
        || parameters.documents_per_bulk == 0
        || parameters.hold_ms != CLIENT_DEATH_LEASE_HOLD_MS
    {
        bail!("embedding_qualification_dead_client_parameters_invalid");
    }
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let _lease = client.acquire_residency_lease()?;
    let input = "q".repeat(parameters.input_bytes.max(1) as usize);
    let documents = (0..parameters.documents_per_bulk)
        .map(|index| format!("{index}:{input}"))
        .collect::<Vec<_>>();
    let bulk_input = documents.join("\n");
    let request_runtime = runtime.clone();
    let workers = spawn_dead_client_workers(
        parameters.query_count,
        parameters.bulk_count,
        input,
        bulk_input,
        crate::embedding_server_transport::NativeEmbeddingClientTransport::capture,
        move |transport, class, input| {
            // Keep an admitted request alive until this process is terminated.
            // Product deadlines would add cancellation retries to the pressure
            // this worker is intended to measure.
            let clock = EmbeddingClientTransport::clock(transport);
            let _ = run_raw_protocol_exchange_with_transport(
                &request_runtime,
                transport,
                clock.as_ref(),
                class,
                ANTI_IDLE_PROTOCOL_DEADLINE_MS,
                Some(input),
            );
        },
    )?;
    loop {
        std::hint::black_box(&workers);
        clock.sleep(Duration::from_secs(1));
    }
}

/// Capture the executable identity once, then fan every worker out over that
/// one capture.
///
/// All of these workers run from the same executable by construction, so a
/// capture inside each thread would re-hash the same file once per worker and
/// stagger the concurrent pressure this operation exists to apply. The capture
/// runs before the first spawn so a capture failure fails the operation instead
/// of leaving workers that silently skip their request.
fn spawn_dead_client_workers<Shared, Capture, Request>(
    query_count: u32,
    bulk_count: u32,
    query_input: String,
    bulk_input: String,
    capture: Capture,
    request: Request,
) -> Result<Vec<std::thread::JoinHandle<()>>>
where
    Shared: Send + Sync + 'static,
    Capture: FnOnce() -> Result<Shared>,
    Request: Fn(&Shared, &'static str, String) + Send + Sync + 'static,
{
    let shared = Arc::new(capture()?);
    let request = Arc::new(request);
    let mut workers = Vec::new();
    for (class, count, input) in [
        ("query", query_count, query_input),
        ("bulk", bulk_count, bulk_input),
    ] {
        for _ in 0..count {
            let shared = Arc::clone(&shared);
            let request = Arc::clone(&request);
            let input = input.clone();
            workers.push(
                std::thread::Builder::new()
                    .name(format!("codestory-dead-client-{class}"))
                    .spawn(move || request(shared.as_ref(), class, input))?,
            );
        }
    }
    Ok(workers)
}

#[cfg(test)]
mod tests {
    use super::spawn_dead_client_workers;
    use anyhow::{Result, bail};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    /// Stands in for the captured executable identity. Its digest names which
    /// capture a worker read, so a second capture is visible in the record.
    struct SharedCapture {
        digest: String,
    }

    struct RendezvousState {
        arrived: usize,
        open: bool,
    }

    /// Releases a worker only once every worker has entered its request, so a
    /// change that serialises the fan-out cannot pass. The wait carries a
    /// timeout purely so such a change fails instead of hanging the suite; a
    /// concurrent fan-out leaves as soon as the last worker arrives and never
    /// approaches it.
    struct Rendezvous {
        expected: usize,
        state: Mutex<RendezvousState>,
        opened: Condvar,
    }

    impl Rendezvous {
        fn new(expected: usize) -> Self {
            Self {
                expected,
                state: Mutex::new(RendezvousState {
                    arrived: 0,
                    open: false,
                }),
                opened: Condvar::new(),
            }
        }

        fn arrive(&self) -> bool {
            let mut state = self.state.lock().expect("rendezvous state");
            state.arrived += 1;
            if state.arrived == self.expected {
                state.open = true;
                self.opened.notify_all();
                return true;
            }
            let (_state, wait) = self
                .opened
                .wait_timeout_while(state, Duration::from_secs(30), |state| !state.open)
                .expect("rendezvous wait");
            !wait.timed_out()
        }
    }

    #[test]
    fn every_worker_shares_one_capture_and_applies_pressure_concurrently() {
        const QUERY_COUNT: u32 = 3;
        const BULK_COUNT: u32 = 2;
        let expected_workers = (QUERY_COUNT + BULK_COUNT) as usize;
        let captures = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let rendezvous = Arc::new(Rendezvous::new(expected_workers));
        let capture_calls = Arc::clone(&captures);
        let worker_observed = Arc::clone(&observed);
        let worker_rendezvous = Arc::clone(&rendezvous);

        let workers = spawn_dead_client_workers(
            QUERY_COUNT,
            BULK_COUNT,
            "query-input".into(),
            "bulk-input".into(),
            move || {
                let captured = capture_calls.fetch_add(1, Ordering::SeqCst);
                Ok(SharedCapture {
                    digest: format!("capture-{captured}"),
                })
            },
            move |shared: &SharedCapture, class, input| {
                let concurrent = worker_rendezvous.arrive();
                worker_observed.lock().expect("observed requests").push((
                    class,
                    input,
                    shared.digest.clone(),
                    concurrent,
                ));
            },
        )
        .expect("dead client workers spawn");
        for worker in workers {
            worker.join().expect("dead client worker joins");
        }

        assert_eq!(
            captures.load(Ordering::SeqCst),
            1,
            "every dead-client worker must share one executable capture"
        );
        let observed = observed.lock().expect("observed requests");
        assert_eq!(
            observed.len(),
            expected_workers,
            "each configured worker must issue exactly one request"
        );
        let queries = observed
            .iter()
            .filter(|(class, input, _, _)| *class == "query" && input == "query-input")
            .count();
        let bulks = observed
            .iter()
            .filter(|(class, input, _, _)| *class == "bulk" && input == "bulk-input")
            .count();
        assert_eq!(
            queries, QUERY_COUNT as usize,
            "query workers keep their input"
        );
        assert_eq!(bulks, BULK_COUNT as usize, "bulk workers keep their input");
        assert!(
            observed
                .iter()
                .all(|(_, _, digest, _)| digest == "capture-0"),
            "every worker must read the one captured identity"
        );
        assert!(
            observed.iter().all(|(_, _, _, concurrent)| *concurrent),
            "workers must be in flight together, not serialised behind one another"
        );
    }

    #[test]
    fn a_failed_capture_fails_the_operation_without_issuing_a_request() {
        let requests = Arc::new(AtomicUsize::new(0));
        let worker_requests = Arc::clone(&requests);
        let error = spawn_dead_client_workers(
            3,
            2,
            "query-input".into(),
            "bulk-input".into(),
            || -> Result<SharedCapture> { bail!("embedding_executable_changed") },
            move |_: &SharedCapture, _, _| {
                worker_requests.fetch_add(1, Ordering::SeqCst);
            },
        )
        .expect_err("a failed capture must fail the operation");
        assert!(
            error.to_string().contains("embedding_executable_changed"),
            "the capture failure must reach the caller: {error}"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "a failed capture must not leave workers that skip their request"
        );
    }
}
