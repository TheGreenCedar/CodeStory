//! Emit SCIP-shaped symbol artifacts from the CodeStory SQLite graph.

use anyhow::{Context, Result, bail};
use codestory_contracts::graph::{EdgeKind, NodeId, NodeKind};
use codestory_store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::path::Path;

pub const SCIP_SYMBOLS_FILE: &str = "symbols.index.json";
pub const SCIP_INDEX_FILE: &str = "index.scip";
pub const SCIP_PRECISE_SEMANTIC_IMPORT_DIR: &str = "precise-semantic-import";
pub const SCIP_IMPORTED_PROOF_PROVENANCE: &str = "imported_scip_proof";
pub const SCIP_PRECISE_SEMANTIC_IMPORT_PUBLIC_PROVENANCE: &str = "precise_semantic_import";
pub const SCIP_GRAPH_PROJECTION_PROVENANCE: &str = "scip_graph_projection";
pub const SCIP_DEFINITION_ROLE: &str = "definition";
pub const SCIP_REFERENCE_ROLE: &str = "reference";
const SCIP_POSITION_ENCODING: &str = "line_one_based_utf16_column_zero_based";
const STUB_MARKER: &str = "index.scip.stub";

/// Graph edge kinds that carry a real reference from one emitted symbol to
/// another. `MEMBER` is containment rather than a reference and `UNKNOWN` is
/// an unresolved relationship, so neither produces reference adjacency.
const SCIP_REFERENCE_EDGE_KINDS: [EdgeKind; 11] = [
    EdgeKind::TYPE_USAGE,
    EdgeKind::USAGE,
    EdgeKind::CALL,
    EdgeKind::INHERITANCE,
    EdgeKind::OVERRIDE,
    EdgeKind::TYPE_ARGUMENT,
    EdgeKind::TEMPLATE_SPECIALIZATION,
    EdgeKind::INCLUDE,
    EdgeKind::IMPORT,
    EdgeKind::MACRO_USAGE,
    EdgeKind::ANNOTATION_USAGE,
];

/// Seed nodes per edge lookup while collecting reference adjacency. Edge rows
/// for one seed batch are discarded before the next batch is read.
const SCIP_ADJACENCY_SEED_BATCH: usize = 512;

/// Outgoing reference records kept per referencing symbol. Pathological
/// fan-out cannot inflate the artifact; dropping records only removes
/// neighbours, never adds one.
const SCIP_MAX_REFERENCES_PER_SYMBOL: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipSymbolRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub path: String,
    pub symbol: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScipPackageIdentity {
    pub manager: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScipProofAdapterContract {
    pub evidence_source: String,
    pub producer: String,
    pub producer_version: String,
    pub producer_args: Vec<String>,
    pub producer_config: String,
    pub revision: String,
    pub package: ScipPackageIdentity,
    pub position_encoding: String,
    pub freshness: String,
}

impl ScipProofAdapterContract {
    pub fn graph_projection(revision: &str) -> Self {
        Self {
            evidence_source: SCIP_GRAPH_PROJECTION_PROVENANCE.into(),
            producer: "codestory-retrieval".into(),
            producer_version: env!("CARGO_PKG_VERSION").into(),
            producer_args: vec!["emit_scip_artifacts_from_store".into()],
            producer_config: "canonical_node_pages".into(),
            revision: revision.into(),
            package: ScipPackageIdentity {
                manager: "codestory".into(),
                name: "local-workspace".into(),
                version: None,
            },
            position_encoding: SCIP_POSITION_ENCODING.into(),
            freshness: "fresh".into(),
        }
    }

    pub(crate) fn evidence_source(&self) -> Option<ScipEvidenceSource> {
        match self.evidence_source.as_str() {
            SCIP_IMPORTED_PROOF_PROVENANCE => Some(ScipEvidenceSource::ImportedProof),
            SCIP_GRAPH_PROJECTION_PROVENANCE => Some(ScipEvidenceSource::GraphProjection),
            _ => None,
        }
    }

    pub(crate) fn provenance_label(&self) -> Option<&'static str> {
        match self.evidence_source()? {
            ScipEvidenceSource::ImportedProof => {
                Some(SCIP_PRECISE_SEMANTIC_IMPORT_PUBLIC_PROVENANCE)
            }
            ScipEvidenceSource::GraphProjection => Some(SCIP_GRAPH_PROJECTION_PROVENANCE),
        }
    }

    pub(crate) fn is_fresh_for(&self, revision: &str) -> bool {
        self.revision == revision
            && self.freshness == "fresh"
            && self.position_encoding == SCIP_POSITION_ENCODING
            && self.evidence_source().is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScipEvidenceSource {
    ImportedProof,
    GraphProjection,
}

impl Default for ScipProofAdapterContract {
    fn default() -> Self {
        let mut contract = Self::graph_projection("");
        contract.freshness = "stale".into();
        contract
    }
}

/// One SCIP occurrence record.
///
/// A `definition` record locates the symbol's own range. A `reference` record
/// locates the *referencing* symbol's range and names the symbol it refers to
/// in `target_symbol`; the graph-projection lane additionally binds both ends
/// to graph node identities so adjacency never falls back to name matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScipProofRecord {
    pub role: String,
    pub path: String,
    pub symbol: String,
    pub start_line: u32,
    pub start_character_utf16: u32,
    pub end_line: u32,
    pub end_character_utf16: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_node_id: Option<String>,
}

impl ScipProofRecord {
    fn definition(symbol: &ScipSymbolRecord) -> Self {
        Self {
            role: SCIP_DEFINITION_ROLE.into(),
            path: symbol.path.clone(),
            symbol: symbol.symbol.clone(),
            start_line: symbol.start_line,
            start_character_utf16: 0,
            end_line: symbol.end_line,
            end_character_utf16: 0,
            target_symbol: None,
            node_id: symbol.node_id.clone(),
            target_node_id: None,
        }
    }

    fn reference(source: &ScipSymbolRecord, target: &ScipSymbolRecord) -> Self {
        Self {
            role: SCIP_REFERENCE_ROLE.into(),
            path: source.path.clone(),
            symbol: source.symbol.clone(),
            start_line: source.start_line,
            start_character_utf16: 0,
            end_line: source.end_line,
            end_character_utf16: 0,
            target_symbol: Some(target.symbol.clone()),
            node_id: source.node_id.clone(),
            target_node_id: target.node_id.clone(),
        }
    }

    pub(crate) fn is_reference(&self) -> bool {
        self.role == SCIP_REFERENCE_ROLE
    }
}

/// Why one artifact record is not admissible evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScipProofDefect {
    EmptyPath,
    EmptySymbol,
    InvalidRange,
    UnknownRole,
    DefinitionNotInSymbols,
    ReferenceMissingTargetSymbol,
    ReferenceTargetSymbolUnknown,
    ReferenceMissingNodeIdentity,
    ReferenceUnknownNodeIdentity,
    ReferenceNodeIdentityDisagreesWithSymbol,
    ReferenceSelfLoop,
    ReferenceForgedNodeIdentity,
}

impl fmt::Display for ScipProofDefect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::EmptyPath => "empty_path",
            Self::EmptySymbol => "empty_symbol",
            Self::InvalidRange => "invalid_range",
            Self::UnknownRole => "unknown_role",
            Self::DefinitionNotInSymbols => "definition_not_in_symbols",
            Self::ReferenceMissingTargetSymbol => "reference_missing_target_symbol",
            Self::ReferenceTargetSymbolUnknown => "reference_target_symbol_unknown",
            Self::ReferenceMissingNodeIdentity => "reference_missing_node_identity",
            Self::ReferenceUnknownNodeIdentity => "reference_unknown_node_identity",
            Self::ReferenceNodeIdentityDisagreesWithSymbol => {
                "reference_node_identity_disagrees_with_symbol"
            }
            Self::ReferenceSelfLoop => "reference_self_loop",
            Self::ReferenceForgedNodeIdentity => "reference_forged_node_identity",
        };
        formatter.write_str(label)
    }
}

/// Why one artifact is not admissible evidence for a retrieval generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScipArtifactDefect {
    MissingGeneration,
    GenerationMismatch {
        expected: String,
        actual: String,
    },
    RevisionMismatch {
        expected: String,
        actual: String,
    },
    UnknownEvidenceSource {
        evidence_source: String,
    },
    InvalidRecord {
        index: usize,
        defect: ScipProofDefect,
    },
    NoValidProofRecords,
}

impl fmt::Display for ScipArtifactDefect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingGeneration => formatter.write_str("scip artifact carries no generation"),
            Self::GenerationMismatch { expected, actual } => write!(
                formatter,
                "scip artifact generation {actual:?} does not match retrieval generation {expected:?}"
            ),
            Self::RevisionMismatch { expected, actual } => write!(
                formatter,
                "scip artifact revision {actual:?} does not match expected revision {expected:?}"
            ),
            Self::UnknownEvidenceSource { evidence_source } => {
                write!(
                    formatter,
                    "unknown scip evidence source {evidence_source:?}"
                )
            }
            Self::InvalidRecord { index, defect } => {
                write!(formatter, "scip record {index} is invalid: {defect}")
            }
            Self::NoValidProofRecords => {
                formatter.write_str("scip artifact carries no valid proof record")
            }
        }
    }
}

impl std::error::Error for ScipArtifactDefect {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipSymbolsIndex {
    /// Retrieval generation this artifact was built for or admitted into.
    /// Legacy artifacts deserialize with an empty generation and are refused.
    #[serde(default)]
    pub generation: String,
    pub revision: String,
    #[serde(default)]
    pub contract: ScipProofAdapterContract,
    pub symbols: Vec<ScipSymbolRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<ScipProofRecord>,
}

impl ScipSymbolsIndex {
    /// Admission check on the retrieval hot path: the artifact must name this
    /// generation and revision, carry a known contract, and hold proof records.
    ///
    /// This deliberately stays cheap. Whole-artifact validation runs once at
    /// generation build and at import, and every reference record is validated
    /// again at the moment it is consumed as adjacency.
    pub(crate) fn is_fresh_for(&self, revision: &str, generation: &str) -> bool {
        !generation.is_empty()
            && self.generation == generation
            && self.revision == revision
            && self.contract.is_fresh_for(revision)
            && self.has_required_proof_records()
    }

    fn has_required_proof_records(&self) -> bool {
        match self.contract.evidence_source() {
            Some(ScipEvidenceSource::GraphProjection) => !self.proofs.is_empty(),
            Some(ScipEvidenceSource::ImportedProof) => {
                let lookup = ScipSymbolLookup::new(&self.symbols);
                self.proofs.iter().any(|proof| {
                    self.proof_defect(proof, &lookup, ScipEvidenceSource::ImportedProof)
                        .is_none()
                })
            }
            None => false,
        }
    }

    /// Whole-artifact validation. Every record must be admissible and at least
    /// one must exist, so a single forged or stale record refuses the artifact.
    pub(crate) fn validate_records(&self, generation: &str) -> Result<(), ScipArtifactDefect> {
        if generation.trim().is_empty() {
            return Err(ScipArtifactDefect::MissingGeneration);
        }
        if self.generation != generation {
            return Err(ScipArtifactDefect::GenerationMismatch {
                expected: generation.to_string(),
                actual: self.generation.clone(),
            });
        }
        if self.contract.revision != self.revision {
            return Err(ScipArtifactDefect::RevisionMismatch {
                expected: self.revision.clone(),
                actual: self.contract.revision.clone(),
            });
        }
        let Some(source) = self.contract.evidence_source() else {
            return Err(ScipArtifactDefect::UnknownEvidenceSource {
                evidence_source: self.contract.evidence_source.clone(),
            });
        };
        let lookup = ScipSymbolLookup::new(&self.symbols);
        for (index, proof) in self.proofs.iter().enumerate() {
            if let Some(defect) = self.proof_defect(proof, &lookup, source) {
                return Err(ScipArtifactDefect::InvalidRecord { index, defect });
            }
        }
        if self.proofs.is_empty() {
            return Err(ScipArtifactDefect::NoValidProofRecords);
        }
        Ok(())
    }

    fn proof_defect(
        &self,
        proof: &ScipProofRecord,
        lookup: &ScipSymbolLookup<'_>,
        source: ScipEvidenceSource,
    ) -> Option<ScipProofDefect> {
        if proof.path.trim().is_empty() {
            return Some(ScipProofDefect::EmptyPath);
        }
        if proof.symbol.trim().is_empty() {
            return Some(ScipProofDefect::EmptySymbol);
        }
        if proof.start_line == 0
            || proof.end_line < proof.start_line
            || (proof.end_line == proof.start_line
                && proof.end_character_utf16 < proof.start_character_utf16)
        {
            return Some(ScipProofDefect::InvalidRange);
        }

        match proof.role.as_str() {
            SCIP_DEFINITION_ROLE => (!lookup.has_containing_definition(proof))
                .then_some(ScipProofDefect::DefinitionNotInSymbols),
            SCIP_REFERENCE_ROLE => reference_defect(proof, lookup, source),
            _ => Some(ScipProofDefect::UnknownRole),
        }
    }
}

/// Validate one reference record against the symbols the same artifact
/// publishes. Graph-projection references must be bound to node identity on
/// both ends; imported references may not claim graph node identity at all.
pub(crate) fn reference_defect(
    proof: &ScipProofRecord,
    lookup: &ScipSymbolLookup<'_>,
    source: ScipEvidenceSource,
) -> Option<ScipProofDefect> {
    let target_symbol = proof.target_symbol.as_deref().unwrap_or("").trim();
    if target_symbol.is_empty() {
        return Some(ScipProofDefect::ReferenceMissingTargetSymbol);
    }
    match source {
        ScipEvidenceSource::ImportedProof => {
            if proof.node_id.is_some() || proof.target_node_id.is_some() {
                return Some(ScipProofDefect::ReferenceForgedNodeIdentity);
            }
            (!lookup.has_symbol_named(target_symbol))
                .then_some(ScipProofDefect::ReferenceTargetSymbolUnknown)
        }
        ScipEvidenceSource::GraphProjection => {
            let (Some(node_id), Some(target_node_id)) =
                (proof.node_id.as_deref(), proof.target_node_id.as_deref())
            else {
                return Some(ScipProofDefect::ReferenceMissingNodeIdentity);
            };
            if node_id == target_node_id {
                return Some(ScipProofDefect::ReferenceSelfLoop);
            }
            let (Some(referencing), Some(referenced)) = (
                lookup.symbol_for_node(node_id),
                lookup.symbol_for_node(target_node_id),
            ) else {
                return Some(ScipProofDefect::ReferenceUnknownNodeIdentity);
            };
            if referencing.path != proof.path
                || referencing.symbol != proof.symbol
                || referenced.symbol != target_symbol
            {
                return Some(ScipProofDefect::ReferenceNodeIdentityDisagreesWithSymbol);
            }
            None
        }
    }
}

/// Symbol lookups over one artifact, so validation is linear in records
/// instead of quadratic in records times symbols.
pub(crate) struct ScipSymbolLookup<'a> {
    by_node_id: HashMap<&'a str, &'a ScipSymbolRecord>,
    by_path_and_symbol: HashMap<(&'a str, &'a str), Vec<&'a ScipSymbolRecord>>,
    symbol_names: BTreeSet<&'a str>,
}

impl<'a> ScipSymbolLookup<'a> {
    pub(crate) fn new(symbols: &'a [ScipSymbolRecord]) -> Self {
        let mut by_node_id = HashMap::new();
        let mut by_path_and_symbol: HashMap<(&str, &str), Vec<&ScipSymbolRecord>> = HashMap::new();
        let mut symbol_names = BTreeSet::new();
        for symbol in symbols {
            if let Some(node_id) = symbol.node_id.as_deref() {
                by_node_id.entry(node_id).or_insert(symbol);
            }
            by_path_and_symbol
                .entry((symbol.path.as_str(), symbol.symbol.as_str()))
                .or_default()
                .push(symbol);
            symbol_names.insert(symbol.symbol.as_str());
        }
        Self {
            by_node_id,
            by_path_and_symbol,
            symbol_names,
        }
    }

    pub(crate) fn symbol_for_node(&self, node_id: &str) -> Option<&'a ScipSymbolRecord> {
        self.by_node_id.get(node_id).copied()
    }

    fn has_symbol_named(&self, symbol: &str) -> bool {
        self.symbol_names.contains(symbol)
    }

    fn has_containing_definition(&self, proof: &ScipProofRecord) -> bool {
        self.by_path_and_symbol
            .get(&(proof.path.as_str(), proof.symbol.as_str()))
            .is_some_and(|symbols| {
                symbols.iter().any(|symbol| {
                    symbol.start_line <= proof.start_line && symbol.end_line >= proof.end_line
                })
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreciseSemanticImportStatus {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
}

impl PreciseSemanticImportStatus {
    pub fn missing(reason: impl Into<String>) -> Self {
        Self {
            status: "missing".into(),
            reason: Some(reason.into()),
            revision: None,
            producer: None,
        }
    }

    pub fn invalid(reason: impl Into<String>) -> Self {
        Self {
            status: "invalid".into(),
            reason: Some(reason.into()),
            revision: None,
            producer: None,
        }
    }
}

/// Admit a configured external SCIP artifact into one retrieval generation.
///
/// The stored copy is stamped with the generation it was admitted into, so a
/// directory carried across generations is refused instead of re-served.
pub fn import_precise_semantic_scip_artifact(
    artifact_path: &Path,
    import_dir: &Path,
    generation: &str,
) -> Result<PreciseSemanticImportStatus> {
    if generation.trim().is_empty() {
        return Ok(PreciseSemanticImportStatus::invalid(
            "imported_proof_generation_missing",
        ));
    }
    if !artifact_path.is_file() {
        return Ok(PreciseSemanticImportStatus::missing(
            "configured_artifact_missing",
        ));
    }
    let body = std::fs::read_to_string(artifact_path).context("read precise semantic import")?;
    let mut index = match serde_json::from_str::<ScipSymbolsIndex>(&body) {
        Ok(index) => index,
        Err(error) => {
            return Ok(PreciseSemanticImportStatus::invalid(format!(
                "invalid_artifact_json: {error}"
            )));
        }
    };
    index.generation = generation.to_string();
    let revision = index.revision.clone();
    if index.validate_records(generation).is_err() || !index.is_fresh_for(&revision, generation) {
        return Ok(PreciseSemanticImportStatus::invalid(
            "imported_proof_contract_invalid",
        ));
    }
    let generation_bound_body =
        serde_json::to_string_pretty(&index).context("serialize precise semantic import")?;
    std::fs::create_dir_all(import_dir).with_context(|| {
        format!(
            "create precise semantic import dir {}",
            import_dir.display()
        )
    })?;
    std::fs::write(import_dir.join(SCIP_SYMBOLS_FILE), generation_bound_body)
        .context("write precise semantic import symbols")?;
    std::fs::write(
        import_dir.join("revision.txt"),
        format!("{}\n", index.revision),
    )
    .context("write precise semantic import revision")?;
    std::fs::write(
        import_dir.join(SCIP_INDEX_FILE),
        format!(
            "codestory-precise-semantic-import-v1\nrevision={}\n",
            index.revision
        ),
    )
    .context("write precise semantic import marker")?;
    Ok(PreciseSemanticImportStatus {
        status: "fresh".into(),
        reason: None,
        revision: Some(index.revision),
        producer: Some(index.contract.producer),
    })
}

/// Write graph-backed SCIP artifacts for one retrieval generation; returns the
/// revision string on success.
///
/// The artifact carries definition records for every emitted symbol and
/// reference records for every validated graph adjacency between two emitted
/// symbols, is stamped with `generation`, and is fully validated before it is
/// written — an artifact that cannot validate is never published.
pub fn emit_scip_artifacts_from_store(
    storage_path: &Path,
    project_dir: &Path,
    generation: &str,
) -> Result<Option<String>> {
    if generation.trim().is_empty() {
        bail!("scip emit requires a non-empty retrieval generation");
    }
    std::fs::create_dir_all(project_dir)
        .with_context(|| format!("create scip dir {}", project_dir.display()))?;
    let storage = Store::open(storage_path).context("open storage for scip emit")?;
    let expected_symbol_rows = u64::from(
        storage
            .get_canonical_search_symbol_count()
            .context("count canonical symbols for scip")?,
    );

    let mut symbols = Vec::new();
    let mut scanned_symbol_rows = 0_u64;
    let mut after = None;
    loop {
        let batch = storage
            .get_canonical_search_symbol_detail_batch_after(after, 4096)
            .context("load symbols for scip")?;
        if batch.is_empty() {
            break;
        }
        scanned_symbol_rows = scanned_symbol_rows
            .checked_add(u64::try_from(batch.len()).context("canonical SCIP page size overflow")?)
            .context("canonical SCIP symbol count overflow")?;
        after = batch.last().map(|row| row.node_id);
        for row in batch {
            if row.node_kind == Some(NodeKind::UNKNOWN as i64) {
                continue;
            }
            let Some(file_path) = row.file_path.as_deref().map(normalize_scip_path) else {
                continue;
            };
            let start_line = row.start_line.unwrap_or(1);
            let end_line = row.end_line.unwrap_or(start_line).max(start_line);
            symbols.push(ScipSymbolRecord {
                node_id: Some(row.node_id.0.to_string()),
                path: file_path,
                symbol: row.display_name,
                start_line,
                end_line,
            });
        }
    }
    if scanned_symbol_rows != expected_symbol_rows {
        bail!(
            "canonical SCIP symbol count changed while streaming: expected {expected_symbol_rows}, scanned {scanned_symbol_rows}"
        );
    }

    if symbols.is_empty() {
        return Ok(None);
    }

    let references = collect_reference_adjacency(&storage, &symbols)
        .context("collect scip reference adjacency")?;
    let revision = scip_revision_for_symbols(&symbols, &references);
    let mut proofs = symbols
        .iter()
        .map(ScipProofRecord::definition)
        .collect::<Vec<_>>();
    proofs.extend(references);
    let index = ScipSymbolsIndex {
        generation: generation.to_string(),
        revision: revision.clone(),
        contract: ScipProofAdapterContract::graph_projection(&revision),
        symbols,
        proofs,
    };
    index
        .validate_records(generation)
        .context("validate scip artifact before publication")?;
    let json = serde_json::to_string_pretty(&index).context("serialize scip symbols index")?;
    std::fs::write(project_dir.join(SCIP_SYMBOLS_FILE), json)
        .context("write symbols.index.json")?;
    std::fs::write(project_dir.join("revision.txt"), format!("{revision}\n"))
        .context("write scip revision")?;
    // Minimal marker so health treats graph lane as backed by a real artifact file.
    std::fs::write(
        project_dir.join(SCIP_INDEX_FILE),
        format!("codestory-scip-v1\nrevision={revision}\n"),
    )
    .context("write index.scip marker")?;
    let stub = project_dir.join(STUB_MARKER);
    if stub.is_file() {
        std::fs::remove_file(stub).context("remove scip stub marker")?;
    }
    Ok(Some(revision))
}

fn scip_revision_for_symbols(
    symbols: &[ScipSymbolRecord],
    references: &[ScipProofRecord],
) -> String {
    let mut hasher = Sha256::new();
    for symbol in symbols {
        if let Some(node_id) = &symbol.node_id {
            hasher.update(node_id.as_bytes());
        }
        hasher.update([0]);
        hasher.update(symbol.path.as_bytes());
        hasher.update(symbol.symbol.as_bytes());
        hasher.update(symbol.start_line.to_le_bytes());
        hasher.update(symbol.end_line.to_le_bytes());
    }
    for reference in references {
        hasher.update([1]);
        hasher.update(reference.node_id.as_deref().unwrap_or_default().as_bytes());
        hasher.update([0]);
        hasher.update(
            reference
                .target_node_id
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
        );
        hasher.update([0]);
        hasher.update(
            reference
                .target_symbol
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    format!("graph-{:x}", hasher.finalize())[..16].to_string()
}

/// Read validated reference adjacency between emitted symbols.
///
/// Edges are read one bounded seed batch at a time and the edge payload is
/// discarded before the next batch, so only the surviving endpoint pairs stay
/// resident. An edge only survives when both effective endpoints are emitted
/// symbols; unresolved call targets and low-confidence call resolutions are
/// dropped, and a same-file symbol pair with no edge produces nothing.
fn collect_reference_adjacency(
    storage: &Store,
    symbols: &[ScipSymbolRecord],
) -> Result<Vec<ScipProofRecord>> {
    let mut symbol_by_node = HashMap::with_capacity(symbols.len());
    let mut seeds = Vec::with_capacity(symbols.len());
    for symbol in symbols {
        let Some(node_id) = symbol
            .node_id
            .as_deref()
            .and_then(|node_id| node_id.parse::<i64>().ok())
        else {
            continue;
        };
        if symbol_by_node.insert(node_id, symbol).is_none() {
            seeds.push(NodeId(node_id));
        }
    }
    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    let mut pairs: BTreeSet<(i64, i64)> = BTreeSet::new();
    for chunk in seeds.chunks(SCIP_ADJACENCY_SEED_BATCH) {
        let edges_by_node = storage
            .get_edges_for_node_ids(chunk)
            .context("read graph edges for scip reference adjacency")?;
        for edge in edges_by_node.into_values().flatten() {
            if !SCIP_REFERENCE_EDGE_KINDS.contains(&edge.kind) {
                continue;
            }
            if edge.kind == EdgeKind::CALL && edge.resolved_target.is_none() {
                continue;
            }
            let (source, target) = edge.effective_endpoints();
            if source == target {
                continue;
            }
            if !symbol_by_node.contains_key(&source.0) || !symbol_by_node.contains_key(&target.0) {
                continue;
            }
            pairs.insert((source.0, target.0));
        }
    }

    let mut references = Vec::new();
    let mut current_source = None;
    let mut emitted_for_source = 0_usize;
    for (source, target) in pairs {
        if current_source != Some(source) {
            current_source = Some(source);
            emitted_for_source = 0;
        }
        if emitted_for_source >= SCIP_MAX_REFERENCES_PER_SYMBOL {
            continue;
        }
        let (Some(source_symbol), Some(target_symbol)) =
            (symbol_by_node.get(&source), symbol_by_node.get(&target))
        else {
            continue;
        };
        references.push(ScipProofRecord::reference(source_symbol, target_symbol));
        emitted_for_source += 1;
    }
    Ok(references)
}

fn normalize_scip_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn load_scip_symbols(project_dir: &Path) -> Result<Option<ScipSymbolsIndex>> {
    let path = project_dir.join(SCIP_SYMBOLS_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(&path).context("read scip symbols index")?;
    let parsed: ScipSymbolsIndex =
        serde_json::from_str(&body).context("parse scip symbols index json")?;
    Ok(Some(parsed))
}

pub(crate) fn load_fresh_scip_symbols(
    project_dir: &Path,
    expected_revision: &str,
    generation: &str,
) -> Result<Option<ScipSymbolsIndex>> {
    let Some(index) = load_scip_symbols(project_dir)? else {
        return Ok(None);
    };
    Ok(index
        .is_fresh_for(expected_revision, generation)
        .then_some(index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Edge, EdgeId, Node, NodeId, NodeKind, ResolutionCertainty};
    use codestory_store::{FileInfo, FileRole, SearchSymbolProjection};
    use tempfile::TempDir;

    #[test]
    fn scip_emit_streams_canonical_pages_independent_of_legacy_projection() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        let file_node_id = NodeId(1);
        storage
            .insert_file(&FileInfo {
                id: file_node_id.0,
                path: project.path().join("src").join("large.ts"),
                language: "typescript".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 4_200,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[Node {
                id: file_node_id,
                kind: NodeKind::FILE,
                serialized_name: "src/large.ts".to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: None,
                start_line: Some(1),
                start_col: Some(0),
                end_line: Some(4_200),
                end_col: Some(0),
            }])
            .expect("insert file node");

        let mut nodes = Vec::new();
        for index in 0..4_100_i64 {
            let id = NodeId(index + 2);
            nodes.push(Node {
                id,
                kind: NodeKind::FUNCTION,
                serialized_name: format!("symbol_{index:04}"),
                qualified_name: Some(format!("symbol_{index:04}")),
                canonical_id: None,
                file_node_id: Some(file_node_id),
                start_line: Some((index + 1) as u32),
                start_col: Some(0),
                end_line: Some((index + 1) as u32),
                end_col: Some(10),
            });
        }
        let unknown_node_id = NodeId(5_000);
        nodes.push(Node {
            id: unknown_node_id,
            kind: NodeKind::UNKNOWN,
            serialized_name: "import_alias".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_node_id: Some(file_node_id),
            start_line: Some(4_101),
            start_col: Some(0),
            end_line: Some(4_101),
            end_col: Some(10),
        });
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        drop(storage);

        let empty_projection_dir = project.path().join("scip-empty-projection");
        emit_scip_artifacts_from_store(&storage_path, &empty_projection_dir, "generation-a")
            .expect("emit scip without legacy projection");
        let symbols = load_scip_symbols(&empty_projection_dir)
            .expect("load scip")
            .expect("symbols");

        let mut storage = Store::open(&storage_path).expect("reopen store");
        storage
            .upsert_search_symbol_projection_batch(&[
                SearchSymbolProjection {
                    node_id: NodeId(2),
                    display_name: "stale_wrong_name".into(),
                },
                SearchSymbolProjection {
                    node_id: unknown_node_id,
                    display_name: "stale_unknown_name".into(),
                },
            ])
            .expect("seed stale legacy projection");
        drop(storage);

        let stale_projection_dir = project.path().join("scip-stale-projection");
        emit_scip_artifacts_from_store(&storage_path, &stale_projection_dir, "generation-a")
            .expect("emit scip with stale legacy projection");
        let stale_projection_symbols = load_scip_symbols(&stale_projection_dir)
            .expect("load stale-projection scip")
            .expect("stale-projection symbols");

        assert_eq!(
            symbols.contract.evidence_source,
            SCIP_GRAPH_PROJECTION_PROVENANCE
        );
        assert_eq!(symbols.contract.producer_config, "canonical_node_pages");
        assert_eq!(symbols.contract.freshness, "fresh");
        assert_eq!(symbols.proofs.len(), symbols.symbols.len());
        assert_eq!(
            serde_json::to_value(&symbols).expect("serialize empty-projection symbols"),
            serde_json::to_value(&stale_projection_symbols)
                .expect("serialize stale-projection symbols"),
            "legacy projection contents must not change canonical SCIP output"
        );
        assert!(
            symbols
                .symbols
                .iter()
                .all(|symbol| symbol.node_id.is_some()),
            "graph-projection SCIP symbols should preserve their exact node identity"
        );
        assert!(
            symbols
                .symbols
                .iter()
                .all(|symbol| symbol.node_id.as_deref() != Some("5000")),
            "unresolvable UNKNOWN nodes should not enter the SCIP candidate lane"
        );
        assert_eq!(symbols.symbols.len(), 4_100);
        assert!(
            symbols
                .symbols
                .iter()
                .any(|symbol| symbol.symbol == "symbol_4099"),
            "SCIP emit should include symbols after the old cap"
        );
    }

    /// One file with three symbols: `Client` calls `parse_client`, and
    /// `ClientConfig` shares the file and a name substring with `Client` but
    /// has no edge to anything.
    fn adjacency_fixture_store(project: &TempDir) -> std::path::PathBuf {
        let storage_path = project.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        let file_node_id = NodeId(1);
        storage
            .insert_file(&FileInfo {
                id: file_node_id.0,
                path: project.path().join("src").join("client.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 90,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![Node {
            id: file_node_id,
            kind: NodeKind::FILE,
            serialized_name: "src/client.rs".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_node_id: None,
            start_line: Some(1),
            start_col: Some(0),
            end_line: Some(90),
            end_col: Some(0),
        }];
        for (id, name, line) in [
            (2_i64, "Client", 10_u32),
            (3, "ClientConfig", 30),
            (4, "parse_client", 50),
        ] {
            nodes.push(Node {
                id: NodeId(id),
                kind: NodeKind::FUNCTION,
                serialized_name: name.to_string(),
                qualified_name: Some(name.to_string()),
                canonical_id: None,
                file_node_id: Some(file_node_id),
                start_line: Some(line),
                start_col: Some(0),
                end_line: Some(line + 5),
                end_col: Some(0),
            });
        }
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage
            .insert_edges_batch(&[Edge {
                id: EdgeId(1),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(file_node_id),
                line: Some(12),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(4)),
                confidence: Some(1.0),
                certainty: Some(ResolutionCertainty::Certain),
                callsite_identity: Some("src/client.rs:12".into()),
                candidate_targets: Vec::new(),
            }])
            .expect("insert edges");
        drop(storage);
        storage_path
    }

    #[test]
    fn scip_emit_binds_the_artifact_to_its_generation_and_carries_real_reference_adjacency() {
        let project = TempDir::new().expect("project");
        let storage_path = adjacency_fixture_store(&project);
        let scip_dir = project.path().join("scip");

        let revision = emit_scip_artifacts_from_store(&storage_path, &scip_dir, "generation-a")
            .expect("emit scip")
            .expect("revision");
        let index = load_scip_symbols(&scip_dir)
            .expect("load scip")
            .expect("index");

        assert_eq!(
            index.generation, "generation-a",
            "the artifact must name the retrieval generation it was built for"
        );
        assert_eq!(index.revision, revision);
        let references = index
            .proofs
            .iter()
            .filter(|proof| proof.is_reference())
            .collect::<Vec<_>>();
        assert_eq!(
            references.len(),
            1,
            "exactly the one real CALL edge becomes reference adjacency: {references:#?}"
        );
        let reference = references[0];
        assert_eq!(reference.node_id.as_deref(), Some("2"));
        assert_eq!(reference.target_node_id.as_deref(), Some("4"));
        assert_eq!(reference.symbol, "Client");
        assert_eq!(reference.target_symbol.as_deref(), Some("parse_client"));
        assert!(
            !index.proofs.iter().any(|proof| {
                proof.is_reference()
                    && (proof.target_symbol.as_deref() == Some("ClientConfig")
                        || proof.symbol == "ClientConfig")
            }),
            "a same-file substring pair with no edge must not become adjacency: {:#?}",
            index.proofs
        );
        index
            .validate_records("generation-a")
            .expect("published artifact validates against its own generation");
        assert!(
            index.is_fresh_for(&revision, "generation-a"),
            "the published artifact must be admissible for its own generation"
        );
        assert!(
            !index.is_fresh_for(&revision, "generation-b"),
            "the published artifact must be refused for a different generation"
        );
    }

    #[test]
    fn scip_emit_drops_unresolved_and_containment_edges_from_reference_adjacency() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("codestory.db");
        let mut storage = Store::open(&storage_path).expect("open store");
        let file_node_id = NodeId(1);
        storage
            .insert_file(&FileInfo {
                id: file_node_id.0,
                path: project.path().join("src").join("member.rs"),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 40,
                file_role: FileRole::Source,
            })
            .expect("insert file");
        let mut nodes = vec![Node {
            id: file_node_id,
            kind: NodeKind::FILE,
            serialized_name: "src/member.rs".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_node_id: None,
            start_line: Some(1),
            start_col: Some(0),
            end_line: Some(40),
            end_col: Some(0),
        }];
        for (id, name, line) in [(2_i64, "Owner", 5_u32), (3, "owned", 15), (4, "callee", 25)] {
            nodes.push(Node {
                id: NodeId(id),
                kind: NodeKind::FUNCTION,
                serialized_name: name.to_string(),
                qualified_name: Some(name.to_string()),
                canonical_id: None,
                file_node_id: Some(file_node_id),
                start_line: Some(line),
                start_col: Some(0),
                end_line: Some(line + 4),
                end_col: Some(0),
            });
        }
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage
            .insert_edges_batch(&[
                Edge {
                    id: EdgeId(1),
                    source: NodeId(2),
                    target: NodeId(3),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(file_node_id),
                    line: Some(6),
                    resolved_source: None,
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                },
                Edge {
                    id: EdgeId(2),
                    source: NodeId(2),
                    target: NodeId(4),
                    kind: EdgeKind::CALL,
                    file_node_id: Some(file_node_id),
                    line: Some(7),
                    resolved_source: Some(NodeId(2)),
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: Some("src/member.rs:7".into()),
                    candidate_targets: Vec::new(),
                },
            ])
            .expect("insert edges");
        drop(storage);
        let scip_dir = project.path().join("scip");

        emit_scip_artifacts_from_store(&storage_path, &scip_dir, "generation-a").expect("emit");
        let index = load_scip_symbols(&scip_dir)
            .expect("load scip")
            .expect("index");

        assert!(
            !index.proofs.iter().any(|proof| proof.is_reference()),
            "containment edges and unresolved call targets carry no reference adjacency: {:#?}",
            index.proofs
        );
    }

    #[test]
    fn scip_emit_refuses_an_empty_generation() {
        let project = TempDir::new().expect("project");
        let storage_path = adjacency_fixture_store(&project);
        let scip_dir = project.path().join("scip");

        let error = emit_scip_artifacts_from_store(&storage_path, &scip_dir, "  ")
            .expect_err("an unbound artifact must never be published");

        assert!(
            error
                .to_string()
                .contains("requires a non-empty retrieval generation"),
            "unexpected error: {error}"
        );
        assert!(!scip_dir.join(SCIP_SYMBOLS_FILE).exists());
    }

    #[test]
    fn configured_precise_semantic_import_copies_fresh_artifact() {
        let project = TempDir::new().expect("project");
        let artifact = project.path().join("import.json");
        let import_dir = project.path().join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR);
        let revision = "imported-a";
        let index = ScipSymbolsIndex {
            generation: String::new(),
            revision: revision.into(),
            contract: ScipProofAdapterContract {
                evidence_source: SCIP_IMPORTED_PROOF_PROVENANCE.into(),
                producer: "scip-fixture".into(),
                producer_version: "0.1.0".into(),
                producer_args: vec!["scip".into(), "index".into()],
                producer_config: "fixture-config-v1".into(),
                revision: revision.into(),
                package: ScipPackageIdentity {
                    manager: "cargo".into(),
                    name: "fixture_package".into(),
                    version: Some("1.2.3".into()),
                },
                position_encoding: SCIP_POSITION_ENCODING.into(),
                freshness: "fresh".into(),
            },
            symbols: vec![ScipSymbolRecord {
                node_id: None,
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 3,
                end_line: 3,
            }],
            proofs: vec![ScipProofRecord {
                role: SCIP_DEFINITION_ROLE.into(),
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 3,
                start_character_utf16: 0,
                end_line: 3,
                end_character_utf16: 4,
                target_symbol: None,
                node_id: None,
                target_node_id: None,
            }],
        };
        std::fs::write(&artifact, serde_json::to_string_pretty(&index).unwrap()).unwrap();

        let status = import_precise_semantic_scip_artifact(&artifact, &import_dir, "generation-a")
            .expect("import");

        assert_eq!(status.status, "fresh");
        assert_eq!(status.revision.as_deref(), Some(revision));
        assert_eq!(status.producer.as_deref(), Some("scip-fixture"));
        assert!(import_dir.join(SCIP_SYMBOLS_FILE).is_file());
        assert!(import_dir.join(SCIP_INDEX_FILE).is_file());
        assert_eq!(
            std::fs::read_to_string(import_dir.join("revision.txt")).unwrap(),
            "imported-a\n"
        );
    }

    #[test]
    fn missing_configured_precise_semantic_import_fails_closed() {
        let project = TempDir::new().expect("project");
        let missing = project.path().join("missing.json");
        let import_dir = project.path().join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR);

        let status = import_precise_semantic_scip_artifact(&missing, &import_dir, "generation-a")
            .expect("import status");

        assert_eq!(status.status, "missing");
        assert_eq!(
            status.reason.as_deref(),
            Some("configured_artifact_missing")
        );
        assert!(!import_dir.join(SCIP_SYMBOLS_FILE).exists());
    }

    #[test]
    fn precise_semantic_import_with_invalid_proof_position_fails_closed() {
        let project = TempDir::new().expect("project");
        let artifact = project.path().join("import.json");
        let import_dir = project.path().join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR);
        let revision = "imported-bad-position";
        let index = ScipSymbolsIndex {
            generation: String::new(),
            revision: revision.into(),
            contract: ScipProofAdapterContract {
                evidence_source: SCIP_IMPORTED_PROOF_PROVENANCE.into(),
                producer: "scip-fixture".into(),
                producer_version: "0.1.0".into(),
                producer_args: vec!["scip".into(), "index".into()],
                producer_config: "fixture-config-v1".into(),
                revision: revision.into(),
                package: ScipPackageIdentity {
                    manager: "cargo".into(),
                    name: "fixture_package".into(),
                    version: None,
                },
                position_encoding: SCIP_POSITION_ENCODING.into(),
                freshness: "fresh".into(),
            },
            symbols: vec![ScipSymbolRecord {
                node_id: None,
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 3,
                end_line: 3,
            }],
            proofs: vec![ScipProofRecord {
                role: SCIP_DEFINITION_ROLE.into(),
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 4,
                start_character_utf16: 0,
                end_line: 3,
                end_character_utf16: 4,
                target_symbol: None,
                node_id: None,
                target_node_id: None,
            }],
        };
        std::fs::write(&artifact, serde_json::to_string_pretty(&index).unwrap()).unwrap();

        let status = import_precise_semantic_scip_artifact(&artifact, &import_dir, "generation-a")
            .expect("import status");

        assert_eq!(status.status, "invalid");
        assert_eq!(
            status.reason.as_deref(),
            Some("imported_proof_contract_invalid")
        );
        assert!(!import_dir.join(SCIP_SYMBOLS_FILE).exists());
    }

    fn imported_index_with_reference(
        revision: &str,
        reference: ScipProofRecord,
    ) -> ScipSymbolsIndex {
        ScipSymbolsIndex {
            generation: String::new(),
            revision: revision.into(),
            contract: ScipProofAdapterContract {
                evidence_source: SCIP_IMPORTED_PROOF_PROVENANCE.into(),
                producer: "scip-fixture".into(),
                producer_version: "0.1.0".into(),
                producer_args: vec!["scip".into(), "index".into()],
                producer_config: "fixture-config-v1".into(),
                revision: revision.into(),
                package: ScipPackageIdentity {
                    manager: "cargo".into(),
                    name: "fixture_package".into(),
                    version: None,
                },
                position_encoding: SCIP_POSITION_ENCODING.into(),
                freshness: "fresh".into(),
            },
            symbols: vec![ScipSymbolRecord {
                node_id: None,
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 3,
                end_line: 3,
            }],
            proofs: vec![
                ScipProofRecord {
                    role: SCIP_DEFINITION_ROLE.into(),
                    path: "src/lib.rs".into(),
                    symbol: "fixture_package::run".into(),
                    start_line: 3,
                    start_character_utf16: 0,
                    end_line: 3,
                    end_character_utf16: 4,
                    target_symbol: None,
                    node_id: None,
                    target_node_id: None,
                },
                reference,
            ],
        }
    }

    #[test]
    fn precise_semantic_import_with_unresolvable_reference_target_fails_closed() {
        let project = TempDir::new().expect("project");
        let artifact = project.path().join("import.json");
        let import_dir = project.path().join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR);
        let revision = "imported-dangling-reference";
        let index = imported_index_with_reference(
            revision,
            ScipProofRecord {
                role: SCIP_REFERENCE_ROLE.into(),
                path: "src/main.rs".into(),
                symbol: "fixture_package::main".into(),
                start_line: 8,
                start_character_utf16: 0,
                end_line: 8,
                end_character_utf16: 4,
                target_symbol: Some("fixture_package::never_defined".into()),
                node_id: None,
                target_node_id: None,
            },
        );
        std::fs::write(&artifact, serde_json::to_string_pretty(&index).unwrap()).unwrap();

        let status = import_precise_semantic_scip_artifact(&artifact, &import_dir, "generation-a")
            .expect("import status");

        assert_eq!(status.status, "invalid");
        assert_eq!(
            status.reason.as_deref(),
            Some("imported_proof_contract_invalid")
        );
        assert!(!import_dir.join(SCIP_SYMBOLS_FILE).exists());
    }

    #[test]
    fn precise_semantic_import_with_forged_graph_node_identity_fails_closed() {
        let project = TempDir::new().expect("project");
        let artifact = project.path().join("import.json");
        let import_dir = project.path().join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR);
        let revision = "imported-forged-identity";
        let index = imported_index_with_reference(
            revision,
            ScipProofRecord {
                role: SCIP_REFERENCE_ROLE.into(),
                path: "src/main.rs".into(),
                symbol: "fixture_package::main".into(),
                start_line: 8,
                start_character_utf16: 0,
                end_line: 8,
                end_character_utf16: 4,
                target_symbol: Some("fixture_package::run".into()),
                node_id: Some("11".into()),
                target_node_id: Some("12".into()),
            },
        );
        std::fs::write(&artifact, serde_json::to_string_pretty(&index).unwrap()).unwrap();

        let status = import_precise_semantic_scip_artifact(&artifact, &import_dir, "generation-a")
            .expect("import status");

        assert_eq!(status.status, "invalid");
        assert_eq!(
            status.reason.as_deref(),
            Some("imported_proof_contract_invalid")
        );
        assert!(!import_dir.join(SCIP_SYMBOLS_FILE).exists());
    }

    #[test]
    fn precise_semantic_import_stamps_the_admitting_generation() {
        let project = TempDir::new().expect("project");
        let artifact = project.path().join("import.json");
        let import_dir = project.path().join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR);
        let revision = "imported-generation-bound";
        let mut index = imported_index_with_reference(
            revision,
            ScipProofRecord {
                role: SCIP_REFERENCE_ROLE.into(),
                path: "src/main.rs".into(),
                symbol: "fixture_package::main".into(),
                start_line: 8,
                start_character_utf16: 0,
                end_line: 8,
                end_character_utf16: 4,
                target_symbol: Some("fixture_package::run".into()),
                node_id: None,
                target_node_id: None,
            },
        );
        index.generation = "some-other-generation".into();
        std::fs::write(&artifact, serde_json::to_string_pretty(&index).unwrap()).unwrap();

        let status = import_precise_semantic_scip_artifact(&artifact, &import_dir, "generation-a")
            .expect("import status");

        assert_eq!(status.status, "fresh");
        let stored = load_scip_symbols(&import_dir)
            .expect("load stored import")
            .expect("stored import");
        assert_eq!(
            stored.generation, "generation-a",
            "an admitted import is bound to the generation that admitted it"
        );
        assert!(!stored.is_fresh_for(revision, "some-other-generation"));
    }
}
