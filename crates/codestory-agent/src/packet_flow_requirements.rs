//! Coverage labels and ordinary CALL receipt validation.
//!
//! Prompt→domain stage dispatchers, `FlowRequirement` lists, and formula
//! proof specs were deleted. `FlowRole` and `CoverageMode` remain only as
//! claim-profile contract labels. They do not select evidence from prompt
//! vocabulary.

use codestory_contracts::api::{AgentCitationDto, EdgeKind, GraphEdgeDto, NodeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowRole {
    Entrypoint,
    Registration,
    Configuration,
    StateOrStorage,
    Dispatch,
    TransformOrValidate,
    TerminalBoundary,
    ErrorOrFallback,
}

impl FlowRole {
    #[cfg(any(test, feature = "test-support"))]
    pub const fn role_id(self) -> &'static str {
        match self {
            Self::Entrypoint => "entrypoint",
            Self::Registration => "registration",
            Self::Configuration => "configuration",
            Self::StateOrStorage => "state_or_storage",
            Self::Dispatch => "dispatch",
            Self::TransformOrValidate => "transform_or_validate",
            Self::TerminalBoundary => "terminal_boundary",
            Self::ErrorOrFallback => "error_or_fallback",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Entrypoint => "entrypoint",
            Self::Registration => "registration",
            Self::Configuration => "configuration",
            Self::StateOrStorage => "state/storage",
            Self::Dispatch => "dispatch",
            Self::TransformOrValidate => "transform/validate",
            Self::TerminalBoundary => "terminal boundary",
            Self::ErrorOrFallback => "error/fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageMode {
    RequiresResolvedSourceOrGraph,
    AllowsSourceRange,
    AllowsLexicalSource,
    DiagnosticOnly,
}

/// Validate one cited CALL as a structural receipt (identity + certainty only).
pub fn ordinary_incident_call_receipt_is_valid(
    citation: &AgentCitationDto,
    edge: &GraphEdgeDto,
    neighbor_kind: NodeKind,
) -> bool {
    edge.kind == EdgeKind::CALL
        && (edge.source == citation.node_id || edge.target == citation.node_id)
        && edge.source != edge.target
        && neighbor_kind != NodeKind::UNKNOWN
        && match edge.certainty.as_deref() {
            Some(certainty) => certainty.eq_ignore_ascii_case("certain"),
            None => true,
        }
}
