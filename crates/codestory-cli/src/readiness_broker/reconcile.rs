use std::path::Path;

use crate::{local_refresh_status, ready_repair_status};

use super::operations::{
    operation_from_local_refresh_lock_cleanup, operation_from_local_refresh_status,
    operation_from_ready_status,
};
use super::paths::clean_path;
use super::types::BrokerReconciliationSnapshot;

pub(crate) fn reconcile_before_enqueue(
    project_root: &Path,
    cache_root: &Path,
    run_id: Option<&str>,
    cli_version: &str,
) -> BrokerReconciliationSnapshot {
    if let Some(active) = ready_repair_status::active_ready_repair_status(project_root, run_id) {
        return BrokerReconciliationSnapshot {
            status: "active_repair".to_string(),
            cleanup_performed: false,
            stale_status_paths_removed: Vec::new(),
            stale_lock_paths_removed: Vec::new(),
            abandoned_repairs: Vec::new(),
            local_refresh_cleanups: Vec::new(),
            active_repair: Some(operation_from_ready_status(
                project_root,
                cli_version,
                active,
                "running",
            )),
            unresolved_orphan_reason: None,
        };
    }

    let cleanups = ready_repair_status::cleanup_abandoned_ready_repair_status(project_root, run_id);
    let mut stale_status_paths_removed = Vec::new();
    let mut stale_lock_paths_removed = Vec::new();
    let mut abandoned_repairs = Vec::new();
    let mut local_refresh_cleanups = Vec::new();
    let mut unresolved_orphan_reason =
        ready_repair_status::stale_live_ready_repair_status(project_root, run_id).map(|status| {
            format!(
                "live_ready_repair_heartbeat_stale:pid={}:phase={}",
                status.pid, status.phase
            )
        });
    for cleanup in cleanups {
        if cleanup.removed_status_path {
            stale_status_paths_removed.push(clean_path(&cleanup.status_path));
        }
        stale_lock_paths_removed.extend(
            cleanup
                .removed_lock_paths
                .iter()
                .map(|path| clean_path(path)),
        );
        abandoned_repairs.push(operation_from_ready_status(
            project_root,
            cli_version,
            cleanup.status,
            "abandoned_cleaned",
        ));
    }
    if let Some(cleanup) =
        local_refresh_status::cleanup_stale_local_refresh_state(cache_root, project_root)
    {
        if cleanup.removed_status_path {
            stale_status_paths_removed.push(clean_path(&cleanup.status_path));
        }
        if cleanup.removed_lock_path {
            stale_lock_paths_removed.push(clean_path(&cleanup.lock_path));
        }
        if !cleanup.removed_status_path && !cleanup.removed_lock_path {
            unresolved_orphan_reason =
                Some(format!("local_refresh_cleanup_blocked:{}", cleanup.reason));
        }
        let operation = match cleanup.status {
            Some(status) => operation_from_local_refresh_status(
                project_root,
                cli_version,
                status,
                if cleanup.removed_status_path || cleanup.removed_lock_path {
                    "stale_cleaned"
                } else {
                    "stale_live"
                },
                Some(cleanup.reason),
            ),
            None => operation_from_local_refresh_lock_cleanup(project_root, cli_version, cleanup),
        };
        local_refresh_cleanups.push(operation);
    }
    let cleanup_performed =
        !stale_status_paths_removed.is_empty() || !stale_lock_paths_removed.is_empty();
    BrokerReconciliationSnapshot {
        status: if cleanup_performed {
            "stale_state_cleaned".to_string()
        } else if unresolved_orphan_reason.is_some() {
            "orphan_unresolved".to_string()
        } else {
            "clean".to_string()
        },
        cleanup_performed,
        stale_status_paths_removed,
        stale_lock_paths_removed,
        abandoned_repairs,
        local_refresh_cleanups,
        active_repair: None,
        unresolved_orphan_reason,
    }
}
