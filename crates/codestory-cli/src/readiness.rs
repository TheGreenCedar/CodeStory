use codestory_contracts::api::{
    IndexFreshnessDto, IndexFreshnessStatusDto, ReadinessGoalDto, ReadinessIndexSnapshotDto,
    ReadinessSetupSnapshotDto, ReadinessSidecarSnapshotDto, ReadinessStatusDto,
    ReadinessVerdictDto, StorageStatsDto,
};

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

pub(crate) fn goal_label(goal: ReadinessGoalDto) -> &'static str {
    match goal {
        ReadinessGoalDto::LocalNavigation => "local_navigation",
        ReadinessGoalDto::AgentPacketSearch => "agent_packet_search",
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

    if goal == ReadinessGoalDto::AgentPacketSearch {
        let sidecar_mode = sidecar
            .map(|sidecar| sidecar.retrieval_mode)
            .unwrap_or("unavailable");
        if sidecar_mode != "full" {
            let full_repair = agent_packet_search_repair_commands(
                project_arg,
                !matches!(
                    freshness.map(|freshness| freshness.status),
                    Some(IndexFreshnessStatusDto::Fresh)
                ),
            );
            let minimum_next = full_repair.iter().take(2).cloned().collect();
            return (
                ReadinessStatusDto::RepairRetrieval,
                format!(
                    "Agent packet/search needs full sidecar retrieval; current mode is `{sidecar_mode}`."
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

fn agent_packet_search_repair_commands(project_arg: &str, include_core_index: bool) -> Vec<String> {
    let mut commands = Vec::new();
    commands.push(ready_repair_command(
        ReadinessGoalDto::AgentPacketSearch,
        project_arg,
    ));
    if include_core_index {
        commands.push(format!("codestory-cli doctor --project {project_arg}"));
    }
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
    let command = ready_repair_command(goal, project_arg);
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

fn ready_repair_command(goal: ReadinessGoalDto, project_arg: &str) -> String {
    format!(
        "codestory-cli ready --goal {} --repair --project {project_arg} --format json",
        ready_goal_cli_label(goal)
    )
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
            }),
            sidecar: Some(ReadinessSidecarInput {
                retrieval_mode: "full",
                degraded_reason: None,
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
    fn fatal_indexed_errors_block_ready_verdicts() {
        let mut stats = stats(3);
        stats.error_count = 2;
        stats.fatal_error_count = 2;
        let freshness = freshness(IndexFreshnessStatusDto::Fresh);
        let verdicts = build_readiness_verdicts(inputs(
            &stats,
            Some(&freshness),
            Some(ReadinessSidecarInput {
                retrieval_mode: "full",
                degraded_reason: None,
                manifest_generation: Some("generation"),
                manifest_input_hash: Some("hash"),
            }),
        ));

        assert!(
            verdicts
                .iter()
                .all(|verdict| verdict.status == ReadinessStatusDto::RepairIndex),
            "fatal index errors should block all readiness goals: {verdicts:?}"
        );
        assert!(
            verdicts
                .iter()
                .all(|verdict| verdict.summary.contains("2 fatal indexing errors")),
            "readiness should explain the recorded fatal index errors: {verdicts:?}"
        );
        assert!(
            verdicts
                .iter()
                .all(|verdict| verdict.minimum_next[0].contains("ready --goal")),
            "error-bearing indexes should request a full refresh repair: {verdicts:?}"
        );
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
        assert!(
            verdict.minimum_next[0].contains("ready --goal agent --repair"),
            "stale index repair should point at the one-command repair path: {verdict:?}"
        );
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
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn agent_readiness_keeps_core_index_repair_when_freshness_is_unknown() {
        let stats = stats(3);
        let verdict = build_readiness_verdict(
            ReadinessGoalDto::AgentPacketSearch,
            inputs(
                &stats,
                None,
                Some(ReadinessSidecarInput {
                    retrieval_mode: "unavailable",
                    degraded_reason: None,
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
            "unknown freshness should keep the conservative agent repair path: {verdict:?}"
        );
    }
}
