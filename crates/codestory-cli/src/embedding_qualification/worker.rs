use self::gate::{
    current_process_start_identity, project_identity_sha256, qualification_nonce,
    read_private_request, required_absolute_directory, sha256_bytes, validate_direct_child,
    validate_gate_path, validate_private_directory, validate_worker_project, wait_for_gate,
    worker_error, write_atomic_json,
};
use self::operations::{
    run_activate_probe, run_cold_race_protocol_exchange, run_dead_client_load, run_queue_load,
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

mod gate;
mod operations;
mod protocol;

const QUALIFICATION_DIR_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIR";
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
    let (result, protocol_exchange, queue_operations, engine_identity, error) =
        if request.operation == "wait_for_absence" {
            match wait_for_owner_absence(&runtime, clock.as_ref()) {
                Ok(result) => (Some(result), None, None, None, None),
                Err(error) => (None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "resident_identity" {
            match PerUserEmbeddingClient::for_runtime(&runtime)
                .and_then(|client| client.ensure_resident())
            {
                Ok(identity) => (None, None, None, Some(identity), None),
                Err(error) => (None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "activate_probe" {
            match run_activate_probe(&runtime, clock.as_ref()) {
                Ok(error) => (None, None, None, None, Some(error)),
                Err(error) => (None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "queue_load" {
            match run_queue_load(&runtime, request.parameters, Arc::clone(&clock)) {
                Ok(operations) => (None, None, Some(operations), None, None),
                Err(error) => (None, None, None, None, Some(worker_error(&error))),
            }
        } else if request.operation == "cold_race_query" {
            match run_cold_race_protocol_exchange(&runtime, clock.as_ref()) {
                Ok(exchange) => (None, Some(exchange), None, None, None),
                Err(error) => (None, None, None, None, Some(worker_error(&error))),
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
                Ok(exchange) => (None, Some(exchange), None, None, None),
                Err(error) => (None, None, None, None, Some(worker_error(&error))),
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
                Ok(result) => (Some(result), None, None, None, None),
                Err(error) => (None, None, None, None, Some(worker_error(&error))),
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
        error,
    };
    write_atomic_json(&command.output, &output)
}
