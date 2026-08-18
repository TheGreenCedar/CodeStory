//! Fire-rate and claim-source telemetry for the versioned packet claim-profile contract.
//!
//! ARCH-005 recorded ~8.4k production lines of fitted claim heuristics whose fire rate on
//! real repositories was unmeasured: when a profile silently does not fire the packet falls
//! back to name-derived templates while reporting the same calibrated-looking confidence.
//! These counters make that fallback visible in the packet trace.
//!
//! Everything published here is derived from the static profile registry and from integer
//! counts. No citation display name, file path, query, or source excerpt may enter a counter
//! key or value: the trace is retained field telemetry, not an evidence surface.
//!
//! The counters ride on the typed `packet_claim_profile_telemetry` trace field, never on
//! `retrieval_trace.annotations`. Annotations are the packet's evidence channel: consumers scan
//! them for gap markers and downgrade packet confidence when one matches. Always-on telemetry
//! published there is misread as a permanent evidence gap, so the two are kept structurally
//! apart rather than separated by how the text happens to be worded.

use std::collections::BTreeMap;

use codestory_contracts::api::{
    PacketClaimProfileFireRateDto, PacketClaimProfileTelemetryDto, PacketClaimSourceCountDto,
    PacketClaimSourceDto,
};

/// Version of the claim-profile contract whose counters this telemetry describes.
///
/// A trace recorded in the field is only comparable against another trace with the same
/// version, so the number is published beside the counts and pinned by contract tests. There
/// is one number: it is the schema version of the checked-in registry document, so a trace
/// cannot claim a contract shape the loaded data was not written against.
pub const PACKET_CLAIM_PROFILE_CONTRACT_VERSION: u32 =
    crate::packet_claim_profile_registry::PACKET_CLAIM_PROFILE_SCHEMA_VERSION;

/// Which layer produced a packet claim.
///
/// ARCH-036 established that source-grounded profile claims and name-derived templates are
/// different facts. Counting them apart is what lets a field trace answer "did the fitted
/// layer fire, or did this packet degrade to naming?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PacketClaimSource {
    /// Source-text-derived claims from the versioned product claim-profile registry.
    SourceProfile,
    /// Claims templated from a command/subcommand shape in the question.
    CommandProfile,
    /// Generic flow templates that are neither profile- nor role-derived.
    FlowTemplate,
    /// Name-and-path-derived evidence-role sentences.
    RoleTemplate,
    /// Test-only evaluation-probe flow templates.
    EvalProbe,
}

impl PacketClaimSource {
    pub const ALL: [Self; 5] = [
        Self::SourceProfile,
        Self::CommandProfile,
        Self::FlowTemplate,
        Self::RoleTemplate,
        Self::EvalProbe,
    ];

    const fn dto(self) -> PacketClaimSourceDto {
        match self {
            Self::SourceProfile => PacketClaimSourceDto::SourceProfile,
            Self::CommandProfile => PacketClaimSourceDto::CommandProfile,
            Self::FlowTemplate => PacketClaimSourceDto::FlowTemplate,
            Self::RoleTemplate => PacketClaimSourceDto::RoleTemplate,
            Self::EvalProbe => PacketClaimSourceDto::EvalProbe,
        }
    }
}

/// Per-profile fire counts for one packet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PacketProfileFireCount {
    /// Citations this profile was offered.
    pub evaluated: u32,
    /// Citations for which this profile emitted at least one claim.
    pub fired: u32,
    /// Claims this profile emitted.
    pub claims: u32,
    /// Citations where a runtime contract violation skipped this profile before it ran.
    pub skipped_invalid: u32,
    /// Typed code of the violation that caused the skip, when there was one.
    pub skip_reason: Option<&'static str>,
}

/// Fire-rate and claim-source counters accumulated while assembling one packet's claims.
#[derive(Debug, Clone, Default)]
pub struct PacketClaimTelemetry {
    citations_considered: u32,
    profiles: BTreeMap<&'static str, PacketProfileFireCount>,
    sources: BTreeMap<PacketClaimSource, u32>,
}

impl PacketClaimTelemetry {
    pub fn record_citation_considered(&mut self) {
        self.citations_considered = self.citations_considered.saturating_add(1);
    }

    pub fn record_profile_evaluated(&mut self, profile_id: &'static str, claims: usize) {
        let entry = self.profiles.entry(profile_id).or_default();
        entry.evaluated = entry.evaluated.saturating_add(1);
        if claims > 0 {
            entry.fired = entry.fired.saturating_add(1);
            entry.claims = entry
                .claims
                .saturating_add(u32::try_from(claims).unwrap_or(u32::MAX));
        }
    }

    pub fn record_profile_skipped(
        &mut self,
        profile_id: &'static str,
        violation_code: &'static str,
    ) {
        let entry = self.profiles.entry(profile_id).or_default();
        entry.evaluated = entry.evaluated.saturating_add(1);
        entry.skipped_invalid = entry.skipped_invalid.saturating_add(1);
        entry.skip_reason = Some(violation_code);
    }

    pub fn record_claim_source(&mut self, source: PacketClaimSource, claims: usize) {
        if claims == 0 {
            return;
        }
        let entry = self.sources.entry(source).or_default();
        *entry = entry.saturating_add(u32::try_from(claims).unwrap_or(u32::MAX));
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn citations_considered(&self) -> u32 {
        self.citations_considered
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn profile_fire_count(&self, profile_id: &str) -> PacketProfileFireCount {
        self.profiles.get(profile_id).copied().unwrap_or_default()
    }

    pub fn claim_source_count(&self, source: PacketClaimSource) -> u32 {
        self.sources.get(&source).copied().unwrap_or(0)
    }

    fn profiles_fired(&self) -> usize {
        self.profiles
            .values()
            .filter(|count| count.fired > 0)
            .count()
    }

    fn profiles_skipped(&self) -> usize {
        self.profiles
            .values()
            .filter(|count| count.skipped_invalid > 0)
            .count()
    }

    /// Redaction-safe typed telemetry: static registry ids and integer counts only.
    ///
    /// This is deliberately *not* a `Vec<String>` appended to `retrieval_trace.annotations`.
    /// Annotations are scanned as evidence, so an always-on telemetry line published there is
    /// classified as an evidence gap on every packet and permanently downgrades confidence.
    pub fn to_dto(
        &self,
        registry: PacketClaimProfileRegistrySummary,
    ) -> PacketClaimProfileTelemetryDto {
        PacketClaimProfileTelemetryDto {
            contract_version: PACKET_CLAIM_PROFILE_CONTRACT_VERSION,
            registered_profiles: saturating_count(registry.registered),
            contracted_profiles: saturating_count(registry.contracted),
            pending_profiles: saturating_count(registry.pending),
            pending_ratchet: saturating_count(registry.pending_ratchet),
            rejected_profiles: saturating_count(registry.rejected),
            rejected_reasons: registry
                .rejection_codes
                .iter()
                .map(|code| (*code).to_string())
                .collect(),
            registry_error: registry.document_rejection.map(str::to_string),
            citations_considered: self.citations_considered,
            profiles_fired: saturating_count(self.profiles_fired()),
            profiles_skipped_invalid: saturating_count(self.profiles_skipped()),
            profiles: self
                .profiles
                .iter()
                .map(|(profile_id, count)| PacketClaimProfileFireRateDto {
                    profile_id: (*profile_id).to_string(),
                    evaluated: count.evaluated,
                    fired: count.fired,
                    claims: count.claims,
                    skipped_invalid: count.skipped_invalid,
                    skip_reason: count.skip_reason.map(str::to_string),
                })
                .collect(),
            claim_sources: PacketClaimSource::ALL
                .into_iter()
                .map(|source| PacketClaimSourceCountDto {
                    source: source.dto(),
                    claims: self.claim_source_count(source),
                })
                .collect(),
        }
    }
}

fn saturating_count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Static shape of the claim-profile registry, published beside the counts so a field trace
/// records how many profiles existed when the counters were taken.
///
/// `rejected` and `document_rejection` describe what the versioned-data loader refused. They
/// exist because the loader fails closed: a refused row or a refused document removes profiles,
/// and a packet that quietly answered from a smaller registry would otherwise look identical to
/// one that answered from the whole registry and found nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketClaimProfileRegistrySummary {
    pub registered: usize,
    pub contracted: usize,
    pub pending: usize,
    pub pending_ratchet: usize,
    pub rejected: usize,
    /// Distinct static codes for the refused rows.
    pub rejection_codes: Vec<&'static str>,
    /// Static code of the whole-document refusal, when the registry loaded empty.
    pub document_rejection: Option<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> PacketClaimProfileRegistrySummary {
        PacketClaimProfileRegistrySummary {
            registered: 7,
            contracted: 7,
            pending: 0,
            pending_ratchet: 0,
            rejected: 0,
            rejection_codes: Vec::new(),
            document_rejection: None,
        }
    }

    #[test]
    fn fire_rate_counters_separate_evaluated_fired_and_skipped() {
        let mut telemetry = PacketClaimTelemetry::default();
        telemetry.record_citation_considered();
        telemetry.record_profile_evaluated("shell-version-use", 2);
        telemetry.record_citation_considered();
        telemetry.record_profile_evaluated("shell-version-use", 0);
        telemetry.record_profile_skipped("session-request-dispatch", "no_allowed_proof_roles");

        let fired = telemetry.profile_fire_count("shell-version-use");
        assert_eq!(fired.evaluated, 2);
        assert_eq!(fired.fired, 1);
        assert_eq!(fired.claims, 2);
        assert_eq!(fired.skipped_invalid, 0);

        let skipped = telemetry.profile_fire_count("session-request-dispatch");
        assert_eq!(skipped.evaluated, 1);
        assert_eq!(skipped.fired, 0);
        assert_eq!(skipped.skipped_invalid, 1);

        assert_eq!(telemetry.citations_considered(), 2);
    }

    #[test]
    fn claim_source_counters_are_reported_for_every_layer() {
        let mut telemetry = PacketClaimTelemetry::default();
        telemetry.record_claim_source(PacketClaimSource::SourceProfile, 3);
        telemetry.record_claim_source(PacketClaimSource::RoleTemplate, 7);
        telemetry.record_claim_source(PacketClaimSource::FlowTemplate, 0);

        let dto = telemetry.to_dto(registry());
        assert_eq!(
            dto.claim_sources,
            vec![
                PacketClaimSourceCountDto {
                    source: PacketClaimSourceDto::SourceProfile,
                    claims: 3,
                },
                PacketClaimSourceCountDto {
                    source: PacketClaimSourceDto::CommandProfile,
                    claims: 0,
                },
                PacketClaimSourceCountDto {
                    source: PacketClaimSourceDto::FlowTemplate,
                    claims: 0,
                },
                PacketClaimSourceCountDto {
                    source: PacketClaimSourceDto::RoleTemplate,
                    claims: 7,
                },
                PacketClaimSourceCountDto {
                    source: PacketClaimSourceDto::EvalProbe,
                    claims: 0,
                },
            ]
        );
    }

    #[test]
    fn typed_telemetry_publishes_the_contract_version_and_registry_shape() {
        let dto = PacketClaimTelemetry::default().to_dto(registry());
        assert_eq!(dto.contract_version, PACKET_CLAIM_PROFILE_CONTRACT_VERSION);
        assert_eq!(dto.registered_profiles, 7);
        assert_eq!(dto.contracted_profiles, 7);
        assert_eq!(dto.pending_profiles, 0);
        assert_eq!(dto.pending_ratchet, 0);
        assert_eq!(dto.citations_considered, 0);
        assert_eq!(dto.profiles_fired, 0);
        assert_eq!(dto.profiles_skipped_invalid, 0);
        assert_eq!(dto.rejected_profiles, 0);
        assert_eq!(dto.registry_error, None);
        assert!(dto.profiles.is_empty());
    }

    #[test]
    fn a_refused_registry_document_is_reported_as_a_typed_error_beside_empty_counts() {
        // The loader fails closed, so a refused document leaves the packet answering from
        // name-derived templates alone. Without these two fields that packet is indistinguishable
        // in the trace from one whose profiles were offered and simply stayed quiet.
        let dto = PacketClaimTelemetry::default().to_dto(PacketClaimProfileRegistrySummary {
            registered: 0,
            contracted: 0,
            pending: 0,
            pending_ratchet: 0,
            rejected: 0,
            rejection_codes: Vec::new(),
            document_rejection: Some("schema_version_mismatch"),
        });
        assert_eq!(dto.registered_profiles, 0);
        assert_eq!(
            dto.registry_error.as_deref(),
            Some("schema_version_mismatch")
        );

        let partial = PacketClaimTelemetry::default().to_dto(PacketClaimProfileRegistrySummary {
            rejected: 2,
            rejection_codes: vec!["unknown_profile_id", "missing_owner_issue"],
            ..registry()
        });
        assert_eq!(partial.rejected_profiles, 2);
        assert_eq!(
            partial.rejected_reasons,
            vec![
                "unknown_profile_id".to_string(),
                "missing_owner_issue".to_string()
            ]
        );
        assert_eq!(partial.registry_error, None);
    }

    #[test]
    fn typed_telemetry_carries_only_static_ids_and_integers() {
        let mut telemetry = PacketClaimTelemetry::default();
        telemetry.record_citation_considered();
        telemetry.record_profile_evaluated("object-mapping-plan", 1);
        telemetry.record_profile_skipped("session-request-dispatch", "no_allowed_proof_roles");
        telemetry.record_claim_source(PacketClaimSource::SourceProfile, 1);

        let dto = telemetry.to_dto(registry());
        let static_id = |value: &str| {
            value
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-'))
        };
        for profile in &dto.profiles {
            assert!(
                static_id(&profile.profile_id),
                "telemetry must not carry free text: {}",
                profile.profile_id
            );
            if let Some(reason) = &profile.skip_reason {
                assert!(
                    static_id(reason),
                    "telemetry must not carry free text: {reason}"
                );
            }
        }
    }

    #[test]
    fn typed_telemetry_serializes_without_a_free_text_annotation_channel() {
        // The counters must never be reachable as annotation prose: annotation text is scanned
        // for gap markers, and "skipped" in an always-on telemetry line reads as a permanent
        // evidence gap on every packet.
        let mut telemetry = PacketClaimTelemetry::default();
        telemetry.record_citation_considered();
        telemetry.record_profile_skipped("session-request-dispatch", "no_allowed_proof_roles");

        let json = serde_json::to_value(telemetry.to_dto(registry())).expect("serialize telemetry");
        assert!(
            json.get("annotations").is_none(),
            "claim-profile telemetry must not expose an annotation channel: {json}"
        );
        assert_eq!(json["profiles_skipped_invalid"], serde_json::json!(1));
        assert_eq!(
            json["profiles"][0]["skip_reason"],
            serde_json::json!("no_allowed_proof_roles")
        );

        // A field trace is only comparable against another trace with the same layer names.
        let layers: Vec<&str> = json["claim_sources"]
            .as_array()
            .expect("claim source array")
            .iter()
            .map(|entry| entry["source"].as_str().expect("layer id"))
            .collect();
        assert_eq!(
            layers,
            [
                "source_profile",
                "command_profile",
                "flow_template",
                "role_template",
                "eval_probe"
            ]
        );
    }
}
