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

use std::collections::BTreeMap;

/// Version of the claim-profile contract whose counters this telemetry describes.
///
/// A trace recorded in the field is only comparable against another trace with the same
/// version, so the number is published beside the counts and pinned by contract tests.
pub(crate) const PACKET_CLAIM_PROFILE_CONTRACT_VERSION: u32 = 1;

/// Which layer produced a packet claim.
///
/// ARCH-036 established that source-grounded profile claims and name-derived templates are
/// different facts. Counting them apart is what lets a field trace answer "did the fitted
/// layer fire, or did this packet degrade to naming?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PacketClaimSource {
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
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::SourceProfile => "source_profile",
            Self::CommandProfile => "command_profile",
            Self::FlowTemplate => "flow_template",
            Self::RoleTemplate => "role_template",
            Self::EvalProbe => "eval_probe",
        }
    }

    pub(crate) const ALL: [Self; 5] = [
        Self::SourceProfile,
        Self::CommandProfile,
        Self::FlowTemplate,
        Self::RoleTemplate,
        Self::EvalProbe,
    ];
}

/// Per-profile fire counts for one packet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PacketProfileFireCount {
    /// Citations this profile was offered.
    pub(crate) evaluated: u32,
    /// Citations for which this profile emitted at least one claim.
    pub(crate) fired: u32,
    /// Claims this profile emitted.
    pub(crate) claims: u32,
    /// Citations where a runtime contract violation skipped this profile before it ran.
    pub(crate) skipped_invalid: u32,
    /// Typed code of the violation that caused the skip, when there was one.
    pub(crate) skip_reason: Option<&'static str>,
}

/// Fire-rate and claim-source counters accumulated while assembling one packet's claims.
#[derive(Debug, Clone, Default)]
pub(crate) struct PacketClaimTelemetry {
    citations_considered: u32,
    profiles: BTreeMap<&'static str, PacketProfileFireCount>,
    sources: BTreeMap<PacketClaimSource, u32>,
}

impl PacketClaimTelemetry {
    pub(crate) fn record_citation_considered(&mut self) {
        self.citations_considered = self.citations_considered.saturating_add(1);
    }

    pub(crate) fn record_profile_evaluated(&mut self, profile_id: &'static str, claims: usize) {
        let entry = self.profiles.entry(profile_id).or_default();
        entry.evaluated = entry.evaluated.saturating_add(1);
        if claims > 0 {
            entry.fired = entry.fired.saturating_add(1);
            entry.claims = entry
                .claims
                .saturating_add(u32::try_from(claims).unwrap_or(u32::MAX));
        }
    }

    pub(crate) fn record_profile_skipped(
        &mut self,
        profile_id: &'static str,
        violation_code: &'static str,
    ) {
        let entry = self.profiles.entry(profile_id).or_default();
        entry.evaluated = entry.evaluated.saturating_add(1);
        entry.skipped_invalid = entry.skipped_invalid.saturating_add(1);
        entry.skip_reason = Some(violation_code);
    }

    pub(crate) fn record_claim_source(&mut self, source: PacketClaimSource, claims: usize) {
        if claims == 0 {
            return;
        }
        let entry = self.sources.entry(source).or_default();
        *entry = entry.saturating_add(u32::try_from(claims).unwrap_or(u32::MAX));
    }

    #[cfg(test)]
    pub(crate) fn citations_considered(&self) -> u32 {
        self.citations_considered
    }

    #[cfg(test)]
    pub(crate) fn profile_fire_count(&self, profile_id: &str) -> PacketProfileFireCount {
        self.profiles.get(profile_id).copied().unwrap_or_default()
    }

    pub(crate) fn claim_source_count(&self, source: PacketClaimSource) -> u32 {
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

    /// Redaction-safe trace lines: static keys, registry profile ids, and integers only.
    pub(crate) fn trace_annotations(
        &self,
        registry: PacketClaimProfileRegistrySummary,
    ) -> Vec<String> {
        let mut annotations = Vec::new();
        annotations.push(format!(
            "packet_claim_profile_contract version={} profiles={} contracted={} pending={} pending_ratchet={} citations_considered={} profiles_fired={} profiles_skipped_invalid={}",
            PACKET_CLAIM_PROFILE_CONTRACT_VERSION,
            registry.registered,
            registry.contracted,
            registry.pending,
            registry.pending_ratchet,
            self.citations_considered,
            self.profiles_fired(),
            self.profiles_skipped(),
        ));

        let mut fire_rates = String::from("packet_claim_profile_fire_rates");
        for (profile_id, count) in &self.profiles {
            fire_rates.push_str(&format!(
                " {profile_id}={}/{} claims={} skipped={}",
                count.fired, count.evaluated, count.claims, count.skipped_invalid
            ));
            if let Some(reason) = count.skip_reason {
                fire_rates.push_str(&format!(" skip_reason={reason}"));
            }
        }
        annotations.push(fire_rates);

        let mut sources = String::from("packet_claim_sources");
        for source in PacketClaimSource::ALL {
            sources.push_str(&format!(
                " {}={}",
                source.id(),
                self.claim_source_count(source)
            ));
        }
        annotations.push(sources);

        annotations
    }
}

/// Static shape of the claim-profile registry, published beside the counts so a field trace
/// records how many profiles existed when the counters were taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PacketClaimProfileRegistrySummary {
    pub(crate) registered: usize,
    pub(crate) contracted: usize,
    pub(crate) pending: usize,
    pub(crate) pending_ratchet: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> PacketClaimProfileRegistrySummary {
        PacketClaimProfileRegistrySummary {
            registered: 20,
            contracted: 4,
            pending: 16,
            pending_ratchet: 16,
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

        let annotation = telemetry
            .trace_annotations(registry())
            .into_iter()
            .find(|line| line.starts_with("packet_claim_sources"))
            .expect("claim-source annotation");
        assert_eq!(
            annotation,
            "packet_claim_sources source_profile=3 command_profile=0 flow_template=0 role_template=7 eval_probe=0"
        );
    }

    #[test]
    fn trace_annotations_publish_the_contract_version_and_registry_shape() {
        let telemetry = PacketClaimTelemetry::default();
        let annotations = telemetry.trace_annotations(registry());
        let expected = [
            "packet_claim_profile_contract",
            &format!("version={PACKET_CLAIM_PROFILE_CONTRACT_VERSION}"),
            "profiles=20",
            "contracted=4",
            "pending=16",
            "pending_ratchet=16",
            "citations_considered=0",
            "profiles_fired=0",
            "profiles_skipped_invalid=0",
        ]
        .join(" ");
        assert_eq!(annotations[0], expected);
    }

    #[test]
    fn trace_annotations_carry_only_static_ids_and_integers() {
        let mut telemetry = PacketClaimTelemetry::default();
        telemetry.record_citation_considered();
        telemetry.record_profile_evaluated("object-mapping-plan", 1);
        telemetry.record_claim_source(PacketClaimSource::SourceProfile, 1);

        for annotation in telemetry.trace_annotations(registry()) {
            assert!(
                annotation.chars().all(|ch| ch.is_ascii_lowercase()
                    || ch.is_ascii_digit()
                    || matches!(ch, '_' | '-' | '=' | '/' | ' ')),
                "telemetry annotation must not carry free text: {annotation}"
            );
        }
    }
}
