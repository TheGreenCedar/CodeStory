use super::super::{NORMAL_WORKER_TIMEOUT, SNAPSHOT_TIMEOUT, btree};
use super::ScenarioRunner;
use super::analysis::{consume_watchdog_marker, same_server_authority, scheduler_values};
use super::process::{
    require_worker_success, stall_worker_timeout, validate_replay_attempts, wait_for_process_exit,
};
use crate::qualification::diagnostic_worker_stall_enabled;
use crate::qualification::output::write_atomic_json;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{EmbeddingQualificationParameters, ProcessStartProbe};
use serde_json::json;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn worker_stall(&mut self) -> Result<()> {
        let before = self.ensure_owner("worker_stall_before")?;
        let survivor_gate = self
            .context
            .output_directory
            .join(".worker-stall-survivor-gate.json");
        let survivor = self.spawn_worker(
            "query",
            EmbeddingQualificationParameters {
                query_count: 1,
                bulk_count: 0,
                documents_per_bulk: 0,
                input_bytes: 64,
                hold_ms: 325_000,
            },
            Some(survivor_gate.clone()),
        )?;
        let survivor_invocation =
            &self.artifact.orchestration.process_invocations[survivor.invocation_index];
        let survivor_pid = survivor_invocation.pid;
        let survivor_process_start_id = survivor_invocation.process_start_id.clone();
        self.control("stall_native", None)?;
        let worker = self.spawn_worker(
            "bulk",
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
        let output = self.finish_worker(worker, stall_worker_timeout())?;
        if diagnostic_worker_stall_enabled()? {
            write_atomic_json(
                &self
                    .context
                    .output_directory
                    .join("worker_stall.replay-worker-output.json"),
                &output,
            )
            .context("retain diagnostic worker_stall replay output")?;
        }
        require_worker_success(&output, "worker_stall_replay")?;
        let operation = output
            .result
            .as_ref()
            .and_then(|result| (result.operations.len() == 1).then(|| &result.operations[0]))
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_stall_operation_missing"))?;
        wait_for_process_exit(&self.clock, before.process.pid, SNAPSHOT_TIMEOUT)?;
        let (watchdog_marker, watchdog_marker_sha256) = consume_watchdog_marker(
            self.context.output_directory,
            self.context.nonce_sha256,
            &before,
        )?;
        let after = self.record_worker_snapshot("worker_stall_replacement", &output)?;
        if before.process.server_instance_id == after.process.server_instance_id {
            bail!("embedding_qualification_stalled_owner_not_replaced");
        }
        validate_replay_attempts(
            &operation.attempts,
            &before.process.server_instance_id,
            &after.process.server_instance_id,
            "worker_stall",
        )?;
        self.transition(
            "watchdog_fail_stop_observed",
            btree([
                ("old_pid", json!(before.process.pid)),
                (
                    "old_server_instance_id",
                    json!(before.process.server_instance_id),
                ),
                ("wire_attempt_count", json!(operation.attempts.len())),
                ("wire_attempts", serde_json::to_value(&operation.attempts)?),
                ("watchdog_marker_sha256", json!(watchdog_marker_sha256)),
                ("watchdog_reason", json!(watchdog_marker.reason)),
                (
                    "watchdog_observed_ns",
                    json!(watchdog_marker.clock.observed_ns),
                ),
                (
                    "watchdog_last_progress_ns",
                    json!(watchdog_marker.last_progress_ns),
                ),
                (
                    "watchdog_progress_sequence",
                    json!(watchdog_marker.progress_sequence),
                ),
                (
                    "hard_native_no_progress_ms",
                    json!(watchdog_marker.hard_native_no_progress_ms),
                ),
                (
                    "watchdog_cadence_ms",
                    json!(watchdog_marker.watchdog_cadence_ms),
                ),
            ]),
        );
        match codestory_retrieval::probe_process_start_identity(survivor_pid) {
            ProcessStartProbe::Running { start_identity }
                if start_identity == survivor_process_start_id => {}
            _ => bail!("embedding_qualification_unrelated_worker_did_not_survive"),
        }
        write_atomic_json(
            &survivor_gate,
            &json!({"schema_version": 1, "released_ns": self.clock.now_ns()}),
        )?;
        let survivor_output = self.finish_worker(survivor, NORMAL_WORKER_TIMEOUT)?;
        self.cleanup_gate(&survivor_gate);
        require_worker_success(&survivor_output, "worker_stall_survivor_query")?;
        let survivor_after =
            self.record_worker_snapshot("worker_stall_survivor_query", &survivor_output)?;
        if !same_server_authority(&after, &survivor_after) {
            bail!("embedding_qualification_survivor_used_wrong_replacement");
        }
        self.control("release_native", None)?;
        self.transition(
            "unrelated_process_survived",
            btree([
                ("pid", json!(survivor_pid)),
                ("process_start_id", json!(survivor_process_start_id)),
                (
                    "new_server_instance_id",
                    json!(survivor_after.process.server_instance_id),
                ),
            ]),
        );
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
