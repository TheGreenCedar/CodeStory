//! Whether the files a packet rested on were actually covered by the index.
//!
//! The companion to [`super::packet_freshness`], with one deliberate
//! difference. Freshness treats a *missing* observation as unknown, because a
//! freshness check that did not run proves nothing. Coverage must not: an empty
//! observation list means no path was checked, which is legitimate and caps
//! nothing. A lookup that failed arrives here as a per-path `NotEstablished`
//! observation instead of as an absence.
//!
//! Getting that backwards in either direction is the whole risk. Cap on absence
//! and every packet that cites nothing becomes `Partial`; fail to cap on a
//! failed lookup and the gap this exists to close reopens.

use codestory_contracts::api::{
    SourceCoverageObservationDto, SourceCoverageStatusDto, SourceCoverageUnprovableCauseDto,
};

/// Prefix every coverage gap sentence shares, so callers can partition them.
pub(crate) const PACKET_COVERAGE_GAP_PREFIX: &str = "source coverage";

/// One file this packet rested on that the index could not prove it covered.
#[derive(Debug, Clone)]
struct UnprovableFile {
    path: String,
    cause: SourceCoverageUnprovableCauseDto,
    /// Observed size and the cap that refused it, when the index recorded them.
    sizes: Option<(u64, u64)>,
}

/// The coverage facts for the files one packet touched.
#[derive(Debug, Clone, Default)]
pub(crate) struct PacketCoverageInput {
    unprovable: Vec<UnprovableFile>,
}

impl PacketCoverageInput {
    pub(crate) fn from_observations(observations: &[SourceCoverageObservationDto]) -> Self {
        let unprovable = observations
            .iter()
            .filter_map(|observation| {
                // Matched exhaustively on purpose. `packet_freshness` can afford
                // a `_` arm because its mapping is total over `Option` and runs
                // first; here a wildcard would silently default a newly added
                // status to "proven", which is the failure this type prevents.
                match observation.status {
                    SourceCoverageStatusDto::Indexed => None,
                    SourceCoverageStatusDto::PolicyExcluded
                    | SourceCoverageStatusDto::Incomplete
                    | SourceCoverageStatusDto::NotEstablished => {
                        SourceCoverageUnprovableCauseDto::for_observation(observation).map(
                            |cause| UnprovableFile {
                                path: observation.path.clone(),
                                cause,
                                sizes: observation.observed_size.zip(observation.byte_cap),
                            },
                        )
                    }
                }
            })
            .collect();
        Self { unprovable }
    }

    /// Whether any file this packet rested on could not be proven covered.
    pub(crate) fn caps_sufficiency(&self) -> bool {
        !self.unprovable.is_empty()
    }

    /// One sentence per unprovable file, naming the cause and, where the index
    /// recorded them, the numbers that produced it.
    pub(crate) fn gaps(&self) -> Vec<String> {
        self.unprovable
            .iter()
            .map(|file| match file.sizes {
                Some((observed_size, byte_cap)) => format!(
                    "{PACKET_COVERAGE_GAP_PREFIX} ({}) is unproven for {path}: \
                     {observed_size} bytes exceeds the {byte_cap} byte cap, so the index \
                     never read it.",
                    file.cause.id(),
                    path = file.path
                ),
                None => format!(
                    "{PACKET_COVERAGE_GAP_PREFIX} ({}) is unproven for {path}.",
                    file.cause.id(),
                    path = file.path
                ),
            })
            .collect()
    }

    /// The paths that could not be proven covered.
    pub(crate) fn unprovable_paths(&self) -> Vec<&str> {
        self.unprovable
            .iter()
            .map(|file| file.path.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::SourceCoverageNotEstablishedCauseDto;

    fn observation(path: &str, status: SourceCoverageStatusDto) -> SourceCoverageObservationDto {
        SourceCoverageObservationDto {
            path: path.to_string(),
            status,
            reason: None,
            not_established_cause: None,
            observed_size: None,
            byte_cap: None,
        }
    }

    #[test]
    fn an_excluded_observation_caps_sufficiency() {
        let input = PacketCoverageInput::from_observations(&[observation(
            "data/big.json",
            SourceCoverageStatusDto::PolicyExcluded,
        )]);
        assert!(input.caps_sufficiency());
        assert!(input.gaps()[0].starts_with(PACKET_COVERAGE_GAP_PREFIX));
        assert_eq!(input.unprovable_paths(), vec!["data/big.json"]);
    }

    /// The EV-78 asymmetry, and the reason coverage cannot copy freshness
    /// wholesale: no path checked must cap nothing. This fails the moment
    /// someone swaps the per-path producer for a repository-wide exclusion
    /// list, which is the natural-looking simplification.
    #[test]
    fn no_observations_cap_nothing() {
        let input = PacketCoverageInput::from_observations(&[]);
        assert!(!input.caps_sufficiency());
        assert!(input.gaps().is_empty());
    }

    #[test]
    fn an_indexed_observation_caps_nothing() {
        let input = PacketCoverageInput::from_observations(&[observation(
            "src/main.rs",
            SourceCoverageStatusDto::Indexed,
        )]);
        assert!(!input.caps_sufficiency());
    }

    /// Every status must map to a definite answer. Fails if a variant is added
    /// and absorbed into a permissive arm.
    #[test]
    fn every_status_reaches_a_definite_verdict() {
        for status in [
            SourceCoverageStatusDto::Indexed,
            SourceCoverageStatusDto::PolicyExcluded,
            SourceCoverageStatusDto::Incomplete,
            SourceCoverageStatusDto::NotEstablished,
        ] {
            let caps = PacketCoverageInput::from_observations(&[observation("f.rs", status)])
                .caps_sufficiency();
            assert_eq!(
                caps,
                status != SourceCoverageStatusDto::Indexed,
                "{status:?} must cap unless it is Indexed"
            );
        }
    }

    /// An unnamed defect must stay typed rather than defaulting to covered —
    /// the direct analog of freshness's unlabelled `NotChecked`.
    #[test]
    fn an_unnamed_defect_is_still_unprovable() {
        let incomplete = observation("src/odd.rs", SourceCoverageStatusDto::Incomplete);
        let input = PacketCoverageInput::from_observations(&[incomplete]);
        assert!(input.caps_sufficiency());
        assert!(input.gaps()[0].contains("reason_unreported"));

        let unestablished = observation("src/odd.rs", SourceCoverageStatusDto::NotEstablished);
        let input = PacketCoverageInput::from_observations(&[unestablished]);
        assert!(input.caps_sufficiency());
        assert!(input.gaps()[0].contains("cause_unreported"));
    }

    #[test]
    fn a_failed_lookup_caps_and_names_itself() {
        let mut observation = observation("src/main.rs", SourceCoverageStatusDto::NotEstablished);
        observation.not_established_cause =
            Some(SourceCoverageNotEstablishedCauseDto::LookupUnavailable);
        let input = PacketCoverageInput::from_observations(&[observation]);
        assert!(input.caps_sufficiency());
        assert!(input.gaps()[0].contains("lookup_unavailable"));
    }

    /// A structural source refused for its *unit* count has
    /// `observed_size <= byte_cap`, so a byte-overrun sentence would state
    /// something false about a file the index did read. The producer withholds
    /// the sizes for those rows; this pins that the renderer then says nothing
    /// numeric rather than something wrong.
    #[test]
    fn a_gap_without_sizes_makes_no_claim_about_bytes() {
        let input = PacketCoverageInput::from_observations(&[observation(
            "db/structure.sql",
            SourceCoverageStatusDto::PolicyExcluded,
        )]);
        let gap = &input.gaps()[0];
        assert!(gap.contains("db/structure.sql"), "{gap}");
        assert!(
            !gap.contains("bytes exceeds"),
            "a unit-bound exclusion must not claim a byte overrun: {gap}"
        );
    }

    #[test]
    fn an_exclusion_gap_names_the_size_and_the_cap() {
        let mut observation = observation("data/big.json", SourceCoverageStatusDto::PolicyExcluded);
        observation.observed_size = Some(1_500_000);
        observation.byte_cap = Some(1_048_576);
        let input = PacketCoverageInput::from_observations(&[observation]);
        let gap = &input.gaps()[0];
        assert!(gap.contains("1500000"), "{gap}");
        assert!(gap.contains("1048576"), "{gap}");
    }
}
