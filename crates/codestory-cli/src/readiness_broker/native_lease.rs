use anyhow::{Context, Result, bail};
use std::fs;
use std::time::{Duration, Instant};

use super::machine_lock::{
    BrokerMachineResourceBusy, BrokerMachineResourceLock, BrokerMachineResourceLockAttempt,
    NATIVE_EMBEDDING_RESOURCE, release_machine_resource_lock_for_native_launch,
    transfer_machine_resource_lock_to_native_launch, try_acquire_machine_resource_lock,
};
use super::types::{BrokerResourceSnapshot, BrokerScope};

#[derive(Debug)]
pub(crate) enum BrokerNativeEmbeddingResourceLease {
    Acquired(BrokerMachineResourceLock),
    Reused { pid: u32 },
}

pub(crate) fn transfer_native_embedding_resource_lease(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    state: &codestory_retrieval::SidecarStateFile,
) -> Result<()> {
    transfer_native_embedding_resource_lease_with_validator(
        lease,
        state,
        codestory_retrieval::ensure_native_embedding_launch_identity,
    )
}

pub(crate) fn transfer_native_embedding_resource_lease_with_validator(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    state: &codestory_retrieval::SidecarStateFile,
    mut validate_launch: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<()> {
    let Some(launch) = native_embedding_launch_from_sidecar_state(state) else {
        if matches!(
            lease,
            Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
        ) {
            bail!("reused native embedding broker lease missing final state pid");
        }
        return Ok(());
    };
    let Some(pid) = launch.pid else {
        if matches!(
            lease,
            Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
        ) {
            bail!("reused native embedding broker lease missing final state pid");
        }
        return Ok(());
    };
    match lease {
        Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)) => {
            let validated_pid = validate_launch(launch)
                .with_context(|| format!("validate native embedding broker handoff pid {pid}"))?;
            if validated_pid != pid {
                bail!(
                    "validated native embedding broker handoff pid mismatch: expected {pid}, got {validated_pid}"
                );
            }
            if !transfer_machine_resource_lock_to_native_launch(lock, launch)? {
                bail!("native embedding broker lock handoff failed for pid {pid}");
            }
        }
        Some(BrokerNativeEmbeddingResourceLease::Reused { pid: reused_pid }) => {
            let validated_pid = validate_launch(launch)
                .with_context(|| format!("validate reused native embedding broker pid {pid}"))?;
            if validated_pid != pid {
                bail!(
                    "validated reused native embedding broker pid mismatch: expected {pid}, got {validated_pid}"
                );
            }
            if *reused_pid != pid {
                bail!(
                    "reused native embedding broker lease pid mismatch: expected {reused_pid}, got {pid}"
                );
            }
        }
        None => bail!("native embedding process spawned without broker machine lock"),
    }
    Ok(())
}

pub(crate) fn cleanup_native_embedding_resource_lease_after_bootstrap_error(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<()> {
    cleanup_native_embedding_resource_lease_after_bootstrap_error_with_cleanup(
        lease,
        sidecar,
        || codestory_retrieval::sidecar_down_for_runtime(sidecar),
        codestory_retrieval::ensure_native_embedding_launch_identity,
    )
}

pub(crate) fn cleanup_native_embedding_resource_lease_after_bootstrap_error_with_cleanup(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    cleanup: impl FnOnce() -> Result<()>,
    validate_launch: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<()> {
    if matches!(lease, Some(BrokerNativeEmbeddingResourceLease::Acquired(_))) {
        match cleanup() {
            Ok(()) => {}
            Err(cleanup_error) => {
                if let Some(state) = read_sidecar_state_file(sidecar)? {
                    transfer_native_embedding_resource_lease_with_validator(
                        lease,
                        &state,
                        validate_launch,
                    )
                    .with_context(|| {
                        format!(
                            "preserve native embedding broker lock after cleanup failed: {cleanup_error}"
                        )
                    })?;
                }
                return Err(cleanup_error);
            }
        }
    }
    Ok(())
}

pub(crate) fn cleanup_native_embedding_resource_lease_after_transfer_error(
    lease: &Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<()> {
    cleanup_native_embedding_resource_lease_after_transfer_error_with_cleanup(lease, || {
        codestory_retrieval::sidecar_down_for_runtime(sidecar)
    })
}

pub(crate) fn cleanup_native_embedding_resource_lease_after_transfer_error_with_cleanup(
    lease: &Option<BrokerNativeEmbeddingResourceLease>,
    cleanup: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if matches!(
        lease,
        Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
    ) {
        return Ok(());
    }
    cleanup()
}

pub(crate) fn native_embedding_launch_from_sidecar_state_file(
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<Option<codestory_retrieval::EmbeddingLaunchMetadata>> {
    Ok(read_sidecar_state_file(sidecar)?
        .and_then(|state| native_embedding_launch_from_sidecar_state(&state).cloned()))
}

pub(crate) fn cleanup_transferred_native_embedding_resource_after_error(
    lease: &Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<()> {
    if matches!(
        lease,
        Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
    ) {
        return Ok(());
    }
    let launch = native_embedding_launch_from_sidecar_state_file(sidecar)?;
    cleanup_transferred_native_embedding_resource_after_error_with_cleanup(
        lease,
        launch.as_ref(),
        || codestory_retrieval::sidecar_down_for_runtime(sidecar),
        |launch| release_machine_resource_lock_for_native_launch(NATIVE_EMBEDDING_RESOURCE, launch),
    )
}

pub(crate) fn cleanup_transferred_native_embedding_resource_after_error_with_cleanup(
    lease: &Option<BrokerNativeEmbeddingResourceLease>,
    launch: Option<&codestory_retrieval::EmbeddingLaunchMetadata>,
    cleanup: impl FnOnce() -> Result<()>,
    mut release: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<bool>,
) -> Result<()> {
    if matches!(
        lease,
        Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
    ) {
        return Ok(());
    }
    cleanup()?;
    if let Some(launch) = launch {
        release(launch)?;
    }
    Ok(())
}

fn native_embedding_launch_from_sidecar_state(
    state: &codestory_retrieval::SidecarStateFile,
) -> Option<&codestory_retrieval::EmbeddingLaunchMetadata> {
    state.embedding_launch.as_ref().filter(|launch| {
        launch.launch_mode == codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned.as_str()
    })
}

fn read_sidecar_state_file(
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<Option<codestory_retrieval::SidecarStateFile>> {
    if !sidecar.layout.state_file.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&sidecar.layout.state_file)
        .with_context(|| format!("read {}", sidecar.layout.state_file.display()))?;
    let state = serde_json::from_str(&contents)
        .with_context(|| format!("parse {}", sidecar.layout.state_file.display()))?;
    Ok(Some(state))
}

pub(crate) fn acquire_native_embedding_resource_lease_if_needed(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    wait: Duration,
    poll: Duration,
) -> Result<Option<BrokerNativeEmbeddingResourceLease>> {
    acquire_native_embedding_resource_lease_if_needed_with_validator(
        scope,
        sidecar,
        wait,
        poll,
        codestory_retrieval::ensure_native_embedding_launch_identity,
    )
}

/// Shared native-embedding lease lifecycle used by ready-repair and retrieval bootstrap.
///
/// Ordering is fixed:
/// acquire → bootstrap → bootstrap cleanup on Err → transfer → transfer cleanup on Err →
/// post-transfer → transferred cleanup on Err.
pub(crate) struct NativeEmbeddingLeaseLifecycleParams<'a> {
    pub(crate) scope: &'a BrokerScope,
    pub(crate) sidecar: &'a codestory_retrieval::SidecarRuntimeConfig,
    pub(crate) wait: Duration,
    pub(crate) poll: Duration,
    pub(crate) bootstrap_context: &'a str,
    pub(crate) sidecar_cleanup_label: &'a str,
}

pub(crate) fn run_with_native_embedding_lease_lifecycle<Bootstrap, Output>(
    params: NativeEmbeddingLeaseLifecycleParams<'_>,
    bootstrap: impl FnOnce(bool) -> Result<Bootstrap>,
    bootstrap_state: impl FnOnce(&Bootstrap) -> &codestory_retrieval::SidecarStateFile,
    post_transfer: impl FnOnce(Bootstrap) -> Result<Output>,
) -> Result<Output> {
    let NativeEmbeddingLeaseLifecycleParams {
        scope,
        sidecar,
        wait,
        poll,
        bootstrap_context,
        sidecar_cleanup_label,
    } = params;
    let mut embedding_resource_lease =
        acquire_native_embedding_resource_lease_if_needed(scope, sidecar, wait, poll)?;
    let allow_native_embedding_spawn = !matches!(
        embedding_resource_lease,
        Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
    );
    let bootstrap = match bootstrap(allow_native_embedding_spawn) {
        Ok(report) => report,
        Err(error) => {
            if let Err(cleanup_error) =
                cleanup_native_embedding_resource_lease_after_bootstrap_error(
                    &mut embedding_resource_lease,
                    sidecar,
                )
            {
                return Err(error).context(format!(
                    "{bootstrap_context}; native embedding cleanup failed: {cleanup_error}"
                ));
            }
            return Err(error).context(bootstrap_context.to_string());
        }
    };
    if let Err(error) = transfer_native_embedding_resource_lease(
        &mut embedding_resource_lease,
        bootstrap_state(&bootstrap),
    ) {
        cleanup_native_embedding_resource_lease_after_transfer_error(
            &embedding_resource_lease,
            sidecar,
        )
        .with_context(|| {
            format!(
                "cleanup {sidecar_cleanup_label} after native embedding lease transfer failed: {error}"
            )
        })?;
        return Err(error).context("native embedding lease transfer");
    }
    match post_transfer(bootstrap) {
        Ok(output) => Ok(output),
        Err(error) => {
            cleanup_transferred_native_embedding_resource_after_error(
                &embedding_resource_lease,
                sidecar,
            )
            .with_context(|| {
                format!("cleanup {sidecar_cleanup_label} after post-transfer failure: {error}")
            })?;
            Err(error)
        }
    }
}

fn acquire_native_embedding_resource_lease_if_needed_with_validator(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    wait: Duration,
    poll: Duration,
    mut validate_launch: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<BrokerNativeEmbeddingResourceLease>> {
    if codestory_retrieval::embedding_server_launch_mode()?
        != codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned
    {
        return Ok(None);
    }
    let deadline = Instant::now() + wait;
    loop {
        match try_acquire_machine_resource_lock(NATIVE_EMBEDDING_RESOURCE, scope)? {
            BrokerMachineResourceLockAttempt::Acquired(lock) => {
                return Ok(Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)));
            }
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                if let Some(pid) = reusable_native_embedding_resource_pid(
                    scope,
                    sidecar,
                    &busy,
                    &mut validate_launch,
                )? {
                    return Ok(Some(BrokerNativeEmbeddingResourceLease::Reused { pid }));
                }
                if Instant::now() >= deadline {
                    return bail_native_embedding_busy(&busy);
                }
                std::thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    }
}

pub(crate) fn reusable_native_embedding_resource_pid_for_snapshot(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    snapshot: &BrokerResourceSnapshot,
) -> Result<Option<u32>> {
    if snapshot.status != "busy" || snapshot.resource != NATIVE_EMBEDDING_RESOURCE {
        return Ok(None);
    }
    let busy = BrokerMachineResourceBusy {
        snapshot: snapshot.clone(),
    };
    let mut validate_launch = codestory_retrieval::ensure_native_embedding_launch_identity;
    reusable_native_embedding_resource_pid(scope, sidecar, &busy, &mut validate_launch)
}

pub(crate) fn reusable_native_embedding_resource_pid(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    busy: &BrokerMachineResourceBusy,
    validate_launch: &mut impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<u32>> {
    let Some(owner_pid) = busy.snapshot.owner_pid else {
        return Ok(None);
    };
    if busy.snapshot.owner_project_id.as_deref() != Some(scope.project_id.as_str())
        || busy.snapshot.owner_workspace_root.as_deref() != Some(scope.workspace_root.as_str())
    {
        return Ok(None);
    }
    let Some(state) = read_sidecar_state_file(sidecar)? else {
        return Ok(None);
    };
    if !codestory_retrieval::sidecar_state_matches_runtime(&state, sidecar) {
        return Ok(None);
    }
    let Some(launch) = state.embedding_launch.as_ref() else {
        return Ok(None);
    };
    if launch.launch_mode != codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned.as_str()
        || launch.endpoint
            != codestory_retrieval::SidecarLayout::embed_base_url(sidecar.embed_http_port)
        || launch.pid != Some(owner_pid)
    {
        return Ok(None);
    }
    let validated_pid = validate_launch(launch)
        .with_context(|| format!("validate reusable native embedding pid {owner_pid}"))?;
    if validated_pid != owner_pid {
        bail!(
            "validated reusable native embedding pid mismatch: expected {owner_pid}, got {validated_pid}"
        );
    }
    Ok(Some(owner_pid))
}

fn bail_native_embedding_busy<T>(busy: &BrokerMachineResourceBusy) -> Result<T> {
    let owner = busy
        .snapshot
        .owner_workspace_root
        .as_deref()
        .unwrap_or("unknown");
    bail!(
        "native embedding runtime is busy for another CodeStory operation: resource={} owner_project={} owner_workspace={} owner_pid={:?}; retry after the current repair reaches full retrieval",
        busy.snapshot.resource,
        busy.snapshot
            .owner_project_id
            .as_deref()
            .unwrap_or("unknown"),
        owner,
        busy.snapshot.owner_pid
    );
}
