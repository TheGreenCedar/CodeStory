//! Typed freshness input to packet sufficiency (EV-7).
//!
//! Serving and proving are separate questions. The runtime serving waiver
//! (`index_freshness_admits_operation`) deliberately keeps a bounded-inventory repository usable:
//! discovery stopped at a size bound, but the publication itself is complete, and refusing there
//! would lock every large repository out of packet and search with no way back in. That waiver is
//! unchanged by this module.
//!
//! What the waiver never established is that the publication still matches the working tree. A
//! packet assembled over an unobserved publication can cite a symbol that no longer exists, so
//! reporting it `sufficient` is the false-safe answer #1200 exists to remove. This module carries
//! that distinction as a type: freshness reaches sufficiency as `Fresh`, `Unknown { cause }`, or
//! `Stale`, and anything but `Fresh` caps the verdict at `partial` with a typed gap.
//!
//! `Unknown` is the permanent floor beneath the filesystem observer (EV-7b): when the observer is
//! unavailable, overflows, or is unsupported on the host, it degrades into a typed cause here
//! rather than into a silent `Fresh`.

use codestory_contracts::api::{
    FreshnessUnknownCauseDto, IndexFreshnessDto, IndexFreshnessStatusDto,
};

/// Machine-readable prefix every freshness gap carries, so callers can match the gap class
/// without parsing prose.
pub(crate) const PACKET_FRESHNESS_GAP_PREFIX: &str = "freshness";

/// Freshness as packet sufficiency must consume it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacketFreshnessInput {
    /// The full planned inventory was compared and matched.
    Fresh,
    /// Drift was never established. The publication may still be servable; it is not provable.
    Unknown { cause: FreshnessUnknownCauseDto },
    /// Drift was observed against the working tree.
    Stale,
}

impl PacketFreshnessInput {
    /// Read a freshness observation as a sufficiency input.
    ///
    /// The absent observation is `Unknown`, not `Fresh`: on the production path `None` means the
    /// freshness call itself failed, and defaulting a failed check to "fresh" is precisely the
    /// CR-001 exposure.
    pub(crate) fn from_observation(freshness: Option<&IndexFreshnessDto>) -> Self {
        if let Some(cause) = FreshnessUnknownCauseDto::for_observation(freshness) {
            return Self::Unknown { cause };
        }
        match freshness.map(|freshness| freshness.status) {
            Some(IndexFreshnessStatusDto::Stale) => Self::Stale,
            // `for_observation` already claimed every non-verdict observation, so the only shape
            // left is a compared, matching inventory.
            _ => Self::Fresh,
        }
    }

    /// Whether this input forbids reporting broad evidence as sufficient.
    pub(crate) fn caps_sufficiency(self) -> bool {
        !matches!(self, Self::Fresh)
    }

    /// The typed gap this input publishes on the packet verdict, if any.
    pub(crate) fn gap(self) -> Option<String> {
        match self {
            Self::Fresh => None,
            Self::Unknown { cause } => Some(format!(
                "{PACKET_FRESHNESS_GAP_PREFIX} unknown ({}): index drift against the working tree \
                 was never established, so this packet's evidence cannot be reported as proven. \
                 Local navigation over the current publication remains available.",
                cause.id()
            )),
            Self::Stale => Some(format!(
                "{PACKET_FRESHNESS_GAP_PREFIX} stale: the index has changed, new, or removed files \
                 relative to the working tree, so this packet's evidence cannot be reported as \
                 proven."
            )),
        }
    }
}

/// A fully compared, matching inventory, for fixtures whose subject is not freshness.
///
/// Test answers default to no freshness observation, which this module correctly reads as
/// `Unknown`. Fixtures that are exercising claim or route behavior have to opt in to an observed
/// publication or every one of them would be testing the freshness cap instead.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn fresh_index_observation() -> IndexFreshnessDto {
    IndexFreshnessDto {
        status: IndexFreshnessStatusDto::Fresh,
        changed_file_count: 0,
        new_file_count: 0,
        removed_file_count: 0,
        checked_file_count: 8,
        indexed_file_count: 8,
        duration_ms: 1,
        reason: None,
        not_checked_cause: None,
        samples: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::IndexFreshnessNotCheckedCauseDto;

    fn observation(
        status: IndexFreshnessStatusDto,
        cause: Option<IndexFreshnessNotCheckedCauseDto>,
    ) -> IndexFreshnessDto {
        IndexFreshnessDto {
            status,
            changed_file_count: 0,
            new_file_count: 0,
            removed_file_count: 0,
            checked_file_count: 4,
            indexed_file_count: 4,
            duration_ms: 1,
            reason: None,
            not_checked_cause: cause,
            samples: Vec::new(),
        }
    }

    #[test]
    fn bounded_inventory_is_unknown_and_caps_sufficiency() {
        let freshness = observation(
            IndexFreshnessStatusDto::NotChecked,
            Some(IndexFreshnessNotCheckedCauseDto::BoundedInventory),
        );

        let input = PacketFreshnessInput::from_observation(Some(&freshness));

        assert_eq!(
            input,
            PacketFreshnessInput::Unknown {
                cause: FreshnessUnknownCauseDto::BoundedInventory
            }
        );
        assert!(input.caps_sufficiency());
        assert!(
            input
                .gap()
                .expect("unknown freshness publishes a gap")
                .contains("freshness unknown (bounded_inventory)")
        );
    }

    #[test]
    fn absent_observation_is_unknown_not_fresh() {
        let input = PacketFreshnessInput::from_observation(None);

        assert_eq!(
            input,
            PacketFreshnessInput::Unknown {
                cause: FreshnessUnknownCauseDto::ObservationUnavailable
            }
        );
        assert!(input.caps_sufficiency());
    }

    #[test]
    fn not_checked_without_a_cause_is_still_typed_unknown() {
        let freshness = observation(IndexFreshnessStatusDto::NotChecked, None);

        assert_eq!(
            PacketFreshnessInput::from_observation(Some(&freshness)),
            PacketFreshnessInput::Unknown {
                cause: FreshnessUnknownCauseDto::CauseUnreported
            }
        );
    }

    #[test]
    fn fresh_observation_publishes_no_gap_and_does_not_cap() {
        let freshness = observation(IndexFreshnessStatusDto::Fresh, None);

        let input = PacketFreshnessInput::from_observation(Some(&freshness));

        assert_eq!(input, PacketFreshnessInput::Fresh);
        assert!(!input.caps_sufficiency());
        assert_eq!(input.gap(), None);
        assert_eq!(
            FreshnessUnknownCauseDto::for_observation(Some(&freshness)),
            None
        );
    }

    #[test]
    fn stale_caps_sufficiency_but_is_not_reported_as_unknown() {
        let freshness = observation(IndexFreshnessStatusDto::Stale, None);

        let input = PacketFreshnessInput::from_observation(Some(&freshness));

        assert_eq!(input, PacketFreshnessInput::Stale);
        assert!(input.caps_sufficiency());
        assert!(
            input
                .gap()
                .expect("stale freshness publishes a gap")
                .starts_with("freshness stale:")
        );
        assert_eq!(
            FreshnessUnknownCauseDto::for_observation(Some(&freshness)),
            None
        );
    }
}
