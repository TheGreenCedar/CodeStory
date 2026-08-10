//! Retired source-text claim-profile registry.
//!
//! Packet claims are now derived from typed obligations, evidence roles, source tiers, and
//! validated graph relations. The empty versioned registry remains in the public telemetry path
//! so older consumers keep receiving the same DTO shape and can see that no heuristic profile
//! participated in an answer.

use std::sync::OnceLock;

use crate::packet_claim_profile_registry::{ClaimProfileRegistry, load_claim_profile_registry};
use crate::packet_profile_telemetry::PacketClaimProfileRegistrySummary;

const CLAIM_PROFILE_DOCUMENT: &str = include_str!("data/claim_profiles.v2.json");

pub fn claim_profile_registry() -> &'static ClaimProfileRegistry {
    static REGISTRY: OnceLock<ClaimProfileRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| load_claim_profile_registry(CLAIM_PROFILE_DOCUMENT, &[]))
}

pub fn packet_claim_profile_registry_summary() -> PacketClaimProfileRegistrySummary {
    let registry = claim_profile_registry();
    PacketClaimProfileRegistrySummary {
        registered: registry.profiles().len(),
        contracted: registry.contracted(),
        pending: registry.pending(),
        pending_ratchet: registry.declared_ratchet(),
        rejected: registry.rejected().len(),
        rejection_codes: registry.rejection_codes(),
        document_rejection: registry.document_rejection().map(|reason| reason.code()),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub use crate::packet_claim_profiles_legacy_tests::{
    packet_generic_css_animation_flow_claims, packet_generic_string_predicate_flow_claims,
    packet_source_derived_claims_for_citation, packet_source_derived_claims_for_citation_counted,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_registry_is_an_explicit_zero_ratchet() {
        let summary = packet_claim_profile_registry_summary();
        assert_eq!(summary.registered, 0);
        assert_eq!(summary.contracted, 0);
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.pending_ratchet, 0);
        assert_eq!(summary.rejected, 0);
        assert!(summary.rejection_codes.is_empty());
        assert_eq!(summary.document_rejection, None);
    }
}
