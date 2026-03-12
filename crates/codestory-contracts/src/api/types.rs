use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum IndexMode {
    Full,
    Incremental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[allow(non_camel_case_types)]
pub enum NodeKind {
    // Structural
    MODULE,
    NAMESPACE,
    PACKAGE,
    FILE,

    // Types
    STRUCT,
    CLASS,
    INTERFACE,
    ANNOTATION,
    UNION,
    ENUM,
    TYPEDEF,
    TYPE_PARAMETER,
    BUILTIN_TYPE,

    // Callable/Executable
    FUNCTION,
    METHOD,
    MACRO,

    // Variables/Constants
    GLOBAL_VARIABLE,
    FIELD,
    VARIABLE,
    CONSTANT,
    ENUM_CONSTANT,

    // Other
    UNKNOWN,
}

impl From<crate::graph::NodeKind> for NodeKind {
    fn from(value: crate::graph::NodeKind) -> Self {
        match value {
            crate::graph::NodeKind::MODULE => Self::MODULE,
            crate::graph::NodeKind::NAMESPACE => Self::NAMESPACE,
            crate::graph::NodeKind::PACKAGE => Self::PACKAGE,
            crate::graph::NodeKind::FILE => Self::FILE,
            crate::graph::NodeKind::STRUCT => Self::STRUCT,
            crate::graph::NodeKind::CLASS => Self::CLASS,
            crate::graph::NodeKind::INTERFACE => Self::INTERFACE,
            crate::graph::NodeKind::ANNOTATION => Self::ANNOTATION,
            crate::graph::NodeKind::UNION => Self::UNION,
            crate::graph::NodeKind::ENUM => Self::ENUM,
            crate::graph::NodeKind::TYPEDEF => Self::TYPEDEF,
            crate::graph::NodeKind::TYPE_PARAMETER => Self::TYPE_PARAMETER,
            crate::graph::NodeKind::BUILTIN_TYPE => Self::BUILTIN_TYPE,
            crate::graph::NodeKind::FUNCTION => Self::FUNCTION,
            crate::graph::NodeKind::METHOD => Self::METHOD,
            crate::graph::NodeKind::MACRO => Self::MACRO,
            crate::graph::NodeKind::GLOBAL_VARIABLE => Self::GLOBAL_VARIABLE,
            crate::graph::NodeKind::FIELD => Self::FIELD,
            crate::graph::NodeKind::VARIABLE => Self::VARIABLE,
            crate::graph::NodeKind::CONSTANT => Self::CONSTANT,
            crate::graph::NodeKind::ENUM_CONSTANT => Self::ENUM_CONSTANT,
            crate::graph::NodeKind::UNKNOWN => Self::UNKNOWN,
        }
    }
}

impl From<NodeKind> for crate::graph::NodeKind {
    fn from(value: NodeKind) -> Self {
        match value {
            NodeKind::MODULE => Self::MODULE,
            NodeKind::NAMESPACE => Self::NAMESPACE,
            NodeKind::PACKAGE => Self::PACKAGE,
            NodeKind::FILE => Self::FILE,
            NodeKind::STRUCT => Self::STRUCT,
            NodeKind::CLASS => Self::CLASS,
            NodeKind::INTERFACE => Self::INTERFACE,
            NodeKind::ANNOTATION => Self::ANNOTATION,
            NodeKind::UNION => Self::UNION,
            NodeKind::ENUM => Self::ENUM,
            NodeKind::TYPEDEF => Self::TYPEDEF,
            NodeKind::TYPE_PARAMETER => Self::TYPE_PARAMETER,
            NodeKind::BUILTIN_TYPE => Self::BUILTIN_TYPE,
            NodeKind::FUNCTION => Self::FUNCTION,
            NodeKind::METHOD => Self::METHOD,
            NodeKind::MACRO => Self::MACRO,
            NodeKind::GLOBAL_VARIABLE => Self::GLOBAL_VARIABLE,
            NodeKind::FIELD => Self::FIELD,
            NodeKind::VARIABLE => Self::VARIABLE,
            NodeKind::CONSTANT => Self::CONSTANT,
            NodeKind::ENUM_CONSTANT => Self::ENUM_CONSTANT,
            NodeKind::UNKNOWN => Self::UNKNOWN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[allow(non_camel_case_types)]
pub enum EdgeKind {
    MEMBER,
    TYPE_USAGE,
    USAGE,
    CALL,
    INHERITANCE,
    OVERRIDE,
    TYPE_ARGUMENT,
    TEMPLATE_SPECIALIZATION,
    INCLUDE,
    IMPORT,
    MACRO_USAGE,
    ANNOTATION_USAGE,
    UNKNOWN,
}

impl From<crate::graph::EdgeKind> for EdgeKind {
    fn from(value: crate::graph::EdgeKind) -> Self {
        match value {
            crate::graph::EdgeKind::MEMBER => Self::MEMBER,
            crate::graph::EdgeKind::TYPE_USAGE => Self::TYPE_USAGE,
            crate::graph::EdgeKind::USAGE => Self::USAGE,
            crate::graph::EdgeKind::CALL => Self::CALL,
            crate::graph::EdgeKind::INHERITANCE => Self::INHERITANCE,
            crate::graph::EdgeKind::OVERRIDE => Self::OVERRIDE,
            crate::graph::EdgeKind::TYPE_ARGUMENT => Self::TYPE_ARGUMENT,
            crate::graph::EdgeKind::TEMPLATE_SPECIALIZATION => Self::TEMPLATE_SPECIALIZATION,
            crate::graph::EdgeKind::INCLUDE => Self::INCLUDE,
            crate::graph::EdgeKind::IMPORT => Self::IMPORT,
            crate::graph::EdgeKind::MACRO_USAGE => Self::MACRO_USAGE,
            crate::graph::EdgeKind::ANNOTATION_USAGE => Self::ANNOTATION_USAGE,
            crate::graph::EdgeKind::UNKNOWN => Self::UNKNOWN,
        }
    }
}

impl From<EdgeKind> for crate::graph::EdgeKind {
    fn from(value: EdgeKind) -> Self {
        match value {
            EdgeKind::MEMBER => Self::MEMBER,
            EdgeKind::TYPE_USAGE => Self::TYPE_USAGE,
            EdgeKind::USAGE => Self::USAGE,
            EdgeKind::CALL => Self::CALL,
            EdgeKind::INHERITANCE => Self::INHERITANCE,
            EdgeKind::OVERRIDE => Self::OVERRIDE,
            EdgeKind::TYPE_ARGUMENT => Self::TYPE_ARGUMENT,
            EdgeKind::TEMPLATE_SPECIALIZATION => Self::TEMPLATE_SPECIALIZATION,
            EdgeKind::INCLUDE => Self::INCLUDE,
            EdgeKind::IMPORT => Self::IMPORT,
            EdgeKind::MACRO_USAGE => Self::MACRO_USAGE,
            EdgeKind::ANNOTATION_USAGE => Self::ANNOTATION_USAGE,
            EdgeKind::UNKNOWN => Self::UNKNOWN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum TrailMode {
    #[default]
    Neighborhood,
    AllReferenced,
    AllReferencing,
    ToTargetSymbol,
}

impl From<crate::graph::TrailMode> for TrailMode {
    fn from(value: crate::graph::TrailMode) -> Self {
        match value {
            crate::graph::TrailMode::Neighborhood => Self::Neighborhood,
            crate::graph::TrailMode::AllReferenced => Self::AllReferenced,
            crate::graph::TrailMode::AllReferencing => Self::AllReferencing,
            crate::graph::TrailMode::ToTargetSymbol => Self::ToTargetSymbol,
        }
    }
}

impl From<TrailMode> for crate::graph::TrailMode {
    fn from(value: TrailMode) -> Self {
        match value {
            TrailMode::Neighborhood => Self::Neighborhood,
            TrailMode::AllReferenced => Self::AllReferenced,
            TrailMode::AllReferencing => Self::AllReferencing,
            TrailMode::ToTargetSymbol => Self::ToTargetSymbol,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum TrailDirection {
    Incoming,
    Outgoing,
    Both,
}

impl From<crate::graph::TrailDirection> for TrailDirection {
    fn from(value: crate::graph::TrailDirection) -> Self {
        match value {
            crate::graph::TrailDirection::Incoming => Self::Incoming,
            crate::graph::TrailDirection::Outgoing => Self::Outgoing,
            crate::graph::TrailDirection::Both => Self::Both,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum TrailCallerScope {
    #[default]
    ProductionOnly,
    IncludeTestsAndBenches,
}

impl From<crate::graph::TrailCallerScope> for TrailCallerScope {
    fn from(value: crate::graph::TrailCallerScope) -> Self {
        match value {
            crate::graph::TrailCallerScope::ProductionOnly => Self::ProductionOnly,
            crate::graph::TrailCallerScope::IncludeTestsAndBenches => Self::IncludeTestsAndBenches,
        }
    }
}

impl From<TrailCallerScope> for crate::graph::TrailCallerScope {
    fn from(value: TrailCallerScope) -> Self {
        match value {
            TrailCallerScope::ProductionOnly => Self::ProductionOnly,
            TrailCallerScope::IncludeTestsAndBenches => Self::IncludeTestsAndBenches,
        }
    }
}

impl From<TrailDirection> for crate::graph::TrailDirection {
    fn from(value: TrailDirection) -> Self {
        match value {
            TrailDirection::Incoming => Self::Incoming,
            TrailDirection::Outgoing => Self::Outgoing,
            TrailDirection::Both => Self::Both,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum LayoutDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl From<crate::graph::LayoutDirection> for LayoutDirection {
    fn from(value: crate::graph::LayoutDirection) -> Self {
        match value {
            crate::graph::LayoutDirection::Horizontal => Self::Horizontal,
            crate::graph::LayoutDirection::Vertical => Self::Vertical,
        }
    }
}

impl From<LayoutDirection> for crate::graph::LayoutDirection {
    fn from(value: LayoutDirection) -> Self {
        match value {
            LayoutDirection::Horizontal => Self::Horizontal,
            LayoutDirection::Vertical => Self::Vertical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum MemberAccess {
    #[default]
    Public,
    Protected,
    Private,
    Default,
}

impl From<crate::graph::AccessKind> for MemberAccess {
    fn from(value: crate::graph::AccessKind) -> Self {
        match value {
            crate::graph::AccessKind::Public => Self::Public,
            crate::graph::AccessKind::Protected => Self::Protected,
            crate::graph::AccessKind::Private => Self::Private,
            crate::graph::AccessKind::Default => Self::Default,
        }
    }
}

impl From<MemberAccess> for crate::graph::AccessKind {
    fn from(value: MemberAccess) -> Self {
        match value {
            MemberAccess::Public => Self::Public,
            MemberAccess::Protected => Self::Protected,
            MemberAccess::Private => Self::Private,
            MemberAccess::Default => Self::Default,
        }
    }
}
