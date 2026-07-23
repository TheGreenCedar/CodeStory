use serde::{Deserialize, Serialize};
use specta::Type;

use super::dto::{FileCoverageDiagnosticDto, ReadinessVerdictDto};

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<ApiErrorDetails>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ApiErrorDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause_code: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_capacity: Option<EmbeddingCapacityPressureDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_retry: Option<EmbeddingRetryStateDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coverage_gaps: Vec<FileCoverageDiagnosticDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct EmbeddingCapacityPressureDto {
    pub reason: String,
    pub queue_class: String,
    pub capacity: u64,
    pub depth: u64,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    pub owner_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_request_class: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct EmbeddingRetryStateDto {
    pub code: String,
    pub retry_class: String,
    pub retry_after_ms: u64,
    pub retry_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<EmbeddingCapacityPressureDto>,
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
    pub fn cause(cause_code: impl Into<String>) -> Self {
        Self {
            cause_code: Some(cause_code.into()),
            failed_layer: None,
            project: None,
            next_commands: Vec::new(),
            minimum_next: Vec::new(),
            full_repair: Vec::new(),
            readiness: None,
            embedding_capacity: None,
            embedding_retry: None,
            coverage_gaps: Vec::new(),
        }
    }

    pub fn retrieval_unavailable(project: impl Into<String>, next_commands: Vec<String>) -> Self {
        let minimum_next = next_commands.iter().take(1).cloned().collect::<Vec<_>>();
        Self {
            cause_code: None,
            failed_layer: Some("retrieval_engine".to_string()),
            project: Some(project.into()),
            minimum_next,
            full_repair: next_commands.clone(),
            next_commands,
            readiness: None,
            embedding_capacity: None,
            embedding_retry: None,
            coverage_gaps: Vec::new(),
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

    pub fn source_coverage(coverage_gaps: Vec<FileCoverageDiagnosticDto>) -> Self {
        Self {
            cause_code: None,
            failed_layer: Some("source_verification".to_string()),
            project: None,
            next_commands: Vec::new(),
            minimum_next: Vec::new(),
            full_repair: Vec::new(),
            readiness: None,
            embedding_capacity: None,
            embedding_retry: None,
            coverage_gaps,
        }
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

    pub fn source_coverage_failure(
        code: impl Into<String>,
        message: impl Into<String>,
        coverage_gaps: Vec<FileCoverageDiagnosticDto>,
    ) -> Self {
        Self::with_details(
            code,
            message,
            ApiErrorDetails::source_coverage(coverage_gaps),
        )
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

    pub fn embedding_capacity(
        message: impl Into<String>,
        pressure: EmbeddingCapacityPressureDto,
    ) -> Self {
        Self::with_details(
            "embedding_capacity",
            message,
            ApiErrorDetails {
                cause_code: None,
                failed_layer: Some("embedding_admission".into()),
                project: None,
                next_commands: Vec::new(),
                minimum_next: Vec::new(),
                full_repair: Vec::new(),
                readiness: None,
                embedding_capacity: Some(pressure.clone()),
                embedding_retry: Some(EmbeddingRetryStateDto {
                    code: "embedding_capacity".into(),
                    retry_class: "after_capacity_change".into(),
                    retry_after_ms: pressure.retry_after_ms,
                    retry_condition: pressure.retry_condition.clone(),
                    capacity: Some(pressure),
                }),
                coverage_gaps: Vec::new(),
            },
        )
    }

    pub fn embedding_retry(
        code: impl Into<String>,
        message: impl Into<String>,
        retry: EmbeddingRetryStateDto,
    ) -> Self {
        Self::with_details(
            code,
            message,
            ApiErrorDetails {
                cause_code: None,
                failed_layer: Some("embedding_runtime".into()),
                project: None,
                next_commands: Vec::new(),
                minimum_next: Vec::new(),
                full_repair: Vec::new(),
                readiness: None,
                embedding_capacity: retry.capacity.clone(),
                embedding_retry: Some(retry),
                coverage_gaps: Vec::new(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_unavailable_error_serializes_recovery_details() {
        let error = ApiError::retrieval_unavailable(
            "retrieval is unavailable or degraded",
            "C:/repo/example",
            vec![
                "codestory-cli retrieval index --profile agent --refresh auto --project \"C:/repo/example\" --format json"
                    .to_string(),
                "codestory-cli retrieval status --project \"C:/repo/example\" --format json"
                    .to_string(),
                "codestory-cli doctor --project \"C:/repo/example\" --format markdown".to_string(),
            ],
        );

        let value = serde_json::to_value(error).expect("serialize api error");

        assert_eq!(value["code"], "retrieval_unavailable");
        assert_eq!(value["details"]["failed_layer"], "retrieval_engine");
        assert_eq!(value["details"]["project"], "C:/repo/example");
        assert_eq!(
            value["details"]["next_commands"][0],
            "codestory-cli retrieval index --profile agent --refresh auto --project \"C:/repo/example\" --format json"
        );
        assert_eq!(
            value["details"]["minimum_next"][0],
            "codestory-cli retrieval index --profile agent --refresh auto --project \"C:/repo/example\" --format json"
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

    #[test]
    fn embedding_capacity_is_typed_and_has_no_repair_commands() {
        let error = ApiError::embedding_capacity(
            "embedding query capacity is unavailable",
            EmbeddingCapacityPressureDto {
                reason: "queue_full".into(),
                queue_class: "query".into(),
                capacity: 64,
                depth: 64,
                retry_after_ms: 25,
                retry_condition: "a query slot becomes available".into(),
                owner_state: "ready".into(),
                active_scope_id: Some("opaque-scope".into()),
                active_request_id: Some("opaque-request".into()),
                active_request_class: Some("bulk".into()),
            },
        );

        let value = serde_json::to_value(error).expect("serialize capacity error");
        assert_eq!(value["code"], "embedding_capacity");
        assert_eq!(
            value["details"]["embedding_capacity"]["retry_condition"],
            "a query slot becomes available"
        );
        assert_eq!(
            value["details"]["embedding_retry"]["retry_class"],
            "after_capacity_change"
        );
        assert!(value["details"].get("project").is_none());
        assert!(value["details"].get("next_commands").is_none());
        assert!(value["details"].get("minimum_next").is_none());
        assert!(value["details"].get("full_repair").is_none());
    }

    #[test]
    fn generic_embedding_retry_is_typed_without_repair_commands() {
        let error = ApiError::embedding_retry(
            "embedding_retryable",
            "the active owner must become idle",
            EmbeddingRetryStateDto {
                code: "embedding_server_incompatible_active_owner".into(),
                retry_class: "after_owner_idle".into(),
                retry_after_ms: 0,
                retry_condition: "the incompatible server exits while fully idle".into(),
                capacity: None,
            },
        );

        let value = serde_json::to_value(error).expect("serialize retry error");
        assert_eq!(value["code"], "embedding_retryable");
        assert_eq!(
            value["details"]["embedding_retry"]["retry_condition"],
            "the incompatible server exits while fully idle"
        );
        assert!(value["details"].get("embedding_capacity").is_none());
        assert!(value["details"].get("next_commands").is_none());
    }
}
