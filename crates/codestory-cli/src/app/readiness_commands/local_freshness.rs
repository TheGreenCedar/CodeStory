use crate::args;
use crate::args::ProjectArgs;
use crate::runtime::RuntimeContext;
use crate::{display, local_refresh_status, readiness};
use anyhow::Result;
use codestory_contracts::api::{
    IndexFreshnessStatusDto, ProjectSummary, ReadinessGoalDto, ReadinessStatusDto,
};

pub(crate) fn wait_for_local_freshness(
    project: &ProjectArgs,
    inspect_runtime: &RuntimeContext,
) -> Result<(ProjectSummary, Option<readiness::LocalRefreshOutput>)> {
    let summary = inspect_runtime.open_project_summary()?;
    if !local_freshness_needs_refresh(&summary) {
        let mut output = local_refresh_output_from_summary(&summary);
        if output.state == readiness::LocalRefreshState::Refreshed {
            output.reason = Some("already_fresh".to_string());
        }
        return Ok((summary, Some(output)));
    }

    let lock = match local_refresh_status::try_acquire_local_refresh_lock(
        &inspect_runtime.cache_root,
        &inspect_runtime.project_root,
    )? {
        local_refresh_status::LocalRefreshLockAttempt::Acquired(lock) => lock,
        local_refresh_status::LocalRefreshLockAttempt::Busy(busy) => {
            let mut output = local_refresh_output_from_summary(&summary);
            output.state = readiness::LocalRefreshState::Refreshing;
            output.blocks_local_surfaces = true;
            output.readiness_status = ReadinessStatusDto::RepairIndex;
            output.reason = Some(if busy.status.is_some() {
                "refreshing".to_string()
            } else {
                "refresh_lock_held".to_string()
            });
            if let Some(status) = busy.status {
                output.phase = Some(status.phase);
                output.pid = Some(status.pid);
                output.started_at_epoch_ms = Some(status.started_at_epoch_ms);
                output.updated_at_epoch_ms = Some(status.updated_at_epoch_ms);
                output.last_failure_reason = status.last_failure_reason;
            } else {
                output.pid = busy.pid;
                output.started_at_epoch_ms = busy.started_at_epoch_ms;
                output.phase = Some("starting".to_string());
            }
            output.lock_path = Some(display::clean_path_string(
                &busy.lock_path.to_string_lossy(),
            ));
            attach_complete_publication(&mut output, &summary);
            return Ok((summary, Some(output)));
        }
    };
    let summary = inspect_runtime.open_project_summary()?;
    if !local_freshness_needs_refresh(&summary) {
        let mut output = local_refresh_output_from_summary(&summary);
        output.reason = Some("coalesced_refresh_completed".to_string());
        return Ok((summary, Some(output)));
    }
    let refresh_started_at_epoch_ms = lock.started_at_epoch_ms();
    let refresh_pid = lock.pid();
    let refresh_phase = "incremental_index";
    if !lock.write_status(
        &inspect_runtime.project_root,
        "refreshing",
        refresh_phase,
        None,
    )? {
        anyhow::bail!("local refresh ownership changed before indexing");
    }
    let heartbeat = local_refresh_status::LocalRefreshHeartbeat::start(
        &lock,
        &inspect_runtime.project_root,
        refresh_phase,
    );

    let index_runtime = RuntimeContext::new(project)?;
    let refresh_result = index_runtime.ensure_open(args::RefreshMode::Incremental);
    heartbeat.stop();
    match refresh_result {
        Ok(opened) => {
            let _ = lock.write_status(
                &inspect_runtime.project_root,
                "refreshed",
                refresh_phase,
                None,
            );
            let mut output = local_refresh_output_from_summary(&opened.summary);
            output.phase = Some(refresh_phase.to_string());
            output.pid = Some(refresh_pid);
            output.started_at_epoch_ms = Some(refresh_started_at_epoch_ms);
            output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
            if output.state == readiness::LocalRefreshState::Refreshed {
                output.reason = Some("refreshed".to_string());
            } else {
                output.state = readiness::LocalRefreshState::Failed;
                output.blocks_local_surfaces = true;
                output.reason = Some("refresh_did_not_reach_fresh".to_string());
            }
            attach_complete_publication(&mut output, &opened.summary);
            Ok((opened.summary, Some(output)))
        }
        Err(error) => {
            let error_text = error.to_string();
            let _ = lock.write_status(
                &inspect_runtime.project_root,
                "failed",
                refresh_phase,
                Some(error_text.clone()),
            );
            let mut output = local_refresh_output_from_summary(&summary);
            output.state = classify_local_refresh_failure_state(&error);
            output.blocks_local_surfaces = true;
            output.readiness_status = ReadinessStatusDto::RepairIndex;
            output.reason = Some(error_text.clone());
            output.phase = Some(refresh_phase.to_string());
            output.pid = Some(refresh_pid);
            output.started_at_epoch_ms = Some(refresh_started_at_epoch_ms);
            output.updated_at_epoch_ms = Some(local_refresh_status::now_epoch_ms());
            output.last_failure_reason = Some(error_text);
            attach_complete_publication(&mut output, &summary);
            Ok((summary, Some(output)))
        }
    }
}

pub(crate) fn attach_complete_publication(
    output: &mut readiness::LocalRefreshOutput,
    summary: &ProjectSummary,
) {
    output.serving_publication = summary
        .publication
        .as_ref()
        .and_then(|publication| serde_json::to_value(publication).ok());
    if output.serving_publication.is_some()
        && output.state == readiness::LocalRefreshState::Refreshing
    {
        output.blocks_local_surfaces = false;
        output.readiness_status = ReadinessStatusDto::Ready;
    }
}

pub(in crate::app) fn local_freshness_needs_refresh(summary: &ProjectSummary) -> bool {
    summary.freshness.as_ref().is_some_and(|freshness| {
        matches!(
            freshness.status,
            IndexFreshnessStatusDto::Stale | IndexFreshnessStatusDto::NotChecked
        )
    })
}

pub(crate) fn local_refresh_output_from_summary(
    summary: &ProjectSummary,
) -> readiness::LocalRefreshOutput {
    let verdict = readiness::build_readiness_verdict(
        ReadinessGoalDto::LocalNavigation,
        readiness::ReadinessInputs {
            project: &summary.root,
            stats: &summary.stats,
            freshness: summary.freshness.as_ref(),
            sidecar: None,
        },
    );
    readiness::local_refresh_output(&verdict)
}

pub(in crate::app) fn classify_local_refresh_failure_state(
    error: &anyhow::Error,
) -> readiness::LocalRefreshState {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("cache_busy")
        || message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("cache is busy")
    {
        readiness::LocalRefreshState::Skipped
    } else {
        readiness::LocalRefreshState::Failed
    }
}
