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

/// Requested indexing mode.
///
/// Emission stays PascalCase. The `snake_case` aliases accept the spelling the
/// mirrored `IndexPublicationModeDto` emits on the same wire surfaces, so a
/// caller that read one vocabulary can write the other without a rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum IndexMode {
    #[serde(alias = "full")]
    Full,
    #[serde(alias = "incremental")]
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

/// Declared member access.
///
/// Emission stays PascalCase. The `snake_case` aliases accept the spelling the
/// mirrored `CanonicalMemberVisibility` emits on the same wire surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type, Default)]
pub enum MemberAccess {
    #[default]
    #[serde(alias = "public")]
    Public,
    #[serde(alias = "protected")]
    Protected,
    #[serde(alias = "private")]
    Private,
    #[serde(alias = "default")]
    Default,
}

impl_mirrored_enum_conversions!(
    MemberAccess,
    crate::graph::AccessKind,
    [Public, Protected, Private, Default,]
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrored_request_enums_accept_both_casings_and_emit_one() {
        // CR-032: `IndexMode` and `MemberAccess` emit PascalCase while their
        // mirrored response enums emit snake_case for the same vocabulary. The
        // aliases let a caller write back what it read; emission is unchanged.
        for spelling in ["Incremental", "incremental"] {
            let parsed: IndexMode = serde_json::from_value(serde_json::json!(spelling))
                .unwrap_or_else(|error| panic!("decode {spelling}: {error}"));
            assert_eq!(parsed, IndexMode::Incremental);
        }
        assert_eq!(
            serde_json::to_value(IndexMode::Incremental).expect("serialize index mode"),
            serde_json::json!("Incremental"),
            "emission must stay PascalCase"
        );

        let request: crate::api::StartIndexingRequest =
            serde_json::from_str(r#"{"mode":"full"}"#).expect("snake_case request mode decodes");
        assert_eq!(request.mode, IndexMode::Full);

        for spelling in ["Private", "private"] {
            let parsed: MemberAccess = serde_json::from_value(serde_json::json!(spelling))
                .unwrap_or_else(|error| panic!("decode {spelling}: {error}"));
            assert_eq!(parsed, MemberAccess::Private);
        }
        assert_eq!(
            serde_json::to_value(MemberAccess::Private).expect("serialize member access"),
            serde_json::json!("Private"),
            "emission must stay PascalCase"
        );

        assert!(
            serde_json::from_value::<IndexMode>(serde_json::json!("INCREMENTAL")).is_err(),
            "aliases add the mirrored spelling only, not arbitrary casing"
        );
    }
}
