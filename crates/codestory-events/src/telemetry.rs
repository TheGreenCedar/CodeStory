use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info};
use uuid::Uuid;

const TELEMETRY_TARGET: &str = "codestory::events::telemetry";

/// Contract names from WS-B boundary tasks.
pub const CMD_REFRESH_WORKSPACE: &str = "RefreshWorkspace";
pub const CMD_DELETE_FILE: &str = "DeleteFile";
pub const CMD_ACTIVATE_NODE: &str = "ActivateNode";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandLifecycle {
    Start,
    Success,
    Failure,
}

impl fmt::Display for CommandLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start => write!(f, "command_start"),
            Self::Success => write!(f, "command_success"),
            Self::Failure => write!(f, "command_failure"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandTelemetry {
    pub correlation_id: String,
    pub command: String,
    pub lifecycle: CommandLifecycle,
    pub error_reason: Option<String>,
    pub duration_ms: Option<u128>,
    pub context: Option<String>,
}

impl CommandTelemetry {
    pub fn start(command: impl Into<String>, correlation_id: &str) -> Self {
        Self {
            correlation_id: correlation_id.to_string(),
            command: command.into(),
            lifecycle: CommandLifecycle::Start,
            error_reason: None,
            duration_ms: None,
            context: None,
        }
    }

    pub fn success(
        command: impl Into<String>,
        correlation_id: &str,
        duration_ms: Option<u128>,
    ) -> Self {
        Self {
            correlation_id: correlation_id.to_string(),
            command: command.into(),
            lifecycle: CommandLifecycle::Success,
            error_reason: None,
            duration_ms,
            context: None,
        }
    }

    pub fn failure(
        command: impl Into<String>,
        correlation_id: &str,
        reason: Option<String>,
    ) -> Self {
        Self {
            correlation_id: correlation_id.to_string(),
            command: command.into(),
            lifecycle: CommandLifecycle::Failure,
            error_reason: reason,
            duration_ms: None,
            context: None,
        }
    }

    fn now_unix_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default()
    }
}

pub fn new_correlation_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn command_start(command: &str, correlation_id: &str) -> CommandTelemetry {
    let telemetry = CommandTelemetry::start(command, correlation_id);
    info!(
        target: TELEMETRY_TARGET,
        command = %telemetry.command,
        correlation_id = %telemetry.correlation_id,
        lifecycle = %telemetry.lifecycle,
        timestamp_ms = CommandTelemetry::now_unix_ms(),
        "command_start"
    );
    telemetry
}

pub fn command_success(
    command: &str,
    correlation_id: &str,
    duration_ms: Option<u128>,
) -> CommandTelemetry {
    let telemetry = CommandTelemetry::success(command, correlation_id, duration_ms);
    info!(
        target: TELEMETRY_TARGET,
        command = %telemetry.command,
        correlation_id = %telemetry.correlation_id,
        lifecycle = %telemetry.lifecycle,
        duration_ms = ?telemetry.duration_ms,
        timestamp_ms = CommandTelemetry::now_unix_ms(),
        "command_success"
    );
    telemetry
}

pub fn command_failure(
    command: &str,
    correlation_id: &str,
    reason: Option<String>,
) -> CommandTelemetry {
    let telemetry = CommandTelemetry::failure(command, correlation_id, reason);
    let error_reason = telemetry.error_reason.as_deref().unwrap_or("unclassified");

    error!(
        target: TELEMETRY_TARGET,
        command = %telemetry.command,
        correlation_id = %telemetry.correlation_id,
        lifecycle = %telemetry.lifecycle,
        error = %error_reason,
        timestamp_ms = CommandTelemetry::now_unix_ms(),
        "command_failure"
    );

    telemetry
}

pub fn debug_context(command: &str, correlation_id: &str, context: &str) {
    debug!(
        target: TELEMETRY_TARGET,
        command = %command,
        correlation_id = %correlation_id,
        context = %context,
        timestamp_ms = CommandTelemetry::now_unix_ms(),
        "command_context"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_ids_are_uuid_like() {
        let id = new_correlation_id();
        assert!(!id.is_empty());
        assert_eq!(id.len(), 36);
    }

    #[test]
    fn command_telemetry_lifecycle() {
        let correlation_id = new_correlation_id();
        let start = CommandTelemetry::start("RefreshWorkspace", &correlation_id);
        let success = CommandTelemetry::success("RefreshWorkspace", &correlation_id, Some(123));
        let failure =
            CommandTelemetry::failure("DeleteFile", &correlation_id, Some("boom".to_string()));

        assert_eq!(start.lifecycle, CommandLifecycle::Start);
        assert!(start.duration_ms.is_none());
        assert_eq!(success.lifecycle, CommandLifecycle::Success);
        assert_eq!(success.duration_ms, Some(123));
        assert_eq!(failure.lifecycle, CommandLifecycle::Failure);
        assert_eq!(failure.error_reason, Some("boom".to_string()));
    }
}
