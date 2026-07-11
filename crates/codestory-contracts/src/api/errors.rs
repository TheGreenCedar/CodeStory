use serde::{Deserialize, Serialize};
use specta::Type;

use super::dto::ReadinessVerdictDto;

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<ApiErrorDetails>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ApiErrorDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub minimum_next: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub full_repair: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<ReadinessVerdictDto>,
}

pub const COMMAND_FAILURE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandFailureEnvelope {
    pub schema_version: u32,
    pub error: ApiError,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

impl CommandFailureEnvelope {
    pub fn new(error: ApiError) -> Self {
        Self {
            schema_version: COMMAND_FAILURE_SCHEMA_VERSION,
            error,
            context: None,
        }
    }

    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }
}

impl ApiErrorDetails {
    pub fn retrieval_unavailable(project: impl Into<String>, next_commands: Vec<String>) -> Self {
        let minimum_next = next_commands.iter().take(1).cloned().collect::<Vec<_>>();
        Self {
            failed_layer: Some("retrieval_sidecar".to_string()),
            project: Some(project.into()),
            minimum_next,
            full_repair: next_commands.clone(),
            next_commands,
            readiness: None,
        }
    }

    pub fn with_readiness(mut self, readiness: ReadinessVerdictDto) -> Self {
        if self.minimum_next.is_empty() {
            self.minimum_next = readiness.minimum_next.clone();
        }
        if self.full_repair.is_empty() {
            self.full_repair = readiness.full_repair.clone();
        }
        if self.next_commands.is_empty() {
            self.next_commands = self.full_repair.clone();
        }
        self.readiness = Some(readiness);
        self
    }
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        details: ApiErrorDetails,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Some(Box::new(details)),
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new("invalid_argument", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }

    pub fn retrieval_unavailable(
        message: impl Into<String>,
        project: impl Into<String>,
        next_commands: Vec<String>,
    ) -> Self {
        Self::with_details(
            "retrieval_unavailable",
            message,
            ApiErrorDetails::retrieval_unavailable(project, next_commands),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_unavailable_error_serializes_repair_details() {
        let error = ApiError::retrieval_unavailable(
            "sidecar retrieval primary is unavailable or degraded",
            "C:/repo/example",
            vec![
                "codestory-cli ready --goal agent --repair --project \"C:/repo/example\" --format json"
                    .to_string(),
                "codestory-cli retrieval status --project \"C:/repo/example\" --format json"
                    .to_string(),
                "codestory-cli doctor --project \"C:/repo/example\" --format markdown".to_string(),
            ],
        );

        let value = serde_json::to_value(error).expect("serialize api error");

        assert_eq!(value["code"], "retrieval_unavailable");
        assert_eq!(value["details"]["failed_layer"], "retrieval_sidecar");
        assert_eq!(value["details"]["project"], "C:/repo/example");
        assert_eq!(
            value["details"]["next_commands"][0],
            "codestory-cli ready --goal agent --repair --project \"C:/repo/example\" --format json"
        );
        assert_eq!(
            value["details"]["minimum_next"][0],
            "codestory-cli ready --goal agent --repair --project \"C:/repo/example\" --format json"
        );
        assert_eq!(
            value["details"]["full_repair"][1],
            "codestory-cli retrieval status --project \"C:/repo/example\" --format json"
        );
    }

    #[test]
    fn command_failure_envelope_round_trips_shared_api_error() {
        let envelope = CommandFailureEnvelope::new(ApiError::invalid_argument("bad input"))
            .with_context(serde_json::json!({"argument": "--format"}));

        let json = serde_json::to_string(&envelope).expect("serialize envelope");
        let decoded: CommandFailureEnvelope =
            serde_json::from_str(&json).expect("deserialize envelope");

        assert_eq!(decoded, envelope);
        assert_eq!(decoded.schema_version, COMMAND_FAILURE_SCHEMA_VERSION);
    }
}
