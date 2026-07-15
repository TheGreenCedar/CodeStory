use std::path::Path;

use crate::display;
use crate::{local_refresh_status, ready_repair_status};

use super::scope::{agent_repair_scope, broker_operation_id};
use super::types::BrokerOperationSnapshot;

pub(crate) fn operation_from_ready_status(
    project_root: &Path,
    cli_version: &str,
    status: ready_repair_status::ReadyRepairStatus,
    operation_status: &str,
) -> BrokerOperationSnapshot {
    let scope = agent_repair_scope(project_root, status.run_id.as_deref(), cli_version);
    BrokerOperationSnapshot {
        operation_id: broker_operation_id(&scope),
        operation_kind: "agent_repair".to_string(),
        status: operation_status.to_string(),
        project_id: scope.project_id,
        workspace_root: scope.workspace_root,
        profile: status.profile,
        run_id: status.run_id.clone(),
        agent_id: status.run_id,
        namespace: Some(status.namespace),
        phase: Some(status.phase),
        pid: Some(status.pid),
        started_at_epoch_ms: Some(status.started_at_epoch_ms),
        updated_at_epoch_ms: Some(status.updated_at_epoch_ms),
        degraded_reason: None,
    }
}

pub(crate) fn operation_from_local_refresh(
    project_root: &Path,
    cli_version: &str,
    status: local_refresh_status::LocalRefreshStatus,
) -> BrokerOperationSnapshot {
    operation_from_local_refresh_status(project_root, cli_version, status, "running", None)
}

pub(crate) fn operation_from_local_refresh_status(
    project_root: &Path,
    cli_version: &str,
    status: local_refresh_status::LocalRefreshStatus,
    operation_status: &str,
    degraded_reason: Option<String>,
) -> BrokerOperationSnapshot {
    let identity = codestory_workspace::project_identity_v3(project_root);
    let project_id = identity.project_id;
    let workspace_id = identity.workspace_id;
    BrokerOperationSnapshot {
        operation_id: format!("local_graph_refresh:{workspace_id}"),
        operation_kind: "local_graph_refresh".to_string(),
        status: operation_status.to_string(),
        project_id,
        workspace_root: display::clean_path_string(&project_root.to_string_lossy()),
        profile: "local".to_string(),
        run_id: None,
        agent_id: None,
        namespace: None,
        phase: Some(status.phase),
        pid: Some(status.pid),
        started_at_epoch_ms: Some(status.started_at_epoch_ms),
        updated_at_epoch_ms: Some(status.updated_at_epoch_ms),
        degraded_reason: degraded_reason
            .or(status.last_failure_reason)
            .or_else(|| (cli_version.is_empty()).then(|| "missing_cli_version".to_string())),
    }
}

pub(crate) fn operation_from_local_refresh_lock_cleanup(
    project_root: &Path,
    cli_version: &str,
    cleanup: local_refresh_status::LocalRefreshCleanup,
) -> BrokerOperationSnapshot {
    let identity = codestory_workspace::project_identity_v3(project_root);
    let project_id = identity.project_id;
    let workspace_id = identity.workspace_id;
    BrokerOperationSnapshot {
        operation_id: format!("local_graph_refresh:{workspace_id}"),
        operation_kind: "local_graph_refresh".to_string(),
        status: "stale_cleaned".to_string(),
        project_id,
        workspace_root: display::clean_path_string(&project_root.to_string_lossy()),
        profile: "local".to_string(),
        run_id: None,
        agent_id: None,
        namespace: None,
        phase: Some("unknown".to_string()),
        pid: cleanup.lock_pid,
        started_at_epoch_ms: cleanup.lock_started_at_epoch_ms,
        updated_at_epoch_ms: None,
        degraded_reason: Some(cleanup.reason)
            .or_else(|| (cli_version.is_empty()).then(|| "missing_cli_version".to_string())),
    }
}
