//! Emit SCIP-shaped symbol artifacts from the CodeStory SQLite graph.

use anyhow::{Context, Result, bail};
use codestory_contracts::graph::{EdgeKind, NodeId, NodeKind};
use codestory_contracts::owned_artifacts::sqlite_file_with_sidecars;
use codestory_contracts::validation_receipts::{SealedReceiptCache, TransferableReceipt};
use codestory_store::Store;
use codestory_workspace::paths::sqlite_open_path;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const SCIP_SYMBOLS_FILE: &str = "symbols.index.json";
const SCIP_SYMBOLS_DATABASE_FILE: &str = "symbols.index.sqlite3";

pub(crate) fn scip_symbols_component_path(project_dir: &Path) -> PathBuf {
    let database = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
    if database.is_file() {
        database
    } else {
        project_dir.join(SCIP_SYMBOLS_FILE)
    }
}
pub const SCIP_INDEX_FILE: &str = "index.scip";
pub const SCIP_PRECISE_SEMANTIC_IMPORT_DIR: &str = "precise-semantic-import";
pub const SCIP_IMPORTED_PROOF_PROVENANCE: &str = "imported_scip_proof";
pub const SCIP_PRECISE_SEMANTIC_IMPORT_PUBLIC_PROVENANCE: &str = "precise_semantic_import";
pub const SCIP_GRAPH_PROJECTION_PROVENANCE: &str = "scip_graph_projection";
pub const SCIP_DEFINITION_ROLE: &str = "definition";
pub const SCIP_REFERENCE_ROLE: &str = "reference";
const SCIP_POSITION_ENCODING: &str = "line_one_based_utf16_column_zero_based";
/// Marker written beside stubbed SCIP artifacts. One spelling, so a probe and a
/// producer cannot disagree about what "stubbed" looks like on disk.
pub const SCIP_STUB_MARKER_FILE: &str = "index.scip.stub";
const SCIP_PARSED_INDEX_RECEIPT_CAPACITY: usize = 4;

static SCIP_PARSED_INDEX_RECEIPTS: SealedReceiptCache<PathBuf, Arc<ScipQueryData>> =
    SealedReceiptCache::new(SCIP_PARSED_INDEX_RECEIPT_CAPACITY);

/// Header of the graph-projection `index.scip` marker.
const SCIP_INDEX_MARKER_HEADER: &str = "codestory-scip-v1";
/// Header of the imported precise-semantic `index.scip` marker.
const SCIP_IMPORT_MARKER_HEADER: &str = "codestory-precise-semantic-import-v1";
/// Every header a readable `index.scip` marker may carry.
const SCIP_INDEX_MARKER_HEADERS: [&str; 2] = [SCIP_INDEX_MARKER_HEADER, SCIP_IMPORT_MARKER_HEADER];
/// The marker is a two-line contract. Anything materially larger is not the
/// artifact this product wrote, and is refused without reading it.
const SCIP_INDEX_MARKER_MAX_BYTES: u64 = 4_096;
const SCIP_INDEX_MARKER_REVISION_PREFIX: &str = "revision=";

/// Why a present `index.scip` is not the artifact its generation claims.
///
/// Existence used to be the whole check, so a truncated, empty, or
/// leftover-from-another-generation marker published as a healthy graph lane.
/// Each variant is a distinct, typed reason a generation is damaged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScipIndexMarkerError {
    /// No marker file at all.
    Missing,
    /// Present but its bytes could not be read as a marker.
    Unreadable { detail: String },
    /// Present but far larger than the marker contract allows.
    Oversized { bytes: u64 },
    /// Present but its first line is not a marker header this product writes.
    HeaderUnrecognized,
    /// Present, headed correctly, but carrying no revision line.
    RevisionMissing,
    /// Present and parsable, but describing a different generation.
    RevisionMismatch { expected: String, found: String },
}

impl ScipIndexMarkerError {
    /// Stable machine code for this defect.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Missing => "scip_index_marker_missing",
            Self::Unreadable { .. } => "scip_index_marker_unreadable",
            Self::Oversized { .. } => "scip_index_marker_oversized",
            Self::HeaderUnrecognized => "scip_index_marker_header_unrecognized",
            Self::RevisionMissing => "scip_index_marker_revision_missing",
            Self::RevisionMismatch { .. } => "scip_index_marker_revision_mismatch",
        }
    }
}

/// A parsed, revision-matched `index.scip` marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScipIndexMarker {
    pub(crate) header: &'static str,
    pub(crate) revision: String,
}

/// Write the graph-projection marker for `revision`.
pub(crate) fn write_scip_index_marker(project_dir: &Path, revision: &str) -> Result<()> {
    std::fs::write(
        project_dir.join(SCIP_INDEX_FILE),
        format!("{SCIP_INDEX_MARKER_HEADER}\n{SCIP_INDEX_MARKER_REVISION_PREFIX}{revision}\n"),
    )
    .context("write index.scip marker")
}

/// Write the imported precise-semantic marker for `revision`.
fn write_scip_import_marker(import_dir: &Path, revision: &str) -> Result<()> {
    std::fs::write(
        import_dir.join(SCIP_INDEX_FILE),
        format!("{SCIP_IMPORT_MARKER_HEADER}\n{SCIP_INDEX_MARKER_REVISION_PREFIX}{revision}\n"),
    )
    .context("write precise semantic import marker")
}

/// Parse `index.scip` and bind it to `expected_revision`.
///
/// This replaces the previous `.is_file()` test. A marker that exists but does
/// not parse, or parses to a different revision, is a damaged generation — the
/// caller must fall through to a rebuild rather than publish it as healthy.
pub(crate) fn parse_scip_index_marker(
    project_dir: &Path,
    expected_revision: &str,
) -> Result<ScipIndexMarker, ScipIndexMarkerError> {
    let path = project_dir.join(SCIP_INDEX_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ScipIndexMarkerError::Missing);
        }
        Err(error) => {
            return Err(ScipIndexMarkerError::Unreadable {
                detail: error.to_string(),
            });
        }
    };
    if !metadata.is_file() {
        return Err(ScipIndexMarkerError::Missing);
    }
    if metadata.len() > SCIP_INDEX_MARKER_MAX_BYTES {
        return Err(ScipIndexMarkerError::Oversized {
            bytes: metadata.len(),
        });
    }
    let body =
        std::fs::read_to_string(&path).map_err(|error| ScipIndexMarkerError::Unreadable {
            detail: error.to_string(),
        })?;
    let mut lines = body.lines().map(str::trim);
    let header = lines
        .next()
        .and_then(|line| {
            SCIP_INDEX_MARKER_HEADERS
                .into_iter()
                .find(|header| *header == line)
        })
        .ok_or(ScipIndexMarkerError::HeaderUnrecognized)?;
    let revision = lines
        .find_map(|line| line.strip_prefix(SCIP_INDEX_MARKER_REVISION_PREFIX))
        .map(str::trim)
        .filter(|revision| !revision.is_empty())
        .ok_or(ScipIndexMarkerError::RevisionMissing)?;
    if revision != expected_revision {
        return Err(ScipIndexMarkerError::RevisionMismatch {
            expected: expected_revision.to_string(),
            found: revision.to_string(),
        });
    }
    Ok(ScipIndexMarker {
        header,
        revision: revision.to_string(),
    })
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<EdgeKind>,
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
            edge_kind: None,
        }
    }

    fn reference(
        source: &ScipSymbolRecord,
        target: &ScipSymbolRecord,
        edge_kind: EdgeKind,
    ) -> Self {
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
            edge_kind: Some(edge_kind),
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
    ReferenceMissingEdgeKind,
    ReferenceUnsupportedEdgeKind,
    ReferenceForgedEdgeKind,
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
            Self::ReferenceMissingEdgeKind => "reference_missing_edge_kind",
            Self::ReferenceUnsupportedEdgeKind => "reference_unsupported_edge_kind",
            Self::ReferenceForgedEdgeKind => "reference_forged_edge_kind",
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Generation-bound query state derived once from one sealed SCIP artifact.
///
/// The JSON records remain the serialized source of truth. This view only
/// removes repeated hot-path parsing, normalization, symbol-map construction,
/// and whole-proof scans. It is cached under the same sealed file identity as
/// the parsed artifact and cannot outlive that receipt.
#[derive(Debug)]
pub(crate) struct ScipQueryView {
    generation: String,
    data: Arc<ScipQueryData>,
}

#[derive(Debug)]
struct ScipQueryData {
    index: Arc<ScipSymbolsIndex>,
    normalized_symbols: Vec<ScipNormalizedSymbol>,
    by_node_id: HashMap<String, usize>,
    adjacency_by_node: HashMap<String, Vec<ScipTypedAdjacency>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScipNormalizedSymbol {
    pub(crate) symbol_lower: String,
    pub(crate) path_lower: String,
    pub(crate) terminal_lower: String,
    pub(crate) file_stem_lower: Option<String>,
    pub(crate) path_segments_lower: Vec<String>,
    pub(crate) path_segments_compact: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScipTypedAdjacency {
    pub(crate) proof_ordinal: usize,
    pub(crate) neighbor_symbol_index: usize,
    pub(crate) direction: ScipAdjacencyDirection,
    pub(crate) edge_kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScipAdjacencyDirection {
    Outgoing,
    Incoming,
}

impl ScipQueryView {
    fn from_data(data: Arc<ScipQueryData>, generation: &str) -> Result<Self> {
        if generation.trim().is_empty() {
            bail!("scip artifact carries no generation");
        }
        Ok(Self {
            generation: generation.to_string(),
            data,
        })
    }

    pub(crate) fn generation(&self) -> &str {
        &self.generation
    }

    pub(crate) fn contract(&self) -> &ScipProofAdapterContract {
        &self.data.index.contract
    }

    pub(crate) fn symbol_count(&self) -> usize {
        self.data.index.symbols.len()
    }

    pub(crate) fn proof_count(&self) -> usize {
        self.data.index.proofs.len()
    }

    pub(crate) fn symbols(
        &self,
    ) -> impl Iterator<Item = (&ScipSymbolRecord, &ScipNormalizedSymbol)> {
        self.data
            .index
            .symbols
            .iter()
            .zip(&self.data.normalized_symbols)
    }

    pub(crate) fn symbol_at(&self, index: usize) -> Option<&ScipSymbolRecord> {
        self.data.index.symbols.get(index)
    }

    pub(crate) fn symbol_for_node(&self, node_id: &str) -> Option<&ScipSymbolRecord> {
        self.data
            .by_node_id
            .get(node_id)
            .and_then(|index| self.symbol_at(*index))
    }

    pub(crate) fn adjacency(&self, node_id: &str) -> &[ScipTypedAdjacency] {
        self.data
            .adjacency_by_node
            .get(node_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

impl ScipQueryData {
    fn build(index: Arc<ScipSymbolsIndex>) -> Result<Self> {
        index
            .validate_records(&index.generation)
            .map_err(anyhow::Error::new)?;

        let normalized_symbols = index
            .symbols
            .iter()
            .map(ScipNormalizedSymbol::new)
            .collect::<Vec<_>>();
        let mut by_node_id = HashMap::new();
        for (symbol_index, symbol) in index.symbols.iter().enumerate() {
            if let Some(node_id) = symbol.node_id.as_ref() {
                by_node_id.entry(node_id.clone()).or_insert(symbol_index);
            }
        }

        let mut adjacency_by_node = HashMap::<String, Vec<ScipTypedAdjacency>>::new();
        if index.contract.evidence_source() == Some(ScipEvidenceSource::GraphProjection) {
            for (proof_ordinal, proof) in index.proofs.iter().enumerate() {
                if !proof.is_reference() {
                    continue;
                }
                let (Some(node_id), Some(target_node_id), Some(edge_kind)) = (
                    proof.node_id.as_ref(),
                    proof.target_node_id.as_ref(),
                    proof.edge_kind,
                ) else {
                    continue;
                };
                let Some(&source_symbol_index) = by_node_id.get(node_id) else {
                    continue;
                };
                let Some(&target_symbol_index) = by_node_id.get(target_node_id) else {
                    continue;
                };
                adjacency_by_node
                    .entry(node_id.clone())
                    .or_default()
                    .push(ScipTypedAdjacency {
                        proof_ordinal,
                        neighbor_symbol_index: target_symbol_index,
                        direction: ScipAdjacencyDirection::Outgoing,
                        edge_kind,
                    });
                adjacency_by_node
                    .entry(target_node_id.clone())
                    .or_default()
                    .push(ScipTypedAdjacency {
                        proof_ordinal,
                        neighbor_symbol_index: source_symbol_index,
                        direction: ScipAdjacencyDirection::Incoming,
                        edge_kind,
                    });
            }
        }

        Ok(Self {
            index,
            normalized_symbols,
            by_node_id,
            adjacency_by_node,
        })
    }
}

impl ScipNormalizedSymbol {
    fn new(symbol: &ScipSymbolRecord) -> Self {
        let symbol_lower = symbol.symbol.to_ascii_lowercase();
        let path_lower = symbol.path.to_ascii_lowercase();
        let terminal_lower = symbol_lower
            .rsplit("::")
            .next()
            .unwrap_or(&symbol_lower)
            .rsplit('.')
            .next()
            .unwrap_or(&symbol_lower)
            .to_string();
        let file_name = symbol
            .path
            .rsplit(['/', '\\'])
            .next()
            .filter(|file_name| !file_name.is_empty());
        let file_stem_lower = file_name.map(|file_name| {
            file_name
                .rsplit_once('.')
                .map_or(file_name, |(stem, _)| stem)
                .to_ascii_lowercase()
        });
        let path_segments_lower = symbol
            .path
            .replace('\\', "/")
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let path_segments_compact = path_segments_lower
            .iter()
            .map(|segment| compact_alphanumeric(segment))
            .collect();
        Self {
            symbol_lower,
            path_lower,
            terminal_lower,
            file_stem_lower,
            path_segments_lower,
            path_segments_compact,
        }
    }
}

fn compact_alphanumeric(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
            if proof.edge_kind.is_some() {
                return Some(ScipProofDefect::ReferenceForgedEdgeKind);
            }
            (!lookup.has_symbol_named(target_symbol))
                .then_some(ScipProofDefect::ReferenceTargetSymbolUnknown)
        }
        ScipEvidenceSource::GraphProjection => {
            let Some(edge_kind) = proof.edge_kind else {
                return Some(ScipProofDefect::ReferenceMissingEdgeKind);
            };
            if !SCIP_REFERENCE_EDGE_KINDS.contains(&edge_kind) {
                return Some(ScipProofDefect::ReferenceUnsupportedEdgeKind);
            }
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
    write_scip_import_marker(import_dir, &index.revision)?;
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
#[cfg(any(test, feature = "test-support"))]
#[allow(dead_code)]
pub fn emit_scip_artifacts_from_store(
    storage_path: &Path,
    project_dir: &Path,
    generation: &str,
) -> Result<Option<String>> {
    emit_scip_artifacts_from_store_incremental(storage_path, project_dir, generation, None, || {
        Ok(())
    })
    .map(|outcome| outcome.revision)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScipIncrementalOutcome {
    pub revision: Option<String>,
    pub retained_records: u64,
    pub inserted_records: u64,
    pub removed_records: u64,
    pub reordered_records: u64,
    pub cloned: bool,
    pub direct_reference: bool,
}

pub(crate) fn emit_scip_artifacts_from_store_incremental(
    storage_path: &Path,
    project_dir: &Path,
    generation: &str,
    previous_project_dir: Option<&Path>,
    mut before_publish: impl FnMut() -> Result<()>,
) -> Result<ScipIncrementalOutcome> {
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
        return Ok(ScipIncrementalOutcome {
            revision: None,
            retained_records: 0,
            inserted_records: 0,
            removed_records: 0,
            reordered_records: 0,
            cloned: false,
            direct_reference: false,
        });
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
    let work = publish_scip_component(
        project_dir,
        previous_project_dir,
        &index,
        &mut before_publish,
    )?;
    before_publish()?;
    std::fs::write(project_dir.join("revision.txt"), format!("{revision}\n"))
        .context("write scip revision")?;
    // Minimal marker so health treats graph lane as backed by a real artifact
    // file. Health parses it and binds it to this revision, so it is evidence
    // rather than a presence flag.
    write_scip_index_marker(project_dir, &revision)?;
    let stub = project_dir.join(SCIP_STUB_MARKER_FILE);
    if stub.is_file() {
        std::fs::remove_file(stub).context("remove scip stub marker")?;
    }
    Ok(ScipIncrementalOutcome {
        revision: Some(revision),
        retained_records: work.retained,
        inserted_records: work.inserted,
        removed_records: work.removed,
        reordered_records: work.reordered,
        cloned: work.cloned,
        direct_reference: work.direct_reference,
    })
}

/// Publish a generation envelope over graph bytes already proven equivalent.
///
/// The heavy SQLite component is hard-linked from the validated predecessor;
/// the small revision and marker files are re-emitted for the new generation
/// directory. The parsed query-view receipt follows the hard link, so a warm
/// runtime neither streams the core graph nor rereads the unchanged component.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reference_equivalent_scip_generation(
    previous_project_dir: &Path,
    project_dir: &Path,
    previous_generation: &str,
    generation: &str,
    expected_revision: &str,
    mut before_publish: impl FnMut() -> Result<()>,
) -> Result<Option<ScipIncrementalOutcome>> {
    if previous_generation.trim().is_empty()
        || generation.trim().is_empty()
        || expected_revision.trim().is_empty()
    {
        return Ok(None);
    }
    let Some(previous_view) =
        load_fresh_scip_query_view(previous_project_dir, expected_revision, previous_generation)?
    else {
        return Ok(None);
    };
    let previous_path = previous_project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
    if !previous_path.is_file() {
        return Ok(None);
    }
    std::fs::create_dir_all(project_dir)
        .with_context(|| format!("create scip dir {}", project_dir.display()))?;
    let path = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
    if path.exists() {
        return Ok(None);
    }
    let previous_key = previous_path.clone();
    let previous_artifacts = sqlite_file_with_sidecars(&previous_path);
    let transferable =
        SCIP_PARSED_INDEX_RECEIPTS.transferable_receipt(&previous_key, &previous_artifacts);
    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&path, "scip-symbols")?;
    drop(reserved);
    std::fs::remove_file(&temp_path)?;
    if !crate::copy_on_write::reference_file(&previous_path, &temp_path)? {
        return Ok(None);
    }

    let result = (|| {
        before_publish()?;
        crate::copy_on_write::publish_immutable_file_atomic(&temp_path, &path)?;
        before_publish()?;
        std::fs::write(
            project_dir.join("revision.txt"),
            format!("{expected_revision}\n"),
        )
        .context("write referenced scip revision")?;
        write_scip_index_marker(project_dir, expected_revision)?;
        let stub = project_dir.join(SCIP_STUB_MARKER_FILE);
        if stub.is_file() {
            std::fs::remove_file(stub).context("remove scip stub marker")?;
        }
        if let Some(transferable) = transferable {
            let _ = SCIP_PARSED_INDEX_RECEIPTS.install_hard_link_alias(
                &previous_key,
                &previous_artifacts,
                path.clone(),
                &sqlite_file_with_sidecars(&path),
                transferable,
                Ok::<_, anyhow::Error>,
            )?;
        }
        let retained_records = previous_view
            .symbol_count()
            .saturating_add(previous_view.proof_count()) as u64;
        Ok(ScipIncrementalOutcome {
            revision: Some(expected_revision.to_string()),
            retained_records,
            inserted_records: 0,
            removed_records: 0,
            reordered_records: 0,
            cloned: false,
            direct_reference: true,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
        for owned in [
            path,
            project_dir.join("revision.txt"),
            project_dir.join(SCIP_INDEX_FILE),
        ] {
            let _ = std::fs::remove_file(owned);
        }
    }
    result.map(Some)
}

#[derive(Debug, Clone, Copy)]
struct ScipComponentWork {
    retained: u64,
    inserted: u64,
    removed: u64,
    reordered: u64,
    cloned: bool,
    direct_reference: bool,
}

#[derive(Debug, Clone)]
struct ScipComponentRow {
    key: String,
    kind: &'static str,
    record_sha256: String,
    record_json: String,
    ordinal: u64,
}

fn publish_scip_component(
    project_dir: &Path,
    previous_project_dir: Option<&Path>,
    index: &ScipSymbolsIndex,
    before_publish: &mut dyn FnMut() -> Result<()>,
) -> Result<ScipComponentWork> {
    let rows = scip_component_rows(index)?;
    let path = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&path, "scip-symbols")?;
    drop(reserved);
    let result: Result<ScipComponentWork> = (|| {
        std::fs::remove_file(&temp_path)?;
        let mut cloned = false;
        let mut direct_reference = false;
        if let Some(previous_dir) = previous_project_dir {
            let previous_path = previous_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
            if let Ok(previous) = load_scip_symbols_database(&previous_path) {
                crate::copy_on_write::make_file_immutable(&previous_path)?;
                if same_physical_scip_component(&previous, index)
                    && crate::copy_on_write::reference_file(&previous_path, &temp_path)?
                {
                    direct_reference = true;
                } else if crate::copy_on_write::clone_file(&previous_path, &temp_path)? {
                    crate::copy_on_write::make_file_owner_writable(&temp_path)?;
                    cloned = true;
                }
            }
        }
        let work = if direct_reference {
            ScipComponentWork {
                retained: rows.len() as u64,
                inserted: 0,
                removed: 0,
                reordered: 0,
                cloned: false,
                direct_reference: true,
            }
        } else if cloned {
            reconcile_scip_component(&temp_path, index, &rows)?
        } else {
            write_scip_component(&temp_path, index, &rows)?
        };
        let observed = load_scip_symbols_database_for_generation(&temp_path, &index.generation)?;
        if &observed != index {
            bail!("staged scip component differs from its pinned graph projection");
        }
        before_publish()?;
        crate::copy_on_write::publish_immutable_file_atomic(&temp_path, &path)?;
        let legacy = project_dir.join(SCIP_SYMBOLS_FILE);
        if legacy.is_file() {
            std::fs::remove_file(legacy)?;
        }
        Ok(ScipComponentWork {
            cloned,
            direct_reference,
            ..work
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn scip_component_rows(index: &ScipSymbolsIndex) -> Result<Vec<ScipComponentRow>> {
    let mut rows = Vec::with_capacity(index.symbols.len() + index.proofs.len());
    let mut keys = BTreeMap::<String, String>::new();
    for (ordinal, symbol) in index.symbols.iter().enumerate() {
        let node_id = symbol
            .node_id
            .as_deref()
            .context("graph-projection scip symbol is missing its node identity")?;
        let record_json = serde_json::to_string(symbol)?;
        let record_sha256 = sha256_text(&record_json);
        let key = format!("symbol:{node_id}");
        if keys.insert(key.clone(), record_sha256.clone()).is_some() {
            bail!("duplicate graph-projection scip symbol identity");
        }
        rows.push(ScipComponentRow {
            key,
            kind: "symbol",
            record_sha256,
            record_json,
            ordinal: ordinal as u64,
        });
    }
    let mut proof_occurrences = HashMap::<String, u32>::new();
    for (ordinal, proof) in index.proofs.iter().enumerate() {
        let record_json = serde_json::to_string(proof)?;
        let record_sha256 = sha256_text(&record_json);
        let occurrence = proof_occurrences.entry(record_sha256.clone()).or_default();
        let key = format!("proof:{record_sha256}:{occurrence}");
        *occurrence = occurrence
            .checked_add(1)
            .context("scip proof occurrence overflow")?;
        if let Some(previous_hash) = keys.insert(key.clone(), record_sha256.clone())
            && previous_hash != record_sha256
        {
            bail!("scip component key collision");
        }
        rows.push(ScipComponentRow {
            key,
            kind: "proof",
            record_sha256,
            record_json,
            ordinal: ordinal as u64,
        });
    }
    rows.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(rows)
}

fn same_physical_scip_component(left: &ScipSymbolsIndex, right: &ScipSymbolsIndex) -> bool {
    left.revision == right.revision
        && left.contract == right.contract
        && left.symbols == right.symbols
        && left.proofs == right.proofs
}

fn write_scip_component(
    path: &Path,
    index: &ScipSymbolsIndex,
    rows: &[ScipComponentRow],
) -> Result<ScipComponentWork> {
    let mut connection = Connection::open(sqlite_open_path(path))?;
    connection.execute_batch(
        "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=FULL;
         PRAGMA user_version=1;
         CREATE TABLE metadata (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             generation TEXT NOT NULL,
             revision TEXT NOT NULL,
             contract_json TEXT NOT NULL,
             symbol_count INTEGER NOT NULL,
             proof_count INTEGER NOT NULL,
             component_sha256 TEXT NOT NULL
         );
         CREATE TABLE records (
             record_key TEXT PRIMARY KEY NOT NULL,
             kind TEXT NOT NULL,
             record_sha256 TEXT NOT NULL,
             record_json TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE record_order (
             record_key TEXT PRIMARY KEY NOT NULL REFERENCES records(record_key),
             ordinal INTEGER NOT NULL
         ) WITHOUT ROWID;",
    )?;
    let transaction = connection.transaction()?;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO records(record_key, kind, record_sha256, record_json)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut insert_order =
            transaction.prepare("INSERT INTO record_order(record_key, ordinal) VALUES (?1, ?2)")?;
        for row in rows {
            insert.execute(params![
                row.key,
                row.kind,
                row.record_sha256,
                row.record_json
            ])?;
            insert_order.execute(params![
                row.key,
                i64::try_from(row.ordinal).context("scip record ordinal overflow")?
            ])?;
        }
    }
    write_scip_component_metadata(&transaction, index)?;
    transaction.commit()?;
    drop(connection);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(ScipComponentWork {
        retained: 0,
        inserted: rows.len() as u64,
        removed: 0,
        reordered: 0,
        cloned: false,
        direct_reference: false,
    })
}

fn reconcile_scip_component(
    path: &Path,
    index: &ScipSymbolsIndex,
    rows: &[ScipComponentRow],
) -> Result<ScipComponentWork> {
    let desired = rows
        .iter()
        .map(|row| (row.key.as_str(), row.record_sha256.as_str()))
        .collect::<HashMap<_, _>>();
    let mut connection = Connection::open(sqlite_open_path(path))?;
    let schema: i32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema != 1 {
        bail!("cloned scip component schema is not current");
    }
    connection.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=FULL;")?;
    let transaction = connection.transaction()?;
    let existing = {
        let mut statement = transaction.prepare(
            "SELECT r.record_key, r.record_sha256, o.ordinal
             FROM records r JOIN record_order o ON o.record_key = r.record_key
             ORDER BY r.record_key",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let existing_map = existing
        .iter()
        .map(|(key, hash, _)| (key.as_str(), hash.as_str()))
        .collect::<HashMap<_, _>>();
    let retained = existing
        .iter()
        .filter(|(key, hash, _)| desired.get(key.as_str()) == Some(&hash.as_str()))
        .count();
    let retained_order = existing
        .iter()
        .filter(|(key, hash, _)| desired.get(key.as_str()) == Some(&hash.as_str()))
        .map(|(key, _, ordinal)| (key.as_str(), *ordinal))
        .collect::<HashMap<_, _>>();
    for (key, hash, _) in &existing {
        if desired.get(key.as_str()) != Some(&hash.as_str()) {
            transaction.execute(
                "DELETE FROM record_order WHERE record_key = ?1",
                params![key],
            )?;
            transaction.execute("DELETE FROM records WHERE record_key = ?1", params![key])?;
        }
    }
    let missing = rows
        .iter()
        .filter(|row| existing_map.get(row.key.as_str()) != Some(&row.record_sha256.as_str()))
        .collect::<Vec<_>>();
    {
        let mut insert = transaction.prepare(
            "INSERT INTO records(record_key, kind, record_sha256, record_json)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for row in &missing {
            insert.execute(params![
                row.key,
                row.kind,
                row.record_sha256,
                row.record_json
            ])?;
        }
    }
    {
        let mut insert_order = transaction.prepare(
            "INSERT INTO record_order(record_key, ordinal) VALUES (?1, ?2)
             ON CONFLICT(record_key) DO UPDATE SET ordinal = excluded.ordinal",
        )?;
        for row in rows.iter().filter(|row| {
            retained_order.get(row.key.as_str())
                != Some(&i64::try_from(row.ordinal).unwrap_or(i64::MAX))
        }) {
            insert_order.execute(params![
                row.key,
                i64::try_from(row.ordinal).context("scip record ordinal overflow")?
            ])?;
        }
    }
    transaction.execute("DELETE FROM metadata", [])?;
    write_scip_component_metadata(&transaction, index)?;
    transaction.commit()?;
    drop(connection);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(ScipComponentWork {
        retained: retained as u64,
        inserted: missing.len() as u64,
        removed: existing.len().saturating_sub(retained) as u64,
        reordered: rows
            .iter()
            .filter(|row| {
                retained_order.get(row.key.as_str())
                    != Some(&i64::try_from(row.ordinal).unwrap_or(i64::MAX))
            })
            .count() as u64,
        cloned: true,
        direct_reference: false,
    })
}

fn write_scip_component_metadata(connection: &Connection, index: &ScipSymbolsIndex) -> Result<()> {
    let component_sha256 = scip_component_digest(connection)?;
    connection.execute(
        "INSERT INTO metadata VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            index.generation,
            index.revision,
            serde_json::to_string(&index.contract)?,
            i64::try_from(index.symbols.len()).context("scip symbol count overflow")?,
            i64::try_from(index.proofs.len()).context("scip proof count overflow")?,
            component_sha256,
        ],
    )?;
    Ok(())
}

fn scip_component_digest(connection: &Connection) -> Result<String> {
    let mut statement =
        connection.prepare("SELECT record_key, record_sha256 FROM records ORDER BY record_key")?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    digest.update(b"codestory-scip-component-v1\0");
    while let Some(row) = rows.next()? {
        let key = row.get::<_, String>(0)?;
        let hash = row.get::<_, String>(1)?;
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("scip component row digest is invalid");
        }
        digest.update((key.len() as u64).to_le_bytes());
        digest.update(key.as_bytes());
        digest.update(hash.as_bytes());
    }
    drop(rows);
    drop(statement);
    let mut order =
        connection.prepare("SELECT record_key, ordinal FROM record_order ORDER BY record_key")?;
    let mut rows = order.query([])?;
    while let Some(row) = rows.next()? {
        let key = row.get::<_, String>(0)?;
        let ordinal = row.get::<_, i64>(1)?;
        if ordinal < 0 {
            bail!("scip component record ordinal is invalid");
        }
        digest.update((key.len() as u64).to_le_bytes());
        digest.update(key.as_bytes());
        digest.update(ordinal.to_le_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
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
        hasher.update(
            reference
                .edge_kind
                .map(|kind| kind as i32)
                .unwrap_or(-1)
                .to_le_bytes(),
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

    let mut pairs: BTreeMap<(i64, i64), EdgeKind> = BTreeMap::new();
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
            pairs
                .entry((source.0, target.0))
                .and_modify(|existing| {
                    if scip_relation_priority(edge.kind) < scip_relation_priority(*existing) {
                        *existing = edge.kind;
                    }
                })
                .or_insert(edge.kind);
        }
    }

    let mut references = Vec::new();
    let mut current_source = None;
    let mut emitted_for_source = 0_usize;
    for ((source, target), edge_kind) in pairs {
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
        references.push(ScipProofRecord::reference(
            source_symbol,
            target_symbol,
            edge_kind,
        ));
        emitted_for_source += 1;
    }
    Ok(references)
}

fn scip_relation_priority(kind: EdgeKind) -> u8 {
    match kind {
        EdgeKind::CALL => 0,
        EdgeKind::OVERRIDE => 1,
        EdgeKind::INHERITANCE => 2,
        EdgeKind::MEMBER => 3,
        EdgeKind::TYPE_USAGE => 4,
        EdgeKind::ANNOTATION_USAGE => 5,
        EdgeKind::USAGE => 6,
        EdgeKind::TYPE_ARGUMENT => 7,
        EdgeKind::TEMPLATE_SPECIALIZATION => 8,
        EdgeKind::IMPORT => 9,
        EdgeKind::INCLUDE => 10,
        EdgeKind::MACRO_USAGE => 11,
        EdgeKind::UNKNOWN => 12,
    }
}

fn normalize_scip_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn load_scip_symbols(project_dir: &Path) -> Result<Option<ScipSymbolsIndex>> {
    let database_path = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
    if database_path.is_file() {
        return load_scip_symbols_database(&database_path).map(Some);
    }
    let path = project_dir.join(SCIP_SYMBOLS_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(&path).context("read scip symbols index")?;
    let parsed: ScipSymbolsIndex =
        serde_json::from_str(&body).context("parse scip symbols index json")?;
    Ok(Some(parsed))
}

fn load_scip_symbols_database(path: &Path) -> Result<ScipSymbolsIndex> {
    let index = read_scip_symbols_database(path)?;
    index
        .validate_records(&index.generation)
        .context("validate scip component records")?;
    Ok(index)
}

fn read_scip_symbols_database(path: &Path) -> Result<ScipSymbolsIndex> {
    let connection = Connection::open_with_flags(
        sqlite_open_path(path),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let schema: i32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema != 1 {
        bail!("scip component schema is not current");
    }
    let check: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if check != "ok" {
        bail!("scip component failed quick_check");
    }
    let (generation, revision, contract_json, symbol_count, proof_count, expected_digest) =
        connection.query_row(
            "SELECT generation, revision, contract_json, symbol_count, proof_count,
                        component_sha256
                 FROM metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )?;
    if scip_component_digest(&connection)? != expected_digest {
        bail!("scip component digest mismatch");
    }
    let mut symbols = Vec::new();
    let mut proofs = Vec::new();
    let mut statement = connection.prepare(
        "SELECT r.kind, r.record_sha256, r.record_json
         FROM records r
         JOIN record_order o ON o.record_key = r.record_key
         ORDER BY CASE r.kind WHEN 'symbol' THEN 0 ELSE 1 END, o.ordinal",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let kind = row.get::<_, String>(0)?;
        let expected_hash = row.get::<_, String>(1)?;
        let json = row.get::<_, String>(2)?;
        if sha256_text(&json) != expected_hash {
            bail!("scip component record digest mismatch");
        }
        match kind.as_str() {
            "symbol" => symbols.push(serde_json::from_str(&json)?),
            "proof" => proofs.push(serde_json::from_str(&json)?),
            _ => bail!("scip component contains an unknown record kind"),
        }
    }
    if i64::try_from(symbols.len()).unwrap_or(i64::MAX) != symbol_count
        || i64::try_from(proofs.len()).unwrap_or(i64::MAX) != proof_count
    {
        bail!("scip component record cardinality mismatch");
    }
    Ok(ScipSymbolsIndex {
        generation,
        revision,
        contract: serde_json::from_str(&contract_json)?,
        symbols,
        proofs,
    })
}

fn load_scip_symbols_database_for_generation(
    path: &Path,
    generation: &str,
) -> Result<ScipSymbolsIndex> {
    if generation.trim().is_empty() {
        bail!("scip artifact carries no generation");
    }
    let mut index = load_scip_symbols_database(path)?;
    index.generation = generation.to_string();
    index
        .validate_records(generation)
        .context("validate scip component against its publication envelope")?;
    Ok(index)
}

pub(crate) fn load_fresh_scip_query_view(
    project_dir: &Path,
    expected_revision: &str,
    generation: &str,
) -> Result<Option<Arc<ScipQueryView>>> {
    if generation.trim().is_empty() {
        return Ok(None);
    }
    let path = scip_symbols_component_path(project_dir);
    let revision_path = project_dir.join("revision.txt");
    let marker_path = project_dir.join(SCIP_INDEX_FILE);
    if !path.is_file() || !revision_path.is_file() || !marker_path.is_file() {
        return Ok(None);
    }
    let stored_revision = std::fs::read_to_string(&revision_path)
        .context("read scip revision")?
        .trim()
        .to_string();
    if stored_revision != expected_revision
        || parse_scip_index_marker(project_dir, expected_revision).is_err()
    {
        return Ok(None);
    }
    let data = SCIP_PARSED_INDEX_RECEIPTS.validate_sealed(
        path.clone(),
        &sqlite_file_with_sidecars(&path),
        || {
            let index = if path.file_name().and_then(|name| name.to_str())
                == Some(SCIP_SYMBOLS_DATABASE_FILE)
            {
                read_scip_symbols_database(&path)?
            } else {
                let Some(index) = load_scip_symbols(project_dir)? else {
                    bail!("scip component disappeared during validation");
                };
                index
            };
            Ok::<_, anyhow::Error>(Arc::new(ScipQueryData::build(Arc::new(index))?))
        },
    )?;
    if data.index.revision != expected_revision
        || !data.index.contract.is_fresh_for(expected_revision)
        || !data.index.has_required_proof_records()
        || data.index.symbols.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(Arc::new(ScipQueryView::from_data(data, generation)?)))
}

pub(crate) struct ScipGenerationReceiptRefresh {
    key: PathBuf,
    artifacts: Vec<PathBuf>,
    receipt: TransferableReceipt<Arc<ScipQueryData>>,
}

pub(crate) fn capture_scip_generation_receipt(
    project_dir: &Path,
) -> Option<ScipGenerationReceiptRefresh> {
    let key = scip_symbols_component_path(project_dir);
    let artifacts = sqlite_file_with_sidecars(&key);
    SCIP_PARSED_INDEX_RECEIPTS
        .transferable_receipt(&key, &artifacts)
        .map(|receipt| ScipGenerationReceiptRefresh {
            key,
            artifacts,
            receipt,
        })
}

impl ScipGenerationReceiptRefresh {
    pub(crate) fn refresh_after_owned_link_cleanup(self) -> bool {
        SCIP_PARSED_INDEX_RECEIPTS.refresh_after_hard_links(self.key, &self.artifacts, self.receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Edge, EdgeId, Node, NodeId, NodeKind, ResolutionCertainty};
    use codestory_store::{FileInfo, FileRole, SearchSymbolProjection};
    use tempfile::TempDir;

    fn component_symbol(node_id: &str, path: &str, symbol: &str) -> ScipSymbolRecord {
        ScipSymbolRecord {
            node_id: Some(node_id.into()),
            path: path.into(),
            symbol: symbol.into(),
            start_line: 1,
            end_line: 1,
        }
    }

    fn component_index(generation: &str, mut symbols: Vec<ScipSymbolRecord>) -> ScipSymbolsIndex {
        symbols.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        let mut proofs = symbols
            .iter()
            .map(ScipProofRecord::definition)
            .collect::<Vec<_>>();
        proofs
            .sort_by_key(|record| sha256_text(&serde_json::to_string(record).expect("proof json")));
        let revision = scip_revision_for_symbols(&symbols, &[]);
        ScipSymbolsIndex {
            generation: generation.into(),
            revision: revision.clone(),
            contract: ScipProofAdapterContract::graph_projection(&revision),
            symbols,
            proofs,
        }
    }

    #[test]
    fn incremental_scip_component_matches_clean_add_change_delete_and_rename() {
        let root = TempDir::new().expect("tempdir");
        let previous_dir = root.path().join("previous");
        let current_dir = root.path().join("current");
        std::fs::create_dir_all(&previous_dir).expect("previous dir");
        std::fs::create_dir_all(&current_dir).expect("current dir");
        let previous = component_index(
            "generation-v1",
            vec![
                component_symbol("1", "src/a.rs", "alpha"),
                component_symbol("2", "src/b.rs", "beta"),
                component_symbol("4", "src/kept.rs", "kept"),
            ],
        );
        let previous_work = publish_scip_component(&previous_dir, None, &previous, &mut || Ok(()))
            .expect("previous component");
        assert!(!previous_work.cloned);

        let current = component_index(
            "generation-v2",
            vec![
                component_symbol("1", "src/a.rs", "alpha_changed"),
                component_symbol("3", "src/renamed.rs", "beta"),
                component_symbol("4", "src/kept.rs", "kept"),
            ],
        );
        let work =
            publish_scip_component(&current_dir, Some(&previous_dir), &current, &mut || Ok(()))
                .expect("incremental component");
        if !work.cloned {
            return;
        }
        assert_eq!(work.retained, 2);
        assert_eq!(work.inserted, 4);
        assert_eq!(work.removed, 4);
        assert_eq!(work.reordered, 4);
        assert_eq!(
            load_scip_symbols_database(&current_dir.join(SCIP_SYMBOLS_DATABASE_FILE))
                .expect("current component"),
            current
        );
    }

    #[test]
    fn identical_scip_records_do_not_rewrite_ordering_rows() {
        let root = TempDir::new().expect("tempdir");
        let previous_dir = root.path().join("previous");
        let current_dir = root.path().join("current");
        std::fs::create_dir_all(&previous_dir).expect("previous dir");
        std::fs::create_dir_all(&current_dir).expect("current dir");
        let previous = component_index(
            "generation-v1",
            vec![
                component_symbol("1", "src/a.rs", "alpha"),
                component_symbol("2", "src/b.rs", "beta"),
            ],
        );
        publish_scip_component(&previous_dir, None, &previous, &mut || Ok(()))
            .expect("previous component");
        let mut current = previous.clone();
        current.generation = "generation-v2".into();

        let work =
            publish_scip_component(&current_dir, Some(&previous_dir), &current, &mut || Ok(()))
                .expect("incremental component");
        if !work.cloned {
            return;
        }
        assert_eq!(work.retained, 4);
        assert_eq!(work.inserted, 0);
        assert_eq!(work.removed, 0);
        assert_eq!(work.reordered, 0);
    }

    #[test]
    fn publication_only_scip_churn_directly_references_the_component_without_clone() {
        let root = TempDir::new().expect("tempdir");
        let previous_dir = root.path().join("previous");
        let current_dir = root.path().join("current");
        std::fs::create_dir_all(&previous_dir).expect("previous dir");
        std::fs::create_dir_all(&current_dir).expect("current dir");
        let previous = component_index(
            "generation-v1",
            vec![
                component_symbol("1", "src/a.rs", "alpha"),
                component_symbol("2", "src/b.rs", "beta"),
            ],
        );
        publish_scip_component(&previous_dir, None, &previous, &mut || Ok(()))
            .expect("previous component");
        let mut current = previous.clone();
        current.generation = "generation-v2".into();
        let previous_path = previous_dir.join(SCIP_SYMBOLS_DATABASE_FILE);

        let work = crate::copy_on_write::with_clone_disabled(|| {
            publish_scip_component(&current_dir, Some(&previous_dir), &current, &mut || Ok(()))
        })
        .expect("publication-only scip component");
        assert!(work.direct_reference);
        assert_eq!(work.retained, 4);
        assert_eq!(work.inserted, 0);
        assert_eq!(work.removed, 0);
        assert_eq!(work.reordered, 0);
        let current_path = current_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        assert_eq!(
            codestory_workspace::workspace_path_identity(&previous_path)
                .expect("previous identity"),
            codestory_workspace::workspace_path_identity(&current_path).expect("current identity"),
        );
        assert_eq!(
            load_scip_symbols_database_for_generation(&current_path, "generation-v2")
                .expect("load current envelope"),
            current,
        );
        assert!(
            std::fs::metadata(&previous_path)
                .expect("previous permissions")
                .permissions()
                .readonly()
        );
        assert!(
            std::fs::metadata(&current_path)
                .expect("current permissions")
                .permissions()
                .readonly()
        );

        let changed_dir = root.path().join("changed");
        std::fs::create_dir_all(&changed_dir).expect("changed dir");
        let changed = component_index(
            "generation-v3",
            vec![
                component_symbol("1", "src/a.rs", "changed-alpha"),
                component_symbol("2", "src/b.rs", "beta"),
            ],
        );
        let changed_work =
            publish_scip_component(&changed_dir, Some(&current_dir), &changed, &mut || Ok(()))
                .expect("changed component from immutable predecessor");
        assert!(!changed_work.direct_reference);
        let changed_path = changed_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        assert_eq!(
            load_scip_symbols_database_for_generation(&changed_path, "generation-v3")
                .expect("load changed component"),
            changed,
        );
        assert!(
            std::fs::metadata(changed_path)
                .expect("changed permissions")
                .permissions()
                .readonly()
        );
    }

    #[test]
    fn graph_equivalent_generation_reuses_the_parsed_component_receipt() {
        let root = TempDir::new().expect("tempdir");
        let previous_dir = root.path().join("previous");
        let current_dir = root.path().join("current");
        std::fs::create_dir_all(&previous_dir).expect("previous dir");
        let previous = component_index(
            "generation-v1",
            vec![
                component_symbol("1", "src/a.rs", "alpha"),
                component_symbol("2", "src/b.rs", "beta"),
            ],
        );
        publish_scip_component(&previous_dir, None, &previous, &mut || Ok(()))
            .expect("previous component");
        std::fs::write(
            previous_dir.join("revision.txt"),
            format!("{}\n", previous.revision),
        )
        .expect("previous revision");
        write_scip_index_marker(&previous_dir, &previous.revision).expect("previous marker");
        let previous_view =
            load_fresh_scip_query_view(&previous_dir, &previous.revision, "generation-v1")
                .expect("validate predecessor")
                .expect("predecessor query view");

        let outcome = reference_equivalent_scip_generation(
            &previous_dir,
            &current_dir,
            "generation-v1",
            "generation-v2",
            &previous.revision,
            || Ok(()),
        )
        .expect("reference equivalent graph")
        .expect("hard-link reference supported");
        assert!(outcome.direct_reference);
        assert_eq!(outcome.inserted_records, 0);
        assert_eq!(outcome.removed_records, 0);
        let previous_path = previous_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        let current_path = current_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        assert_eq!(
            codestory_workspace::workspace_path_identity(&previous_path)
                .expect("previous identity"),
            codestory_workspace::workspace_path_identity(&current_path).expect("current identity"),
        );
        let current_key = current_path;
        let aliased = SCIP_PARSED_INDEX_RECEIPTS
            .stats(&current_key)
            .expect("referenced graph inherits parsed receipt");
        assert_eq!(aliased.validations, 1);
        let current_view =
            load_fresh_scip_query_view(&current_dir, &previous.revision, "generation-v2")
                .expect("reuse current graph")
                .expect("current query view");
        assert_eq!(previous_view.generation(), "generation-v1");
        assert_eq!(current_view.generation(), "generation-v2");
        assert_eq!(previous_view.symbol_count(), current_view.symbol_count());
        let reused = SCIP_PARSED_INDEX_RECEIPTS
            .stats(&current_key)
            .expect("current graph receipt remains sealed");
        assert_eq!(reused.validations, 1);
        assert!(reused.reuses > aliased.reuses);

        let refresh = capture_scip_generation_receipt(&current_dir)
            .expect("capture current graph receipt before owned cleanup");
        std::fs::remove_file(&previous_path).expect("retire predecessor graph hard link");
        assert!(refresh.refresh_after_owned_link_cleanup());
        load_fresh_scip_query_view(&current_dir, &previous.revision, "generation-v2")
            .expect("graph after predecessor cleanup")
            .expect("graph remains available");
        assert_eq!(
            SCIP_PARSED_INDEX_RECEIPTS
                .stats(&current_key)
                .expect("cleanup refreshed current graph receipt")
                .validations,
            1,
            "owned hard-link cleanup must not force another full graph scan",
        );
    }

    #[test]
    fn scip_publication_envelope_is_rechecked_without_rereading_sealed_component() {
        let root = TempDir::new().expect("tempdir");
        let project_dir = root.path().join("generation");
        std::fs::create_dir_all(&project_dir).expect("generation dir");
        let index = component_index(
            "generation-v1",
            vec![component_symbol("1", "src/a.rs", "alpha")],
        );
        publish_scip_component(&project_dir, None, &index, &mut || Ok(())).expect("component");
        let revision_path = project_dir.join("revision.txt");
        std::fs::write(&revision_path, format!("{}\n", index.revision)).expect("revision");
        write_scip_index_marker(&project_dir, &index.revision).expect("marker");
        let component_path = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);

        load_fresh_scip_query_view(&project_dir, &index.revision, "generation-v1")
            .expect("initial validation")
            .expect("initial view");
        let initial = SCIP_PARSED_INDEX_RECEIPTS
            .stats(&component_path)
            .expect("sealed physical component");
        assert_eq!(initial.validations, 1);

        std::fs::write(&revision_path, "wrong-revision\n").expect("damage revision envelope");
        assert!(
            load_fresh_scip_query_view(&project_dir, &index.revision, "generation-v1")
                .expect("revision mismatch is a refusal")
                .is_none()
        );
        std::fs::write(&revision_path, format!("{}\n", index.revision))
            .expect("restore revision envelope");

        std::fs::write(project_dir.join(SCIP_INDEX_FILE), "damaged-marker\n")
            .expect("damage marker envelope");
        assert!(
            load_fresh_scip_query_view(&project_dir, &index.revision, "generation-v1")
                .expect("marker mismatch is a refusal")
                .is_none()
        );
        write_scip_index_marker(&project_dir, &index.revision).expect("restore marker envelope");

        let restored = load_fresh_scip_query_view(&project_dir, &index.revision, "generation-v1")
            .expect("restored envelope")
            .expect("restored view");
        assert_eq!(restored.generation(), "generation-v1");
        let final_stats = SCIP_PARSED_INDEX_RECEIPTS
            .stats(&component_path)
            .expect("component receipt survives envelope refusals");
        assert_eq!(final_stats.validations, 1);
        assert!(final_stats.reuses > initial.reuses);
    }

    #[test]
    fn scip_component_mutation_truncation_and_replacement_invalidate_warm_receipts() {
        use std::io::{Seek, SeekFrom, Write};

        for damage in ["mutation", "truncation"] {
            let root = TempDir::new().expect("tempdir");
            let project_dir = root.path().join(damage);
            std::fs::create_dir_all(&project_dir).expect("generation dir");
            let index = component_index(
                "generation-v1",
                vec![component_symbol("1", "src/a.rs", "alpha")],
            );
            publish_scip_component(&project_dir, None, &index, &mut || Ok(())).expect("component");
            std::fs::write(
                project_dir.join("revision.txt"),
                format!("{}\n", index.revision),
            )
            .expect("revision");
            write_scip_index_marker(&project_dir, &index.revision).expect("marker");
            let component_path = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
            load_fresh_scip_query_view(&project_dir, &index.revision, "generation-v1")
                .expect("warm component")
                .expect("warm view");
            assert_eq!(
                SCIP_PARSED_INDEX_RECEIPTS
                    .stats(&component_path)
                    .expect("warm receipt")
                    .validations,
                1
            );

            crate::copy_on_write::make_file_owner_writable(&component_path)
                .expect("make component writable for hostile mutation");
            if damage == "mutation" {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&component_path)
                    .expect("open component");
                file.seek(SeekFrom::Start(0)).expect("seek component");
                file.write_all(b"X").expect("mutate component");
                file.sync_all().expect("sync mutation");
            } else {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&component_path)
                    .expect("open component")
                    .set_len(64)
                    .expect("truncate component");
            }
            assert!(
                load_fresh_scip_query_view(&project_dir, &index.revision, "generation-v1").is_err(),
                "{damage} must force physical revalidation and refusal"
            );
            assert_eq!(
                SCIP_PARSED_INDEX_RECEIPTS.stats(&component_path),
                None,
                "a failed physical validation must not leave a receipt"
            );
        }

        let root = TempDir::new().expect("tempdir");
        let project_dir = root.path().join("published");
        let replacement_dir = root.path().join("replacement");
        std::fs::create_dir_all(&project_dir).expect("published dir");
        std::fs::create_dir_all(&replacement_dir).expect("replacement dir");
        let original = component_index(
            "generation-v1",
            vec![component_symbol("1", "src/a.rs", "alpha")],
        );
        publish_scip_component(&project_dir, None, &original, &mut || Ok(()))
            .expect("original component");
        std::fs::write(
            project_dir.join("revision.txt"),
            format!("{}\n", original.revision),
        )
        .expect("original revision");
        write_scip_index_marker(&project_dir, &original.revision).expect("original marker");
        let component_path = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        load_fresh_scip_query_view(&project_dir, &original.revision, "generation-v1")
            .expect("warm original")
            .expect("original view");

        let replacement = component_index(
            "generation-replacement",
            vec![component_symbol("2", "src/b.rs", "beta")],
        );
        publish_scip_component(&replacement_dir, None, &replacement, &mut || Ok(()))
            .expect("replacement component");
        let replacement_path = replacement_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        std::fs::remove_file(&component_path).expect("remove original component");
        std::fs::rename(&replacement_path, &component_path).expect("replace component identity");

        assert!(
            load_fresh_scip_query_view(&project_dir, &original.revision, "generation-v1")
                .expect("valid replacement is an envelope refusal")
                .is_none()
        );
        let replaced = SCIP_PARSED_INDEX_RECEIPTS
            .stats(&component_path)
            .expect("replacement physical facts are sealed");
        assert_eq!(replaced.validations, 2);
        assert_eq!(replaced.invalidations, 1);

        std::fs::write(
            project_dir.join("revision.txt"),
            format!("{}\n", replacement.revision),
        )
        .expect("replacement revision");
        write_scip_index_marker(&project_dir, &replacement.revision).expect("replacement marker");
        let admitted =
            load_fresh_scip_query_view(&project_dir, &replacement.revision, "generation-v1")
                .expect("replacement envelope")
                .expect("replacement view");
        assert_eq!(admitted.generation(), "generation-v1");
        assert_eq!(admitted.symbol_count(), 1);
        let admitted_stats = SCIP_PARSED_INDEX_RECEIPTS
            .stats(&component_path)
            .expect("replacement receipt reused");
        assert_eq!(admitted_stats.validations, 2);
        assert!(admitted_stats.reuses > replaced.reuses);
    }

    #[test]
    fn same_generation_scip_retry_replaces_a_readonly_component_before_marker_completion() {
        let root = TempDir::new().expect("tempdir");
        let project_dir = root.path().join("partial");
        std::fs::create_dir_all(&project_dir).expect("partial dir");
        let partial = component_index(
            "generation-v1",
            vec![component_symbol("1", "src/a.rs", "partial")],
        );
        publish_scip_component(&project_dir, None, &partial, &mut || Ok(()))
            .expect("publish component before markers");
        let path = project_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        assert!(
            std::fs::metadata(&path)
                .expect("partial permissions")
                .permissions()
                .readonly()
        );

        let repaired = component_index(
            "generation-v1",
            vec![component_symbol("1", "src/a.rs", "repaired")],
        );
        publish_scip_component(&project_dir, None, &repaired, &mut || Ok(()))
            .expect("repair same-generation component");

        assert_eq!(
            load_scip_symbols_database(&path).expect("load repaired component"),
            repaired
        );
        assert!(
            std::fs::metadata(path)
                .expect("repaired permissions")
                .permissions()
                .readonly()
        );
    }

    #[test]
    fn cancelled_scip_component_leaves_no_candidate_database() {
        let root = TempDir::new().expect("tempdir");
        let previous_dir = root.path().join("previous");
        let cancelled_dir = root.path().join("cancelled");
        std::fs::create_dir_all(&previous_dir).expect("previous dir");
        std::fs::create_dir_all(&cancelled_dir).expect("cancelled dir");
        let previous = component_index(
            "generation-v1",
            vec![component_symbol("1", "src/a.rs", "alpha")],
        );
        publish_scip_component(&previous_dir, None, &previous, &mut || Ok(()))
            .expect("previous component");
        let current = component_index(
            "generation-v2",
            vec![component_symbol("1", "src/a.rs", "changed")],
        );

        let error =
            publish_scip_component(&cancelled_dir, Some(&previous_dir), &current, &mut || {
                bail!("simulated SCIP cancellation")
            })
            .expect_err("cancelled SCIP component must fail");

        assert!(format!("{error:#}").contains("simulated SCIP cancellation"));
        assert!(!cancelled_dir.join(SCIP_SYMBOLS_DATABASE_FILE).exists());
        assert_eq!(
            std::fs::read_dir(&cancelled_dir)
                .expect("cancelled SCIP directory")
                .count(),
            0,
            "failed SCIP staging must not leak a generation-local clone"
        );
    }

    #[test]
    fn corrupt_scip_predecessor_falls_back_to_a_clean_component() {
        let root = TempDir::new().expect("tempdir");
        let previous_dir = root.path().join("previous");
        let current_dir = root.path().join("current");
        std::fs::create_dir_all(&previous_dir).expect("previous dir");
        std::fs::create_dir_all(&current_dir).expect("current dir");
        let previous = component_index(
            "generation-v1",
            vec![component_symbol("1", "src/a.rs", "old")],
        );
        publish_scip_component(&previous_dir, None, &previous, &mut || Ok(()))
            .expect("previous component");
        let previous_path = previous_dir.join(SCIP_SYMBOLS_DATABASE_FILE);
        crate::copy_on_write::make_file_owner_writable(&previous_path)
            .expect("authorize hostile corruption");
        std::fs::write(previous_path, b"not sqlite").expect("corrupt predecessor");
        let current = component_index(
            "generation-v2",
            vec![component_symbol("1", "src/a.rs", "current")],
        );

        let work =
            publish_scip_component(&current_dir, Some(&previous_dir), &current, &mut || Ok(()))
                .expect("complete fallback");

        assert!(!work.cloned);
        assert_eq!(
            load_scip_symbols_database(&current_dir.join(SCIP_SYMBOLS_DATABASE_FILE))
                .expect("load fallback component"),
            current
        );
    }

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
                edge_kind: None,
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
                edge_kind: None,
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
                    edge_kind: None,
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
                edge_kind: None,
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
                edge_kind: Some(EdgeKind::CALL),
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
                edge_kind: None,
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
