//! Packet flow-requirement types retained for obligation/proof helpers.
//! Domain stage lists and prompt→flow dispatchers were deleted in Phase 3.

use crate::packet_evidence_carriers::{
    citation_may_start_command_event_loop_exact_boundary, command_event_loop_driver_call_target,
};
use crate::packet_evidence_roles::{PacketEvidenceRole, packet_evidence_role};
use crate::packet_proof_atoms::FlowProofSpec;
use codestory_contracts::api::{AgentCitationDto, EdgeKind, GraphEdgeDto, NodeKind};

const CALLABLE_NODE_KINDS: &[NodeKind] = &[NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO];
const BEHAVIORAL_OWNER_NODE_KINDS: &[NodeKind] = &[
    NodeKind::FUNCTION,
    NodeKind::METHOD,
    NodeKind::MACRO,
    NodeKind::STRUCT,
    NodeKind::CLASS,
];
const SQL_SCHEMA_NODE_KINDS: &[NodeKind] = &[NodeKind::FILE, NodeKind::ANNOTATION, NodeKind::CLASS];

type SymbolPredicate = fn(&str) -> bool;
type OrderedCallBoundary = (SymbolPredicate, SymbolPredicate);

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

/// What a packet must actually have *cited* for a requirement to count as covered.
///
/// A requirement's `FlowRole` describes where it sits in a flow; it is a label, not a test. Two
/// requirements in one flow may share a role, so matching on the role alone let evidence for one
/// close the other. An evidence predicate belongs to a single requirement and reads only the
/// citation, never the claim's wording.
#[derive(Debug, Clone, Copy)]
pub enum EvidencePredicate {
    /// Covered by a citation the evidence-role classifier places in this part of the flow *and*
    /// that belongs to the subsystem this flow is about.
    ///
    /// The role alone is not enough. The classifier answers a ranking question — "what kind of
    /// evidence is this" — and much of it reads the path: every symbol under `runtime/` is runtime
    /// orchestration, every symbol under `app/` or `views/` is route handling, every symbol under
    /// `flags/` is argument planning. Without the subsystem factor a requirement inherited every
    /// symbol filed in those directories, so `renderChart` in `src/views/` proved a server's
    /// request entrypoint and `Store.delete` proved an indexer's persistence step.
    CitedRoles {
        subsystem: fn(&AgentCitationDto) -> bool,
        roles: &'static [PacketEvidenceRole],
    },
    /// Preserve an established role-backed surface while admitting a structural carrier only when
    /// its cited CALL reaches the next action at this exact flow boundary. A boundary without a
    /// target predicate accepts either an incoming predecessor CALL or an outgoing successor CALL.
    CitedRolesOrCallBoundary {
        subsystem: fn(&AgentCitationDto) -> bool,
        roles: &'static [PacketEvidenceRole],
        carrier: fn(&AgentCitationDto) -> bool,
        call_target: Option<SymbolPredicate>,
    },
    /// Covered by a lawful ordered-stage carrier with an exact CALL either from the preceding
    /// stage or to the following stage. This keeps an unrelated incident CALL from proving the
    /// stage merely because the carrier itself has the right name.
    CitedRolesOrOrderedCallBoundary {
        subsystem: fn(&AgentCitationDto) -> bool,
        roles: &'static [PacketEvidenceRole],
        carrier: fn(&AgentCitationDto) -> bool,
        incoming_source: SymbolPredicate,
        outgoing_target: SymbolPredicate,
    },
    /// Covered by a citation that passes a structural ownership check, used where the evidence
    /// role is too coarse to separate a requirement from its siblings. The carriers carry their own
    /// subsystem factor.
    CitedCarrier(fn(&AgentCitationDto) -> bool),
}

impl EvidencePredicate {
    pub fn citation_proves(self, citation: &AgentCitationDto) -> bool {
        match self {
            Self::CitedRoles { subsystem, roles } => {
                citation_has_named_role(citation, subsystem, roles)
            }
            Self::CitedRolesOrCallBoundary {
                subsystem,
                roles,
                carrier,
                ..
            }
            | Self::CitedRolesOrOrderedCallBoundary {
                subsystem,
                roles,
                carrier,
                ..
            } => citation_has_named_role(citation, subsystem, roles) || carrier(citation),
            Self::CitedCarrier(carrier) => carrier(citation),
        }
    }

    /// Secondary node-kind policy for role-based predicates. Carrier predicates already encode
    /// their own structural contract and return an empty list to mean "predicate-owned".
    pub fn allowed_node_kinds(self) -> &'static [NodeKind] {
        let roles = match self {
            Self::CitedRoles { roles, .. }
            | Self::CitedRolesOrCallBoundary { roles, .. }
            | Self::CitedRolesOrOrderedCallBoundary { roles, .. } => roles,
            Self::CitedCarrier(_) => return &[],
        };
        if roles.iter().any(|role| {
            matches!(
                role,
                PacketEvidenceRole::SqlTableDefinition
                    | PacketEvidenceRole::SqlRelationshipConstraint
                    | PacketEvidenceRole::SqlSchemaFile
            )
        }) {
            SQL_SCHEMA_NODE_KINDS
        } else if roles.iter().any(|role| {
            matches!(
                role,
                PacketEvidenceRole::ClientFactory | PacketEvidenceRole::TransportAdapter
            )
        }) {
            BEHAVIORAL_OWNER_NODE_KINDS
        } else {
            CALLABLE_NODE_KINDS
        }
    }

    /// Role-only evidence can be evaluated without the predicate's exact target. A citation that
    /// also satisfies a call-boundary carrier is only proof after packet finalization validates
    /// that declared boundary, even when its evidence role would otherwise be sufficient.
    pub fn citation_proves_without_call_boundary(self, citation: &AgentCitationDto) -> bool {
        match self {
            Self::CitedRoles { subsystem, roles }
            | Self::CitedRolesOrOrderedCallBoundary {
                subsystem, roles, ..
            } => citation_has_named_role(citation, subsystem, roles),
            Self::CitedRolesOrCallBoundary {
                subsystem,
                roles,
                carrier,
                ..
            } => citation_has_named_role(citation, subsystem, roles) && !carrier(citation),
            Self::CitedCarrier(carrier) => carrier(citation),
        }
    }

    pub fn call_boundary_target(self, citation: &AgentCitationDto) -> Option<SymbolPredicate> {
        let Self::CitedRolesOrCallBoundary {
            carrier,
            call_target,
            ..
        } = self
        else {
            return None;
        };
        carrier(citation).then_some(call_target).flatten()
    }

    pub fn ordered_call_boundary(self, citation: &AgentCitationDto) -> Option<OrderedCallBoundary> {
        let Self::CitedRolesOrOrderedCallBoundary {
            subsystem,
            roles,
            carrier,
            incoming_source,
            outgoing_target,
        } = self
        else {
            return None;
        };
        (citation_has_named_role(citation, subsystem, roles) || carrier(citation))
            .then_some((incoming_source, outgoing_target))
    }
}

/// Validate one cited CALL as a proof receipt for a flow requirement after the caller resolves the
/// incident neighbor. This is deliberately stricter than general graph context: explicit
/// uncertainty never proves a material boundary, and an unresolved syntax-only target needs both
/// parser CALL provenance and a receiver/action pair that satisfies the declared target predicate.
///
/// Callers still own citation-edge identity and graph-neighbor lookup. Keeping the receipt rule in
/// the agent contract lets prebudget reservation, finalization, and runtime candidate filtering use
/// one definition without moving product orchestration into this crate.
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

/// Whether the exact-boundary hydrator may inspect raw CALL rows for this citation. This does not
/// make the citation evidence: discovery-only candidates still need a valid exact receipt before
/// obligation finalization can retain them.
pub fn flow_requirement_call_boundary_is_discoverable(
    requirement: &FlowRequirement,
    citation: &AgentCitationDto,
) -> bool {
    requirement
        .evidence
        .call_boundary_target(citation)
        .is_some()
        || requirement
            .evidence
            .ordered_call_boundary(citation)
            .is_some()
        || (requirement.id == "command_event_loop"
            && citation_may_start_command_event_loop_exact_boundary(citation))
}

pub fn flow_requirement_call_receipt_is_valid(
    requirement: &FlowRequirement,
    citation: &AgentCitationDto,
    edge: &GraphEdgeDto,
    neighbor_label: &str,
    neighbor_kind: NodeKind,
) -> bool {
    if edge.kind != EdgeKind::CALL || edge.source == edge.target {
        return false;
    }
    let target_predicate = if let Some((incoming_source, outgoing_target)) =
        requirement.evidence.ordered_call_boundary(citation)
    {
        if edge.target == citation.node_id {
            Some(incoming_source)
        } else if edge.source == citation.node_id {
            Some(outgoing_target)
        } else {
            return false;
        }
    } else if let Some(outgoing_target) = requirement.evidence.call_boundary_target(citation) {
        if edge.source != citation.node_id {
            return false;
        }
        Some(outgoing_target)
    } else if requirement.id == "command_event_loop"
        && citation_may_start_command_event_loop_exact_boundary(citation)
    {
        if edge.source != citation.node_id {
            return false;
        }
        Some(command_event_loop_driver_call_target as SymbolPredicate)
    } else {
        if !requirement
            .evidence
            .citation_proves_without_call_boundary(citation)
        {
            return false;
        }
        return ordinary_incident_call_receipt_is_valid(citation, edge, neighbor_kind);
    };

    match edge.certainty.as_deref() {
        Some(certainty) if !certainty.eq_ignore_ascii_case("certain") => return false,
        Some(_) if neighbor_kind != NodeKind::UNKNOWN => {
            return target_predicate.is_none_or(|predicate| predicate(neighbor_label));
        }
        Some(_) => return false,
        None if neighbor_kind != NodeKind::UNKNOWN => {
            return target_predicate.is_none_or(|predicate| predicate(neighbor_label));
        }
        None => {}
    }

    let Some(target_predicate) = target_predicate else {
        return false;
    };
    if edge.confidence.is_some() || edge.source != citation.node_id {
        return false;
    }
    let Some(receiver_owner) = parser_call_receiver_owner(edge.callsite_identity.as_deref()) else {
        return false;
    };
    let target = neighbor_label
        .rsplit(['.', ':', '#'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(neighbor_label);
    target_predicate(&format!("{receiver_owner}.{target}"))
}

fn parser_call_receiver_owner(callsite_identity: Option<&str>) -> Option<&str> {
    let identity = callsite_identity?;
    let parser_proven = identity
        .split('|')
        .any(|segment| segment.starts_with("syntax:") && segment.ends_with("-call"));
    if !parser_proven {
        return None;
    }
    identity.split('|').find_map(|segment| {
        segment
            .strip_prefix("receiver-owner:")
            .map(str::trim)
            .filter(|owner| !owner.is_empty())
    })
}

fn citation_has_named_role(
    citation: &AgentCitationDto,
    subsystem: fn(&AgentCitationDto) -> bool,
    roles: &[PacketEvidenceRole],
) -> bool {
    subsystem(citation)
        && packet_evidence_role(citation).is_some_and(|role| roles.contains(&role))
        && role_survives_without_its_path(citation, roles)
}

/// Whether the citation still earns one of `roles` once its path is taken away.
///
/// A path says where a symbol was filed. It cannot say what the symbol does, and the shared role
/// classifier reads it anyway: anything under `runtime/` is runtime orchestration, anything under
/// `app/`, `views/` or `pages/` is route handling, anything under `flags/` is argument planning,
/// anything under `protocol/` is the app-server request protocol. A requirement that took any role
/// the classifier produced therefore inherited every symbol filed in those directories — a symbol
/// named `request` in `src/runtime/` closed a server's dispatch step, and one named `handler` in
/// `app/views/` closed its entrypoint.
///
/// The **file name** is a path segment like any other and the classifier reads it the same way, so
/// stripping only the directories left the defect one level down: `runtime.c`, `store.ts`,
/// `signal_dispatch.rs`, `*_events.jsonl` and a `buffer` stem each still handed out a role on their
/// own, which is how `tooltipHandler` in `src/os/runtime.c` proved a server's dispatch step and
/// `SnapshotDiffViewer` in `src/ui/store.ts` proved an indexer's persistence step. So the whole
/// path goes, down to the extension.
///
/// Asking the question a second time against the bare extension makes the path a purely
/// *narrowing* factor. A `tests/` path still classifies as test coverage on the first question and
/// still fails there; the extension is still present for the `.sql` roles, which are the one place
/// a file genuinely is the evidence. Nothing else about the path can grant a role, and because the
/// full-path answer must match first, this can only reject citations that question already
/// accepted — never admit new ones.
fn role_survives_without_its_path(
    citation: &AgentCitationDto,
    roles: &[PacketEvidenceRole],
) -> bool {
    let Some(path) = citation.file_path.as_deref() else {
        return true;
    };
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let extension = match file_name.rfind('.') {
        Some(index) => &file_name[index..],
        None => "",
    };
    let mut without_path = citation.clone();
    without_path.file_path = Some(extension.to_string());
    packet_evidence_role(&without_path).is_some_and(|role| roles.contains(&role))
}

#[derive(Debug, Clone, Copy)]
pub struct FlowRequirement {
    pub id: &'static str,
    pub role: FlowRole,
    pub query_seeds: &'static [&'static str],
    pub coverage_mode: CoverageMode,
    /// The proof authority this requirement carries. `Legacy` preserves
    /// today's evidence-predicate discharge path exactly; `Atoms` routes
    /// `proof_status` exclusively through the stage-1 proof-atom matcher
    /// (contract R1(a)) — only the six shard requirement ids reference the
    /// const formula groups.
    pub proof: FlowProofSpec,
    pub evidence: EvidencePredicate,
}

impl FlowRequirement {
    #[cfg(any(test, feature = "test-support"))]
    pub const fn role_id(&self) -> &'static str {
        self.role.role_id()
    }
}

