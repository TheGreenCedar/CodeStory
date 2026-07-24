use super::super::{
    ProcessInvocation, ProcessObservation, QUEUE_SETUP_TIMEOUT, WORKER_COMMAND, btree,
};
use super::analysis::project_identity_sha256;
use super::process::{
    cleanup_worker_files, stall_worker_timeout, validate_worker_output, wait_for_child,
    wait_for_process_start,
};
use super::{RunningWorker, ScenarioRunner, WorkerOutput, WorkerRequest};
use crate::qualification::output::write_atomic_json;
use crate::qualification::request::read_private_request;
use anyhow::{Context, Result, bail};
use codestory_retrieval::{
    EmbeddingQualificationParameters, EmbeddingServerSnapshot, SidecarRuntimeConfig,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

impl<'a> ScenarioRunner<'a> {
    pub(super) fn spawn_worker(
        &mut self,
        operation: &str,
        parameters: EmbeddingQualificationParameters,
        start_gate: Option<PathBuf>,
    ) -> Result<RunningWorker> {
        self.spawn_worker_for(
            self.context.primary_index,
            operation,
            parameters,
            start_gate,
        )
    }

    pub(super) fn spawn_worker_for(
        &mut self,
        project_index: usize,
        operation: &str,
        parameters: EmbeddingQualificationParameters,
        start_gate: Option<PathBuf>,
    ) -> Result<RunningWorker> {
        let runtime = self
            .context
            .runtimes
            .get(project_index)
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_project_index_invalid"))?;
        let project = self
            .context
            .projects
            .get(project_index)
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_project_index_invalid"))?;
        let project_identity_sha256 = project_identity_sha256(runtime);
        self.next_worker = self.next_worker.saturating_add(1);
        let invocation_id = format!("{}-{}", self.context.scenario, self.next_worker);
        let request_path = self
            .context
            .output_directory
            .join(format!(".{invocation_id}.worker-request.json"));
        let output_path = self
            .context
            .output_directory
            .join(format!(".{invocation_id}.worker-output.json"));
        let request = WorkerRequest {
            schema_version: 1,
            nonce_sha256: self.context.nonce_sha256.into(),
            executable_sha256: self.executable.sha256.clone(),
            project: project.clone(),
            operation: operation.into(),
            parameters,
            start_gate: start_gate.clone(),
            start_gate_timeout_ms: start_gate.as_ref().map(|_| {
                if self.context.scenario == "worker_stall" {
                    stall_worker_timeout().as_millis() as u64
                } else {
                    QUEUE_SETUP_TIMEOUT.as_millis() as u64
                }
            }),
        };
        if let Some(gate) = start_gate.as_ref() {
            self.active_gates.insert(gate.clone());
        }
        write_atomic_json(&request_path, &request)?;
        let started_ns = self.clock.now_ns();
        let child = Command::new(&self.executable.path)
            .arg(WORKER_COMMAND)
            .arg("--request")
            .arg(&request_path)
            .arg("--output")
            .arg(&output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn qualification worker {invocation_id}"))?;
        let pid = child.id();
        let process_start_id = wait_for_process_start(&*self.clock, pid)?;
        let invocation_index = self.artifact.orchestration.process_invocations.len();
        self.artifact
            .orchestration
            .process_invocations
            .push(ProcessInvocation {
                invocation_id: invocation_id.clone(),
                operation: operation.into(),
                project_identity_sha256: project_identity_sha256.clone(),
                pid,
                process_start_id,
                started_ns,
                finished_ns: None,
                exit_code: None,
                termination: None,
            });
        self.event(
            "orchestrator",
            "worker_started",
            Some(invocation_id),
            btree([("pid", json!(pid)), ("operation", json!(operation))]),
        );
        Ok(RunningWorker {
            invocation_index,
            child,
            request_path,
            output_path,
        })
    }

    pub(super) fn cleanup_gate(&mut self, gate: &Path) {
        let _ = fs::remove_file(gate);
        self.active_gates.remove(gate);
    }

    pub(super) fn primary_runtime(&self) -> &SidecarRuntimeConfig {
        &self.context.runtimes[self.context.primary_index]
    }

    pub(super) fn finish_worker(
        &mut self,
        mut worker: RunningWorker,
        timeout: Duration,
    ) -> Result<WorkerOutput> {
        let status = wait_for_child(&*self.clock, &mut worker.child, timeout)?;
        self.finish_invocation(worker.invocation_index, status, "exited");
        if !status.success() {
            cleanup_worker_files(&worker);
            bail!("embedding_qualification_worker_failed:{status}");
        }
        let bytes = read_private_request(&worker.output_path)?;
        let output: WorkerOutput =
            serde_json::from_slice(&bytes).context("parse qualification worker output")?;
        validate_worker_output(
            &output,
            &self.artifact.orchestration.process_invocations[worker.invocation_index],
            &self.executable.sha256,
        )?;
        self.observation(
            "worker_output",
            btree([
                (
                    "invocation_id",
                    json!(
                        self.artifact.orchestration.process_invocations[worker.invocation_index]
                            .invocation_id
                    ),
                ),
                ("output", serde_json::to_value(&output)?),
            ]),
        );
        cleanup_worker_files(&worker);
        Ok(output)
    }

    pub(super) fn terminate_worker(&mut self, mut worker: RunningWorker) -> Result<()> {
        worker
            .child
            .kill()
            .context("terminate qualification worker")?;
        let status = worker.child.wait().context("reap qualification worker")?;
        self.finish_invocation(worker.invocation_index, status, "terminated");
        cleanup_worker_files(&worker);
        Ok(())
    }

    pub(super) fn finish_invocation(
        &mut self,
        index: usize,
        status: ExitStatus,
        termination: &str,
    ) {
        let finished_ns = self.clock.now_ns();
        let invocation = &mut self.artifact.orchestration.process_invocations[index];
        invocation.finished_ns = Some(finished_ns);
        invocation.exit_code = status.code();
        invocation.termination = Some(termination.into());
        let invocation_id = invocation.invocation_id.clone();
        self.event(
            "orchestrator",
            "worker_finished",
            Some(invocation_id),
            btree([
                ("exit_code", json!(status.code())),
                ("termination", json!(termination)),
            ]),
        );
    }

    pub(super) fn transition(&mut self, name: &str, values: BTreeMap<String, Value>) {
        self.evidence.transitions.insert(name.into());
        self.observation(name, values);
    }

    pub(super) fn record_worker_snapshot(
        &mut self,
        phase: &str,
        output: &WorkerOutput,
    ) -> Result<EmbeddingServerSnapshot> {
        let snapshot = output
            .result
            .as_ref()
            .and_then(|result| result.final_snapshot.clone())
            .or_else(|| {
                output.protocol_exchange.as_ref().map(|exchange| {
                    exchange
                        .final_snapshot
                        .clone()
                        .unwrap_or_else(|| exchange.hello_snapshot.clone())
                })
            })
            .ok_or_else(|| anyhow::anyhow!("embedding_qualification_worker_snapshot_missing"))?;
        self.artifact
            .process_observations
            .push(ProcessObservation::from_snapshot(
                phase,
                self.clock.now_ns(),
                Some(snapshot.clone()),
            ));
        Ok(snapshot)
    }
}
