//! Bounded current-source context from actual physical search observations.
//! These carriers make no indexed, semantic or exhaustive-search proof claim.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceContextRequest {
    pub project: String,
    pub cwd: String,
    pub observations: Vec<SourceRowObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<SourceContextQuery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceContextQuery {
    /// One actual case-sensitive rg pattern; unsupported grammar uses the
    /// ordinary context path. This is literal query coverage, not task policy.
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceContextBundle {
    /// Indices into this response's units. Headers remain explicitly partial;
    /// the renderer admits this primary and its unrendered headers atomically.
    pub primary_unit_index: usize,
    pub required_header_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceContextSelection {
    /// Sampled original zero-based observation indices, retaining raw order.
    /// Per-observation declines still identify unsupported/refused samples.
    pub observation_indices: Vec<usize>,
    pub bundles: Vec<SourceContextBundle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRowObservation {
    pub path: String,
    pub physical_line: usize,
    /// Exact physical row with only its terminal LF removed; retain any CR.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContextBounds {
    pub hits: usize,
    pub files: usize,
    pub per_file_bytes: usize,
    pub aggregate_read_bytes: usize,
    pub display_source_bytes: usize,
    pub display_metadata_bytes: usize,
    pub max_depth: usize,
    pub work_units: usize,
}

pub const SOURCE_CONTEXT_BOUNDS: SourceContextBounds = SourceContextBounds {
    hits: 64,
    files: 16,
    per_file_bytes: 262144,
    aggregate_read_bytes: 2097152,
    display_source_bytes: 12288,
    display_metadata_bytes: 2048,
    max_depth: 256,
    work_units: 8000000,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceContextStatus {
    Unavailable,
    RawFallback,
    BoundedCurrentSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContextUnit {
    pub path: String,
    pub source_sha256: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub kind: String,
    pub body: String,
    pub body_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceContextDeclineReason {
    Unavailable,
    UnsupportedPlatform,
    InvalidProject,
    InvalidCwd,
    InvalidPath,
    UnsupportedSource,
    UnreadableSource,
    InvalidUtf8,
    FileTooLarge,
    MalformedSource,
    ObservedRowMismatch,
    SourceDrift,
    HitLimit,
    FileLimit,
    ReadBudgetExceeded,
    WorkBudgetExceeded,
    DepthExceeded,
    UnitTooLarge,
    InvalidMatchLine,
    NoEnclosingUnit,
    InlineAncestorHeader,
    AncestorHeaderTooLarge,
    AncestorDepthExceeded,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContextDecline {
    /// Original zero-based observation index; None denotes request-wide refusal.
    pub observation_index: Option<usize>,
    pub reason: SourceContextDeclineReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContextCoverage {
    pub observations_received: usize,
    pub observations_considered: usize,
    pub files_read: usize,
    pub files_compiled: usize,
    pub source_read_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContextResponse {
    pub status: SourceContextStatus,
    pub units: Vec<SourceContextUnit>,
    pub declines: Vec<SourceContextDecline>,
    pub coverage: SourceContextCoverage,
    pub bounds: SourceContextBounds,
    /// Current files rechecked after extraction; not an atomic publication.
    pub current_post_search_snapshot: bool,
    pub exhaustive_search: bool,
    pub atomic_publication: bool,
    pub indexed_proof: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SourceContextSelection>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_context_old_json_and_absent_metadata_keep_their_shape() {
        let old = serde_json::json!({"project":"/project","cwd":"/project","observations":[]});
        let request: SourceContextRequest = serde_json::from_value(old.clone()).unwrap();
        assert!(request.query.is_none());
        assert_eq!(serde_json::to_value(request).unwrap(), old);
        assert!(serde_json::from_value::<SourceContextRequest>(serde_json::json!({"project":"/project","cwd":"/project","observations":[],"unknown":true})).is_err());
        let response = serde_json::json!({"status":"raw_fallback","units":[],"declines":[],
            "coverage":{"observations_received":0,"observations_considered":0,"files_read":0,"files_compiled":0,"source_read_bytes":0},
            "bounds":SOURCE_CONTEXT_BOUNDS,"current_post_search_snapshot":false,
            "exhaustive_search":false,"atomic_publication":false,"indexed_proof":false});
        let parsed: SourceContextResponse = serde_json::from_value(response.clone()).unwrap();
        assert!(parsed.selection.is_none());
        assert_eq!(serde_json::to_value(parsed).unwrap(), response);
    }
}
