//! Local expression evidence from a retained, revalidated workspace buffer.
//! This is neither indexed proof nor an interpretation of external callees.
use serde::{Deserialize, Serialize};

pub const MAX_RESULT_BYTES: usize = 16_384;
pub const MAX_SOURCE_BYTES: usize = 262_144;
pub const MAX_READ_BYTES: usize = 2_097_152;
pub const MAX_WORK: usize = 8_000_000;
pub const MAX_DEPTH: usize = 256;
pub const MAX_INPUT_BYTES: usize = 262_144;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceExpressionRequest {
    pub project: String,
    pub path: String,
    pub line: usize,
    pub expression: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStatus {
    Available,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisDecline {
    InvalidRequest,
    UnsupportedPlatform,
    UnsupportedSource,
    InvalidProject,
    InvalidPath,
    UnreadableSource,
    InvalidUtf8,
    SourceDrift,
    FileTooLarge,
    ReadBudgetExceeded,
    WorkBudgetExceeded,
    DepthExceeded,
    Cancelled,
    InvalidLine,
    MissingExpression,
    AmbiguousExpression,
    InvalidCriterion,
    MalformedSource,
    ResultBudgetExceeded,
    RuntimeUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisRole {
    Criterion,
    Binding,
    Definition,
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisBoundaryKind {
    ParameterInput,
    UnresolvedBinding,
    OpaqueCall,
    PossibleOpaqueMutation,
    PossibleClosureMutation,
    UnsupportedConstruct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisGap {
    InvalidCriterion,
    UnsupportedConstruct,
    LoopCarriedState,
    UnknownMutation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisRange {
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSnapshot {
    pub project: String,
    pub path: String,
    pub source_sha256: String,
    pub source_kind: String,
    pub revalidated: bool,
    pub indexed_proof: bool,
    pub atomic_publication: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisSegment {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub body_sha256: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisWitness {
    pub role: AnalysisRole,
    pub range: AnalysisRange,
    pub segment: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisBoundary {
    pub kind: AnalysisBoundaryKind,
    pub range: AnalysisRange,
    pub segment: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisLimits {
    /// Complete serialized MCP CallToolResult; outer JSON-RPC is separate.
    pub max_result_bytes: usize,
    pub max_source_bytes: usize,
    pub max_read_bytes: usize,
    pub max_work: usize,
    pub max_depth: usize,
    pub max_input_bytes: usize,
}

impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_result_bytes: MAX_RESULT_BYTES,
            max_source_bytes: MAX_SOURCE_BYTES,
            max_read_bytes: MAX_READ_BYTES,
            max_work: MAX_WORK,
            max_depth: MAX_DEPTH,
            max_input_bytes: MAX_INPUT_BYTES,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceExpressionResponse {
    pub status: AnalysisStatus,
    pub complete_local: bool,
    pub limits: AnalysisLimits,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decline: Option<AnalysisDecline>,
    /// A bounded lower bound, not an unbounded ambiguity-location list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matching_candidates_at_least: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<AnalysisSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criterion: Option<AnalysisRange>,
    /// Bodies occur only here. Role and uncertainty records reference ranges.
    pub segments: Vec<AnalysisSegment>,
    pub witnesses: Vec<AnalysisWitness>,
    pub boundaries: Vec<AnalysisBoundary>,
    pub gaps: Vec<AnalysisGap>,
    pub source_is_untrusted_data: bool,
}

impl SourceExpressionResponse {
    pub fn unavailable(reason: AnalysisDecline) -> Self {
        Self {
            status: AnalysisStatus::Unavailable,
            complete_local: false,
            limits: AnalysisLimits::default(),
            decline: Some(reason),
            matching_candidates_at_least: None,
            snapshot: None,
            criterion: None,
            segments: Vec::new(),
            witnesses: Vec::new(),
            boundaries: Vec::new(),
            gaps: Vec::new(),
            source_is_untrusted_data: true,
        }
    }
}

/// Canonical schema shared by the real catalog and its cold launcher projection.
pub fn result_schema() -> serde_json::Value {
    use serde_json::{Value, json};
    fn object(properties: Value, required: &[&str]) -> Value {
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
    }
    fn array(items: Value) -> Value {
        json!({"type":"array","items":items})
    }
    let integer = json!({"type":"integer","minimum":0});
    let string = json!({"type":"string"});
    let boolean = json!({"type":"boolean"});
    let range = object(
        json!({"start_byte":integer,"end_byte":integer}),
        &["start_byte", "end_byte"],
    );
    let segment = object(
        json!({"start_byte":integer,"end_byte":integer,"start_line":integer,
        "end_line":integer,"body_sha256":string,"source":string}),
        &[
            "start_byte",
            "end_byte",
            "start_line",
            "end_line",
            "body_sha256",
            "source",
        ],
    );
    let witness = object(
        json!({"role":{"type":"string","enum":["criterion","binding","definition","control"]},
        "range":range,"segment":integer}),
        &["role", "range", "segment"],
    );
    let boundary = object(
        json!({"kind":{"type":"string","enum":["parameter_input","unresolved_binding",
        "opaque_call","possible_opaque_mutation","possible_closure_mutation","unsupported_construct"]},
        "range":range,"segment":integer}),
        &["kind", "range", "segment"],
    );
    let limits = object(
        json!({"max_result_bytes":integer,"max_source_bytes":integer,"max_read_bytes":integer,
        "max_work":integer,"max_depth":integer,"max_input_bytes":integer}),
        &[
            "max_result_bytes",
            "max_source_bytes",
            "max_read_bytes",
            "max_work",
            "max_depth",
            "max_input_bytes",
        ],
    );
    let snapshot = object(
        json!({"project":string,"path":string,"source_sha256":string,"source_kind":string,
        "revalidated":boolean,"indexed_proof":boolean,"atomic_publication":boolean}),
        &[
            "project",
            "path",
            "source_sha256",
            "source_kind",
            "revalidated",
            "indexed_proof",
            "atomic_publication",
        ],
    );
    let result = object(
        json!({"status":{"type":"string","enum":["available","partial","unavailable"]},
        "complete_local":boolean,"limits":limits,
        "decline":{"type":"string","enum":["invalid_request","unsupported_platform","unsupported_source",
            "invalid_project","invalid_path","unreadable_source","invalid_utf8","source_drift","file_too_large",
            "read_budget_exceeded","work_budget_exceeded","depth_exceeded","cancelled","invalid_line",
            "missing_expression","ambiguous_expression","invalid_criterion","malformed_source",
            "result_budget_exceeded","runtime_unavailable"]},
        "matching_candidates_at_least":integer,"snapshot":snapshot,"criterion":range,
        "segments":array(segment),"witnesses":array(witness),"boundaries":array(boundary),
        "gaps":array(json!({"type":"string","enum":["invalid_criterion","unsupported_construct","loop_carried_state","unknown_mutation"]})),
        "source_is_untrusted_data":boolean}),
        &[
            "status",
            "complete_local",
            "limits",
            "segments",
            "witnesses",
            "boundaries",
            "gaps",
            "source_is_untrusted_data",
        ],
    );
    object(json!({"result":result}), &["result"])
}
