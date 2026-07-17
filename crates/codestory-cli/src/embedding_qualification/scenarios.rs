use super::*;
use codestory_retrieval::{
    AwakeMonotonicClock, EmbeddingClientTransport, EmbeddingCompatibility, EmbeddingEngineIdentity,
    EmbeddingOperation, EmbeddingProtocolRequest, EmbeddingProtocolResponse,
    EmbeddingQualificationParameters, EmbeddingQualificationRequest, EmbeddingQualificationResult,
    EmbeddingResult, EmbeddingServerSnapshot, EmbeddingServerStream, EmbeddingTransportIdentity,
    PER_USER_EMBEDDING_MAX_METADATA_BYTES, PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES,
    PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION, PER_USER_EMBEDDING_PROTOCOL_V1,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS, PerUserEmbeddingClient, PerUserEmbeddingError,
    ProcessStartProbe,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

const WORKER_COMMAND: &str = "internal-embedding-qualification-worker";
const POLL: Duration = Duration::from_millis(25);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const QUEUE_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
const NORMAL_WORKER_TIMEOUT: Duration = Duration::from_secs(240);
const FROZEN_WORKER_TIMEOUT: Duration = Duration::from_secs(8);
const STALL_WORKER_TIMEOUT: Duration = Duration::from_secs(325);
const STALL_PROTOCOL_DEADLINE_MS: u64 = 315_000;
const ANTI_IDLE_PROTOCOL_DEADLINE_MS: u64 = 90_000;
const CLIENT_DEATH_LEASE_HOLD_MS: u64 = 600_000;
const DEAD_CLIENT_QUERY_COUNT: usize = 16;
const DEAD_CLIENT_BULK_COUNT: usize = 16;
const MIXED_QUEUE_COUNT: u32 = 65;
const IDLE_EXIT_GRACE: Duration = Duration::from_millis(2_500);

pub(super) struct ScenarioContext<'a> {
    pub(super) scenario: &'a str,
    pub(super) runtimes: &'a [SidecarRuntimeConfig],
    pub(super) projects: &'a [PathBuf],
    pub(super) primary_index: usize,
    pub(super) contracts: &'a QualificationContracts,
    pub(super) qualification_runtime: &'a QualificationRuntime,
    pub(super) output_directory: &'a Path,
    pub(super) nonce_sha256: &'a str,
}

#[derive(Debug, Serialize)]
pub(super) struct ScenarioArtifact {
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
    pub(super) fn summary(&self, artifact: String) -> QualificationScenarioSummary {
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
pub(super) struct MeasurementArtifact {
    schema_version: u32,
    contracts: QualificationContracts,
    external_metrics: Vec<String>,
    metrics: BTreeMap<String, RawMetric>,
}

impl MeasurementArtifact {
    pub(super) fn summary(&self, artifact: String) -> QualificationMeasurementsSummary {
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

struct MeasurementSpanStart {
    process: RawMetricProcess,
    clock: RawMetricClock,
    awake_started_ns: u64,
    inclusive_started_ns: u64,
    boot_id_started: String,
}

#[derive(Clone)]
struct MeasurementInterval {
    process: RawMetricProcess,
    clock: RawMetricClock,
    awake_started_ns: u64,
    awake_finished_ns: u64,
    inclusive_started_ns: u64,
    inclusive_finished_ns: u64,
    boot_id_started: String,
    boot_id_finished: String,
}

impl MeasurementInterval {
    fn sample(
        &self,
        sample_id: &str,
        repeat: u32,
        runtime: &QualificationRuntime,
        workload_id: &str,
        server_identity: RawServerIdentity,
        start_phase: &str,
        end_phase: &str,
        operands: BTreeMap<String, Value>,
    ) -> RawMetricSample {
        RawMetricSample {
            sample_id: sample_id.into(),
            repeat,
            matrix_cell_id: runtime.matrix_cell_id.clone(),
            workload_id: workload_id.into(),
            cache_state: runtime.cache_state.clone(),
            residency_state: runtime.residency_state.clone(),
            process: self.process.clone(),
            server_identity,
            clock: self.clock.clone(),
            start: RawMetricPhase {
                phase: start_phase.into(),
                observed_ns: self.awake_started_ns,
            },
            end: RawMetricPhase {
                phase: end_phase.into(),
                observed_ns: self.awake_finished_ns,
            },
            operands,
            suspend_witness: SuspendWitness {
                awake_started_ns: self.awake_started_ns,
                awake_finished_ns: self.awake_finished_ns,
                inclusive_clock_api: crate::embedding_server_transport::inclusive_clock_api()
                    .into(),
                inclusive_started_ns: self.inclusive_started_ns,
                inclusive_finished_ns: self.inclusive_finished_ns,
                boot_id_started: self.boot_id_started.clone(),
                boot_id_finished: self.boot_id_finished.clone(),
            },
        }
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<EmbeddingQualificationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protocol_exchange: Option<WorkerProtocolExchange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_operations: Option<Vec<WorkerQueueOperation>>,
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
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerQueueOperation {
    correlation_id: String,
    project_identity_sha256: String,
    class: String,
    ordinal: u32,
    submitted_ns: u64,
    completed_ns: u64,
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

struct ScenarioRunner<'a> {
    context: ScenarioContext<'a>,
    executable: crate::embedding_server_transport::ExactExecutable,
    clock: Arc<dyn AwakeMonotonicClock>,
    artifact: ScenarioArtifact,
    evidence: ScenarioEvidence,
    next_sequence: u64,
    next_worker: u64,
}

pub(super) fn run_scenario(context: ScenarioContext<'_>) -> Result<ScenarioArtifact> {
    let mut runner = ScenarioRunner::new(context)?;
    match runner.context.scenario {
        "client_death" => runner.client_death()?,
        "cold_race" => runner.cold_race()?,
        "frozen_owner" => runner.frozen_owner()?,
        "incompatible_owner" => runner.incompatible_owner()?,
        "mixed_queue" => runner.mixed_queue()?,
        "server_crash" => runner.server_crash()?,
        "true_idle_respawn" => runner.true_idle_respawn()?,
        "worker_stall" => runner.worker_stall()?,
        _ => bail!("embedding_qualification_scenario_unknown"),
    }
    runner.finish()
}

pub(super) fn run_measurements(context: ScenarioContext<'_>) -> Result<MeasurementArtifact> {
    let mut runner = ScenarioRunner::new(context)?;
    runner.measurements()
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
        let transport =
            crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
        let clock = EmbeddingClientTransport::clock(&transport);
        let started_ns = clock.now_ns();
        let next_sequence = existing_control_events(context.output_directory)?
            .iter()
            .fold(0, |maximum, event| maximum.max(event.sequence));
        Ok(Self {
            executable: crate::embedding_server_transport::ExactExecutable::capture()?,
            clock,
            artifact: ScenarioArtifact {
                schema_version: 2,
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
            next_sequence,
            next_worker: 0,
        })
    }

    fn finish(mut self) -> Result<ScenarioArtifact> {
        validate_named_evidence(self.context.scenario, &self.evidence)?;
        self.artifact.orchestration.finished_ns = self.clock.now_ns();
        Ok(self.artifact)
    }

    fn begin_measurement(&self) -> Result<MeasurementSpanStart> {
        let process_start_id = current_process_start_identity()?;
        let snapshot = self.clock.snapshot();
        let boot_id_started = crate::embedding_server_transport::boot_id()?;
        if snapshot.domain != "awake_monotonic"
            || snapshot.boot_id != boot_id_started
            || snapshot.api.is_empty()
            || snapshot.resolution_ns == 0
        {
            bail!("embedding_qualification_measurement_clock_invalid");
        }
        let inclusive_started_ns = crate::embedding_server_transport::inclusive_now_ns()?;
        let awake_started_ns = self.clock.now_ns();
        Ok(MeasurementSpanStart {
            process: RawMetricProcess {
                pid: std::process::id(),
                process_start_id,
            },
            clock: RawMetricClock {
                domain: snapshot.domain,
                api: snapshot.api,
                boot_id: snapshot.boot_id,
                resolution_ns: snapshot.resolution_ns,
            },
            awake_started_ns,
            inclusive_started_ns,
            boot_id_started,
        })
    }

    fn finish_measurement(&self, start: MeasurementSpanStart) -> Result<MeasurementInterval> {
        let awake_finished_ns = self.clock.now_ns();
        let inclusive_finished_ns = crate::embedding_server_transport::inclusive_now_ns()?;
        let boot_id_finished = crate::embedding_server_transport::boot_id()?;
        if start.awake_started_ns > awake_finished_ns
            || start.inclusive_started_ns > inclusive_finished_ns
            || start.boot_id_started != start.clock.boot_id
            || boot_id_finished != start.clock.boot_id
        {
            bail!("embedding_qualification_measurement_clock_changed");
        }
        Ok(MeasurementInterval {
            process: start.process,
            clock: start.clock,
            awake_started_ns: start.awake_started_ns,
            awake_finished_ns,
            inclusive_started_ns: start.inclusive_started_ns,
            inclusive_finished_ns,
            boot_id_started: start.boot_id_started,
            boot_id_finished,
        })
    }

    fn measurements(&mut self) -> Result<MeasurementArtifact> {
        let mut metrics = BTreeMap::new();
        let runtime = self.primary_runtime().clone();
        let transport =
            crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;

        for repeat in 1..=3 {
            self.reset_owner(&format!("measure_spawn_no_owner_{repeat}"))?;
            let spawn_start = self.begin_measurement()?;
            transport.spawn_exact_current_exe()?;
            let mut spawn_stream =
                connect_until(&transport, self.clock.as_ref(), Duration::from_secs(15))?;
            let spawn_hello = validated_hello(&mut spawn_stream, &runtime, self.clock.as_ref())?;
            let spawn = self.finish_measurement(spawn_start)?;
            let _ = PerUserEmbeddingClient::for_runtime(&runtime)?.ensure_resident()?;
            let spawn_resident =
                self.wait_for_snapshot("measure_spawn_resident", SNAPSHOT_TIMEOUT, |snapshot| {
                    snapshot.engine.is_some()
                })?;
            if spawn_resident.process.server_instance_id != spawn_hello.process.server_instance_id {
                bail!("embedding_qualification_spawn_owner_changed");
            }
            push_metric(
                &mut metrics,
                "spawn_convergence",
                "milliseconds",
                spawn.sample(
                    &format!("spawn-convergence-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "compatible_hello_absent_owner_v1",
                    raw_server_identity(&spawn_resident)?,
                    "owner_absence_proven",
                    "compatible_hello_validated",
                    BTreeMap::new(),
                ),
            )?;
        }

        for repeat in 1..=3 {
            let connect_start = self.begin_measurement()?;
            let mut existing_stream =
                connect_until(&transport, self.clock.as_ref(), Duration::from_secs(2))?;
            let existing = validated_hello(&mut existing_stream, &runtime, self.clock.as_ref())?;
            let connect = self.finish_measurement(connect_start)?;
            push_metric(
                &mut metrics,
                "existing_owner_connect",
                "milliseconds",
                connect.sample(
                    &format!("existing-owner-connect-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "compatible_hello_existing_owner_v1",
                    raw_server_identity(&existing)?,
                    "client_connect_started",
                    "compatible_hello_validated",
                    BTreeMap::new(),
                ),
            )?;
        }

        for repeat in 1..=3 {
            self.reset_owner(&format!("measure_cold_no_owner_{repeat}"))?;
            let cold_start = self.begin_measurement()?;
            let cold_vector = PerUserEmbeddingClient::for_runtime(&runtime)?
                .embed_query(&workload_input("cold_query_256b_v1", repeat, 0, 256))?;
            validate_product_vector(&cold_vector, "cold")?;
            let cold = self.finish_measurement(cold_start)?;
            let cold_snapshot = PerUserEmbeddingClient::for_runtime(&runtime)?
                .observe()?
                .ok_or_else(|| anyhow::anyhow!("embedding_qualification_cold_owner_missing"))?;
            push_metric(
                &mut metrics,
                "cold_first_vector",
                "milliseconds",
                cold.sample(
                    &format!("cold-first-vector-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "cold_query_256b_v1",
                    raw_server_identity(&cold_snapshot)?,
                    "product_request_started_with_owner_absent",
                    "first_vector_and_engine_evidence_validated",
                    BTreeMap::new(),
                ),
            )?;
        }

        for repeat in 1..=3 {
            let ready_start = self.begin_measurement()?;
            let vector = PerUserEmbeddingClient::for_runtime(&runtime)?
                .embed_query(&workload_input("product_query_256b_v1", repeat, 0, 256))?;
            validate_product_vector(&vector, "product_ready")?;
            let ready = self.finish_measurement(ready_start)?;
            let ready_snapshot = PerUserEmbeddingClient::for_runtime(&runtime)?
                .observe()?
                .ok_or_else(|| anyhow::anyhow!("embedding_qualification_ready_owner_missing"))?;
            push_metric(
                &mut metrics,
                "first_product_ready",
                "milliseconds",
                ready.sample(
                    &format!("first-product-ready-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "product_query_256b_v1",
                    raw_server_identity(&ready_snapshot)?,
                    "product_request_started",
                    "product_result_validated",
                    BTreeMap::new(),
                ),
            )?;
        }

        for repeat in 1..=3 {
            let query = measure_vector_operation(
                &transport,
                &runtime,
                "query",
                vec![workload_input("warm_query_256b_v1", repeat, 0, 256)],
                self,
            )?;
            push_metric(
                &mut metrics,
                "warm_query_ipc",
                "milliseconds",
                query.interval.sample(
                    &format!("warm-query-rpc-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "warm_query_256b_v1",
                    query.server_identity,
                    "client_frame_started",
                    "query_response_identity_and_vector_validated",
                    BTreeMap::new(),
                ),
            )?;
        }

        for repeat in 1..=3 {
            let documents = workload_documents("warm_bulk_64x256b_v1", repeat, 64, 256);
            let bulk = measure_vector_operation(&transport, &runtime, "bulk", documents, self)?;
            push_metric(
                &mut metrics,
                "warm_bulk_ipc",
                "milliseconds",
                bulk.interval.sample(
                    &format!("warm-bulk-rpc-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "warm_bulk_64x256b_v1",
                    bulk.server_identity,
                    "client_frame_started",
                    "bulk_response_identity_and_vectors_validated",
                    BTreeMap::new(),
                ),
            )?;
        }

        for repeat in 1..=3 {
            let documents = workload_documents("bulk_throughput_256x256b_v1", repeat, 256, 256);
            let bulk = measure_vector_operation(&transport, &runtime, "bulk", documents, self)?;
            let completed_tokens =
                completed_token_count(self.context.output_directory, &bulk.request_id)?;
            push_metric(
                &mut metrics,
                "bulk_documents_per_second",
                "documents_per_second",
                bulk.interval.sample(
                    &format!("bulk-documents-throughput-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "bulk_throughput_256x256b_v1",
                    bulk.server_identity.clone(),
                    "bulk_measurement_window_started",
                    "bulk_document_results_validated",
                    btree([("completed_documents", json!(bulk.completed_documents))]),
                ),
            )?;
            push_metric(
                &mut metrics,
                "bulk_tokens_per_second",
                "tokens_per_second",
                bulk.interval.sample(
                    &format!("bulk-tokens-throughput-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "bulk_throughput_256x256b_v1",
                    bulk.server_identity,
                    "bulk_measurement_window_started",
                    "bulk_token_results_validated",
                    btree([("completed_tokens", json!(completed_tokens))]),
                ),
            )?;
        }

        let residency_start = self.begin_measurement()?;
        let residency = PerUserEmbeddingClient::for_runtime(&runtime)?.ensure_resident()?;
        let residency_interval = self.finish_measurement(residency_start)?;
        let residency_snapshot = PerUserEmbeddingClient::for_runtime(&runtime)?
            .observe()?
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_residency_owner_missing"))?;
        push_metric(
            &mut metrics,
            "backend_observed_accelerator_residency",
            "boolean",
            residency_interval.sample(
                "backend-accelerator-residency-1",
                1,
                self.context.qualification_runtime,
                "resident_policy_identity_v1",
                raw_server_identity(&residency_snapshot)?,
                "accelerator_measurement_started",
                "backend_residency_evidence_validated",
                accelerator_operands(&residency),
            ),
        )?;

        for repeat in 1..=3 {
            let busy = self.measure_busy_retry(&transport, &runtime, repeat)?;
            let busy_snapshot = PerUserEmbeddingClient::for_runtime(&runtime)?
                .observe()?
                .ok_or_else(|| anyhow::anyhow!("embedding_qualification_busy_owner_missing"))?;
            push_metric(
                &mut metrics,
                "busy_retry_usefulness",
                "milliseconds",
                busy.sample(
                    &format!("busy-retry-usefulness-{repeat}"),
                    repeat,
                    self.context.qualification_runtime,
                    "saturated_query_65th_retry_v1",
                    raw_server_identity(&busy_snapshot)?,
                    "typed_retry_emitted",
                    "named_retry_condition_became_true",
                    BTreeMap::new(),
                ),
            )?;
        }

        let idle_client = PerUserEmbeddingClient::for_runtime(&runtime)?;
        let idle_vector =
            idle_client.embed_query(&workload_input("true_idle_60000_awake_ms_v1", 1, 0, 256))?;
        validate_product_vector(&idle_vector, "true_idle")?;
        let idle_owner = self.wait_for_snapshot(
            "measurement_true_idle_ready",
            SNAPSHOT_TIMEOUT,
            |snapshot| {
                snapshot.scheduler.active_request_count == 0
                    && snapshot.scheduler.query_depth == 0
                    && snapshot.scheduler.bulk_depth == 0
                    && snapshot.scheduler.lease_count == 0
            },
        )?;
        let idle_start = self.begin_measurement()?;
        self.wait_for_absence(
            "measurement_true_idle_absent",
            Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
                .saturating_add(IDLE_EXIT_GRACE),
        )?;
        let idle = self.finish_measurement(idle_start)?;
        push_metric(
            &mut metrics,
            "true_idle_exit",
            "milliseconds",
            idle.sample(
                "true-idle-exit-1",
                1,
                self.context.qualification_runtime,
                "true_idle_60000_awake_ms_v1",
                raw_server_identity(&idle_owner)?,
                "last_queued_active_or_leased_work_ended",
                "engine_and_server_absent",
                BTreeMap::new(),
            ),
        )?;

        if metrics.len() != REQUIRED_METRICS.len().saturating_sub(2)
            || metrics.contains_key("retrieval_quality")
            || metrics.contains_key("total_codestory_process_memory")
            || metrics.iter().any(|(name, metric)| {
                let expected = if matches!(
                    name.as_str(),
                    "true_idle_exit" | "backend_observed_accelerator_residency"
                ) {
                    1
                } else {
                    3
                };
                metric.samples.len() != expected
            })
        {
            bail!("embedding_qualification_measurement_set_incomplete");
        }
        Ok(MeasurementArtifact {
            schema_version: 2,
            contracts: self.context.contracts.clone(),
            external_metrics: vec![
                "retrieval_quality".into(),
                "total_codestory_process_memory".into(),
            ],
            metrics,
        })
    }

    fn measure_busy_retry(
        &mut self,
        _transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
        runtime: &SidecarRuntimeConfig,
        repeat: u32,
    ) -> Result<MeasurementInterval> {
        self.ensure_owner("measurement_busy_owner")?;
        self.control("hold_class", Some("bulk"))?;
        self.control("hold_class", Some("query"))?;
        let seed_runtime = runtime.clone();
        let seed_clock = Arc::clone(&self.clock);
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
        self.wait_for_snapshot(
            "measurement_busy_seed_active",
            SNAPSHOT_TIMEOUT,
            |snapshot| {
                snapshot
                    .scheduler
                    .active_request
                    .as_ref()
                    .is_some_and(|active| active.class == "bulk")
            },
        )?;

        let mut queued = Vec::new();
        for ordinal in 0..64 {
            let runtime = runtime.clone();
            let clock = Arc::clone(&self.clock);
            let input = workload_input("saturated_query_65th_retry_v1", repeat, ordinal + 1, 256);
            queued.push(
                std::thread::Builder::new()
                    .name(format!("codestory-measurement-busy-query-{ordinal}"))
                    .spawn(move || {
                        run_raw_protocol_exchange_with_input(
                            &runtime,
                            clock.as_ref(),
                            "query",
                            ANTI_IDLE_PROTOCOL_DEADLINE_MS,
                            Some(input),
                        )
                    })?,
            );
        }
        let saturated = self.wait_for_snapshot(
            "measurement_busy_saturated",
            QUEUE_SETUP_TIMEOUT,
            |snapshot| snapshot.scheduler.query_depth == snapshot.scheduler.query_capacity,
        )?;
        let overflow = run_raw_protocol_exchange_with_input(
            runtime,
            self.clock.as_ref(),
            "query",
            ANTI_IDLE_PROTOCOL_DEADLINE_MS,
            Some(workload_input(
                "saturated_query_65th_retry_v1",
                repeat,
                65,
                256,
            )),
        )?;
        let pressure = overflow
            .response
            .as_ref()
            .and_then(|response| response.error.as_ref())
            .and_then(|error| error.capacity.as_ref())
            .ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_busy_retry_pressure_missing")
            })?;
        if overflow.terminal_transport_error.is_some()
            || pressure.reason != "queue_full"
            || pressure.queue_class != "query"
            || pressure.capacity != saturated.scheduler.query_capacity
            || pressure.depth != pressure.capacity
            || pressure.retry_condition.trim().is_empty()
        {
            bail!("embedding_qualification_busy_retry_pressure_invalid");
        }
        let start = self.begin_measurement()?;
        self.control("release_class", Some("query"))?;
        self.control("release_class", Some("bulk"))?;
        self.wait_for_snapshot(
            "measurement_busy_retry_condition_true",
            SNAPSHOT_TIMEOUT,
            |snapshot| snapshot.scheduler.query_depth < pressure.capacity,
        )?;
        let interval = self.finish_measurement(start)?;

        let seed = seed
            .join()
            .map_err(|_| anyhow::anyhow!("embedding_qualification_busy_seed_panicked"))??;
        require_protocol_exchange_success(&seed, "busy_retry_seed")?;
        for worker in queued {
            let exchange = worker
                .join()
                .map_err(|_| anyhow::anyhow!("embedding_qualification_busy_query_panicked"))??;
            require_protocol_exchange_success(&exchange, "busy_retry_query")?;
        }
        self.wait_for_snapshot(
            "measurement_busy_drained",
            QUEUE_SETUP_TIMEOUT,
            |snapshot| {
                snapshot.scheduler.active_request_count == 0
                    && snapshot.scheduler.query_depth == 0
                    && snapshot.scheduler.bulk_depth == 0
            },
        )?;
        Ok(interval)
    }

    fn event(
        &mut self,
        source: &str,
        action: &str,
        correlation_id: Option<String>,
        values: BTreeMap<String, Value>,
    ) {
        self.artifact.events.push(RawEvent {
            sequence: self.artifact.events.len() as u64,
            source: source.into(),
            action: action.into(),
            observed_ns: self.clock.now_ns(),
            correlation_id,
            values,
        });
    }

    fn observation(&mut self, kind: &str, values: BTreeMap<String, Value>) {
        self.artifact.observations.push(RawObservation {
            sequence: self.artifact.observations.len() as u64,
            kind: kind.into(),
            observed_ns: self.clock.now_ns(),
            values,
        });
    }

    fn observe(&mut self, phase: &str) -> Result<Option<EmbeddingServerSnapshot>> {
        let snapshot = PerUserEmbeddingClient::for_runtime(self.primary_runtime())?.observe()?;
        self.artifact
            .process_observations
            .push(ProcessObservation::from_snapshot(
                phase,
                self.clock.now_ns(),
                snapshot.clone(),
            ));
        Ok(snapshot)
    }

    fn wait_for_snapshot(
        &mut self,
        phase: &str,
        timeout: Duration,
        predicate: impl Fn(&EmbeddingServerSnapshot) -> bool,
    ) -> Result<EmbeddingServerSnapshot> {
        let started = self.clock.now_ns();
        loop {
            if let Some(snapshot) =
                PerUserEmbeddingClient::for_runtime(self.primary_runtime())?.observe()?
                && predicate(&snapshot)
            {
                self.artifact
                    .process_observations
                    .push(ProcessObservation::from_snapshot(
                        phase,
                        self.clock.now_ns(),
                        Some(snapshot.clone()),
                    ));
                return Ok(snapshot);
            }
            if elapsed(self.clock.as_ref(), started) >= timeout {
                bail!("embedding_qualification_snapshot_timeout:{phase}");
            }
            self.clock.sleep(POLL);
        }
    }

    fn wait_for_absence(&mut self, phase: &str, timeout: Duration) -> Result<()> {
        let started = self.clock.now_ns();
        loop {
            if let Ok(None) = PerUserEmbeddingClient::for_runtime(self.primary_runtime())?.observe()
            {
                self.artifact
                    .process_observations
                    .push(ProcessObservation::from_snapshot(
                        phase,
                        self.clock.now_ns(),
                        None,
                    ));
                return Ok(());
            }
            if elapsed(self.clock.as_ref(), started) >= timeout {
                bail!("embedding_qualification_owner_exit_timeout:{phase}");
            }
            self.clock.sleep(POLL);
        }
    }

    fn ensure_owner(&mut self, phase: &str) -> Result<EmbeddingServerSnapshot> {
        if let Some(snapshot) = self.observe(&format!("{phase}_existing"))? {
            return Ok(snapshot);
        }
        let worker = self.spawn_worker("query", query_parameters(1), None)?;
        let output = self.finish_worker(worker, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&output, "ensure_owner")?;
        self.wait_for_snapshot(phase, SNAPSHOT_TIMEOUT, |_| true)
    }

    fn reset_owner(&mut self, phase: &str) -> Result<()> {
        if self.observe(&format!("{phase}_before"))?.is_some() {
            self.control("crash_server", None)?;
            self.wait_for_absence(phase, SNAPSHOT_TIMEOUT)?;
        }
        Ok(())
    }

    fn control(&mut self, action: &str, class: Option<&str>) -> Result<ControlEvent> {
        let command_path =
            qualification_command_path(self.context.output_directory, &qualification_nonce()?);
        let wait_started = self.clock.now_ns();
        while command_path.exists() {
            if elapsed(self.clock.as_ref(), wait_started) >= CONTROL_TIMEOUT {
                bail!("embedding_qualification_control_slot_busy");
            }
            self.clock.sleep(POLL);
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        let command = ControlCommand {
            schema_version: 1,
            sequence: self.next_sequence,
            nonce_sha256: self.context.nonce_sha256.into(),
            action: action.into(),
            parameters: ControlCommandParameters {
                class: class.map(str::to_owned),
            },
        };
        write_atomic_json(&command_path, &command)?;
        let started = self.clock.now_ns();
        let mut event = loop {
            if let Some(event) = existing_control_events(self.context.output_directory)?
                .into_iter()
                .find(|event| event.sequence == self.next_sequence)
            {
                break event;
            }
            if elapsed(self.clock.as_ref(), started) >= CONTROL_TIMEOUT {
                bail!("embedding_qualification_control_event_timeout:{action}");
            }
            self.clock.sleep(POLL);
        };
        if event.action != action
            || !matches!(event.status.as_str(), "completed" | "accepted")
            || (action == "crash_server" && event.status != "accepted")
        {
            bail!("embedding_qualification_control_event_invalid:{action}");
        }
        event.authenticated_nonce_sha256 = self.context.nonce_sha256.into();
        self.evidence.controls.insert(control_key(action, class));
        self.event(
            "server_control",
            action,
            Some(event.sequence.to_string()),
            btree([("status", json!(event.status))]),
        );
        self.artifact.control_events.push(event.clone());
        Ok(event)
    }

    fn spawn_worker(
        &mut self,
        operation: &str,
        parameters: EmbeddingQualificationParameters,
        start_gate: Option<PathBuf>,
    ) -> Result<RunningWorker> {
        self.spawn_worker_for(
            self.context.primary_index,
            operation,
            parameters,
            start_gate,
        )
    }

    fn spawn_worker_for(
        &mut self,
        project_index: usize,
        operation: &str,
        parameters: EmbeddingQualificationParameters,
        start_gate: Option<PathBuf>,
    ) -> Result<RunningWorker> {
        let runtime = self
            .context
            .runtimes
            .get(project_index)
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_project_index_invalid"))?;
        let project = self
            .context
            .projects
            .get(project_index)
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_project_index_invalid"))?;
        let project_identity_sha256 = project_identity_sha256(runtime);
        self.next_worker = self.next_worker.saturating_add(1);
        let invocation_id = format!("{}-{}", self.context.scenario, self.next_worker);
        let request_path = self
            .context
            .output_directory
            .join(format!(".{invocation_id}.worker-request.json"));
        let output_path = self
            .context
            .output_directory
            .join(format!(".{invocation_id}.worker-output.json"));
        let request = WorkerRequest {
            schema_version: 1,
            nonce_sha256: self.context.nonce_sha256.into(),
            executable_sha256: self.executable.sha256().into(),
            project: project.clone(),
            operation: operation.into(),
            parameters,
            start_gate,
        };
        write_atomic_json(&request_path, &request)?;
        let started_ns = self.clock.now_ns();
        let child = Command::new(self.executable.path())
            .arg(WORKER_COMMAND)
            .arg("--request")
            .arg(&request_path)
            .arg("--output")
            .arg(&output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn qualification worker {invocation_id}"))?;
        let pid = child.id();
        let process_start_id = wait_for_process_start(&*self.clock, pid)?;
        let invocation_index = self.artifact.orchestration.process_invocations.len();
        self.artifact
            .orchestration
            .process_invocations
            .push(ProcessInvocation {
                invocation_id: invocation_id.clone(),
                operation: operation.into(),
                project_identity_sha256: project_identity_sha256.clone(),
                pid,
                process_start_id,
                started_ns,
                finished_ns: None,
                exit_code: None,
                termination: None,
            });
        self.event(
            "orchestrator",
            "worker_started",
            Some(invocation_id),
            btree([("pid", json!(pid)), ("operation", json!(operation))]),
        );
        Ok(RunningWorker {
            invocation_index,
            child,
            request_path,
            output_path,
        })
    }

    fn primary_runtime(&self) -> &SidecarRuntimeConfig {
        &self.context.runtimes[self.context.primary_index]
    }

    fn finish_worker(
        &mut self,
        mut worker: RunningWorker,
        timeout: Duration,
    ) -> Result<WorkerOutput> {
        let status = wait_for_child(&*self.clock, &mut worker.child, timeout)?;
        self.finish_invocation(worker.invocation_index, status, "exited");
        if !status.success() {
            cleanup_worker_files(&worker);
            bail!("embedding_qualification_worker_failed:{status}");
        }
        let bytes = read_private_request(&worker.output_path)?;
        let output: WorkerOutput =
            serde_json::from_slice(&bytes).context("parse qualification worker output")?;
        validate_worker_output(
            &output,
            &self.artifact.orchestration.process_invocations[worker.invocation_index],
            self.executable.sha256(),
        )?;
        self.observation(
            "worker_output",
            btree([
                (
                    "invocation_id",
                    json!(
                        self.artifact.orchestration.process_invocations[worker.invocation_index]
                            .invocation_id
                    ),
                ),
                ("output", serde_json::to_value(&output)?),
            ]),
        );
        cleanup_worker_files(&worker);
        Ok(output)
    }

    fn terminate_worker(&mut self, mut worker: RunningWorker) -> Result<()> {
        worker
            .child
            .kill()
            .context("terminate qualification worker")?;
        let status = worker.child.wait().context("reap qualification worker")?;
        self.finish_invocation(worker.invocation_index, status, "terminated");
        cleanup_worker_files(&worker);
        Ok(())
    }

    fn finish_invocation(&mut self, index: usize, status: ExitStatus, termination: &str) {
        let finished_ns = self.clock.now_ns();
        let invocation = &mut self.artifact.orchestration.process_invocations[index];
        invocation.finished_ns = Some(finished_ns);
        invocation.exit_code = status.code();
        invocation.termination = Some(termination.into());
        let invocation_id = invocation.invocation_id.clone();
        self.event(
            "orchestrator",
            "worker_finished",
            Some(invocation_id),
            btree([
                ("exit_code", json!(status.code())),
                ("termination", json!(termination)),
            ]),
        );
    }

    fn transition(&mut self, name: &str, values: BTreeMap<String, Value>) {
        self.evidence.transitions.insert(name.into());
        self.observation(name, values);
    }

    fn record_worker_snapshot(
        &mut self,
        phase: &str,
        output: &WorkerOutput,
    ) -> Result<EmbeddingServerSnapshot> {
        let snapshot = output
            .result
            .as_ref()
            .and_then(|result| result.final_snapshot.clone())
            .or_else(|| {
                output
                    .protocol_exchange
                    .as_ref()
                    .map(|exchange| exchange.hello_snapshot.clone())
            })
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_worker_snapshot_missing"))?;
        self.artifact
            .process_observations
            .push(ProcessObservation::from_snapshot(
                phase,
                self.clock.now_ns(),
                Some(snapshot.clone()),
            ));
        Ok(snapshot)
    }

    fn client_death(&mut self) -> Result<()> {
        let owner = self.ensure_owner("client_death_owner")?;
        self.control("hold_class", Some("bulk"))?;
        self.control("hold_class", Some("query"))?;
        let dead_client = self.spawn_worker(
            "dead_client_load",
            EmbeddingQualificationParameters {
                query_count: DEAD_CLIENT_QUERY_COUNT as u32,
                bulk_count: DEAD_CLIENT_BULK_COUNT as u32,
                documents_per_bulk: 4,
                input_bytes: 256,
                hold_ms: CLIENT_DEATH_LEASE_HOLD_MS,
            },
            None,
        )?;
        let lease_snapshot =
            self.wait_for_snapshot("client_death_lease_active", SNAPSHOT_TIMEOUT, |snapshot| {
                snapshot.scheduler.lease_count > 0
                    && snapshot.scheduler.query_depth > 0
                    && snapshot.scheduler.bulk_depth > 0
                    && snapshot.scheduler.active_request_count > 0
            })?;
        self.transition(
            "dead_client_work_observed",
            scheduler_values(&lease_snapshot),
        );
        let other_project = 1_usize.saturating_sub(self.context.primary_index);
        let observer =
            self.spawn_worker_for(other_project, "observe", query_parameters(1), None)?;
        let observer_output = self.finish_worker(observer, FROZEN_WORKER_TIMEOUT)?;
        require_worker_success(&observer_output, "client_death_other_client_observe")?;
        self.transition(
            "other_client_continued",
            btree([(
                "project_identity_sha256",
                json!(project_identity_sha256(
                    &self.context.runtimes[other_project]
                )),
            )]),
        );
        self.terminate_worker(dead_client)?;
        self.transition(
            "client_terminated",
            btree([("termination", json!("terminated"))]),
        );
        let reclaimed = self.wait_for_snapshot(
            "client_death_lease_reclaimed",
            SNAPSHOT_TIMEOUT,
            |snapshot| {
                snapshot.scheduler.lease_count == 0
                    && snapshot.scheduler.query_depth == 0
                    && snapshot.scheduler.bulk_depth == 0
                    && snapshot.scheduler.active_request_count == 0
            },
        )?;
        if reclaimed.process.server_instance_id != owner.process.server_instance_id {
            bail!("embedding_qualification_client_death_replaced_owner");
        }
        self.transition("dead_client_work_reclaimed", scheduler_values(&reclaimed));
        self.control("release_class", Some("bulk"))?;
        self.control("release_class", Some("query"))?;
        let worker = self.spawn_worker_for(other_project, "query", query_parameters(1), None)?;
        let output = self.finish_worker(worker, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&output, "client_death_post_reclaim")?;
        let snapshot = self.record_worker_snapshot("client_death_post_reclaim", &output)?;
        self.transition(
            "post_reclaim_other_client_query",
            btree([(
                "server_instance_id",
                json!(snapshot.process.server_instance_id),
            )]),
        );
        Ok(())
    }

    fn cold_race(&mut self) -> Result<()> {
        self.reset_owner("cold_race_no_owner")?;
        self.evidence
            .transitions
            .insert("no_owner_before_race".into());
        let gate = self
            .context
            .output_directory
            .join(format!(".{}.cold-race-gate.json", self.context.scenario));
        let first = self.spawn_worker_for(
            0,
            "long_protocol_query",
            query_parameters(1),
            Some(gate.clone()),
        )?;
        let second = self.spawn_worker_for(
            1,
            "long_protocol_query",
            query_parameters(1),
            Some(gate.clone()),
        )?;
        write_atomic_json(
            &gate,
            &json!({"schema_version": 1, "released_ns": self.clock.now_ns()}),
        )?;
        self.event(
            "orchestrator",
            "start_gate_released",
            None,
            btree([("worker_count", json!(2))]),
        );
        let first_output = self.finish_worker(first, NORMAL_WORKER_TIMEOUT)?;
        let second_output = self.finish_worker(second, NORMAL_WORKER_TIMEOUT)?;
        let _ = fs::remove_file(&gate);
        require_protocol_success(&first_output, "cold_race_first")?;
        require_protocol_success(&second_output, "cold_race_second")?;
        let first_snapshot = self.record_worker_snapshot("cold_race_first", &first_output)?;
        let second_snapshot = self.record_worker_snapshot("cold_race_second", &second_output)?;
        let invocations = &self.artifact.orchestration.process_invocations;
        let first_invocation = &invocations[invocations.len() - 2];
        let second_invocation = &invocations[invocations.len() - 1];
        if invocations.len() < 2
            || first_invocation.pid == second_invocation.pid
            || first_invocation.project_identity_sha256 == second_invocation.project_identity_sha256
        {
            bail!("embedding_qualification_cold_race_processes_not_independent");
        }
        let first_pid = first_invocation.pid;
        let second_pid = second_invocation.pid;
        let first_project = first_invocation.project_identity_sha256.clone();
        let second_project = second_invocation.project_identity_sha256.clone();
        let first_transport = &first_output
            .protocol_exchange
            .as_ref()
            .expect("protocol success required above")
            .transport_identity;
        let second_transport = &second_output
            .protocol_exchange
            .as_ref()
            .expect("protocol success required above")
            .transport_identity;
        self.transition(
            "two_independent_processes",
            btree([
                ("first_pid", json!(first_pid)),
                ("second_pid", json!(second_pid)),
                ("first_project_identity_sha256", json!(first_project)),
                ("second_project_identity_sha256", json!(second_project)),
                (
                    "first_transport_peer_verified",
                    json!(first_transport.peer_verified),
                ),
                (
                    "second_transport_peer_verified",
                    json!(second_transport.peer_verified),
                ),
            ]),
        );
        if !same_server_authority(&first_snapshot, &second_snapshot) {
            bail!("embedding_qualification_cold_race_multiple_owners");
        }
        self.transition(
            "single_server_convergence",
            btree([
                (
                    "server_instance_id",
                    json!(first_snapshot.process.server_instance_id),
                ),
                (
                    "lifetime_authority_id",
                    json!(first_snapshot.authority.lifetime_authority_id),
                ),
            ]),
        );
        Ok(())
    }

    fn frozen_owner(&mut self) -> Result<()> {
        let before = self.ensure_owner("frozen_owner_before")?;
        self.control("freeze_owner", None)?;
        let worker = self.spawn_worker("query", query_parameters(1), None)?;
        let worker_result = self.finish_worker(worker, FROZEN_WORKER_TIMEOUT);
        let release_result = self.control("release_owner", None);
        let output = worker_result?;
        release_result?;
        require_worker_error(
            &output,
            "embedding_server_owner_unresponsive",
            "frozen_owner",
        )?;
        self.transition(
            "bounded_owner_unresponsive",
            btree([
                ("started_ns", json!(output.started_ns)),
                ("finished_ns", json!(output.finished_ns)),
                (
                    "error_code",
                    json!(output.error.as_ref().map(|error| &error.code)),
                ),
            ]),
        );
        let after = self.wait_for_snapshot("frozen_owner_released", SNAPSHOT_TIMEOUT, |_| true)?;
        if !same_server_authority(&before, &after) {
            bail!("embedding_qualification_frozen_owner_takeover_detected");
        }
        self.transition(
            "owner_identity_stable",
            btree([
                (
                    "server_instance_id",
                    json!(after.process.server_instance_id),
                ),
                (
                    "lifetime_authority_id",
                    json!(after.authority.lifetime_authority_id),
                ),
            ]),
        );
        Ok(())
    }

    fn incompatible_owner(&mut self) -> Result<()> {
        let before = self.ensure_owner("incompatible_owner_before")?;
        let lease = self.spawn_worker(
            "lease",
            EmbeddingQualificationParameters {
                query_count: 1,
                bulk_count: 0,
                documents_per_bulk: 0,
                input_bytes: 64,
                hold_ms: CLIENT_DEATH_LEASE_HOLD_MS,
            },
            None,
        )?;
        self.wait_for_snapshot("incompatible_owner_active", SNAPSHOT_TIMEOUT, |snapshot| {
            snapshot.scheduler.lease_count > 0
        })?;
        self.control("force_incompatible", None)?;
        let active = self.spawn_worker("query", query_parameters(1), None)?;
        let active_output = self.finish_worker(active, FROZEN_WORKER_TIMEOUT)?;
        require_worker_error(
            &active_output,
            "embedding_server_incompatible_active_owner",
            "incompatible_active_owner",
        )?;
        self.transition(
            "active_owner_rejected",
            btree([
                (
                    "compatibility_evidence",
                    json!("injected_contract_mismatch"),
                ),
                (
                    "error_code",
                    json!(
                        active_output
                            .error
                            .as_ref()
                            .map(|error| error.code.as_str())
                    ),
                ),
            ]),
        );
        self.terminate_worker(lease)?;
        self.wait_for_snapshot("incompatible_owner_idle", SNAPSHOT_TIMEOUT, |snapshot| {
            snapshot.scheduler.lease_count == 0 && snapshot.scheduler.active_request_count == 0
        })?;
        let idle = self.spawn_worker("query", query_parameters(1), None)?;
        let idle_output = self.finish_worker(idle, FROZEN_WORKER_TIMEOUT)?;
        require_worker_error(
            &idle_output,
            "embedding_server_draining",
            "incompatible_idle_owner",
        )?;
        self.transition(
            "idle_owner_draining",
            btree([
                (
                    "compatibility_evidence",
                    json!("injected_contract_mismatch"),
                ),
                (
                    "error_code",
                    json!(idle_output.error.as_ref().map(|error| error.code.as_str())),
                ),
            ]),
        );
        self.wait_for_absence("incompatible_owner_exited", SNAPSHOT_TIMEOUT)?;
        let replacement = self.spawn_worker("query", query_parameters(1), None)?;
        let replacement_output = self.finish_worker(replacement, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&replacement_output, "incompatible_replacement")?;
        let after =
            self.record_worker_snapshot("incompatible_owner_replacement", &replacement_output)?;
        if before.process.server_instance_id == after.process.server_instance_id {
            bail!("embedding_qualification_incompatible_owner_not_replaced");
        }
        self.transition(
            "compatible_replacement",
            btree([
                (
                    "old_server_instance_id",
                    json!(before.process.server_instance_id),
                ),
                (
                    "new_server_instance_id",
                    json!(after.process.server_instance_id),
                ),
            ]),
        );
        Ok(())
    }

    fn mixed_queue(&mut self) -> Result<()> {
        self.ensure_owner("mixed_queue_owner")?;
        self.control("hold_class", Some("bulk"))?;
        self.control("hold_class", Some("query"))?;
        let seed = self.spawn_worker("long_protocol_bulk", query_parameters(1), None)?;
        self.wait_for_snapshot("mixed_queue_seed_active", SNAPSHOT_TIMEOUT, |snapshot| {
            snapshot
                .scheduler
                .active_request
                .as_ref()
                .is_some_and(|active| active.class == "bulk")
        })?;
        let first_gate = self
            .context
            .output_directory
            .join(".mixed-queue-first-gate.json");
        let second_gate = self
            .context
            .output_directory
            .join(".mixed-queue-second-gate.json");
        let first = self.spawn_worker_for(
            0,
            "queue_load",
            EmbeddingQualificationParameters {
                query_count: 32,
                bulk_count: 32,
                documents_per_bulk: 1,
                input_bytes: 64,
                hold_ms: 0,
            },
            Some(first_gate.clone()),
        )?;
        let second = self.spawn_worker_for(
            1,
            "queue_load",
            EmbeddingQualificationParameters {
                query_count: 33,
                bulk_count: 33,
                documents_per_bulk: 1,
                input_bytes: 64,
                hold_ms: 0,
            },
            Some(second_gate.clone()),
        )?;
        write_atomic_json(&first_gate, &json!({"schema_version": 1}))?;
        self.wait_for_snapshot(
            "mixed_queue_first_project_enqueued",
            QUEUE_SETUP_TIMEOUT,
            |snapshot| snapshot.scheduler.query_depth >= 32 && snapshot.scheduler.bulk_depth >= 32,
        )?;
        write_atomic_json(&second_gate, &json!({"schema_version": 1}))?;
        let saturated =
            self.wait_for_snapshot("mixed_queue_saturated", QUEUE_SETUP_TIMEOUT, |snapshot| {
                snapshot.scheduler.query_depth == snapshot.scheduler.query_capacity
                    && snapshot.scheduler.bulk_depth == snapshot.scheduler.bulk_capacity
            })?;
        self.transition("queues_saturated", scheduler_values(&saturated));
        self.control("release_class", Some("bulk"))?;
        let query_selected =
            self.wait_for_snapshot("mixed_queue_query_selected", SNAPSHOT_TIMEOUT, |snapshot| {
                snapshot.scheduler.bulk_depth > 0
                    && snapshot
                        .scheduler
                        .active_request
                        .as_ref()
                        .is_some_and(|active| active.class == "query")
            })?;
        self.transition(
            "query_selected_before_bulk_backlog",
            scheduler_values(&query_selected),
        );
        self.control("release_class", Some("query"))?;
        let seed_output = self.finish_worker(seed, NORMAL_WORKER_TIMEOUT)?;
        let first_output = self.finish_worker(first, NORMAL_WORKER_TIMEOUT)?;
        let second_output = self.finish_worker(second, NORMAL_WORKER_TIMEOUT)?;
        let _ = fs::remove_file(&first_gate);
        let _ = fs::remove_file(&second_gate);
        require_protocol_success(&seed_output, "mixed_queue_seed")?;
        if first_output.clock != second_output.clock {
            bail!("embedding_qualification_queue_clock_domain_mismatch");
        }
        let mut operations = first_output
            .queue_operations
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_first_queue_output_missing"))?;
        operations.extend(second_output.queue_operations.ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_second_queue_output_missing")
        })?);
        let analysis = analyze_queue_operations(&operations)?;
        for operation in &operations {
            self.event(
                "worker_request",
                "completed",
                Some(operation.correlation_id.clone()),
                btree([
                    (
                        "project_identity_sha256",
                        json!(operation.project_identity_sha256),
                    ),
                    ("class", json!(operation.class)),
                    ("ordinal", json!(operation.ordinal)),
                    ("submitted_ns", json!(operation.submitted_ns)),
                    ("completed_ns", json!(operation.completed_ns)),
                    ("status", json!(operation.status)),
                    ("error", json!(operation.error)),
                ]),
            );
        }
        self.transition("typed_capacity_retry_observed", analysis.capacity);
        self.transition("per_class_fifo_observed", analysis.class_orders);
        self.transition("global_fifo_across_projects", analysis.project_orders);
        self.transition("query_preference_observed", analysis.query_preference);
        self.transition("bulk_resumed", analysis.bulk_resumption);
        Ok(())
    }

    fn server_crash(&mut self) -> Result<()> {
        let before = self.ensure_owner("server_crash_before")?;
        self.control("hold_class", Some("query"))?;
        let worker = self.spawn_worker("query", query_parameters(1), None)?;
        let active =
            self.wait_for_snapshot("server_crash_inflight", SNAPSHOT_TIMEOUT, |snapshot| {
                snapshot
                    .scheduler
                    .active_request
                    .as_ref()
                    .is_some_and(|active| active.class == "query")
            })?;
        self.transition("inflight_request_observed", scheduler_values(&active));
        self.control("crash_server", None)?;
        let output = self.finish_worker(worker, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&output, "server_crash_replay")?;
        let initial = output
            .result
            .as_ref()
            .and_then(|result| result.initial_snapshot.as_ref())
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_crash_initial_missing"))?;
        if initial.process.server_instance_id != before.process.server_instance_id {
            bail!("embedding_qualification_crash_initial_owner_changed");
        }
        let after = self.record_worker_snapshot("server_crash_replacement", &output)?;
        if before.process.server_instance_id == after.process.server_instance_id {
            bail!("embedding_qualification_crash_owner_not_replaced");
        }
        self.transition(
            "server_replaced",
            btree([
                (
                    "old_server_instance_id",
                    json!(before.process.server_instance_id),
                ),
                (
                    "new_server_instance_id",
                    json!(after.process.server_instance_id),
                ),
            ]),
        );
        let operation_count = output
            .result
            .as_ref()
            .map_or(0, |result| result.operations.len());
        self.transition(
            "query_replayed",
            btree([
                ("submitted_operation_count", json!(operation_count)),
                ("completed_operation_count", json!(operation_count)),
                (
                    "observed_server_instance_ids",
                    json!([
                        before.process.server_instance_id,
                        after.process.server_instance_id
                    ]),
                ),
            ]),
        );
        Ok(())
    }

    fn true_idle_respawn(&mut self) -> Result<()> {
        let before = self.ensure_owner("true_idle_before")?;
        self.evidence.transitions.insert("owner_started".into());
        self.control("hold_class", Some("bulk"))?;
        self.control("hold_class", Some("query"))?;
        let lease = self.spawn_worker(
            "lease",
            EmbeddingQualificationParameters {
                query_count: 1,
                bulk_count: 0,
                documents_per_bulk: 0,
                input_bytes: 64,
                hold_ms: CLIENT_DEATH_LEASE_HOLD_MS,
            },
            None,
        )?;
        self.wait_for_snapshot("true_idle_lease_active", SNAPSHOT_TIMEOUT, |snapshot| {
            snapshot.scheduler.lease_count > 0
        })?;
        let active_bulk = self.spawn_worker("long_protocol_bulk", query_parameters(1), None)?;
        self.wait_for_snapshot("true_idle_active_bulk", SNAPSHOT_TIMEOUT, |snapshot| {
            snapshot
                .scheduler
                .active_request
                .as_ref()
                .is_some_and(|active| active.class == "bulk")
        })?;
        let queued_query = self.spawn_worker_for(
            1_usize.saturating_sub(self.context.primary_index),
            "long_protocol_query",
            query_parameters(1),
            None,
        )?;
        let queued_bulk = self.spawn_worker("long_protocol_bulk", query_parameters(1), None)?;
        let anti_idle =
            self.wait_for_snapshot("true_idle_work_held", SNAPSHOT_TIMEOUT, |snapshot| {
                snapshot.scheduler.lease_count > 0
                    && snapshot.scheduler.active_request_count > 0
                    && snapshot.scheduler.query_depth > 0
                    && snapshot.scheduler.bulk_depth > 0
            })?;
        self.transition("anti_idle_work_observed", scheduler_values(&anti_idle));
        let wait = Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
            .saturating_add(IDLE_EXIT_GRACE);
        let held_started_ns = self.clock.now_ns();
        self.clock.sleep(wait);
        let preserved = self
            .observe("true_idle_work_preserved_owner")?
            .ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_owner_exited_with_anti_idle_work")
            })?;
        if !same_server_authority(&before, &preserved)
            || preserved.scheduler.lease_count == 0
            || preserved.scheduler.active_request_count == 0
            || preserved.scheduler.query_depth == 0
            || preserved.scheduler.bulk_depth == 0
        {
            bail!("embedding_qualification_anti_idle_contract_missing");
        }
        self.transition(
            "owner_preserved_across_idle_boundary",
            btree([
                ("held_started_ns", json!(held_started_ns)),
                ("held_observed_ns", json!(self.clock.now_ns())),
                (
                    "contract_idle_timeout_ms",
                    json!(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS),
                ),
                (
                    "server_instance_id",
                    json!(preserved.process.server_instance_id),
                ),
            ]),
        );
        self.control("release_class", Some("bulk"))?;
        self.control("release_class", Some("query"))?;
        let active_output = self.finish_worker(active_bulk, NORMAL_WORKER_TIMEOUT)?;
        let query_output = self.finish_worker(queued_query, NORMAL_WORKER_TIMEOUT)?;
        let bulk_output = self.finish_worker(queued_bulk, NORMAL_WORKER_TIMEOUT)?;
        require_protocol_success(&active_output, "true_idle_active_bulk")?;
        require_protocol_success(&query_output, "true_idle_queued_query")?;
        require_protocol_success(&bulk_output, "true_idle_queued_bulk")?;
        self.terminate_worker(lease)?;
        let reclaimed =
            self.wait_for_snapshot("true_idle_work_reclaimed", SNAPSHOT_TIMEOUT, |snapshot| {
                snapshot.scheduler.lease_count == 0
                    && snapshot.scheduler.active_request_count == 0
                    && snapshot.scheduler.query_depth == 0
                    && snapshot.scheduler.bulk_depth == 0
            })?;
        self.transition("anti_idle_work_reclaimed", scheduler_values(&reclaimed));
        let started_ns = self.clock.now_ns();
        self.clock.sleep(wait);
        self.observation(
            "true_idle_wait",
            btree([
                ("started_ns", json!(started_ns)),
                ("finished_ns", json!(self.clock.now_ns())),
                (
                    "contract_idle_timeout_ms",
                    json!(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS),
                ),
            ]),
        );
        if self.observe("true_idle_after_wait")?.is_some() {
            bail!("embedding_qualification_true_idle_owner_still_present");
        }
        self.transition(
            "owner_absent_after_true_idle",
            btree([(
                "old_server_instance_id",
                json!(before.process.server_instance_id),
            )]),
        );
        let worker = self.spawn_worker("query", query_parameters(1), None)?;
        let output = self.finish_worker(worker, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&output, "true_idle_respawn")?;
        let after = self.record_worker_snapshot("true_idle_respawned", &output)?;
        if before.process.server_instance_id == after.process.server_instance_id {
            bail!("embedding_qualification_true_idle_owner_not_replaced");
        }
        self.transition(
            "server_respawned",
            btree([(
                "new_server_instance_id",
                json!(after.process.server_instance_id),
            )]),
        );
        Ok(())
    }

    fn worker_stall(&mut self) -> Result<()> {
        let before = self.ensure_owner("worker_stall_before")?;
        self.control("stall_native", None)?;
        let worker = self.spawn_worker(
            "stall_protocol_bulk",
            EmbeddingQualificationParameters {
                query_count: 0,
                bulk_count: 1,
                documents_per_bulk: 4,
                input_bytes: 256,
                hold_ms: 0,
            },
            None,
        )?;
        let active =
            self.wait_for_snapshot("worker_stall_inflight", SNAPSHOT_TIMEOUT, |snapshot| {
                snapshot
                    .scheduler
                    .active_request
                    .as_ref()
                    .is_some_and(|active| active.class == "bulk")
            })?;
        self.transition("stalled_request_observed", scheduler_values(&active));
        let output = self.finish_worker(worker, STALL_WORKER_TIMEOUT)?;
        let exchange = output
            .protocol_exchange
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_stall_exchange_missing"))?;
        if exchange.response.is_some() || exchange.terminal_transport_error.is_none() {
            bail!("embedding_qualification_watchdog_fail_stop_missing");
        }
        wait_for_process_exit(&*self.clock, before.process.pid, SNAPSHOT_TIMEOUT)?;
        self.wait_for_absence("worker_stall_owner_absent", SNAPSHOT_TIMEOUT)?;
        self.transition(
            "watchdog_fail_stop_observed",
            btree([
                ("old_pid", json!(before.process.pid)),
                (
                    "terminal_transport_error",
                    json!(exchange.terminal_transport_error),
                ),
            ]),
        );
        let replacement = self.spawn_worker("query", query_parameters(1), None)?;
        let replacement_output = self.finish_worker(replacement, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&replacement_output, "worker_stall_replacement")?;
        let after = self.record_worker_snapshot("worker_stall_replacement", &replacement_output)?;
        if before.process.server_instance_id == after.process.server_instance_id {
            bail!("embedding_qualification_stalled_owner_not_replaced");
        }
        self.transition(
            "post_stall_replacement",
            btree([(
                "new_server_instance_id",
                json!(after.process.server_instance_id),
            )]),
        );
        Ok(())
    }
}

struct VectorMeasurement {
    interval: MeasurementInterval,
    request_id: String,
    completed_documents: u64,
    server_identity: RawServerIdentity,
}

fn connect_until(
    transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
    clock: &dyn AwakeMonotonicClock,
    budget: Duration,
) -> Result<crate::embedding_server_transport::NativeEmbeddingStream> {
    let started = clock.now_ns();
    loop {
        match transport.connect(budget.saturating_sub(elapsed(clock, started)))? {
            crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => {
                return Ok(stream);
            }
            crate::embedding_server_transport::NativeConnectOutcome::NoOwner
            | crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {}
        }
        if elapsed(clock, started) >= budget {
            bail!("embedding_qualification_connect_timeout");
        }
        clock.sleep(POLL);
    }
}

fn validated_hello(
    stream: &mut crate::embedding_server_transport::NativeEmbeddingStream,
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
) -> Result<EmbeddingServerSnapshot> {
    EmbeddingServerStream::set_read_timeout(stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(stream, Some(Duration::from_secs(2)))?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let request_id = qualification_request_id("measurement-hello", clock.now_ns());
    write_protocol_frame(
        stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.clone(),
            compatibility: compatibility.clone(),
            operation: EmbeddingOperation::Hello {
                intent: "activate".into(),
            },
        },
        &[],
    )?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) = read_protocol_frame(stream)?;
    if !payload.is_empty()
        || response.request_id != request_id
        || response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || response.error.is_some()
    {
        bail!("embedding_qualification_measurement_hello_invalid");
    }
    let (compatibility_sha256, snapshot) = match response.result {
        Some(EmbeddingResult::Hello {
            compatibility_sha256,
            snapshot,
        }) => (compatibility_sha256, *snapshot),
        _ => bail!("embedding_qualification_measurement_hello_missing"),
    };
    if compatibility_sha256 != compatibility.digest()? {
        bail!("embedding_qualification_measurement_hello_incompatible");
    }
    authenticate_snapshot(&snapshot, stream.identity())?;
    Ok(snapshot)
}

fn measure_vector_operation(
    transport: &crate::embedding_server_transport::NativeEmbeddingClientTransport,
    runtime: &SidecarRuntimeConfig,
    class: &str,
    inputs: Vec<String>,
    runner: &ScenarioRunner<'_>,
) -> Result<VectorMeasurement> {
    if inputs.is_empty() || inputs.iter().any(|input| input.trim().is_empty()) {
        bail!("embedding_qualification_measurement_inputs_invalid");
    }
    let mut stream = connect_until(transport, runner.clock.as_ref(), Duration::from_secs(2))?;
    let hello = validated_hello(&mut stream, runtime, runner.clock.as_ref())?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let request_id =
        qualification_request_id(&format!("measurement-{class}"), runner.clock.now_ns());
    let scope_seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    let scope_id = sha256_bytes(scope_seed.as_bytes());
    let operation = match class {
        "query" if inputs.len() == 1 => EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms: 180_000,
            retry_after_ms: 100,
            input: inputs[0].clone(),
        },
        "bulk" => EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms: 180_000,
            retry_after_ms: 100,
            inputs: inputs.clone(),
        },
        _ => bail!("embedding_qualification_measurement_class_invalid"),
    };
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_secs(180)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_secs(180)))?;
    let start = runner.begin_measurement()?;
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
    let interval = runner.finish_measurement(start)?;
    Ok(VectorMeasurement {
        interval,
        request_id,
        completed_documents: inputs.len() as u64,
        server_identity: RawServerIdentity {
            server_instance_id: identity.server_instance_id,
            process_start_id: hello.process.process_start_id,
            load_generation: identity.load_generation,
        },
    })
}

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

fn require_protocol_exchange_success(exchange: &WorkerProtocolExchange, phase: &str) -> Result<()> {
    if exchange.terminal_transport_error.is_some()
        || exchange.response_payload_bytes == 0
        || exchange.response.as_ref().is_none_or(|response| {
            response.error.is_some()
                || !matches!(response.result, Some(EmbeddingResult::Vectors { .. }))
        })
    {
        bail!("embedding_qualification_protocol_exchange_failed:{phase}");
    }
    Ok(())
}

fn completed_token_count(directory: &Path, request_id: &str) -> Result<u64> {
    existing_control_events(directory)?
        .into_iter()
        .rev()
        .find_map(|event| {
            (event.action == "completed_tokens"
                && event.status == "completed"
                && event
                    .details
                    .as_ref()
                    .and_then(|details| details.get("request_id"))
                    .is_some_and(|observed| observed == request_id))
            .then(|| {
                event
                    .details
                    .as_ref()
                    .and_then(|details| details.get("completed_tokens"))
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .flatten()
        })
        .filter(|count| *count > 0)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_completed_tokens_missing"))
}

fn accelerator_operands(identity: &EmbeddingEngineIdentity) -> BTreeMap<String, Value> {
    btree([
        ("policy", json!(identity.policy)),
        ("backend", json!(identity.backend)),
        (
            "accelerator_execution_verified",
            json!(identity.accelerator_execution_verified),
        ),
        (
            "resident_accelerator_tensor_count",
            json!(identity.resident_accelerator_tensor_count),
        ),
        (
            "resident_accelerator_tensor_bytes",
            json!(identity.resident_accelerator_tensor_bytes),
        ),
        (
            "offloaded_layer_count",
            json!(identity.offloaded_layer_count),
        ),
        ("model_layer_count", json!(identity.model_layer_count)),
    ])
}

fn raw_server_identity(snapshot: &EmbeddingServerSnapshot) -> Result<RawServerIdentity> {
    let load_generation = snapshot
        .engine
        .as_ref()
        .map(|engine| engine.load_generation)
        .filter(|generation| *generation > 0)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_server_generation_missing"))?;
    Ok(RawServerIdentity {
        server_instance_id: snapshot.process.server_instance_id.clone(),
        process_start_id: snapshot.process.process_start_id.clone(),
        load_generation,
    })
}

pub(super) fn run_worker(command: InternalEmbeddingQualificationCommand) -> Result<()> {
    let request_bytes = read_private_request(&command.request)?;
    let request: WorkerRequest =
        serde_json::from_slice(&request_bytes).context("parse embedding qualification worker")?;
    if request.schema_version != 1 {
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
        wait_for_gate(clock.as_ref(), gate)?;
    }
    let process_start_id = current_process_start_identity()?;
    let started_ns = clock.now_ns();
    let defaults = crate::sidecar_runtime::process_defaults();
    let runtime = SidecarRuntimeConfig::for_project_auto_with_process_defaults(
        &request.project,
        &defaults,
        &SidecarRuntimeOverrides::default(),
    );
    if request.operation == "dead_client_load" {
        return run_dead_client_load(&runtime, request.parameters, clock.as_ref());
    }
    let (result, protocol_exchange, queue_operations, error) = if request.operation == "queue_load"
    {
        match run_queue_load(&runtime, request.parameters, Arc::clone(&clock)) {
            Ok(operations) => (None, None, Some(operations), None),
            Err(error) => (None, None, None, Some(worker_error(&error))),
        }
    } else if matches!(
        request.operation.as_str(),
        "stall_protocol_bulk" | "long_protocol_query" | "long_protocol_bulk"
    ) {
        let (class, deadline_ms) = match request.operation.as_str() {
            "stall_protocol_bulk" => ("bulk", STALL_PROTOCOL_DEADLINE_MS),
            "long_protocol_query" => ("query", ANTI_IDLE_PROTOCOL_DEADLINE_MS),
            "long_protocol_bulk" => ("bulk", ANTI_IDLE_PROTOCOL_DEADLINE_MS),
            _ => unreachable!("matched exact protocol operations"),
        };
        match run_raw_protocol_exchange(&runtime, clock.as_ref(), class, deadline_ms) {
            Ok(exchange) => (None, Some(exchange), None, None),
            Err(error) => (None, None, None, Some(worker_error(&error))),
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
            Ok(result) => (Some(result), None, None, None),
            Err(error) => (None, None, None, Some(worker_error(&error))),
        }
    };
    let output = WorkerOutput {
        schema_version: 1,
        pid: std::process::id(),
        process_start_id,
        executable_sha256: executable.sha256().into(),
        executable_version: executable.version().into(),
        project_identity_sha256: project_identity_sha256(&runtime),
        clock: clock.snapshot(),
        started_ns,
        finished_ns: clock.now_ns(),
        result,
        protocol_exchange,
        queue_operations,
        error,
    };
    write_atomic_json(&command.output, &output)
}

fn run_dead_client_load(
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
        let client = client.clone();
        let input = input.clone();
        workers.push(
            std::thread::Builder::new()
                .name("codestory-dead-client-query".into())
                .spawn(move || {
                    let _ = client.embed_query(&input);
                })?,
        );
    }
    for _ in 0..parameters.bulk_count {
        let client = client.clone();
        let documents = documents.clone();
        workers.push(
            std::thread::Builder::new()
                .name("codestory-dead-client-bulk".into())
                .spawn(move || {
                    let _ = client.embed_documents(&documents);
                })?,
        );
    }
    loop {
        std::hint::black_box(&workers);
        clock.sleep(Duration::from_secs(1));
    }
}

fn run_queue_load(
    runtime: &SidecarRuntimeConfig,
    parameters: EmbeddingQualificationParameters,
    clock: Arc<dyn AwakeMonotonicClock>,
) -> Result<Vec<WorkerQueueOperation>> {
    if parameters.query_count == 0
        || parameters.bulk_count == 0
        || parameters.query_count > MIXED_QUEUE_COUNT
        || parameters.bulk_count > MIXED_QUEUE_COUNT
    {
        bail!("embedding_qualification_queue_load_parameters_invalid");
    }
    let runtime = runtime.clone();
    let project_identity = project_identity_sha256(&runtime);
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let mut workers = Vec::new();
    let maximum = parameters.query_count.max(parameters.bulk_count);
    for ordinal in 0..maximum {
        for class in ["bulk", "query"] {
            let count = if class == "bulk" {
                parameters.bulk_count
            } else {
                parameters.query_count
            };
            if ordinal >= count {
                continue;
            }
            let runtime = runtime.clone();
            let clock = Arc::clone(&clock);
            let transport = transport.clone();
            let project_identity = project_identity.clone();
            let correlation_id = format!(
                "queue-{}-{class}-{ordinal}-{}",
                std::process::id(),
                &project_identity[..12]
            );
            let (submitted_tx, submitted_rx) = std::sync::mpsc::sync_channel(1);
            let worker = std::thread::Builder::new()
                .name(format!("codestory-queue-{class}-{ordinal}"))
                .spawn(move || {
                    run_queue_operation(
                        &runtime,
                        transport,
                        clock.as_ref(),
                        &project_identity,
                        class,
                        ordinal,
                        correlation_id,
                        submitted_tx,
                    )
                })?;
            submitted_rx
                .recv_timeout(CONTROL_TIMEOUT)
                .context("wait for qualification queue submission")?;
            workers.push(worker);
        }
    }
    workers
        .into_iter()
        .map(|worker| {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("embedding_qualification_queue_worker_panicked"))?
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_queue_operation(
    runtime: &SidecarRuntimeConfig,
    transport: crate::embedding_server_transport::NativeEmbeddingClientTransport,
    clock: &dyn AwakeMonotonicClock,
    project_identity_sha256: &str,
    class: &str,
    ordinal: u32,
    correlation_id: String,
    submitted_tx: std::sync::mpsc::SyncSender<u64>,
) -> Result<WorkerQueueOperation> {
    let mut stream = match transport.connect(Duration::from_secs(2))? {
        crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => stream,
        crate::embedding_server_transport::NativeConnectOutcome::NoOwner => {
            bail!("embedding_server_absent")
        }
        crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {
            bail!("embedding_server_owner_unresponsive")
        }
    };
    let transport_identity = stream.identity().clone();
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_secs(2)))?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let hello_id = format!("{correlation_id}-hello");
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: hello_id.clone(),
            compatibility: compatibility.clone(),
            operation: EmbeddingOperation::Hello {
                intent: "activate".into(),
            },
        },
        &[],
    )?;
    let (hello, hello_payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_protocol_frame(&mut stream)?;
    if !hello_payload.is_empty() || hello.request_id != hello_id || hello.error.is_some() {
        bail!("embedding_qualification_queue_hello_invalid");
    }
    let hello_snapshot = match hello.result {
        Some(EmbeddingResult::Hello { snapshot, .. }) => *snapshot,
        _ => bail!("embedding_qualification_queue_hello_missing"),
    };
    authenticate_snapshot(&hello_snapshot, &transport_identity)?;
    let scope_seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    let scope_id = sha256_bytes(scope_seed.as_bytes());
    let deadline_ms = 120_000;
    let operation = match class {
        "query" => EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            input: format!("qualification-queue-{ordinal}"),
        },
        "bulk" => EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            inputs: vec![format!("qualification-queue-{ordinal}")],
        },
        _ => bail!("embedding_qualification_queue_class_invalid"),
    };
    let submitted_ns = clock.now_ns();
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: correlation_id.clone(),
            compatibility,
            operation,
        },
        &[],
    )?;
    submitted_tx
        .send(submitted_ns)
        .context("publish qualification queue submission")?;
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    let (response, payload): (EmbeddingProtocolResponse, Vec<u8>) =
        read_protocol_frame(&mut stream)?;
    if response.request_id != correlation_id
        || response.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || response.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || (response.result.is_some() == response.error.is_some())
    {
        bail!("embedding_qualification_queue_response_invalid");
    }
    let status = if response.error.is_some() {
        "failed"
    } else {
        "ok"
    };
    Ok(WorkerQueueOperation {
        correlation_id,
        project_identity_sha256: project_identity_sha256.into(),
        class: class.into(),
        ordinal,
        submitted_ns,
        completed_ns: clock.now_ns(),
        status: status.into(),
        error: response.error,
        response_payload_bytes: payload.len() as u64,
        transport_identity,
        hello_snapshot,
    })
}

fn run_raw_protocol_exchange(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    class: &str,
    deadline_ms: u64,
) -> Result<WorkerProtocolExchange> {
    run_raw_protocol_exchange_with_input(runtime, clock, class, deadline_ms, None)
}

fn run_raw_protocol_exchange_with_input(
    runtime: &SidecarRuntimeConfig,
    clock: &dyn AwakeMonotonicClock,
    class: &str,
    deadline_ms: u64,
    measured_input: Option<String>,
) -> Result<WorkerProtocolExchange> {
    let transport = crate::embedding_server_transport::NativeEmbeddingClientTransport::capture()?;
    let mut stream = match transport.connect(Duration::from_secs(2))? {
        crate::embedding_server_transport::NativeConnectOutcome::Connected(stream) => stream,
        crate::embedding_server_transport::NativeConnectOutcome::NoOwner => {
            bail!("embedding_server_absent")
        }
        crate::embedding_server_transport::NativeConnectOutcome::OwnerUnresponsive => {
            bail!("embedding_server_owner_unresponsive")
        }
    };
    let transport_identity = stream.identity().clone();
    if !transport_identity.peer_verified
        || transport_identity.peer_pid.is_none()
        || transport_identity
            .peer_process_start_id
            .as_deref()
            .is_none_or(str::is_empty)
        || transport_identity
            .peer_executable_sha256
            .as_deref()
            .is_none_or(str::is_empty)
    {
        bail!("embedding_qualification_stall_peer_unverified");
    }
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_secs(2)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_secs(2)))?;
    let compatibility = EmbeddingCompatibility::current(runtime.embedding.allow_cpu);
    let hello_id = qualification_request_id("stall-hello", clock.now_ns());
    write_protocol_frame(
        &mut stream,
        &EmbeddingProtocolRequest {
            protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
            schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
            request_id: hello_id.clone(),
            compatibility: compatibility.clone(),
            operation: EmbeddingOperation::Hello {
                intent: "activate".into(),
            },
        },
        &[],
    )?;
    let (hello, payload): (EmbeddingProtocolResponse, Vec<u8>) = read_protocol_frame(&mut stream)?;
    if !payload.is_empty()
        || hello.request_id != hello_id
        || hello.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || hello.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || hello.error.is_some()
    {
        bail!("embedding_qualification_stall_hello_invalid");
    }
    let hello_snapshot = match hello.result {
        Some(EmbeddingResult::Hello { snapshot, .. }) => *snapshot,
        _ => bail!("embedding_qualification_stall_hello_missing"),
    };
    authenticate_snapshot(&hello_snapshot, &transport_identity)?;
    let request_id = qualification_request_id(&format!("qualification-{class}"), clock.now_ns());
    let scope_seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    let submitted_ns = clock.now_ns();
    let scope_id = sha256_bytes(scope_seed.as_bytes());
    let operation = match class {
        "query" => EmbeddingOperation::EmbedQuery {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            input: measured_input.unwrap_or_else(|| "qualification-long-query".into()),
        },
        "bulk" => EmbeddingOperation::EmbedDocuments {
            scope_id,
            deadline_ms,
            retry_after_ms: 100,
            inputs: vec![measured_input.unwrap_or_else(|| "qualification-long-bulk".into())],
        },
        _ => bail!("embedding_qualification_protocol_class_invalid"),
    };
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
    EmbeddingServerStream::set_read_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    EmbeddingServerStream::set_write_timeout(&stream, Some(Duration::from_millis(deadline_ms)))?;
    let (response, response_payload_bytes, terminal_transport_error) =
        match read_protocol_frame::<EmbeddingProtocolResponse>(&mut stream) {
            Ok((response, payload)) => (Some(response), payload.len() as u64, None),
            Err(error) => (None, 0, Some(error_head(&error))),
        };
    Ok(WorkerProtocolExchange {
        request_id,
        submitted_ns,
        finished_ns: clock.now_ns(),
        transport_identity,
        hello_snapshot,
        response,
        response_payload_bytes,
        terminal_transport_error,
    })
}

fn authenticate_snapshot(
    snapshot: &EmbeddingServerSnapshot,
    transport: &EmbeddingTransportIdentity,
) -> Result<()> {
    if !snapshot.authority.peer_verified
        || snapshot.process.pid != transport.peer_pid.unwrap_or_default()
        || Some(snapshot.process.process_start_id.as_str())
            != transport.peer_process_start_id.as_deref()
        || Some(snapshot.process.executable_sha256.as_str())
            != transport.peer_executable_sha256.as_deref()
        || snapshot.authority.endpoint_namespace_id != transport.endpoint_namespace_id
        || snapshot.authority.lifetime_authority_id != transport.lifetime_authority_id
        || snapshot.authority.listener_id != transport.listener_id
        || snapshot.protocol.protocol_sha256 != PER_USER_EMBEDDING_PROTOCOL_SHA256
        || snapshot.protocol.constant_set_sha256 != PER_USER_EMBEDDING_CONSTANT_SET_SHA256
        || snapshot.protocol.measurement_protocol_sha256
            != PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256
    {
        bail!("embedding_qualification_snapshot_authentication_failed");
    }
    Ok(())
}

fn write_protocol_frame<T: Serialize>(
    stream: &mut dyn EmbeddingServerStream,
    control: &T,
    payload: &[u8],
) -> Result<()> {
    let control = serde_json::to_vec(control).context("serialize qualification protocol frame")?;
    if control.len() > PER_USER_EMBEDDING_MAX_METADATA_BYTES
        || payload.len() > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
    {
        bail!("embedding_server_frame_too_large");
    }
    stream.write_all(&(control.len() as u32).to_be_bytes())?;
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(&control)?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

fn read_protocol_frame<T: for<'de> Deserialize<'de>>(
    stream: &mut dyn EmbeddingServerStream,
) -> Result<(T, Vec<u8>)> {
    let mut control_len = [0_u8; 4];
    let mut payload_len = [0_u8; 4];
    stream.read_exact(&mut control_len)?;
    stream.read_exact(&mut payload_len)?;
    let control_len = u32::from_be_bytes(control_len) as usize;
    let payload_len = u32::from_be_bytes(payload_len) as usize;
    if control_len == 0
        || control_len > PER_USER_EMBEDDING_MAX_METADATA_BYTES
        || payload_len > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
    {
        bail!("embedding_server_frame_too_large");
    }
    let mut control = vec![0_u8; control_len];
    let mut payload = vec![0_u8; payload_len];
    stream.read_exact(&mut control)?;
    stream.read_exact(&mut payload)?;
    Ok((serde_json::from_slice(&control)?, payload))
}

fn validate_named_evidence(scenario: &str, evidence: &ScenarioEvidence) -> Result<()> {
    let (controls, transitions): (&[&str], &[&str]) = match scenario {
        "client_death" => (
            &[
                "hold_class:bulk",
                "hold_class:query",
                "release_class:bulk",
                "release_class:query",
            ],
            &[
                "dead_client_work_observed",
                "other_client_continued",
                "client_terminated",
                "dead_client_work_reclaimed",
                "post_reclaim_other_client_query",
            ],
        ),
        "cold_race" => (
            &[],
            &[
                "no_owner_before_race",
                "two_independent_processes",
                "single_server_convergence",
            ],
        ),
        "frozen_owner" => (
            &["freeze_owner", "release_owner"],
            &["bounded_owner_unresponsive", "owner_identity_stable"],
        ),
        "incompatible_owner" => (
            &["force_incompatible"],
            &[
                "active_owner_rejected",
                "idle_owner_draining",
                "compatible_replacement",
            ],
        ),
        "mixed_queue" => (
            &[
                "hold_class:bulk",
                "hold_class:query",
                "release_class:bulk",
                "release_class:query",
            ],
            &[
                "queues_saturated",
                "query_selected_before_bulk_backlog",
                "typed_capacity_retry_observed",
                "per_class_fifo_observed",
                "global_fifo_across_projects",
                "query_preference_observed",
                "bulk_resumed",
            ],
        ),
        "server_crash" => (
            &["hold_class:query", "crash_server"],
            &[
                "inflight_request_observed",
                "server_replaced",
                "query_replayed",
            ],
        ),
        "true_idle_respawn" => (
            &[
                "hold_class:bulk",
                "hold_class:query",
                "release_class:bulk",
                "release_class:query",
            ],
            &[
                "owner_started",
                "anti_idle_work_observed",
                "owner_preserved_across_idle_boundary",
                "anti_idle_work_reclaimed",
                "owner_absent_after_true_idle",
                "server_respawned",
            ],
        ),
        "worker_stall" => (
            &["stall_native"],
            &[
                "stalled_request_observed",
                "watchdog_fail_stop_observed",
                "post_stall_replacement",
            ],
        ),
        _ => bail!("embedding_qualification_scenario_unknown"),
    };
    if controls
        .iter()
        .any(|control| !evidence.controls.contains(*control))
        || transitions
            .iter()
            .any(|transition| !evidence.transitions.contains(*transition))
    {
        bail!("embedding_qualification_named_evidence_incomplete:{scenario}");
    }
    Ok(())
}

fn existing_control_events(directory: &Path) -> Result<Vec<ControlEvent>> {
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

fn qualification_command_path(directory: &Path, nonce: &str) -> PathBuf {
    directory.join(format!("{nonce}.command.json"))
}

fn qualification_nonce() -> Result<String> {
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

fn validate_worker_project(project: &Path) -> Result<()> {
    if !project.is_absolute() {
        bail!("embedding_qualification_project_not_absolute");
    }
    let metadata = fs::symlink_metadata(project)
        .with_context(|| format!("inspect qualification project {}", project.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("embedding_qualification_project_untrusted");
    }
    canonical_existing(project)?;
    Ok(())
}

fn validate_gate_path(path: &Path, directory: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.parent() != Some(directory)
        || path.extension().and_then(|extension| extension.to_str()) != Some("json")
    {
        bail!("embedding_qualification_start_gate_untrusted");
    }
    Ok(())
}

fn wait_for_gate(clock: &dyn AwakeMonotonicClock, path: &Path) -> Result<()> {
    let started = clock.now_ns();
    loop {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(());
            }
            Ok(_) => bail!("embedding_qualification_start_gate_untrusted"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect embedding qualification start gate"),
        }
        if elapsed(clock, started) >= CONTROL_TIMEOUT {
            bail!("embedding_qualification_start_gate_timeout");
        }
        clock.sleep(POLL);
    }
}

fn wait_for_process_start(clock: &dyn AwakeMonotonicClock, pid: u32) -> Result<String> {
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

fn current_process_start_identity() -> Result<String> {
    match codestory_retrieval::probe_process_start_identity(std::process::id()) {
        ProcessStartProbe::Running { start_identity } => Ok(start_identity),
        ProcessStartProbe::NotRunning => bail!("embedding_qualification_worker_not_running"),
        ProcessStartProbe::Unknown { reason } => {
            bail!("embedding_qualification_worker_identity_unknown:{reason}")
        }
    }
}

fn wait_for_process_exit(
    clock: &dyn AwakeMonotonicClock,
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

fn wait_for_child(
    clock: &dyn AwakeMonotonicClock,
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

fn cleanup_worker_files(worker: &RunningWorker) {
    let _ = fs::remove_file(&worker.request_path);
    let _ = fs::remove_file(&worker.output_path);
}

fn validate_worker_output(
    output: &WorkerOutput,
    invocation: &ProcessInvocation,
    executable_sha256: &str,
) -> Result<()> {
    if output.schema_version != 1
        || output.pid != invocation.pid
        || output.process_start_id != invocation.process_start_id
        || output.executable_sha256 != executable_sha256
        || output.project_identity_sha256 != invocation.project_identity_sha256
        || output.clock.domain != "awake_monotonic"
        || output.clock.boot_id.is_empty()
        || output.started_ns > output.finished_ns
        || (output.result.is_some() as u8
            + output.protocol_exchange.is_some() as u8
            + output.queue_operations.is_some() as u8
            + output.error.is_some() as u8)
            != 1
    {
        bail!("embedding_qualification_worker_output_invalid");
    }
    Ok(())
}

fn require_worker_success(output: &WorkerOutput, phase: &str) -> Result<()> {
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

fn require_worker_error(output: &WorkerOutput, expected: &str, phase: &str) -> Result<()> {
    if output.error.as_ref().map(|error| error.code.as_str()) != Some(expected) {
        bail!("embedding_qualification_worker_error_missing:{phase}:{expected}");
    }
    Ok(())
}

fn require_protocol_success(output: &WorkerOutput, phase: &str) -> Result<()> {
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

fn worker_error(error: &anyhow::Error) -> WorkerError {
    let code = error
        .chain()
        .find_map(|cause| {
            cause
                .downcast_ref::<PerUserEmbeddingError>()
                .map(|error| error.code.clone())
        })
        .unwrap_or_else(|| error_head(error));
    WorkerError {
        code,
        message_head: error_head(error),
    }
}

fn error_head(error: &anyhow::Error) -> String {
    error
        .to_string()
        .split([':', '\n'])
        .next()
        .unwrap_or("embedding_qualification_failed")
        .chars()
        .take(128)
        .collect()
}

fn query_parameters(count: u32) -> EmbeddingQualificationParameters {
    EmbeddingQualificationParameters {
        query_count: count,
        bulk_count: 0,
        documents_per_bulk: 0,
        input_bytes: 64,
        hold_ms: 0,
    }
}

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

fn validate_product_vector(vector: &[f32], phase: &str) -> Result<()> {
    if vector.len() != codestory_retrieval::semantic_vector_dim()
        || vector.iter().any(|value| !value.is_finite())
    {
        bail!("embedding_qualification_product_vector_invalid:{phase}");
    }
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if !(0.98..=1.02).contains(&norm) {
        bail!("embedding_qualification_product_vector_not_normalized:{phase}");
    }
    Ok(())
}

fn scheduler_values(snapshot: &EmbeddingServerSnapshot) -> BTreeMap<String, Value> {
    btree([
        ("query_capacity", json!(snapshot.scheduler.query_capacity)),
        ("query_depth", json!(snapshot.scheduler.query_depth)),
        ("bulk_capacity", json!(snapshot.scheduler.bulk_capacity)),
        ("bulk_depth", json!(snapshot.scheduler.bulk_depth)),
        (
            "active_request_count",
            json!(snapshot.scheduler.active_request_count),
        ),
        (
            "active_request_class",
            json!(
                snapshot
                    .scheduler
                    .active_request
                    .as_ref()
                    .map(|active| active.class.as_str())
            ),
        ),
    ])
}

struct QueueAnalysis {
    capacity: BTreeMap<String, Value>,
    class_orders: BTreeMap<String, Value>,
    project_orders: BTreeMap<String, Value>,
    query_preference: BTreeMap<String, Value>,
    bulk_resumption: BTreeMap<String, Value>,
}

fn analyze_queue_operations(operations: &[WorkerQueueOperation]) -> Result<QueueAnalysis> {
    let first = operations
        .first()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_queue_operations_missing"))?;
    if operations.iter().any(|operation| {
        !same_server_authority(&first.hello_snapshot, &operation.hello_snapshot)
            || (operation.status == "ok"
                && (operation.error.is_some() || operation.response_payload_bytes == 0))
            || (operation.status == "failed"
                && (operation.error.is_none() || operation.response_payload_bytes != 0))
    }) {
        bail!("embedding_qualification_queue_operation_identity_invalid");
    }
    let mut capacity = BTreeMap::new();
    let mut class_orders = BTreeMap::new();
    let mut project_orders = BTreeMap::new();
    let mut completed_by_class = BTreeMap::<&str, Vec<&WorkerQueueOperation>>::new();
    for class in ["query", "bulk"] {
        let class_operations = operations
            .iter()
            .filter(|operation| operation.class == class)
            .collect::<Vec<_>>();
        if class_operations.len() != MIXED_QUEUE_COUNT as usize {
            bail!("embedding_qualification_queue_operation_count_invalid:{class}");
        }
        let failures = class_operations
            .iter()
            .copied()
            .filter(|operation| operation.status == "failed")
            .collect::<Vec<_>>();
        if failures.len() != 1 {
            bail!("embedding_qualification_queue_capacity_failure_count:{class}");
        }
        let pressure = failures[0]
            .error
            .as_ref()
            .and_then(|error| error.capacity.as_ref())
            .ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_queue_capacity_untyped:{class}")
            })?;
        if pressure.queue_class != class
            || pressure.capacity != 64
            || pressure.depth != pressure.capacity
            || pressure.retry_condition.trim().is_empty()
        {
            bail!("embedding_qualification_queue_capacity_contract_invalid:{class}");
        }
        capacity.insert(
            format!("{class}_65th"),
            json!({
                "correlation_id": failures[0].correlation_id,
                "error": failures[0].error,
                "submitted_ns": failures[0].submitted_ns,
                "completed_ns": failures[0].completed_ns,
            }),
        );
        let mut submitted = class_operations
            .iter()
            .copied()
            .filter(|operation| operation.status == "ok")
            .collect::<Vec<_>>();
        submitted.sort_by_key(|operation| (operation.submitted_ns, &operation.correlation_id));
        let mut completed = submitted.clone();
        completed.sort_by_key(|operation| (operation.completed_ns, &operation.correlation_id));
        let submitted_ids = submitted
            .iter()
            .map(|operation| operation.correlation_id.clone())
            .collect::<Vec<_>>();
        let completed_ids = completed
            .iter()
            .map(|operation| operation.correlation_id.clone())
            .collect::<Vec<_>>();
        if submitted_ids != completed_ids {
            bail!("embedding_qualification_queue_fifo_violation:{class}");
        }
        let submitted_projects = submitted
            .iter()
            .map(|operation| operation.project_identity_sha256.clone())
            .collect::<Vec<_>>();
        let completed_projects = completed
            .iter()
            .map(|operation| operation.project_identity_sha256.clone())
            .collect::<Vec<_>>();
        if submitted_projects != completed_projects
            || submitted_projects.iter().collect::<BTreeSet<_>>().len() != 2
        {
            bail!("embedding_qualification_queue_scope_order_invalid:{class}");
        }
        class_orders.insert(
            format!("{class}_submitted_request_ids"),
            json!(submitted_ids),
        );
        class_orders.insert(
            format!("{class}_completed_request_ids"),
            json!(completed_ids),
        );
        project_orders.insert(
            format!("{class}_submitted_project_identities"),
            json!(submitted_projects),
        );
        project_orders.insert(
            format!("{class}_completed_project_identities"),
            json!(completed_projects),
        );
        completed_by_class.insert(class, completed);
    }
    let queries = &completed_by_class["query"];
    let bulks = &completed_by_class["bulk"];
    let first_query = queries
        .first()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_query_completion_missing"))?;
    let first_bulk = bulks
        .first()
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_bulk_completion_missing"))?;
    if first_query.completed_ns >= first_bulk.completed_ns {
        bail!("embedding_qualification_query_preference_missing");
    }
    let last_query = queries.last().expect("non-empty query completions");
    let last_bulk = bulks.last().expect("non-empty bulk completions");
    if last_bulk.completed_ns <= last_query.completed_ns {
        bail!("embedding_qualification_bulk_resumption_missing");
    }
    Ok(QueueAnalysis {
        capacity,
        class_orders,
        project_orders,
        query_preference: btree([
            ("first_query_request_id", json!(first_query.correlation_id)),
            ("first_query_completed_ns", json!(first_query.completed_ns)),
            ("first_bulk_request_id", json!(first_bulk.correlation_id)),
            ("first_bulk_completed_ns", json!(first_bulk.completed_ns)),
        ]),
        bulk_resumption: btree([
            ("last_query_request_id", json!(last_query.correlation_id)),
            ("last_query_completed_ns", json!(last_query.completed_ns)),
            ("last_bulk_request_id", json!(last_bulk.correlation_id)),
            ("last_bulk_completed_ns", json!(last_bulk.completed_ns)),
        ]),
    })
}

fn same_server_authority(
    first: &EmbeddingServerSnapshot,
    second: &EmbeddingServerSnapshot,
) -> bool {
    first.process.server_instance_id == second.process.server_instance_id
        && first.process.pid == second.process.pid
        && first.process.process_start_id == second.process.process_start_id
        && first.authority.lifetime_authority_id == second.authority.lifetime_authority_id
        && first.authority.listener_id == second.authority.listener_id
}

fn control_key(action: &str, class: Option<&str>) -> String {
    class.map_or_else(|| action.into(), |class| format!("{action}:{class}"))
}

fn qualification_request_id(prefix: &str, now_ns: u64) -> String {
    format!("{prefix}-{}-{now_ns}", std::process::id())
}

fn project_identity_sha256(runtime: &SidecarRuntimeConfig) -> String {
    let seed = runtime
        .project_identity
        .as_ref()
        .map(|identity| format!("{}:{}", identity.project_id, identity.workspace_id))
        .unwrap_or_else(|| runtime.namespace.clone());
    sha256_bytes(seed.as_bytes())
}

fn elapsed(clock: &dyn AwakeMonotonicClock, started_ns: u64) -> Duration {
    Duration::from_nanos(clock.now_ns().saturating_sub(started_ns))
}

fn btree<const N: usize>(entries: [(&str, Value); N]) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generic_operation_alias_cannot_satisfy_any_named_scenario() {
        let mut generic = ScenarioEvidence::default();
        generic.transitions.insert("generic_query_completed".into());
        generic
            .transitions
            .insert("generic_observe_completed".into());
        for scenario in REQUIRED_SCENARIOS {
            let error = validate_named_evidence(scenario, &generic)
                .expect_err("generic evidence must not satisfy a named scenario");
            assert!(
                error
                    .to_string()
                    .contains("embedding_qualification_named_evidence_incomplete")
            );
        }
    }

    #[test]
    fn named_scenarios_require_their_fault_controls() {
        let cases = [
            ("frozen_owner", "freeze_owner"),
            ("incompatible_owner", "force_incompatible"),
            ("mixed_queue", "hold_class:query"),
            ("server_crash", "crash_server"),
            ("worker_stall", "stall_native"),
        ];
        for (scenario, required_control) in cases {
            let mut evidence = complete_evidence(scenario);
            evidence.controls.remove(required_control);
            assert!(validate_named_evidence(scenario, &evidence).is_err());
        }
    }

    #[test]
    fn scenario_artifact_schema_has_raw_fields_without_verdicts() {
        let value = serde_json::to_value(ScenarioArtifact {
            schema_version: 2,
            scenario: "cold_race".into(),
            contracts: QualificationContracts {
                protocol_sha256: "a".repeat(64),
                constant_set_sha256: "b".repeat(64),
                measurement_protocol_sha256: "c".repeat(64),
            },
            orchestration: ScenarioOrchestration {
                started_ns: 1,
                finished_ns: 2,
                process_invocations: Vec::new(),
            },
            control_events: Vec::new(),
            process_observations: Vec::new(),
            observations: Vec::new(),
            events: Vec::new(),
        })
        .expect("serialize scenario artifact");
        let object = value.as_object().expect("artifact object");
        assert_eq!(
            object.keys().cloned().collect::<BTreeSet<_>>(),
            [
                "schema_version",
                "scenario",
                "contracts",
                "orchestration",
                "control_events",
                "process_observations",
                "observations",
                "events",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        for forbidden in ["status", "pass", "passed", "assertions", "core_scenario"] {
            assert!(!object.contains_key(forbidden));
        }
    }

    fn complete_evidence(scenario: &str) -> ScenarioEvidence {
        let (controls, transitions): (&[&str], &[&str]) = match scenario {
            "frozen_owner" => (
                &["freeze_owner", "release_owner"],
                &["bounded_owner_unresponsive", "owner_identity_stable"],
            ),
            "incompatible_owner" => (
                &["force_incompatible"],
                &[
                    "active_owner_rejected",
                    "idle_owner_draining",
                    "compatible_replacement",
                ],
            ),
            "mixed_queue" => (
                &[
                    "hold_class:bulk",
                    "hold_class:query",
                    "release_class:bulk",
                    "release_class:query",
                ],
                &[
                    "queues_saturated",
                    "query_selected_before_bulk_backlog",
                    "typed_capacity_retry_observed",
                    "per_class_fifo_observed",
                    "global_fifo_across_projects",
                    "query_preference_observed",
                    "bulk_resumed",
                ],
            ),
            "server_crash" => (
                &["hold_class:query", "crash_server"],
                &[
                    "inflight_request_observed",
                    "server_replaced",
                    "query_replayed",
                ],
            ),
            "worker_stall" => (
                &["stall_native"],
                &[
                    "stalled_request_observed",
                    "watchdog_fail_stop_observed",
                    "post_stall_replacement",
                ],
            ),
            _ => unreachable!("test covers fault-controlled scenarios"),
        };
        ScenarioEvidence {
            controls: controls.iter().map(|value| (*value).into()).collect(),
            transitions: transitions.iter().map(|value| (*value).into()).collect(),
        }
    }
}
