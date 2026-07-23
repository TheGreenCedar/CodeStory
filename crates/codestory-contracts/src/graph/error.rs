use super::{EnumConversionError, NodeId};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub message: String,
    pub file_id: Option<NodeId>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub is_fatal: bool,
    pub index_step: IndexStep,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_reason: Option<FileCoverageReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FileCoverageReason {
    ParserPartial,
    SourceChanged,
    Unreadable,
    Malformed,
    Binary,
    Oversized,
    DiscoveryIncomplete,
    CollectorFailure,
}

impl FileCoverageReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParserPartial => "parser_partial",
            Self::SourceChanged => "source_changed",
            Self::Unreadable => "unreadable",
            Self::Malformed => "malformed",
            Self::Binary => "binary",
            Self::Oversized => "oversized",
            Self::DiscoveryIncomplete => "discovery_incomplete",
            Self::CollectorFailure => "collector_failure",
        }
    }
}

impl TryFrom<&str> for FileCoverageReason {
    type Error = EnumConversionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "parser_partial" => Ok(Self::ParserPartial),
            "source_changed" => Ok(Self::SourceChanged),
            "unreadable" => Ok(Self::Unreadable),
            "malformed" => Ok(Self::Malformed),
            "binary" => Ok(Self::Binary),
            "oversized" => Ok(Self::Oversized),
            "discovery_incomplete" => Ok(Self::DiscoveryIncomplete),
            "collector_failure" => Ok(Self::CollectorFailure),
            _ => Err(EnumConversionError::InvalidFileCoverageReason(
                value.to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IndexStep {
    Collection,
    Indexing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorFilter {
    /// Show only fatal errors
    pub fatal_only: bool,
    /// Show only errors from the indexing step (vs collection)
    pub indexed_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_reason_uses_stable_snake_case_values() {
        let cases = [
            (FileCoverageReason::ParserPartial, "parser_partial"),
            (FileCoverageReason::SourceChanged, "source_changed"),
            (FileCoverageReason::Unreadable, "unreadable"),
            (FileCoverageReason::Malformed, "malformed"),
            (FileCoverageReason::Binary, "binary"),
            (FileCoverageReason::Oversized, "oversized"),
            (
                FileCoverageReason::DiscoveryIncomplete,
                "discovery_incomplete",
            ),
            (FileCoverageReason::CollectorFailure, "collector_failure"),
        ];

        for (reason, label) in cases {
            assert_eq!(reason.as_str(), label);
            assert_eq!(FileCoverageReason::try_from(label).unwrap(), reason);
            assert_eq!(
                serde_json::to_string(&reason).unwrap(),
                format!("\"{label}\"")
            );
        }
    }

    #[test]
    fn missing_coverage_reason_deserializes_as_none() {
        let error: ErrorInfo = serde_json::from_str(
            r#"{"message":"legacy","file_id":null,"line":null,"column":null,"is_fatal":false,"index_step":"Indexing"}"#,
        )
        .unwrap();

        assert_eq!(error.coverage_reason, None);
    }
}
