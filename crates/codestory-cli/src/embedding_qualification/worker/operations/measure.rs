//! Measurement operations that stamp the measurement protocol's declared
//! start/end instants inside the worker process.
//!
//! PR #1420 moved qualification measurement into per-sample worker processes,
//! and the driver stamped every metric with the worker's whole-process window.
//! The protocol's `phase_boundaries` name metric-specific instants (frame
//! write -> validated response, typed retry -> named condition true, ...), and
//! the WP5-floored frozen constants derive their meaning from those instants,
//! so each operation here observes the declared window itself and returns it
//! as a `MeasurementSpan` with the suspend-inclusive witness sampled at the
//! same instants.

use super::super::gate::{
    POLL, elapsed, project_identity_sha256, qualification_request_id, sha256_bytes,
    write_atomic_json,
};
use super::super::protocol::{
    connect_until, read_protocol_frame, run_raw_protocol_exchange,
    run_raw_protocol_exchange_with_input, validated_hello, write_protocol_frame,
};
use super::ANTI_IDLE_PROTOCOL_DEADLINE_MS;
use super::owner_exit::{OwnerExitObservation, observe_owner_exit, wait_for_owner_exit};
use super::queue::{QueueOperation, run_queue_operation};
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingCompatibility, EmbeddingEngineIdentity, EmbeddingOperation,
    EmbeddingProtocolRequest, EmbeddingProtocolResponse, EmbeddingQualificationParameters,
    EmbeddingQualificationWorkerMeasurement as WorkerMeasurement,
    EmbeddingQualificationWorkerMeasurementSpan as WorkerMeasurementSpan,
    EmbeddingQualificationWorkerProtocolExchange as WorkerProtocolExchange, EmbeddingResult,
    EmbeddingServerSnapshot, EmbeddingServerStream, PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
    PER_USER_EMBEDDING_PROTOCOL_V1, PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
    PerUserEmbeddingClient, SidecarRuntimeConfig,
};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const EXISTING_OWNER_CONNECT_BUDGET: Duration = Duration::from_secs(2);
const SPAWN_CONVERGENCE_BUDGET: Duration = Duration::from_secs(15);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
const OWNER_ABSENCE_GRACE: Duration = Duration::from_secs(30);
const QUALIFICATION_QUEUE_CAPACITY: u64 = 64;
const MEASUREMENT_FRAME_DEADLINE: Duration = Duration::from_secs(180);

/// Deterministic `sha256_counter_utf8_v1` input generator declared by the
/// measurement protocol's workloads.
fn workload_input(workload_id: &str, repeat: u32, ordinal: u32, bytes: usize) -> String {
    let mut output = String::with_capacity(bytes);
    let mut counter = 0_u64;
    while output.len() < bytes {
        output.push_str(&sha256_bytes(
            format!("{workload_id}:{repeat}:{ordinal}:{counter}").as_bytes(),
        ));
        counter = counter.saturating_add(1);
    }
    output.truncate(bytes);
    debug_assert!(output.is_ascii());
    output
}

fn workload_documents(workload_id: &str, repeat: u32, count: usize, bytes: usize) -> Vec<String> {
    (0..count)
        .map(|ordinal| workload_input(workload_id, repeat, ordinal as u32, bytes))
        .collect()
}

struct MeasurementSpanStart {
    awake_started_ns: u64,
    inclusive_started_ns: u64,
    boot_id_started: String,
}

/// Stamp the declared start instant: boot id and suspend-inclusive reading
/// first, awake reading last so the awake timestamp sits closest to the
/// measured interaction.
fn begin_span(clock: &dyn AwakeMonotonicClock) -> Result<MeasurementSpanStart> {
    let boot_id_started = crate::embedding_server_transport::boot_id()?;
    let inclusive_started_ns = crate::embedding_server_transport::inclusive_now_ns()?;
    let awake_started_ns = clock.now_ns();
    Ok(MeasurementSpanStart {
        awake_started_ns,
        inclusive_started_ns,
        boot_id_started,
    })
}

/// Stamp the declared end instant: awake reading first, then the
/// suspend-inclusive reading and boot id.
fn finish_span(
    clock: &dyn AwakeMonotonicClock,
    start: MeasurementSpanStart,
) -> Result<WorkerMeasurementSpan> {
    let awake_finished_ns = clock.now_ns();
    let inclusive_finished_ns = crate::embedding_server_transport::inclusive_now_ns()?;
    let boot_id_finished = crate::embedding_server_transport::boot_id()?;
    if start.awake_started_ns > awake_finished_ns
        || start.inclusive_started_ns > inclusive_finished_ns
        || start.boot_id_started != boot_id_finished
    {
        bail!("embedding_qualification_measurement_clock_changed");
    }
    Ok(WorkerMeasurementSpan {
        awake_started_ns: start.awake_started_ns,
        awake_finished_ns,
        inclusive_started_ns: start.inclusive_started_ns,
        inclusive_finished_ns,
        boot_id_started: start.boot_id_started,
        boot_id_finished,
    })
}

fn wait_for_observed_snapshot(
    client: &PerUserEmbeddingClient,
    clock: &dyn AwakeMonotonicClock,
    timeout: Duration,
    error: &'static str,
    predicate: impl Fn(&EmbeddingServerSnapshot) -> bool,
) -> Result<EmbeddingServerSnapshot> {
    let started = clock.now_ns();
    loop {
        if let Some(snapshot) = client.observe()?
            && predicate(&snapshot)
        {
            return Ok(snapshot);
        }
        if elapsed(clock, started) >= timeout {
            bail!("{error}");
        }
        clock.sleep(POLL);
    }
}

fn resident_engine_generation(snapshot: &EmbeddingServerSnapshot) -> bool {
    snapshot
        .engine
        .as_ref()
        .is_some_and(|engine| engine.load_generation > 0)
}

fn measurement(
    span: WorkerMeasurementSpan,
    snapshot: EmbeddingServerSnapshot,
) -> WorkerMeasurement {
    WorkerMeasurement {
        span,
        snapshot,
        engine_identity: None,
        request_id: None,
        completed_documents: None,
        pressure: None,
    }
}

/// `existing_owner_connect`: span
/// [`client_connect_started` -> `compatible_hello_validated`] — one connect
/// plus one validated hello against the resident owner, transport capture
/// outside the window.
pub(in crate::embedding_qualification::worker) fn run_measure_hello(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<WorkerMeasurement> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let start = begin_span(clock)?;
    let mut stream = connect_until(&transport, clock, EXISTING_OWNER_CONNECT_BUDGET, None)?;
    let hello = validated_hello(&mut stream, &transport, runtime, clock)?;
    let span = finish_span(clock, start)?;
    if !resident_engine_generation(&hello) {
        bail!("embedding_qualification_existing_owner_not_resident");
    }
    Ok(measurement(span, hello))
}

/// `spawn_convergence`: the worker proves owner absence itself, then spans
/// [`owner_absence_proven` -> `compatible_hello_validated`] around
/// spawn + connect + validated hello only — no model load and no embed inside
/// the window. Residency is established after the window purely to witness
/// the sample's server identity.
pub(in crate::embedding_qualification::worker) fn run_measure_spawn_hello(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<WorkerMeasurement> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    if client.observe()?.is_some() {
        bail!("embedding_qualification_spawn_owner_present");
    }
    let start = begin_span(clock)?;
    let spawn_attempt = transport.spawn_exact_current_exe()?;
    let mut stream = connect_until(
        &transport,
        clock,
        SPAWN_CONVERGENCE_BUDGET,
        Some(&spawn_attempt),
    )?;
    let hello = validated_hello(&mut stream, &transport, runtime, clock)?;
    let span = finish_span(clock, start)?;
    let _ = client.ensure_resident()?;
    let resident = wait_for_observed_snapshot(
        &client,
        clock,
        SNAPSHOT_TIMEOUT,
        "embedding_qualification_spawn_resident_timeout",
        resident_engine_generation,
    )?;
    if resident.process.server_instance_id != hello.process.server_instance_id {
        bail!("embedding_qualification_spawn_owner_changed");
    }
    Ok(measurement(span, resident))
}

/// `cold_first_vector` and `first_product_ready`: span
/// [`product_request_started(...)` -> `..._validated`] around client
/// construction plus one `embed_query`; the client validates identity,
/// dimension, finiteness, and normalization before returning, so the return
/// instant is the validated-evidence instant.
pub(in crate::embedding_qualification::worker) fn run_measure_product_query(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    workload_id: &str,
    repeat: u32,
    input_bytes: u32,
) -> Result<WorkerMeasurement> {
    let input = workload_input(workload_id, repeat, 0, input_bytes.max(1) as usize);
    let start = begin_span(clock)?;
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let vector = client.embed_query(&input)?;
    let span = finish_span(clock, start)?;
    if vector.is_empty() {
        bail!("embedding_qualification_product_vector_missing");
    }
    let snapshot = client
        .observe()?
        .filter(resident_engine_generation)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_product_owner_missing"))?;
    Ok(measurement(span, snapshot))
}

/// `warm_query_ipc`, `warm_bulk_ipc`, and both bulk throughput metrics:
/// connect and validated hello outside the window, then span
/// [`client_frame_started` -> `..._validated`] around one raw protocol frame
/// write, the response read, and full response validation (identity plus
/// per-row norm checks) — the pre-#1420 `measure_vector_operation` moved into
/// the worker.
pub(in crate::embedding_qualification::worker) fn run_measure_vector_frame(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    class: &str,
    workload_id: &str,
    repeat: u32,
    parameters: &EmbeddingQualificationParameters,
) -> Result<WorkerMeasurement> {
    let input_bytes = parameters.input_bytes.max(1) as usize;
    let inputs = match class {
        "query" => vec![workload_input(workload_id, repeat, 0, input_bytes)],
        "bulk" => {
            if parameters.documents_per_bulk == 0 {
                bail!("embedding_qualification_measurement_inputs_invalid");
            }
            workload_documents(
                workload_id,
                repeat,
                parameters.documents_per_bulk as usize,
                input_bytes,
            )
        }
        _ => bail!("embedding_qualification_measurement_class_invalid"),
    };
    if inputs.is_empty() || inputs.iter().any(|input| input.trim().is_empty()) {
        bail!("embedding_qualification_measurement_inputs_invalid");
    }
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let mut stream = connect_until(&transport, clock, EXISTING_OWNER_CONNECT_BUDGET, None)?;
    let hello = validated_hello(&mut stream, &transport, runtime, clock)?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let request_id = qualification_request_id(&format!("measurement-{class}"), clock.now_ns());
    let scope_id = project_identity_sha256(runtime);
    let deadline_ms = MEASUREMENT_FRAME_DEADLINE.as_millis() as u64;
    let operation = match class {
        "query" => EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            cancel_token: None,
            input: inputs[0].clone(),
        },
        _ => EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            cancel_token: None,
            inputs: inputs.clone(),
        },
    };
    EmbeddingServerStream::set_read_timeout(&stream, Some(MEASUREMENT_FRAME_DEADLINE))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(MEASUREMENT_FRAME_DEADLINE))?;
    let start = begin_span(clock)?;
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.clone(),
            compatibility,
            operation,
        },
        &[],
    )?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_protocol_frame(&mut stream)?;
    let identity =
        validate_vector_response(&response, &payload, &request_id, inputs.len(), &hello)?;
    let span = finish_span(clock, start)?;
    Ok(WorkerMeasurement {
        span,
        snapshot: hello,
        engine_identity: Some(identity),
        request_id: Some(request_id),
        completed_documents: Some(inputs.len() as u64),
        pressure: None,
    })
}

/// `backend_observed_accelerator_residency`: span
/// [`accelerator_measurement_started` -> `backend_residency_evidence_validated`]
/// around client construction plus `ensure_resident` returning the validated
/// engine identity.
pub(in crate::embedding_qualification::worker) fn run_measure_resident_identity(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<WorkerMeasurement> {
    let start = begin_span(clock)?;
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let identity = client.ensure_resident()?;
    let span = finish_span(clock, start)?;
    let snapshot = client
        .observe()?
        .filter(resident_engine_generation)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_residency_owner_missing"))?;
    Ok(WorkerMeasurement {
        span,
        snapshot,
        engine_identity: Some(identity),
        request_id: None,
        completed_documents: None,
        pressure: None,
    })
}

fn true_idle_scheduler_is_drained(
    active_request_count: u64,
    query_depth: u64,
    bulk_depth: u64,
    lease_count: u64,
) -> bool {
    active_request_count == 0 && query_depth == 0 && bulk_depth == 0 && lease_count == 0
}

fn snapshot_is_true_idle_boundary(snapshot: &EmbeddingServerSnapshot) -> bool {
    snapshot.lifecycle == "resident"
        && resident_engine_generation(snapshot)
        && true_idle_scheduler_is_drained(
            snapshot.scheduler.active_request_count,
            snapshot.scheduler.query_depth,
            snapshot.scheduler.bulk_depth,
            snapshot.scheduler.lease_count,
        )
}

fn same_true_idle_owner(
    expected_server_instance_id: &str,
    observed_server_instance_id: &str,
) -> bool {
    expected_server_instance_id == observed_server_instance_id
}

fn validate_true_idle_boundary(
    expected_server_instance_id: &str,
    snapshot: &EmbeddingServerSnapshot,
) -> Result<()> {
    if !same_true_idle_owner(
        expected_server_instance_id,
        &snapshot.process.server_instance_id,
    ) {
        bail!("embedding_qualification_true_idle_owner_changed");
    }
    if !snapshot_is_true_idle_boundary(snapshot) {
        bail!("embedding_qualification_true_idle_not_quiescent");
    }
    Ok(())
}

/// The product and observation operations used by the true-idle measurement.
///
/// The worker implements this interface with the real per-user embedding
/// client. Tests use a scripted implementation to execute this same
/// `run_measure_true_idle` algorithm rather than a helper that production can
/// bypass.
pub(in crate::embedding_qualification::worker) trait TrueIdleMeasurementClient {
    fn observe_snapshot(&self) -> Result<Option<EmbeddingServerSnapshot>>;
    fn complete_product_query(&self, input: &str) -> Result<()>;
    fn observe_owner_exit(&self) -> Result<OwnerExitObservation>;
}

impl TrueIdleMeasurementClient for PerUserEmbeddingClient {
    fn observe_snapshot(&self) -> Result<Option<EmbeddingServerSnapshot>> {
        self.observe()
    }

    fn complete_product_query(&self, input: &str) -> Result<()> {
        let _ = self.embed_query(input)?;
        Ok(())
    }

    fn observe_owner_exit(&self) -> Result<OwnerExitObservation> {
        observe_owner_exit(self)
    }
}

/// `true_idle_exit`: this worker completes the last product request itself,
/// stamps its return as `last_queued_active_or_leased_work_ended`, proves the
/// resident scheduler is drained, then ends at the observation that returned
/// no owner (`engine_and_server_absent`).
pub(in crate::embedding_qualification::worker) fn run_measure_true_idle(
    client: &dyn TrueIdleMeasurementClient,
    clock: &dyn AwakeMonotonicClock,
    input_bytes: u32,
) -> Result<WorkerMeasurement> {
    let expected_owner = client
        .observe_snapshot()?
        .filter(resident_engine_generation)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_true_idle_owner_missing"))?;
    let input = "q".repeat(input_bytes.max(1) as usize);
    client.complete_product_query(&input)?;
    let start = begin_span(clock)?;
    let idle_owner = client
        .observe_snapshot()?
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_true_idle_owner_missing"))?;
    validate_true_idle_boundary(&expected_owner.process.server_instance_id, &idle_owner)?;
    let timeout = Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
        .saturating_add(OWNER_ABSENCE_GRACE);
    let wait_started = clock.now_ns();
    wait_for_owner_exit(
        clock,
        wait_started,
        timeout,
        &idle_owner.process.server_instance_id,
        || client.observe_owner_exit(),
    )?;
    let span = finish_span(clock, start)?;
    Ok(measurement(span, idle_owner))
}

/// `busy_retry_usefulness`: the whole saturated-65th-retry experiment in one
/// process, because the clock policy forbids cross-process timestamp
/// subtraction. The driver holds the bulk and query classes before spawning
/// this worker; the worker seeds one bulk, fills the query queue to capacity,
/// validates the typed capacity retry for the 65th query, stamps
/// `typed_retry_emitted`, signals the driver through the retry marker, and
/// stamps `named_retry_condition_became_true` when the first queued query
/// completes after the driver releases the classes.
pub(in crate::embedding_qualification::worker) fn run_measure_busy_retry(
    runtime: &SidecarRuntimeConfig,
    clock: Arc<dyn AwakeMonotonicClock>,
    workload_id: &str,
    repeat: u32,
    input_bytes: u32,
    retry_marker: &Path,
) -> Result<WorkerMeasurement> {
    let input_bytes = input_bytes.max(1) as usize;
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let observer = PerUserEmbeddingClient::for_runtime(runtime)?;
    let project_identity = project_identity_sha256(runtime);

    let seed_runtime = runtime.clone();
    let seed_clock = Arc::clone(&clock);
    let seed = std::thread::Builder::new()
        .name("codestory-measurement-busy-seed".into())
        .spawn(move || {
            run_raw_protocol_exchange(
                &seed_runtime,
                seed_clock.as_ref(),
                "bulk",
                ANTI_IDLE_PROTOCOL_DEADLINE_MS,
            )
        })?;
    let seed_active = wait_for_observed_snapshot(
        &observer,
        clock.as_ref(),
        SNAPSHOT_TIMEOUT,
        "embedding_qualification_busy_seed_timeout",
        |snapshot| {
            snapshot
                .scheduler
                .active_request
                .as_ref()
                .is_some_and(|active| active.class == "bulk")
        },
    )?;
    if seed_active.scheduler.query_capacity != QUALIFICATION_QUEUE_CAPACITY {
        bail!("embedding_qualification_busy_retry_query_capacity_invalid");
    }

    let mut queued = Vec::new();
    for ordinal in 0..QUALIFICATION_QUEUE_CAPACITY {
        let ordinal = u32::try_from(ordinal).context("convert qualification query ordinal")?;
        let input = workload_input(workload_id, repeat, ordinal + 1, input_bytes);
        let correlation_id = format!(
            "measurement-busy-{repeat}-{ordinal}-{}",
            &project_identity[..12]
        );
        let (submitted_tx, submitted_rx) = std::sync::mpsc::sync_channel(1);
        let operation = QueueOperation {
            runtime: runtime.clone(),
            transport: transport.clone(),
            clock: Arc::clone(&clock),
            project_identity_sha256: project_identity.clone(),
            class: "query",
            ordinal,
            correlation_id,
            input: Some(input),
            submitted_tx,
        };
        queued.push(
            std::thread::Builder::new()
                .name(format!("codestory-measurement-busy-query-{ordinal}"))
                .spawn(move || run_queue_operation(operation))?,
        );
        submitted_rx
            .recv_timeout(CONTROL_TIMEOUT)
            .context("wait for busy retry queue submission")?;
        let expected_depth = u64::from(ordinal).saturating_add(1);
        wait_for_observed_snapshot(
            &observer,
            clock.as_ref(),
            QUEUE_SETUP_TIMEOUT,
            "embedding_qualification_busy_enqueue_timeout",
            |snapshot| snapshot.scheduler.query_depth >= expected_depth,
        )?;
    }
    wait_for_observed_snapshot(
        &observer,
        clock.as_ref(),
        QUEUE_SETUP_TIMEOUT,
        "embedding_qualification_busy_saturation_timeout",
        |snapshot| {
            snapshot.scheduler.query_capacity == QUALIFICATION_QUEUE_CAPACITY
                && snapshot.scheduler.query_depth == QUALIFICATION_QUEUE_CAPACITY
        },
    )?;

    let overflow_input = workload_input(workload_id, repeat, 65, input_bytes);
    let overflow = run_raw_protocol_exchange_with_input(
        runtime,
        clock.as_ref(),
        "query",
        ANTI_IDLE_PROTOCOL_DEADLINE_MS,
        Some(overflow_input.clone()),
    )?;
    let pressure = overflow
        .response
        .as_ref()
        .and_then(|response| response.error.as_ref())
        .and_then(|error| error.capacity.as_ref())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_busy_retry_pressure_missing"))?;
    if overflow.terminal_transport_error.is_some()
        || pressure.reason != "queue_full"
        || pressure.queue_class != "query"
        || pressure.capacity != QUALIFICATION_QUEUE_CAPACITY
        || pressure.depth != pressure.capacity
        || pressure.retry_condition.trim().is_empty()
    {
        bail!("embedding_qualification_busy_retry_pressure_invalid");
    }

    let start = begin_span(clock.as_ref())?;
    write_atomic_json(
        retry_marker,
        &json!({ "schema_version": 1, "typed_retry_emitted": true }),
    )?;
    let first = queued.remove(0);
    let first_operation = first
        .join()
        .map_err(|_| anyhow::anyhow!("embedding_qualification_busy_query_panicked"))??;
    if first_operation.status != "ok" || first_operation.error.is_some() {
        bail!("embedding_qualification_busy_retry_query_failed");
    }
    let span = finish_span(clock.as_ref(), start)?;

    let replay = run_raw_protocol_exchange_with_input(
        runtime,
        clock.as_ref(),
        "query",
        ANTI_IDLE_PROTOCOL_DEADLINE_MS,
        Some(overflow_input),
    )?;
    require_vector_exchange(&replay, "busy_retry_replay")?;
    let seed = seed
        .join()
        .map_err(|_| anyhow::anyhow!("embedding_qualification_busy_seed_panicked"))??;
    require_vector_exchange(&seed, "busy_retry_seed")?;
    for worker in queued {
        let operation = worker
            .join()
            .map_err(|_| anyhow::anyhow!("embedding_qualification_busy_query_panicked"))??;
        if operation.status != "ok" || operation.error.is_some() {
            bail!("embedding_qualification_busy_retry_query_failed");
        }
    }
    wait_for_observed_snapshot(
        &observer,
        clock.as_ref(),
        QUEUE_SETUP_TIMEOUT,
        "embedding_qualification_busy_drain_timeout",
        |snapshot| {
            snapshot.scheduler.active_request_count == 0
                && snapshot.scheduler.query_depth == 0
                && snapshot.scheduler.bulk_depth == 0
        },
    )?;
    let snapshot = observer
        .observe()?
        .filter(resident_engine_generation)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_busy_owner_missing"))?;
    Ok(WorkerMeasurement {
        span,
        snapshot,
        engine_identity: None,
        request_id: None,
        completed_documents: None,
        pressure: Some(pressure),
    })
}

fn require_vector_exchange(exchange: &WorkerProtocolExchange, phase: &str) -> Result<()> {
    if exchange.terminal_transport_error.is_some()
        || exchange.response.as_ref().is_none_or(|response| {
            response.error.is_some()
                || !matches!(response.result, Some(EmbeddingResult::Vectors { .. }))
        })
        || exchange.response_payload_bytes == 0
    {
        bail!("embedding_qualification_protocol_exchange_failed:{phase}");
    }
    Ok(())
}

/// Full validation of one measured vector response: protocol identity,
/// request correlation, engine evidence, payload shape, and per-row norm
/// checks — the response is only "validated" once every row proved finite and
/// normalized, matching the declared end phases of the frame metrics.
fn validate_vector_response(
    response: &EmbeddingProtocolResponse,
    payload: &[u8],
    request_id: &str,
    expected_rows: usize,
    hello: &EmbeddingServerSnapshot,
) -> Result<EmbeddingEngineIdentity> {
    if response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || response.request_id != request_id
        || response.error.is_some()
    {
        bail!("embedding_qualification_measurement_response_invalid");
    }
    let (rows, columns, encoding, identity) = match response.result.as_ref() {
        Some(EmbeddingResult::Vectors {
            rows,
            columns,
            encoding,
            identity,
        }) => (*rows, *columns, encoding, identity.as_ref()),
        _ => bail!("embedding_qualification_measurement_vectors_missing"),
    };
    let expected_columns = codestory_retrieval::semantic_vector_dim() as u32;
    let expected_bytes = usize::try_from(rows)
        .ok()
        .and_then(|rows| rows.checked_mul(columns as usize))
        .and_then(|elements| elements.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_measurement_payload_overflow"))?;
    if rows as usize != expected_rows
        || columns != expected_columns
        || encoding != "f32_le"
        || payload.len() != expected_bytes
        || identity.server_instance_id != hello.process.server_instance_id
        || identity.residency != "resident"
        || !identity.worker_alive
        || identity.load_error.is_some()
        || identity.model_load_count == 0
        || identity.model_digest.is_empty()
        || identity.ggml_build_identity.is_empty()
    {
        bail!("embedding_qualification_measurement_vector_contract_invalid");
    }
    for row in payload.chunks_exact(columns as usize * std::mem::size_of::<f32>()) {
        let norm = row
            .chunks_exact(std::mem::size_of::<f32>())
            .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
            .try_fold(0.0_f64, |sum, value| {
                if value.is_finite() {
                    Ok(sum + f64::from(value) * f64::from(value))
                } else {
                    Err(anyhow::anyhow!(
                        "embedding_qualification_measurement_vector_non_finite"
                    ))
                }
            })?;
        if !(0.98..=1.02).contains(&norm.sqrt()) {
            bail!("embedding_qualification_measurement_vector_not_normalized");
        }
    }
    Ok(identity.clone())
}

#[cfg(test)]
mod tests {
    use super::super::owner_exit::OwnerExitObservation;
    use super::{
        TrueIdleMeasurementClient, run_measure_true_idle, true_idle_scheduler_is_drained,
        validate_true_idle_boundary, workload_input,
    };
    use anyhow::Result;
    use codestory_retrieval::{
        AwakeMonotonicClock, EmbeddingServerAuthoritySnapshot, EmbeddingServerClockSnapshot,
        EmbeddingServerEngineSnapshot, EmbeddingServerProcessSnapshot,
        EmbeddingServerProtocolSnapshot, EmbeddingServerSchedulerSnapshot, EmbeddingServerSnapshot,
        PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
    };
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct ScriptClock {
        now_ns: AtomicU64,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ScriptClock {
        fn new(events: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                now_ns: AtomicU64::new(0),
                events,
            }
        }

        fn set(&self, now_ns: u64) {
            self.now_ns.store(now_ns, Ordering::Release);
        }
    }

    impl AwakeMonotonicClock for ScriptClock {
        fn now_ns(&self) -> u64 {
            let now_ns = self.now_ns.load(Ordering::Acquire);
            if now_ns == 100 {
                self.events
                    .lock()
                    .expect("lock events")
                    .push("span_started");
            }
            now_ns
        }

        fn sleep(&self, duration: Duration) {
            self.now_ns.fetch_add(
                u64::try_from(duration.as_nanos()).expect("test duration fits u64"),
                Ordering::AcqRel,
            );
        }

        fn snapshot(&self) -> EmbeddingServerClockSnapshot {
            EmbeddingServerClockSnapshot {
                domain: "awake_monotonic".into(),
                api: "script_clock".into(),
                boot_id: "test-boot".into(),
                resolution_ns: 1,
            }
        }
    }

    struct ScriptedTrueIdleClient {
        clock: Arc<ScriptClock>,
        events: Arc<Mutex<Vec<&'static str>>>,
        snapshots: Mutex<VecDeque<EmbeddingServerSnapshot>>,
        exit_observations: Mutex<VecDeque<OwnerExitObservation>>,
        product_calls: AtomicUsize,
    }

    impl TrueIdleMeasurementClient for ScriptedTrueIdleClient {
        fn observe_snapshot(&self) -> Result<Option<EmbeddingServerSnapshot>> {
            let (snapshot, remaining) = {
                let mut snapshots = self.snapshots.lock().expect("lock snapshots");
                let snapshot = snapshots
                    .pop_front()
                    .expect("the algorithm observed past its snapshot script");
                (snapshot, snapshots.len())
            };
            let event = if remaining == 1 {
                "expected_owner_observed"
            } else {
                self.clock.set(145);
                "same_owner_drained_observed"
            };
            self.events.lock().expect("lock events").push(event);
            Ok(Some(snapshot))
        }

        fn complete_product_query(&self, input: &str) -> Result<()> {
            assert_eq!(input, "q".repeat(256));
            self.product_calls.fetch_add(1, Ordering::AcqRel);
            self.clock.set(100);
            self.events
                .lock()
                .expect("lock events")
                .push("product_completed");
            Ok(())
        }

        fn observe_owner_exit(&self) -> Result<OwnerExitObservation> {
            let observation = self
                .exit_observations
                .lock()
                .expect("lock exit observations")
                .pop_front()
                .expect("the algorithm observed past its exit script");
            let event = match &observation {
                OwnerExitObservation::Present(_) => "same_owner_still_present",
                OwnerExitObservation::Lost => "owner_connection_lost",
                OwnerExitObservation::Absent => "owner_absence_observed",
            };
            self.events.lock().expect("lock events").push(event);
            Ok(observation)
        }
    }

    fn true_idle_snapshot(
        owner: &str,
        active_request_count: u64,
        query_depth: u64,
        bulk_depth: u64,
        lease_count: u64,
    ) -> EmbeddingServerSnapshot {
        EmbeddingServerSnapshot {
            schema_version: PER_USER_EMBEDDING_SERVER_SNAPSHOT_SCHEMA_VERSION,
            event_sequence: 1,
            lifecycle: "resident".into(),
            clock: EmbeddingServerClockSnapshot {
                domain: "awake_monotonic".into(),
                api: "script_clock".into(),
                boot_id: "test-boot".into(),
                resolution_ns: 1,
            },
            protocol: EmbeddingServerProtocolSnapshot::current(),
            authority: EmbeddingServerAuthoritySnapshot {
                endpoint_namespace_id: "endpoint".into(),
                lifetime_authority_id: "authority".into(),
                listener_id: "listener".into(),
                peer_verified: true,
            },
            process: EmbeddingServerProcessSnapshot {
                server_instance_id: owner.into(),
                pid: 42,
                process_start_id: "server-start".into(),
                executable_sha256: "a".repeat(64),
                executable_version: "0.16.3".into(),
            },
            scheduler: EmbeddingServerSchedulerSnapshot {
                query_capacity: 64,
                query_depth,
                bulk_capacity: 64,
                bulk_depth,
                connection_count: 1,
                active_request_count,
                lease_count,
                active_request: None,
            },
            engine: Some(EmbeddingServerEngineSnapshot {
                engine_owner_id: owner.into(),
                native_worker_id: "native-worker".into(),
                load_generation: 1,
                model_load_count: 1,
                successful_encode_count: 1,
            }),
            failure: None,
        }
    }

    #[test]
    fn workload_inputs_are_deterministic_ascii_and_distinct_per_ordinal() {
        let first = workload_input("warm_query_256b_v1", 1, 0, 256);
        let again = workload_input("warm_query_256b_v1", 1, 0, 256);
        let other_repeat = workload_input("warm_query_256b_v1", 2, 0, 256);
        let other_ordinal = workload_input("warm_query_256b_v1", 1, 1, 256);
        assert_eq!(first, again);
        assert_eq!(first.len(), 256);
        assert!(first.is_ascii());
        assert_ne!(first, other_repeat);
        assert_ne!(first, other_ordinal);
    }

    #[test]
    fn true_idle_start_is_stamped_at_product_completion_before_observation_lag() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let clock = Arc::new(ScriptClock::new(Arc::clone(&events)));
        let client = ScriptedTrueIdleClient {
            clock: Arc::clone(&clock),
            events: Arc::clone(&events),
            snapshots: Mutex::new(VecDeque::from([
                true_idle_snapshot("measured-owner", 0, 0, 0, 0),
                true_idle_snapshot("measured-owner", 0, 0, 0, 0),
            ])),
            exit_observations: Mutex::new(VecDeque::from([
                OwnerExitObservation::Present("measured-owner".into()),
                OwnerExitObservation::Absent,
            ])),
            product_calls: AtomicUsize::new(0),
        };

        let measurement =
            run_measure_true_idle(&client, clock.as_ref(), 256).expect("measure true idle");

        assert_eq!(client.product_calls.load(Ordering::Acquire), 1);
        assert_eq!(measurement.span.awake_started_ns, 100);
        assert!(measurement.span.awake_finished_ns >= 145);
        assert_eq!(
            measurement.snapshot.process.server_instance_id,
            "measured-owner"
        );
        assert_eq!(
            *events.lock().expect("lock events"),
            [
                "expected_owner_observed",
                "product_completed",
                "span_started",
                "same_owner_drained_observed",
                "same_owner_still_present",
                "owner_absence_observed",
            ]
        );
    }

    #[test]
    fn true_idle_boundary_rejects_each_queued_active_or_leased_shape() {
        let occupied = [
            ("active", (1, 0, 0, 0)),
            ("query", (0, 1, 0, 0)),
            ("bulk", (0, 0, 1, 0)),
            ("lease", (0, 0, 0, 1)),
        ];
        assert!(true_idle_scheduler_is_drained(0, 0, 0, 0));
        for (label, state) in occupied {
            assert!(
                !true_idle_scheduler_is_drained(state.0, state.1, state.2, state.3),
                "{label} work must keep the true-idle boundary open"
            );
            let result = validate_true_idle_boundary(
                "measured-owner",
                &true_idle_snapshot("measured-owner", state.0, state.1, state.2, state.3),
            );
            assert_eq!(
                result
                    .expect_err("occupied scheduler must reject the sample")
                    .to_string(),
                "embedding_qualification_true_idle_not_quiescent",
                "{label} work must fail closed rather than shift the boundary"
            );
        }
    }

    #[test]
    fn true_idle_boundary_rejects_a_different_owner_after_product_completion() {
        let result = validate_true_idle_boundary(
            "measured-owner",
            &true_idle_snapshot("replacement-owner", 0, 0, 0, 0),
        );
        assert_eq!(
            result
                .expect_err("replacement owner must reject the sample")
                .to_string(),
            "embedding_qualification_true_idle_owner_changed"
        );
    }
}
