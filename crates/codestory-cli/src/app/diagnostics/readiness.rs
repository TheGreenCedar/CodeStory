use super::super::readiness_commands::doctor_sidecar_status_is_live_ready;
use super::sidecar::doctor_sidecar_status_for_runtime;
use crate::args::{ReadinessLaneOutput, RetrievalStatusOutput};
use crate::display;
use crate::runtime::RuntimeContext;
use codestory_contracts::api::{ReadinessGoalDto, ReadinessStatusDto};
use std::collections::BTreeMap;
#[cfg(test)]
use std::path::Path;

pub(in crate::app) fn agent_readiness_status(
    runtime: &RuntimeContext,
    run_id: Option<&str>,
) -> RetrievalStatusOutput {
    let agent_runtime = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        codestory_retrieval::SidecarProfile::Agent,
        run_id,
    );
    doctor_sidecar_status_for_runtime(runtime, agent_runtime)
}

pub(crate) fn build_readiness_lanes_for_runtime(
    runtime: &RuntimeContext,
    readiness: &[codestory_contracts::api::ReadinessVerdictDto],
    agent_run_id: Option<&str>,
    selected_agent_status: Option<&RetrievalStatusOutput>,
) -> BTreeMap<String, ReadinessLaneOutput> {
    let project = display::clean_path_string(&runtime.project_root.to_string_lossy());
    let project_arg = display::quote_command_argument_value(&project);
    let local_runtime = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        codestory_retrieval::SidecarProfile::Local,
        None,
    );
    let local_status = doctor_sidecar_status_for_runtime(runtime, local_runtime);
    let agent_status = selected_agent_status.cloned().unwrap_or_else(|| {
        doctor_sidecar_status_for_runtime(
            runtime,
            runtime.sidecar.with_profile_and_run_id(
                Some(&runtime.project_root),
                codestory_retrieval::SidecarProfile::Agent,
                agent_run_id,
            ),
        )
    });
    let agent_verdict = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::AgentPacketSearch);
    let mut lanes = BTreeMap::new();
    lanes.insert(
        "local_default".to_string(),
        readiness_lane_output("local_default", &local_status, None, &project_arg),
    );
    lanes.insert(
        "agent_packet_search".to_string(),
        readiness_lane_output(
            "agent_packet_search",
            &agent_status,
            agent_verdict,
            &project_arg,
        ),
    );
    lanes
}

#[cfg(test)]
pub(in crate::app) fn agent_readiness_sidecar_runtime(
    project_root: &Path,
    run_id: Option<&str>,
) -> codestory_retrieval::SidecarRuntimeConfig {
    crate::sidecar_runtime::for_project_with_run_id(
        project_root,
        codestory_retrieval::SidecarProfile::Agent,
        run_id,
    )
}

pub(in crate::app) fn readiness_lane_output(
    lane: &str,
    sidecar: &RetrievalStatusOutput,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
    project_arg: &str,
) -> ReadinessLaneOutput {
    let status = readiness_lane_status(sidecar, verdict);
    ReadinessLaneOutput {
        status,
        profile: sidecar
            .profile
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        run_id: sidecar.run_id.clone(),
        namespace: None,
        phase: None,
        repair_updated_at_epoch_ms: None,
        retrieval_mode: sidecar.retrieval_mode.clone(),
        degraded_reason: sidecar.degraded_reason.clone(),
        next_command: lane_next_command(lane, sidecar, status, verdict, project_arg),
    }
}

pub(in crate::app::diagnostics) fn readiness_lane_status(
    sidecar: &RetrievalStatusOutput,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
) -> ReadinessStatusDto {
    let sidecar_status = if doctor_sidecar_status_is_live_ready(sidecar) {
        ReadinessStatusDto::Ready
    } else {
        ReadinessStatusDto::RepairRetrieval
    };
    if sidecar.profile.as_deref() == Some("agent")
        && sidecar_status == ReadinessStatusDto::RepairRetrieval
        && sidecar
            .degraded_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("embedding_runtime_unavailable:"))
    {
        return ReadinessStatusDto::RepairRetrieval;
    }
    match verdict.map(|verdict| verdict.status) {
        Some(ReadinessStatusDto::Blocked) => ReadinessStatusDto::Blocked,
        Some(status @ (ReadinessStatusDto::RepairSetup | ReadinessStatusDto::RepairIndex)) => {
            status
        }
        Some(ReadinessStatusDto::CheckIndex) if sidecar_status == ReadinessStatusDto::Ready => {
            ReadinessStatusDto::CheckIndex
        }
        _ => sidecar_status,
    }
}

pub(in crate::app::diagnostics) fn lane_next_command(
    lane: &str,
    sidecar: &RetrievalStatusOutput,
    status: ReadinessStatusDto,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
    project_arg: &str,
) -> Option<String> {
    if status == ReadinessStatusDto::Ready {
        return Some(retrieval_status_command(sidecar, project_arg));
    }
    if let Some(command) = verdict.and_then(|verdict| verdict.minimum_next.first()) {
        return Some(command.clone());
    }
    match lane {
        "agent_packet_search" if !doctor_sidecar_status_is_live_ready(sidecar) => Some(format!(
            "codestory-cli retrieval index --project {project_arg} --profile agent --refresh auto --format json"
        )),
        "local_default" if !doctor_sidecar_status_is_live_ready(sidecar) => Some(format!(
            "codestory-cli retrieval index --project {project_arg} --profile local --refresh full --format json"
        )),
        _ => Some(retrieval_status_command(sidecar, project_arg)),
    }
}

pub(in crate::app::diagnostics) fn retrieval_status_command(
    sidecar: &RetrievalStatusOutput,
    project_arg: &str,
) -> String {
    let mut command = format!(
        "codestory-cli retrieval status --project {project_arg} --profile {}",
        sidecar.profile.as_deref().unwrap_or("local")
    );
    if let Some(run_id) = sidecar.run_id.as_deref() {
        command.push_str(" --run-id ");
        command.push_str(&display::quote_command_argument_value(run_id));
    }
    command.push_str(" --format json");
    command
}
