use super::super::super::{
    EmbeddingRequestClass, EmbeddingServerTransport, PerUserEmbeddingServerState,
    SERVER_QUALIFICATION_MAX_COMMAND_BYTES, hex_sha256,
};
use super::ServerQualificationControl;
use super::event_log::{
    ServerQualificationEvent, ServerQualificationEventClock, opaque_qualification_details,
    qualification_detail, write_server_qualification_event,
};
use super::filesystem::{
    CommandAbsence, Observation, native_file_identity, optional_native_path_identity,
    qualification_file_absence, qualification_file_lost_its_last_name,
    validate_private_qualification_file_metadata,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// How many consecutive ticks may answer `ACCESS_DENIED` before the server
/// stops calling it a removal in progress.
///
/// A Windows delete-pending entry cannot outlive the handles that put it in
/// that state, and this server's own handle closes inside one consume, so the
/// real case clears within a tick or two. A denial that persists is something
/// else -- an ACL that genuinely refuses the open -- and treating that as "no
/// command this tick" forever recreates the unattributable multi-minute hang
/// this whole lane exists to remove. The accept loop polls roughly every 25ms,
/// so this bound is a few seconds: far beyond any real removal race, far short
/// of a proof budget.
pub(in crate::per_user_embedding) const MAX_DENIED_COMMAND_TICKS: u32 = 200;

#[derive(Debug)]
pub(in crate::per_user_embedding) struct ServerQualificationCommandFile {
    pub(in crate::per_user_embedding) bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerQualificationCommand {
    schema_version: u32,
    sequence: u64,
    nonce_sha256: String,
    action: String,
    parameters: ServerQualificationCommandParameters,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerQualificationCommandParameters {
    #[serde(default)]
    class: Option<String>,
}

/// Observe the command file's own metadata, reporting why nothing is there.
fn optional_command_metadata(
    path: &Path,
    context_message: &'static str,
) -> Result<Observation<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if qualification_file_lost_its_last_name(&metadata) => {
            Ok(Err(CommandAbsence::Removed))
        }
        Ok(metadata) => Ok(Ok(metadata)),
        Err(error) => match qualification_file_absence(&error) {
            Some(absence) => Ok(Err(absence)),
            None => Err(error).context(context_message),
        },
    }
}

/// Record one tick that found no command, and decide whether the reason is
/// still credible.
///
/// A removal is proof and resets the denial run. A denial is only ever
/// provisional: it is tolerated while it stays inside the bound and named as a
/// failure once it outlives it.
pub(in crate::per_user_embedding) fn absent_command(
    control: &ServerQualificationControl,
    absence: CommandAbsence,
) -> Result<Option<ServerQualificationCommandFile>> {
    match absence {
        CommandAbsence::Removed => {
            control.denied_command_ticks.store(0, Ordering::Release);
            Ok(None)
        }
        CommandAbsence::Denied => {
            let denied = control
                .denied_command_ticks
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1);
            if denied > MAX_DENIED_COMMAND_TICKS {
                bail!("embedding_qualification_command_denied");
            }
            Ok(None)
        }
    }
}

/// Consume the pinned qualification command for one poll tick.
///
/// `Ok(None)` means "no command this tick", and covers both "the writer has not
/// written one yet" and "the writer took its command back while this tick was
/// reading it". The second case is ordinary: the proof harness removes each
/// command once it has seen the matching event, and that removal races every
/// tick that already opened the file. What the loser of that race observes is
/// platform- and filesystem-dependent -- `NotFound`, a zero link count, or, on
/// a Windows volume without POSIX delete semantics, `ACCESS_DENIED` from an
/// entry left delete-pending -- so a poll loop that treats any of them as fatal
/// kills the server during routine command cleanup.
///
/// Every other outcome stays fatal, because tolerance here must not become a
/// blanket "ignore all IO errors" that turns real control-plane failures into
/// silent waits: untrusted metadata, a different file substituted at the pinned
/// path, an oversized command, a replaced pinned directory, and any other
/// filesystem error all still stop the server. The Windows denial is bounded
/// as well as narrow: `ACCESS_DENIED` is indistinguishable from an ACL that
/// simply refuses the open, so it is tolerated only for as long as a real
/// delete-pending entry could last, then fails as
/// `embedding_qualification_command_denied` rather than waiting forever.
///
/// Every step of the sequence can lose that race, and each loses it
/// differently: a `stat` fails or reports a zero link count, the native path
/// identity fails, the open fails, and the opened handle keeps serving a file
/// that no longer has a name. All of them are classified here, and nowhere
/// else -- the shared metadata gate and the accept loop both stay fail-closed.
pub(in crate::per_user_embedding) fn read_server_qualification_command(
    control: &ServerQualificationControl,
) -> Result<Option<ServerQualificationCommandFile>> {
    match consume_server_qualification_command(control)? {
        Ok(command) => {
            control.denied_command_ticks.store(0, Ordering::Release);
            Ok(Some(command))
        }
        Err(absence) => absent_command(control, absence),
    }
}

/// One consume attempt, reporting either the command or why there was none.
fn consume_server_qualification_command(
    control: &ServerQualificationControl,
) -> Result<Observation<ServerQualificationCommandFile>> {
    control.directory.revalidate()?;
    let path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    let path_metadata =
        match optional_command_metadata(&path, "inspect embedding qualification command")? {
            Ok(metadata) => metadata,
            Err(absence) => return Ok(Err(absence)),
        };
    validate_private_qualification_file_metadata(
        &path_metadata,
        SERVER_QUALIFICATION_MAX_COMMAND_BYTES,
    )?;
    let identity = match optional_native_path_identity(&path)? {
        Ok(identity) => identity,
        Err(absence) => return Ok(Err(absence)),
    };
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) => match qualification_file_absence(&error) {
            Some(absence) => return Ok(Err(absence)),
            None => return Err(error).context("open embedding qualification command"),
        },
    };
    let opened = file
        .metadata()
        .context("inspect opened embedding qualification command")?;
    if qualification_file_lost_its_last_name(&opened) {
        return Ok(Err(CommandAbsence::Removed));
    }
    validate_private_qualification_file_metadata(&opened, SERVER_QUALIFICATION_MAX_COMMAND_BYTES)?;
    if native_file_identity(&file)? != identity {
        bail!("embedding_qualification_command_replaced");
    }
    control.directory.revalidate()?;
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.take(SERVER_QUALIFICATION_MAX_COMMAND_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read embedding qualification command")?;
    if bytes.len() as u64 > SERVER_QUALIFICATION_MAX_COMMAND_BYTES {
        bail!("embedding_qualification_command_limit");
    }
    let path_metadata =
        match optional_command_metadata(&path, "reinspect embedding qualification command")? {
            Ok(metadata) => metadata,
            Err(absence) => return Ok(Err(absence)),
        };
    validate_private_qualification_file_metadata(
        &path_metadata,
        SERVER_QUALIFICATION_MAX_COMMAND_BYTES,
    )?;
    let reobserved = match optional_native_path_identity(&path)? {
        Ok(identity) => identity,
        Err(absence) => return Ok(Err(absence)),
    };
    if reobserved != identity {
        // A missing Unix path still yields a lexical identity, so separate "the
        // writer removed it" from "a different file now sits at the pinned
        // path". Only the latter is a substitution the server must refuse.
        if let Err(absence) =
            optional_command_metadata(&path, "reinspect embedding qualification command")?
        {
            return Ok(Err(absence));
        }
        bail!("embedding_qualification_command_replaced");
    }
    control.directory.revalidate()?;
    Ok(Ok(ServerQualificationCommandFile { bytes }))
}

pub(in crate::per_user_embedding) fn poll_server_qualification_command(
    state: &Arc<PerUserEmbeddingServerState>,
    transport: &dyn EmbeddingServerTransport,
) -> Result<()> {
    let Some(control) = state.qualification.as_ref() else {
        return Ok(());
    };
    let Some(command_file) = read_server_qualification_command(control)? else {
        return Ok(());
    };
    let command_sha256 = hex_sha256(&command_file.bytes);
    if control.command_was_processed(&command_sha256) {
        return Ok(());
    }
    let parsed = serde_json::from_slice::<ServerQualificationCommand>(&command_file.bytes);
    if parsed.as_ref().is_ok_and(|command| {
        command.schema_version == 1
            && command.nonce_sha256 == control.nonce_sha256
            && command.sequence <= control.last_sequence.load(Ordering::Acquire)
    }) {
        control.mark_command_processed(command_sha256);
        return Ok(());
    }
    let (sequence, action) = parsed
        .as_ref()
        .map(|command| (command.sequence, command.action.clone()))
        .unwrap_or_else(|_| (0, "invalid".into()));
    let mut status = "completed";
    let mut details = None;
    let mut snapshot = None;
    let mut crash = false;
    match parsed {
        Ok(command)
            if command.schema_version == 1
                && command.nonce_sha256 == control.nonce_sha256
                && command.sequence > control.last_sequence.load(Ordering::Acquire) =>
        {
            let result = match command.action.as_str() {
                "crash_server" => {
                    crash = true;
                    status = "accepted";
                    Ok(())
                }
                "stall_native" => {
                    codestory_llama_sys::set_embedding_qualification_native_stall(true);
                    Ok(())
                }
                "release_native" => {
                    codestory_llama_sys::set_embedding_qualification_native_stall(false);
                    Ok(())
                }
                "hold_class" => qualification_hold_class(command.parameters.class.as_deref(), true),
                "release_class" => {
                    qualification_hold_class(command.parameters.class.as_deref(), false)
                }
                "force_incompatible" => {
                    control.force_incompatible.store(true, Ordering::Release);
                    Ok(())
                }
                "clear_incompatible" => {
                    control.force_incompatible.store(false, Ordering::Release);
                    Ok(())
                }
                "snapshot" => {
                    let current = state.snapshot();
                    details = Some(std::collections::BTreeMap::from([
                        (
                            "idle_epoch_ns".into(),
                            state.last_work_ended_ns.load(Ordering::Acquire).to_string(),
                        ),
                        ("true_idle".into(), state.true_idle().to_string()),
                        ("clock_domain".into(), current.clock.domain.clone()),
                        ("clock_boot_id".into(), current.clock.boot_id.clone()),
                        (
                            "server_instance_id".into(),
                            current.process.server_instance_id.clone(),
                        ),
                    ]));
                    snapshot = Some(current);
                    Ok(())
                }
                "freeze_owner" => {
                    control.freeze_owner.store(true, Ordering::Release);
                    Ok(())
                }
                "release_owner" => {
                    control.freeze_owner.store(false, Ordering::Release);
                    Ok(())
                }
                _ => bail!("embedding_qualification_action_unknown"),
            };
            if let Err(error) = result {
                status = "failed";
                details = Some(opaque_qualification_details(&error));
            }
            control
                .last_sequence
                .store(command.sequence, Ordering::Release);
        }
        Ok(_) => {
            status = "failed";
            details = Some(qualification_detail(
                "code",
                "embedding_qualification_command_rejected",
            ));
        }
        Err(_) => {
            status = "failed";
            details = Some(qualification_detail(
                "code",
                "embedding_qualification_command_invalid",
            ));
        }
    }
    write_server_qualification_event(
        control,
        state,
        ServerQualificationEvent {
            schema_version: 1,
            sequence,
            action,
            status: status.into(),
            server_event_sequence: state.event_sequence.load(Ordering::Acquire),
            clock: {
                let clock = state.clock.snapshot();
                ServerQualificationEventClock {
                    domain: clock.domain,
                    api: clock.api,
                    boot_id: clock.boot_id,
                    observed_ns: state.clock.now_ns(),
                }
            },
            snapshot,
            details,
        },
    )?;
    control.mark_command_processed(command_sha256);
    if crash {
        transport.fail_stop("embedding_qualification_crash");
        state.draining.store(true, Ordering::Release);
    }
    Ok(())
}

fn qualification_hold_class(class: Option<&str>, hold: bool) -> Result<()> {
    match class {
        Some("query") => {
            codestory_llama_sys::set_embedding_qualification_class_hold(
                EmbeddingRequestClass::Query,
                hold,
            );
            Ok(())
        }
        Some("bulk") => {
            codestory_llama_sys::set_embedding_qualification_class_hold(
                EmbeddingRequestClass::Bulk,
                hold,
            );
            Ok(())
        }
        _ => bail!("embedding_qualification_class_invalid"),
    }
}
