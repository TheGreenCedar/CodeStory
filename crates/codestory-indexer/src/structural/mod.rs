//! Structural-language entity collectors (HTML, CSS, SQL) with explicit node/edge mapping.

mod blanking;
mod common;
pub(crate) mod css;
mod html;
mod sql;

pub use blanking::{
    EmbeddedRegion, EmbeddedRegionKind, blank_non_script_regions, blank_outside_regions,
    extract_embedded_regions,
};
pub fn structural_language_name(path: &Path) -> &'static str {
    common::structural_language_name(path)
}

use crate::intermediate_storage::IntermediateStorage;
use anyhow::Result;
use codestory_contracts::graph::NodeId;
use std::path::Path;

pub fn is_structural_candidate_path(path: &Path) -> bool {
    matches!(
        structural_extension(path).as_deref(),
        Some("html" | "htm" | "css" | "sql")
    )
}

pub fn index_structural_file(path: &Path) -> Result<IntermediateStorage> {
    let source = std::fs::read_to_string(path)?;
    index_structural_source(path, &source)
}

pub fn collect_embedded_style_css(
    path: &Path,
    style_source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    line_offset: u32,
) {
    css::collect_css_entities(path, style_source, file_id, storage, line_offset);
}

pub fn index_structural_source(path: &Path, source: &str) -> Result<IntermediateStorage> {
    let mut storage = IntermediateStorage::default();
    let (file_node, _file_name, file_id) = crate::file_node_from_source(path, source);
    storage.files.push(codestory_store::FileInfo {
        id: file_id.0,
        path: path.to_path_buf(),
        language: common::structural_language_name(path).to_string(),
        modification_time: file_modification_time(path),
        indexed: true,
        complete: true,
        line_count: source.lines().count() as u32,
        file_role: codestory_store::FileRole::classify_path(path),
    });
    storage.nodes.push(file_node);

    match structural_extension(path).as_deref() {
        Some("html" | "htm") => html::collect_html_entities(path, source, file_id, &mut storage),
        Some("css") => css::collect_css_entities(path, source, file_id, &mut storage, 1),
        Some("sql") => sql::collect_sql_entities(path, source, file_id, &mut storage),
        _ => {}
    }

    storage.callable_projection_states = crate::build_callable_projection_states(
        &storage.nodes,
        &storage.edges,
        &storage.occurrences,
    );
    Ok(storage)
}

fn structural_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

fn file_modification_time(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(|time| {
            time.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{EdgeKind, NodeKind};
    use codestory_store::{ProjectionBatch, Store as Storage};
    use std::collections::HashSet;

    #[test]
    fn indexes_dedicated_sql_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("schema.sql");
        let sql =
            "CREATE TABLE public.items (id INT);\nCREATE INDEX items_id_idx ON public.items (id);";
        std::fs::write(&path, sql).expect("write sql");
        let storage = index_structural_file(&path).expect("index sql");
        assert!(storage.nodes.iter().any(|n| n.kind == NodeKind::CLASS));
        assert_eq!(storage.files[0].language, "sql");
    }

    #[test]
    fn html_inline_endpoint_calls_do_not_keep_delegated_file_edges() -> anyhow::Result<()> {
        let html = r#"<!doctype html>
<html>
  <body>
    <script>
      axios.get('/get/server').then(function (response) {
        return response.data;
      });
    </script>
  </body>
</html>"#;
        let projected = index_structural_source(Path::new("examples/get/index.html"), html)?;
        let node_ids = projected
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<HashSet<_>>();
        for edge in &projected.edges {
            assert!(
                node_ids.contains(&edge.source),
                "edge source should be present: {edge:?}"
            );
            assert!(
                node_ids.contains(&edge.target),
                "edge target should be present: {edge:?}"
            );
            if let Some(file_node_id) = edge.file_node_id {
                assert!(
                    node_ids.contains(&file_node_id),
                    "edge file node should be present: {edge:?}"
                );
            }
        }

        assert!(
            projected.edges.iter().any(|edge| {
                edge.kind == EdgeKind::CALL
                    && projected.nodes.iter().any(|node| {
                        node.id == edge.target
                            && node.canonical_id.as_deref()
                                == Some("openapi:endpoint:GET /get/server")
                    })
            }),
            "expected an inline endpoint CALL edge"
        );

        let mut storage = Storage::new_in_memory()?;
        storage
            .projections()
            .flush_projection_batch(ProjectionBatch {
                files: &projected.files,
                nodes: &projected.nodes,
                edges: &projected.edges,
                occurrences: &projected.occurrences,
                component_access: &projected.component_access,
                callable_projection_states: &projected.callable_projection_states,
            })?;
        Ok(())
    }
}
