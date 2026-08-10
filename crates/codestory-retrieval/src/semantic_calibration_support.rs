//! Development-only evidence capture and deterministic semantic-abstention selection.

use crate::embedded_vector::{
    VectorEvidenceContract, VectorGenerationManifest, open_read_only,
    search_database_for_semantic_calibration, validate_database,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const CALIBRATION_CORPUS_SCHEMA_VERSION: u32 = 1;
pub const CALIBRATION_FEATURE: &str = "semantic-calibration-support";
pub const CALIBRATION_FIXTURE_PATH: &str =
    "crates/codestory-indexer/tests/fixtures/call_resolution_comprehensive/rust_workflow.rs";
pub const CALIBRATION_EDGE_CONTRACT_PATH: &str =
    "crates/codestory-indexer/tests/call_resolution_common_methods.rs";
pub const CALIBRATION_FIXTURE_TRANSFORMATION: &str = "rust-public-owner-anchors-v1";
pub const QUERY_VECTOR_CAPTURE_DIR_ENV: &str = "CODESTORY_SEMANTIC_CALIBRATION_QUERY_VECTOR_DIR";
const MRR_SCALE: u64 = 2_520;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationQuerySpec {
    pub task_id: &'static str,
    pub query: &'static str,
    pub expected_call: Option<(&'static str, &'static str, &'static str, &'static str)>,
    pub noise_nonce: Option<&'static str>,
}

const DEVELOPMENT_QUERIES: &[CalibrationQuerySpec] = &[
    CalibrationQuerySpec {
        task_id: "dev-workflow-notification-flow",
        query: "how does workflow execution notify observers",
        expected_call: Some(("run", "Workflow", "Notifier", "notify_event")),
        noise_nonce: None,
    },
    CalibrationQuerySpec {
        task_id: "dev-workflow-notification-delegation",
        query: "where does a workflow delegate notifications",
        expected_call: Some(("run", "Workflow", "Notifier", "notify_event")),
        noise_nonce: None,
    },
    CalibrationQuerySpec {
        task_id: "dev-workflow-repository-save",
        query: "how does workflow execution save repository values",
        expected_call: Some(("run", "Workflow", "Repository", "save")),
        noise_nonce: None,
    },
    CalibrationQuerySpec {
        task_id: "dev-workflow-repository-delegation",
        query: "where does a workflow delegate repository persistence",
        expected_call: Some(("run", "Workflow", "Repository", "save")),
        noise_nonce: None,
    },
    CalibrationQuerySpec {
        task_id: "dev-workflow-persist-flow",
        query: "how does a workflow persist a value",
        expected_call: Some(("run", "Workflow", "Workflow", "persist")),
        noise_nonce: None,
    },
    CalibrationQuerySpec {
        task_id: "dev-noise-quartz",
        query: "calibrationabsentquartz",
        expected_call: None,
        noise_nonce: Some("calibrationabsentquartz"),
    },
    CalibrationQuerySpec {
        task_id: "dev-noise-zephyr",
        query: "calibrationabsentzephyr",
        expected_call: None,
        noise_nonce: Some("calibrationabsentzephyr"),
    },
];

pub fn development_queries() -> &'static [CalibrationQuerySpec] {
    DEVELOPMENT_QUERIES
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationCaptureIdentity {
    pub source_commit: String,
    pub capture_feature: String,
    pub cli_sha256: String,
    pub vector_generation_manifest_file: String,
    pub vector_generation_manifest_sha256: String,
    pub vector_database_file: String,
    pub vector_database_sha256: String,
    pub capture_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationFixtureIdentity {
    pub source_path: String,
    pub source_sha256: String,
    pub edge_contract_path: String,
    pub edge_contract_sha256: String,
    pub transformation_id: String,
    pub materialized_sha256: String,
    pub disjointness_manifest_path: String,
    pub disjointness_manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationSelectionContract {
    pub baseline_relative_percent: u8,
    pub absolute_floor_hundredths: Vec<u8>,
    pub additive_margin_hundredths: Vec<u8>,
    pub max_retained_growth_percent: u8,
}

impl CalibrationSelectionContract {
    pub fn exact_grid() -> Self {
        Self {
            baseline_relative_percent: 50,
            absolute_floor_hundredths: (10..=50).step_by(5).collect(),
            additive_margin_hundredths: (5..=30).step_by(5).collect(),
            max_retained_growth_percent: 25,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationExpectedCall {
    pub caller: String,
    pub caller_owner: String,
    pub callee_owner: String,
    pub callee: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationCandidate {
    pub node_id: String,
    pub document_hash: String,
    pub display_name: String,
    pub file_path: String,
    pub raw_score_bits: u32,
    pub rank: usize,
}

impl CalibrationCandidate {
    pub fn raw_score(&self) -> f32 {
        f32::from_bits(self.raw_score_bits)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationQuery {
    pub task_id: String,
    pub query: String,
    pub query_sha256: String,
    pub query_vector_sha256: String,
    pub query_vector_f32_le_hex: String,
    pub expected_call: Option<CalibrationExpectedCall>,
    pub noise_nonce: Option<String>,
    pub candidates: Vec<CalibrationCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationPolicy {
    pub absolute_floor_hundredths: u8,
    pub additive_margin_hundredths: u8,
}

impl CalibrationPolicy {
    pub fn absolute_floor(self) -> f32 {
        f32::from(self.absolute_floor_hundredths) / 100.0
    }

    pub fn additive_margin(self) -> f32 {
        f32::from(self.additive_margin_hundredths) / 100.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationMetrics {
    pub mrr_at_10_scaled_2520: u64,
    pub relevant_at_10: u64,
    pub relevant_total: u64,
    pub noisy_query_false_positives: u64,
    pub retained_candidates: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationSelection {
    pub baseline: CalibrationMetrics,
    pub policy: CalibrationPolicy,
    pub metrics: CalibrationMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticCalibrationCorpus {
    pub schema_version: u32,
    pub capture: CalibrationCaptureIdentity,
    pub fixture: CalibrationFixtureIdentity,
    pub selection_contract: CalibrationSelectionContract,
    pub queries: Vec<CalibrationQuery>,
    pub selection: CalibrationSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationRawHit {
    pub node_id: String,
    pub document_hash: String,
    pub display_name: String,
    pub file_path: String,
    pub raw_score_bits: u32,
    pub rank: usize,
}

pub fn raw_semantic_scan(
    database_path: &Path,
    generation: &str,
    input_hash: &str,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<CalibrationRawHit>> {
    let connection = open_read_only(database_path)?;
    let mut statement = connection.prepare(
        "SELECT node_id, document_hash, display_name, file_path FROM vectors ORDER BY node_id ASC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut identities = BTreeMap::new();
    for row in rows {
        let (node_id, document_hash, display_name, file_path) = row?;
        identities.insert(node_id, (document_hash, display_name, file_path));
    }
    drop(statement);
    drop(connection);

    search_database_for_semantic_calibration(
        database_path,
        generation,
        input_hash,
        query_vector,
        limit,
    )?
    .into_iter()
    .enumerate()
    .map(|(index, hit)| {
        let node_id = hit
            .node_id
            .context("raw semantic candidate is missing its node id")?;
        let (document_hash, display_name, stored_path) = identities
            .get(&node_id)
            .with_context(|| format!("raw semantic candidate {node_id} is absent from vectors"))?;
        if hit.symbol_name.as_deref() != Some(display_name.as_str()) {
            bail!("raw semantic candidate {node_id} changed its display name");
        }
        let file_path = stored_path.clone().unwrap_or_else(|| display_name.clone());
        if hit.file_path != file_path {
            bail!("raw semantic candidate {node_id} changed its file path");
        }
        Ok(CalibrationRawHit {
            node_id,
            document_hash: document_hash.clone(),
            display_name: display_name.clone(),
            file_path,
            raw_score_bits: hit.score.to_bits(),
            rank: index + 1,
        })
    })
    .collect()
}

pub fn validate_vector_artifacts(manifest_path: &Path, database_path: &Path) -> Result<()> {
    let manifest: VectorGenerationManifest = serde_json::from_slice(
        &std::fs::read(manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parse {}", manifest_path.display()))?;
    manifest.validate()?;
    let connection = open_read_only(database_path)?;
    let mut statement =
        connection.prepare("SELECT node_id, document_hash FROM vectors ORDER BY node_id ASC")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut anchors = BTreeMap::new();
    for row in rows {
        let (node_id, document_hash) = row?;
        anchors.insert(node_id, document_hash);
    }
    drop(statement);
    drop(connection);
    let contract = VectorEvidenceContract::new(
        &manifest.vectors.embedding_backend,
        manifest.vectors.embedding_dim,
        &manifest.vectors.producer_identity,
        &manifest.vectors.evidence_contract_identity,
    );
    validate_database(
        database_path,
        &manifest.vectors.generation,
        &manifest.vectors.input_hash,
        &contract,
        &anchors,
        Some(&manifest.vectors),
    )?;
    Ok(())
}

pub fn load_attested_corpus(
    directory: &Path,
    repository_root: &Path,
    disjointness_manifest_path: &str,
) -> Result<SemanticCalibrationCorpus> {
    let corpus_path = directory.join("capture.json");
    let corpus: SemanticCalibrationCorpus =
        serde_json::from_slice(&std::fs::read(&corpus_path).with_context(|| {
            format!(
                "attested semantic calibration corpus is unavailable at {}",
                corpus_path.display()
            )
        })?)
        .with_context(|| {
            format!(
                "parse attested calibration corpus {}",
                corpus_path.display()
            )
        })?;
    validate_corpus_shape(&corpus)?;
    validate_attested_repository_inputs(&corpus, repository_root, disjointness_manifest_path)?;
    let manifest_path =
        checked_artifact_path(directory, &corpus.capture.vector_generation_manifest_file)?;
    let database_path = checked_artifact_path(directory, &corpus.capture.vector_database_file)?;
    if sha256_file(&manifest_path)? != corpus.capture.vector_generation_manifest_sha256 {
        bail!("semantic calibration vector manifest digest mismatch");
    }
    if sha256_file(&database_path)? != corpus.capture.vector_database_sha256 {
        bail!("semantic calibration vector database digest mismatch");
    }
    validate_vector_artifacts(&manifest_path, &database_path)?;
    let vector_manifest: VectorGenerationManifest = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parse {}", manifest_path.display()))?;
    for query in &corpus.queries {
        let vector = query_vector_from_hex(&query.query_vector_f32_le_hex)?;
        if vector.len() != vector_manifest.vectors.embedding_dim {
            bail!("semantic calibration query vector dimension mismatch");
        }
        let replayed = raw_semantic_scan(
            &database_path,
            &vector_manifest.vectors.generation,
            &vector_manifest.vectors.input_hash,
            &vector,
            query.candidates.len(),
        )?;
        if replayed.len() != query.candidates.len()
            || replayed
                .iter()
                .zip(&query.candidates)
                .any(|(actual, expected)| {
                    actual.node_id != expected.node_id
                        || actual.document_hash != expected.document_hash
                        || actual.display_name != expected.display_name
                        || actual.file_path != expected.file_path
                        || actual.raw_score_bits != expected.raw_score_bits
                        || actual.rank != expected.rank
                })
        {
            bail!("semantic calibration raw dense replay does not match the corpus");
        }
    }
    let selected = select_policy(&corpus)?;
    if selected != corpus.selection {
        bail!("semantic calibration selection does not match the attested raw corpus");
    }
    Ok(corpus)
}

pub fn validate_attested_repository_inputs(
    corpus: &SemanticCalibrationCorpus,
    repository_root: &Path,
    disjointness_manifest_path: &str,
) -> Result<()> {
    if corpus.fixture.source_path != CALIBRATION_FIXTURE_PATH
        || corpus.fixture.edge_contract_path != CALIBRATION_EDGE_CONTRACT_PATH
        || corpus.fixture.disjointness_manifest_path != disjointness_manifest_path
        || corpus.fixture.transformation_id != CALIBRATION_FIXTURE_TRANSFORMATION
    {
        bail!("semantic calibration corpus changed its source-owned input contract");
    }
    validate_development_query_contract(corpus)?;

    let fixture_path = repository_root.join(CALIBRATION_FIXTURE_PATH);
    let edge_contract_path = repository_root.join(CALIBRATION_EDGE_CONTRACT_PATH);
    let disjointness_manifest_path = repository_root.join(disjointness_manifest_path);
    if sha256_file(&fixture_path)? != corpus.fixture.source_sha256 {
        bail!("semantic calibration source fixture digest mismatch");
    }
    if sha256_file(&edge_contract_path)? != corpus.fixture.edge_contract_sha256 {
        bail!("semantic calibration edge contract digest mismatch");
    }
    if sha256_file(&disjointness_manifest_path)? != corpus.fixture.disjointness_manifest_sha256 {
        bail!("semantic calibration disjointness manifest digest mismatch");
    }

    let source = std::fs::read_to_string(&fixture_path)
        .with_context(|| format!("read {}", fixture_path.display()))?;
    let materialized_source = materialize_public_owner_fixture(&source)?;
    if sha256_bytes(materialized_source.as_bytes()) != corpus.fixture.materialized_sha256 {
        bail!("semantic calibration materialized fixture digest mismatch");
    }
    let edge_contract = std::fs::read_to_string(&edge_contract_path)
        .with_context(|| format!("read {}", edge_contract_path.display()))?;
    validate_fixture_grounding(corpus, &materialized_source, &edge_contract)?;
    let disjointness_manifest = std::fs::read(&disjointness_manifest_path)
        .with_context(|| format!("read {}", disjointness_manifest_path.display()))?;
    validate_holdout_disjointness(corpus, &disjointness_manifest)
}

pub fn query_vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_bits().to_le_bytes())
        .collect()
}

pub fn query_vector_from_hex(hex: &str) -> Result<Vec<f32>> {
    let bytes = decode_hex(hex)?;
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        bail!("query vector hex must contain complete f32 values");
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_bits(u32::from_le_bytes(chunk.try_into().expect("four bytes"))))
        .collect())
}

pub fn hex_bytes(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<String> {
    Ok(sha256_bytes(
        &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
    ))
}

pub fn materialize_public_owner_fixture(source: &str) -> Result<String> {
    let replacements = [
        ("trait Notifier {", "pub trait Notifier {"),
        ("trait Repository<T> {", "pub trait Repository<T> {"),
        ("struct EmailNotifier;", "pub struct EmailNotifier;"),
        ("struct MemoryRepository;", "pub struct MemoryRepository;"),
        ("trait Workflow {", "pub trait Workflow {"),
        ("struct CheckoutWorkflow;", "pub struct CheckoutWorkflow;"),
    ];
    let mut materialized = source.to_string();
    for (from, to) in replacements {
        if materialized.matches(from).count() != 1 {
            bail!("fixture visibility transform expected one `{from}` declaration");
        }
        materialized = materialized.replacen(from, to, 1);
    }
    Ok(materialized)
}

pub fn validate_fixture_grounding(
    corpus: &SemanticCalibrationCorpus,
    materialized_source: &str,
    edge_contract: &str,
) -> Result<()> {
    let rust_edges = edge_contract
        .split_once("const RUST_RESOLVED: &[ResolvedCallExpectation] = &[")
        .and_then(|(_, tail)| tail.split_once("const RUST_RESOLVED_BY_NAME"))
        .map(|(section, _)| section)
        .context("locate the fixture-owned Rust resolved-call contract")?;
    let source_tokens = tokens(materialized_source);
    let candidate_tokens = corpus
        .queries
        .iter()
        .flat_map(|query| query.candidates.iter())
        .flat_map(|candidate| tokens(&candidate.display_name))
        .collect::<BTreeSet<_>>();
    for query in &corpus.queries {
        if let Some(edge) = &query.expected_call {
            let expected = format!(
                "(\"{}\", \"{}\", \"{}\")",
                edge.caller, edge.callee_owner, edge.callee
            );
            if !rust_edges.contains(&expected) {
                bail!("calibration edge is absent from the fixture contract: {expected}");
            }
            if !materialized_source.contains(&format!("trait {} {{", edge.caller_owner))
                || !materialized_source.contains(&format!("fn {}(", edge.caller))
            {
                bail!("calibration caller owner is absent from the fixture source");
            }
        } else {
            let nonce = query
                .noise_nonce
                .as_deref()
                .context("noise calibration query omitted its nonce")?;
            if query.query != nonce
                || source_tokens.contains(nonce)
                || candidate_tokens.contains(nonce)
            {
                bail!("noise calibration nonce occurs in fixture evidence");
            }
        }
    }
    Ok(())
}

pub fn validate_holdout_disjointness(
    corpus: &SemanticCalibrationCorpus,
    holdout_manifest: &[u8],
) -> Result<()> {
    let manifest: Value =
        serde_json::from_slice(holdout_manifest).context("parse canonical holdout manifest")?;
    let tasks = manifest
        .get("tasks")
        .and_then(Value::as_array)
        .context("canonical holdout manifest omitted tasks")?;
    if tasks.len() != 18 {
        bail!(
            "semantic calibration disjointness expected 18 holdout tasks, found {}",
            tasks.len()
        );
    }
    let task_ids = tasks
        .iter()
        .map(|task| nested_string(task, &["id"]))
        .collect::<Result<BTreeSet<_>>>()?;
    let repository_refs = tasks
        .iter()
        .map(|task| nested_string(task, &["repo", "ref"]))
        .collect::<Result<BTreeSet<_>>>()?;
    let query_hashes = tasks
        .iter()
        .map(|task| nested_string(task, &["prompt"]).map(|prompt| sha256_bytes(prompt.as_bytes())))
        .collect::<Result<BTreeSet<_>>>()?;
    if repository_refs.contains(corpus.capture.source_commit.as_str()) {
        bail!("development calibration source commit overlaps a holdout repository ref");
    }
    for query in &corpus.queries {
        if task_ids.contains(query.task_id.as_str())
            || query_hashes.contains(query.query_sha256.as_str())
        {
            bail!("development calibration query overlaps the holdout corpus");
        }
    }
    Ok(())
}

pub fn select_policy(corpus: &SemanticCalibrationCorpus) -> Result<CalibrationSelection> {
    validate_corpus_shape(corpus)?;
    let baseline = evaluate(corpus, RetentionRule::RelativeBaseline)?;
    let max_growth = u64::from(corpus.selection_contract.max_retained_growth_percent);
    let mut best: Option<CalibrationSelection> = None;
    for &floor in &corpus.selection_contract.absolute_floor_hundredths {
        for &margin in &corpus.selection_contract.additive_margin_hundredths {
            let policy = CalibrationPolicy {
                absolute_floor_hundredths: floor,
                additive_margin_hundredths: margin,
            };
            let metrics = evaluate(corpus, RetentionRule::AbsoluteAdditive(policy))?;
            if metrics.relevant_at_10 < baseline.relevant_at_10
                || metrics.noisy_query_false_positives > baseline.noisy_query_false_positives
                || metrics.retained_candidates.saturating_mul(100)
                    > baseline
                        .retained_candidates
                        .saturating_mul(100 + max_growth)
            {
                continue;
            }
            let candidate = CalibrationSelection {
                baseline,
                policy,
                metrics,
            };
            if best
                .as_ref()
                .is_none_or(|current| selection_is_better(&candidate, current))
            {
                best = Some(candidate);
            }
        }
    }
    best.context("no semantic abstention floor/margin pair satisfies the calibration constraints")
}

fn validate_corpus_shape(corpus: &SemanticCalibrationCorpus) -> Result<()> {
    if corpus.schema_version != CALIBRATION_CORPUS_SCHEMA_VERSION {
        bail!("unsupported semantic calibration corpus schema");
    }
    if corpus.capture.capture_feature != CALIBRATION_FEATURE {
        bail!("semantic calibration corpus was not captured with the raw-lane feature");
    }
    if corpus.selection_contract != CalibrationSelectionContract::exact_grid() {
        bail!("semantic calibration corpus changed the authorized grid or constraints");
    }
    if corpus.queries.is_empty() {
        bail!("semantic calibration corpus has no queries");
    }
    let mut task_ids = BTreeSet::new();
    let mut query_hashes = BTreeSet::new();
    for query in &corpus.queries {
        if !task_ids.insert(query.task_id.as_str()) {
            bail!("semantic calibration corpus contains a duplicate task id");
        }
        if !query_hashes.insert(query.query_sha256.as_str()) {
            bail!("semantic calibration corpus contains a duplicate query hash");
        }
        if sha256_bytes(query.query.as_bytes()) != query.query_sha256 {
            bail!(
                "semantic calibration query hash mismatch for {}",
                query.task_id
            );
        }
        let vector_bytes = decode_hex(&query.query_vector_f32_le_hex)?;
        if sha256_bytes(&vector_bytes) != query.query_vector_sha256 {
            bail!(
                "semantic calibration query vector hash mismatch for {}",
                query.task_id
            );
        }
        if query.expected_call.is_some() == query.noise_nonce.is_some() {
            bail!("semantic calibration query must be either edge-backed or nonce noise");
        }
        if query.candidates.is_empty() {
            bail!("semantic calibration query has no raw candidates");
        }
        let mut previous = f32::INFINITY;
        let mut node_ids = BTreeSet::new();
        for (index, candidate) in query.candidates.iter().enumerate() {
            let score = candidate.raw_score();
            if !score.is_finite() || score > previous || candidate.rank != index + 1 {
                bail!(
                    "semantic calibration candidate order is invalid for {}",
                    query.task_id
                );
            }
            if !node_ids.insert(candidate.node_id.as_str()) {
                bail!("semantic calibration query repeats a candidate node id");
            }
            previous = score;
        }
    }
    Ok(())
}

fn validate_development_query_contract(corpus: &SemanticCalibrationCorpus) -> Result<()> {
    if corpus.queries.len() != DEVELOPMENT_QUERIES.len() {
        bail!("semantic calibration corpus changed the development query set");
    }
    for (query, expected) in corpus.queries.iter().zip(DEVELOPMENT_QUERIES) {
        let observed_call = query.expected_call.as_ref().map(|edge| {
            (
                edge.caller.as_str(),
                edge.caller_owner.as_str(),
                edge.callee_owner.as_str(),
                edge.callee.as_str(),
            )
        });
        if query.task_id != expected.task_id
            || query.query != expected.query
            || observed_call != expected.expected_call
            || query.noise_nonce.as_deref() != expected.noise_nonce
        {
            bail!("semantic calibration corpus changed the development query contract");
        }
    }
    Ok(())
}

fn checked_artifact_path(directory: &Path, file_name: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(file_name);
    if path.components().count() != 1
        || path.file_name().and_then(|name| name.to_str()) != Some(file_name)
    {
        bail!("semantic calibration artifact name must be a single UTF-8 file name");
    }
    Ok(directory.join(path))
}

#[derive(Debug, Clone, Copy)]
enum RetentionRule {
    RelativeBaseline,
    AbsoluteAdditive(CalibrationPolicy),
}

fn evaluate(corpus: &SemanticCalibrationCorpus, rule: RetentionRule) -> Result<CalibrationMetrics> {
    let mut metrics = CalibrationMetrics::default();
    for query in &corpus.queries {
        let best = query
            .candidates
            .first()
            .context("calibration query lost its best candidate")?
            .raw_score();
        let retained = query
            .candidates
            .iter()
            .filter(|candidate| retains(rule, best, candidate.raw_score()))
            .take(10)
            .collect::<Vec<_>>();
        metrics.retained_candidates = metrics
            .retained_candidates
            .saturating_add(retained.len() as u64);
        if let Some(edge) = &query.expected_call {
            let relevant = BTreeSet::from([edge.caller_owner.as_str(), edge.callee_owner.as_str()]);
            metrics.relevant_total = metrics.relevant_total.saturating_add(relevant.len() as u64);
            let found = retained
                .iter()
                .filter(|candidate| relevant.contains(candidate.display_name.as_str()))
                .map(|candidate| candidate.display_name.as_str())
                .collect::<BTreeSet<_>>();
            metrics.relevant_at_10 = metrics.relevant_at_10.saturating_add(found.len() as u64);
            if let Some(rank) = retained
                .iter()
                .position(|candidate| relevant.contains(candidate.display_name.as_str()))
            {
                metrics.mrr_at_10_scaled_2520 = metrics
                    .mrr_at_10_scaled_2520
                    .saturating_add(MRR_SCALE / (rank as u64 + 1));
            }
        } else {
            metrics.noisy_query_false_positives = metrics
                .noisy_query_false_positives
                .saturating_add(retained.len() as u64);
        }
    }
    Ok(metrics)
}

fn retains(rule: RetentionRule, best: f32, score: f32) -> bool {
    match rule {
        RetentionRule::RelativeBaseline => score > 0.0 && score >= best * 0.5,
        RetentionRule::AbsoluteAdditive(policy) => {
            best >= policy.absolute_floor()
                && score.is_finite()
                && score >= policy.absolute_floor()
                && best - score <= policy.additive_margin()
        }
    }
}

fn selection_is_better(candidate: &CalibrationSelection, current: &CalibrationSelection) -> bool {
    selection_key(candidate) > selection_key(current)
}

fn selection_key(
    selection: &CalibrationSelection,
) -> (u64, u64, Reverse<u64>, Reverse<u64>, u8, Reverse<u8>) {
    (
        selection.metrics.mrr_at_10_scaled_2520,
        selection.metrics.relevant_at_10,
        Reverse(selection.metrics.noisy_query_false_positives),
        Reverse(selection.metrics.retained_candidates),
        selection.policy.absolute_floor_hundredths,
        Reverse(selection.policy.additive_margin_hundredths),
    )
}

fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        bail!("hex input must have an even length");
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .with_context(|| format!("invalid hex byte at offset {index}"))
        })
        .collect()
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn nested_string<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
    let value = path.iter().try_fold(value, |current, key| {
        current
            .get(*key)
            .with_context(|| format!("holdout manifest omitted {}", path.join(".")))
    })?;
    value
        .as_str()
        .with_context(|| format!("holdout field {} is not a string", path.join(".")))
}
