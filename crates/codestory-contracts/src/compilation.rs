//! Packet seed, admission, and repository-derived compilation contracts.
//!
//! These types make the compiler boundary enforceable. Only
//! [`RetrievalSeedPlanV1`] may carry the original question. Everything after
//! retrieval is expressed as stable identities, hydrated repository evidence,
//! and a pinned publication.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Complete serialized public packet/response budget (source, citations, gaps,
/// continuation metadata, provenance, and publication identity).
pub const PUBLIC_PACKET_SERIALIZED_MAX_BYTES: usize = 16 * 1024;

/// Interim admission before exact hydration: at most this many identities
/// across the whole packet, including initial retrieval and subqueries.
pub const INTERIM_MAX_ADMITTED_CANDIDATES: usize = 16;

/// Packet-scoped conservative source-byte budget charged before exact
/// source/graph hydration. Matches the public serialized packet cap.
pub const INTERIM_MAX_ADMITTED_SOURCE_BYTES: usize = PUBLIC_PACKET_SERIALIZED_MAX_BYTES;

/// Maximum source bytes one interim candidate may contribute to the public
/// evidence row. Retrieval descriptors record this bound explicitly; a
/// missing bound is not silently replaced with a speculative fallback.
pub const INTERIM_SOURCE_ROW_UPPER_BOUND: usize = 512;

/// Production never asserts semantic sufficiency. Offline evaluation owns that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnswerSufficiencyV1 {
    #[default]
    NotAsserted,
}

pub const PACKET_COMPILATION_CONTRACT_VERSION_V1: u16 = 1;
pub const PACKET_RETRIEVAL_SCORE_VERSION_V1: &str = "retrieval-score/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PacketSeedSelectorV1 {
    ExactPath { path: String },
    CanonicalId { id: String },
    QualifiedSymbol { symbol: String },
}

/// The sole compiler-side value allowed to retain the original wording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct RetrievalSeedPlanV1 {
    pub contract_version: u16,
    pub generic_query: String,
    /// Exact selectors copied from typed probes. Raw question text has no
    /// authority to populate this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_selectors: Vec<PacketSeedSelectorV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub free_queries: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PacketRetrievalLaneV1 {
    ExactSelector,
    Lexical,
    Semantic,
    Graph,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct VersionedRetrievalScoreV1 {
    pub version: String,
    pub value: f32,
}

/// Metadata sufficient to decide admission without opening core records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketCandidateDescriptorV1 {
    pub stable_identity: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub retrieval_lane: PacketRetrievalLaneV1,
    pub retrieval_score: VersionedRetrievalScoreV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_bytes_upper_bound: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_selector_ordinal: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PacketAdmissionOriginV1 {
    ExactTypedSelector,
    Retrieval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketAdmissionReceiptV1 {
    pub packet_ordinal: u32,
    pub stable_identity: String,
    pub score_version: String,
    pub reserved_source_bytes: u32,
    pub origin: PacketAdmissionOriginV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PacketAdmissionGapKindV1 {
    CandidateCountExceeded,
    SourceBudgetExceeded,
    StableIdentityMissing,
    SourceBoundMissing,
    SourceUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketAdmissionGapV1 {
    pub kind: PacketAdmissionGapKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_selector_ordinal: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PacketParserCompletenessV1 {
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketHydratedSourceRangeV1 {
    pub stable_identity: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub source: String,
    pub parser_completeness: PacketParserCompletenessV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PacketRelationCertaintyV1 {
    Certain,
    Probable,
    Uncertain,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PacketRelationKindV1 {
    Member,
    TypeUsage,
    Usage,
    Call,
    Inheritance,
    Override,
    TypeArgument,
    TemplateSpecialization,
    Include,
    Import,
    MacroUsage,
    AnnotationUsage,
    Unknown,
}

impl PacketRelationKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::TypeUsage => "type_usage",
            Self::Usage => "usage",
            Self::Call => "call",
            Self::Inheritance => "inheritance",
            Self::Override => "override",
            Self::TypeArgument => "type_argument",
            Self::TemplateSpecialization => "template_specialization",
            Self::Include => "include",
            Self::Import => "import",
            Self::MacroUsage => "macro_usage",
            Self::AnnotationUsage => "annotation_usage",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketDirectedRelationV1 {
    pub relation_id: String,
    pub from_identity: String,
    pub to_identity: String,
    pub relation_kind: PacketRelationKindV1,
    pub certainty: PacketRelationCertaintyV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketIdentityAmbiguityV1 {
    pub selector: String,
    #[serde(default)]
    pub candidate_identities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketCompilationPublicationV1 {
    pub project_id: String,
    pub core_generation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_generation: Option<String>,
}

/// The pure compiler input. It intentionally cannot carry prompt text, task
/// classes, obligations, coverage roles, or answer-stage labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketCompilationInputV1 {
    pub contract_version: u16,
    pub publication: PacketCompilationPublicationV1,
    #[serde(default)]
    pub admissions: Vec<PacketAdmissionReceiptV1>,
    #[serde(default)]
    pub sources: Vec<PacketHydratedSourceRangeV1>,
    #[serde(default)]
    pub relations: Vec<PacketDirectedRelationV1>,
    #[serde(default)]
    pub ambiguities: Vec<PacketIdentityAmbiguityV1>,
    #[serde(default)]
    pub admission_gaps: Vec<PacketAdmissionGapV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PacketStructuralGapReasonV1 {
    CandidateCountExceeded,
    SourceBudgetExceeded,
    SourceUnavailable,
    AmbiguousSelector,
    DisconnectedSeed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct PacketContinuationSelectorV1 {
    pub stable_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<String>,
    pub reason: PacketStructuralGapReasonV1,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizon_a_admission_constants_are_stable() {
        assert_eq!(PUBLIC_PACKET_SERIALIZED_MAX_BYTES, 16 * 1024);
        assert_eq!(INTERIM_MAX_ADMITTED_CANDIDATES, 16);
        assert_eq!(
            INTERIM_MAX_ADMITTED_SOURCE_BYTES,
            PUBLIC_PACKET_SERIALIZED_MAX_BYTES
        );
        assert_eq!(INTERIM_SOURCE_ROW_UPPER_BOUND, 512);
    }

    #[test]
    fn answer_sufficiency_is_always_not_asserted() {
        assert_eq!(
            serde_json::to_value(AnswerSufficiencyV1::NotAsserted).unwrap(),
            serde_json::json!("not_asserted")
        );
    }

    #[test]
    fn compilation_input_cannot_serialize_prompt_policy() {
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: Some("retrieval".into()),
            },
            admissions: Vec::new(),
            sources: Vec::new(),
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let value = serde_json::to_value(input).unwrap();
        for forbidden in [
            "question",
            "prompt",
            "task_class",
            "obligations",
            "coverage_role",
        ] {
            assert!(
                value.get(forbidden).is_none(),
                "unexpected {forbidden}: {value}"
            );
        }
    }

    #[test]
    fn directed_relation_kind_is_a_closed_typed_contract() {
        let error = serde_json::from_value::<PacketDirectedRelationV1>(serde_json::json!({
            "relation_id": "edge-1",
            "from_identity": "node:1",
            "to_identity": "node:2",
            "relation_kind": "benchmark_shaped_relation",
            "certainty": "certain"
        }))
        .expect_err("arbitrary relation labels must not enter compilation");
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }
}
