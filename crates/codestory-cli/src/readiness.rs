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

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(node_count: u32) -> StorageStatsDto {
        StorageStatsDto {
            node_count,
            edge_count: node_count.saturating_sub(1),
            file_count: u32::from(node_count > 0),
            error_count: 0,
        }
    }

    fn freshness(status: IndexFreshnessStatusDto) -> IndexFreshnessDto {
        IndexFreshnessDto {
            status,
            changed_file_count: u32::from(status == IndexFreshnessStatusDto::Stale),
            new_file_count: 0,
            removed_file_count: 0,
            checked_file_count: 1,
            indexed_file_count: 1,
            duration_ms: 1,
            reason: None,
            samples: Vec::new(),
        }
    }

    fn inputs<'a>(
        stats: &'a StorageStatsDto,
        freshness: Option<&'a IndexFreshnessDto>,
        sidecar: Option<ReadinessSidecarInput<'a>>,
    ) -> ReadinessInputs<'a> {
        ReadinessInputs {
            project: "C:/workspace/project",
            stats,
            freshness,
            sidecar,
        }
    }

    #[test]
    fn missing_index_requires_index_repair_for_all_goals() {
        let stats = stats(0);
        let verdicts = build_readiness_verdicts(inputs(&stats, None, None));

        assert_eq!(verdicts.len(), 2);
        assert!(
            verdicts
                .iter()
                .all(|verdict| verdict.status == ReadinessStatusDto::RepairIndex),
            "missing index should block all readiness goals: {verdicts:?}"
        );
        assert!(
            verdicts[0].minimum_next[0].contains("--refresh full"),
            "missing index repair should request full refresh: {verdicts:?}"
        );
    }

    #[test]
    fn unchecked_index_requires_drift_check_before_ready() {
        let stats = stats(3);
        let freshness = freshness(IndexFreshnessStatusDto::NotChecked);
        let verdict = build_readiness_verdict(
            ReadinessGoalDto::LocalNavigation,
            inputs(&stats, Some(&freshness), None),
        );

        assert_eq!(verdict.status, ReadinessStatusDto::CheckIndex);
        assert_eq!(
            verdict.index.as_ref().and_then(|index| index.status),
            Some(IndexFreshnessStatusDto::NotChecked)
        );
        assert!(verdict.minimum_next[0].contains("--refresh incremental"));
    }

    #[test]
    fn stale_index_requires_incremental_repair() {
        let stats = stats(3);
        let freshness = freshness(IndexFreshnessStatusDto::Stale);
        let verdict = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(
                &stats,
                Some(&freshness),
                Some(ReadinessSidecarInput {
                    retrieval_mode: "full",
                    degraded_reason: None,
                    manifest_generation: Some("generation"),
                    manifest_input_hash: Some("hash"),
                }),
            ),
        );

        assert_eq!(verdict.status, ReadinessStatusDto::RepairIndex);
        assert!(verdict.minimum_next[0].contains("--refresh incremental"));
        assert!(verdict.summary.contains("changed, new, or removed files"));
    }

    #[test]
    fn agent_readiness_requires_full_sidecar_retrieval() {
        let stats = stats(3);
        let freshness = freshness(IndexFreshnessStatusDto::Fresh);
        let unavailable = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(&stats, Some(&freshness), None),
        );

        assert_eq!(unavailable.status, ReadinessStatusDto::RepairRetrieval);
        assert!(
            unavailable
                .summary
                .contains("current mode is `unavailable`")
        );
        assert!(unavailable.sidecar.is_none());

        let degraded = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(
                &stats,
                Some(&freshness),
                Some(ReadinessSidecarInput {
                    retrieval_mode: "no_semantic",
                    degraded_reason: Some("semantic store unavailable"),
                    manifest_generation: Some("generation"),
                    manifest_input_hash: Some("hash"),
                }),
            ),
        );

        assert_eq!(degraded.status, ReadinessStatusDto::RepairRetrieval);
        assert_eq!(
            degraded
                .sidecar
                .as_ref()
                .and_then(|sidecar| sidecar.degraded_reason.as_deref()),
            Some("semantic store unavailable")
        );
        assert!(
            degraded
                .full_repair
                .iter()
                .any(|command| command.contains("retrieval index")
                    && command.contains("--refresh full")),
            "non-full sidecar repair should include full retrieval index: {degraded:?}"
        );
    }
}
