//! Listener lifecycle, handler reaping, and idle shutdown.

use super::super::admission::ServerRequestAdmission;
use super::super::qualification_control::{
    poll_server_qualification_command, server_qualification_control_from_env,
};
use super::super::scheduler::spawn_server_watchdog;
use super::super::{
    EmbeddingServerAuthoritySnapshot, EmbeddingServerBindOutcome, EmbeddingServerListener,
    EmbeddingServerProcessSnapshot, PER_USER_EMBEDDING_BOOTSTRAP_VERSION,
    PER_USER_EMBEDDING_CONSTANT_SET_SHA256, PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256,
    PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION, PER_USER_EMBEDDING_PROTOCOL_SHA256,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS, SERVER_ACCEPT_POLL, elapsed_since,
};
use super::PerUserEmbeddingServerConfig;
use super::connection::{
    serve_embedding_connection, serve_embedding_connection_at_handler_capacity,
};
use super::state::PerUserEmbeddingServerState;
use crate::embedding_contract::native_engine_config;
use anyhow::{Context, Result, bail};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

pub fn run_per_user_embedding_server(config: PerUserEmbeddingServerConfig) -> Result<()> {
    validate_server_config(&config)?;
    let listener = match config.transport.bind().map_err(anyhow::Error::new)? {
        EmbeddingServerBindOutcome::Bound(listener) => {
            Arc::<dyn EmbeddingServerListener>::from(listener)
        }
        EmbeddingServerBindOutcome::AlreadyOwned => return Ok(()),
    };
    let authority = listener.identity().clone();
    if !authority.peer_verified {
        bail!("embedding_server_listener_peer_proof_missing");
    }
    let clock = config.transport.clock();
    let server_instance_id = Uuid::new_v4().to_string();
    let state = Arc::new(PerUserEmbeddingServerState {
        clock: Arc::clone(&clock),
        engine_cache_root: config.engine_cache_root,
        engine_config: native_engine_config(config.allow_cpu)?,
        engine: Mutex::new(None),
        process: EmbeddingServerProcessSnapshot {
            server_instance_id,
            pid: config.executable.pid,
            process_start_id: config.executable.process_start_id,
            executable_sha256: config.executable.executable_sha256,
            executable_version: config.executable.executable_version,
        },
        protocol: config.protocol,
        authority: EmbeddingServerAuthoritySnapshot {
            endpoint_namespace_id: authority.endpoint_namespace_id,
            lifetime_authority_id: authority.lifetime_authority_id,
            listener_id: authority.listener_id,
            peer_verified: authority.peer_verified,
        },
        connections: AtomicUsize::new(0),
        pre_request_connections: AtomicUsize::new(0),
        admission_gate: Mutex::new(()),
        request_admission: Arc::new(ServerRequestAdmission::default()),
        active: Mutex::new(std::collections::BTreeMap::new()),
        cancellations: Mutex::new(std::collections::BTreeMap::new()),
        draining: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        last_work_ended_ns: AtomicU64::new(clock.now_ns()),
        event_sequence: AtomicU64::new(1),
        last_failure: Mutex::new(None),
        qualification: server_qualification_control_from_env()?,
    });

    let watchdog = spawn_server_watchdog(
        Arc::clone(&state),
        Arc::clone(&config.transport),
        config.budgets,
    )?;
    let mut connections = Vec::new();
    let serve_result = (|| -> Result<()> {
        loop {
            poll_server_qualification_command(&state, config.transport.as_ref())?;
            if state
                .qualification
                .as_ref()
                .is_some_and(|control| control.freeze_owner.load(Ordering::Acquire))
            {
                clock.sleep(SERVER_ACCEPT_POLL);
                continue;
            }
            if state.draining.load(Ordering::Acquire) {
                break;
            }
            if state.true_idle()
                && elapsed_since(
                    clock.as_ref(),
                    state.last_work_ended_ns.load(Ordering::Acquire),
                ) >= config.budgets.idle_timeout
                && state.begin_draining_if_idle()
            {
                break;
            }
            match listener.accept(SERVER_ACCEPT_POLL) {
                Ok(Some(stream)) => {
                    if let Some(connection_guard) = state.try_begin_connection() {
                        let state_for_connection = Arc::clone(&state);
                        connections.push(
                            thread::Builder::new()
                                .name("codestory-embedding-connection".into())
                                .spawn(move || {
                                    let _guard = connection_guard;
                                    if let Err(error) =
                                        serve_embedding_connection(state_for_connection, stream)
                                    {
                                        tracing::debug!(
                                            error = %error,
                                            "embedding connection closed"
                                        );
                                    }
                                })
                                .context("spawn embedding connection handler")?,
                        );
                    } else if let Some(rejection_guard) = state.try_begin_rejection_connection() {
                        let state_for_rejection = Arc::clone(&state);
                        connections.push(
                            thread::Builder::new()
                                .name("codestory-embedding-capacity-rejection".into())
                                .spawn(move || {
                                    let _guard = rejection_guard;
                                    if let Err(error) =
                                        serve_embedding_connection_at_handler_capacity(
                                            state_for_rejection,
                                            stream,
                                        )
                                    {
                                        tracing::debug!(
                                            error = %error,
                                            "embedding capacity rejection closed"
                                        );
                                    }
                                })
                                .context("spawn embedding capacity rejection handler")?,
                        );
                    } else {
                        // Total live handlers remain hard bounded even when
                        // hostile partial handshakes occupy the rejection
                        // reserve.
                        let _ = stream.shutdown();
                    }
                }
                Ok(None) => {}
                Err(_error) if state.draining.load(Ordering::Acquire) => break,
                Err(error) => return Err(anyhow::Error::new(error)),
            }
            reap_finished_connection_handlers(&mut connections);
        }
        Ok(())
    })();

    state.draining.store(true, Ordering::Release);
    let _ = listener.close();
    let state_for_cleanup = Arc::clone(&state);
    let cleanup = thread::Builder::new()
        .name("codestory-embedding-cleanup".into())
        .spawn(move || {
            state_for_cleanup.shutdown_engine();
            state_for_cleanup.stopped.store(true, Ordering::Release);
        })
        .context("spawn embedding server cleanup")?;
    let _ = watchdog.join();
    if cleanup.is_finished() {
        let _ = cleanup.join();
    }
    serve_result
}

pub(in crate::per_user_embedding) fn reap_finished_connection_handlers(
    connections: &mut Vec<thread::JoinHandle<()>>,
) {
    connections.retain(|connection| !connection.is_finished());
}

fn validate_server_config(config: &PerUserEmbeddingServerConfig) -> Result<()> {
    if config.budgets.idle_timeout
        != Duration::from_millis(PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS)
    {
        bail!("embedding_server_idle_timeout_contract_mismatch");
    }
    if config.budgets.native_no_progress.is_zero()
        || config.budgets.watchdog_poll.is_zero()
        || config.protocol.bootstrap_version != PER_USER_EMBEDDING_BOOTSTRAP_VERSION
        || config.protocol.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || config.protocol.protocol_sha256 != PER_USER_EMBEDDING_PROTOCOL_SHA256
        || config.protocol.constant_set_sha256 != PER_USER_EMBEDDING_CONSTANT_SET_SHA256
        || config.protocol.measurement_protocol_sha256
            != PER_USER_EMBEDDING_MEASUREMENT_PROTOCOL_SHA256
    {
        bail!("embedding_server_constant_contract_mismatch");
    }
    for value in [
        config.executable.process_start_id.as_str(),
        config.executable.executable_sha256.as_str(),
        config.executable.executable_version.as_str(),
    ] {
        if value.trim().is_empty() {
            bail!("embedding_server_process_identity_incomplete");
        }
    }
    Ok(())
}
