use codestory_contracts::api::{EdgeId, EdgeKind, GraphEdgeDto, MemberAccess, NodeId};
use std::sync::OnceLock;

pub(crate) fn edge_certainty_label(
    kind: codestory_contracts::graph::EdgeKind,
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
    confidence: Option<f32>,
) -> Option<String> {
    certainty
        .or_else(|| codestory_contracts::graph::ResolutionCertainty::from_confidence(confidence))
        .or_else(|| structural_edge_default_certainty(kind))
        .map(|value| value.as_str().to_string())
}

fn structural_edge_default_certainty(
    kind: codestory_contracts::graph::EdgeKind,
) -> Option<codestory_contracts::graph::ResolutionCertainty> {
    use codestory_contracts::graph::{EdgeKind, ResolutionCertainty};

    match kind {
        EdgeKind::MEMBER
        | EdgeKind::INHERITANCE
        | EdgeKind::OVERRIDE
        | EdgeKind::TYPE_ARGUMENT
        | EdgeKind::TEMPLATE_SPECIALIZATION
        | EdgeKind::INCLUDE
        | EdgeKind::IMPORT => Some(ResolutionCertainty::Certain),
        EdgeKind::CALL
        | EdgeKind::USAGE
        | EdgeKind::TYPE_USAGE
        | EdgeKind::MACRO_USAGE
        | EdgeKind::ANNOTATION_USAGE
        | EdgeKind::UNKNOWN => None,
    }
}

pub(crate) fn is_structural_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::graph::NodeKind::CLASS
            | codestory_contracts::graph::NodeKind::STRUCT
            | codestory_contracts::graph::NodeKind::INTERFACE
            | codestory_contracts::graph::NodeKind::UNION
            | codestory_contracts::graph::NodeKind::ENUM
            | codestory_contracts::graph::NodeKind::NAMESPACE
            | codestory_contracts::graph::NodeKind::MODULE
    )
}

pub(crate) fn member_access_dto(
    access: Option<codestory_contracts::graph::AccessKind>,
) -> Option<MemberAccess> {
    access.map(MemberAccess::from)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AppGraphFeatureFlags {
    pub(crate) include_edge_certainty: bool,
    pub(crate) include_callsite_identity: bool,
    pub(crate) include_candidate_targets: bool,
}

impl AppGraphFeatureFlags {
    fn from_env() -> Self {
        Self {
            include_edge_certainty: env_flag("CODESTORY_GRAPH_INCLUDE_EDGE_CERTAINTY", true),
            include_callsite_identity: env_flag("CODESTORY_GRAPH_INCLUDE_CALLSITE_IDENTITY", true),
            include_candidate_targets: env_flag("CODESTORY_GRAPH_INCLUDE_CANDIDATE_TARGETS", true),
        }
    }
}

pub(crate) fn app_graph_flags() -> AppGraphFeatureFlags {
    static FLAGS: OnceLock<AppGraphFeatureFlags> = OnceLock::new();
    *FLAGS.get_or_init(AppGraphFeatureFlags::from_env)
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => default,
    }
}

pub(crate) fn graph_edge_dto(
    edge: codestory_contracts::graph::Edge,
    flags: AppGraphFeatureFlags,
) -> GraphEdgeDto {
    GraphEdgeDto {
        id: EdgeId::from(edge.id),
        source: NodeId::from(edge.source),
        target: NodeId::from(edge.target),
        kind: EdgeKind::from(edge.kind),
        confidence: edge.confidence,
        certainty: if flags.include_edge_certainty {
            edge_certainty_label(edge.kind, edge.certainty, edge.confidence)
        } else {
            None
        },
        callsite_identity: if flags.include_callsite_identity {
            edge.callsite_identity.clone()
        } else {
            None
        },
        candidate_targets: if flags.include_candidate_targets {
            edge.candidate_targets
                .iter()
                .copied()
                .map(NodeId::from)
                .collect()
        } else {
            Vec::new()
        },
    }
}
