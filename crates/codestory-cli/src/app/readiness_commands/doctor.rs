use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::diagnostics::{
    agent_readiness_status, build_doctor_output, build_readiness_lanes_for_runtime,
    build_summary_readiness, doctor_sidecar_status,
};
use super::local_freshness::wait_for_local_freshness;
use crate::args;
use crate::args::{DoctorCommand, ReadyCommand, ReadyOutput, RetrievalStatusOutput};
use crate::output::{emit, render_doctor_markdown, render_ready_markdown};
use crate::runtime::RuntimeContext;
use anyhow::Result;

pub(in crate::app) fn run_doctor(cmd: DoctorCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "doctor")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let summary = runtime.open_project_summary()?;
    let output = build_doctor_output(&runtime, &summary);
    let markdown = render_doctor_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

pub(in crate::app) fn run_ready(cmd: ReadyCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "ready")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let output = build_ready_output(&cmd)?;
    let markdown = render_ready_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn build_ready_output(cmd: &ReadyCommand) -> Result<ReadyOutput> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let agent_run_id = cmd.run_id.as_deref();
    let (summary, local_refresh) = if cmd.wait_fresh {
        wait_for_local_freshness(&cmd.project, &runtime)?
    } else {
        (runtime.open_project_summary()?, None)
    };
    let readiness_sidecar = if matches!(cmd.goal, None | Some(args::ReadyGoal::Agent)) {
        agent_readiness_status(&runtime, agent_run_id)
    } else {
        doctor_sidecar_status(&runtime)
    };
    let selected_agent_run_id = readiness_sidecar
        .run_id
        .as_deref()
        .or(agent_run_id)
        .map(str::to_string);
    let mut verdicts = build_summary_readiness(
        &summary.root,
        &summary.stats,
        summary.freshness.as_ref(),
        &readiness_sidecar,
    );
    let readiness_lanes = build_readiness_lanes_for_runtime(
        &runtime,
        &verdicts,
        selected_agent_run_id.as_deref(),
        Some(&readiness_sidecar),
    );
    if let Some(goal) = cmd.goal {
        let goal = goal.as_dto();
        verdicts.retain(|verdict| verdict.goal == goal);
    }
    let output = ReadyOutput {
        verdicts,
        local_refresh,
        readiness_lanes,
    };
    Ok(output)
}

pub(in crate::app) fn doctor_sidecar_status_is_live_ready(status: &RetrievalStatusOutput) -> bool {
    status.retrieval_mode == "full" && status.degraded_reason.is_none()
}
