//! Obligation coverage labels retained after Phase 3 decontamination.
//!
//! Prompt→domain stage dispatchers and carrier taxonomies were deleted. These
//! types remain only so empty `FlowRequirement` slices and claim-profile
//! coverage labels still type-check. They do not select evidence from prompt
//! vocabulary.

use crate::packet_proof_atoms::FlowProofSpec;
use codestory_contracts::api::{AgentCitationDto, EdgeKind, GraphEdgeDto, NodeKind};

type CallBoundaryNamePredicate = fn(&str) -> bool;

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

/// Decontaminated evidence predicate: never proves from role/carrier taxonomies.
#[derive(Debug, Clone, Copy, Default)]
pub enum EvidencePredicate {
    #[default]
    Never,
}

impl EvidencePredicate {
    pub fn citation_proves(self, _citation: &AgentCitationDto) -> bool {
        false
    }

    pub fn citation_proves_without_call_boundary(self, _citation: &AgentCitationDto) -> bool {
        false
    }

    pub fn call_boundary_target(
        self,
        _citation: &AgentCitationDto,
    ) -> Option<CallBoundaryNamePredicate> {
        None
    }

    pub fn ordered_call_boundary(
        self,
        _citation: &AgentCitationDto,
    ) -> Option<(CallBoundaryNamePredicate, CallBoundaryNamePredicate)> {
        None
    }

    pub fn preferred_node_kinds(self) -> &'static [NodeKind] {
        &[]
    }

    pub fn allowed_node_kinds(self) -> &'static [NodeKind] {
        self.preferred_node_kinds()
    }
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

pub fn flow_requirement_call_boundary_is_discoverable(
    _requirement: &FlowRequirement,
    _citation: &AgentCitationDto,
) -> bool {
    false
}

pub fn flow_requirement_call_receipt_is_valid(
    _requirement: &FlowRequirement,
    citation: &AgentCitationDto,
    edge: &GraphEdgeDto,
    _neighbor_label: &str,
    neighbor_kind: NodeKind,
) -> bool {
    ordinary_incident_call_receipt_is_valid(citation, edge, neighbor_kind)
}

#[derive(Debug, Clone, Copy)]
pub struct FlowRequirement {
    pub id: &'static str,
    pub role: FlowRole,
    pub query_seeds: &'static [&'static str],
    pub coverage_mode: CoverageMode,
    pub proof: FlowProofSpec,
    pub evidence: EvidencePredicate,
}

impl FlowRequirement {
    #[cfg(any(test, feature = "test-support"))]
    pub const fn role_id(&self) -> &'static str {
        self.role.role_id()
    }
}
