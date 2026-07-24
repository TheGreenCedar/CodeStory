//! Server-side nonce gate, pinned control files, and durable qualification events.

use super::super::{
    EmbeddingRequestClass, EmbeddingServerSnapshot, EmbeddingServerTransport,
    PerUserEmbeddingServerState, SERVER_QUALIFICATION_MAX_COMMAND_BYTES,
    SERVER_QUALIFICATION_MAX_EVENT_BYTES, SERVER_QUALIFICATION_MAX_EVENT_RECORDS, hex_sha256,
};
use anyhow::{Context, Result, anyhow, bail};
use codestory_workspace::{
    WorkspacePathIdentity, workspace_file_identity, workspace_path_identity,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub(in crate::per_user_embedding) struct PinnedQualificationDirectory {
    pub(in crate::per_user_embedding) path: PathBuf,
    pub(super) identity: NativeFileIdentity,
    #[cfg(unix)]
    pub(super) handle: File,
}

type NativeFileIdentity = WorkspacePathIdentity;

#[derive(Debug)]
pub(in crate::per_user_embedding) struct ServerQualificationEventLog {
    pub(in crate::per_user_embedding) path: PathBuf,
    pub(in crate::per_user_embedding) file: File,
    pub(in crate::per_user_embedding) identity: NativeFileIdentity,
    pub(in crate::per_user_embedding) bytes: u64,
    pub(in crate::per_user_embedding) records: u64,
    pub(in crate::per_user_embedding) last_sequence: u64,
}

#[derive(Debug)]
pub(in crate::per_user_embedding) struct ServerQualificationCommandFile {
    pub(in crate::per_user_embedding) bytes: Vec<u8>,
}

#[derive(Debug)]
pub(in crate::per_user_embedding) struct ServerQualificationControl {
    pub(in crate::per_user_embedding) directory: PinnedQualificationDirectory,
    pub(in crate::per_user_embedding) events: Mutex<ServerQualificationEventLog>,
    pub(in crate::per_user_embedding) nonce: String,
    pub(in crate::per_user_embedding) nonce_sha256: String,
    pub(in crate::per_user_embedding) last_sequence: AtomicU64,
    pub(in crate::per_user_embedding) processed_command_sha256: Mutex<Option<String>>,
    pub(in crate::per_user_embedding) force_incompatible: AtomicBool,
    pub(in crate::per_user_embedding) freeze_owner: AtomicBool,
}

impl ServerQualificationControl {
    pub(in crate::per_user_embedding) fn command_was_processed(
        &self,
        command_sha256: &str,
    ) -> bool {
        self.processed_command_sha256
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_deref()
            == Some(command_sha256)
    }

    pub(in crate::per_user_embedding) fn mark_command_processed(&self, command_sha256: String) {
        *self
            .processed_command_sha256
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(command_sha256);
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServerQualificationCommand {
    pub(super) schema_version: u32,
    pub(super) sequence: u64,
    pub(super) nonce_sha256: String,
    pub(super) action: String,
    pub(super) parameters: ServerQualificationCommandParameters,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServerQualificationCommandParameters {
    #[serde(default)]
    pub(super) class: Option<String>,
}

#[derive(Debug, Serialize)]
pub(in crate::per_user_embedding) struct ServerQualificationEventClock {
    pub(in crate::per_user_embedding) domain: String,
    pub(in crate::per_user_embedding) api: String,
    pub(in crate::per_user_embedding) boot_id: String,
    pub(in crate::per_user_embedding) observed_ns: u64,
}

#[derive(Debug, Serialize)]
pub(in crate::per_user_embedding) struct ServerQualificationEvent {
    pub(in crate::per_user_embedding) schema_version: u32,
    pub(in crate::per_user_embedding) sequence: u64,
    pub(in crate::per_user_embedding) action: String,
    pub(in crate::per_user_embedding) status: String,
    pub(in crate::per_user_embedding) server_event_sequence: u64,
    pub(in crate::per_user_embedding) clock: ServerQualificationEventClock,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::per_user_embedding) snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::per_user_embedding) details: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ExistingServerQualificationEvent {
    pub(super) schema_version: u32,
    pub(super) sequence: u64,
}

pub(in crate::per_user_embedding) fn server_qualification_control_from_env()
-> Result<Option<ServerQualificationControl>> {
    server_qualification_control_from_values(
        std::env::var_os("CODESTORY_EMBED_QUALIFICATION_DIR"),
        std::env::var("CODESTORY_EMBED_QUALIFICATION_NONCE").ok(),
    )
}

pub(in crate::per_user_embedding) fn server_qualification_control_from_values(
    directory: Option<std::ffi::OsString>,
    nonce: Option<String>,
) -> Result<Option<ServerQualificationControl>> {
    match (directory, nonce) {
        (None, None) => Ok(None),
        (Some(directory), Some(nonce))
            if !directory.is_empty()
                && !nonce.is_empty()
                && nonce.len() <= 128
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) =>
        {
            let directory = PinnedQualificationDirectory::open(&PathBuf::from(directory))?;
            let events = ServerQualificationEventLog::open(&directory, &nonce)?;
            let last_sequence = events.last_sequence;
            Ok(Some(ServerQualificationControl {
                directory,
                events: Mutex::new(events),
                nonce_sha256: hex_sha256(nonce.as_bytes()),
                nonce,
                last_sequence: AtomicU64::new(last_sequence),
                processed_command_sha256: Mutex::new(None),
                force_incompatible: AtomicBool::new(false),
                freeze_owner: AtomicBool::new(false),
            }))
        }
        _ => bail!("embedding_qualification_gate_incomplete"),
    }
}

impl PinnedQualificationDirectory {
    pub(super) fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            bail!("embedding_qualification_directory_not_absolute");
        }
        let source = fs::symlink_metadata(path).with_context(|| {
            format!(
                "inspect embedding qualification directory {}",
                path.display()
            )
        })?;
        validate_private_qualification_directory_metadata(&source)?;
        let canonical = fs::canonicalize(path).with_context(|| {
            format!(
                "canonicalize embedding qualification directory {}",
                path.display()
            )
        })?;
        #[cfg(not(windows))]
        if canonical != path {
            bail!("embedding_qualification_directory_untrusted");
        }
        let metadata = fs::symlink_metadata(&canonical)
            .context("reinspect canonical embedding qualification directory")?;
        validate_private_qualification_directory_metadata(&metadata)?;
        let identity = native_path_identity(&canonical)?;
        #[cfg(windows)]
        {
            let caller = fs::symlink_metadata(path)
                .context("reinspect caller embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&caller)?;
            if native_path_identity(path)? != identity {
                bail!("embedding_qualification_directory_untrusted");
            }
        }
        #[cfg(unix)]
        let handle = {
            use std::os::unix::fs::OpenOptionsExt;
            let handle = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&canonical)
                .context("pin embedding qualification directory")?;
            let opened = handle
                .metadata()
                .context("inspect pinned embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&opened)?;
            if native_file_identity(&handle)? != identity {
                bail!("embedding_qualification_directory_replaced");
            }
            handle
        };
        let pinned = Self {
            path: canonical,
            identity,
            #[cfg(unix)]
            handle,
        };
        pinned.revalidate()?;
        Ok(pinned)
    }

    pub(in crate::per_user_embedding) fn revalidate(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.path)
            .context("revalidate embedding qualification directory")?;
        validate_private_qualification_directory_metadata(&metadata)?;
        if native_path_identity(&self.path)? != self.identity {
            bail!("embedding_qualification_directory_replaced");
        }
        #[cfg(unix)]
        {
            let opened = self
                .handle
                .metadata()
                .context("revalidate pinned embedding qualification directory")?;
            validate_private_qualification_directory_metadata(&opened)?;
            if native_file_identity(&self.handle)? != self.identity {
                bail!("embedding_qualification_directory_replaced");
            }
        }
        Ok(())
    }

    pub(in crate::per_user_embedding) fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }
}

impl ServerQualificationEventLog {
    pub(super) fn open(directory: &PinnedQualificationDirectory, nonce: &str) -> Result<Self> {
        directory.revalidate()?;
        let path = directory.join(format!("{nonce}.events.jsonl"));
        let mut create = OpenOptions::new();
        create.read(true).append(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            create.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = match create.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&path)
                    .context("inspect existing embedding qualification event log")?;
                validate_private_qualification_file_metadata(
                    &metadata,
                    SERVER_QUALIFICATION_MAX_EVENT_BYTES,
                )?;
                let mut existing = OpenOptions::new();
                existing.read(true).append(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    existing.custom_flags(libc::O_NOFOLLOW);
                }
                existing
                    .open(&path)
                    .context("open existing embedding qualification event log")?
            }
            Err(error) => {
                return Err(error).context("create private embedding qualification event log");
            }
        };
        directory.revalidate()?;
        let metadata = file
            .metadata()
            .context("inspect opened embedding qualification event log")?;
        validate_private_qualification_file_metadata(
            &metadata,
            SERVER_QUALIFICATION_MAX_EVENT_BYTES,
        )?;
        let identity = native_file_identity(&file)?;
        let path_metadata =
            fs::symlink_metadata(&path).context("reinspect embedding qualification event log")?;
        validate_private_qualification_file_metadata(
            &path_metadata,
            SERVER_QUALIFICATION_MAX_EVENT_BYTES,
        )?;
        if native_path_identity(&path)? != identity {
            bail!("embedding_qualification_event_log_replaced");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.try_clone()
            .context("clone embedding qualification event log")?
            .take(SERVER_QUALIFICATION_MAX_EVENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("read existing embedding qualification event log")?;
        if bytes.len() as u64 > SERVER_QUALIFICATION_MAX_EVENT_BYTES
            || (!bytes.is_empty() && !bytes.ends_with(b"\n"))
        {
            bail!("embedding_qualification_event_log_untrusted");
        }
        let mut records = 0_u64;
        let mut last_sequence = 0_u64;
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let event = serde_json::from_slice::<ExistingServerQualificationEvent>(line)
                .context("parse existing embedding qualification event")?;
            if event.schema_version != 1 {
                bail!("embedding_qualification_event_log_untrusted");
            }
            records = records.saturating_add(1);
            last_sequence = last_sequence.max(event.sequence);
        }
        if records > SERVER_QUALIFICATION_MAX_EVENT_RECORDS {
            bail!("embedding_qualification_event_log_record_limit");
        }
        Ok(Self {
            path,
            file,
            identity,
            bytes: bytes.len() as u64,
            records,
            last_sequence,
        })
    }

    pub(in crate::per_user_embedding) fn record(
        &mut self,
        directory: &PinnedQualificationDirectory,
        event: &ServerQualificationEvent,
    ) -> Result<()> {
        directory.revalidate()?;
        let path_metadata = fs::symlink_metadata(&self.path)
            .context("revalidate embedding qualification event log path")?;
        validate_private_qualification_file_metadata(
            &path_metadata,
            SERVER_QUALIFICATION_MAX_EVENT_BYTES,
        )?;
        let opened = self
            .file
            .metadata()
            .context("revalidate opened embedding qualification event log")?;
        if native_path_identity(&self.path)? != self.identity
            || native_file_identity(&self.file)? != self.identity
            || opened.len() != self.bytes
        {
            bail!("embedding_qualification_event_log_replaced");
        }
        let mut encoded =
            serde_json::to_vec(event).context("encode embedding qualification event")?;
        encoded.push(b'\n');
        if self.records >= SERVER_QUALIFICATION_MAX_EVENT_RECORDS
            || self.bytes.saturating_add(encoded.len() as u64)
                > SERVER_QUALIFICATION_MAX_EVENT_BYTES
        {
            bail!("embedding_qualification_event_log_limit");
        }
        self.file
            .write_all(&encoded)
            .context("append embedding qualification event")?;
        self.file
            .flush()
            .context("flush embedding qualification event")?;
        self.file
            .sync_all()
            .context("sync embedding qualification event")?;
        let next_bytes = self.bytes + encoded.len() as u64;
        directory.revalidate()?;
        let path_metadata = fs::symlink_metadata(&self.path)
            .context("reinspect embedding qualification event log after append")?;
        let opened = self
            .file
            .metadata()
            .context("reinspect opened embedding qualification event log after append")?;
        if native_path_identity(&self.path)? != self.identity
            || native_file_identity(&self.file)? != self.identity
            || path_metadata.len() != next_bytes
            || opened.len() != next_bytes
        {
            bail!("embedding_qualification_event_log_replaced");
        }
        self.bytes = next_bytes;
        self.records += 1;
        self.last_sequence = self.last_sequence.max(event.sequence);
        Ok(())
    }
}

pub(super) fn validate_private_qualification_directory_metadata(
    metadata: &fs::Metadata,
) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("embedding_qualification_directory_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("embedding_qualification_directory_untrusted");
        }
    }
    Ok(())
}

pub(in crate::per_user_embedding) fn validate_private_qualification_file_metadata(
    metadata: &fs::Metadata,
    maximum_bytes: u64,
) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum_bytes {
        bail!("embedding_qualification_file_untrusted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            bail!("embedding_qualification_file_untrusted");
        }
    }
    Ok(())
}

pub(super) fn native_path_identity(path: &Path) -> Result<NativeFileIdentity> {
    workspace_path_identity(path)
        .context("embedding qualification filesystem path identity is unavailable")
}

pub(super) fn native_file_identity(file: &File) -> Result<NativeFileIdentity> {
    workspace_file_identity(file)
        .context("embedding qualification filesystem file identity is unavailable")
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

pub(in crate::per_user_embedding) fn qualification_hold_class(
    class: Option<&str>,
    hold: bool,
) -> Result<()> {
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

pub(in crate::per_user_embedding) fn write_server_qualification_event(
    control: &ServerQualificationControl,
    _state: &PerUserEmbeddingServerState,
    event: ServerQualificationEvent,
) -> Result<()> {
    control
        .events
        .lock()
        .map_err(|_| anyhow!("embedding_qualification_event_log_poisoned"))?
        .record(&control.directory, &event)
}

pub(in crate::per_user_embedding) fn opaque_qualification_details(
    error: &anyhow::Error,
) -> std::collections::BTreeMap<String, String> {
    qualification_detail(
        "code",
        error
            .to_string()
            .split(':')
            .next()
            .unwrap_or("embedding_qualification_failed"),
    )
}

pub(in crate::per_user_embedding) fn qualification_detail(
    key: &str,
    value: &str,
) -> std::collections::BTreeMap<String, String> {
    [(key.into(), value.into())].into_iter().collect()
}

#[cfg(unix)]
pub(in crate::per_user_embedding) fn sync_qualification_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .context("sync embedding qualification directory")
}

#[cfg(not(unix))]
pub(in crate::per_user_embedding) fn sync_qualification_directory(_path: &Path) -> Result<()> {
    Ok(())
}
