//! Durable identity for immutable core database generations.
//!
//! The mutable publication pointer is deliberately smaller than the SQLite
//! database it selects. It binds one active generation, an optional rollback
//! generation, and a digest over that exact pair. The store owns validation,
//! filesystem placement, and atomic replacement; this crate owns only the
//! serializable cross-crate shape.

use serde::{Deserialize, Serialize};

/// Current durable pointer schema.
pub const CORE_PUBLICATION_POINTER_SCHEMA_VERSION: u32 = 1;

/// Identity of one sealed `core/generations/<generation-id>/codestory.db`.
///
/// `logical_bytes` is SQLite `page_count * page_size`, not filesystem
/// allocation. It lets inventory and publication receipts account for the
/// selected image without re-reading the complete database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreGenerationIdentityV1 {
    pub generation_id: String,
    pub run_id: String,
    pub logical_bytes: u64,
    pub published_at_epoch_ms: i64,
}

/// Atomically replaced selector for immutable core generations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorePublicationPointerV1 {
    pub schema_version: u32,
    pub active: CoreGenerationIdentityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<CoreGenerationIdentityV1>,
    /// Lowercase SHA-256 over the schema, active identity, and rollback
    /// identity. Database validation is a separate pre-publication fence.
    pub receipt_digest: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_round_trip_keeps_active_and_rollback_distinct() {
        let pointer = CorePublicationPointerV1 {
            schema_version: CORE_PUBLICATION_POINTER_SCHEMA_VERSION,
            active: CoreGenerationIdentityV1 {
                generation_id: "generation-2".into(),
                run_id: "run-2".into(),
                logical_bytes: 8_192,
                published_at_epoch_ms: 2,
            },
            rollback: Some(CoreGenerationIdentityV1 {
                generation_id: "generation-1".into(),
                run_id: "run-1".into(),
                logical_bytes: 4_096,
                published_at_epoch_ms: 1,
            }),
            receipt_digest: "a".repeat(64),
        };

        let encoded = serde_json::to_vec(&pointer).expect("serialize pointer");
        let decoded: CorePublicationPointerV1 =
            serde_json::from_slice(&encoded).expect("parse pointer");

        assert_eq!(decoded, pointer);
        assert_ne!(decoded.active, decoded.rollback.expect("rollback"));
    }
}
