//! Versioned-data loader for the packet claim-profile registry.
//!
//! ARCH-005 recorded the claim layer as a hand-written Rust array: which profiles exist, what
//! each one is contracted to, and who owns it were all compiled-in facts that only a Rust diff
//! could restate. EV-6 gave that array a runtime-enforced contract and typed telemetry; this
//! module makes the array itself checked-in, schema-versioned data and reuses the EV-6 contract
//! as its loader.
//!
//! Three properties are load-bearing and each has a typed failure:
//!
//! * **Schema-versioned.** The document states the schema it was written against. A document
//!   whose version is not the one this binary implements is refused whole — no profile claims
//!   are served from a shape the binary does not understand.
//! * **Fail-closed.** Every rejection removes a profile from the registry; none ever adds one.
//!   A malformed document yields an empty registry, which degrades the packet to name-derived
//!   claims rather than serving claims from an unvalidated contract. Rejections are counted so
//!   the degradation is visible in the trace instead of silent.
//! * **Ratcheted.** The number of profiles still shipping without an anti-overfit contract is a
//!   compile-time ceiling. Data may declare a ratchet at or below that ceiling and may carry at
//!   or below its own ratchet; data can never raise either. Burning a profile down therefore
//!   takes a Rust diff *and* a data diff in the same change, and cannot be undone by data alone.
//!
//! Nothing here reads repository text. Profile identities are interned against the compiled
//! matcher list, so a telemetry key is always a static slug and never a string from the document.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::packet_flow_requirements::{CoverageMode, FlowRole};

/// Schema version of the checked-in claim-profile document this binary implements.
///
/// Published in the packet trace beside the counters, so a field trace records the contract
/// shape its numbers were taken under.
pub const PACKET_CLAIM_PROFILE_SCHEMA_VERSION: u32 = 2;

/// Compile-time ceiling on registry profiles still allowed to ship without an anti-overfit
/// contract.
///
/// The ratchet only ever falls: a new profile has to arrive contracted, and migrating a pending
/// profile has to lower this number and the document's `pending_ratchet` in the same diff.
pub const PACKET_CLAIM_PROFILE_PENDING_MIGRATION_RATCHET: usize = 0;

/// Typed reasons a contracted profile is not runtime-valid.
///
/// Validation runs twice on purpose: once when the document is loaded, so an invalid row never
/// enters the registry, and again per citation at collect time, so a contract constructed in
/// process still cannot serve claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimProfileContractViolation {
    NonProductScope,
    DomainIsNotProfileIdentity,
    DiagnosticOnlyEvidenceTier,
    NoAllowedProofRoles,
    DuplicateAllowedProofRole,
    MissingPositiveFixture,
    MissingFalsePositiveFixture,
    FixtureIdsNotDistinct,
    MissingGeneralizationFixture,
    GeneralizationFixtureNotDistinct,
}

impl ClaimProfileContractViolation {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NonProductScope => "non_product_scope",
            Self::DomainIsNotProfileIdentity => "domain_is_not_profile_identity",
            Self::DiagnosticOnlyEvidenceTier => "diagnostic_only_evidence_tier",
            Self::NoAllowedProofRoles => "no_allowed_proof_roles",
            Self::DuplicateAllowedProofRole => "duplicate_allowed_proof_role",
            Self::MissingPositiveFixture => "missing_positive_fixture",
            Self::MissingFalsePositiveFixture => "missing_false_positive_fixture",
            Self::FixtureIdsNotDistinct => "fixture_ids_not_distinct",
            Self::MissingGeneralizationFixture => "missing_generalization_fixture",
            Self::GeneralizationFixtureNotDistinct => "generalization_fixture_not_distinct",
        }
    }
}

/// Typed reasons the loader refused one document row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimProfileRowRejection {
    /// The document names a profile no compiled matcher implements.
    UnknownProfileId,
    /// The document lists the same profile twice.
    DuplicateProfileId,
    /// The row carries no owning issue number, so a field failure has nobody to route to.
    MissingOwnerIssue,

    /// A contracted row shipped without a contract block.
    MissingContract,
    /// A pending row shipped a contract block, which would read as contracted coverage.
    ContractOnPendingProfile,
    /// A contracted row shipped without the measured evidence that justified the migration.
    MissingContractEvidence,
    /// The row names an evidence tier this binary does not implement.
    UnknownEvidenceTier,
    /// The row names a proof role this binary does not implement.
    UnknownProofRole,
    /// The row names a scope this binary does not implement.
    UnknownScope,
    /// The row names a status this binary does not implement.
    UnknownStatus,
    /// The contract block is present but fails EV-6 runtime validation.
    Contract(ClaimProfileContractViolation),
}

impl ClaimProfileRowRejection {
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownProfileId => "unknown_profile_id",
            Self::DuplicateProfileId => "duplicate_profile_id",
            Self::MissingOwnerIssue => "missing_owner_issue",
            Self::MissingContract => "missing_contract",
            Self::ContractOnPendingProfile => "contract_on_pending_profile",
            Self::MissingContractEvidence => "missing_contract_evidence",
            Self::UnknownEvidenceTier => "unknown_evidence_tier",
            Self::UnknownProofRole => "unknown_proof_role",
            Self::UnknownScope => "unknown_scope",
            Self::UnknownStatus => "unknown_status",
            Self::Contract(violation) => violation.code(),
        }
    }
}

/// Typed reasons the loader refused the whole document.
///
/// Every one of these empties the registry: an unreadable contract serves nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimProfileDocumentRejection {
    Malformed,
    SchemaVersionMismatch,
    RatchetAboveCeiling,
    PendingAboveRatchet,
}

impl ClaimProfileDocumentRejection {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Malformed => "malformed_document",
            Self::SchemaVersionMismatch => "schema_version_mismatch",
            Self::RatchetAboveCeiling => "ratchet_above_ceiling",
            Self::PendingAboveRatchet => "pending_above_ratchet",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimProfileScope {
    Product,
}

/// Anti-overfit contract for one profile, as loaded from the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimProfileContract {
    pub domain: String,
    pub scope: ClaimProfileScope,
    pub allowed_evidence_tier: CoverageMode,
    pub allowed_proof_roles: Vec<FlowRole>,
    pub positive_fixture_id: String,
    pub false_positive_fixture_id: String,
    /// Fixture from a different language family than `positive_fixture_id`, proving the profile
    /// fires somewhere other than the corpus shape it was written against.
    pub generalization_fixture_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimProfileStatus {
    Contracted(ClaimProfileContract),
    PendingMigration,
}

impl ClaimProfileStatus {
    /// EV-6 runtime validation, reused as the document loader's row validator.
    pub fn validate(&self, profile_id: &str) -> Result<(), ClaimProfileContractViolation> {
        let Self::Contracted(contract) = self else {
            return Ok(());
        };
        if !matches!(contract.scope, ClaimProfileScope::Product) {
            return Err(ClaimProfileContractViolation::NonProductScope);
        }
        if contract.domain != profile_id {
            return Err(ClaimProfileContractViolation::DomainIsNotProfileIdentity);
        }
        if matches!(contract.allowed_evidence_tier, CoverageMode::DiagnosticOnly) {
            return Err(ClaimProfileContractViolation::DiagnosticOnlyEvidenceTier);
        }
        if contract.allowed_proof_roles.is_empty() {
            return Err(ClaimProfileContractViolation::NoAllowedProofRoles);
        }
        for (index, role) in contract.allowed_proof_roles.iter().enumerate() {
            if contract.allowed_proof_roles[..index]
                .iter()
                .any(|earlier| earlier == role)
            {
                return Err(ClaimProfileContractViolation::DuplicateAllowedProofRole);
            }
        }
        if contract.positive_fixture_id.is_empty() {
            return Err(ClaimProfileContractViolation::MissingPositiveFixture);
        }
        if contract.false_positive_fixture_id.is_empty() {
            return Err(ClaimProfileContractViolation::MissingFalsePositiveFixture);
        }
        if contract.positive_fixture_id == contract.false_positive_fixture_id {
            return Err(ClaimProfileContractViolation::FixtureIdsNotDistinct);
        }
        if contract.generalization_fixture_id.is_empty() {
            return Err(ClaimProfileContractViolation::MissingGeneralizationFixture);
        }
        if contract.generalization_fixture_id == contract.positive_fixture_id
            || contract.generalization_fixture_id == contract.false_positive_fixture_id
        {
            return Err(ClaimProfileContractViolation::GeneralizationFixtureNotDistinct);
        }
        Ok(())
    }
}

/// One accepted document row, bound to the compiled matcher identity it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredClaimProfile {
    /// Interned against the compiled matcher list, never a string from the document.
    pub id: &'static str,
    pub status: ClaimProfileStatus,
    /// Tracking issue number that owns this profile. A number, not a URL: the generalization
    /// lint bans this repository's own identity from production paths, and attribution does
    /// not need the host to be routable.
    pub owner_issue: u32,
}

/// One refused document row. Refused rows never serve claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedClaimProfile {
    pub reason: ClaimProfileRowRejection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimProfileRegistry {
    profiles: Vec<RegisteredClaimProfile>,
    rejected: Vec<RejectedClaimProfile>,
    declared_ratchet: usize,
    document_rejection: Option<ClaimProfileDocumentRejection>,
}

impl ClaimProfileRegistry {
    fn refused(rejection: ClaimProfileDocumentRejection) -> Self {
        Self {
            profiles: Vec::new(),
            rejected: Vec::new(),
            declared_ratchet: 0,
            document_rejection: Some(rejection),
        }
    }

    pub fn profiles(&self) -> &[RegisteredClaimProfile] {
        &self.profiles
    }

    pub fn rejected(&self) -> &[RejectedClaimProfile] {
        &self.rejected
    }

    /// Distinct typed codes for the rows this load refused, in stable order.
    ///
    /// A count alone says the registry shrank; the codes say why, which is the difference
    /// between "the document has a typo" and "the document was written for another binary".
    pub fn rejection_codes(&self) -> Vec<&'static str> {
        let codes: BTreeSet<&'static str> = self
            .rejected
            .iter()
            .map(|entry| entry.reason.code())
            .collect();
        codes.into_iter().collect()
    }

    pub fn declared_ratchet(&self) -> usize {
        self.declared_ratchet
    }

    pub fn document_rejection(&self) -> Option<ClaimProfileDocumentRejection> {
        self.document_rejection
    }

    pub fn pending(&self) -> usize {
        self.profiles
            .iter()
            .filter(|entry| matches!(entry.status, ClaimProfileStatus::PendingMigration))
            .count()
    }

    pub fn contracted(&self) -> usize {
        self.profiles.len() - self.pending()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimProfileDocument {
    schema_version: u32,
    pending_ratchet: usize,
    profiles: Vec<ClaimProfileDocumentRow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimProfileDocumentRow {
    id: String,
    status: String,
    attribution: ClaimProfileAttributionRow,
    #[serde(default)]
    contract: Option<ClaimProfileContractRow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimProfileAttributionRow {
    owner_issue: u32,
    /// What was measured before this profile left the pending ratchet. Required on contracted
    /// rows; a burn-down without stated evidence is a deletion, not a migration.
    #[serde(default)]
    migration_evidence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimProfileContractRow {
    domain: String,
    scope: String,
    allowed_evidence_tier: String,
    allowed_proof_roles: Vec<String>,
    positive_fixture_id: String,
    false_positive_fixture_id: String,
    generalization_fixture_id: String,
}

const MIGRATION_EVIDENCE_FLOOR: usize = 40;

fn parse_scope(value: &str) -> Option<ClaimProfileScope> {
    match value {
        "product" => Some(ClaimProfileScope::Product),
        _ => None,
    }
}

fn parse_evidence_tier(value: &str) -> Option<CoverageMode> {
    match value {
        "requires_resolved_source_or_graph" => Some(CoverageMode::RequiresResolvedSourceOrGraph),
        "allows_source_range" => Some(CoverageMode::AllowsSourceRange),
        "allows_lexical_source" => Some(CoverageMode::AllowsLexicalSource),
        "diagnostic_only" => Some(CoverageMode::DiagnosticOnly),
        _ => None,
    }
}

fn parse_proof_role(value: &str) -> Option<FlowRole> {
    match value {
        "entrypoint" => Some(FlowRole::Entrypoint),
        "registration" => Some(FlowRole::Registration),
        "configuration" => Some(FlowRole::Configuration),
        "state_or_storage" => Some(FlowRole::StateOrStorage),
        "dispatch" => Some(FlowRole::Dispatch),
        "transform_or_validate" => Some(FlowRole::TransformOrValidate),
        "terminal_boundary" => Some(FlowRole::TerminalBoundary),
        "error_or_fallback" => Some(FlowRole::ErrorOrFallback),
        _ => None,
    }
}

/// Load the checked-in registry document.
///
/// `known_ids` is the compiled matcher list. A row is accepted only when the document identity
/// matches one of them, and the accepted row carries the *compiled* `&'static str`, so no
/// document string ever reaches a telemetry key.
pub fn load_claim_profile_registry(
    document: &str,
    known_ids: &[&'static str],
) -> ClaimProfileRegistry {
    load_claim_profile_registry_with_ceiling(
        document,
        known_ids,
        PACKET_CLAIM_PROFILE_PENDING_MIGRATION_RATCHET,
    )
}

fn load_claim_profile_registry_with_ceiling(
    document: &str,
    known_ids: &[&'static str],
    pending_ceiling: usize,
) -> ClaimProfileRegistry {
    let Ok(parsed) = serde_json::from_str::<ClaimProfileDocument>(document) else {
        return ClaimProfileRegistry::refused(ClaimProfileDocumentRejection::Malformed);
    };
    if parsed.schema_version != PACKET_CLAIM_PROFILE_SCHEMA_VERSION {
        return ClaimProfileRegistry::refused(ClaimProfileDocumentRejection::SchemaVersionMismatch);
    }
    if parsed.pending_ratchet > pending_ceiling {
        return ClaimProfileRegistry::refused(ClaimProfileDocumentRejection::RatchetAboveCeiling);
    }

    let mut profiles = Vec::new();
    let mut rejected = Vec::new();
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();

    for row in parsed.profiles {
        match accept_row(&row, known_ids, &mut seen) {
            Ok(profile) => profiles.push(profile),
            Err(reason) => rejected.push(RejectedClaimProfile { reason }),
        }
    }

    let pending = profiles
        .iter()
        .filter(|entry| matches!(entry.status, ClaimProfileStatus::PendingMigration))
        .count();
    if pending > parsed.pending_ratchet {
        return ClaimProfileRegistry::refused(ClaimProfileDocumentRejection::PendingAboveRatchet);
    }

    ClaimProfileRegistry {
        profiles,
        rejected,
        declared_ratchet: parsed.pending_ratchet,
        document_rejection: None,
    }
}

fn accept_row(
    row: &ClaimProfileDocumentRow,
    known_ids: &[&'static str],
    seen: &mut BTreeSet<&'static str>,
) -> Result<RegisteredClaimProfile, ClaimProfileRowRejection> {
    let Some(id) = known_ids.iter().copied().find(|known| *known == row.id) else {
        return Err(ClaimProfileRowRejection::UnknownProfileId);
    };
    if !seen.insert(id) {
        return Err(ClaimProfileRowRejection::DuplicateProfileId);
    }
    let owner_issue = row.attribution.owner_issue;
    if owner_issue == 0 {
        return Err(ClaimProfileRowRejection::MissingOwnerIssue);
    }

    let status = match row.status.as_str() {
        "pending_migration" => {
            if row.contract.is_some() {
                return Err(ClaimProfileRowRejection::ContractOnPendingProfile);
            }
            ClaimProfileStatus::PendingMigration
        }
        "contracted" => {
            let Some(contract) = row.contract.as_ref() else {
                return Err(ClaimProfileRowRejection::MissingContract);
            };
            if row.attribution.migration_evidence.trim().len() < MIGRATION_EVIDENCE_FLOOR {
                return Err(ClaimProfileRowRejection::MissingContractEvidence);
            }
            let Some(scope) = parse_scope(&contract.scope) else {
                return Err(ClaimProfileRowRejection::UnknownScope);
            };
            let Some(allowed_evidence_tier) = parse_evidence_tier(&contract.allowed_evidence_tier)
            else {
                return Err(ClaimProfileRowRejection::UnknownEvidenceTier);
            };
            let mut allowed_proof_roles = Vec::with_capacity(contract.allowed_proof_roles.len());
            for role in &contract.allowed_proof_roles {
                let Some(parsed) = parse_proof_role(role) else {
                    return Err(ClaimProfileRowRejection::UnknownProofRole);
                };
                allowed_proof_roles.push(parsed);
            }
            ClaimProfileStatus::Contracted(ClaimProfileContract {
                domain: contract.domain.clone(),
                scope,
                allowed_evidence_tier,
                allowed_proof_roles,
                positive_fixture_id: contract.positive_fixture_id.clone(),
                false_positive_fixture_id: contract.false_positive_fixture_id.clone(),
                generalization_fixture_id: contract.generalization_fixture_id.clone(),
            })
        }
        _ => return Err(ClaimProfileRowRejection::UnknownStatus),
    };

    status
        .validate(id)
        .map_err(ClaimProfileRowRejection::Contract)?;

    Ok(RegisteredClaimProfile {
        id,
        status,
        owner_issue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: &[&str] = &["shell-version-use", "sql-schema"];

    fn contracted_row(id: &str) -> String {
        format!(
            r#"{{
              "id": "{id}",
              "status": "contracted",
              "attribution": {{
                "owner_issue": 1674,
                "migration_evidence": "fixture triple measures fire on two families and silence on the helper"
              }},
              "contract": {{
                "domain": "{id}",
                "scope": "product",
                "allowed_evidence_tier": "allows_lexical_source",
                "allowed_proof_roles": ["dispatch"],
                "positive_fixture_id": "{id}-positive",
                "false_positive_fixture_id": "{id}-negative",
                "generalization_fixture_id": "{id}-generalization"
              }}
            }}"#
        )
    }

    fn contracted_row_without_contract(id: &str) -> String {
        format!(
            r#"{{
              "id": "{id}",
              "status": "contracted",
              "attribution": {{
                "owner_issue": 1674,
                "migration_evidence": "fixture triple measures fire on two families and silence on the helper"
              }}
            }}"#
        )
    }

    fn pending_row(id: &str) -> String {
        format!(
            r#"{{
              "id": "{id}",
              "status": "pending_migration",
              "attribution": {{
                "owner_issue": 1573
              }}
            }}"#
        )
    }

    fn document(ratchet: usize, rows: &[String]) -> String {
        format!(
            r#"{{"schema_version": {PACKET_CLAIM_PROFILE_SCHEMA_VERSION}, "pending_ratchet": {ratchet}, "profiles": [{}]}}"#,
            rows.join(",")
        )
    }

    fn load_parser_fixture(document: &str, known_ids: &[&'static str]) -> ClaimProfileRegistry {
        load_claim_profile_registry_with_ceiling(document, known_ids, usize::MAX)
    }

    #[test]
    fn a_well_formed_document_loads_both_statuses() {
        let registry = load_parser_fixture(
            &document(
                1,
                &[
                    contracted_row("shell-version-use"),
                    pending_row("sql-schema"),
                ],
            ),
            KNOWN,
        );
        assert_eq!(registry.document_rejection(), None);
        assert_eq!(registry.profiles().len(), 2);
        assert_eq!(registry.contracted(), 1);
        assert_eq!(registry.pending(), 1);
        assert_eq!(registry.declared_ratchet(), 1);
        assert!(registry.rejected().is_empty());
        // The accepted identity is the compiled slug, not the document's string.
        assert!(std::ptr::eq(registry.profiles()[0].id, KNOWN[0]));
    }

    #[test]
    fn a_malformed_document_serves_no_profiles_at_all() {
        let registry = load_parser_fixture("{ not json", KNOWN);
        assert_eq!(
            registry.document_rejection(),
            Some(ClaimProfileDocumentRejection::Malformed)
        );
        assert!(registry.profiles().is_empty());
    }

    #[test]
    fn a_document_written_against_another_schema_serves_no_profiles() {
        let raw = document(1, &[pending_row("sql-schema")]).replace(
            &format!("\"schema_version\": {PACKET_CLAIM_PROFILE_SCHEMA_VERSION}"),
            "\"schema_version\": 99",
        );
        let registry = load_parser_fixture(&raw, KNOWN);
        assert_eq!(
            registry.document_rejection(),
            Some(ClaimProfileDocumentRejection::SchemaVersionMismatch)
        );
        assert!(registry.profiles().is_empty());
    }

    #[test]
    fn data_cannot_raise_the_compiled_ratchet_ceiling() {
        let registry = load_claim_profile_registry(
            &document(
                PACKET_CLAIM_PROFILE_PENDING_MIGRATION_RATCHET + 1,
                &[pending_row("sql-schema")],
            ),
            KNOWN,
        );
        assert_eq!(
            registry.document_rejection(),
            Some(ClaimProfileDocumentRejection::RatchetAboveCeiling)
        );
        assert!(registry.profiles().is_empty());
    }

    #[test]
    fn more_pending_profiles_than_the_declared_ratchet_serves_nothing() {
        let registry = load_claim_profile_registry(
            &document(
                0,
                &[pending_row("sql-schema"), pending_row("shell-version-use")],
            ),
            KNOWN,
        );
        assert_eq!(
            registry.document_rejection(),
            Some(ClaimProfileDocumentRejection::PendingAboveRatchet)
        );
        assert!(registry.profiles().is_empty());
    }

    #[test]
    fn every_row_rejection_drops_exactly_that_row_with_its_typed_code() {
        let good = pending_row("sql-schema");
        let cases: Vec<(String, ClaimProfileRowRejection)> = vec![
            (
                pending_row("no-such-profile"),
                ClaimProfileRowRejection::UnknownProfileId,
            ),
            (
                pending_row("shell-version-use").replace("pending_migration", "invented_status"),
                ClaimProfileRowRejection::UnknownStatus,
            ),
            (
                pending_row("shell-version-use")
                    .replace("\"owner_issue\": 1573", "\"owner_issue\": 0"),
                ClaimProfileRowRejection::MissingOwnerIssue,
            ),
            (
                contracted_row_without_contract("shell-version-use"),
                ClaimProfileRowRejection::MissingContract,
            ),
            (
                contracted_row("shell-version-use")
                    .replace("\"contracted\"", "\"pending_migration\""),
                ClaimProfileRowRejection::ContractOnPendingProfile,
            ),
            (
                contracted_row("shell-version-use").replace(
                    "fixture triple measures fire on two families and silence on the helper",
                    "looks fine",
                ),
                ClaimProfileRowRejection::MissingContractEvidence,
            ),
            (
                contracted_row("shell-version-use").replace("\"product\"", "\"internal\""),
                ClaimProfileRowRejection::UnknownScope,
            ),
            (
                contracted_row("shell-version-use")
                    .replace("\"allows_lexical_source\"", "\"allows_anything\""),
                ClaimProfileRowRejection::UnknownEvidenceTier,
            ),
            (
                contracted_row("shell-version-use").replace("[\"dispatch\"]", "[\"teleport\"]"),
                ClaimProfileRowRejection::UnknownProofRole,
            ),
            (
                contracted_row("shell-version-use").replace("[\"dispatch\"]", "[]"),
                ClaimProfileRowRejection::Contract(
                    ClaimProfileContractViolation::NoAllowedProofRoles,
                ),
            ),
            (
                contracted_row("shell-version-use").replace(
                    "\"domain\": \"shell-version-use\"",
                    "\"domain\": \"sql-schema\"",
                ),
                ClaimProfileRowRejection::Contract(
                    ClaimProfileContractViolation::DomainIsNotProfileIdentity,
                ),
            ),
            (
                contracted_row("shell-version-use").replace(
                    "\"generalization_fixture_id\": \"shell-version-use-generalization\"",
                    "\"generalization_fixture_id\": \"\"",
                ),
                ClaimProfileRowRejection::Contract(
                    ClaimProfileContractViolation::MissingGeneralizationFixture,
                ),
            ),
            (
                contracted_row("shell-version-use").replace(
                    "\"generalization_fixture_id\": \"shell-version-use-generalization\"",
                    "\"generalization_fixture_id\": \"shell-version-use-positive\"",
                ),
                ClaimProfileRowRejection::Contract(
                    ClaimProfileContractViolation::GeneralizationFixtureNotDistinct,
                ),
            ),
        ];

        let mut codes = BTreeSet::new();
        for (row, expected) in cases {
            // The ratchet is slack here on purpose: a rejection that stops rejecting has to
            // surface as an accepted row, not as a ratchet overflow that hides which case broke.
            let registry = load_parser_fixture(&document(2, &[good.clone(), row]), KNOWN);
            assert_eq!(registry.document_rejection(), None);
            assert_eq!(
                registry.profiles().len(),
                1,
                "only the good row may survive: {registry:?}"
            );
            assert_eq!(
                registry.rejected(),
                &[RejectedClaimProfile { reason: expected }],
                "row rejection drifted"
            );
            codes.insert(expected.code());
        }
        assert_eq!(codes.len(), 13, "typed rejection codes must stay distinct");
    }

    #[test]
    fn a_field_this_binary_does_not_understand_refuses_the_whole_document() {
        // A future document that grew a field is not partially understandable: the row shape it
        // describes is not the shape this binary validates, so nothing from it may serve claims.
        let raw = pending_row("sql-schema").replace(
            "\"status\": \"pending_migration\"",
            "\"status\": \"pending_migration\", \"waiver\": true",
        );
        let registry = load_parser_fixture(&document(1, &[raw]), KNOWN);
        assert_eq!(
            registry.document_rejection(),
            Some(ClaimProfileDocumentRejection::Malformed)
        );
        assert!(registry.profiles().is_empty());
    }

    #[test]
    fn a_duplicated_identity_keeps_the_first_row_and_refuses_the_second() {
        let registry = load_parser_fixture(
            &document(
                1,
                &[pending_row("sql-schema"), contracted_row("sql-schema")],
            ),
            KNOWN,
        );
        assert_eq!(registry.profiles().len(), 1);
        assert_eq!(
            registry.profiles()[0].status,
            ClaimProfileStatus::PendingMigration
        );
        assert_eq!(
            registry.rejected(),
            &[RejectedClaimProfile {
                reason: ClaimProfileRowRejection::DuplicateProfileId,
            }]
        );
    }
}
