use super::super::{
    MIXED_QUEUE_PROJECT_COUNT, NORMAL_WORKER_TIMEOUT, QUALIFICATION_QUEUE_CAPACITY,
    QUEUE_SETUP_TIMEOUT, SNAPSHOT_TIMEOUT, btree,
};
use super::ScenarioRunner;
use super::analysis::{
    analyze_queue_operations, attach_native_completion_sequences,
    require_pre_release_capacity_overflow, scheduler_values,
};
use super::process::{query_parameters, require_protocol_success};
use crate::qualification::output::write_atomic_json;
use anyhow::{Result, bail};
use codestory_retrieval::EmbeddingQualificationParameters;
use serde_json::json;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn mixed_queue(&mut self) -> Result<()> {
        let owner = self.ensure_owner("mixed_queue_owner")?;
        if owner.scheduler.query_capacity != QUALIFICATION_QUEUE_CAPACITY
            || owner.scheduler.bulk_capacity != QUALIFICATION_QUEUE_CAPACITY
        {
            bail!("embedding_qualification_mixed_queue_capacity_invalid");
        }
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
                query_count: MIXED_QUEUE_PROJECT_COUNT,
                bulk_count: MIXED_QUEUE_PROJECT_COUNT,
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
                query_count: MIXED_QUEUE_PROJECT_COUNT,
                bulk_count: MIXED_QUEUE_PROJECT_COUNT,
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
            |snapshot| {
                snapshot.scheduler.query_depth >= u64::from(MIXED_QUEUE_PROJECT_COUNT)
                    && snapshot.scheduler.bulk_depth >= u64::from(MIXED_QUEUE_PROJECT_COUNT)
            },
        )?;
        write_atomic_json(&second_gate, &json!({"schema_version": 1}))?;
        let saturated =
            self.wait_for_snapshot("mixed_queue_saturated", QUEUE_SETUP_TIMEOUT, |snapshot| {
                snapshot.scheduler.query_capacity == QUALIFICATION_QUEUE_CAPACITY
                    && snapshot.scheduler.bulk_capacity == QUALIFICATION_QUEUE_CAPACITY
                    && snapshot.scheduler.query_depth == QUALIFICATION_QUEUE_CAPACITY
                    && snapshot.scheduler.bulk_depth == QUALIFICATION_QUEUE_CAPACITY
            })?;
        self.transition("queues_saturated", scheduler_values(&saturated));
        let overflow = self.spawn_worker_for(
            1,
            "queue_load",
            EmbeddingQualificationParameters {
                query_count: 1,
                bulk_count: 1,
                documents_per_bulk: 1,
                input_bytes: 64,
                hold_ms: 0,
            },
            None,
        )?;
        let overflow_output = self.finish_worker(overflow, QUEUE_SETUP_TIMEOUT)?;
        let mut overflow_operations = overflow_output.queue_operations.ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_overflow_queue_output_missing")
        })?;
        require_pre_release_capacity_overflow(&overflow_operations)?;
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
        self.cleanup_gate(&first_gate);
        self.cleanup_gate(&second_gate);
        require_protocol_success(&seed_output, "mixed_queue_seed")?;
        if first_output.clock != second_output.clock {
            bail!("embedding_qualification_queue_clock_domain_mismatch");
        }
        let mut operations = first_output
            .queue_operations
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_first_queue_output_missing"))?;
        for operation in &mut operations {
            operation.submission_batch = 0;
        }
        let mut second_operations = second_output.queue_operations.ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_second_queue_output_missing")
        })?;
        for operation in &mut second_operations {
            operation.submission_batch = 1;
        }
        for operation in &mut overflow_operations {
            operation.submission_batch = 2;
        }
        operations.extend(second_operations);
        operations.extend(overflow_operations);
        attach_native_completion_sequences(self.context.output_directory, &mut operations)?;
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
                    ("submission_batch", json!(operation.submission_batch)),
                    ("submitted_ns", json!(operation.submitted_ns)),
                    ("completed_ns", json!(operation.completed_ns)),
                    (
                        "native_completion_sequence",
                        json!(operation.native_completion_sequence),
                    ),
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
}
