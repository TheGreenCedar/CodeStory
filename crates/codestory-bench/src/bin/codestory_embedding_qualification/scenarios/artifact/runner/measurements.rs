use super::super::{
    ConstantCalibrationRunArtifact, MeasurementArtifact, MeasurementInterval, POLL,
    ProcessObservation, QUALIFICATION_QUEUE_CAPACITY, RawMetric, RawMetricClock, RawMetricProcess,
    RawMetricSampleInput, RawServerIdentity, successful_operation_duration_ns,
    successful_operation_operands,
};
use super::analysis::{
    accelerator_operands, completed_token_count, completed_token_count_for_nonce, elapsed,
    raw_server_identity, snapshot_has_resident_generation,
};
use super::process::{
    busy_retry_marker_timeout, busy_retry_worker_timeout, measurement_worker_timeout,
    query_parameters,
};
use super::{
    RunningWorker, ScenarioRunner, WorkerOutput, opaque_constant_calibration_sample_id, push_metric,
};
use crate::qualification::request::REQUIRED_METRICS;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    EmbeddingCapacityPressureWire, EmbeddingEngineIdentity, EmbeddingQualificationParameters,
    EmbeddingQualificationWorkerMeasurementSpan as WorkerMeasurementSpan, EmbeddingServerSnapshot,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

pub(super) const CONSTANT_CALIBRATION_METRICS: &[&str] = &[
    "spawn_convergence",
    "existing_owner_connect",
    "cold_first_vector",
    "first_product_ready",
    "warm_query_ipc",
    "warm_bulk_ipc",
    "bulk_documents_per_second",
    "bulk_tokens_per_second",
    "busy_retry_usefulness",
];

/// The measurement protocol's declared `phase_boundaries` for every metric the
/// driver records. The WP5-floored frozen constants derive their meaning from
/// these instants, so the driver stamps exactly these names onto each sample
/// and the measurement workers observe exactly these windows; the checker
/// rejects anything else.
pub(super) fn declared_phase_boundaries(metric: &str) -> Result<[&'static str; 2]> {
    Ok(match metric {
        "existing_owner_connect" => ["client_connect_started", "compatible_hello_validated"],
        "spawn_convergence" => ["owner_absence_proven", "compatible_hello_validated"],
        "cold_first_vector" => [
            "product_request_started_with_owner_absent",
            "first_vector_and_engine_evidence_validated",
        ],
        "first_product_ready" => ["product_request_started", "product_result_validated"],
        "warm_query_ipc" => [
            "client_frame_started",
            "query_response_identity_and_vector_validated",
        ],
        "warm_bulk_ipc" => [
            "client_frame_started",
            "bulk_response_identity_and_vectors_validated",
        ],
        "bulk_documents_per_second" => [
            "bulk_measurement_window_started",
            "bulk_document_results_validated",
        ],
        "bulk_tokens_per_second" => [
            "bulk_measurement_window_started",
            "bulk_token_results_validated",
        ],
        "busy_retry_usefulness" => ["typed_retry_emitted", "named_retry_condition_became_true"],
        "true_idle_exit" => [
            "last_queued_active_or_leased_work_ended",
            "engine_and_server_absent",
        ],
        "backend_observed_accelerator_residency" => [
            "accelerator_measurement_started",
            "backend_residency_evidence_validated",
        ],
        _ => bail!("embedding_qualification_metric_phases_unknown:{metric}"),
    })
}

/// The measurement protocol's declared workload id for every metric the
/// driver records.
pub(super) fn declared_workload_id(metric: &str) -> Result<&'static str> {
    Ok(match metric {
        "existing_owner_connect" => "compatible_hello_existing_owner_v1",
        "spawn_convergence" => "compatible_hello_absent_owner_v1",
        "cold_first_vector" => "cold_query_256b_v1",
        "first_product_ready" => "product_query_256b_v1",
        "warm_query_ipc" => "warm_query_256b_v1",
        "warm_bulk_ipc" => "warm_bulk_64x256b_v1",
        "bulk_documents_per_second" | "bulk_tokens_per_second" => "bulk_throughput_256x256b_v1",
        "busy_retry_usefulness" => "saturated_query_65th_retry_v1",
        "true_idle_exit" => "true_idle_60000_awake_ms_v1",
        "backend_observed_accelerator_residency" => "resident_policy_identity_v1",
        _ => bail!("embedding_qualification_metric_workload_unknown:{metric}"),
    })
}

/// One finished measurement worker: the declared-instant interval plus the
/// evidence the worker observed for the sample.
pub(super) struct MeasuredWorker {
    pub(super) interval: MeasurementInterval,
    pub(super) snapshot: EmbeddingServerSnapshot,
    pub(super) engine_identity: Option<EmbeddingEngineIdentity>,
    pub(super) request_id: Option<String>,
    pub(super) completed_documents: Option<u64>,
    pub(super) pressure: Option<EmbeddingCapacityPressureWire>,
}

impl<'a> ScenarioRunner<'a> {
    pub(super) fn constant_calibration_runs(
        &mut self,
        required_runs: u32,
        model_sha256: &str,
    ) -> Result<Vec<ConstantCalibrationRunArtifact>> {
        if required_runs != 3 {
            bail!("embedding_constant_calibration_run_count_invalid");
        }
        let mut observed_identities = BTreeSet::new();
        let mut runs = Vec::with_capacity(required_runs as usize);
        for run_index in 1..=required_runs {
            let run = self.constant_calibration_run(run_index, model_sha256)?;
            retain_fresh_server_identities(&mut observed_identities, run.server_identities())?;
            runs.push(run);
        }
        self.reset_owner("constant_calibration_finish")?;
        Ok(runs)
    }

    fn constant_calibration_run(
        &mut self,
        run_index: u32,
        model_sha256: &str,
    ) -> Result<ConstantCalibrationRunArtifact> {
        let mut metrics = BTreeMap::new();
        let mut sampled_identities = BTreeSet::new();
        let expected_materialized_reused = expected_materialization_reuse(run_index)?;

        self.reset_owner(&format!("constant_calibration_run_{run_index}_start"))?;
        let spawn = self.run_measure_worker(
            "measure_constant_spawn_hello",
            "spawn_convergence",
            1,
            query_parameters(1),
        )?;
        let cold = self.run_measure_worker(
            "measure_constant_cold_query",
            "cold_first_vector",
            1,
            measurement_parameters(1, 0, 0, 256),
        )?;
        let cold_identity = raw_server_identity(&cold.snapshot)?;

        let first_ready = self.run_measure_worker(
            "measure_product_query",
            "first_product_ready",
            1,
            measurement_parameters(1, 0, 0, 256),
        )?;
        let first_ready_identity = raw_server_identity(&first_ready.snapshot)?;

        let warm_query = self.run_measure_worker(
            "measure_query_frame",
            "warm_query_ipc",
            1,
            measurement_parameters(1, 0, 0, 256),
        )?;
        require_completed_documents(&warm_query, 1)?;
        let warm_query_identity = frame_server_identity(&warm_query)?;
        let engine = require_constant_engine_identity(
            &warm_query,
            &warm_query_identity,
            self.context.qualification_runtime.expected_backend.as_str(),
            model_sha256,
            expected_materialized_reused,
        )?;
        let engine_backend = engine.backend.clone();
        let engine_policy = engine.policy.clone();
        let engine_model_sha256 = engine.model_digest.clone();
        let engine_materialized_reused = engine.materialized_reused;
        if spawn.snapshot.process.server_instance_id != warm_query_identity.server_instance_id
            || spawn.snapshot.process.process_start_id != warm_query_identity.process_start_id
            || cold_identity != warm_query_identity
            || first_ready_identity != warm_query_identity
        {
            bail!("embedding_constant_calibration_generation_changed");
        }

        sampled_identities.insert(warm_query_identity.clone());
        self.record_constant_metric(
            &mut metrics,
            "spawn_convergence",
            run_index,
            &spawn.interval,
            warm_query_identity.clone(),
            BTreeMap::new(),
        )?;
        sampled_identities.insert(cold_identity.clone());
        self.record_constant_metric(
            &mut metrics,
            "cold_first_vector",
            run_index,
            &cold.interval,
            cold_identity,
            successful_operation_operands(&cold.interval),
        )?;
        sampled_identities.insert(first_ready_identity.clone());
        self.record_constant_metric(
            &mut metrics,
            "first_product_ready",
            run_index,
            &first_ready.interval,
            first_ready_identity,
            successful_operation_operands(&first_ready.interval),
        )?;
        self.record_constant_metric(
            &mut metrics,
            "warm_query_ipc",
            run_index,
            &warm_query.interval,
            warm_query_identity.clone(),
            successful_operation_operands(&warm_query.interval),
        )?;

        let existing = self.run_measure_worker(
            "measure_hello",
            "existing_owner_connect",
            1,
            query_parameters(1),
        )?;
        let existing_identity = raw_server_identity(&existing.snapshot)?;
        sampled_identities.insert(existing_identity.clone());
        self.record_constant_metric(
            &mut metrics,
            "existing_owner_connect",
            run_index,
            &existing.interval,
            existing_identity,
            BTreeMap::new(),
        )?;

        let warm_bulk = self.run_measure_worker(
            "measure_bulk_frame",
            "warm_bulk_ipc",
            1,
            measurement_parameters(0, 1, 64, 256),
        )?;
        require_completed_documents(&warm_bulk, 64)?;
        let warm_bulk_identity = frame_server_identity(&warm_bulk)?;
        require_constant_engine_identity(
            &warm_bulk,
            &warm_bulk_identity,
            self.context.qualification_runtime.expected_backend.as_str(),
            model_sha256,
            expected_materialized_reused,
        )?;
        sampled_identities.insert(warm_bulk_identity.clone());
        self.record_constant_metric(
            &mut metrics,
            "warm_bulk_ipc",
            run_index,
            &warm_bulk.interval,
            warm_bulk_identity,
            successful_operation_operands(&warm_bulk.interval),
        )?;

        let throughput = self.run_measure_worker(
            "measure_bulk_frame",
            "bulk_documents_per_second",
            1,
            measurement_parameters(0, 1, 256, 256),
        )?;
        require_completed_documents(&throughput, 256)?;
        let throughput_identity = frame_server_identity(&throughput)?;
        require_constant_engine_identity(
            &throughput,
            &throughput_identity,
            self.context.qualification_runtime.expected_backend.as_str(),
            model_sha256,
            expected_materialized_reused,
        )?;
        let request_id = throughput.request_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!("embedding_constant_calibration_bulk_request_id_missing")
        })?;
        let completed_tokens = completed_token_count_for_nonce(
            self.context.output_directory,
            request_id,
            &self.qualification_nonce,
        )?;
        let duration_ns = successful_operation_duration_ns(&throughput.interval);
        sampled_identities.insert(throughput_identity.clone());
        self.record_constant_metric(
            &mut metrics,
            "bulk_documents_per_second",
            run_index,
            &throughput.interval,
            throughput_identity.clone(),
            BTreeMap::from([
                ("completed_documents".into(), json!(256)),
                (
                    "successful_operation_duration_ns".into(),
                    json!(duration_ns),
                ),
            ]),
        )?;
        self.record_constant_metric(
            &mut metrics,
            "bulk_tokens_per_second",
            run_index,
            &throughput.interval,
            throughput_identity,
            BTreeMap::from([
                ("completed_tokens".into(), json!(completed_tokens)),
                (
                    "successful_operation_duration_ns".into(),
                    json!(duration_ns),
                ),
            ]),
        )?;

        let busy = self.measure_busy_retry(1)?;
        let busy_identity = raw_server_identity(&busy.snapshot)?;
        sampled_identities.insert(busy_identity.clone());
        self.record_constant_metric(
            &mut metrics,
            "busy_retry_usefulness",
            run_index,
            &busy.interval,
            busy_identity,
            BTreeMap::new(),
        )?;

        if metrics.len() != CONSTANT_CALIBRATION_METRICS.len()
            || metrics.keys().map(String::as_str).collect::<BTreeSet<_>>()
                != CONSTANT_CALIBRATION_METRICS
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>()
            || metrics.values().any(|metric| metric.samples.len() != 1)
        {
            bail!("embedding_constant_calibration_measurement_set_invalid");
        }
        require_one_run_server_identity(&sampled_identities, &warm_query_identity)?;

        Ok(ConstantCalibrationRunArtifact {
            schema_version: 1,
            run_index,
            contracts: self.context.contracts.clone(),
            metrics,
            server_identities: sampled_identities.into_iter().collect(),
            backend: engine_backend,
            policy: engine_policy,
            model_sha256: engine_model_sha256,
            materialized_reused: engine_materialized_reused,
        })
    }

    pub(super) fn measurements(&mut self) -> Result<MeasurementArtifact> {
        let mut metrics = BTreeMap::new();

        for repeat in 1..=3 {
            self.reset_owner(&format!("measure_spawn_no_owner_{repeat}"))?;
            let measured = self.run_measure_worker(
                "measure_spawn_hello",
                "spawn_convergence",
                repeat,
                query_parameters(1),
            )?;
            let identity = raw_server_identity(&measured.snapshot)?;
            self.record_metric(
                &mut metrics,
                "spawn_convergence",
                repeat,
                &measured.interval,
                identity,
                BTreeMap::new(),
            )?;
        }

        for repeat in 1..=3 {
            let measured = self.run_measure_worker(
                "measure_hello",
                "existing_owner_connect",
                repeat,
                query_parameters(1),
            )?;
            let identity = raw_server_identity(&measured.snapshot)?;
            self.record_metric(
                &mut metrics,
                "existing_owner_connect",
                repeat,
                &measured.interval,
                identity,
                BTreeMap::new(),
            )?;
        }

        for repeat in 1..=3 {
            self.reset_owner(&format!("measure_cold_no_owner_{repeat}"))?;
            let measured = self.run_measure_worker(
                "measure_product_query",
                "cold_first_vector",
                repeat,
                measurement_parameters(1, 0, 0, 256),
            )?;
            let operands = successful_operation_operands(&measured.interval);
            let identity = raw_server_identity(&measured.snapshot)?;
            self.record_metric(
                &mut metrics,
                "cold_first_vector",
                repeat,
                &measured.interval,
                identity,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let measured = self.run_measure_worker(
                "measure_product_query",
                "first_product_ready",
                repeat,
                measurement_parameters(1, 0, 0, 256),
            )?;
            let operands = successful_operation_operands(&measured.interval);
            let identity = raw_server_identity(&measured.snapshot)?;
            self.record_metric(
                &mut metrics,
                "first_product_ready",
                repeat,
                &measured.interval,
                identity,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let measured = self.run_measure_worker(
                "measure_query_frame",
                "warm_query_ipc",
                repeat,
                measurement_parameters(1, 0, 0, 256),
            )?;
            require_completed_documents(&measured, 1)?;
            let operands = successful_operation_operands(&measured.interval);
            let identity = frame_server_identity(&measured)?;
            self.record_metric(
                &mut metrics,
                "warm_query_ipc",
                repeat,
                &measured.interval,
                identity,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let measured = self.run_measure_worker(
                "measure_bulk_frame",
                "warm_bulk_ipc",
                repeat,
                measurement_parameters(0, 1, 64, 256),
            )?;
            require_completed_documents(&measured, 64)?;
            let operands = successful_operation_operands(&measured.interval);
            let identity = frame_server_identity(&measured)?;
            self.record_metric(
                &mut metrics,
                "warm_bulk_ipc",
                repeat,
                &measured.interval,
                identity,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let measured = self.run_measure_worker(
                "measure_bulk_frame",
                "bulk_documents_per_second",
                repeat,
                measurement_parameters(0, 1, 256, 256),
            )?;
            require_completed_documents(&measured, 256)?;
            let request_id = measured.request_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_bulk_request_id_missing")
            })?;
            let completed_tokens =
                completed_token_count(self.context.output_directory, request_id)?;
            let duration_ns = successful_operation_duration_ns(&measured.interval);
            let identity = frame_server_identity(&measured)?;
            self.record_metric(
                &mut metrics,
                "bulk_documents_per_second",
                repeat,
                &measured.interval,
                identity.clone(),
                BTreeMap::from([
                    ("completed_documents".into(), json!(256)),
                    (
                        "successful_operation_duration_ns".into(),
                        json!(duration_ns),
                    ),
                ]),
            )?;
            self.record_metric(
                &mut metrics,
                "bulk_tokens_per_second",
                repeat,
                &measured.interval,
                identity,
                BTreeMap::from([
                    ("completed_tokens".into(), json!(completed_tokens)),
                    (
                        "successful_operation_duration_ns".into(),
                        json!(duration_ns),
                    ),
                ]),
            )?;
        }

        let measured = self.run_measure_worker(
            "measure_resident_identity",
            "backend_observed_accelerator_residency",
            1,
            query_parameters(1),
        )?;
        let engine = measured
            .engine_identity
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_residency_identity_missing"))?;
        let identity = raw_server_identity(&measured.snapshot)?;
        self.record_metric(
            &mut metrics,
            "backend_observed_accelerator_residency",
            1,
            &measured.interval,
            identity,
            accelerator_operands(engine),
        )?;

        for repeat in 1..=3 {
            let measured = self.measure_busy_retry(repeat)?;
            let identity = raw_server_identity(&measured.snapshot)?;
            self.record_metric(
                &mut metrics,
                "busy_retry_usefulness",
                repeat,
                &measured.interval,
                identity,
                BTreeMap::new(),
            )?;
        }

        let measured = self.run_measure_worker(
            "measure_true_idle",
            "true_idle_exit",
            1,
            query_parameters(1),
        )?;
        if !snapshot_has_resident_generation(&measured.snapshot)
            || measured.snapshot.scheduler.active_request_count != 0
            || measured.snapshot.scheduler.query_depth != 0
            || measured.snapshot.scheduler.bulk_depth != 0
            || measured.snapshot.scheduler.lease_count != 0
        {
            bail!("embedding_qualification_true_idle_worker_witness_invalid");
        }
        let identity = raw_server_identity(&measured.snapshot)?;
        self.record_metric(
            &mut metrics,
            "true_idle_exit",
            1,
            &measured.interval,
            identity,
            BTreeMap::new(),
        )?;

        if metrics.len() != REQUIRED_METRICS.len().saturating_sub(1) {
            bail!("embedding_qualification_measurement_set_incomplete");
        }
        Ok(MeasurementArtifact {
            schema_version: 2,
            contracts: self.context.contracts.clone(),
            external_metrics: vec!["total_codestory_process_memory".into()],
            metrics,
        })
    }

    fn run_measure_worker(
        &mut self,
        operation: &str,
        metric: &str,
        repeat: u32,
        parameters: EmbeddingQualificationParameters,
    ) -> Result<MeasuredWorker> {
        let workload_id = declared_workload_id(metric)?;
        let worker = self.spawn_measure_worker(operation, parameters, workload_id, repeat, None)?;
        let output = self.finish_worker(worker, measurement_worker_timeout(operation))?;
        self.measured_worker(operation, &output)
    }

    fn measured_worker(
        &mut self,
        operation: &str,
        output: &WorkerOutput,
    ) -> Result<MeasuredWorker> {
        if let Some(error) = output.error.as_ref() {
            bail!(
                "embedding_qualification_measure_worker_failed:{operation}:{}",
                error.code
            );
        }
        let measurement = output.measurement.as_ref().ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_worker_measurement_missing:{operation}")
        })?;
        let interval = measurement_span_interval(output, &measurement.span)?;
        self.artifact
            .process_observations
            .push(ProcessObservation::from_snapshot(
                "measurement_worker",
                self.clock.now_ns(),
                Some(measurement.snapshot.clone()),
            ));
        Ok(MeasuredWorker {
            interval,
            snapshot: measurement.snapshot.clone(),
            engine_identity: measurement.engine_identity.clone(),
            request_id: measurement.request_id.clone(),
            completed_documents: measurement.completed_documents,
            pressure: measurement.pressure.clone(),
        })
    }

    /// The single-process saturated-65th-retry experiment: the driver holds
    /// the bulk and query classes, spawns the busy-retry worker, releases the
    /// classes once the worker's typed-retry marker appears, and validates
    /// the returned pressure evidence.
    fn measure_busy_retry(&mut self, repeat: u32) -> Result<MeasuredWorker> {
        self.ensure_owner("measurement_busy_owner")?;
        self.control("hold_class", Some("bulk"))?;
        self.control("hold_class", Some("query"))?;
        let marker = self
            .context
            .output_directory
            .join(format!(".measure-busy-retry-{repeat}.marker.json"));
        let mut worker = self.spawn_measure_worker(
            "measure_busy_retry",
            measurement_parameters(QUALIFICATION_QUEUE_CAPACITY as u32 + 1, 0, 0, 256),
            declared_workload_id("busy_retry_usefulness")?,
            repeat,
            Some(marker.clone()),
        )?;
        let marker_present =
            self.wait_for_retry_marker(&mut worker, &marker, busy_retry_marker_timeout())?;
        if marker_present {
            self.control("release_class", Some("bulk"))?;
            self.control("release_class", Some("query"))?;
        }
        let output = self.finish_worker(worker, busy_retry_worker_timeout())?;
        self.cleanup_gate(&marker);
        let measured = self.measured_worker("measure_busy_retry", &output)?;
        if !marker_present {
            // The worker exited before signalling the typed retry, so the
            // measured window cannot exist; `measured_worker` above surfaces
            // the worker's own error first.
            bail!("embedding_qualification_busy_retry_marker_missing");
        }
        let pressure = measured.pressure.as_ref().ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_busy_retry_pressure_missing")
        })?;
        if pressure.reason != "queue_full"
            || pressure.queue_class != "query"
            || pressure.capacity != QUALIFICATION_QUEUE_CAPACITY
            || pressure.depth != pressure.capacity
            || pressure.retry_condition.trim().is_empty()
        {
            bail!("embedding_qualification_busy_retry_pressure_invalid");
        }
        Ok(measured)
    }

    /// Poll for the busy-retry worker's typed-retry marker with the standard
    /// coordination cadence. Returns `false` when the worker exited before
    /// signalling, so the caller can surface the worker's own error.
    fn wait_for_retry_marker(
        &mut self,
        worker: &mut RunningWorker,
        marker: &Path,
        timeout: Duration,
    ) -> Result<bool> {
        let started = self.clock.now_ns();
        loop {
            match fs::symlink_metadata(marker) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    return Ok(true);
                }
                Ok(_) => bail!("embedding_qualification_busy_retry_marker_untrusted"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context("inspect embedding qualification retry marker");
                }
            }
            if worker
                .child
                .try_wait()
                .context("poll busy retry qualification worker")?
                .is_some()
            {
                return Ok(false);
            }
            if elapsed(&self.clock, started) >= timeout {
                bail!("embedding_qualification_busy_retry_marker_timeout");
            }
            self.clock.sleep(POLL);
        }
    }

    fn record_metric(
        &self,
        metrics: &mut BTreeMap<String, RawMetric>,
        metric: &str,
        repeat: u32,
        interval: &MeasurementInterval,
        server_identity: super::super::RawServerIdentity,
        operands: BTreeMap<String, serde_json::Value>,
    ) -> Result<()> {
        let [start_phase, end_phase] = declared_phase_boundaries(metric)?;
        let sample = interval.sample(RawMetricSampleInput {
            sample_id: &self.measurement_sample_id(metric, repeat),
            repeat,
            runtime: self.context.qualification_runtime,
            workload_id: declared_workload_id(metric)?,
            server_identity,
            start_phase,
            end_phase,
            operands,
        });
        push_metric(metrics, metric, metric_unit(metric), sample)
    }

    fn record_constant_metric(
        &self,
        metrics: &mut BTreeMap<String, RawMetric>,
        metric: &str,
        run_index: u32,
        interval: &MeasurementInterval,
        server_identity: RawServerIdentity,
        operands: BTreeMap<String, serde_json::Value>,
    ) -> Result<()> {
        let [start_phase, end_phase] = declared_constant_phase_boundaries(metric)?;
        let sample_id = opaque_constant_calibration_sample_id(
            self.context.nonce_sha256,
            &self.context.qualification_runtime.matrix_cell_id,
            run_index,
            metric,
        );
        let sample = interval.sample(RawMetricSampleInput {
            sample_id: &sample_id,
            repeat: 1,
            runtime: self.context.qualification_runtime,
            workload_id: declared_workload_id(metric)?,
            server_identity,
            start_phase,
            end_phase,
            operands,
        });
        push_metric(metrics, metric, metric_unit(metric), sample)
    }

    pub(super) fn measurement_sample_id(&self, metric: &str, repeat: u32) -> String {
        super::opaque_measurement_sample_id(
            self.context.nonce_sha256,
            &self.context.qualification_runtime.matrix_cell_id,
            metric,
            repeat,
        )
    }
}

fn declared_constant_phase_boundaries(metric: &str) -> Result<[&'static str; 2]> {
    if metric == "cold_first_vector" {
        return Ok([
            "product_request_started_with_fresh_owner_model_absent",
            "first_vector_and_engine_evidence_validated",
        ]);
    }
    declared_phase_boundaries(metric)
}

fn require_constant_engine_identity<'a>(
    measured: &'a MeasuredWorker,
    server_identity: &RawServerIdentity,
    expected_backend: &str,
    expected_model_sha256: &str,
    expected_materialized_reused: bool,
) -> Result<&'a EmbeddingEngineIdentity> {
    let identity = measured
        .engine_identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("embedding_constant_calibration_engine_identity_missing"))?;
    validate_constant_engine_evidence(
        identity,
        server_identity,
        expected_backend,
        expected_model_sha256,
        expected_materialized_reused,
    )?;
    if measured.snapshot.process.server_instance_id != identity.server_instance_id
        || measured
            .snapshot
            .engine
            .as_ref()
            .is_none_or(|engine| engine.load_generation != identity.load_generation)
    {
        bail!("embedding_constant_calibration_engine_identity_invalid");
    }
    Ok(identity)
}

fn validate_constant_engine_evidence(
    identity: &EmbeddingEngineIdentity,
    server_identity: &RawServerIdentity,
    expected_backend: &str,
    expected_model_sha256: &str,
    expected_materialized_reused: bool,
) -> Result<()> {
    if identity.server_instance_id != server_identity.server_instance_id
        || identity.load_generation != server_identity.load_generation
        || identity.load_generation == 0
        || identity.model_load_count == 0
        || identity.residency != "resident"
        || !identity.worker_alive
        || identity.load_error.is_some()
        || identity.policy != "accelerated"
        || identity.backend.eq_ignore_ascii_case("cpu")
        || !constant_backend_matches_expected(&identity.backend, expected_backend)
        || identity.model_digest != expected_model_sha256
        || identity.materialized_model_sha256 != expected_model_sha256
        || !identity.embedded_model
        || identity.materialized_reused != expected_materialized_reused
    {
        bail!("embedding_constant_calibration_engine_identity_invalid");
    }
    Ok(())
}

fn constant_backend_matches_expected(observed: &str, expected: &str) -> bool {
    let Some(expected_family) = constant_backend_family(expected) else {
        return false;
    };
    constant_backend_family(observed) == Some(expected_family)
}

fn constant_backend_family(value: &str) -> Option<&'static str> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "metal" | "mtl" => Some("metal"),
        "vulkan" => Some("vulkan"),
        value
            if value.strip_prefix("vulkan").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            }) =>
        {
            Some("vulkan")
        }
        _ => None,
    }
}

fn expected_materialization_reuse(run_index: u32) -> Result<bool> {
    match run_index {
        1 => Ok(false),
        2 | 3 => Ok(true),
        _ => bail!("embedding_constant_calibration_run_index_invalid"),
    }
}

fn retain_fresh_server_identities(
    observed: &mut BTreeSet<RawServerIdentity>,
    current: &[RawServerIdentity],
) -> Result<()> {
    if current.is_empty() {
        bail!("embedding_constant_calibration_server_identity_missing");
    }
    for identity in current {
        if !observed.insert(identity.clone()) {
            bail!("embedding_constant_calibration_server_identity_reused");
        }
    }
    Ok(())
}

fn require_one_run_server_identity(
    sampled: &BTreeSet<RawServerIdentity>,
    expected: &RawServerIdentity,
) -> Result<()> {
    if sampled != &BTreeSet::from([expected.clone()]) {
        bail!("embedding_constant_calibration_generation_changed");
    }
    Ok(())
}

/// Build the recorded interval from the worker's declared-instant span. The
/// span must lie inside the worker's whole-process window and share its boot
/// identity, so a span from another clock, boot, or process fails closed.
pub(super) fn measurement_span_interval(
    output: &WorkerOutput,
    span: &WorkerMeasurementSpan,
) -> Result<MeasurementInterval> {
    if span.awake_started_ns > span.awake_finished_ns
        || span.inclusive_started_ns > span.inclusive_finished_ns
        || span.boot_id_started != output.clock.boot_id
        || span.boot_id_finished != output.clock.boot_id
        || span.awake_started_ns < output.started_ns
        || span.awake_finished_ns > output.finished_ns
        || span.inclusive_started_ns < output.inclusive_started_ns
        || span.inclusive_finished_ns > output.inclusive_finished_ns
    {
        bail!("embedding_qualification_worker_measurement_clock_invalid");
    }
    Ok(MeasurementInterval {
        process: RawMetricProcess {
            pid: output.pid,
            process_start_id: output.process_start_id.clone(),
        },
        clock: RawMetricClock {
            domain: output.clock.domain.clone(),
            api: output.clock.api.clone(),
            boot_id: output.clock.boot_id.clone(),
            resolution_ns: output.clock.resolution_ns,
        },
        awake_started_ns: span.awake_started_ns,
        awake_finished_ns: span.awake_finished_ns,
        inclusive_clock_api: output.inclusive_clock_api.clone(),
        inclusive_started_ns: span.inclusive_started_ns,
        inclusive_finished_ns: span.inclusive_finished_ns,
        boot_id_started: span.boot_id_started.clone(),
        boot_id_finished: span.boot_id_finished.clone(),
    })
}

/// Frame measurements witness the server through the engine identity the
/// validated response carried, paired with the process identity from the
/// pre-window hello on the same connection.
fn frame_server_identity(measured: &MeasuredWorker) -> Result<super::super::RawServerIdentity> {
    let engine = measured
        .engine_identity
        .as_ref()
        .filter(|identity| identity.load_generation > 0)
        .ok_or_else(|| anyhow::anyhow!("embedding_qualification_frame_identity_missing"))?;
    if engine.server_instance_id != measured.snapshot.process.server_instance_id {
        bail!("embedding_qualification_frame_identity_mismatch");
    }
    Ok(super::super::RawServerIdentity {
        server_instance_id: engine.server_instance_id.clone(),
        process_start_id: measured.snapshot.process.process_start_id.clone(),
        load_generation: engine.load_generation,
    })
}

fn require_completed_documents(measured: &MeasuredWorker, expected: u64) -> Result<()> {
    if measured.completed_documents != Some(expected) {
        bail!("embedding_qualification_measurement_document_count_invalid");
    }
    Ok(())
}

fn measurement_parameters(
    query_count: u32,
    bulk_count: u32,
    documents_per_bulk: u32,
    input_bytes: u32,
) -> EmbeddingQualificationParameters {
    EmbeddingQualificationParameters {
        query_count,
        bulk_count,
        documents_per_bulk,
        input_bytes,
        hold_ms: 0,
    }
}

fn metric_unit(metric: &str) -> &'static str {
    match metric {
        "bulk_documents_per_second" => "documents_per_second",
        "bulk_tokens_per_second" => "tokens_per_second",
        "backend_observed_accelerator_residency" => "boolean",
        _ => "milliseconds",
    }
}

#[cfg(test)]
mod constant_calibration_tests {
    use super::{
        CONSTANT_CALIBRATION_METRICS, RawServerIdentity, declared_constant_phase_boundaries,
        declared_phase_boundaries, expected_materialization_reuse,
        opaque_constant_calibration_sample_id, require_one_run_server_identity,
        retain_fresh_server_identities, validate_constant_engine_evidence,
    };
    use codestory_retrieval::EmbeddingEngineIdentity;
    use std::collections::BTreeSet;

    fn server_identity(name: &str) -> RawServerIdentity {
        RawServerIdentity {
            server_instance_id: name.into(),
            process_start_id: format!("boot:{name}"),
            load_generation: 1,
        }
    }

    fn engine_identity(name: &str, backend: &str, reused: bool) -> EmbeddingEngineIdentity {
        EmbeddingEngineIdentity {
            server_instance_id: name.into(),
            load_generation: 1,
            model_load_count: 1,
            residency: "resident".into(),
            worker_alive: true,
            load_error: None,
            model_digest: "a".repeat(64),
            ggml_build_identity: "ggml-test".into(),
            backend: backend.into(),
            adapter_name: backend.into(),
            adapter_description: "test".into(),
            policy: "accelerated".into(),
            embedded_model: true,
            materialized_model_sha256: "a".repeat(64),
            materialized_reused: reused,
            initialization_ms: 1,
            smoke_ms: 1,
            adapter_memory_total: 0,
            adapter_memory_used_by_load: 0,
            execution_device_names: Vec::new(),
            execution_backend_names: Vec::new(),
            execution_observation_source: "test".into(),
            encode_count: 0,
            execution_node_count: 0,
            resident_accelerator_tensor_count: 0,
            resident_accelerator_tensor_bytes: 0,
            model_layer_count: 0,
            offloaded_layer_count: 0,
            accelerator_execution_verified: false,
        }
    }

    #[test]
    fn constant_plan_is_exactly_the_nine_runtime_constant_metrics() {
        assert_eq!(
            CONSTANT_CALIBRATION_METRICS,
            [
                "spawn_convergence",
                "existing_owner_connect",
                "cold_first_vector",
                "first_product_ready",
                "warm_query_ipc",
                "warm_bulk_ipc",
                "bulk_documents_per_second",
                "bulk_tokens_per_second",
                "busy_retry_usefulness",
            ]
        );
        for forbidden in [
            "true_idle_exit",
            "total_codestory_process_memory",
            "backend_observed_accelerator_residency",
            "retrieval_quality",
        ] {
            assert!(!CONSTANT_CALIBRATION_METRICS.contains(&forbidden));
        }
        assert_eq!(
            declared_constant_phase_boundaries("cold_first_vector")
                .expect("constant cold boundary")[0],
            "product_request_started_with_fresh_owner_model_absent"
        );
        assert_eq!(
            declared_phase_boundaries("cold_first_vector").expect("qualification cold boundary")[0],
            "product_request_started_with_owner_absent",
            "the full qualification protocol must remain unchanged"
        );
    }

    #[test]
    fn constant_sample_identity_is_unique_per_logical_run() {
        let first = opaque_constant_calibration_sample_id("nonce", "metal", 1, "warm_query_ipc");
        let second = opaque_constant_calibration_sample_id("nonce", "metal", 2, "warm_query_ipc");
        assert_ne!(first, second);
        assert_eq!(
            first,
            opaque_constant_calibration_sample_id("nonce", "metal", 1, "warm_query_ipc")
        );
    }

    #[test]
    fn materialized_model_is_new_once_then_reused_twice() {
        assert!(!expected_materialization_reuse(1).expect("run one"));
        assert!(expected_materialization_reuse(2).expect("run two"));
        assert!(expected_materialization_reuse(3).expect("run three"));
        for invalid in [0, 4] {
            assert!(expected_materialization_reuse(invalid).is_err());
        }
    }

    #[test]
    fn engine_precondition_binds_accelerated_backend_model_and_reuse_only() {
        let server = server_identity("server-a");
        let metal = engine_identity("server-a", "Metal", false);
        for (observed, expected) in [
            ("Metal", "metal"),
            ("MTL", "metal"),
            ("Vulkan", "vulkan"),
            ("Vulkan0", "vulkan"),
        ] {
            let identity = engine_identity("server-a", observed, false);
            validate_constant_engine_evidence(&identity, &server, expected, &"a".repeat(64), false)
                .unwrap_or_else(|error| {
                    panic!("{observed} must satisfy expected GPU family {expected}: {error}")
                });
        }
        for (observed, expected) in [
            ("CPU", "metal"),
            ("cpu_explicit", "metal"),
            ("", "metal"),
            ("unknown", "metal"),
            ("metal-cpu", "metal"),
            ("mtl0", "metal"),
            ("MTL", "vulkan"),
            ("Vulkan", "metal"),
            ("vulkan-cpu", "vulkan"),
        ] {
            let identity = engine_identity("server-a", observed, false);
            assert!(
                validate_constant_engine_evidence(
                    &identity,
                    &server,
                    expected,
                    &"a".repeat(64),
                    false,
                )
                .is_err(),
                "{observed} must not satisfy expected GPU family {expected}"
            );
        }

        let mut invalid = metal.clone();
        invalid.policy = "cpu_explicit".into();
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.backend = "CPU".into();
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        assert!(
            validate_constant_engine_evidence(&metal, &server, "vulkan", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.load_generation = 2;
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.model_load_count = 0;
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.residency = "absent".into();
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.worker_alive = false;
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.load_error = Some("model load failed".into());
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.model_digest = "b".repeat(64);
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.materialized_model_sha256 = "b".repeat(64);
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.materialized_reused = true;
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal.clone();
        invalid.embedded_model = false;
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
        invalid = metal;
        invalid.server_instance_id = "server-b".into();
        assert!(
            validate_constant_engine_evidence(&invalid, &server, "metal", &"a".repeat(64), false)
                .is_err()
        );
    }

    #[test]
    fn server_identities_must_be_disjoint_across_records() {
        let first = server_identity("server-a");
        let second = server_identity("server-b");
        let third = server_identity("server-c");
        let mut observed = BTreeSet::new();
        retain_fresh_server_identities(&mut observed, std::slice::from_ref(&first))
            .expect("first run");
        retain_fresh_server_identities(&mut observed, std::slice::from_ref(&second))
            .expect("second run");
        assert!(
            retain_fresh_server_identities(&mut observed, std::slice::from_ref(&first)).is_err()
        );
        retain_fresh_server_identities(&mut observed, std::slice::from_ref(&third))
            .expect("third fresh run");
        assert!(retain_fresh_server_identities(&mut observed, &[]).is_err());
    }

    #[test]
    fn every_metric_in_one_record_must_share_one_generation() {
        let expected = server_identity("server-a");
        require_one_run_server_identity(&BTreeSet::from([expected.clone()]), &expected)
            .expect("one generation");
        assert!(
            require_one_run_server_identity(
                &BTreeSet::from([expected.clone(), server_identity("server-b")]),
                &expected,
            )
            .is_err()
        );
        assert!(require_one_run_server_identity(&BTreeSet::new(), &expected).is_err());
    }
}
