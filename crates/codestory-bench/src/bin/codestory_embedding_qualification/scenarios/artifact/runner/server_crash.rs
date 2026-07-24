use super::super::{NORMAL_WORKER_TIMEOUT, SNAPSHOT_TIMEOUT, btree};
use super::ScenarioRunner;
use super::analysis::scheduler_values;
use super::process::{query_parameters, require_worker_success, validate_replay_attempts};
use anyhow::{Result, bail};
use serde_json::json;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn server_crash(&mut self) -> Result<()> {
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
        let operations = &output
            .result
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_crash_result_missing"))?
            .operations;
        if operations.len() != 1 {
            bail!("embedding_qualification_crash_logical_operation_count");
        }
        let attempts = &operations[0].attempts;
        validate_replay_attempts(
            attempts,
            &before.process.server_instance_id,
            &after.process.server_instance_id,
            "server_crash",
        )?;
        self.transition(
            "query_replayed",
            btree([
                ("logical_operation_count", json!(operations.len())),
                ("wire_attempt_count", json!(attempts.len())),
                ("wire_attempts", serde_json::to_value(attempts)?),
            ]),
        );
        Ok(())
    }
}
