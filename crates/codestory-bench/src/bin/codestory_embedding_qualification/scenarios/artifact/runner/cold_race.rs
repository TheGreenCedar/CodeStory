use super::super::{NORMAL_WORKER_TIMEOUT, btree};
use super::ScenarioRunner;
use super::analysis::same_server_authority;
use super::process::{query_parameters, require_protocol_success};
use crate::qualification::output::write_atomic_json;
use anyhow::{Result, bail};
use serde_json::json;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn cold_race(&mut self) -> Result<()> {
        self.reset_owner("cold_race_no_owner")?;
        self.evidence
            .transitions
            .insert("no_owner_before_race".into());
        let gate = self
            .context
            .output_directory
            .join(format!(".{}.cold-race-gate.json", self.context.scenario));
        let first = self.spawn_worker_for(
            0,
            "cold_race_query",
            query_parameters(1),
            Some(gate.clone()),
        )?;
        let second = self.spawn_worker_for(
            1,
            "cold_race_query",
            query_parameters(1),
            Some(gate.clone()),
        )?;
        write_atomic_json(
            &gate,
            &json!({"schema_version": 1, "released_ns": self.clock.now_ns()}),
        )?;
        self.event(
            "orchestrator",
            "start_gate_released",
            None,
            btree([("worker_count", json!(2))]),
        );
        let first_output = self.finish_worker(first, NORMAL_WORKER_TIMEOUT)?;
        let second_output = self.finish_worker(second, NORMAL_WORKER_TIMEOUT)?;
        self.cleanup_gate(&gate);
        require_protocol_success(&first_output, "cold_race_first")?;
        require_protocol_success(&second_output, "cold_race_second")?;
        let first_snapshot = self.record_worker_snapshot("cold_race_first", &first_output)?;
        let second_snapshot = self.record_worker_snapshot("cold_race_second", &second_output)?;
        let invocations = &self.artifact.orchestration.process_invocations;
        let first_invocation = &invocations[invocations.len() - 2];
        let second_invocation = &invocations[invocations.len() - 1];
        if invocations.len() < 2
            || first_invocation.pid == second_invocation.pid
            || first_invocation.project_identity_sha256 == second_invocation.project_identity_sha256
        {
            bail!("embedding_qualification_cold_race_processes_not_independent");
        }
        let first_pid = first_invocation.pid;
        let second_pid = second_invocation.pid;
        let first_project = first_invocation.project_identity_sha256.clone();
        let second_project = second_invocation.project_identity_sha256.clone();
        let first_transport = &first_output
            .protocol_exchange
            .as_ref()
            .expect("protocol success required above")
            .transport_identity;
        let second_transport = &second_output
            .protocol_exchange
            .as_ref()
            .expect("protocol success required above")
            .transport_identity;
        self.transition(
            "two_independent_processes",
            btree([
                ("first_pid", json!(first_pid)),
                ("second_pid", json!(second_pid)),
                ("first_project_identity_sha256", json!(first_project)),
                ("second_project_identity_sha256", json!(second_project)),
                (
                    "first_transport_peer_verified",
                    json!(first_transport.peer_verified),
                ),
                (
                    "second_transport_peer_verified",
                    json!(second_transport.peer_verified),
                ),
            ]),
        );
        if !same_server_authority(&first_snapshot, &second_snapshot) {
            bail!("embedding_qualification_cold_race_multiple_owners");
        }
        self.transition(
            "single_server_convergence",
            btree([
                (
                    "server_instance_id",
                    json!(first_snapshot.process.server_instance_id),
                ),
                (
                    "lifetime_authority_id",
                    json!(first_snapshot.authority.lifetime_authority_id),
                ),
            ]),
        );
        Ok(())
    }
}
