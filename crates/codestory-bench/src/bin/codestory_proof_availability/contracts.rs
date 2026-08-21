use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const CORPUS_SCHEMA: &str = "codestory.proof-availability-corpus/v1";
pub const PATH_SCHEMA: &str = "codestory.proof-availability-path/v1";
pub const REPORT_SCHEMA: &str = "codestory.proof-availability-report/v1";
pub const THRESHOLDS_SCHEMA: &str = "codestory.proof-availability-thresholds/v1";
const FIXTURE_MUTATION_COUNT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDocument {
    Corpus,
    Path,
    Report,
    Thresholds,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceHashV1 {
    pub path: String,
    pub sha256: String,
    pub byte_length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceRevisionV1 {
    pub source_id: String,
    pub repository: String,
    pub commit: String,
    pub tree: String,
    pub source_hashes: Vec<SourceHashV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceRangeV1 {
    pub path: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedPathOutcomeV1 {
    Proven,
    Partial,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OracleStepV1 {
    pub step_id: String,
    pub selector: String,
    pub source_range: SourceRangeV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OraclePathV1 {
    pub schema: String,
    pub path_id: String,
    pub source_id: String,
    pub expected_step_count: u32,
    pub steps: Vec<OracleStepV1>,
    pub expected_outcome: ExpectedPathOutcomeV1,
    pub notes: Option<String>,
}

impl OraclePathV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != PATH_SCHEMA
            || self.path_id.is_empty()
            || self.source_id.is_empty()
            || self.expected_step_count == 0
            || self.steps.len() != self.expected_step_count as usize
            || !unique(self.steps.iter().map(|step| step.step_id.as_str()))
        {
            bail!("proof_availability_oracle_path_invalid");
        }
        for step in &self.steps {
            validate_source_range(&step.source_range, None)?;
        }
        Ok(())
    }

    pub fn validate_against_source(&self, byte_length: u64) -> Result<()> {
        self.validate()?;
        for step in &self.steps {
            validate_source_range(&step.source_range, Some(byte_length))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationKindV1 {
    RemoveExpectedEdge,
    AddAmbiguousEdge,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CorpusMutationV1 {
    pub mutation_id: String,
    pub kind: MutationKindV1,
    pub path_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CorpusV1 {
    pub schema: String,
    pub corpus_id: String,
    pub sources: Vec<SourceRevisionV1>,
    pub path_count: u32,
    pub paths: Vec<OraclePathV1>,
    pub mutation_count: u32,
    pub mutations: Vec<CorpusMutationV1>,
    pub methodology_sha256: String,
    pub created_at: String,
}

impl CorpusV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != CORPUS_SCHEMA
            || self.corpus_id.is_empty()
            || self.path_count == 0
            || self.paths.len() != self.path_count as usize
            || self.mutation_count != FIXTURE_MUTATION_COUNT as u32
            || self.mutations.len() != self.mutation_count as usize
            || !is_lower_hex(&self.methodology_sha256, 64)
            || !unique(self.sources.iter().map(|source| source.source_id.as_str()))
            || !unique(self.paths.iter().map(|path| path.path_id.as_str()))
            || !unique(
                self.mutations
                    .iter()
                    .map(|mutation| mutation.mutation_id.as_str()),
            )
        {
            bail!("proof_availability_corpus_invalid");
        }
        for source in &self.sources {
            validate_source(source)?;
        }
        for path in &self.paths {
            path.validate()?;
            let source = self
                .sources
                .iter()
                .find(|source| source.source_id == path.source_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_path_source_missing"))?;
            for step in &path.steps {
                let source_hash = source
                    .source_hashes
                    .iter()
                    .find(|hash| hash.path == step.source_range.path)
                    .ok_or_else(|| anyhow::anyhow!("proof_availability_source_hash_missing"))?;
                if source_hash.sha256 != step.source_range.sha256 {
                    bail!("proof_availability_source_range_hash_mismatch");
                }
                validate_source_range(&step.source_range, Some(source_hash.byte_length))?;
            }
        }
        let path_ids = self
            .paths
            .iter()
            .map(|path| path.path_id.as_str())
            .collect::<BTreeSet<_>>();
        if self
            .mutations
            .iter()
            .any(|mutation| !path_ids.contains(mutation.path_id.as_str()))
        {
            bail!("proof_availability_mutation_path_missing");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HardGatesV1 {
    pub all_corpus_sources_materialized: bool,
    pub all_oracle_ranges_verified: bool,
    pub no_unclassified_failures: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentalThresholdsV1 {
    pub minimum_proven_step_ratio_milli: u16,
    pub minimum_actionable_partial_ratio_milli: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StableExplicitThresholdsV1 {
    pub minimum_proven_path_ratio_milli: u16,
    pub minimum_proven_step_ratio_milli: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutomaticThresholdsV1 {
    pub minimum_proven_path_ratio_milli: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ThresholdsV1 {
    pub schema: String,
    pub thresholds_id: String,
    pub hard_gates: HardGatesV1,
    pub experimental: ExperimentalThresholdsV1,
    pub stable_explicit: StableExplicitThresholdsV1,
    pub automatic: AutomaticThresholdsV1,
    pub methodology_sha256: String,
}

impl ThresholdsV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema != THRESHOLDS_SCHEMA
            || self.thresholds_id.is_empty()
            || !is_lower_hex(&self.methodology_sha256, 64)
            || !all_milli([
                self.experimental.minimum_proven_step_ratio_milli,
                self.experimental.minimum_actionable_partial_ratio_milli,
                self.stable_explicit.minimum_proven_path_ratio_milli,
                self.stable_explicit.minimum_proven_step_ratio_milli,
                self.automatic.minimum_proven_path_ratio_milli,
            ])
        {
            bail!("proof_availability_thresholds_invalid");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentReportV1 {
    pub environment_id: String,
    pub os: String,
    pub architecture: String,
    pub codestory_version: String,
    pub command_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InventoryStatusV1 {
    Complete,
    Incomplete,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InventoryReportV1 {
    pub source_id: String,
    pub status: InventoryStatusV1,
    pub observed_files: u64,
    pub source_hashes_verified: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrailOutcomeV1 {
    Proven,
    Partial,
    Rejected,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrailReportV1 {
    pub path_id: String,
    pub outcome: TrailOutcomeV1,
    pub proven_step_count: u32,
    pub observed_step_count: u32,
    pub first_failure_gate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaseDispositionV1 {
    Passed,
    FailedHardGate,
    FailedExperimental,
    FailedStable,
    DependencyBlocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaseReportV1 {
    pub case_id: String,
    pub source_id: String,
    pub disposition: CaseDispositionV1,
    pub trail: Option<TrailReportV1>,
    pub measurement_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailureBucketV1 {
    pub gate: String,
    pub expected_steps: u64,
    pub failed_steps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailureFunnelReportV1 {
    pub buckets: Vec<FailureBucketV1>,
    pub unclassified_failures: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivationOutcomeV1 {
    PublicExactVerifier,
    ExperimentalManualVerifier,
    KeepProofDark,
    DelayFullV3Cut,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivationDecisionV1 {
    pub outcome: ActivationOutcomeV1,
    pub rationale: String,
    pub automatic_thresholds_met: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QualificationSummaryV1 {
    pub schema: String,
    pub corpus_sha256: String,
    pub thresholds_sha256: String,
    pub environment: EnvironmentReportV1,
    pub inventory: Vec<InventoryReportV1>,
    pub cases: Vec<CaseReportV1>,
    pub failure_funnel: FailureFunnelReportV1,
    pub decision: Option<ActivationDecisionV1>,
}

pub fn schema_json(document: SchemaDocument) -> Value {
    match document {
        SchemaDocument::Corpus => schema_for::<CorpusV1>(CORPUS_SCHEMA),
        SchemaDocument::Path => schema_for::<OraclePathV1>(PATH_SCHEMA),
        SchemaDocument::Report => schema_for::<QualificationSummaryV1>(REPORT_SCHEMA),
        SchemaDocument::Thresholds => schema_for::<ThresholdsV1>(THRESHOLDS_SCHEMA),
    }
}

fn schema_for<T: JsonSchema>(id: &str) -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).expect("serialize schema");
    let object = schema.as_object_mut().expect("schemars root object");
    object.insert("$id".into(), Value::String(id.into()));
    object.insert(
        "$schema".into(),
        Value::String("https://json-schema.org/draft/2020-12/schema".into()),
    );
    schema
}

fn validate_source(source: &SourceRevisionV1) -> Result<()> {
    if source.source_id.is_empty()
        || source.repository.is_empty()
        || !is_lower_hex(&source.commit, 40)
        || !is_lower_hex(&source.tree, 40)
        || source.source_hashes.is_empty()
        || !unique(source.source_hashes.iter().map(|hash| hash.path.as_str()))
        || source.source_hashes.iter().any(|hash| {
            hash.path.is_empty() || hash.byte_length == 0 || !is_lower_hex(&hash.sha256, 64)
        })
    {
        bail!("proof_availability_source_invalid");
    }
    Ok(())
}

fn validate_source_range(range: &SourceRangeV1, byte_length: Option<u64>) -> Result<()> {
    if range.path.is_empty()
        || range.start_byte >= range.end_byte
        || !is_lower_hex(&range.sha256, 64)
        || byte_length.is_some_and(|length| range.end_byte > length)
    {
        bail!("proof_availability_source_range_invalid");
    }
    Ok(())
}

fn unique<'a>(mut values: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    values.all(|value| !value.is_empty() && seen.insert(value))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn all_milli(values: impl IntoIterator<Item = u16>) -> bool {
    values.into_iter().all(|value| value <= 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    #[ignore = "only used to regenerate checked-in schemas"]
    fn write_checked_in_schemas() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/proof-availability/schemas");
        fs::create_dir_all(&root).expect("create schema directory");
        for (name, document) in [
            ("corpus.schema.json", SchemaDocument::Corpus),
            ("path.schema.json", SchemaDocument::Path),
            ("report.schema.json", SchemaDocument::Report),
            ("thresholds.schema.json", SchemaDocument::Thresholds),
        ] {
            let rendered =
                serde_json::to_string_pretty(&schema_json(document)).expect("render schema");
            fs::write(root.join(name), format!("{rendered}\n")).expect("write schema");
        }
    }
}
