use anyhow::Result;
use std::collections::BTreeMap;
use std::fs;

use crate::display;
use crate::{local_refresh_status, ready_repair_status};

use super::gpu_proof::{bind_verified_runtime_identity, gpu_proof, inherit_verified_smoke};
use super::machine_lock::{NATIVE_EMBEDDING_RESOURCE, machine_resource_snapshot};
use super::operations::{operation_from_local_refresh, operation_from_ready_status};
use super::paths::{
    BROKER_SNAPSHOT_FILE, broker_snapshot_path, clean_path, install_id, now_epoch_ms,
};
use super::scope::{BROKER_SCHEMA_VERSION, effective_record_identity, legacy_canonical_root_hash};
use super::types::{
    BrokerGpuRuntimeIdentity, BrokerReconciliationSnapshot, BrokerSnapshotInput,
    ReadinessBrokerSnapshot,
};

pub(crate) fn refresh_broker_snapshot(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    #[cfg(test)]
    return super::paths::with_test_broker_root(|| refresh_broker_snapshot_inner(input));
    #[cfg(not(test))]
    refresh_broker_snapshot_inner(input)
}

fn refresh_broker_snapshot_inner(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    let identity = codestory_workspace::project_identity_v3(&input.project_root);
    let runtime_identity = current_gpu_runtime_identity(&input, &identity);
    refresh_broker_snapshot_with_identity(input, identity, runtime_identity.as_ref(), None)
}

fn refresh_broker_snapshot_with_identity(
    input: BrokerSnapshotInput,
    identity: codestory_workspace::ProjectIdentityV3,
    runtime_identity: Option<&BrokerGpuRuntimeIdentity>,
    sidecar: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> ReadinessBrokerSnapshot {
    let mut snapshot = build_broker_snapshot(input, identity, sidecar);
    if let Some(proof) = snapshot.gpu_proof.as_mut() {
        bind_verified_runtime_identity(proof, runtime_identity);
    }
    snapshot.updated_at_epoch_ms = now_epoch_ms();
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

pub(crate) fn observe_broker_snapshot(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    #[cfg(test)]
    return super::paths::with_test_broker_root(|| observe_broker_snapshot_inner(input));
    #[cfg(not(test))]
    observe_broker_snapshot_inner(input)
}

pub(crate) fn observe_broker_snapshot_for_sidecar(
    input: BrokerSnapshotInput,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> ReadinessBrokerSnapshot {
    #[cfg(test)]
    return super::paths::with_test_broker_root(|| {
        observe_broker_snapshot_for_sidecar_inner(input, sidecar)
    });
    #[cfg(not(test))]
    observe_broker_snapshot_for_sidecar_inner(input, sidecar)
}

pub(crate) fn refresh_broker_snapshot_for_sidecar(
    input: BrokerSnapshotInput,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> ReadinessBrokerSnapshot {
    #[cfg(test)]
    return super::paths::with_test_broker_root(|| {
        refresh_broker_snapshot_for_sidecar_inner(input, sidecar)
    });
    #[cfg(not(test))]
    refresh_broker_snapshot_for_sidecar_inner(input, sidecar)
}

fn observe_broker_snapshot_for_sidecar_inner(
    input: BrokerSnapshotInput,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> ReadinessBrokerSnapshot {
    let identity = codestory_workspace::cached_project_identity_v3(&input.project_root);
    let runtime_identity =
        gpu_runtime_identity_for_sidecar(sidecar, &input.project_root, &identity);
    observe_broker_snapshot_with_identity(input, identity, runtime_identity.as_ref(), Some(sidecar))
}

fn refresh_broker_snapshot_for_sidecar_inner(
    input: BrokerSnapshotInput,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> ReadinessBrokerSnapshot {
    let identity = codestory_workspace::project_identity_v3(&input.project_root);
    let runtime_identity =
        gpu_runtime_identity_for_sidecar(sidecar, &input.project_root, &identity);
    refresh_broker_snapshot_with_identity(input, identity, runtime_identity.as_ref(), Some(sidecar))
}

fn observe_broker_snapshot_inner(input: BrokerSnapshotInput) -> ReadinessBrokerSnapshot {
    let identity = codestory_workspace::cached_project_identity_v3(&input.project_root);
    let runtime_identity = current_gpu_runtime_identity(&input, &identity);
    observe_broker_snapshot_with_identity(input, identity, runtime_identity.as_ref(), None)
}

fn observe_broker_snapshot_with_identity(
    input: BrokerSnapshotInput,
    identity: codestory_workspace::ProjectIdentityV3,
    runtime_identity: Option<&BrokerGpuRuntimeIdentity>,
    sidecar: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> ReadinessBrokerSnapshot {
    let project_root = input.project_root.clone();
    let mut snapshot = build_broker_snapshot(input, identity, sidecar);
    if let Some(proof) = snapshot.gpu_proof.as_mut() {
        bind_verified_runtime_identity(proof, runtime_identity);
    }
    let path = broker_snapshot_path(&snapshot.canonical_root_hash);
    snapshot.snapshot_path = Some(clean_path(&path));

    let persisted = read_snapshot_file(&path, &project_root).or_else(|| {
        let legacy_hash = legacy_canonical_root_hash(&project_root.to_string_lossy())?;
        read_snapshot_file(&broker_snapshot_path(&legacy_hash), &project_root)
    });
    if let Some(persisted) = persisted {
        if let (Some(observed_proof), Some(persisted_proof)) =
            (snapshot.gpu_proof.as_mut(), persisted.gpu_proof.as_ref())
        {
            inherit_verified_smoke(observed_proof, persisted_proof, runtime_identity);
        }
        if snapshots_have_same_state(&snapshot, &persisted) {
            snapshot.updated_at_epoch_ms = persisted.updated_at_epoch_ms;
            snapshot.persistence_status = "persisted".to_string();
            snapshot.persistence_error = persisted.persistence_error;
            return snapshot;
        }
    }

    snapshot.persistence_status = "observed".to_string();
    snapshot.persistence_error = None;
    snapshot
}

#[cfg(test)]
pub(super) fn refresh_broker_snapshot_with_runtime_identity(
    input: BrokerSnapshotInput,
    runtime_identity: Option<&BrokerGpuRuntimeIdentity>,
) -> ReadinessBrokerSnapshot {
    super::paths::with_test_broker_root(|| {
        let identity = codestory_workspace::project_identity_v3(&input.project_root);
        refresh_broker_snapshot_with_identity(input, identity, runtime_identity, None)
    })
}

#[cfg(test)]
pub(super) fn observe_broker_snapshot_with_runtime_identity(
    input: BrokerSnapshotInput,
    runtime_identity: Option<&BrokerGpuRuntimeIdentity>,
) -> ReadinessBrokerSnapshot {
    super::paths::with_test_broker_root(|| {
        let identity = codestory_workspace::cached_project_identity_v3(&input.project_root);
        observe_broker_snapshot_with_identity(input, identity, runtime_identity, None)
    })
}

fn current_gpu_runtime_identity(
    input: &BrokerSnapshotInput,
    expected_project_identity: &codestory_workspace::ProjectIdentityV3,
) -> Option<BrokerGpuRuntimeIdentity> {
    let profile = if input.agent_run_id.is_some() {
        codestory_retrieval::SidecarProfile::Agent
    } else {
        codestory_retrieval::SidecarProfile::Local
    };
    #[cfg(not(test))]
    let runtime = crate::sidecar_runtime::for_project_with_run_id(
        &input.project_root,
        profile,
        input.agent_run_id.as_deref(),
    );
    #[cfg(test)]
    let runtime = crate::sidecar_runtime::for_project_with_run_id_in_cache(
        Some(&input.project_root),
        profile,
        input.agent_run_id.as_deref(),
        &super::paths::broker_cache_root(),
    );
    gpu_runtime_identity_for_sidecar(&runtime, &input.project_root, expected_project_identity)
}

pub(super) fn gpu_runtime_identity_for_sidecar(
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    project_root: &std::path::Path,
    expected_project_identity: &codestory_workspace::ProjectIdentityV3,
) -> Option<BrokerGpuRuntimeIdentity> {
    let state: codestory_retrieval::SidecarStateFile =
        serde_json::from_slice(&fs::read(&runtime.layout.state_file).ok()?).ok()?;
    if !codestory_retrieval::sidecar_state_matches_runtime(&state, runtime) {
        return None;
    }
    if runtime.project_identity.as_ref()? != expected_project_identity
        || expected_project_identity.workspace_id
            != codestory_workspace::workspace_id_v3_for_root(project_root)
        || state.started_at_epoch_ms <= 0
    {
        return None;
    }
    if let Some(launch) = state.embedding_launch.as_ref()
        && launch.launch_mode
            == codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned.as_str()
    {
        let validated_pid =
            codestory_retrieval::ensure_native_embedding_launch_identity(launch).ok()?;
        if launch.pid != Some(validated_pid) {
            return None;
        }
    }
    Some(BrokerGpuRuntimeIdentity {
        workspace_id: expected_project_identity.workspace_id.clone(),
        profile: state.profile,
        run_id: state.run_id,
        namespace: state.namespace,
        compose_project: state.compose_project,
        embed_url: state.embed_url,
        started_at_epoch_ms: state.started_at_epoch_ms,
        embedding_launch: state.embedding_launch,
    })
}

fn build_broker_snapshot(
    input: BrokerSnapshotInput,
    identity: codestory_workspace::ProjectIdentityV3,
    sidecar: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> ReadinessBrokerSnapshot {
    let canonical_root_hash = identity.workspace_id.clone();
    let project_id = identity.project_id.clone();
    let cli_version = input.cli_version;
    let mut operations = Vec::new();
    let active_repair = match sidecar {
        Some(sidecar) => ready_repair_status::active_ready_repair_status_for_sidecar(
            &input.project_root,
            sidecar,
        ),
        None => ready_repair_status::active_ready_repair_status(
            &input.project_root,
            input.agent_run_id.as_deref(),
        )
        .or_else(|| ready_repair_status::active_ready_repair_status(&input.project_root, None)),
    };
    if let Some(active) = active_repair {
        operations.push(operation_from_ready_status(
            &input.project_root,
            &cli_version,
            active,
            "running",
        ));
    } else if let Some(abandoned) = match sidecar {
        Some(sidecar) => ready_repair_status::abandoned_ready_repair_status_for_sidecar(
            &input.project_root,
            sidecar,
        ),
        None => ready_repair_status::abandoned_ready_repair_status(
            &input.project_root,
            input.agent_run_id.as_deref(),
        ),
    } {
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
        identity: Some(identity),
        install_id: install_id(),
        project_id,
        canonical_root_hash,
        workspace_root: display::clean_path_string(&input.project_root.to_string_lossy()),
        cli_version,
        updated_at_epoch_ms: latest_authoritative_epoch_ms(&operations, &resources),
        snapshot_path: None,
        persistence_status: "pending".to_string(),
        persistence_error: None,
        operations,
        resources,
        reconciliation,
        gpu_proof: input.gpu_proof.map(gpu_proof),
    }
}

fn read_snapshot_file(
    path: &std::path::Path,
    expected_project_root: &std::path::Path,
) -> Option<ReadinessBrokerSnapshot> {
    let mut snapshot: ReadinessBrokerSnapshot =
        serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let identity = snapshot.effective_identity()?;
    if identity.workspace_id != codestory_workspace::workspace_id_v3_for_root(expected_project_root)
        || !codestory_workspace::same_workspace_path(
            std::path::Path::new(&snapshot.workspace_root),
            expected_project_root,
        )
    {
        return None;
    }
    if snapshot.schema_version != BROKER_SCHEMA_VERSION
        && let Some(runtime_identity) = snapshot
            .gpu_proof
            .as_mut()
            .and_then(|proof| proof.runtime_identity.as_mut())
        && runtime_identity.workspace_id
            == codestory_workspace::workspace_id_for_root(expected_project_root)
    {
        runtime_identity.workspace_id = identity.workspace_id;
    }
    Some(snapshot)
}

impl ReadinessBrokerSnapshot {
    pub(crate) fn effective_identity(&self) -> Option<codestory_workspace::ProjectIdentityV3> {
        effective_record_identity(
            self.schema_version,
            self.identity.as_ref(),
            &self.project_id,
            &self.canonical_root_hash,
            &self.workspace_root,
        )
    }
}

fn snapshots_have_same_state(
    observed: &ReadinessBrokerSnapshot,
    persisted: &ReadinessBrokerSnapshot,
) -> bool {
    let mut observed = observed.clone();
    let mut persisted = persisted.clone();
    normalize_snapshot_metadata(&mut observed);
    normalize_snapshot_metadata(&mut persisted);
    observed == persisted
}

fn normalize_snapshot_metadata(snapshot: &mut ReadinessBrokerSnapshot) {
    snapshot.updated_at_epoch_ms = 0;
    snapshot.snapshot_path = None;
    snapshot.persistence_status.clear();
    snapshot.persistence_error = None;
}

fn latest_authoritative_epoch_ms(
    operations: &[super::types::BrokerOperationSnapshot],
    resources: &BTreeMap<String, super::types::BrokerResourceSnapshot>,
) -> i64 {
    operations
        .iter()
        .flat_map(|operation| [operation.started_at_epoch_ms, operation.updated_at_epoch_ms])
        .flatten()
        .chain(
            resources
                .values()
                .filter_map(|resource| resource.started_at_epoch_ms),
        )
        .max()
        .unwrap_or(0)
}

pub(crate) fn write_snapshot_file(
    path: &std::path::Path,
    snapshot: &ReadinessBrokerSnapshot,
) -> Result<()> {
    crate::file_state::write_json_atomic(path, BROKER_SNAPSHOT_FILE, snapshot)
}
