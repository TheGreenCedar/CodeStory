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

impl From<codestory_core::NodeKind> for NodeKind {
    fn from(value: codestory_core::NodeKind) -> Self {
        match value {
            codestory_core::NodeKind::MODULE => Self::MODULE,
            codestory_core::NodeKind::NAMESPACE => Self::NAMESPACE,
            codestory_core::NodeKind::PACKAGE => Self::PACKAGE,
            codestory_core::NodeKind::FILE => Self::FILE,
            codestory_core::NodeKind::STRUCT => Self::STRUCT,
            codestory_core::NodeKind::CLASS => Self::CLASS,
            codestory_core::NodeKind::INTERFACE => Self::INTERFACE,
            codestory_core::NodeKind::ANNOTATION => Self::ANNOTATION,
            codestory_core::NodeKind::UNION => Self::UNION,
            codestory_core::NodeKind::ENUM => Self::ENUM,
            codestory_core::NodeKind::TYPEDEF => Self::TYPEDEF,
            codestory_core::NodeKind::TYPE_PARAMETER => Self::TYPE_PARAMETER,
            codestory_core::NodeKind::BUILTIN_TYPE => Self::BUILTIN_TYPE,
            codestory_core::NodeKind::FUNCTION => Self::FUNCTION,
            codestory_core::NodeKind::METHOD => Self::METHOD,
            codestory_core::NodeKind::MACRO => Self::MACRO,
            codestory_core::NodeKind::GLOBAL_VARIABLE => Self::GLOBAL_VARIABLE,
            codestory_core::NodeKind::FIELD => Self::FIELD,
            codestory_core::NodeKind::VARIABLE => Self::VARIABLE,
            codestory_core::NodeKind::CONSTANT => Self::CONSTANT,
            codestory_core::NodeKind::ENUM_CONSTANT => Self::ENUM_CONSTANT,
            codestory_core::NodeKind::UNKNOWN => Self::UNKNOWN,
        }
    }
}

impl From<NodeKind> for codestory_core::NodeKind {
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

impl From<codestory_core::EdgeKind> for EdgeKind {
    fn from(value: codestory_core::EdgeKind) -> Self {
        match value {
            codestory_core::EdgeKind::MEMBER => Self::MEMBER,
            codestory_core::EdgeKind::TYPE_USAGE => Self::TYPE_USAGE,
            codestory_core::EdgeKind::USAGE => Self::USAGE,
            codestory_core::EdgeKind::CALL => Self::CALL,
            codestory_core::EdgeKind::INHERITANCE => Self::INHERITANCE,
            codestory_core::EdgeKind::OVERRIDE => Self::OVERRIDE,
            codestory_core::EdgeKind::TYPE_ARGUMENT => Self::TYPE_ARGUMENT,
            codestory_core::EdgeKind::TEMPLATE_SPECIALIZATION => Self::TEMPLATE_SPECIALIZATION,
            codestory_core::EdgeKind::INCLUDE => Self::INCLUDE,
            codestory_core::EdgeKind::IMPORT => Self::IMPORT,
            codestory_core::EdgeKind::MACRO_USAGE => Self::MACRO_USAGE,
            codestory_core::EdgeKind::ANNOTATION_USAGE => Self::ANNOTATION_USAGE,
            codestory_core::EdgeKind::UNKNOWN => Self::UNKNOWN,
        }
    }
}

impl From<EdgeKind> for codestory_core::EdgeKind {
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

impl From<codestory_core::TrailMode> for TrailMode {
    fn from(value: codestory_core::TrailMode) -> Self {
        match value {
            codestory_core::TrailMode::Neighborhood => Self::Neighborhood,
            codestory_core::TrailMode::AllReferenced => Self::AllReferenced,
            codestory_core::TrailMode::AllReferencing => Self::AllReferencing,
            codestory_core::TrailMode::ToTargetSymbol => Self::ToTargetSymbol,
        }
    }
}

impl From<TrailMode> for codestory_core::TrailMode {
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

impl From<codestory_core::TrailDirection> for TrailDirection {
    fn from(value: codestory_core::TrailDirection) -> Self {
        match value {
            codestory_core::TrailDirection::Incoming => Self::Incoming,
            codestory_core::TrailDirection::Outgoing => Self::Outgoing,
            codestory_core::TrailDirection::Both => Self::Both,
        }
    }
}

impl From<TrailDirection> for codestory_core::TrailDirection {
    fn from(value: TrailDirection) -> Self {
        match value {
            TrailDirection::Incoming => Self::Incoming,
            TrailDirection::Outgoing => Self::Outgoing,
            TrailDirection::Both => Self::Both,
        }
    }
}
