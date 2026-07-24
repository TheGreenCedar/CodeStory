use super::super::{
    CLIENT_DEATH_LEASE_HOLD_MS, IDLE_EXIT_GRACE, NORMAL_WORKER_TIMEOUT, POLL, SNAPSHOT_TIMEOUT,
    btree,
};
use super::ScenarioRunner;
use super::analysis::{elapsed, same_server_authority, scheduler_values, validated_idle_epoch};
use super::process::{query_parameters, require_protocol_success, require_worker_success};
use anyhow::{Result, bail};
use codestory_retrieval::{
    EmbeddingQualificationParameters, PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
};
use serde_json::json;
use std::time::Duration;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn true_idle_respawn(&mut self) -> Result<()> {
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
        let (reclaimed, idle_epoch_ns, _) =
            self.wait_for_true_idle_epoch("true_idle_work_reclaimed", SNAPSHOT_TIMEOUT)?;
        self.transition("anti_idle_work_reclaimed", scheduler_values(&reclaimed));
        let client_idle_observed_ns = self.clock.now_ns();
        let mut diagnostic_count = 0_u64;
        let mut idle_connection_close_count = 0_u64;
        let mut last_diagnostic_client_elapsed_ns = 0_u64;
        let mut last_idle_connection_close_client_elapsed_ns = 0_u64;
        let mut final_server_idle_elapsed = None;
        for target_offset in [
            Duration::ZERO,
            Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS / 2),
        ] {
            self.wait_for_server_idle_elapsed(
                "true_idle_diagnostic_wait",
                &before,
                idle_epoch_ns,
                target_offset,
            )?;
            let phase = format!("true_idle_diagnostic_{diagnostic_count}");
            let diagnostic = self.observe(&phase)?.ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_true_idle_owner_exited_before_probe")
            })?;
            if !same_server_authority(&before, &diagnostic) {
                bail!("embedding_qualification_true_idle_owner_changed");
            }
            diagnostic_count = diagnostic_count.saturating_add(1);
            last_diagnostic_client_elapsed_ns = elapsed(&self.clock, client_idle_observed_ns)
                .as_nanos()
                .try_into()
                .unwrap_or(u64::MAX);
            let observer = self.spawn_worker("observe", query_parameters(1), None)?;
            let observer_output = self.finish_worker(observer, SNAPSHOT_TIMEOUT)?;
            require_worker_success(&observer_output, "true_idle_observe")?;
            let observer_snapshot =
                self.record_worker_snapshot("true_idle_worker_observe", &observer_output)?;
            if !same_server_authority(&before, &observer_snapshot) {
                bail!("embedding_qualification_idle_connection_owner_changed");
            }
            idle_connection_close_count = idle_connection_close_count.saturating_add(1);
            last_idle_connection_close_client_elapsed_ns =
                elapsed(&self.clock, client_idle_observed_ns)
                    .as_nanos()
                    .try_into()
                    .unwrap_or(u64::MAX);
            let event = self.control("snapshot", None)?;
            let confirmation = event.snapshot.as_ref().ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_idle_epoch_snapshot_missing")
            })?;
            if validated_idle_epoch(&event, confirmation)? != idle_epoch_ns
                || !same_server_authority(&before, confirmation)
            {
                bail!("embedding_qualification_true_idle_epoch_changed");
            }
            final_server_idle_elapsed = Some(Duration::from_nanos(
                event
                    .clock
                    .observed_ns
                    .checked_sub(idle_epoch_ns)
                    .ok_or_else(|| {
                        anyhow::anyhow!("embedding_qualification_idle_epoch_in_future")
                    })?,
            ));
        }
        let final_server_idle_elapsed = final_server_idle_elapsed.ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_idle_epoch_final_snapshot_missing")
        })?;
        let contract_idle_timeout =
            Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS);
        let client_wait_required = contract_idle_timeout.saturating_sub(final_server_idle_elapsed);
        let client_wait_origin_ns = self.clock.now_ns();
        while elapsed(&self.clock, client_wait_origin_ns) < client_wait_required {
            self.clock.sleep(POLL);
        }
        let client_wait_elapsed = elapsed(&self.clock, client_wait_origin_ns);
        self.wait_for_absence(
            "true_idle_after_wait",
            Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS / 2)
                .saturating_add(Duration::from_secs(15)),
        )?;
        self.observation(
            "true_idle_wait",
            btree([
                ("server_idle_epoch_ns", json!(idle_epoch_ns)),
                (
                    "server_idle_elapsed_before_client_wait_ns",
                    json!(final_server_idle_elapsed.as_nanos()),
                ),
                (
                    "client_wait_required_ns",
                    json!(client_wait_required.as_nanos()),
                ),
                (
                    "client_wait_elapsed_ns",
                    json!(client_wait_elapsed.as_nanos()),
                ),
                (
                    "contract_idle_timeout_ms",
                    json!(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS),
                ),
                ("clock_boot_id", json!(reclaimed.clock.boot_id)),
            ]),
        );
        self.transition(
            "idle_surfaces_exercised",
            btree([
                ("diagnostic_count", json!(diagnostic_count)),
                (
                    "idle_connection_close_count",
                    json!(idle_connection_close_count),
                ),
                (
                    "last_diagnostic_client_elapsed_ns",
                    json!(last_diagnostic_client_elapsed_ns),
                ),
                (
                    "last_idle_connection_close_client_elapsed_ns",
                    json!(last_idle_connection_close_client_elapsed_ns),
                ),
            ]),
        );
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
        let after_engine = after.engine.as_ref().ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_true_idle_respawn_engine_missing")
        })?;
        let identity_worker = self.spawn_worker("resident_identity", query_parameters(1), None)?;
        let identity_output = self.finish_worker(identity_worker, SNAPSHOT_TIMEOUT)?;
        let respawn_identity = identity_output.engine_identity.as_ref().ok_or_else(|| {
            anyhow::anyhow!("embedding_qualification_true_idle_respawn_identity_missing")
        })?;
        if respawn_identity.server_instance_id != after.process.server_instance_id
            || respawn_identity.load_generation != after_engine.load_generation
            || respawn_identity.model_load_count != after_engine.model_load_count
        {
            bail!("embedding_qualification_true_idle_respawn_identity_changed");
        }
        self.transition(
            "server_respawned",
            btree([
                (
                    "new_server_instance_id",
                    json!(after.process.server_instance_id),
                ),
                ("load_generation", json!(respawn_identity.load_generation)),
                ("model_load_count", json!(respawn_identity.model_load_count)),
                (
                    "materialized_model_sha256",
                    json!(respawn_identity.materialized_model_sha256),
                ),
                (
                    "materialized_reused",
                    json!(respawn_identity.materialized_reused),
                ),
            ]),
        );
        Ok(())
    }
}
