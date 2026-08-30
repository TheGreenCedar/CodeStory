//! Closed public DTO vocabulary for CodeStory v3 evidence projections.
//!
//! These types deliberately carry rendered evidence availability, not the
//! internal planning or proof state that produced it. Packet, context, and
//! search adapters serialize only these bounded projections.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

pub const PACKET_PROJECTION_V3_SCHEMA_VERSION: u16 = 3;

pub const IDENTITY_MAX_BYTES_V3: usize = 256;
pub const PATH_MAX_BYTES_V3: usize = 4_096;
pub const SYMBOL_ID_MAX_BYTES_V3: usize = 1_024;
pub const SUMMARY_MAX_BYTES_V3: usize = 8_192;
pub const MESSAGE_MAX_BYTES_V3: usize = 4_096;
pub const EXCERPT_MAX_BYTES_V3: usize = 8_192;
pub const DIAGNOSTIC_CODE_MAX_BYTES_V3: usize = 128;
pub const EVIDENCE_ROWS_MAX_V3: usize = 256;
pub const GAP_ROWS_MAX_V3: usize = 256;
pub const REFERENCE_ROWS_MAX_V3: usize = 256;
pub const DIAGNOSTIC_ROWS_MAX_V3: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct BoundedTextV3<const MAX: usize>(String);

impl<const MAX: usize> BoundedTextV3<MAX> {
    pub fn new(value: impl Into<String>) -> Result<Self, BoundViolationV3> {
        let value = value.into();
        if value.len() > MAX {
            return Err(BoundViolationV3::TooManyBytes {
                maximum: MAX,
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for BoundedTextV3<MAX> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BoundedVecV3<T, const MAX: usize>(Vec<T>);

impl<T, const MAX: usize> BoundedVecV3<T, MAX> {
    pub fn new(value: Vec<T>) -> Result<Self, BoundViolationV3> {
        if value.len() > MAX {
            return Err(BoundViolationV3::TooManyItems {
                maximum: MAX,
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}

impl<'de, T, const MAX: usize> Deserialize<'de> for BoundedVecV3<T, MAX>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Vec::<T>::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundViolationV3 {
    TooManyBytes { maximum: usize, actual: usize },
    TooManyItems { maximum: usize, actual: usize },
}

impl fmt::Display for BoundViolationV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyBytes { maximum, actual } => {
                write!(formatter, "maximum {maximum} bytes, got {actual}")
            }
            Self::TooManyItems { maximum, actual } => {
                write!(formatter, "maximum {maximum} items, got {actual}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256DigestV3Dto(String);

impl Sha256DigestV3Dto {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidSha256DigestV3> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(InvalidSha256DigestV3);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256DigestV3Dto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSha256DigestV3;

impl fmt::Display for InvalidSha256DigestV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected a 64-character hexadecimal SHA-256 digest")
    }
}

pub type IdentityTextV3 = BoundedTextV3<IDENTITY_MAX_BYTES_V3>;
pub type PathTextV3 = BoundedTextV3<PATH_MAX_BYTES_V3>;
pub type SymbolIdTextV3 = BoundedTextV3<SYMBOL_ID_MAX_BYTES_V3>;
pub type SummaryTextV3 = BoundedTextV3<SUMMARY_MAX_BYTES_V3>;
pub type MessageTextV3 = BoundedTextV3<MESSAGE_MAX_BYTES_V3>;
pub type ExcerptTextV3 = BoundedTextV3<EXCERPT_MAX_BYTES_V3>;
pub type DiagnosticCodeTextV3 = BoundedTextV3<DIAGNOSTIC_CODE_MAX_BYTES_V3>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketRequestIdentityV3Dto {
    pub packet_id: IdentityTextV3,
    pub request_id: IdentityTextV3,
    pub question_sha256: Sha256DigestV3Dto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorePublicationIdentityV3Dto {
    pub project_id: IdentityTextV3,
    pub generation_id: IdentityTextV3,
    pub run_id: IdentityTextV3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalPublicationIdentityV3Dto {
    pub core_generation_id: IdentityTextV3,
    pub core_run_id: IdentityTextV3,
    pub retrieval_generation: IdentityTextV3,
    pub retrieval_input_sha256: Sha256DigestV3Dto,
    pub semantic_generation: IdentityTextV3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationIdentityV3Dto {
    pub core: CorePublicationIdentityV3Dto,
    pub retrieval: Option<RetrievalPublicationIdentityV3Dto>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceIdentityV3Dto {
    pub evidence_id: IdentityTextV3,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapIdentityV3Dto {
    pub gap_id: IdentityTextV3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAvailabilityV3Dto {
    Available,
    ContinuationAvailable,
    NoUsefulEvidence,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalStateV3Dto {
    Full,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalStateDescriptorV3Dto {
    pub state: RetrievalStateV3Dto,
    pub generation_id: Option<IdentityTextV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKindV3Dto {
    ExactSource,
    StructuralSource,
    GraphRelation,
    RetrievalExcerpt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKindV3Dto {
    EvidenceMissing,
    RetrievalUnavailable,
    SourceUnavailable,
    ContinuationRequired,
    OutputBudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketEvidenceRowV3Dto {
    pub identity: EvidenceIdentityV3Dto,
    pub kind: EvidenceKindV3Dto,
    pub path: Option<PathTextV3>,
    pub symbol_id: Option<SymbolIdTextV3>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub summary: Option<SummaryTextV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionGapRowV3Dto {
    pub identity: GapIdentityV3Dto,
    pub kind: GapKindV3Dto,
    pub message: Option<MessageTextV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContinuationStateV3Dto {
    pub continuation_id: IdentityTextV3,
    pub remaining_rounds: u16,
    pub gap_ids: BoundedVecV3<GapIdentityV3Dto, REFERENCE_ROWS_MAX_V3>,
}

impl ContinuationStateV3Dto {
    pub fn new(
        continuation_id: IdentityTextV3,
        remaining_rounds: u16,
        gap_ids: BoundedVecV3<GapIdentityV3Dto, REFERENCE_ROWS_MAX_V3>,
    ) -> Result<Self, ContinuationStateViolationV3> {
        let value = Self {
            continuation_id,
            remaining_rounds,
            gap_ids,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), ContinuationStateViolationV3> {
        if self.remaining_rounds == 0 {
            return Err(ContinuationStateViolationV3::ZeroRemainingRounds);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationStateViolationV3 {
    ZeroRemainingRounds,
}

impl fmt::Display for ContinuationStateViolationV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroRemainingRounds => {
                formatter.write_str("continuation remaining_rounds must be positive")
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinuationStateV3DtoWire {
    continuation_id: IdentityTextV3,
    remaining_rounds: u16,
    gap_ids: BoundedVecV3<GapIdentityV3Dto, REFERENCE_ROWS_MAX_V3>,
}

impl<'de> Deserialize<'de> for ContinuationStateV3Dto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ContinuationStateV3DtoWire::deserialize(deserializer)?;
        Self::new(wire.continuation_id, wire.remaining_rounds, wire.gap_ids)
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticReferenceV3Dto {
    pub artifact_id: IdentityTextV3,
    pub sha256: Sha256DigestV3Dto,
    pub byte_length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_expiry_epoch_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiagnosticsCapabilityV3Dto {
    Unavailable,
    Available { reference: DiagnosticReferenceV3Dto },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PacketProjectionV3Dto {
    Complete {
        schema_version: u16,
        identity: PacketRequestIdentityV3Dto,
        publication: PublicationIdentityV3Dto,
        status: EvidenceAvailabilityV3Dto,
        retrieval: RetrievalStateDescriptorV3Dto,
        evidence: BoundedVecV3<PacketEvidenceRowV3Dto, EVIDENCE_ROWS_MAX_V3>,
        gaps: BoundedVecV3<ProjectionGapRowV3Dto, GAP_ROWS_MAX_V3>,
        continuation: Option<ContinuationStateV3Dto>,
        diagnostics: DiagnosticsCapabilityV3Dto,
    },
    BudgetExceeded {
        schema_version: u16,
        identity: PacketRequestIdentityV3Dto,
        publication: PublicationIdentityV3Dto,
        status: EvidenceAvailabilityV3Dto,
        retrieval: RetrievalStateDescriptorV3Dto,
        diagnostics: DiagnosticsCapabilityV3Dto,
        gaps: BoundedVecV3<ProjectionGapRowV3Dto, GAP_ROWS_MAX_V3>,
        maximum_bytes: u64,
        required_complete_bytes: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProjectionKindV3Dto {
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextTargetV3Dto {
    pub path: Option<PathTextV3>,
    pub symbol_id: Option<SymbolIdTextV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextEvidenceRowV3Dto {
    pub identity: EvidenceIdentityV3Dto,
    pub path: PathTextV3,
    pub symbol_id: Option<SymbolIdTextV3>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub excerpt: Option<ExcerptTextV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProjectionV3Dto {
    pub kind: ContextProjectionKindV3Dto,
    pub schema_version: u16,
    pub identity: PacketRequestIdentityV3Dto,
    pub publication: PublicationIdentityV3Dto,
    pub status: EvidenceAvailabilityV3Dto,
    pub target: ContextTargetV3Dto,
    pub evidence: BoundedVecV3<ContextEvidenceRowV3Dto, EVIDENCE_ROWS_MAX_V3>,
    pub gaps: BoundedVecV3<ProjectionGapRowV3Dto, GAP_ROWS_MAX_V3>,
    pub continuation: Option<ContinuationStateV3Dto>,
    pub diagnostics: DiagnosticsCapabilityV3Dto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProjectionKindV3Dto {
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchEvidenceRowV3Dto {
    pub identity: EvidenceIdentityV3Dto,
    pub path: PathTextV3,
    pub symbol_id: Option<SymbolIdTextV3>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub excerpt: Option<ExcerptTextV3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchProjectionV3Dto {
    pub kind: SearchProjectionKindV3Dto,
    pub schema_version: u16,
    pub identity: PacketRequestIdentityV3Dto,
    pub publication: PublicationIdentityV3Dto,
    pub status: EvidenceAvailabilityV3Dto,
    pub evidence: BoundedVecV3<SearchEvidenceRowV3Dto, EVIDENCE_ROWS_MAX_V3>,
    pub gaps: BoundedVecV3<ProjectionGapRowV3Dto, GAP_ROWS_MAX_V3>,
    pub continuation: Option<ContinuationStateV3Dto>,
    pub retrieval: RetrievalStateDescriptorV3Dto,
    pub diagnostics: DiagnosticsCapabilityV3Dto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticArtifactKindV3Dto {
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategoryV3Dto {
    Retrieval,
    Freshness,
    Coverage,
    Budget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRowV3Dto {
    pub diagnostic_id: IdentityTextV3,
    pub category: DiagnosticCategoryV3Dto,
    pub code: DiagnosticCodeTextV3,
    pub evidence_ids: BoundedVecV3<EvidenceIdentityV3Dto, REFERENCE_ROWS_MAX_V3>,
    pub gap_ids: BoundedVecV3<GapIdentityV3Dto, REFERENCE_ROWS_MAX_V3>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticArtifactV3Dto {
    pub kind: DiagnosticArtifactKindV3Dto,
    pub schema_version: u16,
    pub artifact_id: IdentityTextV3,
    pub packet_id: IdentityTextV3,
    pub publication: PublicationIdentityV3Dto,
    pub rows: BoundedVecV3<DiagnosticRowV3Dto, DIAGNOSTIC_ROWS_MAX_V3>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn text<const MAX: usize>(value: &str) -> BoundedTextV3<MAX> {
        BoundedTextV3::new(value).expect("bounded fixture text")
    }

    fn list<T, const MAX: usize>(values: Vec<T>) -> BoundedVecV3<T, MAX> {
        BoundedVecV3::new(values).expect("bounded fixture list")
    }

    fn identity() -> PacketRequestIdentityV3Dto {
        PacketRequestIdentityV3Dto {
            packet_id: text("packet-1"),
            request_id: text("request-1"),
            question_sha256: Sha256DigestV3Dto::new("a".repeat(64)).expect("question digest"),
        }
    }

    fn publication() -> PublicationIdentityV3Dto {
        PublicationIdentityV3Dto {
            core: CorePublicationIdentityV3Dto {
                project_id: text("project-1"),
                generation_id: text("core-generation-1"),
                run_id: text("core-run-1"),
            },
            retrieval: Some(RetrievalPublicationIdentityV3Dto {
                core_generation_id: text("core-generation-1"),
                core_run_id: text("core-run-1"),
                retrieval_generation: text("retrieval-generation-1"),
                retrieval_input_sha256: Sha256DigestV3Dto::new("b".repeat(64))
                    .expect("retrieval digest"),
                semantic_generation: text("semantic-generation-1"),
            }),
        }
    }

    fn evidence_identity(value: &str) -> EvidenceIdentityV3Dto {
        EvidenceIdentityV3Dto {
            evidence_id: text(value),
        }
    }

    fn gap_identity(value: &str) -> GapIdentityV3Dto {
        GapIdentityV3Dto {
            gap_id: text(value),
        }
    }

    fn diagnostic_reference() -> DiagnosticReferenceV3Dto {
        DiagnosticReferenceV3Dto {
            artifact_id: text("diagnostic-1"),
            sha256: Sha256DigestV3Dto::new("c".repeat(64)).expect("artifact digest"),
            byte_length: 512,
            wall_expiry_epoch_ms: None,
        }
    }

    fn diagnostics() -> DiagnosticsCapabilityV3Dto {
        DiagnosticsCapabilityV3Dto::Available {
            reference: diagnostic_reference(),
        }
    }

    fn packet_evidence(kind: EvidenceKindV3Dto) -> PacketEvidenceRowV3Dto {
        PacketEvidenceRowV3Dto {
            identity: evidence_identity("evidence-1"),
            kind,
            path: Some(text("src/lib.rs")),
            symbol_id: Some(text("crate::entry")),
            start_line: Some(4),
            end_line: Some(9),
            summary: Some(text("The entry calls the runtime boundary.")),
        }
    }

    fn gap(kind: GapKindV3Dto) -> ProjectionGapRowV3Dto {
        ProjectionGapRowV3Dto {
            identity: gap_identity("gap-1"),
            kind,
            message: Some(text("Additional evidence is required.")),
        }
    }

    fn retrieval(state: RetrievalStateV3Dto) -> RetrievalStateDescriptorV3Dto {
        RetrievalStateDescriptorV3Dto {
            state,
            generation_id: Some(text("retrieval-generation-1")),
        }
    }

    fn continuation() -> ContinuationStateV3Dto {
        ContinuationStateV3Dto {
            continuation_id: text("continuation-1"),
            remaining_rounds: 1,
            gap_ids: list(vec![gap_identity("gap-1")]),
        }
    }

    fn assert_no_prohibited_keys(value: &Value) {
        const PROHIBITED: &[&str] = &[
            "supported",
            "proof_disposition",
            "claim_discharge",
            "eligible_for_sufficiency",
            "complete_query_negative",
            "plan",
            "obligation",
            "query_trace",
            "score",
            "ranking",
            "local_executable_path",
            "capability_token",
        ];
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    assert!(!PROHIBITED.contains(&key.as_str()), "prohibited key: {key}");
                    assert_no_prohibited_keys(value);
                }
            }
            Value::Array(values) => values.iter().for_each(assert_no_prohibited_keys),
            _ => {}
        }
    }

    #[test]
    fn packet_projection_v3_bounds_hold_at_construction_and_deserialization() {
        let exact_text = "x".repeat(IDENTITY_MAX_BYTES_V3);
        assert!(IdentityTextV3::new(exact_text.clone()).is_ok());
        assert!(IdentityTextV3::new(format!("{exact_text}x")).is_err());
        assert!(serde_json::from_value::<IdentityTextV3>(json!(format!("{exact_text}x"))).is_err());

        let exact_items = vec![gap_identity("gap"); REFERENCE_ROWS_MAX_V3];
        assert!(
            BoundedVecV3::<GapIdentityV3Dto, REFERENCE_ROWS_MAX_V3>::new(exact_items.clone())
                .is_ok()
        );
        let mut excess_items = exact_items;
        excess_items.push(gap_identity("excess"));
        assert!(
            BoundedVecV3::<GapIdentityV3Dto, REFERENCE_ROWS_MAX_V3>::new(excess_items.clone())
                .is_err()
        );
        assert!(
            serde_json::from_value::<BoundedVecV3<GapIdentityV3Dto, REFERENCE_ROWS_MAX_V3>>(
                serde_json::to_value(excess_items).expect("serialize excess fixture")
            )
            .is_err()
        );

        assert!(Sha256DigestV3Dto::new("d".repeat(64)).is_ok());
        assert!(Sha256DigestV3Dto::new("d".repeat(63)).is_err());
        assert!(Sha256DigestV3Dto::new(format!("{}z", "d".repeat(63))).is_err());
        assert!(serde_json::from_value::<Sha256DigestV3Dto>(json!("not-a-digest")).is_err());
    }

    #[test]
    fn packet_projection_v3_rejects_zero_round_continuations_everywhere() {
        assert_eq!(
            ContinuationStateV3Dto::new(
                text("continuation-1"),
                0,
                list(vec![gap_identity("gap-1")])
            ),
            Err(ContinuationStateViolationV3::ZeroRemainingRounds)
        );

        let packet = PacketProjectionV3Dto::Complete {
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::ContinuationAvailable,
            retrieval: retrieval(RetrievalStateV3Dto::Full),
            evidence: list(Vec::new()),
            gaps: list(vec![gap(GapKindV3Dto::ContinuationRequired)]),
            continuation: Some(continuation()),
            diagnostics: diagnostics(),
        };
        let context = ContextProjectionV3Dto {
            kind: ContextProjectionKindV3Dto::Complete,
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::ContinuationAvailable,
            target: ContextTargetV3Dto {
                path: Some(text("src/lib.rs")),
                symbol_id: None,
            },
            evidence: list(Vec::new()),
            gaps: list(vec![gap(GapKindV3Dto::ContinuationRequired)]),
            continuation: Some(continuation()),
            diagnostics: diagnostics(),
        };
        let search = SearchProjectionV3Dto {
            kind: SearchProjectionKindV3Dto::Complete,
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::ContinuationAvailable,
            evidence: list(Vec::new()),
            gaps: list(vec![gap(GapKindV3Dto::ContinuationRequired)]),
            continuation: Some(continuation()),
            retrieval: retrieval(RetrievalStateV3Dto::Full),
            diagnostics: diagnostics(),
        };

        let mut packet_json = serde_json::to_value(packet).expect("serialize packet");
        packet_json["continuation"]["remaining_rounds"] = json!(0);
        assert!(serde_json::from_value::<PacketProjectionV3Dto>(packet_json).is_err());

        let mut context_json = serde_json::to_value(context).expect("serialize context");
        context_json["continuation"]["remaining_rounds"] = json!(0);
        assert!(serde_json::from_value::<ContextProjectionV3Dto>(context_json).is_err());

        let mut search_json = serde_json::to_value(search).expect("serialize search");
        search_json["continuation"]["remaining_rounds"] = json!(0);
        assert!(serde_json::from_value::<SearchProjectionV3Dto>(search_json).is_err());
    }

    #[test]
    fn packet_projection_v3_maximal_fixtures_are_closed_and_authority_free() {
        let complete = PacketProjectionV3Dto::Complete {
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::ContinuationAvailable,
            retrieval: retrieval(RetrievalStateV3Dto::Full),
            evidence: list(vec![packet_evidence(EvidenceKindV3Dto::ExactSource)]),
            gaps: list(vec![gap(GapKindV3Dto::ContinuationRequired)]),
            continuation: Some(continuation()),
            diagnostics: diagnostics(),
        };
        let budget_exceeded = PacketProjectionV3Dto::BudgetExceeded {
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::Unavailable,
            retrieval: retrieval(RetrievalStateV3Dto::Degraded),
            diagnostics: diagnostics(),
            gaps: list(vec![gap(GapKindV3Dto::OutputBudgetExceeded)]),
            maximum_bytes: 16_384,
            required_complete_bytes: 16_385,
        };
        let context = ContextProjectionV3Dto {
            kind: ContextProjectionKindV3Dto::Complete,
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::Available,
            target: ContextTargetV3Dto {
                path: Some(text("src/lib.rs")),
                symbol_id: Some(text("crate::entry")),
            },
            evidence: list(vec![ContextEvidenceRowV3Dto {
                identity: evidence_identity("context-evidence-1"),
                path: text("src/lib.rs"),
                symbol_id: Some(text("crate::entry")),
                start_line: Some(4),
                end_line: Some(9),
                excerpt: Some(text("pub fn entry() { runtime(); }")),
            }]),
            gaps: list(vec![gap(GapKindV3Dto::EvidenceMissing)]),
            continuation: Some(continuation()),
            diagnostics: diagnostics(),
        };
        let search = SearchProjectionV3Dto {
            kind: SearchProjectionKindV3Dto::Complete,
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::NoUsefulEvidence,
            evidence: list(vec![SearchEvidenceRowV3Dto {
                identity: evidence_identity("search-evidence-1"),
                path: text("src/lib.rs"),
                symbol_id: Some(text("crate::entry")),
                start_line: Some(4),
                end_line: Some(9),
                excerpt: Some(text("pub fn entry() { runtime(); }")),
            }]),
            gaps: list(vec![gap(GapKindV3Dto::RetrievalUnavailable)]),
            continuation: Some(continuation()),
            retrieval: retrieval(RetrievalStateV3Dto::Unavailable),
            diagnostics: diagnostics(),
        };
        let artifact = DiagnosticArtifactV3Dto {
            kind: DiagnosticArtifactKindV3Dto::Complete,
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            artifact_id: text("diagnostic-1"),
            packet_id: text("packet-1"),
            publication: publication(),
            rows: list(vec![DiagnosticRowV3Dto {
                diagnostic_id: text("diagnostic-row-1"),
                category: DiagnosticCategoryV3Dto::Coverage,
                code: text("coverage_gap"),
                evidence_ids: list(vec![evidence_identity("evidence-1")]),
                gap_ids: list(vec![gap_identity("gap-1")]),
            }]),
        };

        let maximal = json!({
            "packet_variants": [complete, budget_exceeded],
            "context": context,
            "search": search,
            "diagnostic_artifact": artifact,
            "diagnostics_capabilities": [DiagnosticsCapabilityV3Dto::Unavailable, diagnostics()],
            "availability_variants": [
                EvidenceAvailabilityV3Dto::Available,
                EvidenceAvailabilityV3Dto::ContinuationAvailable,
                EvidenceAvailabilityV3Dto::NoUsefulEvidence,
                EvidenceAvailabilityV3Dto::Unavailable,
            ],
            "retrieval_variants": [
                RetrievalStateV3Dto::Full,
                RetrievalStateV3Dto::Degraded,
                RetrievalStateV3Dto::Unavailable,
            ],
            "evidence_variants": [
                EvidenceKindV3Dto::ExactSource,
                EvidenceKindV3Dto::StructuralSource,
                EvidenceKindV3Dto::GraphRelation,
                EvidenceKindV3Dto::RetrievalExcerpt,
            ],
            "gap_variants": [
                GapKindV3Dto::EvidenceMissing,
                GapKindV3Dto::RetrievalUnavailable,
                GapKindV3Dto::SourceUnavailable,
                GapKindV3Dto::ContinuationRequired,
                GapKindV3Dto::OutputBudgetExceeded,
            ],
            "diagnostic_categories": [
                DiagnosticCategoryV3Dto::Retrieval,
                DiagnosticCategoryV3Dto::Freshness,
                DiagnosticCategoryV3Dto::Coverage,
                DiagnosticCategoryV3Dto::Budget,
            ],
        });
        assert_no_prohibited_keys(&maximal);

        let packet_json = serde_json::to_value(PacketProjectionV3Dto::Complete {
            schema_version: PACKET_PROJECTION_V3_SCHEMA_VERSION,
            identity: identity(),
            publication: publication(),
            status: EvidenceAvailabilityV3Dto::Available,
            retrieval: retrieval(RetrievalStateV3Dto::Full),
            evidence: list(vec![packet_evidence(EvidenceKindV3Dto::GraphRelation)]),
            gaps: list(Vec::new()),
            continuation: Some(continuation()),
            diagnostics: diagnostics(),
        })
        .expect("serialize packet root");
        assert_eq!(packet_json["kind"], "complete");
        assert!(packet_json.is_object());

        let mut unknown_root = packet_json.clone();
        unknown_root
            .as_object_mut()
            .expect("packet object")
            .insert("supported".to_owned(), json!(true));
        assert!(serde_json::from_value::<PacketProjectionV3Dto>(unknown_root).is_err());

        let mut unknown_row = packet_json;
        unknown_row["evidence"][0]
            .as_object_mut()
            .expect("evidence row")
            .insert("score".to_owned(), json!(0.99));
        assert!(serde_json::from_value::<PacketProjectionV3Dto>(unknown_row).is_err());
    }
}
