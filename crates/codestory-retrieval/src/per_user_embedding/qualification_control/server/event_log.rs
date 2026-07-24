use super::super::super::{
    EmbeddingServerSnapshot, PerUserEmbeddingServerState, SERVER_QUALIFICATION_MAX_EVENT_BYTES,
    SERVER_QUALIFICATION_MAX_EVENT_RECORDS,
};
use super::ServerQualificationControl;
use super::configuration::PinnedQualificationDirectory;
use super::filesystem::{
    NativeFileIdentity, native_file_identity, native_path_identity,
    validate_private_qualification_file_metadata,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;

#[derive(Debug)]
pub(in crate::per_user_embedding) struct ServerQualificationEventLog {
    pub(in crate::per_user_embedding) path: PathBuf,
    pub(in crate::per_user_embedding) file: File,
    pub(in crate::per_user_embedding) identity: NativeFileIdentity,
    pub(in crate::per_user_embedding) bytes: u64,
    pub(in crate::per_user_embedding) records: u64,
    pub(in crate::per_user_embedding) last_sequence: u64,
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
    pub(in crate::per_user_embedding) details: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ExistingServerQualificationEvent {
    schema_version: u32,
    sequence: u64,
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

pub(super) fn opaque_qualification_details(error: &anyhow::Error) -> BTreeMap<String, String> {
    qualification_detail(
        "code",
        error
            .to_string()
            .split(':')
            .next()
            .unwrap_or("embedding_qualification_failed"),
    )
}

pub(super) fn qualification_detail(key: &str, value: &str) -> BTreeMap<String, String> {
    [(key.into(), value.into())].into_iter().collect()
}
