use super::super::{
    CLIENT_DEATH_LEASE_HOLD_MS, FROZEN_WORKER_TIMEOUT, NORMAL_WORKER_TIMEOUT, SNAPSHOT_TIMEOUT,
    btree,
};
use super::ScenarioRunner;
use super::process::{query_parameters, require_worker_error, require_worker_success};
use anyhow::{Result, bail};
use codestory_retrieval::EmbeddingQualificationParameters;
use serde_json::json;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn incompatible_owner(&mut self) -> Result<()> {
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
        let active = self.spawn_worker("activate_probe", query_parameters(1), None)?;
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
                (
                    "retry",
                    serde_json::to_value(
                        active_output.error.as_ref().expect("error required above"),
                    )?,
                ),
            ]),
        );
        self.terminate_worker(lease)?;
        self.wait_for_control_snapshot("incompatible_owner_idle", SNAPSHOT_TIMEOUT, |snapshot| {
            snapshot.scheduler.lease_count == 0 && snapshot.scheduler.active_request_count == 0
        })?;
        let idle = self.spawn_worker("activate_probe", query_parameters(1), None)?;
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
                (
                    "retry",
                    serde_json::to_value(
                        idle_output.error.as_ref().expect("error required above"),
                    )?,
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
        self.control("clear_incompatible", None)?;
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
}
