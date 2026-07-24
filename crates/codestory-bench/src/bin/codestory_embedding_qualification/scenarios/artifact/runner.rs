use super::{
    ControlEvent, MeasurementArtifact, ProcessInvocation, RawMetric, RawMetricSample,
    ScenarioArtifact, ScenarioContext, ScenarioOrchestration,
};
use crate::qualification::request::{QualificationExecutable, sha256_bytes};
use anyhow::{Result, bail};
use codestory_retrieval::{
    EmbeddingCapacityPressureWire, EmbeddingProtocolResponse, EmbeddingQualificationParameters,
    EmbeddingQualificationResult, EmbeddingServerSnapshot, EmbeddingTransportIdentity,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

mod analysis;
mod client_death;
mod cold_race;
mod control;
mod evidence;
mod frozen_owner;
mod incompatible_owner;
mod measurements;
mod mixed_queue;
mod process;
mod server_crash;
mod true_idle_respawn;
mod worker_process;
mod worker_stall;

use analysis::project_identity_sha256;
use evidence::validate_named_evidence;
use process::{
    cleanup_worker_files, existing_control_events, qualification_command_path, qualification_nonce,
};

#[derive(Debug, Default)]
struct ScenarioEvidence {
    controls: BTreeSet<String>,
    transitions: BTreeSet<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRequest {
    schema_version: u32,
    nonce_sha256: String,
    executable_sha256: String,
    project: PathBuf,
    operation: String,
    parameters: EmbeddingQualificationParameters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_gate: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_gate_timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerOutput {
    schema_version: u32,
    pid: u32,
    process_start_id: String,
    executable_sha256: String,
    executable_version: String,
    project_identity_sha256: String,
    clock: codestory_retrieval::EmbeddingServerClockSnapshot,
    started_ns: u64,
    finished_ns: u64,
    inclusive_clock_api: String,
    inclusive_started_ns: u64,
    inclusive_finished_ns: u64,
    boot_id_started: String,
    boot_id_finished: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<EmbeddingQualificationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protocol_exchange: Option<WorkerProtocolExchange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_operations: Option<Vec<WorkerQueueOperation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    engine_identity: Option<codestory_retrieval::EmbeddingEngineIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<WorkerError>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerProtocolExchange {
    request_id: String,
    submitted_ns: u64,
    finished_ns: u64,
    transport_identity: EmbeddingTransportIdentity,
    hello_snapshot: EmbeddingServerSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    final_snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<EmbeddingProtocolResponse>,
    response_payload_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_transport_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerError {
    code: String,
    message_head: String,
    retry_class: String,
    retry_after_ms: u64,
    retry_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity: Option<EmbeddingCapacityPressureWire>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerQueueOperation {
    correlation_id: String,
    project_identity_sha256: String,
    class: String,
    ordinal: u32,
    #[serde(default)]
    submission_batch: u32,
    submitted_ns: u64,
    completed_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_completion_sequence: Option<u64>,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<codestory_retrieval::EmbeddingProtocolError>,
    response_payload_bytes: u64,
    transport_identity: EmbeddingTransportIdentity,
    hello_snapshot: EmbeddingServerSnapshot,
}

#[derive(Debug, Serialize)]
struct ControlCommand {
    schema_version: u32,
    sequence: u64,
    nonce_sha256: String,
    action: String,
    parameters: ControlCommandParameters,
}

#[derive(Debug, Serialize)]
struct ControlCommandParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    class: Option<String>,
}

struct RunningWorker {
    invocation_index: usize,
    child: Child,
    request_path: PathBuf,
    output_path: PathBuf,
}

impl Drop for RunningWorker {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        cleanup_worker_files(self);
    }
}

struct ScenarioRunner<'a> {
    context: ScenarioContext<'a>,
    executable: QualificationExecutable,
    clock: CoordinatorClock,
    artifact: ScenarioArtifact,
    evidence: ScenarioEvidence,
    active_controls: BTreeSet<String>,
    active_gates: BTreeSet<PathBuf>,
    next_sequence: u64,
    next_worker: u64,
}

struct CoordinatorClock {
    origin: Instant,
}

impl CoordinatorClock {
    fn capture() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    fn now_ns(&self) -> u64 {
        self.origin
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub(in crate::qualification) fn run_scenario(
    context: ScenarioContext<'_>,
) -> Result<ScenarioArtifact> {
    let mut runner = ScenarioRunner::new(context)?;
    let result = match runner.context.scenario {
        "client_death" => runner.client_death(),
        "cold_race" => runner.cold_race(),
        "frozen_owner" => runner.frozen_owner(),
        "incompatible_owner" => runner.incompatible_owner(),
        "mixed_queue" => runner.mixed_queue(),
        "server_crash" => runner.server_crash(),
        "true_idle_respawn" => runner.true_idle_respawn(),
        "worker_stall" => runner.worker_stall(),
        _ => Err(anyhow::anyhow!("embedding_qualification_scenario_unknown")),
    };
    if let Err(error) = result {
        runner.cleanup_after_failure();
        return Err(error);
    }
    runner.finish()
}

pub(in crate::qualification) fn run_measurements(
    context: ScenarioContext<'_>,
) -> Result<MeasurementArtifact> {
    let mut runner = ScenarioRunner::new(context)?;
    let mut result = runner.measurements();
    if result.is_ok() && !runner.active_controls.is_empty() {
        result = Err(anyhow::anyhow!(
            "embedding_qualification_controls_not_released"
        ));
    }
    if result.is_err() {
        runner.cleanup_after_failure();
    }
    result
}

fn push_metric(
    metrics: &mut BTreeMap<String, RawMetric>,
    metric: &str,
    unit: &str,
    sample: RawMetricSample,
) -> Result<()> {
    if let Some(existing) = metrics.get_mut(metric) {
        if existing.unit != unit {
            bail!("embedding_qualification_metric_unit_changed:{metric}");
        }
        existing.samples.push(sample);
        return Ok(());
    }
    metrics.insert(
        metric.into(),
        RawMetric {
            unit: unit.into(),
            samples: vec![sample],
        },
    );
    Ok(())
}

fn opaque_measurement_sample_id(
    nonce_sha256: &str,
    matrix_cell_id: &str,
    metric: &str,
    repeat: u32,
) -> String {
    sha256_bytes(
        format!(
            "codestory-embedding-measurement-sample-v1|{nonce_sha256}|{matrix_cell_id}|{metric}|{repeat}"
        )
        .as_bytes(),
    )
}

impl<'a> ScenarioRunner<'a> {
    fn new(context: ScenarioContext<'a>) -> Result<Self> {
        if context.runtimes.len() != 2
            || context.projects.len() != 2
            || context.primary_index >= 2
            || project_identity_sha256(&context.runtimes[0])
                == project_identity_sha256(&context.runtimes[1])
        {
            bail!("embedding_qualification_scenario_projects_invalid");
        }
        let clock = CoordinatorClock::capture();
        let started_ns = clock.now_ns();
        let next_sequence = existing_control_events(context.output_directory)?
            .iter()
            .fold(0, |maximum, event| maximum.max(event.sequence));
        Ok(Self {
            executable: context.executable.clone(),
            clock,
            artifact: ScenarioArtifact {
                schema_version: 3,
                scenario: context.scenario.into(),
                contracts: context.contracts.clone(),
                orchestration: ScenarioOrchestration {
                    started_ns,
                    finished_ns: 0,
                    process_invocations: Vec::new(),
                },
                control_events: Vec::new(),
                process_observations: Vec::new(),
                observations: Vec::new(),
                events: Vec::new(),
            },
            context,
            evidence: ScenarioEvidence::default(),
            active_controls: BTreeSet::new(),
            active_gates: BTreeSet::new(),
            next_sequence,
            next_worker: 0,
        })
    }

    fn finish(mut self) -> Result<ScenarioArtifact> {
        if !self.active_controls.is_empty() {
            self.cleanup_after_failure();
            bail!("embedding_qualification_controls_not_released");
        }
        if !self.active_gates.is_empty() {
            self.cleanup_after_failure();
            bail!("embedding_qualification_gates_not_cleaned");
        }
        if let Err(error) = validate_named_evidence(self.context.scenario, &self.evidence) {
            self.cleanup_after_failure();
            return Err(error);
        }
        self.artifact.orchestration.finished_ns = self.clock.now_ns();
        Ok(self.artifact)
    }

    fn cleanup_after_failure(&mut self) {
        if !self.active_controls.is_empty() {
            let _ = self.control("crash_server", None);
            self.active_controls.clear();
        }
        if let Ok(nonce) = qualification_nonce() {
            let _ = fs::remove_file(qualification_command_path(
                self.context.output_directory,
                &nonce,
            ));
        }
        for gate in std::mem::take(&mut self.active_gates) {
            let _ = fs::remove_file(gate);
        }
    }
}

#[cfg(test)]
mod tests;
