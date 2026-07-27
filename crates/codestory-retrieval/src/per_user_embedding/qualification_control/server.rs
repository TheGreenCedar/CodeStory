//! Server-side nonce gate, pinned control files, and durable qualification events.

use configuration::PinnedQualificationDirectory;
use event_log::ServerQualificationEventLog;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

mod command_io;
mod configuration;
mod event_log;
mod filesystem;

pub(in crate::per_user_embedding) use command_io::poll_server_qualification_command;
#[cfg(test)]
pub(in crate::per_user_embedding) use command_io::{
    MAX_DENIED_COMMAND_TICKS, absent_command, read_server_qualification_command,
};
pub(in crate::per_user_embedding) use configuration::server_qualification_control_from_env;
#[cfg(test)]
pub(in crate::per_user_embedding) use configuration::server_qualification_control_from_values;
pub(in crate::per_user_embedding) use event_log::{
    ServerQualificationEvent, ServerQualificationEventClock, write_server_qualification_event,
};
#[cfg(test)]
pub(in crate::per_user_embedding) use filesystem::CommandAbsence;
#[cfg(all(test, windows))]
pub(in crate::per_user_embedding) use filesystem::native_path_identity;
pub(in crate::per_user_embedding) use filesystem::{
    sync_qualification_directory, validate_private_qualification_file_metadata,
};

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
    /// Consecutive poll ticks that answered `ACCESS_DENIED` at the pinned
    /// command path. Bounds how long a Windows delete-pending entry may be
    /// assumed, so a permanent denial fails by name instead of hanging.
    pub(in crate::per_user_embedding) denied_command_ticks: AtomicU32,
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
