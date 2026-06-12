use codestory_contracts::api::{
    IndexFreshnessDto, IndexFreshnessStatusDto, ReadinessGoalDto, ReadinessIndexSnapshotDto,
    ReadinessSidecarSnapshotDto, ReadinessStatusDto, ReadinessVerdictDto, StorageStatsDto,
};

use crate::display::{clean_path_string, quote_command_argument_value};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadinessInputs<'a> {
    pub(crate) project: &'a str,
    pub(crate) stats: &'a StorageStatsDto,
    pub(crate) freshness: Option<&'a IndexFreshnessDto>,
    pub(crate) sidecar: Option<ReadinessSidecarInput<'a>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadinessSidecarInput<'a> {
    pub(crate) retrieval_mode: &'a str,
    pub(crate) degraded_reason: Option<&'a str>,
    pub(crate) manifest_generation: Option<&'a str>,
    pub(crate) manifest_input_hash: Option<&'a str>,
}

pub(crate) fn build_readiness_verdicts(inputs: ReadinessInputs<'_>) -> Vec<ReadinessVerdictDto> {
    vec![
        build_readiness_verdict(ReadinessGoalDto::LocalNavigation, inputs),
        build_readiness_verdict(ReadinessGoalDto::AgentPacketSearch, inputs),
    ]
}

pub(crate) fn build_readiness_verdict(
    goal: ReadinessGoalDto,
    inputs: ReadinessInputs<'_>,
) -> ReadinessVerdictDto {
    let project = clean_path_string(inputs.project);
    let project_arg = project_arg(&project);
    let index = readiness_index_snapshot(inputs.stats, inputs.freshness);
    let sidecar = inputs.sidecar.map(readiness_sidecar_snapshot);

    let (status, summary, minimum_next, full_repair) = verdict_state(
        goal,
        inputs.stats,
        inputs.freshness,
        inputs.sidecar,
        &project_arg,
    );

    ReadinessVerdictDto {
        goal,
        status,
        summary,
        minimum_next,
        full_repair,
        index: Some(index),
        sidecar,
    }
}

pub(crate) fn combined_minimum_next(verdicts: &[ReadinessVerdictDto]) -> Vec<String> {
    dedupe_commands(
        verdicts
            .iter()
            .flat_map(|verdict| verdict.minimum_next.iter().cloned()),
    )
}

pub(crate) fn compatibility_next_commands(verdicts: &[ReadinessVerdictDto]) -> Vec<String> {
    if let Some(verdict) = primary_non_ready(verdicts) {
        return verdict.full_repair.clone();
    }
    combined_minimum_next(verdicts)
}

pub(crate) fn primary_non_ready(verdicts: &[ReadinessVerdictDto]) -> Option<&ReadinessVerdictDto> {
    verdicts
        .iter()
        .find(|verdict| verdict.status != ReadinessStatusDto::Ready)
}

pub(crate) fn status_label(status: ReadinessStatusDto) -> &'static str {
    match status {
        ReadinessStatusDto::Ready => "ready",
        ReadinessStatusDto::RepairIndex => "repair_index",
        ReadinessStatusDto::CheckIndex => "check_index",
        ReadinessStatusDto::RepairRetrieval => "repair_retrieval",
        ReadinessStatusDto::CacheBusy => "cache_busy",
    }
}

pub(crate) fn goal_label(goal: ReadinessGoalDto) -> &'static str {
    match goal {
        ReadinessGoalDto::LocalNavigation => "local_navigation",
        ReadinessGoalDto::AgentPacketSearch => "agent_packet_search",
    }
}

fn verdict_state(
    goal: ReadinessGoalDto,
    stats: &StorageStatsDto,
    freshness: Option<&IndexFreshnessDto>,
    sidecar: Option<ReadinessSidecarInput<'_>>,
    project_arg: &str,
) -> (ReadinessStatusDto, String, Vec<String>, Vec<String>) {
    if stats.node_count == 0 {
        return index_repair_state(
            goal,
            "No indexed symbols are available yet.",
            project_arg,
            "full",
        );
    }

    match freshness.map(|freshness| freshness.status) {
        Some(IndexFreshnessStatusDto::Stale) => {
            return index_repair_state(
                goal,
                "The index has changed, new, or removed files.",
                project_arg,
                "incremental",
            );
        }
        Some(IndexFreshnessStatusDto::NotChecked) => {
            let command =
                format!("codestory-cli index --project {project_arg} --refresh incremental");
            return (
                ReadinessStatusDto::CheckIndex,
                "Index drift was not checked for this cache view.".to_string(),
                vec![command.clone()],
                vec![
                    command,
                    format!("codestory-cli doctor --project {project_arg}"),
                ],
            );
        }
        Some(IndexFreshnessStatusDto::Fresh) | None => {}
    }

    if goal == ReadinessGoalDto::AgentPacketSearch {
        let sidecar_mode = sidecar
            .map(|sidecar| sidecar.retrieval_mode)
            .unwrap_or("unavailable");
        if sidecar_mode != "full" {
            return (
                ReadinessStatusDto::RepairRetrieval,
                format!(
                    "Agent packet/search needs full sidecar retrieval; current mode is `{sidecar_mode}`."
                ),
                vec![
                    format!(
                        "codestory-cli retrieval bootstrap --project {project_arg} --format json"
                    ),
                    format!(
                        "codestory-cli retrieval index --project {project_arg} --refresh full --format json"
                    ),
                ],
                vec![
                    format!("codestory-cli retrieval status --project {project_arg} --format json"),
                    format!(
                        "codestory-cli retrieval bootstrap --project {project_arg} --format json"
                    ),
                    format!(
                        "codestory-cli retrieval index --project {project_arg} --refresh full --format json"
                    ),
                    format!("codestory-cli doctor --project {project_arg}"),
                ],
            );
        }
    }

    let minimum_next = match goal {
        ReadinessGoalDto::LocalNavigation => {
            vec![format!("codestory-cli ground --project {project_arg}")]
        }
        ReadinessGoalDto::AgentPacketSearch => vec![format!(
            "codestory-cli packet --project {project_arg} --question {}",
            quote_command_argument_value("How does this system work?")
        )],
    };
    (
        ReadinessStatusDto::Ready,
        match goal {
            ReadinessGoalDto::LocalNavigation => {
                "Local navigation can use the current index.".to_string()
            }
            ReadinessGoalDto::AgentPacketSearch => {
                "Agent packet/search can use the current index and sidecar retrieval.".to_string()
            }
        },
        minimum_next.clone(),
        minimum_next,
    )
}

fn index_repair_state(
    goal: ReadinessGoalDto,
    reason: &str,
    project_arg: &str,
    refresh: &str,
) -> (ReadinessStatusDto, String, Vec<String>, Vec<String>) {
    let command = format!("codestory-cli index --project {project_arg} --refresh {refresh}");
    (
        ReadinessStatusDto::RepairIndex,
        format!(
            "{} {} cannot be trusted until the index is repaired.",
            reason,
            goal_label(goal)
        ),
        vec![command.clone()],
        vec![
            command,
            format!("codestory-cli doctor --project {project_arg}"),
        ],
    )
}

fn readiness_index_snapshot(
    stats: &StorageStatsDto,
    freshness: Option<&IndexFreshnessDto>,
) -> ReadinessIndexSnapshotDto {
    ReadinessIndexSnapshotDto {
        status: freshness.map(|freshness| freshness.status),
        changed_file_count: freshness
            .map(|freshness| freshness.changed_file_count)
            .unwrap_or_default(),
        new_file_count: freshness
            .map(|freshness| freshness.new_file_count)
            .unwrap_or_default(),
        removed_file_count: freshness
            .map(|freshness| freshness.removed_file_count)
            .unwrap_or_default(),
        checked_file_count: freshness
            .map(|freshness| freshness.checked_file_count)
            .unwrap_or_default(),
        indexed_file_count: freshness
            .map(|freshness| freshness.indexed_file_count)
            .unwrap_or(stats.file_count),
    }
}

fn readiness_sidecar_snapshot(input: ReadinessSidecarInput<'_>) -> ReadinessSidecarSnapshotDto {
    ReadinessSidecarSnapshotDto {
        retrieval_mode: input.retrieval_mode.to_string(),
        degraded_reason: input.degraded_reason.map(ToOwned::to_owned),
        manifest_generation: input.manifest_generation.map(ToOwned::to_owned),
        manifest_input_hash: input.manifest_input_hash.map(ToOwned::to_owned),
    }
}

fn dedupe_commands(commands: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for command in commands {
        if !deduped.contains(&command) {
            deduped.push(command);
        }
    }
    deduped
}

fn project_arg(project: &str) -> String {
    quote_command_argument_value(&clean_path_string(project))
}
