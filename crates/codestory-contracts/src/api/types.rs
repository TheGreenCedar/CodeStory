use serde::{Deserialize, Serialize};
use specta::Type;

macro_rules! impl_mirrored_enum_conversions {
    ($api:ty, $core:ty, [$($variant:ident),+ $(,)?]) => {
        impl From<$core> for $api {
            fn from(value: $core) -> Self {
                match value {
                    $(<$core>::$variant => Self::$variant,)+
                }
            }
        }

        impl From<$api> for $core {
            fn from(value: $api) -> Self {
                match value {
                    $(<$api>::$variant => Self::$variant,)+
                }
            }
        }
    };
}

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

impl_mirrored_enum_conversions!(
    NodeKind,
    crate::graph::NodeKind,
    [
        MODULE,
        NAMESPACE,
        PACKAGE,
        FILE,
        STRUCT,
        CLASS,
        INTERFACE,
        ANNOTATION,
        UNION,
        ENUM,
        TYPEDEF,
        TYPE_PARAMETER,
        BUILTIN_TYPE,
        FUNCTION,
        METHOD,
        MACRO,
        GLOBAL_VARIABLE,
        FIELD,
        VARIABLE,
        CONSTANT,
        ENUM_CONSTANT,
        UNKNOWN,
    ]
);

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

impl_mirrored_enum_conversions!(
    EdgeKind,
    crate::graph::EdgeKind,
    [
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
    ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum TrailMode {
    #[default]
    Neighborhood,
    AllReferenced,
    AllReferencing,
    ToTargetSymbol,
}

impl_mirrored_enum_conversions!(
    TrailMode,
    crate::graph::TrailMode,
    [Neighborhood, AllReferenced, AllReferencing, ToTargetSymbol,]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum TrailDirection {
    Incoming,
    Outgoing,
    Both,
}

impl_mirrored_enum_conversions!(
    TrailDirection,
    crate::graph::TrailDirection,
    [Incoming, Outgoing, Both,]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum TrailCallerScope {
    #[default]
    ProductionOnly,
    IncludeTestsAndBenches,
}

impl_mirrored_enum_conversions!(
    TrailCallerScope,
    crate::graph::TrailCallerScope,
    [ProductionOnly, IncludeTestsAndBenches,]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum LayoutDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl_mirrored_enum_conversions!(
    LayoutDirection,
    crate::graph::LayoutDirection,
    [Horizontal, Vertical,]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum MemberAccess {
    #[default]
    Public,
    Protected,
    Private,
    Default,
}

impl_mirrored_enum_conversions!(
    MemberAccess,
    crate::graph::AccessKind,
    [Public, Protected, Private, Default,]
);
