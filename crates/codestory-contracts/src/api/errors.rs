use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<ApiErrorDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ApiErrorDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_commands: Vec<String>,
}

impl ApiErrorDetails {
    pub fn retrieval_unavailable(project: impl Into<String>, next_commands: Vec<String>) -> Self {
        Self {
            failed_layer: Some("retrieval_sidecar".to_string()),
            project: Some(project.into()),
            next_commands,
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
            details: Some(details),
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
                "codestory-cli index --project \"C:/repo/example\" --refresh full".to_string(),
                "codestory-cli retrieval bootstrap --project \"C:/repo/example\" --format json"
                    .to_string(),
            ],
        );

        let value = serde_json::to_value(error).expect("serialize api error");

        assert_eq!(value["code"], "retrieval_unavailable");
        assert_eq!(value["details"]["failed_layer"], "retrieval_sidecar");
        assert_eq!(value["details"]["project"], "C:/repo/example");
        assert_eq!(
            value["details"]["next_commands"][0],
            "codestory-cli index --project \"C:/repo/example\" --refresh full"
        );
    }
}
