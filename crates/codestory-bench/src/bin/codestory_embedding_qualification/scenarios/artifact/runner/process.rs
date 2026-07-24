use super::super::{CONTROL_TIMEOUT, ControlEvent, POLL, SNAPSHOT_TIMEOUT};
use super::analysis::elapsed;
use super::{
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, ProcessInvocation, RunningWorker, WorkerOutput,
};
use crate::qualification::request::QUALIFICATION_NONCE_ENV;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    EmbeddingQualificationAttemptResult, EmbeddingQualificationParameters, EmbeddingResult,
    PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS, ProcessStartProbe,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::time::Duration;

pub(super) fn existing_control_events(directory: &Path) -> Result<Vec<ControlEvent>> {
    let path = directory.join(format!("{}.events.jsonl", qualification_nonce()?));
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("read embedding qualification control events"),
    };
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice(line).context("parse embedding qualification control event")
        })
        .collect()
}

pub(super) fn qualification_command_path(directory: &Path, nonce: &str) -> PathBuf {
    directory.join(format!("{nonce}.command.json"))
}

pub(super) fn qualification_nonce() -> Result<String> {
    std::env::var(QUALIFICATION_NONCE_ENV)
        .ok()
        .filter(|nonce| {
            !nonce.is_empty()
                && nonce.len() <= 128
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_gate_closed"))
}

pub(super) fn wait_for_process_start(clock: &super::CoordinatorClock, pid: u32) -> Result<String> {
    let started = clock.now_ns();
    loop {
        match codestory_retrieval::probe_process_start_identity(pid) {
            ProcessStartProbe::Running { start_identity } => return Ok(start_identity),
            ProcessStartProbe::NotRunning => {
                bail!("embedding_qualification_worker_exited_before_identity")
            }
            ProcessStartProbe::Unknown { .. } => {}
        }
        if elapsed(clock, started) >= Duration::from_secs(2) {
            bail!("embedding_qualification_worker_identity_timeout");
        }
        clock.sleep(POLL);
    }
}

pub(super) fn wait_for_process_exit(
    clock: &super::CoordinatorClock,
    pid: u32,
    timeout: Duration,
) -> Result<()> {
    let started = clock.now_ns();
    loop {
        if matches!(
            codestory_retrieval::probe_process_start_identity(pid),
            ProcessStartProbe::NotRunning
        ) {
            return Ok(());
        }
        if elapsed(clock, started) >= timeout {
            bail!("embedding_qualification_server_process_exit_timeout");
        }
        clock.sleep(POLL);
    }
}

pub(super) fn wait_for_child(
    clock: &super::CoordinatorClock,
    child: &mut Child,
    timeout: Duration,
) -> Result<ExitStatus> {
    let started = clock.now_ns();
    loop {
        if let Some(status) = child.try_wait().context("poll qualification worker")? {
            return Ok(status);
        }
        if elapsed(clock, started) >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("embedding_qualification_worker_timeout");
        }
        clock.sleep(POLL);
    }
}

pub(super) fn cleanup_worker_files(worker: &RunningWorker) {
    let _ = fs::remove_file(&worker.request_path);
    let _ = fs::remove_file(&worker.output_path);
}

pub(super) fn validate_worker_output(
    output: &WorkerOutput,
    invocation: &ProcessInvocation,
    executable_sha256: &str,
) -> Result<()> {
    if output.schema_version != EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION
        || output.pid != invocation.pid
        || output.process_start_id != invocation.process_start_id
        || output.executable_sha256 != executable_sha256
        || output.project_identity_sha256 != invocation.project_identity_sha256
        || output.clock.domain != "awake_monotonic"
        || output.clock.boot_id.is_empty()
        || output.started_ns > output.finished_ns
        || output.inclusive_clock_api.is_empty()
        || output.inclusive_started_ns > output.inclusive_finished_ns
        || output.boot_id_started != output.clock.boot_id
        || output.boot_id_finished != output.clock.boot_id
        || (output.result.is_some() as u8
            + output.protocol_exchange.is_some() as u8
            + output.queue_operations.is_some() as u8
            + output.engine_identity.is_some() as u8
            + output.error.is_some() as u8)
            != 1
    {
        bail!("embedding_qualification_worker_output_invalid");
    }
    Ok(())
}

pub(super) fn require_worker_success(output: &WorkerOutput, phase: &str) -> Result<()> {
    let result = output
        .result
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_worker_result_missing:{phase}"))?;
    if result.operations.is_empty()
        || result
            .operations
            .iter()
            .any(|operation| operation.status != "ok")
    {
        bail!("embedding_qualification_worker_operation_failed:{phase}");
    }
    Ok(())
}

pub(super) fn validate_replay_attempts(
    attempts: &[EmbeddingQualificationAttemptResult],
    old_server_instance_id: &str,
    new_server_instance_id: &str,
    phase: &str,
) -> Result<()> {
    if attempts.len() != 2
        || attempts[0].ordinal != 1
        || attempts[1].ordinal != 2
        || attempts[0].request_id == attempts[1].request_id
        || attempts[0].server_instance_id != old_server_instance_id
        || attempts[0].outcome != "server_loss"
        || attempts[1].server_instance_id != new_server_instance_id
        || attempts[1].outcome != "completed"
        || attempts.iter().any(|attempt| {
            attempt.request_id.trim().is_empty() || attempt.submitted_ns > attempt.completed_ns
        })
        || attempts[0].submitted_ns > attempts[1].submitted_ns
    {
        bail!("embedding_qualification_replay_attempt_contract:{phase}");
    }
    Ok(())
}

pub(super) fn require_worker_error(
    output: &WorkerOutput,
    expected: &str,
    phase: &str,
) -> Result<()> {
    if output.error.as_ref().map(|error| error.code.as_str()) != Some(expected) {
        bail!("embedding_qualification_worker_error_missing:{phase}:{expected}");
    }
    Ok(())
}

pub(super) fn require_protocol_success(output: &WorkerOutput, phase: &str) -> Result<()> {
    let exchange = output.protocol_exchange.as_ref().ok_or_else(|| {
        anyhow::anyhow!("embedding_qualification_protocol_exchange_missing:{phase}")
    })?;
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

pub(super) fn query_parameters(count: u32) -> EmbeddingQualificationParameters {
    EmbeddingQualificationParameters {
        query_count: count,
        bulk_count: 0,
        documents_per_bulk: 0,
        input_bytes: 64,
        hold_ms: 0,
    }
}

pub(super) fn stall_worker_timeout() -> Duration {
    Duration::from_millis(
        PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS
            .saturating_add(SNAPSHOT_TIMEOUT.as_millis() as u64)
            .saturating_add(CONTROL_TIMEOUT.as_millis() as u64),
    )
}
