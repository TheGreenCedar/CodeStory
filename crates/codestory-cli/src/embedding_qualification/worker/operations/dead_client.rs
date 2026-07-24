use super::super::protocol::run_raw_protocol_exchange_with_input;
use super::ANTI_IDLE_PROTOCOL_DEADLINE_MS;
use anyhow::{Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingClientTransport, EmbeddingQualificationParameters,
    PerUserEmbeddingClient, SidecarRuntimeConfig,
};
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
    let mut workers = Vec::new();
    for _ in 0..parameters.query_count {
        workers.push(spawn_dead_client_request(
            runtime.clone(),
            "query",
            input.clone(),
        )?);
    }
    let bulk_input = documents.join("\n");
    for _ in 0..parameters.bulk_count {
        workers.push(spawn_dead_client_request(
            runtime.clone(),
            "bulk",
            bulk_input.clone(),
        )?);
    }
    loop {
        std::hint::black_box(&workers);
        clock.sleep(Duration::from_secs(1));
    }
}

fn spawn_dead_client_request(
    runtime: SidecarRuntimeConfig,
    class: &'static str,
    input: String,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("codestory-dead-client-{class}"))
        .spawn(move || {
            // Keep an admitted request alive until this process is terminated.
            // Product deadlines would add cancellation retries to the pressure
            // this worker is intended to measure.
            let transport =
                match crate::embedding_server_transport::NativeEmbeddingClientTransport::capture() {
                    Ok(transport) => transport,
                    Err(_) => return,
                };
            let clock = EmbeddingClientTransport::clock(&transport);
            let _ = run_raw_protocol_exchange_with_input(
                &runtime,
                clock.as_ref(),
                class,
                ANTI_IDLE_PROTOCOL_DEADLINE_MS,
                Some(input),
            );
        })
}
