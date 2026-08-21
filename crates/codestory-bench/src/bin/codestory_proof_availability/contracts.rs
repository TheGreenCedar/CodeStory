use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const CORPUS_SCHEMA: &str = "codestory.proof-availability-corpus/v1";
pub const PATH_SCHEMA: &str = "codestory.proof-availability-path/v1";
pub const REPORT_SCHEMA: &str = "codestory.proof-availability-report/v1";
pub const THRESHOLDS_SCHEMA: &str = "codestory.proof-availability-thresholds/v1";
const SHA256: &str = "^[0-9a-f]{64}$";
const COMMIT: &str = "^[0-9a-f]{40}$";

mod u128_decimal {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(serde::de::Error::custom(
                "proof_availability_u128_decimal_invalid",
            ));
        }
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDocument {
    Corpus,
    Path,
    Report,
    Thresholds,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleSourceRangeV1 {
    pub path: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub file_byte_length: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleDeclarationV1 {
    pub symbol: String,
    pub range: OracleSourceRangeV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClauseAnchorV1 {
    pub clause_id: String,
    pub text: String,
    pub range: OracleSourceRangeV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallPathSpecV1 {
    pub start: String,
    pub targets: Vec<String>,
    pub expected_step_count: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleStepV1 {
    pub caller: OracleDeclarationV1,
    pub callsite: OracleSourceRangeV1,
    pub target: OracleDeclarationV1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NegativeMutationKindV1 {
    RemoveExpectedRelation,
    AddAmbiguousRelation,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NegativeMutationV1 {
    pub mutation_id: String,
    pub path_id: String,
    pub kind: NegativeMutationKindV1,
    pub step_index: u8,
    pub caller: String,
    pub target: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleAuditV1 {
    pub cohort_path_file: String,
    pub cohort_path_file_sha256: String,
    pub source_tree_sha256: String,
    pub source_area: String,
    pub curator: String,
    pub reviewer: String,
    pub review_date: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OraclePathV1 {
    pub schema: String,
    pub case_id: String,
    pub repository_id: String,
    pub language: String,
    pub source_text: String,
    pub clauses: Vec<ClauseAnchorV1>,
    pub spec: CallPathSpecV1,
    pub oracle_steps: Vec<OracleStepV1>,
    pub negative_mutations: Vec<NegativeMutationV1>,
    pub audit: OracleAuditV1,
}

impl OraclePathV1 {
    #[allow(dead_code)]
    pub fn from_json(value: Value) -> Result<Self> {
        let value: Self = serde_json::from_value(value)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != PATH_SCHEMA
            || empty(&self.case_id)
            || empty(&self.repository_id)
            || empty(&self.language)
            || empty(&self.source_text)
            || self.clauses.is_empty()
            || self.spec.expected_step_count == 0
            || self.spec.expected_step_count > 6
            || self.oracle_steps.len() != self.spec.expected_step_count as usize
            || self.negative_mutations.len() != 2
            || !unique(self.clauses.iter().map(|v| v.clause_id.as_str()))
            || !unique(
                self.negative_mutations
                    .iter()
                    .map(|v| v.mutation_id.as_str()),
            )
        {
            bail!("proof_availability_oracle_path_invalid")
        }
        if empty(&self.spec.start)
            || self.spec.targets.is_empty()
            || self.spec.targets.iter().any(|v| empty(v))
            || self.spec.targets.len() != usize::from(self.spec.expected_step_count)
        {
            bail!("proof_availability_oracle_spec_invalid")
        }
        for c in &self.clauses {
            if empty(&c.text) {
                bail!("proof_availability_clause_invalid")
            }
            range(&c.range)?;
        }
        for (index, step) in self.oracle_steps.iter().enumerate() {
            if empty(&step.caller.symbol) || empty(&step.target.symbol) {
                bail!("proof_availability_oracle_declaration_invalid")
            }
            range(&step.caller.range)?;
            range(&step.callsite)?;
            range(&step.target.range)?;
            if step.target.symbol != self.spec.targets[index]
                || (index == 0 && step.caller.symbol != self.spec.start)
                || (index > 0 && step.caller != self.oracle_steps[index - 1].target)
            {
                bail!("proof_availability_oracle_chain_invalid")
            }
        }
        for m in &self.negative_mutations {
            let Some(step) = self.oracle_steps.get(usize::from(m.step_index)) else {
                bail!("proof_availability_mutation_invalid")
            };
            if m.path_id != self.case_id
                || empty(&m.caller)
                || empty(&m.target)
                || m.caller != step.caller.symbol
                || m.target != step.target.symbol
            {
                bail!("proof_availability_mutation_invalid")
            }
        }
        if self.negative_mutations[0].kind == self.negative_mutations[1].kind {
            bail!("proof_availability_mutation_kinds_not_distinct")
        }
        if !hash(&self.audit.cohort_path_file_sha256)
            || !hash(&self.audit.source_tree_sha256)
            || empty(&self.audit.cohort_path_file)
            || empty(&self.audit.source_area)
            || empty(&self.audit.curator)
            || empty(&self.audit.reviewer)
            || self.audit.curator == self.audit.reviewer
            || !date(&self.audit.review_date)
        {
            bail!("proof_availability_oracle_audit_invalid")
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CohortV1 {
    pub repository_id: String,
    pub repository: String,
    pub commit: String,
    pub workspace: String,
    pub path_file: String,
    pub path_file_sha256: String,
    pub source_tree_sha256: String,
    pub path_count: u16,
    pub positive_step_count: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorpusV1 {
    pub schema: String,
    pub corpus_id: String,
    pub thresholds_sha256: String,
    pub methodology_sha256: String,
    pub curator: String,
    pub reviewer: String,
    pub review_date: String,
    pub cohorts: Vec<CohortV1>,
    pub paths: Vec<OraclePathV1>,
    pub positive_request_count: u16,
    pub positive_step_count: u16,
    pub negative_request_count: u16,
}
impl CorpusV1 {
    pub fn from_json(value: Value) -> Result<Self> {
        let value: Self = serde_json::from_value(value)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != CORPUS_SCHEMA
            || empty(&self.corpus_id)
            || !hash(&self.thresholds_sha256)
            || !hash(&self.methodology_sha256)
            || empty(&self.curator)
            || empty(&self.reviewer)
            || self.curator == self.reviewer
            || !date(&self.review_date)
            || self.cohorts.len() != 4
            || self.paths.len() != 120
            || self.positive_request_count != 120
            || self.positive_step_count != 312
            || self.negative_request_count != 240
            || !unique(self.cohorts.iter().map(|v| v.repository_id.as_str()))
            || !unique(self.paths.iter().map(|v| v.case_id.as_str()))
        {
            bail!("proof_availability_corpus_invalid")
        }
        for c in &self.cohorts {
            if empty(&c.repository_id)
                || empty(&c.repository)
                || !commit(&c.commit)
                || empty(&c.workspace)
                || empty(&c.path_file)
                || !hash(&c.path_file_sha256)
                || !hash(&c.source_tree_sha256)
                || c.path_count != 30
                || c.positive_step_count != 78
            {
                bail!("proof_availability_cohort_invalid")
            }
            let cohort_paths = self
                .paths
                .iter()
                .filter(|path| path.repository_id == c.repository_id)
                .collect::<Vec<_>>();
            if cohort_paths.len() != usize::from(c.path_count)
                || cohort_paths
                    .iter()
                    .map(|path| usize::from(path.spec.expected_step_count))
                    .sum::<usize>()
                    != usize::from(c.positive_step_count)
            {
                bail!("proof_availability_cohort_actual_totals_invalid")
            }
        }
        if self
            .paths
            .iter()
            .map(|path| usize::from(path.spec.expected_step_count))
            .sum::<usize>()
            != usize::from(self.positive_step_count)
            || self
                .paths
                .iter()
                .map(|path| path.negative_mutations.len())
                .sum::<usize>()
                != usize::from(self.negative_request_count)
        {
            bail!("proof_availability_corpus_actual_totals_invalid")
        }
        for p in &self.paths {
            p.validate()?;
            let c = self
                .cohorts
                .iter()
                .find(|c| c.repository_id == p.repository_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_path_cohort_missing"))?;
            if p.audit.cohort_path_file != c.path_file
                || p.audit.cohort_path_file_sha256 != c.path_file_sha256
                || p.audit.source_tree_sha256 != c.source_tree_sha256
                || p.audit.curator != self.curator
                || p.audit.reviewer != self.reviewer
            {
                bail!("proof_availability_path_freeze_mismatch")
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HardGatesV1 {
    pub maximum_false_contract_proven: u16,
    pub require_exact_receipt_matches: bool,
    pub maximum_certified_absence: u16,
    pub require_complete_failure_funnel: bool,
    pub require_complete_provenance: bool,
    pub maximum_invalid_results: u16,
    pub maximum_over_cap_results: u16,
    pub maximum_transport_errors: u16,
    pub maximum_proof_bytes: u64,
    pub require_each_cohort: bool,
    pub require_product_disposition_match: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleThresholdsV1 {
    pub minimum_full_proofs: u16,
    pub minimum_full_proofs_per_cohort: u16,
    pub minimum_full_proof_wilson_lower_milli: u16,
    pub minimum_cohort_wilson_lower_milli: u16,
    pub minimum_positive_step_recall_milli: u16,
    pub minimum_full_or_useful_partial_milli: u16,
    pub minimum_actionable_exact_gap_milli: u16,
    pub maximum_unknown_p95_ms: u64,
    pub maximum_transport_p95_ms: u64,
    pub maximum_complete_response_p95_bytes: u64,
    pub maximum_unknown_response_p95_bytes: u64,
    pub maximum_response_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThresholdsV1 {
    pub schema: String,
    pub thresholds_id: String,
    pub corpus_sha256: String,
    pub methodology_sha256: String,
    pub wilson_z: f64,
    pub expected_cohort_count: u8,
    pub expected_positive_requests: u16,
    pub expected_positive_steps: u16,
    pub expected_negative_requests: u16,
    pub hard_gates: HardGatesV1,
    pub automatic: RoleThresholdsV1,
    pub stable_explicit: RoleThresholdsV1,
    pub experimental: RoleThresholdsV1,
}
impl ThresholdsV1 {
    pub fn from_json(value: Value) -> Result<Self> {
        let value: Self = serde_json::from_value(value)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != THRESHOLDS_SCHEMA
            || empty(&self.thresholds_id)
            || !hash(&self.corpus_sha256)
            || !hash(&self.methodology_sha256)
            || (self.wilson_z - 1.959963984540054).abs() > f64::EPSILON
            || self.expected_cohort_count != 4
            || self.expected_positive_requests != 120
            || self.expected_positive_steps != 312
            || self.expected_negative_requests != 240
            || self.hard_gates.maximum_false_contract_proven != 0
            || !self.hard_gates.require_exact_receipt_matches
            || self.hard_gates.maximum_certified_absence != 0
            || !self.hard_gates.require_complete_failure_funnel
            || !self.hard_gates.require_complete_provenance
            || self.hard_gates.maximum_invalid_results != 0
            || self.hard_gates.maximum_over_cap_results != 0
            || self.hard_gates.maximum_transport_errors != 0
            || self.hard_gates.maximum_proof_bytes != 65536
            || !self.hard_gates.require_each_cohort
            || !self.hard_gates.require_product_disposition_match
        {
            bail!("proof_availability_thresholds_invalid")
        }
        for role in [&self.automatic, &self.stable_explicit, &self.experimental] {
            if role.minimum_full_proofs > 120
                || role.minimum_full_proofs_per_cohort > 30
                || [
                    role.minimum_full_proof_wilson_lower_milli,
                    role.minimum_cohort_wilson_lower_milli,
                    role.minimum_positive_step_recall_milli,
                    role.minimum_full_or_useful_partial_milli,
                    role.minimum_actionable_exact_gap_milli,
                ]
                .iter()
                .any(|v| *v > 1000)
                || role.maximum_response_bytes != 65536
                || role.maximum_complete_response_p95_bytes > 65536
                || role.maximum_unknown_response_p95_bytes > 65536
            {
                bail!("proof_availability_role_threshold_invalid")
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectorFailureV1 {
    Missing,
    Ambiguous,
    NonCallable,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum UnavailableReasonV1 {
    ValidatedContractHashMismatch,
    PublicationPinMismatch,
    SourceNotBoundToPublication,
    ProofFactsUnavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectorGateOutcomeV1 {
    Resolved { node_id: u64 },
    Failed { reason: SelectorFailureV1 },
    Unavailable { reason: UnavailableReasonV1 },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectorQualificationTraceV1 {
    pub selector_index: u8,
    pub outcome: SelectorGateOutcomeV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RawAdmissionFailureV1 {
    WrongKind,
    CertaintyAbsent,
    CertaintyProbable,
    CertaintyUncertain,
    WrongEffectiveSource,
    WrongEffectiveTarget,
    MissingExactResolvedTarget,
    CandidateAlternativesRetained,
    MissingFileNode,
    MissingLine,
    InvalidOrLegacyCallsiteIdentity,
    CallsiteFileMismatch,
    CallsiteLineMismatch,
    CallsiteRawTargetMismatch,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ContainmentFailureV1 {
    EdgeSourceFileMismatch,
    Missing,
    Ambiguous,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceBindingFailureV1 {
    FileIncomplete,
    StoredHashAbsent,
    WorkingTreeReadFailed,
    WorkingTreeHashMismatch,
    InvalidUtf8,
    LineMissing,
    LineOverLimit,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FinalizationFailureV1 {
    ReceiptIntegration,
    ReceiptBudget,
    ProjectionBudget,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateGateV1 {
    RawAdmission,
    Containment,
    SourceBinding,
    Line,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateFailureV1 {
    RawAdmission { reason: RawAdmissionFailureV1 },
    Containment { reason: ContainmentFailureV1 },
    SourceBinding { reason: SourceBindingFailureV1 },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CandidateFailureHistogramV1 {
    pub reason: CandidateFailureV1,
    pub edge_ids: Vec<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepQualificationOutcomeV1 {
    Admitted {
        edge_ids: Vec<u64>,
    },
    FirstZeroSurvivor {
        gate: CandidateGateV1,
        histogram: Vec<CandidateFailureHistogramV1>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepQualificationTraceV1 {
    pub step_index: u8,
    pub candidate_edge_ids: Vec<u64>,
    pub outcome: StepQualificationOutcomeV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FinalizationTraceV1 {
    NotRun,
    Complete { projection_bytes: u64 },
    Failed { failure: FinalizationFailureV1 },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProofQualificationTraceV1 {
    pub selectors: Vec<SelectorQualificationTraceV1>,
    pub selector_early_return: bool,
    pub steps: Vec<StepQualificationTraceV1>,
    pub finalization: FinalizationTraceV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceV1 {
    pub source_commit: String,
    pub source_tree: String,
    pub binary_sha256: String,
    pub corpus_sha256: String,
    pub thresholds_sha256: String,
    pub results_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentReportV1 {
    pub environment_id: String,
    pub os: String,
    pub architecture: String,
    pub rust_host: String,
    pub binary_sha256: String,
    pub core_generation: u64,
    pub core_run_id: String,
    pub database_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InventoryReportV1 {
    pub repository_id: String,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub stored_call_rows: u128,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub effective_endpoint_rows: u128,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub exact_resolved_rows: u128,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub admitted_rows: u128,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub unresolved_placeholder_rows: u128,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailLengthCountsV1 {
    pub length: u8,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub effective_endpoint: u128,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub exact_resolved: u128,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub strictly_admitted: u128,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailReportV1 {
    pub repository_id: String,
    pub lengths: Vec<TrailLengthCountsV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProductDispositionKindV1 {
    ContractProven,
    Unknown,
    CertifiedAbsence,
    Invalid,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductDispositionV1 {
    pub kind: ProductDispositionKindV1,
    pub gaps: Vec<TypedGapV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TypedGapV1 {
    SelectorMissing,
    SelectorAmbiguous,
    RelationMissing,
    Recursion,
    SourceBinding,
    ProjectionBudget,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StageDurationsV1 {
    pub validation: u64,
    pub operation: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransportMeasurementV1 {
    pub revision: McpRevisionV1,
    pub actual_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransportMeasurementSetV1 {
    pub measurements: Vec<TransportMeasurementV1>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum McpRevisionV1 {
    #[serde(rename = "2024-11-05")]
    V2024_11_05,
    #[serde(rename = "2025-03-26")]
    V2025_03_26,
    #[serde(rename = "2025-06-18")]
    V2025_06_18,
    #[serde(rename = "2025-11-25")]
    V2025_11_25,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransportErrorV1 {
    Serialization {
        message: String,
    },
    InvalidProjection {
        projection: String,
    },
    OutputSchemaViolation {},
    ResultExceedsBudget {
        maximum_bytes: u64,
        actual_bytes: u64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransportEvidenceV1 {
    Measurements {
        measurements: TransportMeasurementSetV1,
    },
    Error {
        error: TransportErrorV1,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NegativeMutationResultV1 {
    pub mutation_id: String,
    pub contract_proven: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseReportV1 {
    pub case_id: String,
    pub repository_id: String,
    pub product_disposition: ProductDispositionV1,
    pub authoritative_receipt_count: u64,
    pub oracle_receipts_exact: bool,
    pub proven_step_precision_milli: u16,
    pub proven_step_recall_milli: u16,
    pub proven_prefix_length: u8,
    pub actionable_exact_gap: Option<TypedGapV1>,
    pub diagnostic_candidate_count: u64,
    pub authoritative_receipt_evidence_count: u64,
    pub warm_end_to_end_ms: u64,
    pub stage_durations_ms: StageDurationsV1,
    pub attempted_step_count: u8,
    pub complete_projection_bytes: u64,
    pub transport: TransportEvidenceV1,
    pub negative_mutations: Vec<NegativeMutationResultV1>,
    pub proof_trace: ProofQualificationTraceV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailureBucketV1 {
    pub outcome: FunnelOutcomeV1,
    #[serde(with = "u128_decimal")]
    #[schemars(with = "String")]
    pub count: u128,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FunnelOutcomeV1 {
    Admitted,
    SelectorEarlyReturn {
        outcome: SelectorGateOutcomeV1,
    },
    FirstZeroSurvivor {
        gate: CandidateGateV1,
        histogram: Vec<CandidateFailureHistogramV1>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailureFunnelReportV1 {
    pub attempted_positive_steps: u16,
    pub classified_positive_steps: u16,
    pub unclassified_positive_steps: u16,
    pub buckets: Vec<FailureBucketV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationOutcomeV1 {
    PublicExactVerifier,
    ExperimentalManualVerifier,
    KeepProofDark,
    DelayFullV3Cut,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum QualificationGateKindV1 {
    FalseContractProven,
    ReceiptMismatch,
    CertifiedAbsence,
    FailureFunnel,
    Provenance,
    ResponseSize,
    CohortFailure,
    ProductDispositionMismatch,
    AutomaticThreshold,
    StableThreshold,
    ExperimentalUsefulness,
    IntegrationDependency,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceDependencyKindV1 {
    V3PacketRequiresProof,
    TransportCannotRepresentKeepDark,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum IntegrationDependencyTestKindV1 {
    PacketV3RequiresProof,
    TransportCannotRepresentKeepDark,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum IntegrationDependencyTestStatusV1 {
    Passed,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrationDependencyTestV1 {
    pub test_id: String,
    pub kind: IntegrationDependencyTestKindV1,
    pub status: IntegrationDependencyTestStatusV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceDependencyEvidenceV1 {
    pub source_path: String,
    pub source_range: OracleSourceRangeV1,
    pub source_sha256: String,
    pub dependency: SourceDependencyKindV1,
    pub passing_test: IntegrationDependencyTestV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GateFailureDetailV1 {
    Count {
        #[serde(with = "u128_decimal")]
        #[schemars(with = "String")]
        observed: u128,
        #[serde(with = "u128_decimal")]
        #[schemars(with = "String")]
        required: u128,
    },
    Cohort {
        repository_id: String,
        #[serde(with = "u128_decimal")]
        #[schemars(with = "String")]
        observed: u128,
        #[serde(with = "u128_decimal")]
        #[schemars(with = "String")]
        required: u128,
    },
    Transport {
        evidence: TransportEvidenceV1,
    },
    SourceDependency {
        evidence: SourceDependencyEvidenceV1,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailedGateV1 {
    pub gate_id: String,
    pub kind: QualificationGateKindV1,
    pub detail: GateFailureDetailV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivationDecisionV1 {
    pub outcome: ActivationOutcomeV1,
    pub failed_gates: Vec<FailedGateV1>,
    pub automatic_thresholds_met: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QualificationSummaryV1 {
    pub schema: String,
    pub qualification_id: String,
    pub provenance: ProvenanceV1,
    pub environment: EnvironmentReportV1,
    pub inventory: Vec<InventoryReportV1>,
    pub trails: Vec<TrailReportV1>,
    pub cases: Vec<CaseReportV1>,
    pub failure_funnel: FailureFunnelReportV1,
    pub decision: ActivationDecisionV1,
}
impl QualificationSummaryV1 {
    pub fn from_json(value: Value) -> Result<Self> {
        let value: Self = serde_json::from_value(value)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != REPORT_SCHEMA
            || empty(&self.qualification_id)
            || !commit(&self.provenance.source_commit)
            || !commit(&self.provenance.source_tree)
            || ![
                &self.provenance.binary_sha256,
                &self.provenance.corpus_sha256,
                &self.provenance.thresholds_sha256,
                &self.provenance.results_sha256,
            ]
            .iter()
            .all(|v| hash(v))
            || !hash(&self.environment.binary_sha256)
            || !hash(&self.environment.database_sha256)
            || empty(&self.environment.core_run_id)
            || self.failure_funnel.attempted_positive_steps != 312
            || self.failure_funnel.classified_positive_steps > 312
            || self.failure_funnel.unclassified_positive_steps > 312
            || u32::from(self.failure_funnel.classified_positive_steps)
                + u32::from(self.failure_funnel.unclassified_positive_steps)
                != 312
            || self
                .failure_funnel
                .buckets
                .iter()
                .map(|bucket| bucket.count)
                .sum::<u128>()
                != u128::from(self.failure_funnel.classified_positive_steps)
            || !self
                .failure_funnel
                .buckets
                .iter()
                .all(|bucket| valid_funnel_outcome(&bucket.outcome))
        {
            bail!("proof_availability_summary_invalid")
        }
        for t in &self.trails {
            if t.lengths.len() != 6
                || !t.lengths.iter().enumerate().all(|(i, v)| {
                    v.length == i as u8 + 1
                        && v.strictly_admitted <= v.exact_resolved
                        && v.exact_resolved <= v.effective_endpoint
                })
            {
                bail!("proof_availability_trail_counts_invalid")
            }
        }
        for c in &self.cases {
            if c.negative_mutations.len() != 2
                || c.proven_step_precision_milli > 1000
                || c.proven_step_recall_milli > 1000
                || c.attempted_step_count > 6
                || c.proof_trace.steps.len() != usize::from(c.attempted_step_count)
                || !c
                    .proof_trace
                    .selectors
                    .iter()
                    .enumerate()
                    .all(|(index, selector)| selector.selector_index == index as u8)
                || !valid_selector_trace(&c.proof_trace)
                || !c
                    .proof_trace
                    .steps
                    .iter()
                    .enumerate()
                    .all(|(index, step)| step.step_index == index as u8 && valid_step_trace(step))
                || !valid_finalization(&c.proof_trace.finalization)
                || !valid_transport(&c.transport)
                || (c.proof_trace.selector_early_return
                    != matches!(c.proof_trace.finalization, FinalizationTraceV1::NotRun))
                || (c.proof_trace.selector_early_return && c.attempted_step_count != 0)
            {
                bail!("proof_availability_case_invalid")
            }
        }
        if !unique(
            self.decision
                .failed_gates
                .iter()
                .map(|g| g.gate_id.as_str()),
        ) {
            bail!("proof_availability_decision_invalid")
        }
        for gate in &self.decision.failed_gates {
            if !valid_gate_detail(&gate.kind, &gate.detail) {
                bail!("proof_availability_gate_detail_invalid")
            }
        }
        if matches!(self.decision.outcome, ActivationOutcomeV1::DelayFullV3Cut)
            && !self
                .decision
                .failed_gates
                .iter()
                .any(|gate| matches!(gate.detail, GateFailureDetailV1::SourceDependency { .. }))
        {
            bail!("proof_availability_delay_dependency_missing")
        }
        Ok(())
    }
}

fn valid_step_trace(trace: &StepQualificationTraceV1) -> bool {
    if !strictly_ascending(&trace.candidate_edge_ids) {
        return false;
    }
    match &trace.outcome {
        StepQualificationOutcomeV1::Admitted { edge_ids } => {
            !edge_ids.is_empty() && strictly_ascending(edge_ids)
        }
        StepQualificationOutcomeV1::FirstZeroSurvivor { gate, histogram } => {
            (histogram.is_empty()
                && matches!(gate, CandidateGateV1::RawAdmission)
                && trace.candidate_edge_ids.is_empty())
                || (!histogram.is_empty()
                    && histogram.iter().all(|bucket| {
                        !bucket.edge_ids.is_empty()
                            && strictly_ascending(&bucket.edge_ids)
                            && matches!(
                                (&gate, &bucket.reason),
                                (
                                    CandidateGateV1::RawAdmission,
                                    CandidateFailureV1::RawAdmission { .. },
                                ) | (
                                    CandidateGateV1::Containment,
                                    CandidateFailureV1::Containment { .. },
                                ) | (
                                    CandidateGateV1::SourceBinding,
                                    CandidateFailureV1::SourceBinding {
                                        reason: SourceBindingFailureV1::FileIncomplete
                                            | SourceBindingFailureV1::StoredHashAbsent
                                            | SourceBindingFailureV1::WorkingTreeReadFailed
                                            | SourceBindingFailureV1::WorkingTreeHashMismatch
                                            | SourceBindingFailureV1::InvalidUtf8,
                                    },
                                ) | (
                                    CandidateGateV1::Line,
                                    CandidateFailureV1::SourceBinding {
                                        reason: SourceBindingFailureV1::LineMissing
                                            | SourceBindingFailureV1::LineOverLimit,
                                    },
                                )
                            )
                    }))
        }
    }
}

fn valid_selector_trace(trace: &ProofQualificationTraceV1) -> bool {
    if trace.selector_early_return {
        matches!(
            trace.selectors.last().map(|selector| &selector.outcome),
            Some(SelectorGateOutcomeV1::Failed { .. } | SelectorGateOutcomeV1::Unavailable { .. })
        )
    } else {
        trace
            .selectors
            .iter()
            .all(|selector| matches!(selector.outcome, SelectorGateOutcomeV1::Resolved { .. }))
    }
}

fn strictly_ascending(values: &[u64]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_funnel_outcome(outcome: &FunnelOutcomeV1) -> bool {
    match outcome {
        FunnelOutcomeV1::Admitted => true,
        FunnelOutcomeV1::SelectorEarlyReturn { outcome } => {
            !matches!(outcome, SelectorGateOutcomeV1::Resolved { .. })
        }
        FunnelOutcomeV1::FirstZeroSurvivor { gate, histogram } => {
            valid_step_trace(&StepQualificationTraceV1 {
                step_index: 0,
                candidate_edge_ids: Vec::new(),
                outcome: StepQualificationOutcomeV1::FirstZeroSurvivor {
                    gate: gate.clone(),
                    histogram: histogram.clone(),
                },
            })
        }
    }
}

fn valid_transport(evidence: &TransportEvidenceV1) -> bool {
    match evidence {
        TransportEvidenceV1::Measurements { measurements } => {
            let revisions = [
                McpRevisionV1::V2024_11_05,
                McpRevisionV1::V2025_03_26,
                McpRevisionV1::V2025_06_18,
                McpRevisionV1::V2025_11_25,
            ];
            measurements.measurements.len() == revisions.len()
                && measurements
                    .measurements
                    .iter()
                    .zip(revisions)
                    .all(|(measurement, revision)| {
                        measurement.revision == revision && measurement.actual_bytes <= 65_536
                    })
        }
        TransportEvidenceV1::Error { error } => match error {
            TransportErrorV1::Serialization { message } => !empty(message),
            TransportErrorV1::InvalidProjection { projection } => !empty(projection),
            TransportErrorV1::OutputSchemaViolation {} => true,
            TransportErrorV1::ResultExceedsBudget {
                maximum_bytes,
                actual_bytes,
            } => *maximum_bytes == 65_536 && *actual_bytes > *maximum_bytes,
        },
    }
}

fn valid_finalization(finalization: &FinalizationTraceV1) -> bool {
    match finalization {
        FinalizationTraceV1::NotRun => true,
        FinalizationTraceV1::Complete { .. } => true,
        FinalizationTraceV1::Failed { .. } => true,
    }
}

fn valid_gate_detail(kind: &QualificationGateKindV1, detail: &GateFailureDetailV1) -> bool {
    match detail {
        GateFailureDetailV1::Count { .. } => true,
        GateFailureDetailV1::Cohort { repository_id, .. } => !empty(repository_id),
        GateFailureDetailV1::Transport { evidence } => valid_transport(evidence),
        GateFailureDetailV1::SourceDependency { evidence } => {
            matches!(kind, QualificationGateKindV1::IntegrationDependency)
                && !empty(&evidence.source_path)
                && hash(&evidence.source_sha256)
                && range(&evidence.source_range).is_ok()
                && evidence.source_path == evidence.source_range.path
                && evidence.source_sha256 == evidence.source_range.sha256
                && !empty(&evidence.passing_test.test_id)
                && matches!(
                    (&evidence.dependency, &evidence.passing_test.kind),
                    (
                        SourceDependencyKindV1::V3PacketRequiresProof,
                        IntegrationDependencyTestKindV1::PacketV3RequiresProof,
                    ) | (
                        SourceDependencyKindV1::TransportCannotRepresentKeepDark,
                        IntegrationDependencyTestKindV1::TransportCannotRepresentKeepDark,
                    )
                )
        }
    }
}

pub fn schema_json(document: SchemaDocument) -> Value {
    let (mut value, id) = match document {
        SchemaDocument::Corpus => (schema::<CorpusV1>(), CORPUS_SCHEMA),
        SchemaDocument::Path => (schema::<OraclePathV1>(), PATH_SCHEMA),
        SchemaDocument::Report => (schema::<QualificationSummaryV1>(), REPORT_SCHEMA),
        SchemaDocument::Thresholds => (schema::<ThresholdsV1>(), THRESHOLDS_SCHEMA),
    };
    let root = value.as_object_mut().unwrap();
    root.insert(
        "$schema".into(),
        Value::String("https://json-schema.org/draft/2020-12/schema".into()),
    );
    root.insert("$id".into(), Value::String(id.into()));
    semantic(&mut value, document);
    value
}
fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema")
}
fn semantic(schema: &mut Value, document: SchemaDocument) {
    let root = schema.as_object_mut().expect("schema object");
    let id = match document {
        SchemaDocument::Corpus => CORPUS_SCHEMA,
        SchemaDocument::Path => PATH_SCHEMA,
        SchemaDocument::Report => REPORT_SCHEMA,
        SchemaDocument::Thresholds => THRESHOLDS_SCHEMA,
    };
    if let Some(value) = root
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut("schema"))
        .and_then(Value::as_object_mut)
    {
        value.insert("const".into(), Value::String(id.into()));
    }
    if let Some(properties) = root.get_mut("properties").and_then(Value::as_object_mut) {
        for (name, property) in properties {
            let Some(property) = property.as_object_mut() else {
                continue;
            };
            if matches!(
                name.as_str(),
                "sha256"
                    | "binary_sha256"
                    | "corpus_sha256"
                    | "thresholds_sha256"
                    | "results_sha256"
                    | "methodology_sha256"
            ) {
                property.insert("pattern".into(), Value::String(SHA256.into()));
            }
            if matches!(name.as_str(), "commit" | "source_commit" | "source_tree") {
                property.insert("pattern".into(), Value::String(COMMIT.into()));
            }
        }
    }
    if let Some(definitions) = root.get_mut("$defs").and_then(Value::as_object_mut) {
        for definition in definitions.values_mut() {
            let Some(properties) = definition
                .get_mut("properties")
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            for (name, property) in properties {
                let Some(property) = property.as_object_mut() else {
                    continue;
                };
                if matches!(
                    name.as_str(),
                    "sha256"
                        | "binary_sha256"
                        | "corpus_sha256"
                        | "thresholds_sha256"
                        | "results_sha256"
                        | "methodology_sha256"
                        | "path_file_sha256"
                        | "cohort_path_file_sha256"
                        | "source_sha256"
                        | "source_tree_sha256"
                        | "database_sha256"
                ) {
                    property.insert("pattern".into(), Value::String(SHA256.into()));
                }
                if matches!(name.as_str(), "commit" | "source_commit" | "source_tree") {
                    property.insert("pattern".into(), Value::String(COMMIT.into()));
                }
                if matches!(
                    name.as_str(),
                    "stored_call_rows"
                        | "effective_endpoint_rows"
                        | "exact_resolved_rows"
                        | "admitted_rows"
                        | "unresolved_placeholder_rows"
                        | "effective_endpoint"
                        | "exact_resolved"
                        | "strictly_admitted"
                        | "count"
                        | "observed"
                        | "required"
                ) {
                    property.insert("pattern".into(), Value::String("^(0|[1-9][0-9]*)$".into()));
                }
            }
        }
    }
    if document == SchemaDocument::Path
        && let Some(value) = root
            .get_mut("properties")
            .and_then(Value::as_object_mut)
            .and_then(|properties| properties.get_mut("negative_mutations"))
            .and_then(Value::as_object_mut)
    {
        value.insert("minItems".into(), Value::from(2));
        value.insert("maxItems".into(), Value::from(2));
    }
    semantic_contract_bounds(schema, document);
    annotate_transport_bounds(schema);
}

fn semantic_contract_bounds(schema: &mut Value, document: SchemaDocument) {
    set_bounds(
        schema,
        Some("OracleSourceRangeV1"),
        "file_byte_length",
        Some(1),
        None,
    );
    match document {
        SchemaDocument::Corpus => {
            set_bounds(schema, None, "cohorts", Some(4), Some(4));
            set_bounds(schema, None, "paths", Some(120), Some(120));
            for field in [
                "positive_request_count",
                "positive_step_count",
                "negative_request_count",
            ] {
                let value = match field {
                    "positive_request_count" => 120,
                    "positive_step_count" => 312,
                    _ => 240,
                };
                set_const(schema, None, field, Value::from(value));
            }
            set_const(schema, Some("CohortV1"), "path_count", Value::from(30));
            set_const(
                schema,
                Some("CohortV1"),
                "positive_step_count",
                Value::from(78),
            );
            set_const(
                schema,
                Some("OraclePathV1"),
                "schema",
                Value::String(PATH_SCHEMA.into()),
            );
            set_bounds(
                schema,
                Some("OraclePathV1"),
                "negative_mutations",
                Some(2),
                Some(2),
            );
            set_bounds(
                schema,
                Some("CallPathSpecV1"),
                "expected_step_count",
                Some(1),
                Some(6),
            );
            set_bounds(schema, Some("CallPathSpecV1"), "targets", Some(1), Some(6));
            set_bounds(
                schema,
                Some("OraclePathV1"),
                "oracle_steps",
                Some(1),
                Some(6),
            );
            set_bounds(
                schema,
                Some("NegativeMutationV1"),
                "step_index",
                Some(0),
                Some(5),
            );
        }
        SchemaDocument::Path => {
            set_bounds(
                schema,
                Some("CallPathSpecV1"),
                "expected_step_count",
                Some(1),
                Some(6),
            );
            set_bounds(schema, Some("CallPathSpecV1"), "targets", Some(1), Some(6));
            set_bounds(schema, None, "oracle_steps", Some(1), Some(6));
            set_bounds(
                schema,
                Some("NegativeMutationV1"),
                "step_index",
                Some(0),
                Some(5),
            );
        }
        SchemaDocument::Thresholds => {
            for (field, value) in [
                ("expected_cohort_count", 4),
                ("expected_positive_requests", 120),
                ("expected_positive_steps", 312),
                ("expected_negative_requests", 240),
            ] {
                set_const(schema, None, field, Value::from(value));
            }
            set_const(
                schema,
                Some("HardGatesV1"),
                "maximum_proof_bytes",
                Value::from(65_536),
            );
            for field in [
                "minimum_full_proof_wilson_lower_milli",
                "minimum_cohort_wilson_lower_milli",
                "minimum_positive_step_recall_milli",
                "minimum_full_or_useful_partial_milli",
                "minimum_actionable_exact_gap_milli",
            ] {
                set_bounds(schema, Some("RoleThresholdsV1"), field, Some(0), Some(1000));
            }
            for field in [
                "maximum_complete_response_p95_bytes",
                "maximum_unknown_response_p95_bytes",
                "maximum_response_bytes",
            ] {
                set_bounds(
                    schema,
                    Some("RoleThresholdsV1"),
                    field,
                    Some(0),
                    Some(65_536),
                );
            }
        }
        SchemaDocument::Report => {
            set_bounds(schema, None, "inventory", Some(1), None);
            set_bounds(schema, None, "trails", Some(1), None);
            set_bounds(schema, None, "cases", Some(1), None);
            set_const(
                schema,
                Some("FailureFunnelReportV1"),
                "attempted_positive_steps",
                Value::from(312),
            );
            for field in ["classified_positive_steps", "unclassified_positive_steps"] {
                set_bounds(
                    schema,
                    Some("FailureFunnelReportV1"),
                    field,
                    Some(0),
                    Some(312),
                );
            }
            set_bounds(schema, Some("TrailReportV1"), "lengths", Some(6), Some(6));
            set_bounds(
                schema,
                Some("CaseReportV1"),
                "attempted_step_count",
                Some(0),
                Some(6),
            );
            for field in ["proven_step_precision_milli", "proven_step_recall_milli"] {
                set_bounds(schema, Some("CaseReportV1"), field, Some(0), Some(1000));
            }
            set_bounds(
                schema,
                Some("ProofQualificationTraceV1"),
                "steps",
                Some(0),
                Some(6),
            );
            set_bounds(
                schema,
                Some("TransportMeasurementV1"),
                "actual_bytes",
                Some(0),
                Some(65_536),
            );
        }
    }
}

fn set_const(schema: &mut Value, definition: Option<&str>, field: &str, constant: Value) {
    if let Some(property) = schema_property(schema, definition, field) {
        property.insert("const".into(), constant);
    }
}

fn set_bounds(
    schema: &mut Value,
    definition: Option<&str>,
    field: &str,
    minimum: Option<u64>,
    maximum: Option<u64>,
) {
    if let Some(property) = schema_property(schema, definition, field) {
        let array = property.get("type").and_then(Value::as_str) == Some("array");
        if let Some(minimum) = minimum {
            property.insert(
                if array { "minItems" } else { "minimum" }.into(),
                Value::from(minimum),
            );
        }
        if let Some(maximum) = maximum {
            property.insert(
                if array { "maxItems" } else { "maximum" }.into(),
                Value::from(maximum),
            );
        }
    }
}

fn schema_property<'a>(
    schema: &'a mut Value,
    definition: Option<&str>,
    field: &str,
) -> Option<&'a mut serde_json::Map<String, Value>> {
    let owner = match definition {
        Some(definition) => schema.get_mut("$defs")?.get_mut(definition)?,
        None => schema,
    };
    owner.get_mut("properties")?.get_mut(field)?.as_object_mut()
}

fn annotate_transport_bounds(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let kind = map
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get("kind"))
                .and_then(Value::as_object)
                .and_then(|kind| kind.get("const"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut)
                && kind.as_deref() == Some("result_exceeds_budget")
            {
                if let Some(maximum) = properties
                    .get_mut("maximum_bytes")
                    .and_then(Value::as_object_mut)
                {
                    maximum.insert("const".into(), Value::from(65_536));
                }
                if let Some(actual) = properties
                    .get_mut("actual_bytes")
                    .and_then(Value::as_object_mut)
                {
                    actual.insert("minimum".into(), Value::from(65_537));
                }
            }
            for value in map.values_mut() {
                annotate_transport_bounds(value);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(annotate_transport_bounds),
        _ => {}
    }
}
fn empty(value: &str) -> bool {
    value.trim().is_empty()
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
}
fn range(value: &OracleSourceRangeV1) -> Result<()> {
    if empty(&value.path)
        || value.start_byte >= value.end_byte
        || value.end_byte > value.file_byte_length
        || !hash(&value.sha256)
    {
        bail!("proof_availability_range_invalid")
    }
    Ok(())
}
fn unique<'a>(mut values: impl Iterator<Item = &'a str>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|v| !empty(v) && set.insert(v))
}
