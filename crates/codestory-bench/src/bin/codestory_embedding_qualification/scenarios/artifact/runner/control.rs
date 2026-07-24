use super::super::{
    CONTROL_TIMEOUT, ControlEvent, NORMAL_WORKER_TIMEOUT, POLL, ProcessObservation, RawEvent,
    RawObservation, SNAPSHOT_TIMEOUT, btree,
};
use super::analysis::{control_key, elapsed, same_server_authority, validated_idle_epoch};
use super::process::{
    existing_control_events, qualification_command_path, qualification_nonce, query_parameters,
    require_worker_success,
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
        if self.observe(&format!("{phase}_before"))?.is_some() {
            self.control("crash_server", None)?;
        }
        self.wait_for_absence(phase, SNAPSHOT_TIMEOUT)
    }

    pub(super) fn wait_for_absence_output(&mut self, timeout: Duration) -> Result<WorkerOutput> {
        let worker = self.spawn_worker("wait_for_absence", query_parameters(1), None)?;
        let output = self.finish_worker(worker, timeout)?;
        require_worker_success(&output, "wait_for_absence")?;
        Ok(output)
    }

    fn observe_worker(&mut self) -> Result<Option<EmbeddingServerSnapshot>> {
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
            qualification_command_path(self.context.output_directory, &qualification_nonce()?);
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
                if let Some(event) = existing_control_events(self.context.output_directory)?
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
