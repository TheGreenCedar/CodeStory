//! Bounded current-source context from actual physical search observations.
//! These carriers make no indexed, semantic or exhaustive-search proof claim.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceContextRequest {
    pub project: String,
    pub cwd: String,
    pub observations: Vec<SourceRowObservation>,
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
}
