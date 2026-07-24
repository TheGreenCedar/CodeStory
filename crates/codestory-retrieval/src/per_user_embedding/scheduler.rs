//! Request-class scheduler snapshots and fail-stop watchdog progress.

use super::qualification_control::{
    sync_qualification_directory, validate_private_qualification_file_metadata,
};
use super::{
    AwakeMonotonicClock, EmbeddingQualificationWatchdogClock, EmbeddingQualificationWatchdogMarker,
    EmbeddingServerActiveRequestSnapshot, EmbeddingServerBudgets, EmbeddingServerFailureSnapshot,
    EmbeddingServerSchedulerSnapshot, EmbeddingServerTransport, PerUserEmbeddingServerState,
    ServerQualificationControl, duration_ms, elapsed_since,
    embedding_qualification_watchdog_marker_filename,
};
use anyhow::{Context, Result, bail};
use codestory_llama_sys::{EmbeddingAdmissionSnapshot, EmbeddingRequestClass};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(super) struct ActiveServerRequest {
    pub(super) request_id: String,
    pub(super) scope_id: String,
    pub(super) request_class: EmbeddingRequestClass,
    pub(super) phase: String,
    pub(super) started_ns: u64,
}

pub(super) fn spawn_server_watchdog(
    state: Arc<PerUserEmbeddingServerState>,
    transport: Arc<dyn EmbeddingServerTransport>,
    budgets: EmbeddingServerBudgets,
) -> Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("codestory-embedding-watchdog".into())
        .spawn(move || {
            let started_ns = state.clock.now_ns();
            let mut query_progress = WatchdogClassProgress::new(started_ns);
            let mut bulk_progress = WatchdogClassProgress::new(started_ns);
            let mut draining_progress = WatchdogClassProgress::new(started_ns);
            while !state.stopped.load(Ordering::Acquire) {
                state.clock.sleep(budgets.watchdog_poll);
                if state.stopped.load(Ordering::Acquire) {
                    return;
                }
                let draining = state.draining.load(Ordering::Acquire);
                let active_classes = state.active.lock().map_or_else(
                    |_| ActiveRequestClasses::default(),
                    |active| ActiveRequestClasses {
                        query: active
                            .values()
                            .any(|request| request.request_class == EmbeddingRequestClass::Query),
                        bulk: active
                            .values()
                            .any(|request| request.request_class == EmbeddingRequestClass::Bulk),
                    },
                );
                let progress = state
                    .try_initialized_engine()
                    .map(|engine| {
                        let admission = engine.admission_snapshot();
                        WatchdogProgressSnapshot {
                            overall: admission.progress_sequence,
                            query: admission.query_progress_sequence,
                            bulk: admission.bulk_progress_sequence,
                        }
                    })
                    .unwrap_or_default();
                let stalled = query_progress
                    .observe(
                        active_classes.query,
                        progress.query,
                        state.clock.as_ref(),
                        budgets.native_no_progress,
                    )
                    .or_else(|| {
                        bulk_progress.observe(
                            active_classes.bulk,
                            progress.bulk,
                            state.clock.as_ref(),
                            budgets.native_no_progress,
                        )
                    })
                    .or_else(|| {
                        draining_progress.observe(
                            draining && !active_classes.query && !active_classes.bulk,
                            progress.overall,
                            state.clock.as_ref(),
                            budgets.native_no_progress,
                        )
                    });
                if let Some(stalled) = stalled {
                    state.record_failure(EmbeddingServerFailureSnapshot {
                        code: "embedding_engine_stalled".into(),
                        retry_class: "same_rpc_once".into(),
                        retry_after_ms: 0,
                        retry_condition: "the server instance changes".into(),
                    });
                    if let Some(control) = state.qualification.as_ref()
                        && let Err(error) = publish_watchdog_fail_stop_marker(
                            control,
                            &state,
                            budgets,
                            stalled.sequence,
                            stalled.last_progress_ns,
                        )
                    {
                        tracing::error!(
                            error = %error,
                            "failed to publish embedding qualification watchdog marker"
                        );
                    }
                    transport.fail_stop("embedding_engine_stalled");
                    state.draining.store(true, Ordering::Release);
                    state.stopped.store(true, Ordering::Release);
                    return;
                }
            }
        })
        .context("spawn embedding server watchdog")
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ActiveRequestClasses {
    query: bool,
    bulk: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct WatchdogProgressSnapshot {
    overall: u64,
    query: u64,
    bulk: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WatchdogStall {
    pub(super) sequence: u64,
    pub(super) last_progress_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WatchdogClassProgress {
    sequence: u64,
    last_progress_ns: u64,
    was_active: bool,
}

impl WatchdogClassProgress {
    pub(super) fn new(now_ns: u64) -> Self {
        Self {
            sequence: 0,
            last_progress_ns: now_ns,
            was_active: false,
        }
    }

    pub(super) fn observe(
        &mut self,
        active: bool,
        sequence: u64,
        clock: &dyn AwakeMonotonicClock,
        timeout: Duration,
    ) -> Option<WatchdogStall> {
        if !active {
            self.was_active = false;
            self.sequence = sequence;
            self.last_progress_ns = clock.now_ns();
            return None;
        }
        if !self.was_active || sequence != self.sequence {
            self.was_active = true;
            self.sequence = sequence;
            self.last_progress_ns = clock.now_ns();
            return None;
        }
        (elapsed_since(clock, self.last_progress_ns) >= timeout).then_some(WatchdogStall {
            sequence,
            last_progress_ns: self.last_progress_ns,
        })
    }
}

pub(super) fn publish_watchdog_fail_stop_marker(
    control: &ServerQualificationControl,
    state: &PerUserEmbeddingServerState,
    budgets: EmbeddingServerBudgets,
    progress_sequence: u64,
    last_progress_ns: u64,
) -> Result<()> {
    control.directory.revalidate()?;
    let filename = embedding_qualification_watchdog_marker_filename(
        &control.nonce_sha256,
        &state.process.server_instance_id,
    )?;
    let destination = control.directory.join(&filename);
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => bail!("embedding_qualification_watchdog_marker_exists"),
        Err(error) => return Err(error).context("inspect watchdog marker destination"),
    }
    let clock = state.clock.snapshot();
    let marker = EmbeddingQualificationWatchdogMarker {
        schema_version: 1,
        nonce_sha256: control.nonce_sha256.clone(),
        server_instance_id: state.process.server_instance_id.clone(),
        pid: state.process.pid,
        process_start_id: state.process.process_start_id.clone(),
        executable_sha256: state.process.executable_sha256.clone(),
        executable_version: state.process.executable_version.clone(),
        reason: "embedding_engine_stalled".into(),
        clock: EmbeddingQualificationWatchdogClock {
            domain: clock.domain,
            api: clock.api,
            boot_id: clock.boot_id,
            observed_ns: state.clock.now_ns(),
        },
        progress_sequence,
        last_progress_ns,
        hard_native_no_progress_ms: duration_ms(budgets.native_no_progress),
        watchdog_cadence_ms: duration_ms(budgets.watchdog_poll),
    };
    let mut encoded = serde_json::to_vec(&marker).context("encode watchdog fail-stop marker")?;
    encoded.push(b'\n');
    let temporary = control.directory.join(format!(
        ".{filename}.{}.{}.tmp",
        std::process::id(),
        Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&temporary)
        .context("create watchdog fail-stop marker temp file")?;
    let publish = (|| -> Result<()> {
        file.write_all(&encoded)
            .context("write watchdog fail-stop marker")?;
        file.flush().context("flush watchdog fail-stop marker")?;
        file.sync_all().context("sync watchdog fail-stop marker")?;
        drop(file);
        control.directory.revalidate()?;
        fs::rename(&temporary, &destination).context("publish watchdog fail-stop marker")?;
        sync_qualification_directory(&control.directory.path)?;
        let metadata = fs::symlink_metadata(&destination)
            .context("inspect published watchdog fail-stop marker")?;
        validate_private_qualification_file_metadata(&metadata, 64 * 1024)?;
        if metadata.len() != encoded.len() as u64 {
            bail!("embedding_qualification_watchdog_marker_truncated");
        }
        Ok(())
    })();
    if publish.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    publish
}

pub(super) fn scheduler_snapshot(
    admission: &EmbeddingAdmissionSnapshot,
    connections: usize,
    active: Option<&ActiveServerRequest>,
    clock: &dyn AwakeMonotonicClock,
) -> EmbeddingServerSchedulerSnapshot {
    EmbeddingServerSchedulerSnapshot {
        query_capacity: admission.query_capacity as u64,
        query_depth: admission.query_depth as u64,
        bulk_capacity: admission.bulk_capacity as u64,
        bulk_depth: admission.bulk_depth as u64,
        connection_count: connections as u64,
        active_request_count: admission.active_request_count as u64,
        lease_count: admission.lease_count as u64,
        active_request: active.map(|active| active_request_snapshot(active, clock)),
    }
}

pub(super) fn active_request_snapshot(
    active: &ActiveServerRequest,
    clock: &dyn AwakeMonotonicClock,
) -> EmbeddingServerActiveRequestSnapshot {
    EmbeddingServerActiveRequestSnapshot {
        request_id: active.request_id.clone(),
        scope_id: active.scope_id.clone(),
        class: active.request_class.as_str().into(),
        phase: active.phase.clone(),
        elapsed_ms: duration_ms(elapsed_since(clock, active.started_ns)),
    }
}
