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
    native_file_identity, native_path_identity, validate_private_qualification_file_metadata,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::Ordering;

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

pub(in crate::per_user_embedding) fn read_server_qualification_command(
    control: &ServerQualificationControl,
) -> Result<Option<ServerQualificationCommandFile>> {
    control.directory.revalidate()?;
    let path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    let path_metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).context("inspect embedding qualification command");
        }
    };
    validate_private_qualification_file_metadata(
        &path_metadata,
        SERVER_QUALIFICATION_MAX_COMMAND_BYTES,
    )?;
    let identity = native_path_identity(&path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(&path)
        .context("open embedding qualification command")?;
    let opened = file
        .metadata()
        .context("inspect opened embedding qualification command")?;
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
        fs::symlink_metadata(&path).context("reinspect embedding qualification command")?;
    validate_private_qualification_file_metadata(
        &path_metadata,
        SERVER_QUALIFICATION_MAX_COMMAND_BYTES,
    )?;
    if native_path_identity(&path)? != identity {
        bail!("embedding_qualification_command_replaced");
    }
    control.directory.revalidate()?;
    Ok(Some(ServerQualificationCommandFile { bytes }))
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
