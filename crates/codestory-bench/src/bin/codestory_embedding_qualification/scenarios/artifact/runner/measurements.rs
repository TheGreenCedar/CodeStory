use super::super::{
    IDLE_EXIT_GRACE, MeasurementArtifact, MeasurementInterval, RawMetric, RawMetricClock,
    RawMetricProcess, RawMetricSampleInput, SNAPSHOT_TIMEOUT, successful_operation_operands,
};
use super::analysis::{
    accelerator_operands, completed_token_count, raw_server_identity,
    snapshot_has_resident_generation,
};
use super::process::{query_parameters, require_worker_success};
use super::{ScenarioRunner, WorkerOutput, push_metric};
use crate::qualification::request::REQUIRED_METRICS;
use anyhow::{Result, bail};
use codestory_retrieval::{
    EmbeddingQualificationParameters, EmbeddingServerSnapshot,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn measurements(&mut self) -> Result<MeasurementArtifact> {
        let mut metrics = BTreeMap::new();

        for repeat in 1..=3 {
            self.reset_owner(&format!("measure_spawn_no_owner_{repeat}"))?;
            let (interval, snapshot, _) =
                self.run_measurement_worker("query", query_parameters(1))?;
            self.record_metric(
                &mut metrics,
                "spawn_convergence",
                "compatible_query_absent_owner_v1",
                repeat,
                interval,
                snapshot,
                BTreeMap::new(),
            )?;
        }

        for repeat in 1..=3 {
            let (interval, snapshot, _) =
                self.run_measurement_worker("observe", query_parameters(1))?;
            self.record_metric(
                &mut metrics,
                "existing_owner_connect",
                "observe_existing_owner_v1",
                repeat,
                interval,
                snapshot,
                BTreeMap::new(),
            )?;
        }

        for repeat in 1..=3 {
            self.reset_owner(&format!("measure_cold_no_owner_{repeat}"))?;
            let (interval, snapshot, _) =
                self.run_measurement_worker("query", measurement_parameters(1, 0, 0, 256))?;
            let operands = successful_operation_operands(&interval);
            self.record_metric(
                &mut metrics,
                "cold_first_vector",
                "cold_query_256b_v1",
                repeat,
                interval,
                snapshot,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let (interval, snapshot, _) =
                self.run_measurement_worker("query", measurement_parameters(1, 0, 0, 256))?;
            let operands = successful_operation_operands(&interval);
            self.record_metric(
                &mut metrics,
                "first_product_ready",
                "product_query_256b_v1",
                repeat,
                interval,
                snapshot,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let (interval, snapshot, _) =
                self.run_measurement_worker("query", measurement_parameters(1, 0, 0, 256))?;
            let operands = successful_operation_operands(&interval);
            self.record_metric(
                &mut metrics,
                "warm_query_ipc",
                "warm_query_256b_v1",
                repeat,
                interval,
                snapshot,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let (interval, snapshot, _) =
                self.run_measurement_worker("bulk", measurement_parameters(0, 1, 64, 256))?;
            let operands = successful_operation_operands(&interval);
            self.record_metric(
                &mut metrics,
                "warm_bulk_ipc",
                "warm_bulk_64x256b_v1",
                repeat,
                interval,
                snapshot,
                operands,
            )?;
        }

        for repeat in 1..=3 {
            let (interval, snapshot, output) =
                self.run_measurement_worker("bulk", measurement_parameters(0, 1, 256, 256))?;
            let request_id = output
                .result
                .as_ref()
                .and_then(|result| result.operations.as_slice().first())
                .and_then(|operation| operation.attempts.last())
                .map(|attempt| attempt.request_id.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("embedding_qualification_bulk_request_id_missing")
                })?;
            let completed_tokens =
                completed_token_count(self.context.output_directory, request_id)?;
            let duration_ns = interval
                .awake_finished_ns
                .saturating_sub(interval.awake_started_ns);
            self.record_metric(
                &mut metrics,
                "bulk_documents_per_second",
                "bulk_throughput_256x256b_v1",
                repeat,
                interval.clone(),
                snapshot.clone(),
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
                "bulk_throughput_256x256b_v1",
                repeat,
                interval,
                snapshot,
                BTreeMap::from([
                    ("completed_tokens".into(), json!(completed_tokens)),
                    (
                        "successful_operation_duration_ns".into(),
                        json!(duration_ns),
                    ),
                ]),
            )?;
        }

        let residency_worker = self.spawn_worker("resident_identity", query_parameters(1), None)?;
        let residency_output = self.finish_worker(residency_worker, SNAPSHOT_TIMEOUT)?;
        let residency_interval = measurement_interval(&residency_output)?;
        let identity = residency_output
            .engine_identity
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_residency_identity_missing"))?;
        let (_, residency_snapshot, _) =
            self.run_measurement_worker("observe", query_parameters(1))?;
        self.record_metric(
            &mut metrics,
            "backend_observed_accelerator_residency",
            "resident_policy_identity_v1",
            1,
            residency_interval,
            residency_snapshot.clone(),
            accelerator_operands(identity),
        )?;

        for repeat in 1..=3 {
            self.control("hold_class", Some("query"))?;
            let worker = self.spawn_worker("query", query_parameters(1), None)?;
            self.wait_for_snapshot("measurement_busy_queued", SNAPSHOT_TIMEOUT, |snapshot| {
                snapshot.scheduler.active_request_count > 0 || snapshot.scheduler.query_depth > 0
            })?;
            self.control("release_class", Some("query"))?;
            let output = self.finish_worker(worker, SNAPSHOT_TIMEOUT)?;
            require_worker_success(&output, "busy_retry_usefulness")?;
            let interval = measurement_interval(&output)?;
            let snapshot = self.record_worker_snapshot("measurement_busy_complete", &output)?;
            self.record_metric(
                &mut metrics,
                "busy_retry_usefulness",
                "held_query_release_v1",
                repeat,
                interval,
                snapshot,
                BTreeMap::new(),
            )?;
        }

        let (_, idle_owner, _) = self.run_measurement_worker("query", query_parameters(1))?;
        if !snapshot_has_resident_generation(&idle_owner) {
            bail!("embedding_qualification_true_idle_owner_not_resident");
        }
        let idle_timeout = Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
            .saturating_add(IDLE_EXIT_GRACE)
            .saturating_add(Duration::from_secs(30));
        let idle_output = self.wait_for_absence_output(idle_timeout)?;
        let idle_interval = measurement_interval(&idle_output)?;
        if idle_output.result.as_ref().is_none_or(|result| {
            result.initial_snapshot.is_none() || result.final_snapshot.is_some()
        }) {
            bail!("embedding_qualification_true_idle_worker_witness_invalid");
        }
        self.record_metric(
            &mut metrics,
            "true_idle_exit",
            "true_idle_60000_awake_ms_v1",
            1,
            idle_interval,
            idle_owner,
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

    fn run_measurement_worker(
        &mut self,
        operation: &str,
        parameters: EmbeddingQualificationParameters,
    ) -> Result<(MeasurementInterval, EmbeddingServerSnapshot, WorkerOutput)> {
        let worker = self.spawn_worker(operation, parameters, None)?;
        let output = self.finish_worker(worker, SNAPSHOT_TIMEOUT)?;
        require_worker_success(&output, operation)?;
        let interval = measurement_interval(&output)?;
        let snapshot = self.record_worker_snapshot("measurement_worker", &output)?;
        Ok((interval, snapshot, output))
    }

    fn record_metric(
        &self,
        metrics: &mut BTreeMap<String, RawMetric>,
        metric: &str,
        workload_id: &str,
        repeat: u32,
        interval: MeasurementInterval,
        snapshot: EmbeddingServerSnapshot,
        operands: BTreeMap<String, serde_json::Value>,
    ) -> Result<()> {
        let sample = interval.sample(RawMetricSampleInput {
            sample_id: &self.measurement_sample_id(metric, repeat),
            repeat,
            runtime: self.context.qualification_runtime,
            workload_id,
            server_identity: raw_server_identity(&snapshot)?,
            start_phase: "packaged_worker_operation_started",
            end_phase: "packaged_worker_operation_validated",
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

fn measurement_interval(output: &WorkerOutput) -> Result<MeasurementInterval> {
    if output.started_ns > output.finished_ns
        || output.inclusive_started_ns > output.inclusive_finished_ns
        || output.boot_id_started != output.clock.boot_id
        || output.boot_id_finished != output.clock.boot_id
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
        awake_started_ns: output.started_ns,
        awake_finished_ns: output.finished_ns,
        inclusive_clock_api: output.inclusive_clock_api.clone(),
        inclusive_started_ns: output.inclusive_started_ns,
        inclusive_finished_ns: output.inclusive_finished_ns,
        boot_id_started: output.boot_id_started.clone(),
        boot_id_finished: output.boot_id_finished.clone(),
    })
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
