use anyhow::Result;
use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::display;
use crate::{local_refresh_status, ready_repair_status};

use super::gpu_proof::gpu_proof;
use super::machine_lock::{NATIVE_EMBEDDING_RESOURCE, machine_resource_snapshot};
use super::operations::{operation_from_local_refresh, operation_from_ready_status};
use super::paths::{
    BROKER_SNAPSHOT_FILE, broker_snapshot_path, clean_path, clean_path_text, hash_text, install_id,
    now_epoch_ms, project_id_from_hash,
};
use super::scope::BROKER_SCHEMA_VERSION;
use super::types::{BrokerReconciliationSnapshot, BrokerSnapshotInput, ReadinessBrokerSnapshot};

static SNAPSHOT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn refresh_broker_snapshot(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    let mut snapshot = build_broker_snapshot(input);
    let path = broker_snapshot_path(&snapshot.canonical_root_hash);
    snapshot.snapshot_path = Some(clean_path(&path));
    snapshot.persistence_status = "persisted".to_string();
    snapshot.persistence_error = None;
    match write_snapshot_file(&path, &snapshot) {
        Ok(()) => {}
        Err(error) => {
            snapshot.persistence_status = "failed".to_string();
            snapshot.persistence_error = Some(error.to_string());
        }
    }
    snapshot
}

pub(crate) fn build_broker_snapshot(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    let canonical_root = clean_path_text(&input.project_root);
    let canonical_root_hash = hash_text(&canonical_root);
    let project_id = project_id_from_hash(&canonical_root_hash);
    let cli_version = input.cli_version;
    let mut operations = Vec::new();
    if let Some(active) = ready_repair_status::active_ready_repair_status(
        &input.project_root,
        input.agent_run_id.as_deref(),
    ) {
        operations.push(operation_from_ready_status(
            &input.project_root,
            &cli_version,
            active,
            "running",
        ));
    } else if let Some(abandoned) = ready_repair_status::abandoned_ready_repair_status(
        &input.project_root,
        input.agent_run_id.as_deref(),
    ) {
        operations.push(operation_from_ready_status(
            &input.project_root,
            &cli_version,
            abandoned,
            "abandoned",
        ));
    }
    if let Some(local_refresh) =
        local_refresh_status::active_local_refresh_status(&input.cache_root, &input.project_root)
    {
        operations.push(operation_from_local_refresh(
            &input.project_root,
            &cli_version,
            local_refresh,
        ));
    }

    let mut resources = BTreeMap::new();
    resources.insert(
        NATIVE_EMBEDDING_RESOURCE.to_string(),
        machine_resource_snapshot(NATIVE_EMBEDDING_RESOURCE),
    );
    let reconciliation = input
        .reconciliation
        .unwrap_or_else(|| BrokerReconciliationSnapshot {
            status: "observed".to_string(),
            cleanup_performed: false,
            stale_status_paths_removed: Vec::new(),
            stale_lock_paths_removed: Vec::new(),
            abandoned_repairs: Vec::new(),
            local_refresh_cleanups: Vec::new(),
            active_repair: operations
                .iter()
                .find(|operation| {
                    operation.operation_kind == "agent_repair" && operation.status == "running"
                })
                .cloned(),
            unresolved_orphan_reason: None,
        });

    ReadinessBrokerSnapshot {
        schema_version: BROKER_SCHEMA_VERSION,
        install_id: install_id(),
        project_id,
        canonical_root_hash,
        workspace_root: display::clean_path_string(&input.project_root.to_string_lossy()),
        cli_version,
        updated_at_epoch_ms: now_epoch_ms(),
        snapshot_path: None,
        persistence_status: "pending".to_string(),
        persistence_error: None,
        operations,
        resources,
        reconciliation,
        gpu_proof: input.gpu_proof.map(gpu_proof),
    }
}

pub(crate) fn write_snapshot_file(
    path: &std::path::Path,
    snapshot: &ReadinessBrokerSnapshot,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let counter = SNAPSHOT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        BROKER_SNAPSHOT_FILE,
        std::process::id(),
        counter
    ));
    fs::write(&temp_path, serde_json::to_string_pretty(snapshot)?)?;
    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error) if path.exists() => {
            let _ = fs::remove_file(&temp_path);
            Err(error.into())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(error.into())
        }
    }
}
