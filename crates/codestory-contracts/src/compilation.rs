//! Horizon A packet admission and public-status contracts.
//!
//! Compiler, facet, optimizer, receipt, and causal-loss types belong to
//! Horizon B (`#2106`) and must not ship on this surface.

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

/// Conservative upper bound used when a candidate has no measured source size.
pub const INTERIM_UNMEASURED_SOURCE_UPPER_BOUND: usize = 4 * 1024;

/// Production never asserts semantic sufficiency. Offline evaluation owns that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnswerSufficiencyV1 {
    #[default]
    NotAsserted,
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
        assert_eq!(INTERIM_UNMEASURED_SOURCE_UPPER_BOUND, 4 * 1024);
    }

    #[test]
    fn answer_sufficiency_is_always_not_asserted() {
        assert_eq!(
            serde_json::to_value(AnswerSufficiencyV1::NotAsserted).unwrap(),
            serde_json::json!("not_asserted")
        );
    }
}
