use super::super::{
    CONTROL_TIMEOUT, ControlEvent, NORMAL_WORKER_TIMEOUT, POLL, ProcessObservation, RawEvent,
    RawObservation, SNAPSHOT_TIMEOUT, btree,
};
use super::analysis::{control_key, elapsed, same_server_authority, validated_idle_epoch};
use super::process::{
    existing_control_events_for_nonce, load_establishment_timeout, qualification_command_path,
    query_parameters, require_worker_success, wait_for_exact_process_exit,
};
use super::{ControlCommand, ControlCommandParameters, ScenarioRunner, WorkerOutput};
use crate::qualification::output::write_atomic_json;
use anyhow::{Context, Result, bail};
use codestory_retrieval::EmbeddingServerSnapshot;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn event(
        &mut self,
        source: &str,
        action: &str,
        correlation_id: Option<String>,
        values: BTreeMap<String, Value>,
    ) {
        self.artifact.events.push(RawEvent {
            sequence: self.artifact.events.len() as u64,
            source: source.into(),
            action: action.into(),
            observed_ns: self.clock.now_ns(),
            correlation_id,
            values,
        });
    }

    pub(super) fn observation(&mut self, kind: &str, values: BTreeMap<String, Value>) {
        self.artifact.observations.push(RawObservation {
            sequence: self.artifact.observations.len() as u64,
            kind: kind.into(),
            observed_ns: self.clock.now_ns(),
            values,
        });
    }

    pub(super) fn observe(&mut self, phase: &str) -> Result<Option<EmbeddingServerSnapshot>> {
        let snapshot = self.observe_worker()?;
        self.artifact
            .process_observations
            .push(ProcessObservation::from_snapshot(
                phase,
                self.clock.now_ns(),
                snapshot.clone(),
            ));
        Ok(snapshot)
    }

    pub(super) fn wait_for_snapshot(
        &mut self,
        phase: &str,
        timeout: Duration,
        predicate: impl Fn(&EmbeddingServerSnapshot) -> bool,
    ) -> Result<EmbeddingServerSnapshot> {
        let started = self.clock.now_ns();
        loop {
            if let Some(snapshot) = self.observe_worker()?
                && predicate(&snapshot)
            {
                self.artifact
                    .process_observations
                    .push(ProcessObservation::from_snapshot(
                        phase,
                        self.clock.now_ns(),
                        Some(snapshot.clone()),
                    ));
                return Ok(snapshot);
            }
            if elapsed(&self.clock, started) >= timeout {
                bail!("embedding_qualification_snapshot_timeout:{phase}");
            }
            self.clock.sleep(POLL);
        }
    }

    /// Snapshot wait whose predicate gates on worker-driven load
    /// establishment. The budget comes from the named entry in
    /// `LOAD_ESTABLISHMENT_WAITS`, so the number of establishing clients a
    /// site depends on is declared once and derived once instead of being
    /// re-guessed per call site.
    pub(super) fn wait_for_established_load(
        &mut self,
        phase: &str,
        predicate: impl Fn(&EmbeddingServerSnapshot) -> bool,
    ) -> Result<EmbeddingServerSnapshot> {
        let timeout = load_establishment_timeout(phase)?;
        self.wait_for_snapshot(phase, timeout, predicate)
    }

    pub(super) fn wait_for_control_snapshot(
        &mut self,
        phase: &str,
        timeout: Duration,
        predicate: impl Fn(&EmbeddingServerSnapshot) -> bool,
    ) -> Result<EmbeddingServerSnapshot> {
        let started = self.clock.now_ns();
        loop {
            if let Some(snapshot) = self.control("snapshot", None)?.snapshot
                && predicate(&snapshot)
            {
                self.artifact
                    .process_observations
                    .push(ProcessObservation::from_snapshot(
                        phase,
                        self.clock.now_ns(),
                        Some(snapshot.clone()),
                    ));
                return Ok(snapshot);
            }
            if elapsed(&self.clock, started) >= timeout {
                bail!("embedding_qualification_control_snapshot_timeout:{phase}");
            }
            self.clock.sleep(POLL);
        }
    }

    pub(super) fn wait_for_true_idle_epoch(
        &mut self,
        phase: &str,
        timeout: Duration,
    ) -> Result<(EmbeddingServerSnapshot, u64, ControlEvent)> {
        let started = self.clock.now_ns();
        loop {
            let event = self.control("snapshot", None)?;
            if let Some(snapshot) = event.snapshot.as_ref()
                && snapshot.scheduler.lease_count == 0
                && snapshot.scheduler.active_request_count == 0
                && snapshot.scheduler.query_depth == 0
                && snapshot.scheduler.bulk_depth == 0
            {
                let idle_epoch_ns = validated_idle_epoch(&event, snapshot)?;
                self.artifact
                    .process_observations
                    .push(ProcessObservation::from_snapshot(
                        phase,
                        self.clock.now_ns(),
                        Some(snapshot.clone()),
                    ));
                return Ok((snapshot.clone(), idle_epoch_ns, event));
            }
            if elapsed(&self.clock, started) >= timeout {
                bail!("embedding_qualification_idle_epoch_timeout:{phase}");
            }
            self.clock.sleep(POLL);
        }
    }

    pub(super) fn wait_for_server_idle_elapsed(
        &mut self,
        _phase: &str,
        before: &EmbeddingServerSnapshot,
        idle_epoch_ns: u64,
        target: Duration,
    ) -> Result<(EmbeddingServerSnapshot, ControlEvent, Duration)> {
        loop {
            let event = self.control("snapshot", None)?;
            let snapshot = event.snapshot.as_ref().ok_or_else(|| {
                anyhow::anyhow!("embedding_qualification_idle_epoch_snapshot_missing")
            })?;
            if !same_server_authority(before, snapshot) {
                bail!("embedding_qualification_true_idle_owner_changed");
            }
            let epoch = validated_idle_epoch(&event, snapshot)?;
            if epoch != idle_epoch_ns {
                bail!("embedding_qualification_true_idle_epoch_changed");
            }
            let server_elapsed = Duration::from_nanos(
                event
                    .clock
                    .observed_ns
                    .checked_sub(idle_epoch_ns)
                    .ok_or_else(|| {
                        anyhow::anyhow!("embedding_qualification_idle_epoch_in_future")
                    })?,
            );
            if server_elapsed >= target {
                return Ok((snapshot.clone(), event, server_elapsed));
            }

            let remaining = target.saturating_sub(server_elapsed);
            let client_wait_origin_ns = self.clock.now_ns();
            while elapsed(&self.clock, client_wait_origin_ns) < remaining {
                self.clock.sleep(POLL);
            }
        }
    }

    pub(super) fn wait_for_absence(&mut self, phase: &str, timeout: Duration) -> Result<()> {
        let output = self.wait_for_absence_output(timeout)?;
        if output
            .result
            .as_ref()
            .is_none_or(|result| result.final_snapshot.is_some())
        {
            bail!("embedding_qualification_owner_exit_missing:{phase}");
        }
        self.artifact
            .process_observations
            .push(ProcessObservation::from_snapshot(
                phase,
                self.clock.now_ns(),
                None,
            ));
        Ok(())
    }

    pub(super) fn ensure_owner(&mut self, phase: &str) -> Result<EmbeddingServerSnapshot> {
        if let Some(snapshot) = self.observe(&format!("{phase}_existing"))? {
            return Ok(snapshot);
        }
        let worker = self.spawn_worker("query", query_parameters(1), None)?;
        let output = self.finish_worker(worker, NORMAL_WORKER_TIMEOUT)?;
        require_worker_success(&output, "ensure_owner")?;
        self.record_worker_snapshot(phase, &output)
    }

    pub(super) fn reset_owner(&mut self, phase: &str) -> Result<()> {
        let Some(before) = self.observe(&format!("{phase}_before"))? else {
            return Ok(());
        };
        let accepted = self.control("crash_server", None)?;
        validate_reset_crash_event(&before, &accepted, phase)?;
        wait_for_exact_process_exit(
            &self.clock,
            before.process.pid,
            &before.process.process_start_id,
            SNAPSHOT_TIMEOUT,
        )?;

        // The accepted crash event pins the predecessor. Once that exact native
        // process is gone, run one observation worker and require it to see no
        // owner at either edge of its operation. This keeps a replacement that
        // appears in the handoff from being mistaken for predecessor absence,
        // without asking the generic wait-for-absence operation to wait out a
        // different owner.
        let worker = self.spawn_worker("observe", query_parameters(1), None)?;
        let output = self.finish_worker(worker, SNAPSHOT_TIMEOUT)?;
        require_reset_absence(&output, phase)?;
        self.artifact
            .process_observations
            .push(ProcessObservation::from_snapshot(
                phase,
                self.clock.now_ns(),
                None,
            ));
        Ok(())
    }

    pub(super) fn wait_for_absence_output(&mut self, timeout: Duration) -> Result<WorkerOutput> {
        let worker = self.spawn_worker("wait_for_absence", query_parameters(1), None)?;
        let output = self.finish_worker(worker, timeout)?;
        require_worker_success(&output, "wait_for_absence")?;
        Ok(output)
    }

    fn observe_worker(&mut self) -> Result<Option<EmbeddingServerSnapshot>> {
        // One observe worker per poll: executable capture, connect, one
        // snapshot, no request work and no per-request transport fan-out, so
        // the flat snapshot budget dominates its honest chain. Waits that
        // cover clients establishing load must carry a derived budget instead:
        // `load_establishment_timeout` for admissions and leases (one
        // start-and-capture allowance per establishing client), the queue-setup
        // budget for a seeded queue (`dead_client_setup_timeout`).
        let worker = self.spawn_worker("observe", query_parameters(1), None)?;
        let output = self.finish_worker(worker, SNAPSHOT_TIMEOUT)?;
        require_worker_success(&output, "observe")?;
        Ok(output
            .result
            .as_ref()
            .and_then(|result| result.final_snapshot.clone()))
    }

    pub(super) fn control(&mut self, action: &str, class: Option<&str>) -> Result<ControlEvent> {
        let command_path =
            qualification_command_path(self.context.output_directory, &self.qualification_nonce);
        let wait_started = self.clock.now_ns();
        while command_path.exists() {
            if elapsed(&self.clock, wait_started) >= CONTROL_TIMEOUT {
                bail!("embedding_qualification_control_slot_busy");
            }
            self.clock.sleep(POLL);
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        let command = ControlCommand {
            schema_version: 1,
            sequence: self.next_sequence,
            nonce_sha256: self.context.nonce_sha256.into(),
            action: action.into(),
            parameters: ControlCommandParameters {
                class: class.map(str::to_owned),
            },
        };
        write_atomic_json(&command_path, &command)?;
        let event_result = (|| -> Result<ControlEvent> {
            let started = self.clock.now_ns();
            loop {
                if let Some(event) = existing_control_events_for_nonce(
                    self.context.output_directory,
                    &self.qualification_nonce,
                )?
                .into_iter()
                .find(|event| event.sequence == self.next_sequence)
                {
                    return Ok(event);
                }
                if elapsed(&self.clock, started) >= CONTROL_TIMEOUT {
                    bail!("embedding_qualification_control_event_timeout:{action}");
                }
                self.clock.sleep(POLL);
            }
        })();
        let cleanup_result =
            fs::remove_file(&command_path).context("remove owned embedding qualification command");
        let mut event = match event_result {
            Ok(event) => {
                cleanup_result?;
                event
            }
            Err(error) => {
                let _ = cleanup_result;
                return Err(error);
            }
        };
        if event.action != action
            || !matches!(event.status.as_str(), "completed" | "accepted")
            || (action == "crash_server" && event.status != "accepted")
        {
            bail!("embedding_qualification_control_event_invalid:{action}");
        }
        event.authenticated_nonce_sha256 = self.context.nonce_sha256.into();
        self.evidence.controls.insert(control_key(action, class));
        self.update_active_controls(action, class);
        self.event(
            "server_control",
            action,
            Some(event.sequence.to_string()),
            btree([("status", json!(event.status))]),
        );
        self.artifact.control_events.push(event.clone());
        Ok(event)
    }

    pub(super) fn update_active_controls(&mut self, action: &str, class: Option<&str>) {
        match (action, class) {
            ("hold_class", Some(class)) => {
                self.active_controls
                    .insert(control_key("hold_class", Some(class)));
            }
            ("release_class", Some(class)) => {
                self.active_controls
                    .remove(&control_key("hold_class", Some(class)));
            }
            ("freeze_owner", None) => {
                self.active_controls.insert("freeze_owner".into());
            }
            ("release_owner", None) => {
                self.active_controls.remove("freeze_owner");
            }
            ("force_incompatible", None) => {
                self.active_controls.insert("force_incompatible".into());
            }
            ("clear_incompatible", None) => {
                self.active_controls.remove("force_incompatible");
            }
            ("stall_native", None) => {
                self.active_controls.insert("stall_native".into());
            }
            ("release_native", None) => {
                self.active_controls.remove("stall_native");
            }
            ("crash_server", None) => self.active_controls.clear(),
            _ => {}
        }
    }
}

fn validate_reset_crash_event(
    before: &EmbeddingServerSnapshot,
    accepted: &ControlEvent,
    phase: &str,
) -> Result<()> {
    let snapshot = accepted.snapshot.as_ref().ok_or_else(|| {
        anyhow::anyhow!("embedding_qualification_reset_crash_snapshot_missing:{phase}")
    })?;
    if before.process.pid == 0
        || before.process.process_start_id.is_empty()
        || snapshot.process.pid == 0
        || snapshot.process.process_start_id.is_empty()
        || accepted.action != "crash_server"
        || accepted.status != "accepted"
        || !same_server_authority(before, snapshot)
    {
        bail!("embedding_qualification_reset_crash_owner_mismatch:{phase}");
    }
    Ok(())
}

fn require_reset_absence(output: &WorkerOutput, phase: &str) -> Result<()> {
    require_worker_success(output, phase)?;
    let result = output.result.as_ref().expect("success requires result");
    if result.scenario != "observe"
        || result.initial_snapshot.is_some()
        || result.final_snapshot.is_some()
    {
        bail!("embedding_qualification_owner_replaced_before_absence:{phase}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::super::ControlEventClock;
    use super::*;
    use codestory_retrieval::{
        EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION, EmbeddingQualificationOperationResult,
        EmbeddingQualificationResult, EmbeddingServerAuthoritySnapshot,
        EmbeddingServerClockSnapshot, EmbeddingServerProcessSnapshot,
        EmbeddingServerProtocolSnapshot, EmbeddingServerSchedulerSnapshot,
    };

    fn snapshot() -> EmbeddingServerSnapshot {
        EmbeddingServerSnapshot {
            schema_version: 1,
            event_sequence: 8,
            lifecycle: "listening".into(),
            clock: EmbeddingServerClockSnapshot {
                domain: "awake_monotonic".into(),
                api: "test".into(),
                boot_id: "boot".into(),
                resolution_ns: 1,
            },
            protocol: EmbeddingServerProtocolSnapshot::current(),
            authority: EmbeddingServerAuthoritySnapshot {
                endpoint_namespace_id: "endpoint".into(),
                lifetime_authority_id: "lifetime".into(),
                listener_id: "listener".into(),
                peer_verified: true,
            },
            process: EmbeddingServerProcessSnapshot {
                server_instance_id: "9edfb3ab".into(),
                pid: 41732,
                process_start_id: "windows:639221697923142260".into(),
                executable_sha256: "a".repeat(64),
                executable_version: "0.16.1".into(),
            },
            scheduler: EmbeddingServerSchedulerSnapshot {
                query_capacity: 8,
                query_depth: 0,
                bulk_capacity: 1,
                bulk_depth: 0,
                connection_count: 0,
                active_request_count: 0,
                lease_count: 0,
                active_request: None,
            },
            engine: None,
            failure: None,
        }
    }

    fn accepted_crash(snapshot: Option<EmbeddingServerSnapshot>) -> ControlEvent {
        ControlEvent {
            schema_version: 1,
            sequence: 1,
            action: "crash_server".into(),
            status: "accepted".into(),
            authenticated_nonce_sha256: "b".repeat(64),
            server_event_sequence: 9,
            clock: ControlEventClock {
                domain: "awake_monotonic".into(),
                api: "test".into(),
                boot_id: "boot".into(),
                observed_ns: 1,
            },
            snapshot,
            details: None,
        }
    }

    fn absence_output(
        initial_snapshot: Option<EmbeddingServerSnapshot>,
        final_snapshot: Option<EmbeddingServerSnapshot>,
    ) -> WorkerOutput {
        WorkerOutput {
            schema_version: EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
            pid: 42,
            process_start_id: "worker-start".into(),
            executable_sha256: "c".repeat(64),
            executable_version: "0.16.1".into(),
            project_identity_sha256: "d".repeat(64),
            clock: EmbeddingServerClockSnapshot {
                domain: "awake_monotonic".into(),
                api: "test".into(),
                boot_id: "boot".into(),
                resolution_ns: 1,
            },
            started_ns: 1,
            finished_ns: 2,
            inclusive_clock_api: "test".into(),
            inclusive_started_ns: 1,
            inclusive_finished_ns: 2,
            boot_id_started: "boot".into(),
            boot_id_finished: "boot".into(),
            result: Some(EmbeddingQualificationResult {
                schema_version: 1,
                scenario: "observe".into(),
                started_ns: 1,
                finished_ns: 2,
                operations: vec![EmbeddingQualificationOperationResult {
                    correlation_id: "observe-1".into(),
                    class: "observe".into(),
                    submitted_ns: 1,
                    completed_ns: 2,
                    status: "ok".into(),
                    error_code: None,
                    server_instance_id: None,
                    load_generation: None,
                    attempts: Vec::new(),
                }],
                initial_snapshot,
                final_snapshot,
            }),
            protocol_exchange: None,
            queue_operations: None,
            engine_identity: None,
            measurement: None,
            error: None,
        }
    }

    #[test]
    fn reset_crash_event_must_carry_the_exact_accepted_predecessor() {
        let before = snapshot();
        validate_reset_crash_event(
            &before,
            &accepted_crash(Some(before.clone())),
            "measurement_rebind",
        )
        .expect("exact accepted crash snapshot");

        let missing_snapshot = accepted_crash(None);
        assert_eq!(
            validate_reset_crash_event(&before, &missing_snapshot, "measurement_rebind")
                .expect_err("missing crash snapshot")
                .to_string(),
            "embedding_qualification_reset_crash_snapshot_missing:measurement_rebind"
        );

        let mut missing_pid = before.clone();
        missing_pid.process.pid = 0;
        let mut missing_start = before.clone();
        missing_start.process.process_start_id.clear();
        let mut mismatched_pid = before.clone();
        mismatched_pid.process.pid = 41733;
        let mut mismatched_start = before.clone();
        mismatched_start.process.process_start_id = "windows:reused".into();
        for (case, hostile) in [
            ("missing pid", missing_pid),
            ("missing process start", missing_start),
            ("mismatched pid", mismatched_pid),
            ("mismatched process start", mismatched_start),
        ] {
            assert_eq!(
                validate_reset_crash_event(
                    &before,
                    &accepted_crash(Some(hostile)),
                    "measurement_rebind"
                )
                .expect_err(case)
                .to_string(),
                "embedding_qualification_reset_crash_owner_mismatch:measurement_rebind"
            );
        }

        let mut missing_observed_pid = before.clone();
        missing_observed_pid.process.pid = 0;
        let mut missing_observed_start = before.clone();
        missing_observed_start.process.process_start_id.clear();
        for (case, hostile_before) in [
            ("missing observed pid", missing_observed_pid),
            ("missing observed process start", missing_observed_start),
        ] {
            assert_eq!(
                validate_reset_crash_event(
                    &hostile_before,
                    &accepted_crash(Some(hostile_before.clone())),
                    "measurement_rebind"
                )
                .expect_err(case)
                .to_string(),
                "embedding_qualification_reset_crash_owner_mismatch:measurement_rebind"
            );
        }
    }

    #[test]
    fn reset_absence_rejects_a_replacement_at_either_observation_edge() {
        require_reset_absence(&absence_output(None, None), "measurement_rebind")
            .expect("owner remains absent");

        for (case, output) in [
            (
                "replacement present initially",
                absence_output(Some(snapshot()), None),
            ),
            (
                "replacement present finally",
                absence_output(None, Some(snapshot())),
            ),
        ] {
            assert_eq!(
                require_reset_absence(&output, "measurement_rebind")
                    .expect_err(case)
                    .to_string(),
                "embedding_qualification_owner_replaced_before_absence:measurement_rebind"
            );
        }
    }

    #[test]
    fn reset_owner_keeps_the_absent_fast_path_and_fences_before_one_observation_worker() {
        let source = include_str!("control.rs");
        let start = source
            .find("pub(super) fn reset_owner")
            .expect("reset owner");
        let end = source[start..]
            .find("pub(super) fn wait_for_absence_output")
            .map(|offset| start.saturating_add(offset))
            .expect("reset owner end");
        let body = &source[start..end];
        let absent = body.find("let Some(before)").expect("absent fast path");
        let crash = body
            .find("self.control(\"crash_server\"")
            .expect("accepted crash event");
        let validate = body
            .find("validate_reset_crash_event")
            .expect("crash identity validation");
        let fence = body
            .find("wait_for_exact_process_exit")
            .expect("exact predecessor fence");
        let observe = body
            .find("self.spawn_worker(\"observe\"")
            .expect("absence observation worker");
        assert!(absent < crash && crash < validate && validate < fence && fence < observe);
        assert!(body[absent..crash].contains("return Ok(())"));
        assert_eq!(body.matches("self.spawn_worker(\"observe\"").count(), 1);
        assert!(!body.contains("self.wait_for_absence("));
    }
}
