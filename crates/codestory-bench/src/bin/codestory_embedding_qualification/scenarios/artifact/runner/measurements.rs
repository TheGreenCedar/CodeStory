use super::super::{
    MeasurementArtifact, MeasurementInterval, POLL, ProcessObservation,
    QUALIFICATION_QUEUE_CAPACITY, RawMetric, RawMetricClock, RawMetricProcess,
    RawMetricSampleInput, successful_operation_duration_ns, successful_operation_operands,
};
use super::analysis::{
    accelerator_operands, completed_token_count, elapsed, raw_server_identity,
    snapshot_has_resident_generation,
};
use super::process::{
    busy_retry_marker_timeout, busy_retry_worker_timeout, measurement_worker_timeout,
    query_parameters, require_worker_success,
};
use super::{RunningWorker, ScenarioRunner, WorkerOutput, push_metric};
use crate::qualification::request::REQUIRED_METRICS;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    EmbeddingCapacityPressureWire, EmbeddingEngineIdentity, EmbeddingQualificationParameters,
    EmbeddingQualificationWorkerMeasurementSpan as WorkerMeasurementSpan, EmbeddingServerSnapshot,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

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

        let idle_worker = self.spawn_worker("query", query_parameters(1), None)?;
        let idle_output = self.finish_worker(idle_worker, measurement_worker_timeout("query"))?;
        require_worker_success(&idle_output, "true_idle_owner")?;
        let idle_owner =
            self.record_worker_snapshot("measurement_true_idle_owner", &idle_output)?;
        if !snapshot_has_resident_generation(&idle_owner) {
            bail!("embedding_qualification_true_idle_owner_not_resident");
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

        if metrics.len() != REQUIRED_METRICS.len().saturating_sub(2) {
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

    pub(super) fn measurement_sample_id(&self, metric: &str, repeat: u32) -> String {
        super::opaque_measurement_sample_id(
            self.context.nonce_sha256,
            &self.context.qualification_runtime.matrix_cell_id,
            metric,
            repeat,
        )
    }
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
