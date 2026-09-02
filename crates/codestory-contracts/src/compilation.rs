//! Horizon A packet admission and public-status contracts.
//!
//! Repository-derived compiler input and selection contracts belong to
//! Horizon B (`#2106`) and do not ship on this interim surface.

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

/// Version of the retrieval score consumed by packet admission.
pub const PACKET_RETRIEVAL_SCORE_VERSION_V1: &str = "retrieval-score/v1";

/// Production never asserts semantic sufficiency. Offline evaluation owns that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnswerSufficiencyV1 {
    #[default]
    NotAsserted,
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
    fn packet_descriptors_can_fail_closed_without_a_source_bound() {
        let value = serde_json::to_value(PacketCandidateDescriptorV1 {
            stable_identity: "node:7".into(),
            path: "src/lib.rs".into(),
            symbol: Some("crate::run".into()),
            retrieval_lane: PacketRetrievalLaneV1::Lexical,
            retrieval_score: VersionedRetrievalScoreV1 {
                version: PACKET_RETRIEVAL_SCORE_VERSION_V1.into(),
                value: 0.75,
            },
            source_bytes_upper_bound: None,
            exact_selector_ordinal: None,
        })
        .unwrap();
        assert!(value.get("source_bytes_upper_bound").is_none());
    }
}
