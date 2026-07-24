use crate::qualification::output::{
    QualificationMeasurementsSummary, QualificationScenarioSummary,
};
use crate::qualification::request::{
    QualificationContracts, QualificationExecutable, QualificationRuntime,
};
use codestory_retrieval::{EmbeddingServerSnapshot, SidecarRuntimeConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod runner;

pub(in crate::qualification) use runner::{run_measurements, run_scenario};

const WORKER_COMMAND: &str = "internal-embedding-qualification-worker";
const POLL: Duration = Duration::from_millis(25);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const QUEUE_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
const NORMAL_WORKER_TIMEOUT: Duration = Duration::from_secs(240);
const FROZEN_WORKER_TIMEOUT: Duration = Duration::from_secs(8);
const CLIENT_DEATH_LEASE_HOLD_MS: u64 = 600_000;
const DEAD_CLIENT_QUERY_COUNT: usize = 16;
const DEAD_CLIENT_BULK_COUNT: usize = 16;
const QUALIFICATION_QUEUE_CAPACITY: u64 = 64;
const MIXED_QUEUE_PROJECT_COUNT: u32 = (QUALIFICATION_QUEUE_CAPACITY / 2) as u32;
const MIXED_QUEUE_COUNT: u32 = QUALIFICATION_QUEUE_CAPACITY as u32 + 1;
const IDLE_EXIT_GRACE: Duration = Duration::from_millis(2_500);

pub(in crate::qualification) struct ScenarioContext<'a> {
    pub(in crate::qualification) scenario: &'a str,
    pub(in crate::qualification) runtimes: &'a [SidecarRuntimeConfig],
    pub(in crate::qualification) projects: &'a [PathBuf],
    pub(in crate::qualification) primary_index: usize,
    pub(in crate::qualification) contracts: &'a QualificationContracts,
    pub(in crate::qualification) qualification_runtime: &'a QualificationRuntime,
    pub(in crate::qualification) output_directory: &'a Path,
    pub(in crate::qualification) nonce_sha256: &'a str,
    pub(in crate::qualification) executable: &'a QualificationExecutable,
}

#[derive(Debug, Serialize)]
pub(in crate::qualification) struct ScenarioArtifact {
    schema_version: u32,
    scenario: String,
    contracts: QualificationContracts,
    orchestration: ScenarioOrchestration,
    control_events: Vec<ControlEvent>,
    process_observations: Vec<ProcessObservation>,
    observations: Vec<RawObservation>,
    events: Vec<RawEvent>,
}

impl ScenarioArtifact {
    pub(in crate::qualification) fn summary(
        &self,
        artifact: String,
    ) -> QualificationScenarioSummary {
        QualificationScenarioSummary {
            artifact,
            process_count: self.orchestration.process_invocations.len() as u64,
            control_event_count: self.control_events.len() as u64,
            process_observation_count: self.process_observations.len() as u64,
            observation_count: self.observations.len() as u64,
            event_count: self.events.len() as u64,
        }
    }
}

#[derive(Debug, Serialize)]
struct ScenarioOrchestration {
    started_ns: u64,
    finished_ns: u64,
    process_invocations: Vec<ProcessInvocation>,
}

#[derive(Debug, Serialize)]
struct ProcessInvocation {
    invocation_id: String,
    operation: String,
    project_identity_sha256: String,
    pid: u32,
    process_start_id: String,
    started_ns: u64,
    finished_ns: Option<u64>,
    exit_code: Option<i32>,
    termination: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlEventClock {
    domain: String,
    api: String,
    boot_id: String,
    observed_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlEvent {
    schema_version: u32,
    sequence: u64,
    action: String,
    status: String,
    #[serde(default)]
    authenticated_nonce_sha256: String,
    server_event_sequence: u64,
    clock: ControlEventClock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Serialize)]
struct ProcessObservation {
    phase: String,
    observed_ns: u64,
    server_instance_id: Option<String>,
    pid: Option<u32>,
    process_start_id: Option<String>,
    executable_sha256: Option<String>,
    executable_version: Option<String>,
    endpoint_namespace_id: Option<String>,
    lifetime_authority_id: Option<String>,
    listener_id: Option<String>,
    protocol_sha256: Option<String>,
    constant_set_sha256: Option<String>,
    measurement_protocol_sha256: Option<String>,
    load_generation: Option<u64>,
    snapshot: Option<EmbeddingServerSnapshot>,
}

impl ProcessObservation {
    fn from_snapshot(
        phase: &str,
        observed_ns: u64,
        snapshot: Option<EmbeddingServerSnapshot>,
    ) -> Self {
        let process = snapshot.as_ref().map(|snapshot| &snapshot.process);
        let authority = snapshot.as_ref().map(|snapshot| &snapshot.authority);
        let protocol = snapshot.as_ref().map(|snapshot| &snapshot.protocol);
        let load_generation = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.engine.as_ref())
            .map(|engine| engine.load_generation);
        Self {
            phase: phase.into(),
            observed_ns,
            server_instance_id: process.map(|process| process.server_instance_id.clone()),
            pid: process.map(|process| process.pid),
            process_start_id: process.map(|process| process.process_start_id.clone()),
            executable_sha256: process.map(|process| process.executable_sha256.clone()),
            executable_version: process.map(|process| process.executable_version.clone()),
            endpoint_namespace_id: authority
                .map(|authority| authority.endpoint_namespace_id.clone()),
            lifetime_authority_id: authority
                .map(|authority| authority.lifetime_authority_id.clone()),
            listener_id: authority.map(|authority| authority.listener_id.clone()),
            protocol_sha256: protocol.map(|protocol| protocol.protocol_sha256.clone()),
            constant_set_sha256: protocol.map(|protocol| protocol.constant_set_sha256.clone()),
            measurement_protocol_sha256: protocol
                .map(|protocol| protocol.measurement_protocol_sha256.clone()),
            load_generation,
            snapshot,
        }
    }
}

#[derive(Debug, Serialize)]
struct RawObservation {
    sequence: u64,
    kind: String,
    observed_ns: u64,
    values: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct RawEvent {
    sequence: u64,
    source: String,
    action: String,
    observed_ns: u64,
    correlation_id: Option<String>,
    values: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
pub(in crate::qualification) struct MeasurementArtifact {
    schema_version: u32,
    contracts: QualificationContracts,
    external_metrics: Vec<String>,
    metrics: BTreeMap<String, RawMetric>,
}

impl MeasurementArtifact {
    pub(in crate::qualification) fn summary(
        &self,
        artifact: String,
    ) -> QualificationMeasurementsSummary {
        QualificationMeasurementsSummary {
            artifact,
            metric_count: self.metrics.len() as u64,
            sample_count: self
                .metrics
                .values()
                .map(|metric| metric.samples.len() as u64)
                .sum(),
        }
    }
}

#[derive(Debug, Serialize)]
struct RawMetric {
    unit: String,
    samples: Vec<RawMetricSample>,
}

#[derive(Debug, Clone, Serialize)]
struct RawMetricSample {
    sample_id: String,
    repeat: u32,
    matrix_cell_id: String,
    workload_id: String,
    cache_state: String,
    residency_state: String,
    process: RawMetricProcess,
    server_identity: RawServerIdentity,
    clock: RawMetricClock,
    start: RawMetricPhase,
    end: RawMetricPhase,
    operands: BTreeMap<String, Value>,
    suspend_witness: SuspendWitness,
}

#[derive(Debug, Clone, Serialize)]
struct RawMetricProcess {
    pid: u32,
    process_start_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct RawServerIdentity {
    server_instance_id: String,
    process_start_id: String,
    load_generation: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RawMetricClock {
    domain: String,
    api: String,
    boot_id: String,
    resolution_ns: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RawMetricPhase {
    phase: String,
    observed_ns: u64,
}

#[derive(Debug, Clone, Serialize)]
struct SuspendWitness {
    awake_started_ns: u64,
    awake_finished_ns: u64,
    inclusive_clock_api: String,
    inclusive_started_ns: u64,
    inclusive_finished_ns: u64,
    boot_id_started: String,
    boot_id_finished: String,
}

#[derive(Clone)]
struct MeasurementInterval {
    process: RawMetricProcess,
    clock: RawMetricClock,
    awake_started_ns: u64,
    awake_finished_ns: u64,
    inclusive_clock_api: String,
    inclusive_started_ns: u64,
    inclusive_finished_ns: u64,
    boot_id_started: String,
    boot_id_finished: String,
}

struct RawMetricSampleInput<'a> {
    sample_id: &'a str,
    repeat: u32,
    runtime: &'a QualificationRuntime,
    workload_id: &'a str,
    server_identity: RawServerIdentity,
    start_phase: &'a str,
    end_phase: &'a str,
    operands: BTreeMap<String, Value>,
}

impl MeasurementInterval {
    fn sample(&self, input: RawMetricSampleInput<'_>) -> RawMetricSample {
        RawMetricSample {
            sample_id: input.sample_id.into(),
            repeat: input.repeat,
            matrix_cell_id: input.runtime.matrix_cell_id.clone(),
            workload_id: input.workload_id.into(),
            cache_state: input.runtime.cache_state.clone(),
            residency_state: input.runtime.residency_state.clone(),
            process: self.process.clone(),
            server_identity: input.server_identity,
            clock: self.clock.clone(),
            start: RawMetricPhase {
                phase: input.start_phase.into(),
                observed_ns: self.awake_started_ns,
            },
            end: RawMetricPhase {
                phase: input.end_phase.into(),
                observed_ns: self.awake_finished_ns,
            },
            operands: input.operands,
            suspend_witness: SuspendWitness {
                awake_started_ns: self.awake_started_ns,
                awake_finished_ns: self.awake_finished_ns,
                inclusive_clock_api: self.inclusive_clock_api.clone(),
                inclusive_started_ns: self.inclusive_started_ns,
                inclusive_finished_ns: self.inclusive_finished_ns,
                boot_id_started: self.boot_id_started.clone(),
                boot_id_finished: self.boot_id_finished.clone(),
            },
        }
    }
}

fn successful_operation_duration_ns(interval: &MeasurementInterval) -> u64 {
    interval
        .awake_finished_ns
        .saturating_sub(interval.awake_started_ns)
}

fn successful_operation_operands(interval: &MeasurementInterval) -> BTreeMap<String, Value> {
    btree([(
        "successful_operation_duration_ns",
        json!(successful_operation_duration_ns(interval)),
    )])
}

fn btree<const N: usize>(entries: [(&str, Value); N]) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect()
}
