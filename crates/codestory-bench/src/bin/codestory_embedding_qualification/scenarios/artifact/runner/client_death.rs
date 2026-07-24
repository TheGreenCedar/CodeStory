use super::super::{
    CLIENT_DEATH_LEASE_HOLD_MS, DEAD_CLIENT_BULK_COUNT, DEAD_CLIENT_QUERY_COUNT,
    FROZEN_WORKER_TIMEOUT, NORMAL_WORKER_TIMEOUT, SNAPSHOT_TIMEOUT, btree,
};
use super::ScenarioRunner;
use super::analysis::{project_identity_sha256, scheduler_values};
use super::process::{query_parameters, require_worker_success};
use anyhow::{Result, bail};
use codestory_retrieval::EmbeddingQualificationParameters;
use serde_json::json;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn client_death(&mut self) -> Result<()> {
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
}
