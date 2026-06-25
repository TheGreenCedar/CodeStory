use codestory_contracts::api::{
    IndexFreshnessDto, IndexFreshnessStatusDto, ReadinessGoalDto, ReadinessIndexSnapshotDto,
    ReadinessSetupSnapshotDto, ReadinessSidecarSnapshotDto, ReadinessStatusDto,
    ReadinessVerdictDto, StorageStatsDto,
};
use serde::Serialize;

use crate::display::{clean_path_string, quote_command_argument_value};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadinessInputs<'a> {
    pub(crate) project: &'a str,
    pub(crate) stats: &'a StorageStatsDto,
    pub(crate) freshness: Option<&'a IndexFreshnessDto>,
    pub(crate) setup: Option<&'a ReadinessSetupInput>,
    pub(crate) sidecar: Option<ReadinessSidecarInput<'a>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReadinessSetupInput {
    pub(crate) active_path: String,
    pub(crate) active_version: String,
    pub(crate) latest_version: String,
    pub(crate) newer_installed_path: Option<String>,
    pub(crate) newer_installed_version: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadinessSidecarInput<'a> {
    pub(crate) profile: Option<&'a str>,
    pub(crate) run_id: Option<&'a str>,
    pub(crate) retrieval_mode: &'a str,
    pub(crate) degraded_reason: Option<&'a str>,
    pub(crate) embedding_device_policy: Option<&'a str>,
    pub(crate) embedding_device_state: Option<&'a str>,
    pub(crate) embedding_cpu_allowed: bool,
    pub(crate) manifest_generation: Option<&'a str>,
    pub(crate) manifest_input_hash: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalRefreshState {
    Fresh,
    Stale,
    NotChecked,
    SkippedLocked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LocalRefreshOutput {
    pub(crate) state: LocalRefreshState,
    pub(crate) blocks_local_surfaces: bool,
    pub(crate) readiness_status: ReadinessStatusDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) changed_file_count: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) new_file_count: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) removed_file_count: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) fatal_error_count: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
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
    let setup = inputs.setup.map(readiness_setup_snapshot);
    let sidecar = inputs.sidecar.map(readiness_sidecar_snapshot);

    let (status, summary, minimum_next, full_repair) = verdict_state(
        goal,
        inputs.setup,
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
        setup,
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
        ReadinessStatusDto::RepairSetup => "repair_setup",
        ReadinessStatusDto::RepairIndex => "repair_index",
        ReadinessStatusDto::CheckIndex => "check_index",
        ReadinessStatusDto::RepairRetrieval => "repair_retrieval",
    }
}

pub(crate) fn failed_layer(verdict: &ReadinessVerdictDto) -> Option<&'static str> {
    match verdict.status {
        ReadinessStatusDto::Ready => None,
        ReadinessStatusDto::RepairSetup => Some("runtime_setup"),
        ReadinessStatusDto::RepairIndex => Some("local_index"),
        ReadinessStatusDto::CheckIndex => Some("index_freshness"),
        ReadinessStatusDto::RepairRetrieval => Some("retrieval_sidecar"),
    }
}

pub(crate) fn goal_label(goal: ReadinessGoalDto) -> &'static str {
    match goal {
        ReadinessGoalDto::LocalNavigation => "local_navigation",
        ReadinessGoalDto::AgentPacketSearch => "agent_packet_search",
    }
}

pub(crate) fn local_refresh_state_label(state: LocalRefreshState) -> &'static str {
    match state {
        LocalRefreshState::Fresh => "fresh",
        LocalRefreshState::Stale => "stale",
        LocalRefreshState::NotChecked => "not_checked",
        LocalRefreshState::SkippedLocked => "skipped_locked",
        LocalRefreshState::Failed => "failed",
    }
}

pub(crate) fn local_refresh_output(verdict: &ReadinessVerdictDto) -> LocalRefreshOutput {
    let index = verdict.index.as_ref();
    let state = match verdict.status {
        ReadinessStatusDto::Ready | ReadinessStatusDto::RepairRetrieval => LocalRefreshState::Fresh,
        ReadinessStatusDto::CheckIndex => LocalRefreshState::NotChecked,
        ReadinessStatusDto::RepairSetup => LocalRefreshState::Failed,
        ReadinessStatusDto::RepairIndex => {
            if index.and_then(|index| index.status) == Some(IndexFreshnessStatusDto::Stale) {
                LocalRefreshState::Stale
            } else {
                LocalRefreshState::Failed
            }
        }
    };
    let reason = match state {
        LocalRefreshState::Fresh => None,
        LocalRefreshState::Stale => Some("index_changed".to_string()),
        LocalRefreshState::NotChecked => Some("freshness_not_checked".to_string()),
        LocalRefreshState::SkippedLocked => Some("index_locked".to_string()),
        LocalRefreshState::Failed => Some(verdict.summary.clone()),
    };

    LocalRefreshOutput {
        state,
        blocks_local_surfaces: verdict.status != ReadinessStatusDto::Ready,
        readiness_status: verdict.status,
        reason,
        changed_file_count: index
            .map(|index| index.changed_file_count)
            .unwrap_or_default(),
        new_file_count: index.map(|index| index.new_file_count).unwrap_or_default(),
        removed_file_count: index
            .map(|index| index.removed_file_count)
            .unwrap_or_default(),
        fatal_error_count: index
            .map(|index| index.fatal_error_count)
            .unwrap_or_default(),
    }
}

fn verdict_state(
    goal: ReadinessGoalDto,
    setup: Option<&ReadinessSetupInput>,
    stats: &StorageStatsDto,
    freshness: Option<&IndexFreshnessDto>,
    sidecar: Option<ReadinessSidecarInput<'_>>,
    project_arg: &str,
) -> (ReadinessStatusDto, String, Vec<String>, Vec<String>) {
    if let Some(setup) = setup {
        if let (Some(newer_path), Some(newer_version)) = (
            setup.newer_installed_path.as_ref(),
            setup.newer_installed_version.as_ref(),
        ) {
            let restart = format!(
                "Restart/reload the Codex host/app so MCP relaunches codestory-cli {newer_version} from {newer_path}; then open a fresh agent thread and read codestory://status."
            );
            return (
                ReadinessStatusDto::RepairSetup,
                format!(
                    "Active codestory-cli {} at {} is older than latest release {}; a newer installed codestory-cli {} exists at {}. Restart or reload the host before retrying CodeStory surfaces.",
                    setup.active_version,
                    setup.active_path,
                    setup.latest_version,
                    newer_version,
                    newer_path
                ),
                vec![restart.clone()],
                vec![
                    restart,
                    "where.exe codestory-cli".to_string(),
                    "codestory-cli --version".to_string(),
                ],
            );
        }
        let install = format!(
            "powershell -NoProfile -ExecutionPolicy Bypass -Command '$installer = Join-Path $env:TEMP \"install-codestory.ps1\"; Invoke-WebRequest -UseBasicParsing -Uri \"https://raw.githubusercontent.com/TheGreenCedar/CodeStory/v{}/scripts/install-codestory.ps1\" -OutFile $installer; & $installer -Project {project_arg} -Version {}'",
            setup.latest_version, setup.latest_version
        );
        return (
            ReadinessStatusDto::RepairSetup,
            format!(
                "Active codestory-cli {} at {} is older than latest release {}; repair setup before using CodeStory surfaces.",
                setup.active_version, setup.active_path, setup.latest_version
            ),
            vec![install.clone()],
            vec![
                install,
                "where.exe codestory-cli".to_string(),
                "codestory-cli --version".to_string(),
            ],
        );
    }

    if goal == ReadinessGoalDto::LocalNavigation {
        if stats.node_count == 0 {
            return index_repair_state(goal, "No indexed symbols are available yet.", project_arg);
        }

        if stats.fatal_error_count > 0 {
            let plural = if stats.fatal_error_count == 1 {
                ""
            } else {
                "s"
            };
            return index_repair_state(
                goal,
                &format!(
                    "The index recorded {} fatal indexing error{plural}.",
                    stats.fatal_error_count
                ),
                project_arg,
            );
        }

        match freshness.map(|freshness| freshness.status) {
            Some(IndexFreshnessStatusDto::Stale) => {
                return index_repair_state(
                    goal,
                    "The index has changed, new, or removed files.",
                    project_arg,
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
    }

    if goal == ReadinessGoalDto::AgentPacketSearch {
        let sidecar_profile = sidecar.and_then(|sidecar| sidecar.profile);
        let sidecar_run_id = sidecar.and_then(|sidecar| sidecar.run_id);
        let sidecar_mode = sidecar
            .map(|sidecar| sidecar.retrieval_mode)
            .unwrap_or("unavailable");
        if sidecar_mode != "full" || sidecar_profile != Some("agent") {
            let device_note = sidecar
                .and_then(|sidecar| {
                    sidecar
                        .embedding_device_policy
                        .zip(sidecar.embedding_device_state)
                })
                .map(|(policy, state)| {
                    format!(" embedding_device_policy=`{policy}` observed_device=`{state}`.")
                })
                .unwrap_or_default();
            let full_repair = agent_packet_search_repair_commands(project_arg, sidecar_run_id);
            let minimum_next = full_repair.iter().take(1).cloned().collect();
            return (
                ReadinessStatusDto::RepairRetrieval,
                format!(
                    "Agent packet/search needs full agent sidecar retrieval; current profile is `{}` and mode is `{sidecar_mode}`.{device_note}",
                    sidecar_profile.unwrap_or("unknown")
                ),
                minimum_next,
                full_repair,
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
        ready_summary_with_errors(
            match goal {
                ReadinessGoalDto::LocalNavigation => "Local navigation can use the current index.",
                ReadinessGoalDto::AgentPacketSearch => {
                    "Agent packet/search can use the current index and sidecar retrieval."
                }
            },
            stats,
        ),
        minimum_next.clone(),
        minimum_next,
    )
}

fn ready_summary_with_errors(base: &str, stats: &StorageStatsDto) -> String {
    if stats.error_count > stats.fatal_error_count {
        let nonfatal_count = stats.error_count - stats.fatal_error_count;
        let plural = if nonfatal_count == 1 { "" } else { "s" };
        format!(
            "{base} Recorded {nonfatal_count} nonfatal indexing error{plural}; inspect doctor for partial coverage."
        )
    } else {
        base.to_string()
    }
}

fn agent_packet_search_repair_commands(project_arg: &str, run_id: Option<&str>) -> Vec<String> {
    let mut commands = Vec::new();
    commands.push(ready_repair_command(
        ReadinessGoalDto::AgentPacketSearch,
        project_arg,
        run_id,
    ));
    commands.extend([
        format!("codestory-cli retrieval status --project {project_arg} --format json"),
        format!("codestory-cli doctor --project {project_arg} --format markdown"),
    ]);
    commands
}

fn index_repair_state(
    goal: ReadinessGoalDto,
    reason: &str,
    project_arg: &str,
) -> (ReadinessStatusDto, String, Vec<String>, Vec<String>) {
    let command = ready_repair_command(goal, project_arg, None);
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

fn ready_repair_command(goal: ReadinessGoalDto, project_arg: &str, run_id: Option<&str>) -> String {
    let mut command = format!(
        "codestory-cli ready --goal {} --repair --project {project_arg} --format json",
        ready_goal_cli_label(goal)
    );
    if goal == ReadinessGoalDto::AgentPacketSearch
        && let Some(run_id) = run_id
    {
        command.push_str(" --run-id ");
        command.push_str(&quote_command_argument_value(run_id));
    }
    command
}

fn ready_goal_cli_label(goal: ReadinessGoalDto) -> &'static str {
    match goal {
        ReadinessGoalDto::LocalNavigation => "local",
        ReadinessGoalDto::AgentPacketSearch => "agent",
    }
}

fn readiness_index_snapshot(
    stats: &StorageStatsDto,
    freshness: Option<&IndexFreshnessDto>,
) -> ReadinessIndexSnapshotDto {
    ReadinessIndexSnapshotDto {
        status: freshness.map(|freshness| freshness.status),
        error_count: stats.error_count,
        fatal_error_count: stats.fatal_error_count,
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

fn readiness_setup_snapshot(input: &ReadinessSetupInput) -> ReadinessSetupSnapshotDto {
    ReadinessSetupSnapshotDto {
        active_path: input.active_path.to_string(),
        active_version: input.active_version.to_string(),
        latest_version: input.latest_version.to_string(),
        newer_installed_path: input.newer_installed_path.clone(),
        newer_installed_version: input.newer_installed_version.clone(),
    }
}

fn readiness_sidecar_snapshot(input: ReadinessSidecarInput<'_>) -> ReadinessSidecarSnapshotDto {
    ReadinessSidecarSnapshotDto {
        profile: input.profile.map(ToOwned::to_owned),
        run_id: input.run_id.map(ToOwned::to_owned),
        retrieval_mode: input.retrieval_mode.to_string(),
        degraded_reason: input.degraded_reason.map(ToOwned::to_owned),
        embedding_device_policy: input.embedding_device_policy.map(ToOwned::to_owned),
        embedding_device_state: input.embedding_device_state.map(ToOwned::to_owned),
        embedding_cpu_allowed: input.embedding_cpu_allowed,
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
            fatal_error_count: 0,
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
            setup: None,
            sidecar,
        }
    }

    #[test]
    fn missing_index_blocks_local_graph_only() {
        let stats = stats(0);
        let verdicts = build_readiness_verdicts(inputs(&stats, None, None));

        assert_eq!(verdicts.len(), 2);
        assert_eq!(verdicts[0].status, ReadinessStatusDto::RepairIndex);
        assert_eq!(
            verdicts[1].status,
            ReadinessStatusDto::RepairRetrieval,
            "missing local index should not collapse agent retrieval readiness: {verdicts:?}"
        );
        assert!(
            verdicts[0].minimum_next[0].contains("ready --goal local --repair"),
            "missing index repair should request full refresh: {verdicts:?}"
        );
    }

    #[test]
    fn stale_active_cli_requires_setup_repair_before_index_or_sidecars() {
        let stats = stats(3);
        let freshness = freshness(IndexFreshnessStatusDto::Fresh);
        let verdicts = build_readiness_verdicts(ReadinessInputs {
            project: "C:/workspace/project",
            stats: &stats,
            freshness: Some(&freshness),
            setup: Some(&ReadinessSetupInput {
                active_path: "C:/Users/alber/.local/bin/codestory-cli.exe".to_string(),
                active_version: "0.11.6".to_string(),
                latest_version: "0.11.9".to_string(),
                newer_installed_path: None,
                newer_installed_version: None,
            }),
            sidecar: Some(ReadinessSidecarInput {
                profile: Some("agent"),
                run_id: Some("run"),
                retrieval_mode: "full",
                degraded_reason: None,
                embedding_device_policy: Some("accelerator_required"),
                embedding_device_state: Some("accelerated"),
                embedding_cpu_allowed: false,
                manifest_generation: Some("generation"),
                manifest_input_hash: Some("hash"),
            }),
        });

        assert!(
            verdicts
                .iter()
                .all(|verdict| verdict.status == ReadinessStatusDto::RepairSetup),
            "stale active CLI must block all readiness goals: {verdicts:?}"
        );
        for verdict in verdicts {
            let setup = verdict.setup.as_ref().expect("setup snapshot");
            assert_eq!(setup.active_version, "0.11.6");
            assert_eq!(setup.latest_version, "0.11.9");
            assert!(
                setup.active_path.contains("codestory-cli.exe"),
                "setup snapshot should expose stale executable path: {setup:?}"
            );
            assert!(
                verdict.minimum_next[0].contains("install-codestory.ps1")
                    && verdict.minimum_next[0].contains("0.11.9"),
                "stale CLI repair must be an install action, not advice: {verdict:?}"
            );
        }
    }

    #[test]
    fn stale_active_cli_with_newer_installed_binary_reports_restart_boundary() {
        let stats = stats(3);
        let freshness = freshness(IndexFreshnessStatusDto::Fresh);
        let verdicts = build_readiness_verdicts(ReadinessInputs {
            project: "C:/workspace/project",
            stats: &stats,
            freshness: Some(&freshness),
            setup: Some(&ReadinessSetupInput {
                active_path: "C:/Users/alber/.local/bin/codestory-cli.exe".to_string(),
                active_version: "0.11.10".to_string(),
                latest_version: "0.11.12".to_string(),
                newer_installed_path: Some(
                    "C:/Users/alber/AppData/Local/CodeStory/bin/codestory-cli.exe".to_string(),
                ),
                newer_installed_version: Some("0.11.11".to_string()),
            }),
            sidecar: Some(ReadinessSidecarInput {
                profile: Some("agent"),
                run_id: Some("run"),
                retrieval_mode: "full",
                degraded_reason: None,
                embedding_device_policy: Some("accelerator_required"),
                embedding_device_state: Some("accelerated"),
                embedding_cpu_allowed: false,
                manifest_generation: Some("generation"),
                manifest_input_hash: Some("hash"),
            }),
        });

        assert!(
            verdicts
                .iter()
                .all(|verdict| verdict.status == ReadinessStatusDto::RepairSetup),
            "stale active CLI must remain a setup repair state: {verdicts:?}"
        );
        for verdict in verdicts {
            let setup = verdict.setup.as_ref().expect("setup snapshot");
            assert_eq!(
                setup.newer_installed_path.as_deref(),
                Some("C:/Users/alber/AppData/Local/CodeStory/bin/codestory-cli.exe")
            );
            assert_eq!(setup.newer_installed_version.as_deref(), Some("0.11.11"));
            assert!(
                verdict.minimum_next[0].contains("Restart/reload the Codex host/app"),
                "stale CLI with newer installed binary should report the host boundary: {verdict:?}"
            );
            assert!(
                !verdict.minimum_next[0].contains("install-codestory.ps1"),
                "minimum_next should not repeat the installer after a newer binary is present: {verdict:?}"
            );
        }
    }

    #[test]
    fn fatal_indexed_errors_block_ready_verdicts() {
        let mut stats = stats(3);
        stats.error_count = 2;
        stats.fatal_error_count = 2;
        let freshness = freshness(IndexFreshnessStatusDto::Fresh);
        let verdicts = build_readiness_verdicts(inputs(
            &stats,
            Some(&freshness),
            Some(ReadinessSidecarInput {
                profile: Some("agent"),
                run_id: Some("run"),
                retrieval_mode: "full",
                degraded_reason: None,
                embedding_device_policy: Some("accelerator_required"),
                embedding_device_state: Some("accelerated"),
                embedding_cpu_allowed: false,
                manifest_generation: Some("generation"),
                manifest_input_hash: Some("hash"),
            }),
        ));

        assert_eq!(verdicts[0].status, ReadinessStatusDto::RepairIndex);
        assert_eq!(
            verdicts[1].status,
            ReadinessStatusDto::Ready,
            "fatal local index errors should not block full sidecar retrieval readiness: {verdicts:?}"
        );
        assert!(
            verdicts[0].summary.contains("2 fatal indexing errors"),
            "local readiness should explain the recorded fatal index errors: {verdicts:?}"
        );
        assert!(
            verdicts[0].minimum_next[0].contains("ready --goal"),
            "error-bearing indexes should request a full refresh repair: {verdicts:?}"
        );
        let refresh = local_refresh_output(&verdicts[0]);
        assert_eq!(refresh.state, LocalRefreshState::Failed);
        assert!(refresh.blocks_local_surfaces);
        assert_eq!(refresh.fatal_error_count, 2);
    }

    #[test]
    fn nonfatal_index_errors_keep_ready_with_partial_coverage_warning() {
        let mut stats = stats(3);
        stats.error_count = 2;
        let freshness = freshness(IndexFreshnessStatusDto::Fresh);
        let verdict = build_readiness_verdict(
            ReadinessGoalDto::LocalNavigation,
            inputs(&stats, Some(&freshness), None),
        );

        assert_eq!(verdict.status, ReadinessStatusDto::Ready);
        assert_eq!(
            local_refresh_output(&verdict).state,
            LocalRefreshState::Fresh
        );
        assert!(
            verdict.summary.contains("2 nonfatal indexing errors"),
            "nonfatal errors should be visible without blocking local navigation: {verdict:?}"
        );
        assert_eq!(
            verdict.index.as_ref().map(|index| index.error_count),
            Some(2)
        );
        assert_eq!(
            verdict.index.as_ref().map(|index| index.fatal_error_count),
            Some(0)
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
        let refresh = local_refresh_output(&verdict);
        assert_eq!(refresh.state, LocalRefreshState::NotChecked);
        assert!(refresh.blocks_local_surfaces);
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
            ReadinessGoalDto::LocalNavigation,
            inputs(&stats, Some(&freshness), None),
        );

        assert_eq!(verdict.status, ReadinessStatusDto::RepairIndex);
        let refresh = local_refresh_output(&verdict);
        assert_eq!(refresh.state, LocalRefreshState::Stale);
        assert!(refresh.blocks_local_surfaces);
        assert_eq!(refresh.changed_file_count, 1);
        assert!(
            verdict.minimum_next[0].contains("ready --goal local --repair"),
            "stale index repair should point at the one-command repair path: {verdict:?}"
        );
        assert!(verdict.summary.contains("changed, new, or removed files"));

        let agent = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(
                &stats,
                Some(&freshness),
                Some(ReadinessSidecarInput {
                    profile: Some("agent"),
                    run_id: Some("run"),
                    retrieval_mode: "full",
                    degraded_reason: None,
                    embedding_device_policy: Some("accelerator_required"),
                    embedding_device_state: Some("accelerated"),
                    embedding_cpu_allowed: false,
                    manifest_generation: Some("generation"),
                    manifest_input_hash: Some("hash"),
                }),
            ),
        );
        assert_eq!(
            agent.status,
            ReadinessStatusDto::Ready,
            "stale local graph should not block full sidecar packet/search readiness: {agent:?}"
        );
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
                .contains("current profile is `unknown` and mode is `unavailable`")
        );
        assert!(unavailable.sidecar.is_none());

        let local = build_readiness_verdict(
            ReadinessGoalDto::LocalNavigation,
            inputs(&stats, Some(&freshness), None),
        );
        let refresh = local_refresh_output(&local);
        assert_eq!(refresh.state, LocalRefreshState::Fresh);
        assert!(!refresh.blocks_local_surfaces);

        let local_full = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(
                &stats,
                Some(&freshness),
                Some(ReadinessSidecarInput {
                    profile: Some("local"),
                    run_id: None,
                    retrieval_mode: "full",
                    degraded_reason: None,
                    embedding_device_policy: Some("accelerator_required"),
                    embedding_device_state: Some("accelerated"),
                    embedding_cpu_allowed: false,
                    manifest_generation: Some("generation"),
                    manifest_input_hash: Some("hash"),
                }),
            ),
        );

        assert_eq!(local_full.status, ReadinessStatusDto::RepairRetrieval);
        assert!(
            local_full.summary.contains("current profile is `local`"),
            "local/default full retrieval must not unlock agent packet/search: {local_full:?}"
        );

        let degraded = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(
                &stats,
                Some(&freshness),
                Some(ReadinessSidecarInput {
                    profile: Some("agent"),
                    run_id: Some("run"),
                    retrieval_mode: "no_semantic",
                    degraded_reason: Some("semantic store unavailable"),
                    embedding_device_policy: Some("accelerator_required"),
                    embedding_device_state: Some("unknown"),
                    embedding_cpu_allowed: false,
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
                .summary
                .contains("embedding_device_policy=`accelerator_required`"),
            "blocked full retrieval should expose device policy: {degraded:?}"
        );
        assert!(
            degraded
                .full_repair
                .first()
                .is_some_and(|command| command.contains("ready --goal agent --repair")),
            "fresh-index sidecar repair should start with the one-command repair path: {degraded:?}"
        );
        assert!(
            degraded
                .full_repair
                .iter()
                .any(|command| command.contains("retrieval status")
                    && command.contains("--format json")),
            "non-full sidecar full repair should include retrieval status proof: {degraded:?}"
        );
        assert!(
            degraded.full_repair.last().is_some_and(
                |command| command.contains("doctor") && command.contains("--format markdown")
            ),
            "non-full sidecar full repair should finish with markdown doctor proof: {degraded:?}"
        );
        assert_eq!(
            degraded.minimum_next,
            degraded
                .full_repair
                .iter()
                .take(1)
                .cloned()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_full_sidecars_report_retrieval_layer_and_one_canonical_repair() {
        let stats = stats(3);
        let freshness = freshness(IndexFreshnessStatusDto::Fresh);
        for sidecar in [
            None,
            Some(ReadinessSidecarInput {
                profile: Some("local"),
                run_id: None,
                retrieval_mode: "unavailable",
                degraded_reason: Some("manifest:<missing>"),
                embedding_device_policy: Some("accelerator_required"),
                embedding_device_state: Some("unknown"),
                embedding_cpu_allowed: false,
                manifest_generation: None,
                manifest_input_hash: None,
            }),
        ] {
            let verdict = build_readiness_verdict(
                ReadinessGoalDto::AgentPacketSearch,
                inputs(&stats, Some(&freshness), sidecar),
            );

            assert_eq!(verdict.status, ReadinessStatusDto::RepairRetrieval);
            assert_eq!(failed_layer(&verdict), Some("retrieval_sidecar"));
            assert_eq!(verdict.minimum_next.len(), 1);
            assert!(
                verdict.minimum_next[0].contains("ready --goal agent --repair"),
                "sidecar repair should expose one canonical repair command: {verdict:?}"
            );
        }
    }

    #[test]
    fn agent_readiness_uses_sidecar_gate_when_freshness_is_unknown() {
        let stats = stats(3);
        let verdict = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(
                &stats,
                None,
                Some(ReadinessSidecarInput {
                    profile: Some("agent"),
                    run_id: Some("run"),
                    retrieval_mode: "unavailable",
                    degraded_reason: None,
                    embedding_device_policy: Some("accelerator_required"),
                    embedding_device_state: Some("unknown"),
                    embedding_cpu_allowed: false,
                    manifest_generation: None,
                    manifest_input_hash: None,
                }),
            ),
        );

        assert_eq!(verdict.status, ReadinessStatusDto::RepairRetrieval);
        assert!(
            verdict
                .full_repair
                .first()
                .is_some_and(|command| command.contains("ready --goal agent --repair")),
            "agent readiness should keep the agent sidecar repair path: {verdict:?}"
        );
        assert!(
            !verdict
                .full_repair
                .iter()
                .any(|command| command == "codestory-cli doctor --project C:/workspace/project"),
            "unknown local freshness should not inject local graph repair into agent readiness: {verdict:?}"
        );
    }
}
