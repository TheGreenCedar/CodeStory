use self::gate::{
    current_process_start_identity, project_identity_sha256, qualification_nonce,
    read_private_request, required_absolute_directory, sha256_bytes, validate_direct_child,
    validate_gate_path, validate_private_directory, validate_worker_project, wait_for_gate,
    worker_error, write_atomic_json,
};
use self::operations::{
    run_activate_probe, run_cold_race_protocol_exchange, run_dead_client_load,
    run_measure_busy_retry, run_measure_constant_cold_query, run_measure_constant_spawn_hello,
    run_measure_hello, run_measure_product_query, run_measure_resident_identity,
    run_measure_spawn_hello, run_measure_true_idle, run_measure_vector_frame, run_queue_load,
    wait_for_owner_absence,
};
use self::protocol::run_raw_protocol_exchange;
use crate::args::InternalEmbeddingQualificationCommand;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, EmbeddingClientTransport,
    EmbeddingQualificationRequest, EmbeddingQualificationWorkerOutput as WorkerOutput,
    EmbeddingQualificationWorkerRequest as WorkerRequest,
    PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS, PerUserEmbeddingClient, SidecarRuntimeOverrides,
};
use std::sync::Arc;
use std::time::Duration;

use codestory_contracts::config_registry::EMBED_QUALIFICATION_DIR_ENV as QUALIFICATION_DIR_ENV;

mod gate;
mod operations;
mod protocol;

const ANTI_IDLE_PROTOCOL_DEADLINE_MS: u64 = 90_000;

pub(super) fn run(command: InternalEmbeddingQualificationCommand) -> Result<()> {
    let request_bytes = read_private_request(&command.request)?;
    let request: WorkerRequest =
        serde_json::from_slice(&request_bytes).context("parse embedding qualification worker")?;
    if request.schema_version != EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION {
        bail!("embedding_qualification_worker_schema_invalid");
    }
    let directory = required_absolute_directory(QUALIFICATION_DIR_ENV)?;
    validate_private_directory(&directory)?;
    validate_direct_child(&command.request, &directory, true)?;
    validate_direct_child(&command.output, &directory, false)?;
    if command.output.exists() {
        bail!("embedding_qualification_output_exists");
    }
    let nonce = qualification_nonce()?;
    if request.nonce_sha256 != sha256_bytes(nonce.as_bytes()) {
        bail!("embedding_qualification_worker_gate_closed");
    }
    let executable = crate::embedding_server_transport::ExactExecutable::capture()?;
    if request.executable_sha256 != executable.sha256() {
        bail!("embedding_qualification_worker_executable_mismatch");
    }
    validate_worker_project(&request.project)?;
    if let Some(gate) = request.start_gate.as_deref() {
        validate_gate_path(gate, &directory)?;
    }
    let is_measure_operation = request.operation.starts_with("measure_");
    if !is_measure_operation
        && (request.workload_id.is_some()
            || request.repeat.is_some()
            || request.retry_marker.is_some())
    {
        bail!("embedding_qualification_measurement_fields_unexpected");
    }
    if let Some(marker) = request.retry_marker.as_deref() {
        validate_gate_path(marker, &directory)?;
        if request.operation != "measure_busy_retry" {
            bail!("embedding_qualification_retry_marker_unexpected");
        }
    }
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let clock = EmbeddingClientTransport::clock(&transport);
    if let Some(gate) = request.start_gate.as_deref() {
        let timeout_ms = request
            .start_gate_timeout_ms
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_timeout_missing"))?;
        let timeout = Duration::from_millis(timeout_ms);
        wait_for_gate(clock.as_ref(), gate, timeout)?;
    }
    let process_start_id = current_process_start_identity()?;
    let inclusive_clock_api = crate::embedding_server_transport::inclusive_clock_api().to_string();
    let boot_id_started = crate::embedding_server_transport::boot_id()?;
    let inclusive_started_ns = crate::embedding_server_transport::inclusive_now_ns()?;
    let started_ns = clock.now_ns();
    let defaults = crate::sidecar_runtime::process_defaults();
    let runtime = crate::sidecar_runtime::for_project_auto_with_process_defaults(
        &request.project,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    );
    if request.operation == "dead_client_load" {
        return run_dead_client_load(&runtime, request.parameters, clock.as_ref());
    }
    let (result, protocol_exchange, queue_operations, engine_identity, measurement, error) =
        if is_measure_operation {
            match run_measure_operation(&request, &runtime, &clock) {
                Ok(measurement) => (None, None, None, None, Some(measurement), None),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "wait_for_absence" {
            match wait_for_owner_absence(&runtime, clock.as_ref()) {
                Ok(result) => (Some(result), None, None, None, None, None),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "resident_identity" {
            match PerUserEmbeddingClient::for_runtime(&runtime)
                .and_then(|client| client.ensure_resident())
            {
                Ok(identity) => (None, None, None, Some(identity), None, None),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "activate_probe" {
            match run_activate_probe(&runtime, clock.as_ref()) {
                Ok(error) => (None, None, None, None, None, Some(error)),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "queue_load" {
            match run_queue_load(&runtime, request.parameters, Arc::clone(&clock)) {
                Ok(operations) => (None, None, Some(operations), None, None, None),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "cold_race_query" {
            match run_cold_race_protocol_exchange(&runtime, clock.as_ref()) {
                Ok(exchange) => (None, Some(exchange), None, None, None, None),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        } else if matches!(
            request.operation.as_str(),
            "stall_protocol_bulk" | "long_protocol_query" | "long_protocol_bulk"
        ) {
            let (class, deadline_ms) = match request.operation.as_str() {
                "stall_protocol_bulk" => ("bulk", PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS),
                "long_protocol_query" => ("query", ANTI_IDLE_PROTOCOL_DEADLINE_MS),
                "long_protocol_bulk" => ("bulk", ANTI_IDLE_PROTOCOL_DEADLINE_MS),
                _ => unreachable!("matched exact protocol operations"),
            };
            match run_raw_protocol_exchange(&runtime, clock.as_ref(), class, deadline_ms) {
                Ok(exchange) => (None, Some(exchange), None, None, None, None),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        } else {
            let qualification = codestory_retrieval::run_per_user_embedding_qualification(
                &runtime,
                EmbeddingQualificationRequest {
                    schema_version: 1,
                    nonce_sha256: request.nonce_sha256,
                    scenario: request.operation,
                    parameters: request.parameters,
                },
            );
            match qualification {
                Ok(result) => (Some(result), None, None, None, None, None),
                Err(error) => (None, None, None, None, None, Some(worker_error(&error))),
            }
        };
    let finished_ns = clock.now_ns();
    let inclusive_finished_ns = crate::embedding_server_transport::inclusive_now_ns()?;
    let boot_id_finished = crate::embedding_server_transport::boot_id()?;
    let output = WorkerOutput {
        schema_version: EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
        pid: std::process::id(),
        process_start_id,
        executable_sha256: executable.sha256().into(),
        executable_version: executable.version().into(),
        project_identity_sha256: project_identity_sha256(&runtime),
        clock: clock.snapshot(),
        started_ns,
        finished_ns,
        inclusive_clock_api,
        inclusive_started_ns,
        inclusive_finished_ns,
        boot_id_started,
        boot_id_finished,
        result,
        protocol_exchange,
        queue_operations,
        engine_identity,
        measurement,
        error,
    };
    write_atomic_json(&command.output, &output)
}

/// Dispatch one measurement operation. Measurement operations stamp the
/// measurement protocol's declared start/end instants themselves and return
/// the span with its suspend witness; the driver stamps the declared phase
/// names onto the recorded sample.
fn run_measure_operation(
    request: &WorkerRequest,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    clock: &Arc<dyn codestory_retrieval::AwakeMonotonicClock>,
) -> Result<codestory_retrieval::EmbeddingQualificationWorkerMeasurement> {
    let workload_id = request
        .workload_id
        .as_deref()
        .filter(|workload_id| !workload_id.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_measurement_workload_missing"))?;
    let repeat = request
        .repeat
        .filter(|repeat| *repeat > 0)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_measurement_repeat_missing"))?;
    match request.operation.as_str() {
        "measure_hello" => run_measure_hello(runtime, clock.as_ref()),
        "measure_spawn_hello" => run_measure_spawn_hello(runtime, clock.as_ref()),
        "measure_constant_spawn_hello" => run_measure_constant_spawn_hello(runtime, clock.as_ref()),
        "measure_constant_cold_query" => run_measure_constant_cold_query(
            runtime,
            clock.as_ref(),
            workload_id,
            repeat,
            request.parameters.input_bytes,
        ),
        "measure_product_query" => run_measure_product_query(
            runtime,
            clock.as_ref(),
            workload_id,
            repeat,
            request.parameters.input_bytes,
        ),
        "measure_query_frame" => run_measure_vector_frame(
            runtime,
            clock.as_ref(),
            "query",
            workload_id,
            repeat,
            &request.parameters,
        ),
        "measure_bulk_frame" => run_measure_vector_frame(
            runtime,
            clock.as_ref(),
            "bulk",
            workload_id,
            repeat,
            &request.parameters,
        ),
        "measure_resident_identity" => run_measure_resident_identity(runtime, clock.as_ref()),
        "measure_true_idle" => run_measure_true_idle(
            &PerUserEmbeddingClient::for_runtime(runtime)?,
            clock.as_ref(),
            request.parameters.input_bytes,
        ),
        "measure_busy_retry" => {
            let marker = request
                .retry_marker
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("embedding_qualification_retry_marker_missing"))?;
            run_measure_busy_retry(
                runtime,
                Arc::clone(clock),
                workload_id,
                repeat,
                request.parameters.input_bytes,
                marker,
            )
        }
        _ => bail!("embedding_qualification_measure_operation_unknown"),
    }
}
