use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::diagnostics::{
    agent_readiness_status, build_doctor_output, build_readiness_lanes_for_runtime,
    build_summary_readiness, doctor_sidecar_status,
};
use super::local_freshness::wait_for_local_freshness;
use crate::args;
use crate::args::{
    DiagnosticCoreStatus, DoctorCommand, DoctorOutput, ReadyCommand, ReadyOutput,
    RetrievalStatusOutput,
};
use crate::display::quote_command_path;
use crate::output::{emit, render_doctor_markdown, render_ready_markdown};
use crate::runtime::{RuntimeContext, api_error_in_chain};
use anyhow::Result;
use codestory_contracts::api::{
    ProjectSummary, ReadinessStatusDto, ReadinessVerdictDto, StorageStatsDto,
};

struct ObservedCore {
    summary: ProjectSummary,
    status: Option<DiagnosticCoreStatus>,
    reason: Option<String>,
}

fn observe_core(runtime: &RuntimeContext) -> Result<ObservedCore> {
    match runtime.inspect_project_summary() {
        Ok(Some(summary)) => Ok(ObservedCore {
            summary,
            status: None,
            reason: None,
        }),
        Ok(None) => Ok(ObservedCore {
            summary: unavailable_summary(runtime),
            status: Some(DiagnosticCoreStatus::Unavailable),
            reason: Some("No core cache database is available for this project.".to_string()),
        }),
        Err(error) => {
            let Some(api_error) = api_error_in_chain(&error) else {
                return Err(error);
            };
            if api_error.code != "core_schema_upgrade_required" {
                return Err(error);
            }
            Ok(ObservedCore {
                summary: unavailable_summary(runtime),
                status: Some(DiagnosticCoreStatus::UpgradeRequired),
                reason: Some(api_error.message.clone()),
            })
        }
    }
}

fn unavailable_summary(runtime: &RuntimeContext) -> ProjectSummary {
    ProjectSummary {
        root: runtime.project_root.to_string_lossy().into_owned(),
        stats: StorageStatsDto {
            node_count: 0,
            edge_count: 0,
            file_count: 0,
            error_count: 0,
            fatal_error_count: 0,
        },
        members: Vec::new(),
        retrieval: None,
        freshness: None,
        publication: None,
    }
}

fn mark_unavailable_verdicts(
    runtime: &RuntimeContext,
    verdicts: &mut [ReadinessVerdictDto],
    reason: &str,
) {
    let project = quote_command_path(&runtime.project_root);
    let index_command = format!("codestory-cli index --project {project} --refresh full");
    let doctor_command = format!("codestory-cli doctor --project {project} --format json");
    for verdict in verdicts {
        verdict.status = ReadinessStatusDto::RepairIndex;
        verdict.summary = reason.to_string();
        verdict.minimum_next = vec![index_command.clone()];
        verdict.full_repair = vec![index_command.clone(), doctor_command.clone()];
    }
}

fn mark_unavailable_doctor(
    runtime: &RuntimeContext,
    output: &mut DoctorOutput,
    status: DiagnosticCoreStatus,
    reason: &str,
) {
    output.core_status = Some(status);
    mark_unavailable_verdicts(runtime, &mut output.readiness, reason);
    output.readiness_lanes =
        build_readiness_lanes_for_runtime(runtime, &output.readiness, None, None);
    output.next_commands = crate::readiness::compatibility_next_commands(&output.readiness);
    for check in &mut output.checks {
        if check.name == "cache" || check.name == "index" {
            check.status = "warn".to_string();
            check.message = reason.to_string();
        }
    }
}

pub(in crate::app) fn run_doctor(cmd: DoctorCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "doctor")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let observed = observe_core(&runtime)?;
    let mut output = build_doctor_output(&runtime, &observed.summary);
    if let (Some(status), Some(reason)) = (observed.status, observed.reason.as_deref()) {
        mark_unavailable_doctor(&runtime, &mut output, status, reason);
    }
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
    let (summary, local_refresh, core_status, core_reason) = if cmd.wait_fresh {
        let (summary, local_refresh) = wait_for_local_freshness(&cmd.project, &runtime)?;
        (summary, local_refresh, None, None)
    } else {
        let observed = observe_core(&runtime)?;
        (observed.summary, None, observed.status, observed.reason)
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
    if let Some(reason) = core_reason.as_deref() {
        mark_unavailable_verdicts(&runtime, &mut verdicts, reason);
    }
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
        core_status,
        local_refresh,
        readiness_lanes,
        legacy_retirement: readiness_sidecar.legacy_retirement,
    };
    Ok(output)
}

pub(in crate::app) fn doctor_sidecar_status_is_live_ready(status: &RetrievalStatusOutput) -> bool {
    status.retrieval_mode == "full" && status.degraded_reason.is_none()
}
