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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDocument {
    Corpus,
    Path,
    Report,
    Thresholds,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleSourceRangeV1 {
    pub path: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleDeclarationV1 {
    pub symbol: String,
    pub range: OracleSourceRangeV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NegativeMutationKindV1 {
    RemoveExpectedRelation,
    AddAmbiguousRelation,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NegativeMutationV1 {
    pub mutation_id: String,
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
        {
            bail!("proof_availability_oracle_spec_invalid")
        }
        for c in &self.clauses {
            if empty(&c.text) {
                bail!("proof_availability_clause_invalid")
            }
            range(&c.range)?;
        }
        for step in &self.oracle_steps {
            if empty(&step.caller.symbol) || empty(&step.target.symbol) {
                bail!("proof_availability_oracle_declaration_invalid")
            }
            range(&step.caller.range)?;
            range(&step.callsite)?;
            range(&step.target.range)?;
        }
        for m in &self.negative_mutations {
            if empty(&m.caller) || empty(&m.target) || m.step_index >= self.spec.expected_step_count
            {
                bail!("proof_availability_mutation_invalid")
            }
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
            || self.positive_request_count != 120
            || self.positive_step_count != 312
            || self.negative_request_count != 240
            || !unique(self.cohorts.iter().map(|v| v.repository_id.as_str()))
            || !unique(self.paths.iter().map(|v| v.case_id.as_str()))
        {
            bail!("proof_availability_corpus_invalid")
        }
        let mut paths = 0u16;
        let mut steps = 0u16;
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
            paths += c.path_count;
            steps += c.positive_step_count;
        }
        if paths != self.positive_request_count || steps != self.positive_step_count {
            bail!("proof_availability_corpus_totals_invalid")
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
#[serde(rename_all = "snake_case")]
pub enum SelectorFailureV1 {
    Missing,
    Ambiguous,
    NonCallable,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
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
#[serde(rename_all = "snake_case")]
pub enum ContainmentFailureV1 {
    EdgeSourceFileMismatch,
    Missing,
    Ambiguous,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
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
#[serde(rename_all = "snake_case")]
pub enum FinalizationFailureV1 {
    ReceiptIntegration,
    ReceiptBudget,
    ProjectionBudget,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepFailureV1 {
    Admitted {
        edge_ids: Vec<u64>,
        histogram: Vec<FailureHistogramV1>,
        finalization: Option<FinalizationFailureV1>,
    },
    Selector {
        reason: SelectorFailureV1,
        edge_ids: Vec<u64>,
        histogram: Vec<FailureHistogramV1>,
        finalization: Option<FinalizationFailureV1>,
    },
    RawAdmission {
        reason: RawAdmissionFailureV1,
        edge_ids: Vec<u64>,
        histogram: Vec<FailureHistogramV1>,
        finalization: Option<FinalizationFailureV1>,
    },
    Containment {
        reason: ContainmentFailureV1,
        edge_ids: Vec<u64>,
        histogram: Vec<FailureHistogramV1>,
        finalization: Option<FinalizationFailureV1>,
    },
    SourceBinding {
        reason: SourceBindingFailureV1,
        edge_ids: Vec<u64>,
        histogram: Vec<FailureHistogramV1>,
        finalization: Option<FinalizationFailureV1>,
    },
    Finalization {
        reason: FinalizationFailureV1,
        edge_ids: Vec<u64>,
        histogram: Vec<FailureHistogramV1>,
        finalization: Option<FinalizationFailureV1>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailureHistogramV1 {
    pub reason: RawAdmissionFailureV1,
    pub edge_ids: Vec<u64>,
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
    pub stored_call_rows: u128,
    pub effective_endpoint_rows: u128,
    pub exact_resolved_rows: u128,
    pub admitted_rows: u128,
    pub unresolved_placeholder_rows: u128,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailLengthCountsV1 {
    pub length: u8,
    pub effective_endpoint: u128,
    pub exact_resolved: u128,
    pub strictly_admitted: u128,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrailReportV1 {
    pub repository_id: String,
    pub lengths: Vec<TrailLengthCountsV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
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
#[serde(rename_all = "snake_case")]
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
pub struct ToolResultBytesV1 {
    pub v2024_11_05: u64,
    pub v2025_03_26: u64,
    pub v2025_06_18: u64,
    pub v2025_11_25: u64,
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
    pub complete_projection_bytes: u64,
    pub tool_result_bytes: ToolResultBytesV1,
    pub negative_mutations: Vec<NegativeMutationResultV1>,
    pub first_failure: StepFailureV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailureBucketV1 {
    pub failure: StepFailureV1,
    pub count: u128,
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
#[serde(rename_all = "snake_case")]
pub enum ActivationOutcomeV1 {
    PublicExactVerifier,
    ExperimentalManualVerifier,
    KeepProofDark,
    DelayFullV3Cut,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
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
#[serde(deny_unknown_fields)]
pub struct FailedGateV1 {
    pub kind: QualificationGateKindV1,
    pub detail: String,
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
            || self.failure_funnel.classified_positive_steps != 312
            || self.failure_funnel.unclassified_positive_steps != 0
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
                || c.complete_projection_bytes > 65536
                || [
                    c.tool_result_bytes.v2024_11_05,
                    c.tool_result_bytes.v2025_03_26,
                    c.tool_result_bytes.v2025_06_18,
                    c.tool_result_bytes.v2025_11_25,
                ]
                .iter()
                .any(|v| *v > 65536)
            {
                bail!("proof_availability_case_invalid")
            }
        }
        if !unique(
            self.decision
                .failed_gates
                .iter()
                .map(|g| format!("{:?}", g.kind))
                .collect::<Vec<_>>()
                .iter()
                .map(|v| v.as_str()),
        ) {
            bail!("proof_availability_decision_invalid")
        }
        Ok(())
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
                        | "source_tree_sha256"
                        | "database_sha256"
                ) {
                    property.insert("pattern".into(), Value::String(SHA256.into()));
                }
                if matches!(name.as_str(), "commit" | "source_commit" | "source_tree") {
                    property.insert("pattern".into(), Value::String(COMMIT.into()));
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
    if empty(&value.path) || value.start_byte >= value.end_byte || !hash(&value.sha256) {
        bail!("proof_availability_range_invalid")
    }
    Ok(())
}
fn unique<'a>(mut values: impl Iterator<Item = &'a str>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|v| !empty(v) && set.insert(v))
}
