use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use super::machine_lock::{
    BrokerMachineResourceBusy, BrokerMachineResourceLock, BrokerMachineResourceLockAttempt,
    BrokerMachineResourceReaperLock, NATIVE_EMBEDDING_RESOURCE, machine_lock_owner_state,
    machine_resource_snapshot, quarantine_machine_resource_lock_for_native_launch,
    read_machine_resource_lock_file, release_machine_resource_lock_for_native_launch_with_guard,
    release_owned_quarantined_machine_resource_lock,
    release_quarantined_machine_resource_lock_for_native_launch,
    reset_owned_quarantined_machine_resource_lock, transfer_machine_resource_lock_to_native_launch,
    try_acquire_machine_resource_reaper_lock, try_acquire_native_embedding_machine_resource_lock,
};
use super::paths::machine_resource_lock_path;
use super::types::{BrokerResourceSnapshot, BrokerScope};

const MAX_NATIVE_EMBEDDING_BIND_RETRIES: usize = 2;

#[derive(Debug)]
pub(crate) enum BrokerNativeEmbeddingResourceLease {
    Acquired(BrokerMachineResourceLock),
    Reused {
        pid: u32,
        launch: Box<codestory_retrieval::EmbeddingLaunchMetadata>,
        _handoff_guard: BrokerMachineResourceReaperLock,
    },
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
        Some(BrokerNativeEmbeddingResourceLease::Reused {
            pid: reused_pid, ..
        }) => {
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

fn quarantine_native_embedding_resource_lease_before_finalization(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    state: &codestory_retrieval::SidecarStateFile,
) -> Result<()> {
    let Some(BrokerNativeEmbeddingResourceLease::Acquired(_)) = lease else {
        return Ok(());
    };
    let launch = native_embedding_launch_from_sidecar_state(state)
        .context("native embedding bootstrap state is missing its owned launch")?;
    quarantine_native_embedding_resource_lease_for_launch(lease, launch, "finalization_pending")
}

fn quarantine_native_embedding_resource_lease_for_launch(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
    reason: &str,
) -> Result<()> {
    let Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)) = lease else {
        bail!("new native embedding launch is missing its acquired broker machine lock");
    };
    if !quarantine_machine_resource_lock_for_native_launch(lock, launch, reason)? {
        bail!("native embedding broker pre-handoff quarantine publication failed");
    }
    Ok(())
}

fn cleanup_native_embedding_resource_lease_after_bootstrap_failure(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    bootstrap_error: &anyhow::Error,
    cleanup_published_state: bool,
) -> Result<()> {
    let quarantined_launch =
        codestory_retrieval::native_embedding_startup_cleanup_failure(bootstrap_error)
            .map(|failure| failure.launch().clone());
    cleanup_native_embedding_resource_lease_after_bootstrap_error_with_cleanup_and_quarantine(
        lease,
        sidecar,
        || {
            if cleanup_published_state {
                codestory_retrieval::sidecar_down_after_failed_bootstrap_for_runtime(sidecar)
            } else {
                Ok(())
            }
        },
        codestory_retrieval::ensure_native_embedding_launch_identity,
        quarantined_launch.as_ref(),
        false,
    )
}

fn cleanup_native_embedding_resource_lease_before_bind_retry(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    cleanup_published_state: bool,
) -> Result<()> {
    cleanup_native_embedding_resource_lease_after_bootstrap_error_with_cleanup_and_quarantine(
        lease,
        sidecar,
        || {
            if cleanup_published_state {
                codestory_retrieval::sidecar_down_after_failed_bootstrap_for_runtime(sidecar)
            } else {
                Ok(())
            }
        },
        codestory_retrieval::ensure_native_embedding_launch_identity,
        None,
        true,
    )
}

fn cleanup_native_embedding_resource_lease_after_bootstrap_error_with_cleanup_and_quarantine(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    cleanup: impl FnOnce() -> Result<()>,
    mut validate_launch: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
    quarantined_launch: Option<&codestory_retrieval::EmbeddingLaunchMetadata>,
    retain_acquired_lock: bool,
) -> Result<()> {
    let cleanup_result = cleanup();
    if let (Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)), Some(quarantined_launch)) =
        (lease.as_mut(), quarantined_launch)
    {
        if !quarantine_machine_resource_lock_for_native_launch(
            lock,
            quarantined_launch,
            "pre_state_cleanup_failed",
        )? {
            bail!("native embedding broker quarantine publication failed");
        }
        return cleanup_result;
    }
    match lease {
        Some(BrokerNativeEmbeddingResourceLease::Reused { .. }) | None => cleanup_result,
        Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)) => match cleanup_result {
            Ok(()) => {
                let retained = if retain_acquired_lock {
                    reset_owned_quarantined_machine_resource_lock(lock)?
                } else {
                    release_owned_quarantined_machine_resource_lock(lock)?
                };
                if retain_acquired_lock && !retained {
                    bail!("native embedding broker lock reset failed before bind retry");
                }
                Ok(())
            }
            Err(cleanup_error) => {
                if let Some(state) = read_sidecar_state_file(sidecar)? {
                    let launch = native_embedding_launch_from_sidecar_state(&state)
                        .context("cleanup failure state is missing an owned native launch")?;
                    let expected_pid = launch
                        .pid
                        .context("cleanup failure native launch is missing pid")?;
                    let validated_pid = validate_launch(launch).with_context(|| {
                        format!(
                            "validate native embedding launch after cleanup failed: {cleanup_error}"
                        )
                    })?;
                    if validated_pid != expected_pid {
                        bail!(
                            "validated cleanup failure pid mismatch: expected {expected_pid}, got {validated_pid}"
                        );
                    }
                    if !quarantine_machine_resource_lock_for_native_launch(
                        lock,
                        launch,
                        "cleanup_failed_before_handoff",
                    )? {
                        bail!("native embedding broker quarantine publication failed");
                    }
                }
                Err(cleanup_error)
            }
        },
    }
}

pub(crate) fn cleanup_native_embedding_resource_lease_after_transfer_error(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<()> {
    cleanup_native_embedding_resource_lease_after_transfer_error_with_cleanup(
        lease,
        sidecar,
        || codestory_retrieval::sidecar_down_after_failed_bootstrap_for_runtime(sidecar),
        codestory_retrieval::ensure_native_embedding_launch_identity,
    )
}

pub(crate) fn cleanup_native_embedding_resource_lease_after_transfer_error_with_cleanup(
    lease: &mut Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    cleanup: impl FnOnce() -> Result<()>,
    validate_launch: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<()> {
    cleanup_native_embedding_resource_lease_after_bootstrap_error_with_cleanup_and_quarantine(
        lease,
        sidecar,
        cleanup,
        validate_launch,
        None,
        false,
    )
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
    codestory_retrieval::validate_sidecar_state_matches_runtime(&state, sidecar).with_context(
        || {
            format!(
                "preserve mismatched sidecar state {} before native lease cleanup",
                sidecar.layout.state_file.display()
            )
        },
    )?;
    Ok(Some(state))
}

/// Shared native-embedding lease lifecycle used by ready-repair and retrieval bootstrap.
///
/// Ordering is fixed:
/// acquire → bootstrap (with bounded bind-race retry) → bootstrap cleanup on Err →
/// finalization → finalization cleanup on Err → transfer.
pub(crate) struct NativeEmbeddingLeaseLifecycleParams<'a> {
    pub(crate) scope: &'a BrokerScope,
    pub(crate) sidecar: &'a mut codestory_retrieval::SidecarRuntimeConfig,
    pub(crate) resource: &'a str,
    pub(crate) wait: Duration,
    pub(crate) poll: Duration,
    pub(crate) bootstrap_context: &'a str,
    pub(crate) sidecar_cleanup_label: &'a str,
}

pub(crate) fn run_with_native_embedding_lease_lifecycle<Bootstrap, Output>(
    params: NativeEmbeddingLeaseLifecycleParams<'_>,
    mut bootstrap: impl FnMut(
        &codestory_retrieval::SidecarRuntimeConfig,
        bool,
        Option<&codestory_retrieval::EmbeddingLaunchMetadata>,
        &mut dyn FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<()>,
    ) -> Result<Bootstrap>,
    bootstrap_state: impl FnOnce(&Bootstrap) -> &codestory_retrieval::SidecarStateFile,
    finalize_before_handoff: impl FnOnce(
        Bootstrap,
        &codestory_retrieval::SidecarRuntimeConfig,
    ) -> Result<Output>,
) -> Result<Output> {
    let NativeEmbeddingLeaseLifecycleParams {
        scope,
        sidecar,
        resource,
        wait,
        poll,
        bootstrap_context,
        sidecar_cleanup_label,
    } = params;
    let mut embedding_resource_lease =
        acquire_native_embedding_resource_lease_if_needed_with_validator(
            scope,
            sidecar,
            resource,
            wait,
            poll,
            codestory_retrieval::ensure_native_embedding_launch_identity,
        )?;
    select_native_embedding_endpoint_for_lease(&embedding_resource_lease, sidecar)?;
    let allow_native_embedding_spawn = !matches!(
        embedding_resource_lease,
        Some(BrokerNativeEmbeddingResourceLease::Reused { .. })
    );
    let reused_launch = match embedding_resource_lease.as_ref() {
        Some(BrokerNativeEmbeddingResourceLease::Reused { launch, .. }) => {
            Some(launch.as_ref().clone())
        }
        _ => None,
    };
    let mut bind_retries = 0;
    let bootstrap = loop {
        let state_before_bootstrap = read_optional_file(&sidecar.layout.state_file)?;
        let mut observe_new_native_launch =
            |launch: &codestory_retrieval::EmbeddingLaunchMetadata| {
                let quarantine_reason = if launch.pid.is_none() {
                    "spawn_pending"
                } else {
                    "bootstrap_pending"
                };
                quarantine_native_embedding_resource_lease_for_launch(
                    &mut embedding_resource_lease,
                    launch,
                    quarantine_reason,
                )
            };
        match bootstrap(
            sidecar,
            allow_native_embedding_spawn,
            reused_launch.as_ref(),
            &mut observe_new_native_launch,
        ) {
            Ok(report) => break report,
            Err(error) => {
                let cleanup_published_state = optional_file_changed(
                    &sidecar.layout.state_file,
                    state_before_bootstrap.as_deref(),
                )?;
                let retry_bind = bind_retries < MAX_NATIVE_EMBEDDING_BIND_RETRIES
                    && matches!(
                        embedding_resource_lease,
                        Some(BrokerNativeEmbeddingResourceLease::Acquired(_))
                    )
                    && codestory_retrieval::native_embedding_startup_cleanup_failure(&error)
                        .is_none()
                    && native_embedding_port_bind_failed(&error);
                let cleanup_result = if retry_bind {
                    cleanup_native_embedding_resource_lease_before_bind_retry(
                        &mut embedding_resource_lease,
                        sidecar,
                        cleanup_published_state,
                    )
                } else {
                    cleanup_native_embedding_resource_lease_after_bootstrap_failure(
                        &mut embedding_resource_lease,
                        sidecar,
                        &error,
                        cleanup_published_state,
                    )
                };
                if let Err(cleanup_error) = cleanup_result {
                    return Err(error).context(format!(
                        "{bootstrap_context}; native embedding cleanup failed: {cleanup_error}"
                    ));
                }
                if retry_bind {
                    sidecar
                        .rotate_broker_native_embedding_port()
                        .with_context(|| {
                            format!(
                                "rotate native embedding port after bind race attempt {} failed: {error:#}",
                                bind_retries + 1
                            )
                        })?;
                    bind_retries += 1;
                    continue;
                }
                return Err(error).context(bootstrap_context.to_string());
            }
        }
    };
    let final_state = bootstrap_state(&bootstrap).clone();
    if let Err(error) = quarantine_native_embedding_resource_lease_before_finalization(
        &mut embedding_resource_lease,
        &final_state,
    ) {
        cleanup_native_embedding_resource_lease_after_transfer_error(
            &mut embedding_resource_lease,
            sidecar,
        )
        .with_context(|| {
            format!(
                "cleanup {sidecar_cleanup_label} after native embedding pre-handoff publication failed: {error}"
            )
        })?;
        return Err(error).context("native embedding pre-handoff quarantine publication");
    }
    let output = match finalize_before_handoff(bootstrap, sidecar) {
        Ok(output) => output,
        Err(error) => {
            cleanup_native_embedding_resource_lease_after_transfer_error(
                &mut embedding_resource_lease,
                sidecar,
            )
            .with_context(|| {
                format!("cleanup {sidecar_cleanup_label} after finalization failure: {error}")
            })?;
            return Err(error);
        }
    };
    // Publish the native PID only after every fallible readiness/finalization step. Until this
    // handoff, another runtime cannot attach to a process that this owner may still unwind.
    if let Err(error) =
        transfer_native_embedding_resource_lease(&mut embedding_resource_lease, &final_state)
    {
        cleanup_native_embedding_resource_lease_after_transfer_error(
            &mut embedding_resource_lease,
            sidecar,
        )
        .with_context(|| {
            format!(
                "cleanup {sidecar_cleanup_label} after native embedding lease transfer failed: {error}"
            )
        })?;
        return Err(error).context("native embedding lease transfer");
    }
    Ok(output)
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn optional_file_changed(path: &Path, before: Option<&[u8]>) -> Result<bool> {
    Ok(read_optional_file(path)?.as_deref() != before)
}

fn native_embedding_port_bind_failed(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .to_string()
            .trim_start()
            .strip_prefix(codestory_retrieval::NATIVE_EMBEDDING_PORT_BIND_FAILED_REASON)
            .is_some_and(|rest| rest.starts_with(':'))
    })
}

pub(super) fn select_native_embedding_endpoint_for_lease(
    lease: &Option<BrokerNativeEmbeddingResourceLease>,
    sidecar: &mut codestory_retrieval::SidecarRuntimeConfig,
) -> Result<()> {
    match lease {
        Some(BrokerNativeEmbeddingResourceLease::Acquired(_)) => {
            // A persisted native state may retain the previous process endpoint so status can
            // describe it. Once the broker grants a new machine lease, that endpoint is no
            // longer authoritative. Revalidate the provisional allocation under the registry
            // lock, then launch on that port instead.
            sidecar.revalidate_broker_native_embedding_port()
        }
        Some(BrokerNativeEmbeddingResourceLease::Reused { launch, .. }) => {
            if !adopt_reused_native_launch_endpoint(sidecar, launch)? {
                bail!("verified reusable native embedding launch has an unsupported endpoint");
            }
            Ok(())
        }
        None => Ok(()),
    }
}

pub(super) fn acquire_native_embedding_resource_lease_if_needed_with_validator(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    resource: &str,
    wait: Duration,
    poll: Duration,
    mut validate_launch: impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<BrokerNativeEmbeddingResourceLease>> {
    if codestory_retrieval::embedding_server_launch_mode_for_runtime(sidecar)?
        != codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned
    {
        return Ok(None);
    }
    let deadline = Instant::now() + wait;
    loop {
        match try_acquire_native_embedding_machine_resource_lock(resource, scope)? {
            BrokerMachineResourceLockAttempt::Acquired(lock) => {
                return Ok(Some(BrokerNativeEmbeddingResourceLease::Acquired(lock)));
            }
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                if cleanup_quarantined_native_embedding_resource(&busy)? {
                    continue;
                }
                let Some(handoff_guard) = try_acquire_machine_resource_reaper_lock(resource)?
                else {
                    if Instant::now() >= deadline {
                        return bail_native_embedding_busy(&busy);
                    }
                    std::thread::sleep(
                        poll.min(deadline.saturating_duration_since(Instant::now())),
                    );
                    continue;
                };
                let busy = BrokerMachineResourceBusy {
                    snapshot: machine_resource_snapshot(resource),
                };
                if let Some(launch) = reusable_native_embedding_resource_launch(
                    scope,
                    sidecar,
                    &busy,
                    &mut validate_launch,
                )? {
                    let pid = launch
                        .pid
                        .context("reusable native embedding launch missing pid")?;
                    return Ok(Some(BrokerNativeEmbeddingResourceLease::Reused {
                        pid,
                        launch: Box::new(launch),
                        _handoff_guard: handoff_guard,
                    }));
                }
                if Instant::now() >= deadline {
                    return bail_native_embedding_busy(&busy);
                }
                std::thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
            }
        }
    }
}

pub(crate) fn sidecar_down_with_native_embedding_handoff(
    cache_root: &Path,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<()> {
    let Some(state) = read_sidecar_state_file(sidecar)? else {
        return codestory_retrieval::sidecar_down_for_runtime(sidecar);
    };
    let Some(launch) = state.embedding_launch.as_ref() else {
        return codestory_retrieval::sidecar_down_for_runtime(sidecar);
    };
    let guard = try_acquire_machine_resource_reaper_lock(NATIVE_EMBEDDING_RESOURCE)?
        .context("CodeStory repository search setup is finishing; retry shortly")?;
    if state.owns_embedding_launch() {
        let attachments = codestory_retrieval::attached_native_embedding_state_paths(
            cache_root,
            &sidecar.layout.state_file,
            launch,
        )?;
        if !attachments.is_empty() {
            bail!("CodeStory repository search is still in use by another project");
        }
    }
    codestory_retrieval::sidecar_down_for_runtime(sidecar)?;
    if state.owns_embedding_launch() {
        if !release_machine_resource_lock_for_native_launch_with_guard(
            NATIVE_EMBEDDING_RESOURCE,
            launch,
            &guard,
        )? {
            bail!("CodeStory repository search ownership changed during shutdown; retry shortly");
        }
    }
    Ok(())
}

fn cleanup_quarantined_native_embedding_resource(busy: &BrokerMachineResourceBusy) -> Result<bool> {
    let lock_path = machine_resource_lock_path(&busy.snapshot.resource);
    if super::paths::clean_path(&lock_path) != busy.snapshot.lock_path {
        return Ok(false);
    }
    let Some(lock) = read_machine_resource_lock_file(&lock_path) else {
        return Ok(false);
    };
    if lock.resource != busy.snapshot.resource
        || busy.snapshot.owner_pid != Some(lock.pid)
        || lock.native_embedding_quarantine_reason.is_none()
    {
        return Ok(false);
    }
    if machine_lock_owner_state(&lock)
        != crate::ready_repair_status::ProcessOwnerState::GoneOrReused
    {
        return Ok(false);
    }
    let launch = lock
        .native_embedding_launch
        .as_ref()
        .context("quarantined native embedding lock is missing exact launch metadata")?;
    if launch.pid.is_none() {
        if cfg!(target_os = "macos")
            && launch.spawn_protocol.as_deref()
                == Some(codestory_retrieval::NATIVE_EMBEDDING_DARWIN_EXEC_GATE_PROTOCOL)
        {
            let released = release_quarantined_machine_resource_lock_for_native_launch(
                &lock.resource,
                &lock.token,
                launch,
            )?;
            return Ok(released || !lock_path.exists());
        }
        bail!(
            "quarantined native embedding launch is still pending pid publication without a recoverable exec-gate protocol; refusing automatic cleanup"
        );
    }
    codestory_retrieval::stop_native_embedding_process_for_launch(launch)
        .context("retry exact cleanup for quarantined native embedding launch")?;
    let released = release_quarantined_machine_resource_lock_for_native_launch(
        &lock.resource,
        &lock.token,
        launch,
    )?;
    Ok(released || !lock_path.exists())
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
    Ok(
        reusable_native_embedding_resource_launch(scope, sidecar, &busy, &mut validate_launch)?
            .and_then(|launch| launch.pid),
    )
}

pub(crate) fn native_embedding_owner_down_command(
    snapshot: &BrokerResourceSnapshot,
) -> Option<String> {
    let lock_path = machine_resource_lock_path(&snapshot.resource);
    if super::paths::clean_path(&lock_path) != snapshot.lock_path {
        return None;
    }
    let lock = read_machine_resource_lock_file(&lock_path)?;
    if lock.resource != snapshot.resource
        || Some(lock.pid) != snapshot.owner_pid
        || Some(lock.scope.project_id.as_str()) != snapshot.owner_project_id.as_deref()
        || Some(lock.scope.workspace_root.as_str()) != snapshot.owner_workspace_root.as_deref()
        || lock.native_embedding_launch.is_none()
        || lock.native_embedding_quarantine_reason.is_some()
    {
        return None;
    }
    let project = crate::display::quote_command_argument_value(&lock.scope.workspace_root);
    match lock.scope.profile.as_str() {
        "local" => Some(format!(
            "codestory-cli retrieval down --project {project} --profile local"
        )),
        "agent" => {
            let run_id = lock.scope.run_id.as_deref()?;
            Some(format!(
                "codestory-cli retrieval down --project {project} --profile agent --run-id {}",
                crate::display::quote_command_argument_value(run_id)
            ))
        }
        _ => None,
    }
}

fn reusable_native_embedding_resource_launch(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    busy: &BrokerMachineResourceBusy,
    validate_launch: &mut impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
) -> Result<Option<codestory_retrieval::EmbeddingLaunchMetadata>> {
    let Some(mut launch) = reusable_native_embedding_resource_launch_with_matcher(
        scope,
        sidecar,
        busy,
        validate_launch,
        |owner_scope, requested_runtime, launch| {
            reused_launch_matches_owner_and_requested_runtime(
                owner_scope,
                requested_runtime,
                launch,
            )
        },
    )?
    else {
        return Ok(None);
    };
    if launch.log_path.is_none() {
        let lock_path = machine_resource_lock_path(&busy.snapshot.resource);
        let Some(lock) = read_machine_resource_lock_file(&lock_path) else {
            return Ok(None);
        };
        let owner_profile = match lock.scope.profile.as_str() {
            "local" => codestory_retrieval::SidecarProfile::Local,
            "agent" => codestory_retrieval::SidecarProfile::Agent,
            _ => return Ok(None),
        };
        let owner_root = Path::new(&lock.scope.workspace_root);
        let owner_runtime = sidecar.with_profile_and_run_id(
            Some(owner_root),
            owner_profile,
            lock.scope.run_id.as_deref(),
        );
        let Some(expected_owner) = codestory_retrieval::expected_native_embedding_launch_metadata(
            owner_root,
            &owner_runtime,
        )?
        else {
            return Ok(None);
        };
        if !enrich_legacy_native_launch_log_path(&expected_owner, &mut launch) {
            return Ok(None);
        }
    }
    Ok(Some(launch))
}

pub(crate) fn reusable_native_embedding_resource_launch_with_matcher(
    scope: &BrokerScope,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    busy: &BrokerMachineResourceBusy,
    validate_launch: &mut impl FnMut(&codestory_retrieval::EmbeddingLaunchMetadata) -> Result<u32>,
    mut matches_runtime: impl FnMut(
        &BrokerScope,
        &codestory_retrieval::SidecarRuntimeConfig,
        &codestory_retrieval::EmbeddingLaunchMetadata,
    ) -> Result<bool>,
) -> Result<Option<codestory_retrieval::EmbeddingLaunchMetadata>> {
    if busy.snapshot.status != "busy" {
        return Ok(None);
    }
    let Some(owner_pid) = busy.snapshot.owner_pid else {
        return Ok(None);
    };
    let Some(scope_identity) = super::scope::effective_scope_identity(scope) else {
        return Ok(None);
    };
    if sidecar.project_identity.as_ref() != Some(&scope_identity) {
        return Ok(None);
    }
    let Some(owner_workspace_root) = busy.snapshot.owner_workspace_root.as_deref() else {
        return Ok(None);
    };
    let Some(owner_identity) = super::scope::identity_from_workspace_root(owner_workspace_root)
    else {
        return Ok(None);
    };
    let lock_path = machine_resource_lock_path(&busy.snapshot.resource);
    if super::paths::clean_path(&lock_path) != busy.snapshot.lock_path {
        return Ok(None);
    }
    let Some(lock) = read_machine_resource_lock_file(&lock_path) else {
        return Ok(None);
    };
    if lock.native_embedding_quarantine_reason.is_some() {
        return Ok(None);
    }
    let Some(lock_identity) = super::scope::effective_scope_identity(&lock.scope) else {
        return Ok(None);
    };
    if lock.resource != busy.snapshot.resource
        || lock.pid != owner_pid
        || busy.snapshot.owner_project_id.as_deref() != Some(lock.scope.project_id.as_str())
        || lock_identity.workspace_id != owner_identity.workspace_id
        || !codestory_workspace::same_workspace_path(
            Path::new(&lock.scope.workspace_root),
            Path::new(owner_workspace_root),
        )
    {
        return Ok(None);
    }
    let Some(launch) = lock.native_embedding_launch.as_ref() else {
        return Ok(None);
    };
    if launch.launch_mode != codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned.as_str()
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
    let mut retargeted = sidecar.clone();
    if !retarget_runtime_to_reused_native_launch(&mut retargeted, launch)?
        || !matches_runtime(&lock.scope, &retargeted, launch)?
    {
        return Ok(None);
    }
    Ok(Some(launch.clone()))
}

fn retarget_runtime_to_reused_native_launch(
    sidecar: &mut codestory_retrieval::SidecarRuntimeConfig,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
) -> Result<bool> {
    let Some(port) = reused_native_embedding_endpoint_port(launch) else {
        return Ok(false);
    };
    sidecar.embedding.endpoint = codestory_retrieval::SidecarLayout::embed_base_url(port);
    Ok(true)
}

fn reused_native_embedding_endpoint_port(
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
) -> Option<u16> {
    let port = launch
        .endpoint
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix("/v1/embeddings"))
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)?;
    if launch.endpoint != codestory_retrieval::SidecarLayout::embed_base_url(port) {
        return None;
    }
    Some(port)
}

fn adopt_reused_native_launch_endpoint(
    sidecar: &mut codestory_retrieval::SidecarRuntimeConfig,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
) -> Result<bool> {
    let Some(port) = reused_native_embedding_endpoint_port(launch) else {
        return Ok(false);
    };
    sidecar.use_broker_verified_native_embedding_endpoint(port)?;
    Ok(true)
}

pub(super) fn reused_launch_matches_owner_and_requested_runtime(
    owner_scope: &BrokerScope,
    requested_runtime: &codestory_retrieval::SidecarRuntimeConfig,
    launch: &codestory_retrieval::EmbeddingLaunchMetadata,
) -> Result<bool> {
    if !matches!(owner_scope.profile.as_str(), "local" | "agent") {
        return Ok(false);
    }
    codestory_retrieval::native_embedding_launch_matches_runtime_for_reuse(
        requested_runtime,
        launch,
    )
}

pub(super) fn same_native_launch_configuration(
    expected: &codestory_retrieval::EmbeddingLaunchMetadata,
    actual: &codestory_retrieval::EmbeddingLaunchMetadata,
    require_log_path: bool,
) -> bool {
    expected.provider == actual.provider
        && expected.launch_mode == actual.launch_mode
        && expected.endpoint == actual.endpoint
        && expected.launch_args == actual.launch_args
        && expected.launch_fingerprint_sha256 == actual.launch_fingerprint_sha256
        && expected.executable_path == actual.executable_path
        && expected.model_path == actual.model_path
        && actual
            .model_sha256
            .as_ref()
            .is_none_or(|digest| expected.model_sha256.as_ref() == Some(digest))
        && expected.requested_device == actual.requested_device
        && (!require_log_path
            || actual.log_path.is_none()
            || expected.log_path.as_deref().is_some_and(|expected| {
                actual.log_path.as_deref().is_some_and(|actual| {
                    codestory_workspace::same_workspace_path(Path::new(expected), Path::new(actual))
                })
            }))
}

pub(super) fn enrich_legacy_native_launch_log_path(
    expected: &codestory_retrieval::EmbeddingLaunchMetadata,
    actual: &mut codestory_retrieval::EmbeddingLaunchMetadata,
) -> bool {
    if !same_native_launch_configuration(expected, actual, true) {
        return false;
    }
    if actual.log_path.is_none() {
        actual.log_path.clone_from(&expected.log_path);
    }
    actual.log_path.is_some()
}

fn bail_native_embedding_busy<T>(busy: &BrokerMachineResourceBusy) -> Result<T> {
    let owner = busy
        .snapshot
        .owner_workspace_root
        .as_deref()
        .unwrap_or("unknown");
    let next = native_embedding_owner_down_command(&busy.snapshot)
        .map(|command| format!("the full owner runtime has an incompatible native launch contract; stop it with `{command}`, then retry"))
        .unwrap_or_else(|| {
            "the owner is still active after the bounded wait; retry after its current repair completes"
                .to_string()
        });
    bail!(
        "native embedding runtime is busy for another CodeStory operation: resource={} owner_project={} owner_workspace={} owner_pid={:?}; {next}",
        busy.snapshot.resource,
        busy.snapshot
            .owner_project_id
            .as_deref()
            .unwrap_or("unknown"),
        owner,
        busy.snapshot.owner_pid,
    );
}
