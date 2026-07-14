//! Emit SCIP-shaped symbol artifacts from the CodeStory SQLite graph.

use anyhow::{Context, Result};
use codestory_contracts::graph::NodeKind;
use codestory_store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const SCIP_SYMBOLS_FILE: &str = "symbols.index.json";
pub const SCIP_INDEX_FILE: &str = "index.scip";
pub const SCIP_PRECISE_SEMANTIC_IMPORT_DIR: &str = "precise-semantic-import";
pub const SCIP_IMPORTED_PROOF_PROVENANCE: &str = "imported_scip_proof";
pub const SCIP_PRECISE_SEMANTIC_IMPORT_PUBLIC_PROVENANCE: &str = "precise_semantic_import";
pub const SCIP_GRAPH_PROJECTION_PROVENANCE: &str = "scip_graph_projection";
const SCIP_POSITION_ENCODING: &str = "line_one_based_utf16_column_zero_based";
const STUB_MARKER: &str = "index.scip.stub";

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
            producer_config: "search_symbol_projection".into(),
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
}

impl ScipProofRecord {
    fn definition(symbol: &ScipSymbolRecord) -> Self {
        Self {
            role: "definition".into(),
            path: symbol.path.clone(),
            symbol: symbol.symbol.clone(),
            start_line: symbol.start_line,
            start_character_utf16: 0,
            end_line: symbol.end_line,
            end_character_utf16: 0,
            target_symbol: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipSymbolsIndex {
    pub revision: String,
    #[serde(default)]
    pub contract: ScipProofAdapterContract,
    pub symbols: Vec<ScipSymbolRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<ScipProofRecord>,
}

impl ScipSymbolsIndex {
    pub(crate) fn is_fresh_for(&self, revision: &str) -> bool {
        self.revision == revision
            && self.contract.is_fresh_for(revision)
            && self.has_required_proof_records()
    }

    fn has_required_proof_records(&self) -> bool {
        match self.contract.evidence_source() {
            Some(ScipEvidenceSource::GraphProjection) => true,
            Some(ScipEvidenceSource::ImportedProof) => {
                self.proofs.iter().any(|proof| self.proof_is_valid(proof))
            }
            None => false,
        }
    }

    fn proof_is_valid(&self, proof: &ScipProofRecord) -> bool {
        if proof.path.trim().is_empty()
            || proof.symbol.trim().is_empty()
            || proof.start_line == 0
            || proof.end_line < proof.start_line
            || (proof.end_line == proof.start_line
                && proof.end_character_utf16 < proof.start_character_utf16)
        {
            return false;
        }

        match proof.role.as_str() {
            "definition" => self.symbols.iter().any(|symbol| {
                symbol.path == proof.path
                    && symbol.symbol == proof.symbol
                    && symbol.start_line <= proof.start_line
                    && symbol.end_line >= proof.end_line
            }),
            "reference" => proof
                .target_symbol
                .as_deref()
                .is_some_and(|target| self.symbols.iter().any(|symbol| symbol.symbol == target)),
            _ => false,
        }
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

pub fn import_precise_semantic_scip_artifact(
    artifact_path: &Path,
    import_dir: &Path,
) -> Result<PreciseSemanticImportStatus> {
    if !artifact_path.is_file() {
        return Ok(PreciseSemanticImportStatus::missing(
            "configured_artifact_missing",
        ));
    }
    let body = std::fs::read_to_string(artifact_path).context("read precise semantic import")?;
    let index = match serde_json::from_str::<ScipSymbolsIndex>(&body) {
        Ok(index) => index,
        Err(error) => {
            return Ok(PreciseSemanticImportStatus::invalid(format!(
                "invalid_artifact_json: {error}"
            )));
        }
    };
    if !index.is_fresh_for(&index.revision) {
        return Ok(PreciseSemanticImportStatus::invalid(
            "imported_proof_contract_invalid",
        ));
    }
    std::fs::create_dir_all(import_dir).with_context(|| {
        format!(
            "create precise semantic import dir {}",
            import_dir.display()
        )
    })?;
    std::fs::write(import_dir.join(SCIP_SYMBOLS_FILE), body)
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

/// Write graph-backed SCIP artifacts; returns revision string on success.
pub fn emit_scip_artifacts_from_store(
    storage_path: &Path,
    project_dir: &Path,
) -> Result<Option<String>> {
    std::fs::create_dir_all(project_dir)
        .with_context(|| format!("create scip dir {}", project_dir.display()))?;
    let mut storage = Store::open(storage_path).context("open storage for scip emit")?;
    if storage.get_search_symbol_projection_count().unwrap_or(0) == 0 {
        let _ = storage.rebuild_search_symbol_projection_from_node_table();
    }

    let mut symbols = Vec::new();
    let mut after = None;
    loop {
        let batch = storage
            .get_search_symbol_projection_detail_batch_after(after, 4096)
            .context("load symbols for scip")?;
        if batch.is_empty() {
            break;
        }
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

    if symbols.is_empty() {
        return Ok(None);
    }

    let revision = scip_revision_for_symbols(&symbols);
    let proofs = symbols.iter().map(ScipProofRecord::definition).collect();
    let index = ScipSymbolsIndex {
        revision: revision.clone(),
        contract: ScipProofAdapterContract::graph_projection(&revision),
        symbols,
        proofs,
    };
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

fn scip_revision_for_symbols(symbols: &[ScipSymbolRecord]) -> String {
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
    format!("graph-{:x}", hasher.finalize())[..16].to_string()
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
) -> Result<Option<ScipSymbolsIndex>> {
    let Some(index) = load_scip_symbols(project_dir)? else {
        return Ok(None);
    };
    Ok(index.is_fresh_for(expected_revision).then_some(index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{FileInfo, FileRole, SearchSymbolProjection};
    use tempfile::TempDir;

    #[test]
    fn scip_emit_does_not_stop_at_legacy_smoke_cap() {
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
        let mut projections = Vec::new();
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
            projections.push(SearchSymbolProjection {
                node_id: id,
                display_name: format!("symbol_{index:04}"),
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
        projections.push(SearchSymbolProjection {
            node_id: unknown_node_id,
            display_name: "import_alias".to_string(),
        });
        storage.insert_nodes_batch(&nodes).expect("insert nodes");
        storage
            .upsert_search_symbol_projection_batch(&projections)
            .expect("insert projections");
        drop(storage);

        let scip_dir = project.path().join("scip");
        emit_scip_artifacts_from_store(&storage_path, &scip_dir).expect("emit scip");
        let symbols = load_scip_symbols(&scip_dir)
            .expect("load scip")
            .expect("symbols");

        assert_eq!(
            symbols.contract.evidence_source,
            SCIP_GRAPH_PROJECTION_PROVENANCE
        );
        assert_eq!(symbols.contract.freshness, "fresh");
        assert_eq!(symbols.proofs.len(), symbols.symbols.len());
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
        assert!(
            symbols.symbols.len() >= 4_100,
            "SCIP emit should not truncate at the old 4096-symbol smoke cap"
        );
        assert!(
            symbols
                .symbols
                .iter()
                .any(|symbol| symbol.symbol == "symbol_4099"),
            "SCIP emit should include symbols after the old cap"
        );
    }

    #[test]
    fn configured_precise_semantic_import_copies_fresh_artifact() {
        let project = TempDir::new().expect("project");
        let artifact = project.path().join("import.json");
        let import_dir = project.path().join(SCIP_PRECISE_SEMANTIC_IMPORT_DIR);
        let revision = "imported-a";
        let index = ScipSymbolsIndex {
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
                role: "definition".into(),
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 3,
                start_character_utf16: 0,
                end_line: 3,
                end_character_utf16: 4,
                target_symbol: None,
            }],
        };
        std::fs::write(&artifact, serde_json::to_string_pretty(&index).unwrap()).unwrap();

        let status = import_precise_semantic_scip_artifact(&artifact, &import_dir).expect("import");

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

        let status =
            import_precise_semantic_scip_artifact(&missing, &import_dir).expect("import status");

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
                role: "definition".into(),
                path: "src/lib.rs".into(),
                symbol: "fixture_package::run".into(),
                start_line: 4,
                start_character_utf16: 0,
                end_line: 3,
                end_character_utf16: 4,
                target_symbol: None,
            }],
        };
        std::fs::write(&artifact, serde_json::to_string_pretty(&index).unwrap()).unwrap();

        let status =
            import_precise_semantic_scip_artifact(&artifact, &import_dir).expect("import status");

        assert_eq!(status.status, "invalid");
        assert_eq!(
            status.reason.as_deref(),
            Some("imported_proof_contract_invalid")
        );
        assert!(!import_dir.join(SCIP_SYMBOLS_FILE).exists());
    }
}
