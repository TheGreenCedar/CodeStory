use super::super::AGENT_PREFLIGHT_LOCAL_REFRESH_FOREGROUND_BUDGET;
use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::diagnostics::{
    agent_readiness_status, build_readiness_lanes_for_runtime, build_summary_readiness,
};
use super::local_freshness::{
    local_freshness_needs_refresh, local_refresh_output_from_summary, wait_for_local_freshness,
};
use crate::args;
use crate::args::{ProjectArgs, ReadinessLaneOutput};
use crate::output::emit;
use crate::runtime::RuntimeContext;
use crate::{local_refresh_status, readiness};
use anyhow::Result;
use codestory_contracts::api::{ProjectSummary, ReadinessGoalDto, ReadinessStatusDto};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub(in crate::app) fn run_agent(cmd: args::AgentCommand) -> Result<()> {
    match cmd.action {
        args::AgentAction::Preflight(cmd) => run_agent_preflight(cmd),
    }
}

pub(super) fn run_agent_preflight(cmd: args::AgentPreflightCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "agent preflight")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let summary = runtime.open_project_summary()?;
    let (summary, local_refresh) = if local_freshness_needs_refresh(&summary) {
        wait_for_agent_preflight_local_freshness(&cmd.project, &summary)?
    } else {
        (summary, None)
    };
    let readiness_sidecar = agent_readiness_status(&runtime, None);
    let readiness = build_summary_readiness(
        &summary.root,
        &summary.stats,
        summary.freshness.as_ref(),
        &readiness_sidecar,
    );
    let readiness_lanes =
        build_readiness_lanes_for_runtime(&runtime, &readiness, None, Some(&readiness_sidecar));
    let output = build_agent_preflight_output(&readiness, readiness_lanes, local_refresh);
    let markdown = render_agent_preflight_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn wait_for_agent_preflight_local_freshness(
    project: &ProjectArgs,
    summary: &ProjectSummary,
) -> Result<(ProjectSummary, Option<readiness::LocalRefreshOutput>)> {
    let (tx, rx) = mpsc::channel();
    let project = project.clone();
    thread::spawn(move || {
        let result = RuntimeContext::new_inspect_only(&project)
            .and_then(|runtime| wait_for_local_freshness(&project, &runtime));
        let _ = tx.send(result);
    });

    let budget = agent_preflight_local_refresh_foreground_budget();
    if budget.is_zero() {
        return Ok((
            summary.clone(),
            Some(agent_preflight_local_refresh_timeout_output(summary)),
        ));
    }

    match rx.recv_timeout(budget) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Ok((
            summary.clone(),
            Some(agent_preflight_local_refresh_timeout_output(summary)),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let mut output = local_refresh_output_from_summary(summary);
            output.state = readiness::LocalRefreshState::Failed;
            output.blocks_local_surfaces = true;
            output.readiness_status = ReadinessStatusDto::RepairIndex;
            output.reason = Some("refresh_worker_disconnected".to_string());
            output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
            Ok((summary.clone(), Some(output)))
        }
    }
}

fn agent_preflight_local_refresh_timeout_output(
    summary: &ProjectSummary,
) -> readiness::LocalRefreshOutput {
    let mut output = local_refresh_output_from_summary(summary);
    output.state = readiness::LocalRefreshState::Refreshing;
    output.blocks_local_surfaces = true;
    output.readiness_status = ReadinessStatusDto::RepairIndex;
    output.reason = Some("refresh_timeout".to_string());
    output.phase = Some("incremental_index".to_string());
    output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
    output
}

fn agent_preflight_local_refresh_foreground_budget() -> Duration {
    std::env::var("CODESTORY_AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(AGENT_PREFLIGHT_LOCAL_REFRESH_FOREGROUND_BUDGET)
}

const LOCAL_GRAPH_AGENT_SURFACES: &[&str] = &[
    "ground", "files", "symbol", "callers", "callees", "trail", "trace", "snippet", "affected",
];
const FULL_RETRIEVAL_AGENT_SURFACES: &[&str] = &["packet_full", "search_full", "context_full"];

fn build_agent_preflight_output(
    readiness: &[codestory_contracts::api::ReadinessVerdictDto],
    readiness_lanes: BTreeMap<String, ReadinessLaneOutput>,
    local_refresh: Option<readiness::LocalRefreshOutput>,
) -> args::AgentPreflightOutput {
    let local = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::LocalNavigation)
        .expect("local_navigation readiness verdict");
    let agent = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::AgentPacketSearch)
        .expect("agent_packet_search readiness verdict");
    let local_ready = local.status == ReadinessStatusDto::Ready;
    let full_ready = agent.status == ReadinessStatusDto::Ready;
    let mut safe_surfaces = Vec::new();
    let mut blocked_surfaces = Vec::new();

    if local_ready {
        safe_surfaces.extend(surface_strings(LOCAL_GRAPH_AGENT_SURFACES));
    } else {
        blocked_surfaces.extend(surface_strings(LOCAL_GRAPH_AGENT_SURFACES));
    }
    if full_ready {
        safe_surfaces.extend(surface_strings(FULL_RETRIEVAL_AGENT_SURFACES));
    } else {
        blocked_surfaces.extend(surface_strings(FULL_RETRIEVAL_AGENT_SURFACES));
    }

    let mode = if full_ready {
        "full_retrieval"
    } else if local_ready {
        "local_graph"
    } else {
        "blocked"
    };
    let next_command = readiness::primary_non_ready(readiness)
        .and_then(|verdict| verdict.full_repair.first().cloned());
    let human_summary = agent_preflight_summary(local_ready, full_ready, local);

    args::AgentPreflightOutput {
        usable: local_ready || full_ready,
        mode: mode.to_string(),
        local_graph: agent_preflight_lane(local),
        local_refresh: local_refresh.unwrap_or_else(|| readiness::local_refresh_output(local)),
        full_retrieval: agent_preflight_lane(agent),
        local_default: readiness_lanes
            .get("local_default")
            .cloned()
            .expect("local_default readiness lane"),
        agent_packet_search: readiness_lanes
            .get("agent_packet_search")
            .cloned()
            .expect("agent_packet_search readiness lane"),
        readiness_lanes,
        safe_surfaces,
        blocked_surfaces,
        next_command,
        human_summary,
    }
}

fn surface_strings(surfaces: &[&str]) -> Vec<String> {
    surfaces
        .iter()
        .map(|surface| (*surface).to_string())
        .collect()
}

fn agent_preflight_lane(
    verdict: &codestory_contracts::api::ReadinessVerdictDto,
) -> args::AgentPreflightLaneOutput {
    let sidecar = verdict.sidecar.as_ref();
    args::AgentPreflightLaneOutput {
        ready: verdict.status == ReadinessStatusDto::Ready,
        status: verdict.status,
        failed_layer: readiness::failed_layer(verdict),
        summary: verdict.summary.clone(),
        embedding_device_policy: sidecar
            .and_then(|sidecar| sidecar.embedding_device_policy.clone()),
        embedding_device_state: sidecar.and_then(|sidecar| sidecar.embedding_device_state.clone()),
        embedding_device_observation_source: sidecar
            .and_then(|sidecar| sidecar.embedding_device_observation_source.clone()),
        embedding_detected_provider: sidecar
            .and_then(|sidecar| sidecar.embedding_detected_provider.clone()),
        embedding_detected_gpu: sidecar.and_then(|sidecar| sidecar.embedding_detected_gpu.clone()),
        embedding_accelerator_requested: sidecar
            .map(|sidecar| sidecar.embedding_accelerator_requested),
        embedding_accelerator_request_provider: sidecar
            .and_then(|sidecar| sidecar.embedding_accelerator_request_provider.clone()),
        embedding_accelerator_request_device: sidecar
            .and_then(|sidecar| sidecar.embedding_accelerator_request_device.clone()),
        embedding_cpu_allowed: sidecar.map(|sidecar| sidecar.embedding_cpu_allowed),
    }
}

fn agent_preflight_summary(
    local_ready: bool,
    full_ready: bool,
    local: &codestory_contracts::api::ReadinessVerdictDto,
) -> String {
    match (local_ready, full_ready) {
        (_, true) => "Local graph and full retrieval are ready.".to_string(),
        (true, false) => "Local graph is ready. Full retrieval needs a rebuild.".to_string(),
        (false, _) => format!(
            "Local graph is not ready: {} Full retrieval is also unavailable for agent packet/search.",
            local.summary
        ),
    }
}

fn render_agent_preflight_markdown(output: &args::AgentPreflightOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Agent Preflight");
    let _ = writeln!(markdown, "usable: `{}`", output.usable);
    let _ = writeln!(markdown, "mode: `{}`", output.mode);
    let _ = writeln!(
        markdown,
        "local_graph: {}",
        readiness::status_label(output.local_graph.status)
    );
    let _ = writeln!(
        markdown,
        "local_refresh: {}",
        readiness::local_refresh_state_label(output.local_refresh.state)
    );
    if let Some(layer) = output.local_graph.failed_layer {
        let _ = writeln!(markdown, "local_graph_failed_layer: `{layer}`");
    }
    let _ = writeln!(
        markdown,
        "full_retrieval: {}",
        readiness::status_label(output.full_retrieval.status)
    );
    if let Some(layer) = output.full_retrieval.failed_layer {
        let _ = writeln!(markdown, "full_retrieval_failed_layer: `{layer}`");
    }
    if let (Some(policy), Some(state), Some(cpu_allowed)) = (
        output.full_retrieval.embedding_device_policy.as_deref(),
        output.full_retrieval.embedding_device_state.as_deref(),
        output.full_retrieval.embedding_cpu_allowed,
    ) {
        let source = output
            .full_retrieval
            .embedding_device_observation_source
            .as_deref()
            .map(|source| format!(" observation_source=`{source}`"))
            .unwrap_or_default();
        let detected = output
            .full_retrieval
            .embedding_detected_provider
            .as_deref()
            .map(|provider| {
                let gpu = output
                    .full_retrieval
                    .embedding_detected_gpu
                    .as_deref()
                    .unwrap_or("unknown");
                format!(" detected_provider=`{provider}` detected_gpu=`{gpu}`")
            })
            .unwrap_or_default();
        let request = output
            .full_retrieval
            .embedding_accelerator_requested
            .filter(|requested| *requested)
            .map(|_| {
                let provider = output
                    .full_retrieval
                    .embedding_accelerator_request_provider
                    .as_deref()
                    .unwrap_or("unknown");
                let device = output
                    .full_retrieval
                    .embedding_accelerator_request_device
                    .as_deref()
                    .unwrap_or("unknown");
                format!(" accelerator_request=`{provider}:{device}`")
            })
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "full_retrieval_embedding_device: policy=`{policy}` observed=`{state}`{source}{detected}{request} cpu_allowed={cpu_allowed}"
        );
    }
    let _ = writeln!(markdown, "human_summary: {}", output.human_summary);
    if let Some(command) = output.next_command.as_deref() {
        let _ = writeln!(markdown, "next_command: `{command}`");
    }
    markdown
}
