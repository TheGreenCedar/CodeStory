use super::super::{
    CONTROL_TIMEOUT, ControlEvent, IDLE_EXIT_GRACE, POLL, QUEUE_SETUP_TIMEOUT, SNAPSHOT_TIMEOUT,
};
use super::analysis::elapsed;
use super::{
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, ProcessInvocation, RunningWorker, WorkerOutput,
};
use crate::qualification::request::QUALIFICATION_NONCE_ENV;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    EmbeddingClientBudgets, EmbeddingQualificationAttemptResult, EmbeddingQualificationParameters,
    EmbeddingResult, PER_USER_EMBEDDING_BULK_REQUEST_DEADLINE_MS,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS, ProcessStartProbe,
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
            + output.measurement.is_some() as u8
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
    // Crash and stall scenarios kill a live peer mid-RPC, so the original
    // loss must be the classified transport disconnect; an
    // unresponsive-owner timeout here means the platform transport
    // misclassified a real disconnect again.
    if attempts.len() != 2
        || attempts[0].ordinal != 1
        || attempts[1].ordinal != 2
        || attempts[0].request_id == attempts[1].request_id
        || attempts[0].server_instance_id != old_server_instance_id
        || attempts[0].outcome != "server_loss"
        || attempts[0].loss_code.as_deref() != Some("embedding_server_connection_lost")
        || attempts[1].server_instance_id != new_server_instance_id
        || attempts[1].outcome != "completed"
        || attempts[1].loss_code.is_some()
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

/// Coordinator kill budget for one measurement worker. `wait_for_child` exists
/// only as a hung-worker watchdog, so its budget must strictly dominate the
/// worker's honest worst case, and that worst case is already defined by the
/// constant-set deadlines the worker enforces on itself: connect to or spawn
/// the server, then run one request bounded by its class deadline. The
/// snapshot and control terms cover the coordinator-visible bookkeeping around
/// the operation, matching `stall_worker_timeout`. Every term is
/// contract-derived; a flat snapshot-wait budget killed legitimately
/// progressing bulk workloads on the hosted 2-core calibration cell
/// (run 30197324641: warm_bulk_64x256b needs ~24s and the 256-document
/// throughput workload ~96s at the calibrated 2.658 docs/s, both inside the
/// worker's own bulk deadline).
pub(super) fn measurement_worker_timeout(operation: &str) -> Duration {
    let budgets = EmbeddingClientBudgets::current();
    if operation == "measure_true_idle" {
        // The idle worker first proves the resident owner quiescent (bounded
        // by the snapshot allowance), then waits out the server's own idle
        // deadline plus the exit grace before the absence observation.
        return Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
            .saturating_add(IDLE_EXIT_GRACE)
            .saturating_add(SNAPSHOT_TIMEOUT)
            .saturating_add(SNAPSHOT_TIMEOUT)
            .saturating_add(CONTROL_TIMEOUT);
    }
    let request_deadline = match operation {
        // Bulk frame measurements run one bulk exchange under the contract
        // bulk deadline; spawn-hello, product-query, and residency
        // measurements contain an `EnsureResident` exchange or a cold model
        // load, both bounded by the same bulk deadline the client enforces on
        // itself.
        "bulk"
        | "measure_bulk_frame"
        | "measure_spawn_hello"
        | "measure_product_query"
        | "measure_resident_identity" => budgets.bulk_request,
        _ => budgets.query_request,
    };
    budgets
        .connect
        .saturating_add(budgets.spawn)
        .saturating_add(request_deadline)
        .saturating_add(SNAPSHOT_TIMEOUT)
        .saturating_add(CONTROL_TIMEOUT)
}

/// Deadline the busy-retry worker's queued query threads enforce on
/// themselves (mirrors the worker's queue-operation deadline).
const BUSY_RETRY_QUEUE_REQUEST_DEADLINE_MS: u64 = 120_000;
/// Deadline the busy-retry worker's seed and replay exchanges enforce on
/// themselves (mirrors the worker's anti-idle protocol deadline).
const BUSY_RETRY_PROTOCOL_DEADLINE_MS: u64 = 90_000;

/// Coordinator kill budget for the single-process busy-retry measurement
/// worker. Seeding the held queues is a queue-setup phase; after the driver
/// releases the classes every queued query is bounded by the worker's own
/// per-request deadline and the replay by its protocol deadline, so the
/// watchdog budget is the sum of those self-enforced deadlines plus the
/// snapshot and control allowances around hold/release.
pub(super) fn busy_retry_worker_timeout() -> Duration {
    let budgets = EmbeddingClientBudgets::current();
    budgets
        .connect
        .saturating_add(QUEUE_SETUP_TIMEOUT)
        .saturating_add(Duration::from_millis(BUSY_RETRY_QUEUE_REQUEST_DEADLINE_MS))
        .saturating_add(Duration::from_millis(BUSY_RETRY_PROTOCOL_DEADLINE_MS))
        .saturating_add(SNAPSHOT_TIMEOUT)
        .saturating_add(CONTROL_TIMEOUT)
        .saturating_add(CONTROL_TIMEOUT)
}

/// Driver budget for the busy-retry worker's typed-retry marker: the worker
/// must first seed the held queues (a queue-setup phase) and validate the
/// typed capacity response before it can drop the marker.
pub(super) fn busy_retry_marker_timeout() -> Duration {
    QUEUE_SETUP_TIMEOUT
        .saturating_add(SNAPSHOT_TIMEOUT)
        .saturating_add(CONTROL_TIMEOUT)
}

/// Coordinator budget for observing the dead client's established load. The
/// `client_death_lease_active` wait must see the lease plus the admitted and
/// held query/bulk work in one snapshot, and that bounds a queue-seeding
/// phase, not a single snapshot: the freshly spawned dead client first pays
/// its contract connect and spawn-convergence allowances, then fans out one
/// captured transport per held request before the seeded depths become
/// visible, while every poll of the wait is itself a fresh observe worker
/// spending part of the snapshot allowance. This is the same phase shape as
/// mixed_queue's gated seeding, which already bounds a strictly larger 64+64
/// enqueue with the runner's queue-setup budget, so the dead-client wait
/// reuses that bound instead of inventing a second formula. Regression:
/// calibration run 30200986788 (protected macOS Metal cell) timed out at the
/// flat 20s snapshot wait while the dead client was still legitimately
/// ramping its 16+16 held requests under host load.
pub(super) fn dead_client_setup_timeout() -> Duration {
    QUEUE_SETUP_TIMEOUT
}
