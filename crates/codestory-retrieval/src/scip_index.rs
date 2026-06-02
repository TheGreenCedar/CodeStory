//! Emit SCIP-shaped symbol artifacts from the CodeStory SQLite graph.

use anyhow::{Context, Result};
use codestory_store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const SCIP_SYMBOLS_FILE: &str = "symbols.index.json";
pub const SCIP_INDEX_FILE: &str = "index.scip";
const STUB_MARKER: &str = "index.scip.stub";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipSymbolRecord {
    pub path: String,
    pub symbol: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipSymbolsIndex {
    pub revision: String,
    pub symbols: Vec<ScipSymbolRecord>,
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
            let Some(file_path) = row.file_path.as_deref().map(normalize_scip_path) else {
                continue;
            };
            let start_line = row.start_line.unwrap_or(1);
            let end_line = row.end_line.unwrap_or(start_line).max(start_line);
            symbols.push(ScipSymbolRecord {
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
    let index = ScipSymbolsIndex {
        revision: revision.clone(),
        symbols,
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
        hasher.update(symbol.path.as_bytes());
        hasher.update(symbol.symbol.as_bytes());
        hasher.update(symbol.start_line.to_le_bytes());
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
}
