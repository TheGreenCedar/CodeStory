use anyhow::{Result, bail};
use codestory_agent::proof_qualification_support as product_proof;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const CORPUS_SCHEMA: &str = "codestory.proof-availability-corpus/v1";
pub const PATH_FILE_SCHEMA: &str = "codestory.proof-availability-path-file/v1";
pub const REPORT_SCHEMA: &str = "codestory.proof-availability-report/v1";
pub const THRESHOLDS_SCHEMA: &str = "codestory.proof-availability-thresholds/v1";
pub const MAX_CANDIDATE_EDGES_PER_STEP: usize =
    codestory_runtime::proof_qualification_support::MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP
        as usize;
pub const MAX_OBSERVED_RECEIPTS_PER_CASE: usize =
    codestory_runtime::proof_qualification_support::MAX_QUALIFICATION_OBSERVED_RECEIPTS_PER_CASE;
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
    pub selector: ExactSymbolSelectorV1,
    pub range: OracleSourceRangeV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClauseAnchorV1 {
    pub clause_id: String,
    pub start_byte: u32,
    pub end_byte_exclusive: u32,
    pub quote: String,
    pub classification: ClauseClassificationV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::enum_variant_names)] // Mirrors the dark wire contract names exactly.
pub enum ClauseClassificationV1 {
    ResolvedMaterial { fields: Vec<ProofContractFieldV1> },
    UnresolvedMaterial { reason: UnresolvedMaterialReasonV1 },
    NonMaterial { reason: NonMaterialReasonV1 },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProofContractFieldV1 {
    Start,
    StepTarget { step: u8 },
    Directness { step: u8 },
    Ordering { step: u8 },
    Relation { step: u8 },
    TraversalProhibition { index: u8 },
    ProjectionExclusion { index: u8 },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedMaterialReasonV1 {
    MissingSelectorResolution,
    AmbiguousSelectorResolution,
    UnsupportedInterpretation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NonMaterialReasonV1 {
    Whitespace,
    Punctuation,
    Connector,
    Commentary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExactSymbolSelectorV1 {
    PinnedNode {
        project_id: String,
        core_generation_id: String,
        core_run_id: String,
        node_id: String,
    },
    CanonicalId {
        canonical_id: String,
    },
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExactScopeSelectorV1 {
    PinnedNode {
        project_id: String,
        core_generation_id: String,
        core_run_id: String,
        node_id: String,
    },
    CanonicalId {
        canonical_id: String,
    },
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DirectCallStepV1 {
    pub target: ExactSymbolSelectorV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallPathSpecV1 {
    pub start: ExactSymbolSelectorV1,
    pub steps: Vec<DirectCallStepV1>,
    pub prohibit_traversal_through: Vec<ExactScopeSelectorV1>,
    pub exclude_from_projection: Vec<ExactScopeSelectorV1>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleStepV1 {
    pub caller: OracleDeclarationV1,
    pub callsite_line: u32,
    pub callsite_expression: OracleSourceRangeV1,
    pub receipt_line_window: OracleSourceRangeV1,
    pub target: OracleDeclarationV1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NegativeMutationKindV1 {
    ReplaceStepTarget,
    ReplaceStepSource,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NegativeRelationFindingV1 {
    NoDirectCall,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NegativeRelationAuditV1 {
    pub caller: OracleDeclarationV1,
    pub target: OracleDeclarationV1,
    pub caller_body: OracleSourceRangeV1,
    pub finding: NegativeRelationFindingV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NegativeMutationV1 {
    pub mutation_id: String,
    pub path_id: String,
    pub kind: NegativeMutationKindV1,
    pub step_index: u8,
    pub mutated_spec: CallPathSpecV1,
    pub source_audit: NegativeRelationAuditV1,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OracleAuditV1 {
    pub source_area: String,
    pub curator: String,
    pub reviewer: String,
    pub review_date: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OraclePathV1 {
    pub case_id: String,
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
        if empty(&self.case_id)
            || empty(&self.language)
            || empty(&self.source_text)
            || self.clauses.is_empty()
            || self.spec.steps.is_empty()
            || self.spec.steps.len() > 6
            || self.oracle_steps.len() != self.spec.steps.len()
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
        validate_oracle_contract(&self.source_text, &self.clauses, &self.spec)?;
        for (index, step) in self.oracle_steps.iter().enumerate() {
            if empty(&step.caller.symbol) || step.callsite_line == 0 || empty(&step.target.symbol) {
                bail!("proof_availability_oracle_declaration_invalid")
            }
            validate_declaration(&step.caller)?;
            validate_declaration(&step.target)?;
            range(&step.caller.range)?;
            range(&step.callsite_expression)?;
            range(&step.receipt_line_window)?;
            range(&step.target.range)?;
            if step.callsite_expression.path != step.receipt_line_window.path
                || step.callsite_expression.file_byte_length
                    != step.receipt_line_window.file_byte_length
                || step.callsite_expression.start_byte < step.receipt_line_window.start_byte
                || step.callsite_expression.end_byte > step.receipt_line_window.end_byte
                || step
                    .receipt_line_window
                    .end_byte
                    .checked_sub(step.receipt_line_window.start_byte)
                    .is_none_or(|length| length > 8_192)
                || step.target.selector != self.spec.steps[index].target
                || (index == 0 && step.caller.selector != self.spec.start)
                || (index > 0 && step.caller.selector != self.spec.steps[index - 1].target)
            {
                bail!("proof_availability_oracle_chain_invalid")
            }
        }
        for m in &self.negative_mutations {
            let Some(step) = self.oracle_steps.get(usize::from(m.step_index)) else {
                bail!("proof_availability_mutation_invalid")
            };
            if m.path_id != self.case_id
                || m.mutated_spec == self.spec
                || validate_spec_shape(&m.mutated_spec).is_err()
            {
                bail!("proof_availability_mutation_invalid")
            }
            validate_negative_mutation(&self.spec, step, m)?;
            validate_oracle_contract(&self.source_text, &self.clauses, &m.mutated_spec)?;
        }
        if self.negative_mutations[0].kind == self.negative_mutations[1].kind {
            bail!("proof_availability_mutation_kinds_not_distinct")
        }
        if empty(&self.audit.source_area)
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
pub struct LengthDistributionEntryV1 {
    pub path_length: u8,
    pub path_count: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CohortPathFileV1 {
    pub schema: String,
    pub repository_id: String,
    pub repository: String,
    pub commit: String,
    pub workspace: String,
    pub source_tree_sha256: String,
    pub curator: String,
    pub reviewer: String,
    pub review_date: String,
    pub source_area_requirement: SourceAreaRequirementV1,
    pub paths: Vec<OraclePathV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceAreaRequirementV1 {
    RequiredAtLeastFive,
    NotAvailable { reason: String },
}

impl CohortPathFileV1 {
    pub fn from_json(value: Value) -> Result<Self> {
        let value: Self = serde_json::from_value(value)?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_with_registry(&QUALIFICATION_REPOSITORIES)
    }

    pub(crate) fn validate_with_registry(
        &self,
        registry: &[(&str, &str, &str, &str)],
    ) -> Result<()> {
        if self.schema != PATH_FILE_SCHEMA
            || !registry_entry_matches(
                registry,
                &self.repository_id,
                &self.repository,
                &self.commit,
                &self.workspace,
            )
            || !hash(&self.source_tree_sha256)
            || empty(&self.curator)
            || empty(&self.reviewer)
            || self.curator == self.reviewer
            || !date(&self.review_date)
            || self.paths.len() != 30
            || !unique(self.paths.iter().map(|path| path.case_id.as_str()))
        {
            bail!("proof_availability_path_file_invalid")
        }
        let mut distribution = [0u8; 6];
        let mut source_areas = BTreeSet::new();
        let mut primary_files = BTreeMap::<&str, usize>::new();
        let mut positive_pairs = BTreeSet::new();
        for path in &self.paths {
            path.validate()?;
            distribution[path.spec.steps.len() - 1] += 1;
            source_areas.insert(path.audit.source_area.as_str());
            *primary_files
                .entry(path.oracle_steps[0].caller.range.path.as_str())
                .or_default() += 1;
            for step in &path.oracle_steps {
                let pair =
                    canonical_artifact_bytes(&(&step.caller.selector, &step.target.selector))?;
                if !positive_pairs.insert(pair) {
                    bail!("proof_availability_duplicate_positive_relation")
                }
            }
        }
        if distribution != [10, 7, 5, 3, 3, 2]
            || self
                .paths
                .iter()
                .map(|path| path.spec.steps.len())
                .sum::<usize>()
                != 78
            || (matches!(
                self.source_area_requirement,
                SourceAreaRequirementV1::RequiredAtLeastFive
            ) && source_areas.len() < 5)
            || matches!(&self.source_area_requirement, SourceAreaRequirementV1::NotAvailable { reason } if empty(reason))
            || primary_files.values().any(|count| *count > 6)
        {
            bail!("proof_availability_path_file_distribution_invalid")
        }
        Ok(())
    }
}

pub const QUALIFICATION_REPOSITORIES: [(&str, &str, &str, &str); 4] = [
    (
        "codestory-rust",
        "https://github.com/TheGreenCedar/CodeStory.git",
        "74753c1766c80f8cf27873943409bd509bc30350",
        ".",
    ),
    (
        "vite-ts-js",
        "https://github.com/vitejs/vite.git",
        "80a333a23103ced0442d4463d1191433d90f5e19",
        "packages/vite",
    ),
    (
        "flask-python",
        "https://github.com/pallets/flask.git",
        "7fff56f5172c48b6f3aedf17ee14ef5c2533dfd1",
        ".",
    ),
    (
        "gin-go",
        "https://github.com/gin-gonic/gin.git",
        "d75fcd4c9ab260e5225de590f1f0f8c0e0e12d11",
        ".",
    ),
];

fn registry_entry_matches(
    registry: &[(&str, &str, &str, &str)],
    id: &str,
    repository: &str,
    commit: &str,
    workspace: &str,
) -> bool {
    registry.iter().any(|entry| {
        entry.0 == id && entry.1 == repository && entry.2 == commit && entry.3 == workspace
    })
}

fn validate_declaration(declaration: &OracleDeclarationV1) -> Result<()> {
    if empty(&declaration.symbol) {
        bail!("proof_availability_oracle_declaration_invalid")
    }
    range(&declaration.range)
}

fn validate_oracle_contract(
    source_text: &str,
    clauses: &[ClauseAnchorV1],
    spec: &CallPathSpecV1,
) -> Result<()> {
    validate_spec_shape(spec)?;
    require_product_validated(product_proof::UnvalidatedCallPathContract::new(
        source_text,
        clauses
            .iter()
            .map(to_product_clause)
            .collect::<Result<Vec<_>>>()?,
        to_product_spec(spec),
    ))
}

fn validate_spec_shape(spec: &CallPathSpecV1) -> Result<()> {
    if !(1..=6).contains(&spec.steps.len())
        || spec.prohibit_traversal_through.len() > 6
        || spec.exclude_from_projection.len() > 6
    {
        bail!("proof_availability_oracle_spec_invalid")
    }
    let fields = required_product_fields(spec)?;
    require_product_validated(product_proof::UnvalidatedCallPathContract::new(
        "x",
        vec![product_proof::ClauseAnchor {
            clause_id: "benchmark-shape".into(),
            start: 0,
            end: 1,
            quote: "x".into(),
            classification: product_proof::ClauseClassification::ResolvedMaterial { fields },
        }],
        to_product_spec(spec),
    ))
}

fn require_product_validated(input: product_proof::UnvalidatedCallPathContract) -> Result<()> {
    match product_proof::validate_contract(input) {
        Ok(product_proof::ValidationOutcome::Validated { .. }) => Ok(()),
        Ok(product_proof::ValidationOutcome::Unknown { gaps, .. }) => {
            bail!("proof_availability_product_contract_unknown: {gaps:?}")
        }
        Err(error) => bail!("proof_availability_product_contract_invalid: {error:?}"),
    }
}

fn to_product_clause(clause: &ClauseAnchorV1) -> Result<product_proof::ClauseAnchor> {
    Ok(product_proof::ClauseAnchor {
        clause_id: clause.clause_id.clone(),
        start: usize::try_from(clause.start_byte)?,
        end: usize::try_from(clause.end_byte_exclusive)?,
        quote: clause.quote.clone(),
        classification: match &clause.classification {
            ClauseClassificationV1::ResolvedMaterial { fields } => {
                product_proof::ClauseClassification::ResolvedMaterial {
                    fields: fields.iter().copied().map(to_product_field).collect(),
                }
            }
            ClauseClassificationV1::UnresolvedMaterial { reason } => {
                product_proof::ClauseClassification::UnresolvedMaterial {
                    reason: match reason {
                        UnresolvedMaterialReasonV1::MissingSelectorResolution => {
                            product_proof::UnresolvedMaterialReason::MissingSelectorResolution
                        }
                        UnresolvedMaterialReasonV1::AmbiguousSelectorResolution => {
                            product_proof::UnresolvedMaterialReason::AmbiguousSelectorResolution
                        }
                        UnresolvedMaterialReasonV1::UnsupportedInterpretation => {
                            product_proof::UnresolvedMaterialReason::UnsupportedInterpretation
                        }
                    },
                }
            }
            ClauseClassificationV1::NonMaterial { reason } => {
                product_proof::ClauseClassification::NonMaterial {
                    kind: match reason {
                        NonMaterialReasonV1::Whitespace => {
                            product_proof::NonMaterialKind::Whitespace
                        }
                        NonMaterialReasonV1::Punctuation => {
                            product_proof::NonMaterialKind::Punctuation
                        }
                        NonMaterialReasonV1::Connector => product_proof::NonMaterialKind::Connector,
                        NonMaterialReasonV1::Commentary => {
                            product_proof::NonMaterialKind::Commentary
                        }
                    },
                }
            }
        },
    })
}

fn to_product_field(field: ProofContractFieldV1) -> product_proof::ProofContractField {
    match field {
        ProofContractFieldV1::Start => product_proof::ProofContractField::Start,
        ProofContractFieldV1::StepTarget { step } => {
            product_proof::ProofContractField::StepTarget { step }
        }
        ProofContractFieldV1::Directness { step } => {
            product_proof::ProofContractField::Directness { step }
        }
        ProofContractFieldV1::Ordering { step } => {
            product_proof::ProofContractField::Ordering { step }
        }
        ProofContractFieldV1::Relation { step } => {
            product_proof::ProofContractField::Relation { step }
        }
        ProofContractFieldV1::TraversalProhibition { index } => {
            product_proof::ProofContractField::TraversalProhibition { index }
        }
        ProofContractFieldV1::ProjectionExclusion { index } => {
            product_proof::ProofContractField::ProjectionExclusion { index }
        }
    }
}

fn required_product_fields(
    spec: &CallPathSpecV1,
) -> Result<Vec<product_proof::ProofContractField>> {
    let mut fields = vec![product_proof::ProofContractField::Start];
    for step in 0..spec.steps.len() {
        let step = u8::try_from(step)?;
        fields.extend([
            product_proof::ProofContractField::StepTarget { step },
            product_proof::ProofContractField::Directness { step },
            product_proof::ProofContractField::Ordering { step },
            product_proof::ProofContractField::Relation { step },
        ]);
    }
    fields.extend((0..spec.prohibit_traversal_through.len()).map(|index| {
        product_proof::ProofContractField::TraversalProhibition {
            index: u8::try_from(index).expect("benchmark scope count is bounded"),
        }
    }));
    fields.extend((0..spec.exclude_from_projection.len()).map(|index| {
        product_proof::ProofContractField::ProjectionExclusion {
            index: u8::try_from(index).expect("benchmark scope count is bounded"),
        }
    }));
    Ok(fields)
}

fn to_product_spec(spec: &CallPathSpecV1) -> product_proof::UnvalidatedCallPathSpec {
    product_proof::UnvalidatedCallPathSpec {
        start: to_product_symbol_selector(&spec.start),
        steps: spec
            .steps
            .iter()
            .map(|step| product_proof::UnvalidatedDirectCallStep {
                target: to_product_symbol_selector(&step.target),
            })
            .collect(),
        prohibit_traversal_through: spec
            .prohibit_traversal_through
            .iter()
            .map(to_product_scope_selector)
            .collect(),
        exclude_from_projection: spec
            .exclude_from_projection
            .iter()
            .map(to_product_scope_selector)
            .collect(),
    }
}

fn to_product_symbol_selector(
    selector: &ExactSymbolSelectorV1,
) -> product_proof::UnvalidatedExactSymbolSelector {
    match selector {
        ExactSymbolSelectorV1::PinnedNode {
            project_id,
            core_generation_id,
            core_run_id,
            node_id,
        } => product_proof::UnvalidatedExactSymbolSelector::PinnedNode(
            product_proof::PinnedNodeIdentity {
                project_id: project_id.clone(),
                core_generation_id: core_generation_id.clone(),
                core_run_id: core_run_id.clone(),
                node_id: node_id.clone(),
            },
        ),
        ExactSymbolSelectorV1::CanonicalId { canonical_id } => {
            product_proof::UnvalidatedExactSymbolSelector::CanonicalId(canonical_id.clone())
        }
        ExactSymbolSelectorV1::QualifiedName {
            qualified_name,
            project_file_components,
        } => product_proof::UnvalidatedExactSymbolSelector::QualifiedName {
            qualified_name: qualified_name.clone(),
            project_file_components: project_file_components.clone(),
        },
    }
}

fn to_product_scope_selector(
    selector: &ExactScopeSelectorV1,
) -> product_proof::UnvalidatedExactScopeSelector {
    match selector {
        ExactScopeSelectorV1::PinnedNode {
            project_id,
            core_generation_id,
            core_run_id,
            node_id,
        } => product_proof::UnvalidatedExactScopeSelector::PinnedNode(
            product_proof::PinnedNodeIdentity {
                project_id: project_id.clone(),
                core_generation_id: core_generation_id.clone(),
                core_run_id: core_run_id.clone(),
                node_id: node_id.clone(),
            },
        ),
        ExactScopeSelectorV1::CanonicalId { canonical_id } => {
            product_proof::UnvalidatedExactScopeSelector::CanonicalId(canonical_id.clone())
        }
        ExactScopeSelectorV1::QualifiedName {
            qualified_name,
            project_file_components,
        } => product_proof::UnvalidatedExactScopeSelector::QualifiedName {
            qualified_name: qualified_name.clone(),
            project_file_components: project_file_components.clone(),
        },
    }
}

pub(crate) fn oracle_path_product_contract(
    path: &OraclePathV1,
) -> Result<product_proof::UnvalidatedCallPathContract> {
    path.validate()?;
    Ok(product_proof::UnvalidatedCallPathContract::new(
        &path.source_text,
        path.clauses
            .iter()
            .map(to_product_clause)
            .collect::<Result<Vec<_>>>()?,
        to_product_spec(&path.spec),
    ))
}

pub(crate) fn negative_mutation_product_contract(
    path: &OraclePathV1,
    mutation: &NegativeMutationV1,
) -> Result<product_proof::UnvalidatedCallPathContract> {
    path.validate()?;
    if !path
        .negative_mutations
        .iter()
        .any(|candidate| candidate.mutation_id == mutation.mutation_id)
    {
        bail!("proof_availability_mutation_oracle_missing")
    }
    Ok(product_proof::UnvalidatedCallPathContract::new(
        &path.source_text,
        path.clauses
            .iter()
            .map(to_product_clause)
            .collect::<Result<Vec<_>>>()?,
        to_product_spec(&mutation.mutated_spec),
    ))
}

pub fn validate_project_file(components: &[String]) -> Result<()> {
    if components.is_empty()
        || components.iter().any(|component| {
            empty(component)
                || component == "."
                || component == ".."
                || component.contains('\0')
                || component.contains('/')
                || component.contains('\\')
                || component.starts_with('~')
                || component.contains(':')
        })
    {
        bail!("proof_availability_project_file_invalid")
    }
    Ok(())
}

fn validate_negative_mutation(
    positive: &CallPathSpecV1,
    step: &OracleStepV1,
    mutation: &NegativeMutationV1,
) -> Result<()> {
    let index = usize::from(mutation.step_index);
    let mut expected = positive.clone();
    match mutation.kind {
        NegativeMutationKindV1::ReplaceStepTarget => {
            expected.steps[index].target = mutation.mutated_spec.steps[index].target.clone();
            if expected != mutation.mutated_spec
                || mutation.source_audit.caller.selector != step.caller.selector
                || mutation.source_audit.target.selector
                    != mutation.mutated_spec.steps[index].target
            {
                bail!("proof_availability_target_mutation_invalid")
            }
        }
        NegativeMutationKindV1::ReplaceStepSource => {
            let mutated_source = if index == 0 {
                &mutation.mutated_spec.start
            } else {
                &mutation.mutated_spec.steps[index - 1].target
            };
            if index == 0 {
                expected.start = mutated_source.clone();
            } else {
                expected.steps[index - 1].target = mutated_source.clone();
            }
            if expected != mutation.mutated_spec
                || mutation.source_audit.caller.selector != *mutated_source
                || mutation.source_audit.target.selector != step.target.selector
            {
                bail!("proof_availability_source_mutation_invalid")
            }
        }
    }
    validate_declaration(&mutation.source_audit.caller)?;
    validate_declaration(&mutation.source_audit.target)?;
    range(&mutation.source_audit.caller_body)?;
    if mutation.source_audit.caller.range.path != mutation.source_audit.caller_body.path
        || mutation.source_audit.caller.range.file_byte_length
            != mutation.source_audit.caller_body.file_byte_length
        || mutation.source_audit.caller_body.start_byte
            > mutation.source_audit.caller.range.start_byte
        || mutation.source_audit.caller_body.end_byte < mutation.source_audit.caller.range.end_byte
    {
        bail!("proof_availability_negative_source_audit_invalid")
    }
    Ok(())
}

fn valid_length_distribution(distribution: &[LengthDistributionEntryV1]) -> bool {
    distribution.len() == 6
        && distribution
            .iter()
            .zip([10u8, 7, 5, 3, 3, 2])
            .enumerate()
            .all(|(index, (entry, count))| {
                entry.path_length == u8::try_from(index + 1).unwrap() && entry.path_count == count
            })
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
    pub path_length_distribution: Vec<LengthDistributionEntryV1>,
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
        self.validate_with_registry(&QUALIFICATION_REPOSITORIES)
    }

    fn validate_with_registry(&self, registry: &[(&str, &str, &str, &str)]) -> Result<()> {
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
        {
            bail!("proof_availability_corpus_invalid")
        }
        for c in &self.cohorts {
            if !registry_entry_matches(
                registry,
                &c.repository_id,
                &c.repository,
                &c.commit,
                &c.workspace,
            ) || empty(&c.path_file)
                || c.path_file != format!("paths/{}.json", c.repository_id)
                || !hash(&c.path_file_sha256)
                || !hash(&c.source_tree_sha256)
                || c.path_count != 30
                || c.positive_step_count != 78
                || !valid_length_distribution(&c.path_length_distribution)
            {
                bail!("proof_availability_cohort_invalid")
            }
        }
        Ok(())
    }

    pub fn validate_with_path_files(&self, path_files: &[CohortPathFileV1]) -> Result<()> {
        self.validate_with_path_files_and_registry(path_files, &QUALIFICATION_REPOSITORIES)
    }

    pub(crate) fn validate_with_path_files_and_registry(
        &self,
        path_files: &[CohortPathFileV1],
        registry: &[(&str, &str, &str, &str)],
    ) -> Result<()> {
        self.validate_with_registry(registry)?;
        if path_files.len() != 4
            || !unique(path_files.iter().map(|file| file.repository_id.as_str()))
        {
            bail!("proof_availability_path_file_set_invalid")
        }
        let mut total_paths = 0usize;
        let mut total_steps = 0usize;
        let mut total_mutations = 0usize;
        let mut case_ids = BTreeSet::new();
        for cohort in &self.cohorts {
            let file = path_files
                .iter()
                .find(|file| file.repository_id == cohort.repository_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_path_file_missing"))?;
            file.validate_with_registry(registry)?;
            if file.repository != cohort.repository
                || file.commit != cohort.commit
                || file.workspace != cohort.workspace
                || file.source_tree_sha256 != cohort.source_tree_sha256
                || canonical_cohort_path_file_sha256(file)? != cohort.path_file_sha256
                || file.paths.len() != usize::from(cohort.path_count)
                || file
                    .paths
                    .iter()
                    .map(|path| path.spec.steps.len())
                    .sum::<usize>()
                    != usize::from(cohort.positive_step_count)
            {
                bail!("proof_availability_path_file_binding_invalid")
            }
            total_paths += file.paths.len();
            if file
                .paths
                .iter()
                .any(|path| !case_ids.insert(path.case_id.as_str()))
            {
                bail!("proof_availability_case_id_duplicate")
            }
            total_steps += file
                .paths
                .iter()
                .map(|path| path.spec.steps.len())
                .sum::<usize>();
            total_mutations += file
                .paths
                .iter()
                .map(|path| path.negative_mutations.len())
                .sum::<usize>();
        }
        if total_paths != usize::from(self.positive_request_count)
            || total_steps != usize::from(self.positive_step_count)
            || total_mutations != usize::from(self.negative_request_count)
        {
            bail!("proof_availability_corpus_actual_totals_invalid")
        }
        Ok(())
    }

    pub fn validate_against_thresholds(&self, thresholds: &ThresholdsV1) -> Result<()> {
        self.validate()?;
        thresholds.validate()?;
        if self.thresholds_sha256 != canonical_thresholds_sha256(thresholds)?
            || self.methodology_sha256 != thresholds.methodology_sha256
            || self.corpus_id != thresholds.thresholds_id
            || self.cohorts.len() != usize::from(thresholds.expected_cohort_count)
            || self.positive_request_count != thresholds.expected_positive_requests
            || self.positive_step_count != thresholds.expected_positive_steps
            || self.negative_request_count != thresholds.expected_negative_requests
        {
            bail!("proof_availability_corpus_threshold_binding_invalid")
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
            || self.automatic
                != frozen_role_thresholds(96, 21, 720, 500, 900, 950, 950, 500, 1500, 32768, 16384)
            || self.stable_explicit
                != frozen_role_thresholds(60, 12, 410, 240, 750, 800, 900, 1000, 2000, 32768, 16384)
            || self.experimental
                != frozen_role_thresholds(24, 12, 140, 0, 500, 600, 800, 2000, 3000, 49152, 24576)
        {
            bail!("proof_availability_thresholds_invalid")
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn frozen_role_thresholds(
    minimum_full_proofs: u16,
    minimum_full_proofs_per_cohort: u16,
    minimum_full_proof_wilson_lower_milli: u16,
    minimum_cohort_wilson_lower_milli: u16,
    minimum_positive_step_recall_milli: u16,
    minimum_full_or_useful_partial_milli: u16,
    minimum_actionable_exact_gap_milli: u16,
    maximum_unknown_p95_ms: u64,
    maximum_transport_p95_ms: u64,
    maximum_complete_response_p95_bytes: u64,
    maximum_unknown_response_p95_bytes: u64,
) -> RoleThresholdsV1 {
    RoleThresholdsV1 {
        minimum_full_proofs,
        minimum_full_proofs_per_cohort,
        minimum_full_proof_wilson_lower_milli,
        minimum_cohort_wilson_lower_milli,
        minimum_positive_step_recall_milli,
        minimum_full_or_useful_partial_milli,
        minimum_actionable_exact_gap_milli,
        maximum_unknown_p95_ms,
        maximum_transport_p95_ms,
        maximum_complete_response_p95_bytes,
        maximum_unknown_response_p95_bytes,
        maximum_response_bytes: 65_536,
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
    Resolved { node_id: i64 },
    Failed { reason: SelectorFailureV1 },
    Unavailable { reason: UnavailableReasonV1 },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectorQualificationTraceV1 {
    pub selector_index: u64,
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
impl From<codestory_agent::proof_qualification_support::RawAdmissionFailure>
    for RawAdmissionFailureV1
{
    fn from(value: codestory_agent::proof_qualification_support::RawAdmissionFailure) -> Self {
        match value {
            codestory_agent::proof_qualification_support::RawAdmissionFailure::WrongKind => Self::WrongKind,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::CertaintyAbsent => Self::CertaintyAbsent,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::CertaintyProbable => Self::CertaintyProbable,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::CertaintyUncertain => Self::CertaintyUncertain,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::WrongEffectiveSource => Self::WrongEffectiveSource,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::WrongEffectiveTarget => Self::WrongEffectiveTarget,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::MissingExactResolvedTarget => Self::MissingExactResolvedTarget,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::CandidateAlternativesRetained => Self::CandidateAlternativesRetained,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::MissingFileNode => Self::MissingFileNode,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::MissingLine => Self::MissingLine,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity => Self::InvalidOrLegacyCallsiteIdentity,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::CallsiteFileMismatch => Self::CallsiteFileMismatch,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::CallsiteLineMismatch => Self::CallsiteLineMismatch,
            codestory_agent::proof_qualification_support::RawAdmissionFailure::CallsiteRawTargetMismatch => Self::CallsiteRawTargetMismatch,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ContainmentFailureV1 {
    EdgeSourceFileMismatch,
    Missing,
    Ambiguous,
}
impl From<codestory_runtime::proof_qualification_support::ContainmentFailure>
    for ContainmentFailureV1
{
    fn from(value: codestory_runtime::proof_qualification_support::ContainmentFailure) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::ContainmentFailure::EdgeSourceFileMismatch => Self::EdgeSourceFileMismatch,
            codestory_runtime::proof_qualification_support::ContainmentFailure::Missing => Self::Missing,
            codestory_runtime::proof_qualification_support::ContainmentFailure::Ambiguous => Self::Ambiguous,
        }
    }
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
impl From<codestory_runtime::proof_qualification_support::SourceBindingFailure>
    for SourceBindingFailureV1
{
    fn from(value: codestory_runtime::proof_qualification_support::SourceBindingFailure) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::SourceBindingFailure::FileIncomplete => Self::FileIncomplete,
            codestory_runtime::proof_qualification_support::SourceBindingFailure::StoredHashAbsent => Self::StoredHashAbsent,
            codestory_runtime::proof_qualification_support::SourceBindingFailure::WorkingTreeReadFailed => Self::WorkingTreeReadFailed,
            codestory_runtime::proof_qualification_support::SourceBindingFailure::WorkingTreeHashMismatch => Self::WorkingTreeHashMismatch,
            codestory_runtime::proof_qualification_support::SourceBindingFailure::InvalidUtf8 => Self::InvalidUtf8,
            codestory_runtime::proof_qualification_support::SourceBindingFailure::LineMissing => Self::LineMissing,
            codestory_runtime::proof_qualification_support::SourceBindingFailure::LineOverLimit => Self::LineOverLimit,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FinalizationFailureV1 {
    ReceiptIntegration,
    ReceiptBudget,
    ProjectionBudget,
}
impl From<codestory_runtime::proof_qualification_support::FinalizationFailure>
    for FinalizationFailureV1
{
    fn from(value: codestory_runtime::proof_qualification_support::FinalizationFailure) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::FinalizationFailure::ReceiptIntegration => Self::ReceiptIntegration,
            codestory_runtime::proof_qualification_support::FinalizationFailure::ReceiptBudget => Self::ReceiptBudget,
            codestory_runtime::proof_qualification_support::FinalizationFailure::ProjectionBudget => Self::ProjectionBudget,
        }
    }
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
    pub edge_ids: Vec<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepQualificationOutcomeV1 {
    Admitted {
        edge_ids: Vec<i64>,
    },
    FirstZeroSurvivor {
        gate: CandidateGateV1,
        histogram: Vec<CandidateFailureHistogramV1>,
    },
    CandidateLimitExceeded {
        maximum_candidate_edges: u32,
        observed_candidate_edges_at_least: u32,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepQualificationTraceV1 {
    pub step_index: u64,
    pub candidate_edge_ids: Vec<i64>,
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

impl From<codestory_runtime::proof_qualification_support::SelectorFailure> for SelectorFailureV1 {
    fn from(value: codestory_runtime::proof_qualification_support::SelectorFailure) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::SelectorFailure::Missing => {
                Self::Missing
            }
            codestory_runtime::proof_qualification_support::SelectorFailure::Ambiguous => {
                Self::Ambiguous
            }
            codestory_runtime::proof_qualification_support::SelectorFailure::NonCallable => {
                Self::NonCallable
            }
        }
    }
}

impl From<codestory_agent::proof_qualification_support::UnavailableReason> for UnavailableReasonV1 {
    fn from(value: codestory_agent::proof_qualification_support::UnavailableReason) -> Self {
        match value {
            codestory_agent::proof_qualification_support::UnavailableReason::ValidatedContractHashMismatch => Self::ValidatedContractHashMismatch,
            codestory_agent::proof_qualification_support::UnavailableReason::PublicationPinMismatch => Self::PublicationPinMismatch,
            codestory_agent::proof_qualification_support::UnavailableReason::SourceNotBoundToPublication => Self::SourceNotBoundToPublication,
            codestory_agent::proof_qualification_support::UnavailableReason::ProofFactsUnavailable => Self::ProofFactsUnavailable,
        }
    }
}

impl From<codestory_runtime::proof_qualification_support::SelectorGateOutcome>
    for SelectorGateOutcomeV1
{
    fn from(value: codestory_runtime::proof_qualification_support::SelectorGateOutcome) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::SelectorGateOutcome::Resolved {
                node_id,
            } => Self::Resolved { node_id: node_id.0 },
            codestory_runtime::proof_qualification_support::SelectorGateOutcome::Failed(reason) => {
                Self::Failed {
                    reason: reason.into(),
                }
            }
            codestory_runtime::proof_qualification_support::SelectorGateOutcome::Unavailable(
                reason,
            ) => Self::Unavailable {
                reason: reason.into(),
            },
        }
    }
}

impl TryFrom<codestory_runtime::proof_qualification_support::SelectorQualificationTrace>
    for SelectorQualificationTraceV1
{
    type Error = anyhow::Error;

    fn try_from(
        value: codestory_runtime::proof_qualification_support::SelectorQualificationTrace,
    ) -> Result<Self> {
        Ok(Self {
            selector_index: u64::try_from(value.selector_index)
                .map_err(|_| anyhow::anyhow!("proof_availability_selector_index_overflow"))?,
            outcome: value.outcome.into(),
        })
    }
}

impl From<codestory_runtime::proof_qualification_support::CandidateGate> for CandidateGateV1 {
    fn from(value: codestory_runtime::proof_qualification_support::CandidateGate) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::CandidateGate::RawAdmission => {
                Self::RawAdmission
            }
            codestory_runtime::proof_qualification_support::CandidateGate::Containment => {
                Self::Containment
            }
            codestory_runtime::proof_qualification_support::CandidateGate::SourceBinding => {
                Self::SourceBinding
            }
            codestory_runtime::proof_qualification_support::CandidateGate::Line => Self::Line,
        }
    }
}

impl From<codestory_runtime::proof_qualification_support::CandidateFailure> for CandidateFailureV1 {
    fn from(value: codestory_runtime::proof_qualification_support::CandidateFailure) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::CandidateFailure::RawAdmission(
                reason,
            ) => Self::RawAdmission {
                reason: reason.into(),
            },
            codestory_runtime::proof_qualification_support::CandidateFailure::Containment(
                reason,
            ) => Self::Containment {
                reason: reason.into(),
            },
            codestory_runtime::proof_qualification_support::CandidateFailure::SourceBinding(
                reason,
            ) => Self::SourceBinding {
                reason: reason.into(),
            },
        }
    }
}

impl From<codestory_runtime::proof_qualification_support::CandidateFailureHistogram>
    for CandidateFailureHistogramV1
{
    fn from(
        value: codestory_runtime::proof_qualification_support::CandidateFailureHistogram,
    ) -> Self {
        Self {
            reason: value.reason.into(),
            edge_ids: value.edge_ids.into_iter().map(|id| id.0).collect(),
        }
    }
}

impl From<codestory_runtime::proof_qualification_support::StepQualificationOutcome>
    for StepQualificationOutcomeV1
{
    fn from(
        value: codestory_runtime::proof_qualification_support::StepQualificationOutcome,
    ) -> Self {
        match value {
            codestory_runtime::proof_qualification_support::StepQualificationOutcome::Admitted { edge_ids } => Self::Admitted { edge_ids: edge_ids.into_iter().map(|id| id.0).collect() },
            codestory_runtime::proof_qualification_support::StepQualificationOutcome::FirstZeroSurvivor { gate, histogram } => Self::FirstZeroSurvivor { gate: gate.into(), histogram: histogram.into_iter().map(Into::into).collect() },
            codestory_runtime::proof_qualification_support::StepQualificationOutcome::CandidateLimitExceeded { maximum_candidate_edges, observed_candidate_edges_at_least } => Self::CandidateLimitExceeded { maximum_candidate_edges, observed_candidate_edges_at_least },
        }
    }
}

impl TryFrom<codestory_runtime::proof_qualification_support::StepQualificationTrace>
    for StepQualificationTraceV1
{
    type Error = anyhow::Error;

    fn try_from(
        value: codestory_runtime::proof_qualification_support::StepQualificationTrace,
    ) -> Result<Self> {
        let converted = Self {
            step_index: u64::try_from(value.step_index)
                .map_err(|_| anyhow::anyhow!("proof_availability_step_index_overflow"))?,
            candidate_edge_ids: value
                .candidate_edge_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            outcome: value.outcome.into(),
        };
        if !valid_step_trace(&converted) {
            bail!("proof_availability_step_trace_invalid")
        }
        Ok(converted)
    }
}

impl TryFrom<codestory_runtime::proof_qualification_support::FinalizationTrace>
    for FinalizationTraceV1
{
    type Error = anyhow::Error;

    fn try_from(
        value: codestory_runtime::proof_qualification_support::FinalizationTrace,
    ) -> Result<Self> {
        match value {
            codestory_runtime::proof_qualification_support::FinalizationTrace::NotRun => {
                Ok(Self::NotRun)
            }
            codestory_runtime::proof_qualification_support::FinalizationTrace::Complete {
                projection_bytes,
            } => Ok(Self::Complete {
                projection_bytes: u64::try_from(projection_bytes)
                    .map_err(|_| anyhow::anyhow!("proof_availability_projection_bytes_overflow"))?,
            }),
            codestory_runtime::proof_qualification_support::FinalizationTrace::Failed(failure) => {
                Ok(Self::Failed {
                    failure: failure.into(),
                })
            }
        }
    }
}

impl TryFrom<codestory_runtime::proof_qualification_support::ProofQualificationTrace>
    for ProofQualificationTraceV1
{
    type Error = anyhow::Error;

    fn try_from(
        value: codestory_runtime::proof_qualification_support::ProofQualificationTrace,
    ) -> Result<Self> {
        Ok(Self {
            selectors: value
                .selectors
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_>>()?,
            selector_early_return: value.selector_early_return,
            steps: value
                .steps
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_>>()?,
            finalization: value.finalization.try_into()?,
        })
    }
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
    pub qualification_source_commit: String,
    pub qualification_source_tree: String,
    pub recorded_at: String,
    pub invocation: QualificationInvocationIdentityV1,
    pub projects: Vec<ProjectMaterializationEvidenceV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QualificationInvocationIdentityV1 {
    pub binary_name: String,
    pub operation: QualificationOperationV1,
    pub profile: QualificationProfileV1,
    pub corpus_sha256: String,
    pub thresholds_sha256: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QualificationOperationV1 {
    Run,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QualificationProfileV1 {
    LocalCoreOnly,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum MaterializationFreshnessV1 {
    Fresh,
    Stale,
    Missing,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentIdentityV1 {
    pub project_id: String,
    pub core_generation_id: String,
    pub core_run_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectMaterializationEvidenceV1 {
    pub repository_id: String,
    pub source_head: String,
    pub source_tree: String,
    pub store_schema: String,
    pub file_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    pub freshness: MaterializationFreshnessV1,
    pub database_sha256: String,
    pub core_generation: u64,
    pub identity: EnvironmentIdentityV1,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProductDispositionKindV1 {
    ContractProven,
    Unknown,
    CertifiedAbsence,
    Invalid,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActualProductResultV1 {
    ContractProven {
        contract_digest: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        receipts: Vec<ProjectedReceiptReferenceV1>,
    },
    ContractRefuted {
        contract_digest: String,
        basis: ProductRefutationBasisV1,
    },
    Unknown {
        contract_digest: String,
        gaps: Vec<ActualProofGapV1>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        connected_receipts: Vec<ProjectedReceiptReferenceV1>,
    },
    Unavailable {
        contract_digest: String,
        reasons: Vec<ProductUnavailableReasonV1>,
    },
    Invalid {
        failure: ProductToolFailureV1,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProductRefutationBasisV1 {
    PositiveContradiction {
        step_index: u8,
        prohibition_index: u8,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        connected_receipts: Vec<ProjectedReceiptReferenceV1>,
    },
    CertifiedAbsence {
        step_index: u8,
        extractor_capability_receipt_id: String,
        enumeration_receipt_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        connected_receipts: Vec<ProjectedReceiptReferenceV1>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectedReceiptReferenceV1 {
    pub receipt_id: String,
    pub edge_id: String,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActualProofGapV1 {
    SelectorMissing { selector_index: u8 },
    SelectorAmbiguous { selector_index: u8 },
    NonCallableSelector { selector_index: u8 },
    DirectCallMissing { step_index: u8 },
    RecursiveCallNotRepresentable { step_index: u8 },
    SourceWindowTooLarge { step_index: u8 },
    InvalidUtf8 { step_index: u8 },
    SourceLineOutOfRange { step_index: u8 },
    EdgeContainmentUnproven { step_index: u8 },
    MissingDirectCallReceipt { step_index: u8 },
    ReceiptOrEdgeAlreadyUsed { step_index: u8 },
    ProjectionExclusionConflictsWithRequiredReceipt { step_index: u8 },
    OutputBudgetExceeded,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProductUnavailableReasonV1 {
    ValidatedContractHashMismatch,
    PublicationPinMismatch,
    SourceNotBoundToPublication,
    ProofFactsUnavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProductFailureStageV1 {
    ContractValidation,
    ToolExecution,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductToolFailureV1 {
    pub stage: ProductFailureStageV1,
    pub code: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductDispositionV1 {
    /// Task-9 compatibility summary. Threshold evaluation consumes this only
    /// after validation proves it is the deterministic coarse projection of
    /// `actual`; it must never infer an exact refutation basis from this field.
    pub kind: ProductDispositionKindV1,
    pub gaps: Vec<TypedGapV1>,
    pub authoritative_receipts: Vec<ReceiptReferenceV1>,
    pub actual: ActualProductResultV1,
}

pub(crate) fn observed_product_disposition_to_report(
    observed: &codestory_runtime::proof_qualification_support::ObservedIntegratedProjectedCallPathResult,
) -> Result<ProductDispositionV1> {
    let result = match &observed.result {
        Ok(result) => result,
        Err(error) => {
            return Ok(ProductDispositionV1 {
                kind: ProductDispositionKindV1::Invalid,
                gaps: Vec::new(),
                authoritative_receipts: Vec::new(),
                actual: ActualProductResultV1::Invalid {
                    failure: ProductToolFailureV1 {
                        stage: ProductFailureStageV1::ToolExecution,
                        code: error.code.clone(),
                    },
                },
            });
        }
    };
    let root = match &result.projection {
        product_proof::InternalProjection::Complete { root, .. }
        | product_proof::InternalProjection::BudgetExceeded { root, .. } => root,
    };
    product_disposition_from_projection(root)
}

pub(crate) fn invalid_contract_report(code: impl Into<String>) -> ProductDispositionV1 {
    ProductDispositionV1 {
        kind: ProductDispositionKindV1::Invalid,
        gaps: Vec::new(),
        authoritative_receipts: Vec::new(),
        actual: ActualProductResultV1::Invalid {
            failure: ProductToolFailureV1 {
                stage: ProductFailureStageV1::ContractValidation,
                code: code.into(),
            },
        },
    }
}

pub(crate) fn product_disposition_from_projection(root: &Value) -> Result<ProductDispositionV1> {
    let disposition = root
        .get("disposition")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_product_disposition_missing"))?;
    let contract_digest = disposition
        .get("contract_digest")
        .and_then(Value::as_str)
        .filter(|value| hash(value))
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_contract_digest_invalid"))?;
    let kind = disposition
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_product_disposition_kind_missing"))?;
    let actual = match kind {
        "contract_proven" => ActualProductResultV1::ContractProven {
            contract_digest,
            receipts: projected_receipts(disposition.get("receipts"))?,
        },
        "contract_refuted" => {
            let refutation = disposition
                .get("refutation")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_refutation_missing"))?;
            let step_index = projected_u8(refutation, "step_index")?;
            let connected_receipts = projected_receipts(refutation.get("connected_receipts"))?;
            let basis = match refutation.get("kind").and_then(Value::as_str) {
                Some("prohibited_scope_traversal") => {
                    ProductRefutationBasisV1::PositiveContradiction {
                        step_index,
                        prohibition_index: projected_u8(refutation, "prohibition_index")?,
                        connected_receipts,
                    }
                }
                Some("certified_absence") => ProductRefutationBasisV1::CertifiedAbsence {
                    step_index,
                    extractor_capability_receipt_id: projected_string(
                        refutation,
                        "extractor_capability_receipt_id",
                    )?,
                    enumeration_receipt_id: projected_string(
                        refutation,
                        "untruncated_enumeration_receipt_id",
                    )?,
                    connected_receipts,
                },
                _ => bail!("proof_availability_refutation_kind_invalid"),
            };
            ActualProductResultV1::ContractRefuted {
                contract_digest,
                basis,
            }
        }
        "unknown" => ActualProductResultV1::Unknown {
            contract_digest,
            gaps: disposition
                .get("gaps")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default(),
            connected_receipts: projected_receipts(disposition.get("connected_receipts"))?,
        },
        "unavailable" => ActualProductResultV1::Unavailable {
            contract_digest,
            reasons: disposition
                .get("reasons")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default(),
        },
        _ => bail!("proof_availability_product_disposition_kind_invalid"),
    };
    let projected = match &actual {
        ActualProductResultV1::ContractProven { receipts, .. } => receipts.clone(),
        ActualProductResultV1::ContractRefuted { basis, .. } => match basis {
            ProductRefutationBasisV1::PositiveContradiction {
                connected_receipts, ..
            }
            | ProductRefutationBasisV1::CertifiedAbsence {
                connected_receipts, ..
            } => connected_receipts.clone(),
        },
        ActualProductResultV1::Unknown {
            connected_receipts, ..
        } => connected_receipts.clone(),
        ActualProductResultV1::Unavailable { .. } | ActualProductResultV1::Invalid { .. } => {
            Vec::new()
        }
    };
    let authoritative_receipts = projected
        .into_iter()
        .map(|reference| {
            Ok(ReceiptReferenceV1 {
                receipt_id: reference.receipt_id,
                edge_id: reference
                    .edge_id
                    .parse()
                    .map_err(|_| anyhow::anyhow!("proof_availability_receipt_edge_id_invalid"))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let (summary_kind, gaps) = match &actual {
        ActualProductResultV1::ContractProven { .. } => {
            (ProductDispositionKindV1::ContractProven, Vec::new())
        }
        ActualProductResultV1::ContractRefuted {
            basis: ProductRefutationBasisV1::CertifiedAbsence { .. },
            ..
        } => (ProductDispositionKindV1::CertifiedAbsence, Vec::new()),
        ActualProductResultV1::ContractRefuted { .. } => {
            (ProductDispositionKindV1::Unknown, Vec::new())
        }
        ActualProductResultV1::Unknown { gaps, .. } => (
            ProductDispositionKindV1::Unknown,
            gaps.iter()
                .map(coarse_gap)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
        ActualProductResultV1::Unavailable { .. } => {
            (ProductDispositionKindV1::Unknown, Vec::new())
        }
        ActualProductResultV1::Invalid { .. } => (ProductDispositionKindV1::Invalid, Vec::new()),
    };
    Ok(ProductDispositionV1 {
        kind: summary_kind,
        gaps,
        authoritative_receipts,
        actual,
    })
}

fn projected_receipts(value: Option<&Value>) -> Result<Vec<ProjectedReceiptReferenceV1>> {
    value
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map(|value| value.unwrap_or_default())
        .map_err(Into::into)
}

fn projected_u8(object: &serde_json::Map<String, Value>, field: &str) -> Result<u8> {
    u8::try_from(
        object
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_projection_field_invalid"))?,
    )
    .map_err(Into::into)
}

fn projected_string(object: &serde_json::Map<String, Value>, field: &str) -> Result<String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !empty(value))
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_projection_field_invalid"))
}

fn coarse_gap(gap: &ActualProofGapV1) -> TypedGapV1 {
    match gap {
        ActualProofGapV1::SelectorMissing { .. } => TypedGapV1::SelectorMissing,
        ActualProofGapV1::SelectorAmbiguous { .. } => TypedGapV1::SelectorAmbiguous,
        ActualProofGapV1::DirectCallMissing { .. }
        | ActualProofGapV1::MissingDirectCallReceipt { .. } => TypedGapV1::RelationMissing,
        ActualProofGapV1::RecursiveCallNotRepresentable { .. }
        | ActualProofGapV1::ReceiptOrEdgeAlreadyUsed { .. } => TypedGapV1::Recursion,
        ActualProofGapV1::NonCallableSelector { .. }
        | ActualProofGapV1::SourceWindowTooLarge { .. }
        | ActualProofGapV1::InvalidUtf8 { .. }
        | ActualProofGapV1::SourceLineOutOfRange { .. }
        | ActualProofGapV1::EdgeContainmentUnproven { .. } => TypedGapV1::SourceBinding,
        ActualProofGapV1::ProjectionExclusionConflictsWithRequiredReceipt { .. }
        | ActualProofGapV1::OutputBudgetExceeded => TypedGapV1::ProjectionBudget,
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptReferenceV1 {
    pub receipt_id: String,
    pub edge_id: i64,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
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
    pub elapsed_ns: u64,
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
impl TryFrom<&str> for McpRevisionV1 {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "2024-11-05" => Ok(Self::V2024_11_05),
            "2025-03-26" => Ok(Self::V2025_03_26),
            "2025-06-18" => Ok(Self::V2025_06_18),
            "2025-11-25" => Ok(Self::V2025_11_25),
            _ => bail!("proof_availability_mcp_revision_invalid"),
        }
    }
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
impl
    TryFrom<
        std::result::Result<
            Vec<codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement>,
            codestory_cli::proof_qualification_support::ProofQualificationTransportError,
        >,
    > for TransportEvidenceV1
{
    type Error = anyhow::Error;

    fn try_from(
        value: std::result::Result<
            Vec<codestory_cli::proof_qualification_support::RevisionNativeToolResultMeasurement>,
            codestory_cli::proof_qualification_support::ProofQualificationTransportError,
        >,
    ) -> Result<Self> {
        match value {
            Ok(measurements) => Ok(Self::Measurements {
                measurements: TransportMeasurementSetV1 {
                    measurements: measurements
                        .into_iter()
                        .map(|measurement| {
                            Ok(TransportMeasurementV1 {
                                revision: measurement.revision.as_str().try_into()?,
                                actual_bytes: u64::try_from(measurement.byte_length).map_err(
                                    |_| anyhow::anyhow!("proof_availability_transport_bytes_overflow"),
                                )?,
                                elapsed_ns: measurement.elapsed_ns,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                },
            }),
            Err(error) => Ok(Self::Error {
                error: match error {
                    codestory_cli::proof_qualification_support::ProofQualificationTransportError::Serialization(message) => TransportErrorV1::Serialization { message },
                    codestory_cli::proof_qualification_support::ProofQualificationTransportError::InvalidProjection(projection) => TransportErrorV1::InvalidProjection { projection },
                    codestory_cli::proof_qualification_support::ProofQualificationTransportError::OutputSchemaViolation => TransportErrorV1::OutputSchemaViolation {},
                    codestory_cli::proof_qualification_support::ProofQualificationTransportError::ResultExceedsBudget { maximum_bytes, actual_bytes } => TransportErrorV1::ResultExceedsBudget {
                        maximum_bytes: u64::try_from(maximum_bytes).map_err(|_| anyhow::anyhow!("proof_availability_transport_bytes_overflow"))?,
                        actual_bytes: u64::try_from(actual_bytes).map_err(|_| anyhow::anyhow!("proof_availability_transport_bytes_overflow"))?,
                    },
                },
            }),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NegativeMutationResultV1 {
    pub mutation_id: String,
    pub path_id: String,
    pub kind: NegativeMutationKindV1,
    pub step_index: u8,
    pub mutated_spec: CallPathSpecV1,
    pub contract_proven: bool,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptMismatchFieldV1 {
    Caller,
    CallsiteLine,
    CallsiteWindow,
    Target,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptOracleComparisonV1 {
    Exact {
        oracle_step_index: u8,
        oracle_step: ReceiptOracleStepV1,
    },
    Mismatched {
        oracle_step_index: u8,
        oracle_step: ReceiptOracleStepV1,
        mismatches: Vec<ReceiptMismatchFieldV1>,
    },
}
impl ReceiptOracleComparisonV1 {
    fn oracle_step_index(&self) -> u8 {
        match self {
            Self::Exact {
                oracle_step_index, ..
            }
            | Self::Mismatched {
                oracle_step_index, ..
            } => *oracle_step_index,
        }
    }

    fn oracle_step(&self) -> &ReceiptOracleStepV1 {
        match self {
            Self::Exact { oracle_step, .. } | Self::Mismatched { oracle_step, .. } => oracle_step,
        }
    }

    fn is_exact(&self) -> bool {
        matches!(self, Self::Exact { .. })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptOracleStepV1 {
    pub caller_symbol: String,
    pub callsite_line: u32,
    pub receipt_line_window: OracleSourceRangeV1,
    pub target_symbol: String,
}

impl From<&OracleStepV1> for ReceiptOracleStepV1 {
    fn from(value: &OracleStepV1) -> Self {
        Self {
            caller_symbol: value.caller.symbol.clone(),
            callsite_line: value.callsite_line,
            receipt_line_window: value.receipt_line_window.clone(),
            target_symbol: value.target.symbol.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedLineWindowV1 {
    pub kind: String,
    pub project_file_components: Vec<String>,
    pub indexed_sha256: String,
    pub observed_sha256: String,
    pub byte_start: u64,
    pub byte_end: u64,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedNodeIdentityV1 {
    pub project_id: String,
    pub core_generation_id: String,
    pub core_run_id: String,
    pub node_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedNodeIdentityV1 {
    pub pinned: PinnedNodeIdentityV1,
    pub canonical_id: String,
    pub qualified_name: String,
    pub project_file_components: Vec<String>,
}
impl From<&codestory_agent::proof_qualification_support::ResolvedNodeIdentity>
    for ResolvedNodeIdentityV1
{
    fn from(value: &codestory_agent::proof_qualification_support::ResolvedNodeIdentity) -> Self {
        Self {
            pinned: PinnedNodeIdentityV1 {
                project_id: value.pinned.project_id.clone(),
                core_generation_id: value.pinned.core_generation_id.clone(),
                core_run_id: value.pinned.core_run_id.clone(),
                node_id: value.pinned.node_id.clone(),
            },
            canonical_id: value.canonical_id.clone(),
            qualified_name: value.qualified_name.clone(),
            project_file_components: value.project_file_components.clone(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptCertaintyV1 {
    Certain,
}
impl TryFrom<codestory_contracts::graph::ResolutionCertainty> for ReceiptCertaintyV1 {
    type Error = anyhow::Error;

    fn try_from(value: codestory_contracts::graph::ResolutionCertainty) -> Result<Self> {
        match value {
            codestory_contracts::graph::ResolutionCertainty::Certain => Ok(Self::Certain),
            _ => bail!("proof_availability_receipt_certainty_invalid"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallableContainmentEvidenceV1 {
    pub file_node_id: i64,
    pub owner_node_id: i64,
    pub start_line: u32,
    pub end_line: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedReceiptV1 {
    pub receipt_id: String,
    pub step_index: u8,
    pub edge_id: i64,
    pub source: ResolvedNodeIdentityV1,
    pub target: ResolvedNodeIdentityV1,
    pub certainty: ReceiptCertaintyV1,
    pub callsite_identity: String,
    pub callsite_line: u32,
    pub containment: CallableContainmentEvidenceV1,
    pub line_window: ObservedLineWindowV1,
    pub oracle_comparison: ReceiptOracleComparisonV1,
}
impl ObservedReceiptV1 {
    pub fn from_task6(
        step_index: u8,
        receipt: &codestory_agent::proof_qualification_support::IndexedCallEdgeReceipt,
        oracle_comparison: ReceiptOracleComparisonV1,
    ) -> Result<Self> {
        let observed = Self {
            receipt_id: receipt.receipt.receipt_id.clone(),
            step_index,
            edge_id: receipt
                .receipt
                .edge_id
                .parse()
                .map_err(|_| anyhow::anyhow!("proof_availability_receipt_edge_id_invalid"))?,
            source: (&receipt.source).into(),
            target: (&receipt.target).into(),
            certainty: receipt.certainty.try_into()?,
            callsite_identity: receipt.callsite_identity.clone(),
            callsite_line: receipt.line_window.anchor_line,
            containment: CallableContainmentEvidenceV1 {
                file_node_id: receipt.containment.file_node_id.0,
                owner_node_id: receipt.containment.owner_node_id.0,
                start_line: receipt.containment.start_line,
                end_line: receipt.containment.end_line,
            },
            line_window: ObservedLineWindowV1 {
                kind: receipt.line_window.kind.to_owned(),
                project_file_components: receipt.line_window.project_file_components.clone(),
                indexed_sha256: receipt.line_window.indexed_sha256.clone(),
                observed_sha256: receipt.line_window.observed_sha256.clone(),
                byte_start: u64::try_from(receipt.line_window.byte_start)
                    .map_err(|_| anyhow::anyhow!("proof_availability_receipt_window_overflow"))?,
                byte_end: u64::try_from(receipt.line_window.byte_end)
                    .map_err(|_| anyhow::anyhow!("proof_availability_receipt_window_overflow"))?,
                text: receipt.line_window.text.clone(),
            },
            oracle_comparison,
        };
        if observed.oracle_comparison.oracle_step_index() != step_index
            || !valid_observed_receipt_shape(&observed)
        {
            bail!("proof_availability_receipt_oracle_comparison_invalid")
        }
        Ok(observed)
    }
}

pub(crate) fn compare_task6_receipt_to_oracle(
    step_index: u8,
    receipt: &codestory_agent::proof_qualification_support::IndexedCallEdgeReceipt,
    oracle: &OracleStepV1,
) -> Result<ReceiptOracleComparisonV1> {
    let oracle_step = ReceiptOracleStepV1::from(oracle);
    let mut mismatches = Vec::new();
    if receipt.source.qualified_name != oracle_step.caller_symbol {
        mismatches.push(ReceiptMismatchFieldV1::Caller);
    }
    if receipt.line_window.anchor_line != oracle_step.callsite_line {
        mismatches.push(ReceiptMismatchFieldV1::CallsiteLine);
    }
    let observed_start = u64::try_from(receipt.line_window.byte_start)?;
    let observed_end = u64::try_from(receipt.line_window.byte_end)?;
    if receipt.line_window.project_file_components.join("/") != oracle_step.receipt_line_window.path
        || receipt.line_window.indexed_sha256 != oracle_step.receipt_line_window.sha256
        || receipt.line_window.observed_sha256 != oracle_step.receipt_line_window.sha256
        || observed_start != oracle_step.receipt_line_window.start_byte
        || observed_end != oracle_step.receipt_line_window.end_byte
    {
        mismatches.push(ReceiptMismatchFieldV1::CallsiteWindow);
    }
    if receipt.target.qualified_name != oracle_step.target_symbol {
        mismatches.push(ReceiptMismatchFieldV1::Target);
    }
    Ok(if mismatches.is_empty() {
        ReceiptOracleComparisonV1::Exact {
            oracle_step_index: step_index,
            oracle_step,
        }
    } else {
        ReceiptOracleComparisonV1::Mismatched {
            oracle_step_index: step_index,
            oracle_step,
            mismatches,
        }
    })
}

pub(crate) fn observed_receipt_from_task6(
    step_index: u8,
    receipt: &codestory_agent::proof_qualification_support::IndexedCallEdgeReceipt,
    oracle: &OracleStepV1,
) -> Result<ObservedReceiptV1> {
    ObservedReceiptV1::from_task6(
        step_index,
        receipt,
        compare_task6_receipt_to_oracle(step_index, receipt, oracle)?,
    )
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MissingOracleStepV1 {
    pub step_index: u8,
    pub oracle_step: ReceiptOracleStepV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiptEvidenceV1 {
    pub observed_receipts: Vec<ObservedReceiptV1>,
    pub missing_oracle_steps: Vec<MissingOracleStepV1>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptEvidenceBuildOutcomeV1 {
    Complete(ReceiptEvidenceV1),
    LimitExceeded {
        maximum_observed_receipts: usize,
        observed_receipts_at_least: usize,
    },
}
impl ReceiptEvidenceV1 {
    pub fn bounded(
        observed_receipts: Vec<ObservedReceiptV1>,
        missing_oracle_steps: Vec<MissingOracleStepV1>,
    ) -> ReceiptEvidenceBuildOutcomeV1 {
        if observed_receipts.len() > MAX_OBSERVED_RECEIPTS_PER_CASE {
            return ReceiptEvidenceBuildOutcomeV1::LimitExceeded {
                maximum_observed_receipts: MAX_OBSERVED_RECEIPTS_PER_CASE,
                observed_receipts_at_least: observed_receipts.len(),
            };
        }
        ReceiptEvidenceBuildOutcomeV1::Complete(Self {
            observed_receipts,
            missing_oracle_steps,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseReportV1 {
    pub case_id: String,
    pub repository_id: String,
    pub product_disposition: ProductDispositionV1,
    pub actionable_exact_gap: Option<TypedGapV1>,
    pub warm_end_to_end_ms: u64,
    pub stage_durations_ms: StageDurationsV1,
    pub attempted_step_count: u8,
    pub unclassified_step_indices: Vec<u8>,
    pub receipt_evidence: ReceiptEvidenceV1,
    pub complete_projection_bytes: u64,
    pub transport: TransportEvidenceV1,
    pub negative_mutations: Vec<NegativeMutationResultV1>,
    pub proof_trace: ProofQualificationTraceV1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CaseReceiptMetricsV1 {
    pub observed_receipt_count: u64,
    pub authoritative_receipt_count: u64,
    pub authoritative_exact_receipt_count: u64,
    pub false_positive_receipt_count: u64,
    pub missing_oracle_step_count: u8,
    pub exact_oracle_step_count: u8,
    pub all_authoritative_receipts_exact: bool,
    pub oracle_receipts_exact: bool,
    pub proven_step_precision_milli: u16,
    pub proven_step_recall_milli: u16,
    pub proven_prefix_length: u8,
    pub diagnostic_candidate_count: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CaseEvaluableFactsV1 {
    pub contract_proven_supported: bool,
    pub false_contract_proven: bool,
    pub product_disposition_matches_evidence: bool,
}
impl CaseReportV1 {
    pub fn receipt_metrics(&self) -> Result<CaseReceiptMetricsV1> {
        let authoritative = self
            .product_disposition
            .authoritative_receipts
            .iter()
            .map(|reference| {
                self.receipt_evidence
                    .observed_receipts
                    .iter()
                    .find(|receipt| {
                        receipt.receipt_id == reference.receipt_id
                            && receipt.edge_id == reference.edge_id
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("proof_availability_authoritative_receipt_missing")
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let exact_authoritative = authoritative
            .iter()
            .filter(|receipt| receipt.oracle_comparison.is_exact())
            .copied()
            .collect::<Vec<_>>();
        let exact_steps = exact_authoritative
            .iter()
            .map(|receipt| receipt.step_index)
            .collect::<BTreeSet<_>>();
        let mut proven_prefix_length = 0u8;
        while proven_prefix_length < self.attempted_step_count
            && exact_steps.contains(&proven_prefix_length)
        {
            proven_prefix_length += 1;
        }
        let authoritative_receipt_count = u64::try_from(authoritative.len())?;
        let authoritative_exact_receipt_count = u64::try_from(exact_authoritative.len())?;
        let exact_oracle_step_count = u8::try_from(exact_steps.len())?;
        let missing_oracle_step_count =
            u8::try_from(self.receipt_evidence.missing_oracle_steps.len())?;
        let all_authoritative_receipts_exact =
            authoritative_receipt_count == authoritative_exact_receipt_count;
        Ok(CaseReceiptMetricsV1 {
            observed_receipt_count: u64::try_from(self.receipt_evidence.observed_receipts.len())?,
            authoritative_receipt_count,
            authoritative_exact_receipt_count,
            false_positive_receipt_count: u64::try_from(
                self.receipt_evidence
                    .observed_receipts
                    .iter()
                    .filter(|receipt| !receipt.oracle_comparison.is_exact())
                    .count(),
            )?,
            missing_oracle_step_count,
            exact_oracle_step_count,
            all_authoritative_receipts_exact,
            oracle_receipts_exact: all_authoritative_receipts_exact
                && exact_oracle_step_count == self.attempted_step_count
                && missing_oracle_step_count == 0,
            proven_step_precision_milli: ratio_milli(
                authoritative_exact_receipt_count,
                authoritative_receipt_count,
            )?,
            proven_step_recall_milli: ratio_milli(
                u64::from(exact_oracle_step_count),
                u64::from(self.attempted_step_count),
            )?,
            proven_prefix_length,
            diagnostic_candidate_count: self
                .proof_trace
                .steps
                .iter()
                .try_fold(0u64, |total, step| {
                    total.checked_add(u64::try_from(step.candidate_edge_ids.len()).ok()?)
                })
                .ok_or_else(|| anyhow::anyhow!("proof_availability_candidate_count_overflow"))?,
        })
    }

    pub fn evaluable_facts(&self) -> Result<CaseEvaluableFactsV1> {
        let metrics = self.receipt_metrics()?;
        let contract_proven_supported = self.product_disposition.gaps.is_empty()
            && self.actionable_exact_gap.is_none()
            && metrics.authoritative_receipt_count > 0
            && metrics.oracle_receipts_exact
            && metrics.proven_prefix_length == self.attempted_step_count;
        let product_disposition_matches_evidence = match &self.product_disposition.actual {
            ActualProductResultV1::ContractProven { .. } => contract_proven_supported,
            ActualProductResultV1::Unknown { .. } => {
                metrics.proven_prefix_length < self.attempted_step_count
                    && (!self.product_disposition.gaps.is_empty()
                        || self.actionable_exact_gap.is_some())
            }
            ActualProductResultV1::ContractRefuted { basis, .. } => match basis {
                ProductRefutationBasisV1::PositiveContradiction { step_index, .. } => {
                    metrics.authoritative_receipt_count > 0
                        && metrics.proven_prefix_length >= *step_index
                }
                ProductRefutationBasisV1::CertifiedAbsence { .. } => true,
            },
            ActualProductResultV1::Unavailable { reasons, .. } => !reasons.is_empty(),
            ActualProductResultV1::Invalid { failure } => !empty(&failure.code),
        };
        Ok(CaseEvaluableFactsV1 {
            contract_proven_supported,
            false_contract_proven: matches!(
                self.product_disposition.kind,
                ProductDispositionKindV1::ContractProven
            ) && !contract_proven_supported,
            product_disposition_matches_evidence,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct QualificationReceiptMetricsV1 {
    pub observed_receipt_count: u64,
    pub authoritative_receipt_count: u64,
    pub authoritative_exact_receipt_count: u64,
    pub false_positive_receipt_count: u64,
    pub missing_oracle_step_count: u16,
    pub exact_oracle_step_count: u16,
    pub all_authoritative_receipts_exact: bool,
    pub positive_step_precision_milli: u16,
    pub positive_step_recall_milli: u16,
    pub proven_prefix_step_count: u16,
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
impl ActivationDecisionV1 {
    pub fn validate(&self) -> Result<()> {
        if !unique(self.failed_gates.iter().map(|gate| gate.gate_id.as_str()))
            || self
                .failed_gates
                .iter()
                .any(|gate| empty(&gate.gate_id) || !valid_gate_detail(&gate.kind, &gate.detail))
            || (matches!(self.outcome, ActivationOutcomeV1::DelayFullV3Cut)
                && !self.failed_gates.iter().any(|gate| {
                    matches!(gate.detail, GateFailureDetailV1::SourceDependency { .. })
                }))
        {
            bail!("proof_availability_decision_invalid")
        }
        Ok(())
    }
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
}
impl QualificationSummaryV1 {
    pub fn from_json(value: Value) -> Result<Self> {
        let value: Self = serde_json::from_value(value)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        let funnel_total = self
            .failure_funnel
            .buckets
            .iter()
            .try_fold(0u128, |total, bucket| {
                if bucket.count > 312 {
                    return None;
                }
                total.checked_add(bucket.count)
            });
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
            || self.environment.binary_sha256 != self.provenance.binary_sha256
            || self.environment.qualification_source_commit != self.provenance.source_commit
            || self.environment.qualification_source_tree != self.provenance.source_tree
            || !rfc3339_utc(&self.environment.recorded_at)
            || self.environment.invocation.binary_name != "codestory-proof-availability"
            || self.environment.invocation.operation != QualificationOperationV1::Run
            || self.environment.invocation.profile != QualificationProfileV1::LocalCoreOnly
            || self.environment.invocation.corpus_sha256 != self.provenance.corpus_sha256
            || self.environment.invocation.thresholds_sha256 != self.provenance.thresholds_sha256
            || !sanitized_environment(&self.environment)
            || self.environment.projects.len() != 4
            || !unique(
                self.environment
                    .projects
                    .iter()
                    .map(|project| project.repository_id.as_str()),
            )
            || self
                .environment
                .projects
                .iter()
                .map(|project| {
                    (
                        project.identity.project_id.as_str(),
                        project.identity.core_generation_id.as_str(),
                        project.identity.core_run_id.as_str(),
                    )
                })
                .collect::<BTreeSet<_>>()
                .len()
                != self.environment.projects.len()
            || self.inventory.len() != 4
            || self.trails.len() != 4
            || self.cases.len() != 120
            || !unique(
                self.inventory
                    .iter()
                    .map(|inventory| inventory.repository_id.as_str()),
            )
            || !unique(self.trails.iter().map(|trail| trail.repository_id.as_str()))
            || !unique(self.cases.iter().map(|case| case.case_id.as_str()))
            || self.failure_funnel.attempted_positive_steps != 312
            || self.failure_funnel.classified_positive_steps > 312
            || self.failure_funnel.unclassified_positive_steps > 312
            || u32::from(self.failure_funnel.classified_positive_steps)
                + u32::from(self.failure_funnel.unclassified_positive_steps)
                != 312
            || funnel_total != Some(u128::from(self.failure_funnel.classified_positive_steps))
            || !self
                .failure_funnel
                .buckets
                .iter()
                .all(|bucket| valid_funnel_outcome(&bucket.outcome))
        {
            bail!("proof_availability_summary_invalid")
        }
        if self.provenance.results_sha256
            != results_evidence_sha256(
                &self.environment,
                &self.inventory,
                &self.trails,
                &self.cases,
                &self.failure_funnel,
            )?
        {
            bail!("proof_availability_results_digest_invalid")
        }
        for project in &self.environment.projects {
            if !commit(&project.source_head)
                || !hash(&project.source_tree)
                || empty(&project.store_schema)
                || !hash(&project.database_sha256)
                || empty(&project.identity.project_id)
                || empty(&project.identity.core_generation_id)
                || empty(&project.identity.core_run_id)
            {
                bail!("proof_availability_materialization_invalid")
            }
        }
        let project_ids = self
            .environment
            .projects
            .iter()
            .map(|project| project.repository_id.as_str())
            .collect::<BTreeSet<_>>();
        if !self
            .inventory
            .iter()
            .all(|inventory| project_ids.contains(inventory.repository_id.as_str()))
            || !self
                .trails
                .iter()
                .all(|trail| project_ids.contains(trail.repository_id.as_str()))
        {
            bail!("proof_availability_report_project_set_invalid")
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
        let mut cases_per_project = BTreeMap::<&str, usize>::new();
        let mut attempted_total = 0u16;
        let mut expected_funnel = BTreeMap::<String, u128>::new();
        let mut expected_unclassified = 0u16;
        let mut expected_classified = 0u16;
        let mut mutation_ids = BTreeSet::new();
        for c in &self.cases {
            c.receipt_metrics()?;
            if empty(&c.case_id)
                || empty(&c.repository_id)
                || c.negative_mutations.len() != 2
                || c.attempted_step_count == 0
                || c.attempted_step_count > 6
                || usize::from(c.attempted_step_count)
                    != c.proof_trace.steps.len() + c.unclassified_step_indices.len()
                || !c
                    .proof_trace
                    .selectors
                    .iter()
                    .enumerate()
                    .all(|(index, selector)| selector.selector_index == index as u64)
                || c.proof_trace.selectors.len() != usize::from(c.attempted_step_count) + 1
                || !valid_selector_trace(&c.proof_trace)
                || !c.proof_trace.steps.iter().all(|step| {
                    step.step_index < u64::from(c.attempted_step_count) && valid_step_trace(step)
                })
                || !unique_u64(c.proof_trace.steps.iter().map(|step| step.step_index))
                || !unique_u8(c.unclassified_step_indices.iter().copied())
                || c.unclassified_step_indices
                    .iter()
                    .any(|index| *index >= c.attempted_step_count)
                || c.proof_trace.steps.iter().any(|step| {
                    c.unclassified_step_indices
                        .contains(&(step.step_index as u8))
                })
                || !valid_finalization(&c.proof_trace.finalization)
                || matches!(c.proof_trace.finalization, FinalizationTraceV1::NotRun)
                || !valid_case_finalization(c)
                || !valid_transport(&c.transport)
                || !unique(
                    c.negative_mutations
                        .iter()
                        .map(|mutation| mutation.mutation_id.as_str()),
                )
                || c.negative_mutations.iter().any(|mutation| {
                    mutation.path_id != c.case_id
                        || mutation.step_index >= c.attempted_step_count
                        || validate_spec_shape(&mutation.mutated_spec).is_err()
                })
                || !valid_receipts(
                    c,
                    self.environment
                        .projects
                        .iter()
                        .find(|project| project.repository_id == c.repository_id),
                )
                || !valid_disposition_structure(c)
            {
                bail!("proof_availability_case_invalid")
            }
            *cases_per_project
                .entry(c.repository_id.as_str())
                .or_default() += 1;
            attempted_total = attempted_total
                .checked_add(u16::from(c.attempted_step_count))
                .ok_or_else(|| anyhow::anyhow!("proof_availability_attempted_steps_overflow"))?;
            expected_unclassified = expected_unclassified
                .checked_add(
                    u16::try_from(c.unclassified_step_indices.len())
                        .map_err(|_| anyhow::anyhow!("proof_availability_unclassified_overflow"))?,
                )
                .ok_or_else(|| anyhow::anyhow!("proof_availability_unclassified_overflow"))?;
            for mutation in &c.negative_mutations {
                if !mutation_ids.insert(mutation.mutation_id.as_str()) {
                    bail!("proof_availability_mutation_result_duplicate")
                }
            }
            for step in &c.proof_trace.steps {
                let outcome = match &step.outcome {
                    StepQualificationOutcomeV1::Admitted { .. } => Some(FunnelOutcomeV1::Admitted),
                    StepQualificationOutcomeV1::FirstZeroSurvivor { gate, histogram } => {
                        Some(FunnelOutcomeV1::FirstZeroSurvivor {
                            gate: gate.clone(),
                            histogram: histogram.clone(),
                        })
                    }
                    StepQualificationOutcomeV1::CandidateLimitExceeded { .. } => None,
                };
                if let Some(outcome) = outcome {
                    expected_classified = expected_classified
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("proof_availability_classified_overflow"))?;
                    let key = serde_json::to_string(&outcome)?;
                    *expected_funnel.entry(key).or_default() += 1;
                } else {
                    expected_unclassified =
                        expected_unclassified.checked_add(1).ok_or_else(|| {
                            anyhow::anyhow!("proof_availability_unclassified_overflow")
                        })?;
                }
            }
        }
        if !self
            .cases
            .iter()
            .all(|case| project_ids.contains(case.repository_id.as_str()))
            || project_ids
                .iter()
                .any(|id| cases_per_project.get(id).copied() != Some(30))
            || attempted_total != 312
            || mutation_ids.len() != 240
            || expected_classified != self.failure_funnel.classified_positive_steps
            || expected_unclassified != self.failure_funnel.unclassified_positive_steps
        {
            bail!("proof_availability_report_evidence_totals_invalid")
        }
        let mut actual_funnel = BTreeMap::<String, u128>::new();
        for bucket in &self.failure_funnel.buckets {
            let key = serde_json::to_string(&bucket.outcome)?;
            if actual_funnel.insert(key, bucket.count).is_some() {
                bail!("proof_availability_funnel_bucket_duplicate")
            }
        }
        if expected_funnel != actual_funnel {
            bail!("proof_availability_funnel_evidence_mismatch")
        }
        Ok(())
    }

    pub fn receipt_metrics(&self) -> Result<QualificationReceiptMetricsV1> {
        let mut observed_receipt_count = 0u64;
        let mut authoritative_receipt_count = 0u64;
        let mut authoritative_exact_receipt_count = 0u64;
        let mut false_positive_receipt_count = 0u64;
        let mut missing_oracle_step_count = 0u16;
        let mut exact_oracle_step_count = 0u16;
        let mut proven_prefix_step_count = 0u16;
        let mut all_authoritative_receipts_exact = true;
        for case in &self.cases {
            let metrics = case.receipt_metrics()?;
            observed_receipt_count = observed_receipt_count
                .checked_add(metrics.observed_receipt_count)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_count_overflow"))?;
            authoritative_receipt_count = authoritative_receipt_count
                .checked_add(metrics.authoritative_receipt_count)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_count_overflow"))?;
            authoritative_exact_receipt_count = authoritative_exact_receipt_count
                .checked_add(metrics.authoritative_exact_receipt_count)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_count_overflow"))?;
            false_positive_receipt_count = false_positive_receipt_count
                .checked_add(metrics.false_positive_receipt_count)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_count_overflow"))?;
            missing_oracle_step_count = missing_oracle_step_count
                .checked_add(u16::from(metrics.missing_oracle_step_count))
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_count_overflow"))?;
            exact_oracle_step_count = exact_oracle_step_count
                .checked_add(u16::from(metrics.exact_oracle_step_count))
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_count_overflow"))?;
            proven_prefix_step_count = proven_prefix_step_count
                .checked_add(u16::from(metrics.proven_prefix_length))
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_count_overflow"))?;
            all_authoritative_receipts_exact &= metrics.all_authoritative_receipts_exact;
        }
        Ok(QualificationReceiptMetricsV1 {
            observed_receipt_count,
            authoritative_receipt_count,
            authoritative_exact_receipt_count,
            false_positive_receipt_count,
            missing_oracle_step_count,
            exact_oracle_step_count,
            all_authoritative_receipts_exact,
            positive_step_precision_milli: ratio_milli(
                authoritative_exact_receipt_count,
                authoritative_receipt_count,
            )?,
            positive_step_recall_milli: ratio_milli(u64::from(exact_oracle_step_count), 312)?,
            proven_prefix_step_count,
        })
    }

    pub fn validate_against_corpus(&self, corpus: &CorpusV1) -> Result<()> {
        self.validate()?;
        corpus.validate()?;
        if self.provenance.corpus_sha256 != canonical_corpus_sha256(corpus)?
            || self.provenance.thresholds_sha256 != corpus.thresholds_sha256
            || self.provenance.binary_sha256 != self.environment.binary_sha256
        {
            bail!("proof_availability_provenance_corpus_binding_invalid")
        }
        let cohorts = corpus
            .cohorts
            .iter()
            .map(|cohort| (cohort.repository_id.as_str(), cohort))
            .collect::<BTreeMap<_, _>>();
        if self.environment.projects.iter().any(|project| {
            let Some(cohort) = cohorts.get(project.repository_id.as_str()) else {
                return true;
            };
            project.source_head != cohort.commit || project.source_tree != cohort.source_tree_sha256
        }) {
            bail!("proof_availability_materialization_corpus_binding_invalid")
        }
        Ok(())
    }

    pub fn validate_against_oracle(
        &self,
        corpus: &CorpusV1,
        path_files: &[CohortPathFileV1],
    ) -> Result<()> {
        self.validate_against_corpus(corpus)?;
        corpus.validate_with_path_files(path_files)?;
        let paths = path_files
            .iter()
            .flat_map(|file| {
                file.paths
                    .iter()
                    .map(move |path| (path.case_id.as_str(), (file.repository_id.as_str(), path)))
            })
            .collect::<BTreeMap<_, _>>();
        let mut result_ids = BTreeSet::new();
        for case in &self.cases {
            let (repository_id, path) = paths
                .get(case.case_id.as_str())
                .ok_or_else(|| anyhow::anyhow!("proof_availability_case_oracle_missing"))?;
            if *repository_id != case.repository_id
                || usize::from(case.attempted_step_count) != path.spec.steps.len()
            {
                bail!("proof_availability_case_oracle_mismatch")
            }
            for receipt in &case.receipt_evidence.observed_receipts {
                let oracle_step_index = receipt.oracle_comparison.oracle_step_index();
                let Some(step) = path.oracle_steps.get(usize::from(oracle_step_index)) else {
                    bail!("proof_availability_receipt_oracle_missing")
                };
                if receipt.step_index != oracle_step_index
                    || receipt.oracle_comparison.oracle_step() != &ReceiptOracleStepV1::from(step)
                {
                    bail!("proof_availability_receipt_oracle_mismatch")
                }
            }
            for missing in &case.receipt_evidence.missing_oracle_steps {
                let Some(step) = path.oracle_steps.get(usize::from(missing.step_index)) else {
                    bail!("proof_availability_receipt_oracle_missing")
                };
                if missing.oracle_step != ReceiptOracleStepV1::from(step) {
                    bail!("proof_availability_receipt_oracle_mismatch")
                }
            }
            let mutations = path
                .negative_mutations
                .iter()
                .map(|mutation| (mutation.mutation_id.as_str(), mutation))
                .collect::<BTreeMap<_, _>>();
            for result in &case.negative_mutations {
                let mutation = mutations
                    .get(result.mutation_id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("proof_availability_mutation_oracle_missing"))?;
                if result.path_id != mutation.path_id
                    || result.kind != mutation.kind
                    || result.step_index != mutation.step_index
                    || result.mutated_spec != mutation.mutated_spec
                    || !result_ids.insert(result.mutation_id.as_str())
                {
                    bail!("proof_availability_mutation_oracle_mismatch")
                }
            }
        }
        if result_ids.len() != 240 {
            bail!("proof_availability_mutation_oracle_total_invalid")
        }
        Ok(())
    }

    pub fn validate_against_inputs(
        &self,
        corpus: &CorpusV1,
        thresholds: &ThresholdsV1,
    ) -> Result<()> {
        corpus.validate_against_thresholds(thresholds)?;
        self.validate_against_corpus(corpus)?;
        if self.provenance.thresholds_sha256 != canonical_thresholds_sha256(thresholds)? {
            bail!("proof_availability_provenance_threshold_binding_invalid")
        }
        Ok(())
    }
}

pub fn canonical_corpus_sha256(corpus: &CorpusV1) -> Result<String> {
    canonical_artifact_sha256(b"codestory.proof-availability-corpus/v1\0", corpus)
}

pub fn canonical_cohort_path_file_sha256(path_file: &CohortPathFileV1) -> Result<String> {
    canonical_artifact_sha256(
        b"codestory.proof-availability-cohort-path-file/v1\0",
        path_file,
    )
}

pub fn canonical_thresholds_sha256(thresholds: &ThresholdsV1) -> Result<String> {
    canonical_artifact_sha256(b"codestory.proof-availability-thresholds/v1\0", thresholds)
}

#[derive(Serialize)]
struct ResultsEvidenceV1<'a> {
    environment: &'a EnvironmentReportV1,
    inventory: &'a [InventoryReportV1],
    trails: &'a [TrailReportV1],
    cases: &'a [CaseReportV1],
    failure_funnel: &'a FailureFunnelReportV1,
}

pub(crate) fn results_evidence_sha256(
    environment: &EnvironmentReportV1,
    inventory: &[InventoryReportV1],
    trails: &[TrailReportV1],
    cases: &[CaseReportV1],
    failure_funnel: &FailureFunnelReportV1,
) -> Result<String> {
    let mut environment = environment.clone();
    environment
        .projects
        .sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    let mut inventory = inventory.to_vec();
    inventory.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    let mut trails = trails.to_vec();
    trails.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    let mut cases = cases.to_vec();
    cases.sort_by(|left, right| left.case_id.cmp(&right.case_id));
    let mut failure_funnel = failure_funnel.clone();
    failure_funnel.buckets.sort_by(|left, right| {
        canonical_artifact_bytes(&left.outcome)
            .expect("closed funnel outcome serializes")
            .cmp(
                &canonical_artifact_bytes(&right.outcome)
                    .expect("closed funnel outcome serializes"),
            )
    });
    canonical_artifact_sha256(
        b"codestory.proof-availability-results-evidence/v1\0",
        &ResultsEvidenceV1 {
            environment: &environment,
            inventory: &inventory,
            trails: &trails,
            cases: &cases,
            failure_funnel: &failure_funnel,
        },
    )
}

pub(crate) fn results_evidence_sha256_from_json(value: &Value) -> Result<String> {
    #[derive(Deserialize)]
    struct EvidenceFields {
        environment: EnvironmentReportV1,
        inventory: Vec<InventoryReportV1>,
        trails: Vec<TrailReportV1>,
        cases: Vec<CaseReportV1>,
        failure_funnel: FailureFunnelReportV1,
    }
    let evidence: EvidenceFields = serde_json::from_value(value.clone())?;
    results_evidence_sha256(
        &evidence.environment,
        &evidence.inventory,
        &evidence.trails,
        &evidence.cases,
        &evidence.failure_funnel,
    )
}

fn canonical_artifact_sha256<T: Serialize>(domain: &[u8], value: &T) -> Result<String> {
    let canonical = canonical_artifact_bytes(value)?;
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(canonical);
    Ok(format!("{:x}", digest.finalize()))
}

fn canonical_artifact_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    codestory_agent::proof_qualification_support::canonical_json_bytes(value)
        .map_err(|error| anyhow::anyhow!(error))
}

fn rfc3339_utc(value: &str) -> bool {
    let Some(without_z) = value.strip_suffix('Z') else {
        return false;
    };
    let Some((date_part, time_part)) = without_z.split_once('T') else {
        return false;
    };
    if !date(date_part) {
        return false;
    }
    let (whole_time, fraction) = time_part
        .split_once('.')
        .map_or((time_part, None), |(whole, fraction)| {
            (whole, Some(fraction))
        });
    let mut fields = whole_time.split(':');
    let parsed = (
        fields.next().and_then(|value| value.parse::<u8>().ok()),
        fields.next().and_then(|value| value.parse::<u8>().ok()),
        fields.next().and_then(|value| value.parse::<u8>().ok()),
    );
    let (Some(hour), Some(minute), Some(second)) = parsed else {
        return false;
    };
    fields.next().is_none()
        && hour <= 23
        && minute <= 59
        && second <= 60
        && fraction.is_none_or(|fraction| {
            !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn sanitized_environment(environment: &EnvironmentReportV1) -> bool {
    sanitized_atom(&environment.environment_id)
        && sanitized_atom(&environment.os)
        && sanitized_atom(&environment.architecture)
        && sanitized_atom(&environment.rust_host)
        && environment.projects.iter().all(|project| {
            sanitized_atom(&project.repository_id)
                && sanitized_schema_id(&project.store_schema)
                && sanitized_atom(&project.identity.project_id)
                && sanitized_atom(&project.identity.core_generation_id)
                && sanitized_atom(&project.identity.core_run_id)
        })
}

fn sanitized_atom(value: &str) -> bool {
    !empty(value)
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn sanitized_schema_id(value: &str) -> bool {
    !empty(value)
        && value.len() <= 128
        && !value.starts_with('/')
        && !value.starts_with('~')
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
}

fn valid_step_trace(trace: &StepQualificationTraceV1) -> bool {
    if trace.candidate_edge_ids.len() > MAX_CANDIDATE_EDGES_PER_STEP
        || !strictly_ascending(&trace.candidate_edge_ids)
    {
        return false;
    }
    let candidates = trace
        .candidate_edge_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    match &trace.outcome {
        StepQualificationOutcomeV1::Admitted { edge_ids } => {
            !edge_ids.is_empty()
                && strictly_ascending(edge_ids)
                && edge_ids.iter().all(|edge| candidates.contains(edge))
        }
        StepQualificationOutcomeV1::FirstZeroSurvivor { gate, histogram } => {
            (histogram.is_empty()
                && matches!(gate, CandidateGateV1::RawAdmission)
                && trace.candidate_edge_ids.is_empty())
                || (!histogram.is_empty()
                    && unique_i64(
                        histogram
                            .iter()
                            .flat_map(|bucket| bucket.edge_ids.iter().copied()),
                    )
                    && histogram.iter().all(|bucket| {
                        !bucket.edge_ids.is_empty()
                            && strictly_ascending(&bucket.edge_ids)
                            && bucket.edge_ids.iter().all(|edge| candidates.contains(edge))
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
        StepQualificationOutcomeV1::CandidateLimitExceeded {
            maximum_candidate_edges,
            observed_candidate_edges_at_least,
        } => {
            trace.candidate_edge_ids.len() == MAX_CANDIDATE_EDGES_PER_STEP
                && usize::try_from(*maximum_candidate_edges).ok()
                    == Some(MAX_CANDIDATE_EDGES_PER_STEP)
                && usize::try_from(*observed_candidate_edges_at_least).ok()
                    == MAX_CANDIDATE_EDGES_PER_STEP.checked_add(1)
        }
    }
}

fn valid_selector_trace(trace: &ProofQualificationTraceV1) -> bool {
    if trace.selector_early_return {
        trace.selectors.iter().any(|selector| {
            matches!(
                selector.outcome,
                SelectorGateOutcomeV1::Failed { .. } | SelectorGateOutcomeV1::Unavailable { .. }
            )
        })
    } else {
        trace
            .selectors
            .iter()
            .all(|selector| matches!(selector.outcome, SelectorGateOutcomeV1::Resolved { .. }))
    }
}

fn strictly_ascending(values: &[i64]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_funnel_outcome(outcome: &FunnelOutcomeV1) -> bool {
    match outcome {
        FunnelOutcomeV1::Admitted => true,
        FunnelOutcomeV1::FirstZeroSurvivor { gate, histogram } => {
            let mut candidate_edge_ids = histogram
                .iter()
                .flat_map(|bucket| bucket.edge_ids.iter().copied())
                .collect::<Vec<_>>();
            candidate_edge_ids.sort_unstable();
            candidate_edge_ids.dedup();
            valid_step_trace(&StepQualificationTraceV1 {
                step_index: 0,
                candidate_edge_ids,
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
        FinalizationTraceV1::Complete { projection_bytes } => *projection_bytes <= 65_536,
        FinalizationTraceV1::Failed { .. } => true,
    }
}

fn valid_case_finalization(case: &CaseReportV1) -> bool {
    match case.proof_trace.finalization {
        FinalizationTraceV1::NotRun => false,
        FinalizationTraceV1::Complete { projection_bytes } => {
            case.complete_projection_bytes == projection_bytes && projection_bytes <= 65_536
        }
        FinalizationTraceV1::Failed { .. } => case.complete_projection_bytes == 0,
    }
}

fn valid_receipts(case: &CaseReportV1, project: Option<&ProjectMaterializationEvidenceV1>) -> bool {
    let Some(project) = project else {
        return false;
    };
    let evidence = &case.receipt_evidence;
    if case.product_disposition.authoritative_receipts.len() > 6
        || evidence.observed_receipts.len() > MAX_OBSERVED_RECEIPTS_PER_CASE
        || evidence.missing_oracle_steps.len() > usize::from(case.attempted_step_count)
        || !unique(
            evidence
                .observed_receipts
                .iter()
                .map(|receipt| receipt.receipt_id.as_str()),
        )
        || !unique_receipt_edges(
            evidence
                .observed_receipts
                .iter()
                .map(|receipt| (receipt.step_index, receipt.edge_id)),
        )
        || !unique_receipt_references(&case.product_disposition.authoritative_receipts)
        || !unique_u8(
            evidence
                .missing_oracle_steps
                .iter()
                .map(|missing| missing.step_index),
        )
    {
        return false;
    }
    let mut admitted_edges = BTreeSet::new();
    for step in &case.proof_trace.steps {
        if let StepQualificationOutcomeV1::Admitted { edge_ids } = &step.outcome {
            let Ok(step_index) = u8::try_from(step.step_index) else {
                return false;
            };
            for edge_id in edge_ids {
                if !admitted_edges.insert((step_index, *edge_id)) {
                    return false;
                }
            }
        }
    }
    let observed_edges = evidence
        .observed_receipts
        .iter()
        .map(|receipt| (receipt.step_index, receipt.edge_id))
        .collect::<BTreeSet<_>>();
    if admitted_edges != observed_edges {
        return false;
    }
    let mut resolved_nodes = BTreeMap::<u8, &ResolvedNodeIdentityV1>::new();
    for receipt in &evidence.observed_receipts {
        let Some(source_node_id) =
            resolved_selector_node_id(&case.proof_trace, u64::from(receipt.step_index))
        else {
            return false;
        };
        let Some(target_node_id) =
            resolved_selector_node_id(&case.proof_trace, u64::from(receipt.step_index) + 1)
        else {
            return false;
        };
        if receipt.step_index >= case.attempted_step_count
            || !valid_observed_receipt(receipt, &project.identity, source_node_id, target_node_id)
            || !consistent_resolved_node(&mut resolved_nodes, receipt.step_index, &receipt.source)
            || !consistent_resolved_node(
                &mut resolved_nodes,
                receipt.step_index + 1,
                &receipt.target,
            )
        {
            return false;
        }
    }
    if case
        .product_disposition
        .authoritative_receipts
        .iter()
        .any(|reference| {
            !evidence.observed_receipts.iter().any(|receipt| {
                receipt.receipt_id == reference.receipt_id && receipt.edge_id == reference.edge_id
            })
        })
    {
        return false;
    }
    let exact_authoritative_steps = case
        .product_disposition
        .authoritative_receipts
        .iter()
        .filter_map(|reference| {
            evidence.observed_receipts.iter().find(|receipt| {
                receipt.receipt_id == reference.receipt_id
                    && receipt.edge_id == reference.edge_id
                    && receipt.oracle_comparison.is_exact()
            })
        })
        .map(|receipt| receipt.step_index)
        .collect::<BTreeSet<_>>();
    let expected_missing = (0..case.attempted_step_count)
        .filter(|step_index| !exact_authoritative_steps.contains(step_index))
        .collect::<BTreeSet<_>>();
    let actual_missing = evidence
        .missing_oracle_steps
        .iter()
        .map(|missing| missing.step_index)
        .collect::<BTreeSet<_>>();
    expected_missing == actual_missing
        && evidence.missing_oracle_steps.iter().all(|missing| {
            missing.step_index < case.attempted_step_count
                && valid_oracle_step(&missing.oracle_step)
        })
}

fn valid_disposition_structure(case: &CaseReportV1) -> bool {
    unique_typed_gaps(case.product_disposition.gaps.iter().copied())
        && case
            .actionable_exact_gap
            .as_ref()
            .is_none_or(|gap| case.product_disposition.gaps.contains(gap))
        && valid_actual_product_result(&case.product_disposition.actual)
        && disposition_summary_matches_actual(&case.product_disposition)
}

fn valid_actual_product_result(actual: &ActualProductResultV1) -> bool {
    match actual {
        ActualProductResultV1::ContractProven {
            contract_digest,
            receipts,
        } => hash(contract_digest) && valid_projected_receipts(receipts),
        ActualProductResultV1::ContractRefuted {
            contract_digest,
            basis,
        } => {
            hash(contract_digest)
                && match basis {
                    ProductRefutationBasisV1::PositiveContradiction {
                        step_index,
                        prohibition_index,
                        connected_receipts,
                    } => {
                        *step_index < 6
                            && *prohibition_index < 6
                            && !connected_receipts.is_empty()
                            && valid_projected_receipts(connected_receipts)
                    }
                    ProductRefutationBasisV1::CertifiedAbsence {
                        step_index,
                        extractor_capability_receipt_id,
                        enumeration_receipt_id,
                        connected_receipts,
                    } => {
                        *step_index < 6
                            && !empty(extractor_capability_receipt_id)
                            && sanitized_atom(extractor_capability_receipt_id)
                            && !empty(enumeration_receipt_id)
                            && sanitized_atom(enumeration_receipt_id)
                            && valid_projected_receipts(connected_receipts)
                    }
                }
        }
        ActualProductResultV1::Unknown {
            contract_digest,
            gaps,
            connected_receipts,
        } => {
            hash(contract_digest)
                && (1..=6).contains(&gaps.len())
                && gaps.iter().copied().collect::<BTreeSet<_>>().len() == gaps.len()
                && gaps.iter().all(valid_actual_gap)
                && valid_projected_receipts(connected_receipts)
        }
        ActualProductResultV1::Unavailable {
            contract_digest,
            reasons,
        } => {
            hash(contract_digest)
                && (1..=4).contains(&reasons.len())
                && reasons.iter().copied().collect::<BTreeSet<_>>().len() == reasons.len()
        }
        ActualProductResultV1::Invalid { failure } => {
            !empty(&failure.code) && sanitized_atom(&failure.code)
        }
    }
}

fn valid_actual_gap(gap: &ActualProofGapV1) -> bool {
    match gap {
        ActualProofGapV1::SelectorMissing { selector_index }
        | ActualProofGapV1::SelectorAmbiguous { selector_index }
        | ActualProofGapV1::NonCallableSelector { selector_index } => *selector_index < 7,
        ActualProofGapV1::DirectCallMissing { step_index }
        | ActualProofGapV1::RecursiveCallNotRepresentable { step_index }
        | ActualProofGapV1::SourceWindowTooLarge { step_index }
        | ActualProofGapV1::InvalidUtf8 { step_index }
        | ActualProofGapV1::SourceLineOutOfRange { step_index }
        | ActualProofGapV1::EdgeContainmentUnproven { step_index }
        | ActualProofGapV1::MissingDirectCallReceipt { step_index }
        | ActualProofGapV1::ReceiptOrEdgeAlreadyUsed { step_index }
        | ActualProofGapV1::ProjectionExclusionConflictsWithRequiredReceipt { step_index } => {
            *step_index < 6
        }
        ActualProofGapV1::OutputBudgetExceeded => true,
    }
}

fn valid_projected_receipts(receipts: &[ProjectedReceiptReferenceV1]) -> bool {
    receipts.len() <= 6
        && unique(
            receipts
                .iter()
                .map(|reference| reference.receipt_id.as_str()),
        )
        && unique(receipts.iter().map(|reference| reference.edge_id.as_str()))
        && receipts.iter().all(|reference| {
            valid_receipt_id(&reference.receipt_id)
                && reference
                    .edge_id
                    .parse::<i64>()
                    .is_ok_and(|edge_id| edge_id.to_string() == reference.edge_id)
        })
}

fn disposition_summary_matches_actual(disposition: &ProductDispositionV1) -> bool {
    let projected = match &disposition.actual {
        ActualProductResultV1::ContractProven { receipts, .. } => {
            if disposition.kind != ProductDispositionKindV1::ContractProven
                || !disposition.gaps.is_empty()
            {
                return false;
            }
            receipts
        }
        ActualProductResultV1::ContractRefuted { basis, .. } => match basis {
            ProductRefutationBasisV1::PositiveContradiction {
                connected_receipts, ..
            } => {
                if disposition.kind != ProductDispositionKindV1::Unknown
                    || !disposition.gaps.is_empty()
                {
                    return false;
                }
                connected_receipts
            }
            ProductRefutationBasisV1::CertifiedAbsence {
                extractor_capability_receipt_id,
                enumeration_receipt_id,
                connected_receipts,
                ..
            } => {
                if disposition.kind != ProductDispositionKindV1::CertifiedAbsence
                    || !disposition.gaps.is_empty()
                    || empty(extractor_capability_receipt_id)
                    || empty(enumeration_receipt_id)
                {
                    return false;
                }
                connected_receipts
            }
        },
        ActualProductResultV1::Unknown {
            gaps,
            connected_receipts,
            ..
        } => {
            let expected = gaps
                .iter()
                .map(coarse_gap)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if disposition.kind != ProductDispositionKindV1::Unknown
                || gaps.is_empty()
                || disposition.gaps != expected
            {
                return false;
            }
            connected_receipts
        }
        ActualProductResultV1::Unavailable { reasons, .. } => {
            return disposition.kind == ProductDispositionKindV1::Unknown
                && disposition.gaps.is_empty()
                && disposition.authoritative_receipts.is_empty()
                && !reasons.is_empty();
        }
        ActualProductResultV1::Invalid { failure } => {
            return disposition.kind == ProductDispositionKindV1::Invalid
                && disposition.gaps.is_empty()
                && disposition.authoritative_receipts.is_empty()
                && !empty(&failure.code)
                && sanitized_atom(&failure.code);
        }
    };
    let converted = projected
        .iter()
        .map(|reference| {
            reference
                .edge_id
                .parse::<i64>()
                .ok()
                .map(|edge_id| ReceiptReferenceV1 {
                    receipt_id: reference.receipt_id.clone(),
                    edge_id,
                })
        })
        .collect::<Option<Vec<_>>>();
    converted.as_ref() == Some(&disposition.authoritative_receipts)
}

fn valid_oracle_step(step: &ReceiptOracleStepV1) -> bool {
    !empty(&step.caller_symbol)
        && step.callsite_line > 0
        && !empty(&step.target_symbol)
        && range(&step.receipt_line_window).is_ok()
}

fn valid_observed_line_window(window: &ObservedLineWindowV1) -> bool {
    window.kind == "indexed_line_v1"
        && !window.project_file_components.is_empty()
        && window
            .project_file_components
            .iter()
            .all(|component| !empty(component) && component != "." && component != "..")
        && hash(&window.indexed_sha256)
        && hash(&window.observed_sha256)
        && window.byte_start < window.byte_end
        && !empty(&window.text)
        && window
            .byte_end
            .checked_sub(window.byte_start)
            .is_some_and(|length| {
                length <= 8_192 && u64::try_from(window.text.len()).ok() == Some(length)
            })
}

fn valid_observed_receipt(
    receipt: &ObservedReceiptV1,
    environment: &EnvironmentIdentityV1,
    source_selector_node_id: i64,
    target_selector_node_id: i64,
) -> bool {
    valid_observed_receipt_shape(receipt)
        && pinned_matches_environment(&receipt.source.pinned, environment)
        && pinned_matches_environment(&receipt.target.pinned, environment)
        && parse_node_id(&receipt.source.pinned.node_id) == Some(source_selector_node_id)
        && parse_node_id(&receipt.target.pinned.node_id) == Some(target_selector_node_id)
}

fn valid_observed_receipt_shape(receipt: &ObservedReceiptV1) -> bool {
    let Some(source_node_id) = valid_resolved_node_identity(&receipt.source) else {
        return false;
    };
    let Some(target_node_id) = valid_resolved_node_identity(&receipt.target) else {
        return false;
    };
    let Some((file_node_id, callsite_line, _, callsite_target_node_id)) =
        parse_callsite_identity(&receipt.callsite_identity)
    else {
        return false;
    };
    valid_receipt_id(&receipt.receipt_id)
        && receipt.certainty == ReceiptCertaintyV1::Certain
        && receipt.source.pinned.project_id == receipt.target.pinned.project_id
        && receipt.source.pinned.core_generation_id == receipt.target.pinned.core_generation_id
        && receipt.source.pinned.core_run_id == receipt.target.pinned.core_run_id
        && callsite_line == receipt.callsite_line
        && file_node_id == receipt.containment.file_node_id
        && callsite_target_node_id == target_node_id
        && receipt.containment.owner_node_id == source_node_id
        && receipt.containment.start_line > 0
        && receipt.containment.start_line <= receipt.callsite_line
        && receipt.callsite_line <= receipt.containment.end_line
        && receipt.line_window.project_file_components == receipt.source.project_file_components
        && receipt.oracle_comparison.oracle_step_index() == receipt.step_index
        && valid_receipt_oracle_comparison(receipt)
}

fn valid_receipt_id(receipt_id: &str) -> bool {
    receipt_id
        .strip_prefix("indexed-call-edge:")
        .is_some_and(|suffix| !empty(suffix))
}

fn parse_callsite_identity(identity: &str) -> Option<(i64, u32, u32, i64)> {
    if empty(identity) {
        return None;
    }
    let pre_marker = identity
        .split_once('|')
        .map_or(identity, |(identity, _)| identity);
    let mut fields = pre_marker.split(':');
    let parsed = (
        fields.next().and_then(|value| value.parse::<i64>().ok()),
        fields.next().and_then(|value| value.parse::<u32>().ok()),
        fields.next().and_then(|value| value.parse::<u32>().ok()),
        fields.next().and_then(|value| value.parse::<i64>().ok()),
    );
    let (Some(file), Some(parsed_line), Some(column_or_ordinal), Some(target)) = parsed else {
        return None;
    };
    (fields.next().is_none()
        && parsed_line > 0
        && format!("{file}:{parsed_line}:{column_or_ordinal}:{target}") == pre_marker)
        .then_some((file, parsed_line, column_or_ordinal, target))
}

fn valid_resolved_node_identity(identity: &ResolvedNodeIdentityV1) -> Option<i64> {
    (!empty(&identity.pinned.project_id)
        && !empty(&identity.pinned.core_generation_id)
        && !empty(&identity.pinned.core_run_id)
        && !empty(&identity.canonical_id)
        && !empty(&identity.qualified_name)
        && valid_project_file_components(&identity.project_file_components))
    .then(|| parse_node_id(&identity.pinned.node_id))
    .flatten()
}

fn parse_node_id(value: &str) -> Option<i64> {
    let parsed = value.parse::<i64>().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn valid_project_file_components(components: &[String]) -> bool {
    !components.is_empty()
        && components
            .iter()
            .all(|component| !empty(component) && component != "." && component != "..")
}

fn pinned_matches_environment(
    pinned: &PinnedNodeIdentityV1,
    environment: &EnvironmentIdentityV1,
) -> bool {
    pinned.project_id == environment.project_id
        && pinned.core_generation_id == environment.core_generation_id
        && pinned.core_run_id == environment.core_run_id
}

fn resolved_selector_node_id(
    trace: &ProofQualificationTraceV1,
    selector_index: u64,
) -> Option<i64> {
    trace
        .selectors
        .iter()
        .find(|selector| selector.selector_index == selector_index)
        .and_then(|selector| match &selector.outcome {
            SelectorGateOutcomeV1::Resolved { node_id } => Some(*node_id),
            SelectorGateOutcomeV1::Failed { .. } | SelectorGateOutcomeV1::Unavailable { .. } => {
                None
            }
        })
}

fn consistent_resolved_node<'a>(
    nodes: &mut BTreeMap<u8, &'a ResolvedNodeIdentityV1>,
    selector_index: u8,
    identity: &'a ResolvedNodeIdentityV1,
) -> bool {
    match nodes.insert(selector_index, identity) {
        Some(existing) => existing == identity,
        None => true,
    }
}

fn valid_receipt_oracle_comparison(receipt: &ObservedReceiptV1) -> bool {
    if !valid_observed_line_window(&receipt.line_window)
        || !valid_oracle_step(receipt.oracle_comparison.oracle_step())
    {
        return false;
    }
    let expected = receipt_mismatches(receipt, receipt.oracle_comparison.oracle_step());
    match &receipt.oracle_comparison {
        ReceiptOracleComparisonV1::Exact { .. } => expected.is_empty(),
        ReceiptOracleComparisonV1::Mismatched { mismatches, .. } => {
            !mismatches.is_empty()
                && strictly_ascending_mismatch_fields(mismatches)
                && mismatches == &expected
        }
    }
}

fn receipt_mismatches(
    receipt: &ObservedReceiptV1,
    oracle: &ReceiptOracleStepV1,
) -> Vec<ReceiptMismatchFieldV1> {
    let mut mismatches = Vec::new();
    if receipt.source.qualified_name != oracle.caller_symbol {
        mismatches.push(ReceiptMismatchFieldV1::Caller);
    }
    if receipt.callsite_line != oracle.callsite_line {
        mismatches.push(ReceiptMismatchFieldV1::CallsiteLine);
    }
    if receipt.line_window.project_file_components.join("/") != oracle.receipt_line_window.path
        || receipt.line_window.indexed_sha256 != oracle.receipt_line_window.sha256
        || receipt.line_window.observed_sha256 != oracle.receipt_line_window.sha256
        || receipt.line_window.byte_start != oracle.receipt_line_window.start_byte
        || receipt.line_window.byte_end != oracle.receipt_line_window.end_byte
    {
        mismatches.push(ReceiptMismatchFieldV1::CallsiteWindow);
    }
    if receipt.target.qualified_name != oracle.target_symbol {
        mismatches.push(ReceiptMismatchFieldV1::Target);
    }
    mismatches
}

fn strictly_ascending_mismatch_fields(values: &[ReceiptMismatchFieldV1]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
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
        SchemaDocument::Path => (schema::<CohortPathFileV1>(), PATH_FILE_SCHEMA),
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
        SchemaDocument::Path => PATH_FILE_SCHEMA,
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
            if matches!(
                name.as_str(),
                "commit"
                    | "source_commit"
                    | "source_tree"
                    | "source_head"
                    | "qualification_source_commit"
                    | "qualification_source_tree"
            ) {
                property.insert("pattern".into(), Value::String(COMMIT.into()));
            }
        }
    }
    if let Some(definitions) = root.get_mut("$defs").and_then(Value::as_object_mut) {
        for (definition_name, definition) in definitions.iter_mut() {
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
                        | "indexed_sha256"
                        | "observed_sha256"
                ) {
                    property.insert("pattern".into(), Value::String(SHA256.into()));
                }
                if name == "source_tree" && definition_name == "ProjectMaterializationEvidenceV1" {
                    property.insert("pattern".into(), Value::String(SHA256.into()));
                } else if matches!(
                    name.as_str(),
                    "commit"
                        | "source_commit"
                        | "source_tree"
                        | "source_head"
                        | "qualification_source_commit"
                        | "qualification_source_tree"
                ) {
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
    semantic_contract_bounds(schema, document);
    annotate_transport_bounds(schema);
    annotate_finalization_bounds(schema);
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
        }
        SchemaDocument::Path => {
            set_bounds(schema, None, "paths", Some(30), Some(30));
            set_bounds(schema, Some("CallPathSpecV1"), "steps", Some(1), Some(6));
            for field in ["prohibit_traversal_through", "exclude_from_projection"] {
                set_bounds(schema, Some("CallPathSpecV1"), field, Some(0), Some(6));
            }
            set_bounds(
                schema,
                Some("OraclePathV1"),
                "oracle_steps",
                Some(1),
                Some(6),
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
                Some("NegativeMutationV1"),
                "step_index",
                Some(0),
                Some(5),
            );
            set_bounds(schema, Some("OracleStepV1"), "callsite_line", Some(1), None);
            set_recursive_field_bounds(schema, "ProofContractFieldV1", "step", Some(0), Some(5));
            set_recursive_field_bounds(schema, "ProofContractFieldV1", "index", Some(0), Some(5));
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
            set_bounds(schema, Some("OracleStepV1"), "callsite_line", Some(1), None);
            set_bounds(schema, None, "inventory", Some(4), Some(4));
            set_bounds(schema, None, "trails", Some(4), Some(4));
            set_bounds(schema, None, "cases", Some(120), Some(120));
            set_bounds(
                schema,
                Some("EnvironmentReportV1"),
                "projects",
                Some(4),
                Some(4),
            );
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
                Some("TrailLengthCountsV1"),
                "length",
                Some(1),
                Some(6),
            );
            set_bounds(
                schema,
                Some("CaseReportV1"),
                "attempted_step_count",
                Some(1),
                Some(6),
            );
            set_bounds(
                schema,
                Some("ProductDispositionV1"),
                "authoritative_receipts",
                Some(0),
                Some(6),
            );
            set_pattern(
                schema,
                "EnvironmentReportV1",
                "recorded_at",
                "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?Z$",
            );
            set_recursive_field_pattern(schema, "ActualProductResultV1", "contract_digest", SHA256);
            set_recursive_field_bounds(
                schema,
                "ActualProductResultV1",
                "receipts",
                Some(0),
                Some(6),
            );
            set_recursive_field_bounds(
                schema,
                "ActualProductResultV1",
                "connected_receipts",
                Some(0),
                Some(6),
            );
            set_recursive_field_bounds(schema, "ActualProductResultV1", "gaps", Some(1), Some(6));
            set_recursive_field_bounds(
                schema,
                "ActualProductResultV1",
                "reasons",
                Some(1),
                Some(4),
            );
            set_recursive_field_bounds(
                schema,
                "ProductRefutationBasisV1",
                "connected_receipts",
                Some(0),
                Some(6),
            );
            for definition in ["ActualProductResultV1", "ProductRefutationBasisV1"] {
                set_recursive_field_bounds(schema, definition, "step_index", Some(0), Some(5));
                set_recursive_field_bounds(
                    schema,
                    definition,
                    "prohibition_index",
                    Some(0),
                    Some(5),
                );
            }
            set_recursive_field_bounds(
                schema,
                "ActualProductResultV1",
                "selector_index",
                Some(0),
                Some(6),
            );
            set_bounds(
                schema,
                Some("ProductDispositionV1"),
                "gaps",
                Some(0),
                Some(6),
            );
            set_bounds(
                schema,
                Some("ObservedReceiptV1"),
                "step_index",
                Some(0),
                Some(5),
            );
            set_bounds(
                schema,
                Some("ObservedReceiptV1"),
                "callsite_line",
                Some(1),
                None,
            );
            for field in ["start_line", "end_line"] {
                set_bounds(
                    schema,
                    Some("CallableContainmentEvidenceV1"),
                    field,
                    Some(1),
                    None,
                );
            }
            set_bounds(
                schema,
                Some("MissingOracleStepV1"),
                "step_index",
                Some(0),
                Some(5),
            );
            set_bounds(
                schema,
                Some("NegativeMutationResultV1"),
                "step_index",
                Some(0),
                Some(5),
            );
            set_bounds(
                schema,
                Some("StepQualificationTraceV1"),
                "step_index",
                Some(0),
                Some(5),
            );
            set_bounds(
                schema,
                Some("SelectorQualificationTraceV1"),
                "selector_index",
                Some(0),
                Some(6),
            );
            set_bounds(
                schema,
                Some("ReceiptEvidenceV1"),
                "observed_receipts",
                Some(0),
                Some(MAX_OBSERVED_RECEIPTS_PER_CASE as u64),
            );
            set_bounds(
                schema,
                Some("ReceiptEvidenceV1"),
                "missing_oracle_steps",
                Some(0),
                Some(6),
            );
            set_bounds(
                schema,
                Some("ObservedLineWindowV1"),
                "project_file_components",
                Some(1),
                None,
            );
            for definition in ["ResolvedNodeIdentityV1", "ObservedLineWindowV1"] {
                set_bounds(
                    schema,
                    Some(definition),
                    "project_file_components",
                    Some(1),
                    None,
                );
            }
            set_bounds(
                schema,
                Some("StepQualificationTraceV1"),
                "candidate_edge_ids",
                Some(0),
                Some(MAX_CANDIDATE_EDGES_PER_STEP as u64),
            );
            set_bounds(
                schema,
                Some("CandidateFailureHistogramV1"),
                "edge_ids",
                Some(1),
                Some(MAX_CANDIDATE_EDGES_PER_STEP as u64),
            );
            set_const(
                schema,
                Some("ObservedLineWindowV1"),
                "kind",
                Value::String("indexed_line_v1".into()),
            );
            set_bounds(
                schema,
                Some("ProofQualificationTraceV1"),
                "steps",
                Some(0),
                Some(6),
            );
            set_bounds(
                schema,
                Some("ProofQualificationTraceV1"),
                "selectors",
                Some(2),
                Some(7),
            );
            set_bounds(
                schema,
                Some("CaseReportV1"),
                "negative_mutations",
                Some(2),
                Some(2),
            );
            set_bounds(
                schema,
                Some("CaseReportV1"),
                "unclassified_step_indices",
                Some(0),
                Some(6),
            );
            set_bounds(
                schema,
                Some("CaseReportV1"),
                "complete_projection_bytes",
                Some(0),
                Some(65_536),
            );
            set_bounds(
                schema,
                Some("FailureFunnelReportV1"),
                "buckets",
                Some(0),
                Some(312),
            );
            set_bounds(
                schema,
                Some("TransportMeasurementSetV1"),
                "measurements",
                Some(4),
                Some(4),
            );
            set_bounds(
                schema,
                Some("TransportMeasurementV1"),
                "actual_bytes",
                Some(0),
                Some(65_536),
            );
            for (definition, field) in [
                ("ReceiptReferenceV1", "receipt_id"),
                ("ObservedReceiptV1", "receipt_id"),
                ("ObservedReceiptV1", "callsite_identity"),
                ("ObservedLineWindowV1", "kind"),
                ("ObservedLineWindowV1", "text"),
                ("EnvironmentIdentityV1", "project_id"),
                ("EnvironmentIdentityV1", "core_generation_id"),
                ("EnvironmentIdentityV1", "core_run_id"),
                ("PinnedNodeIdentityV1", "project_id"),
                ("PinnedNodeIdentityV1", "core_generation_id"),
                ("PinnedNodeIdentityV1", "core_run_id"),
                ("PinnedNodeIdentityV1", "node_id"),
                ("ResolvedNodeIdentityV1", "canonical_id"),
                ("ResolvedNodeIdentityV1", "qualified_name"),
                ("ProjectedReceiptReferenceV1", "receipt_id"),
                ("ProjectedReceiptReferenceV1", "edge_id"),
                ("ProductToolFailureV1", "code"),
            ] {
                set_min_length(schema, definition, field, 1);
            }
            set_max_length(schema, "ObservedLineWindowV1", "text", 8_192);
            set_pattern(
                schema,
                "ReceiptReferenceV1",
                "receipt_id",
                "^indexed-call-edge:.+$",
            );
            set_pattern(
                schema,
                "ObservedReceiptV1",
                "receipt_id",
                "^indexed-call-edge:.+$",
            );
            set_pattern(
                schema,
                "ObservedReceiptV1",
                "callsite_identity",
                "^-?(0|[1-9][0-9]*):[1-9][0-9]*:(0|[1-9][0-9]*):-?(0|[1-9][0-9]*)(\\|.*)?$",
            );
            set_pattern(
                schema,
                "PinnedNodeIdentityV1",
                "node_id",
                "^-?(0|[1-9][0-9]*)$",
            );
            set_pattern(
                schema,
                "ProjectedReceiptReferenceV1",
                "receipt_id",
                "^indexed-call-edge:.+$",
            );
            set_pattern(
                schema,
                "ProjectedReceiptReferenceV1",
                "edge_id",
                "^-?(0|[1-9][0-9]*)$",
            );
            set_array_item_min_length(schema, "ObservedLineWindowV1", "project_file_components", 1);
            set_array_item_min_length(
                schema,
                "ResolvedNodeIdentityV1",
                "project_file_components",
                1,
            );
            annotate_receipt_comparison_bounds(schema);
            annotate_candidate_outcome_bounds(schema);
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

fn set_recursive_field_bounds(
    schema: &mut Value,
    definition: &str,
    field: &str,
    minimum: Option<u64>,
    maximum: Option<u64>,
) {
    let Some(definition) = schema
        .get_mut("$defs")
        .and_then(|definitions| definitions.get_mut(definition))
    else {
        return;
    };
    apply_recursive_field_bounds(definition, field, minimum, maximum);
}

fn apply_recursive_field_bounds(
    value: &mut Value,
    field: &str,
    minimum: Option<u64>,
    maximum: Option<u64>,
) {
    match value {
        Value::Object(map) => {
            if let Some(property) = map
                .get_mut("properties")
                .and_then(Value::as_object_mut)
                .and_then(|properties| properties.get_mut(field))
                .and_then(Value::as_object_mut)
            {
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
            for nested in map.values_mut() {
                apply_recursive_field_bounds(nested, field, minimum, maximum);
            }
        }
        Value::Array(values) => {
            for nested in values {
                apply_recursive_field_bounds(nested, field, minimum, maximum);
            }
        }
        _ => {}
    }
}

fn set_recursive_field_pattern(schema: &mut Value, definition: &str, field: &str, pattern: &str) {
    let Some(definition) = schema
        .get_mut("$defs")
        .and_then(|definitions| definitions.get_mut(definition))
    else {
        return;
    };
    apply_recursive_field_pattern(definition, field, pattern);
}

fn apply_recursive_field_pattern(value: &mut Value, field: &str, pattern: &str) {
    match value {
        Value::Object(map) => {
            if let Some(property) = map
                .get_mut("properties")
                .and_then(Value::as_object_mut)
                .and_then(|properties| properties.get_mut(field))
                .and_then(Value::as_object_mut)
            {
                property.insert("pattern".into(), Value::String(pattern.into()));
            }
            for nested in map.values_mut() {
                apply_recursive_field_pattern(nested, field, pattern);
            }
        }
        Value::Array(values) => {
            for nested in values {
                apply_recursive_field_pattern(nested, field, pattern);
            }
        }
        _ => {}
    }
}

fn set_min_length(schema: &mut Value, definition: &str, field: &str, minimum: u64) {
    if let Some(property) = schema_property(schema, Some(definition), field) {
        property.insert("minLength".into(), Value::from(minimum));
    }
}

fn set_max_length(schema: &mut Value, definition: &str, field: &str, maximum: u64) {
    if let Some(property) = schema_property(schema, Some(definition), field) {
        property.insert("maxLength".into(), Value::from(maximum));
    }
}

fn set_pattern(schema: &mut Value, definition: &str, field: &str, pattern: &str) {
    if let Some(property) = schema_property(schema, Some(definition), field) {
        property.insert("pattern".into(), Value::String(pattern.into()));
    }
}

fn set_array_item_min_length(schema: &mut Value, definition: &str, field: &str, minimum: u64) {
    if let Some(items) = schema_property(schema, Some(definition), field)
        .and_then(|property| property.get_mut("items"))
        .and_then(Value::as_object_mut)
    {
        items.insert("minLength".into(), Value::from(minimum));
    }
}

fn annotate_receipt_comparison_bounds(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) {
                if let Some(step_index) = properties
                    .get_mut("oracle_step_index")
                    .and_then(Value::as_object_mut)
                {
                    step_index.insert("minimum".into(), Value::from(0));
                    step_index.insert("maximum".into(), Value::from(5));
                }
                if let Some(mismatches) = properties
                    .get_mut("mismatches")
                    .and_then(Value::as_object_mut)
                {
                    mismatches.insert("minItems".into(), Value::from(1));
                    mismatches.insert("maxItems".into(), Value::from(4));
                }
            }
            for value in map.values_mut() {
                annotate_receipt_comparison_bounds(value);
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(annotate_receipt_comparison_bounds),
        _ => {}
    }
}

fn annotate_candidate_outcome_bounds(value: &mut Value) {
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
            if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) {
                if kind.as_deref() == Some("admitted")
                    && let Some(edge_ids) = properties
                        .get_mut("edge_ids")
                        .and_then(Value::as_object_mut)
                {
                    edge_ids.insert("minItems".into(), Value::from(1));
                    edge_ids.insert("maxItems".into(), Value::from(MAX_CANDIDATE_EDGES_PER_STEP));
                }
                if kind.as_deref() == Some("candidate_limit_exceeded") {
                    if let Some(maximum) = properties
                        .get_mut("maximum_candidate_edges")
                        .and_then(Value::as_object_mut)
                    {
                        maximum.insert("const".into(), Value::from(MAX_CANDIDATE_EDGES_PER_STEP));
                    }
                    if let Some(observed) = properties
                        .get_mut("observed_candidate_edges_at_least")
                        .and_then(Value::as_object_mut)
                    {
                        observed.insert(
                            "const".into(),
                            Value::from(MAX_CANDIDATE_EDGES_PER_STEP + 1),
                        );
                    }
                }
            }
            for value in map.values_mut() {
                annotate_candidate_outcome_bounds(value);
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(annotate_candidate_outcome_bounds),
        _ => {}
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

fn annotate_finalization_bounds(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let kind = map
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get("kind"))
                .and_then(Value::as_object)
                .and_then(|kind| kind.get("const"))
                .and_then(Value::as_str);
            if kind == Some("complete")
                && let Some(bytes) = map
                    .get_mut("properties")
                    .and_then(Value::as_object_mut)
                    .and_then(|properties| properties.get_mut("projection_bytes"))
                    .and_then(Value::as_object_mut)
            {
                bytes.insert("maximum".into(), Value::from(65_536));
            }
            for value in map.values_mut() {
                annotate_finalization_bounds(value);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(annotate_finalization_bounds),
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
        || validate_oracle_source_path(&value.path).is_err()
        || value.start_byte >= value.end_byte
        || value.end_byte > value.file_byte_length
        || !hash(&value.sha256)
    {
        bail!("proof_availability_range_invalid")
    }
    Ok(())
}

fn validate_oracle_source_path(path: &str) -> Result<()> {
    if path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || path.contains('\\')
        || path.as_bytes().get(1) == Some(&b':')
    {
        bail!("proof_availability_oracle_source_path_invalid")
    }
    let components = path.split('/').map(ToOwned::to_owned).collect::<Vec<_>>();
    validate_project_file(&components)
}
fn unique<'a>(mut values: impl Iterator<Item = &'a str>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|v| !empty(v) && set.insert(v))
}

fn unique_u64(mut values: impl Iterator<Item = u64>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|value| set.insert(value))
}

fn unique_u8(mut values: impl Iterator<Item = u8>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|value| set.insert(value))
}

fn unique_i64(mut values: impl Iterator<Item = i64>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|value| set.insert(value))
}

fn unique_typed_gaps(mut values: impl Iterator<Item = TypedGapV1>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|value| set.insert(value))
}

fn unique_receipt_edges(mut values: impl Iterator<Item = (u8, i64)>) -> bool {
    let mut set = BTreeSet::new();
    values.all(|value| set.insert(value))
}

fn unique_receipt_references(values: &[ReceiptReferenceV1]) -> bool {
    let mut receipt_ids = BTreeSet::new();
    let mut references = BTreeSet::new();
    values.iter().all(|reference| {
        valid_receipt_id(&reference.receipt_id)
            && receipt_ids.insert(reference.receipt_id.as_str())
            && references.insert((reference.receipt_id.as_str(), reference.edge_id))
    })
}

fn ratio_milli(numerator: u64, denominator: u64) -> Result<u16> {
    if denominator == 0 {
        return Ok(0);
    }
    let scaled = numerator
        .checked_mul(1000)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_metric_overflow"))?
        / denominator;
    u16::try_from(scaled).map_err(Into::into)
}
