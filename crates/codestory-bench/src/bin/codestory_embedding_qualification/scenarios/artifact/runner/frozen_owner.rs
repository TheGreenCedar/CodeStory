use super::super::{FROZEN_WORKER_TIMEOUT, NORMAL_WORKER_TIMEOUT, SNAPSHOT_TIMEOUT, btree};
use super::ScenarioRunner;
use super::analysis::same_server_authority;
use super::process::{query_parameters, require_worker_error, require_worker_success};
use anyhow::{Result, bail};
use serde_json::json;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn frozen_owner(&mut self) -> Result<()> {
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
                (
                    "retry",
                    serde_json::to_value(output.error.as_ref().expect("error required above"))?,
                ),
                ("timeout_ms", json!(8_000)),
                ("clock_domain", json!(output.clock.domain)),
                ("clock_boot_id", json!(output.clock.boot_id)),
            ]),
        );
        let after = self.wait_for_snapshot("frozen_owner_released", SNAPSHOT_TIMEOUT, |_| true)?;
        if !same_server_authority(&before, &after) {
            bail!("embedding_qualification_frozen_owner_takeover_detected");
        }
        let probe = self.spawn_worker("query", query_parameters(1), None)?;
        let probe_output = self.finish_worker(probe, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&probe_output, "frozen_owner_post_release_query")?;
        let connected =
            self.record_worker_snapshot("frozen_owner_post_release_query", &probe_output)?;
        if !same_server_authority(&before, &connected) {
            bail!("embedding_qualification_frozen_owner_post_release_changed");
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
                    json!(connected.authority.lifetime_authority_id),
                ),
                ("listener_id", json!(connected.authority.listener_id)),
                ("pid", json!(connected.process.pid)),
                (
                    "process_start_id",
                    json!(connected.process.process_start_id),
                ),
                ("post_release_query_succeeded", json!(true)),
            ]),
        );
        Ok(())
    }
}
