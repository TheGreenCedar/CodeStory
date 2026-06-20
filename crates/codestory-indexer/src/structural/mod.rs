//! Structural-language entity collectors (HTML, CSS, SQL) with explicit node/edge mapping.

mod blanking;
mod cargo_manifest;
mod common;
pub(crate) mod css;
mod docker_compose;
mod github_actions;
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
use codestory_contracts::language_support::{
    is_cargo_manifest_file_path, is_docker_compose_file_path, is_github_actions_workflow_path,
};
use std::path::Path;

pub fn is_structural_candidate_path(path: &Path) -> bool {
    let path_text = path.to_string_lossy();
    if is_github_actions_workflow_path(path_text.as_ref())
        || is_docker_compose_file_path(path_text.as_ref())
        || is_cargo_manifest_file_path(path_text.as_ref())
    {
        return true;
    }
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

    let path_key = path.to_string_lossy();
    if is_github_actions_workflow_path(path_key.as_ref()) {
        github_actions::collect_github_actions_workflow_entities(
            path,
            source,
            file_id,
            &mut storage,
        );
    } else if is_docker_compose_file_path(path_key.as_ref()) {
        docker_compose::collect_docker_compose_entities(path, source, file_id, &mut storage);
    } else if is_cargo_manifest_file_path(path_key.as_ref()) {
        cargo_manifest::collect_cargo_manifest_entities(path, source, file_id, &mut storage);
    } else {
        match structural_extension(path).as_deref() {
            Some("html" | "htm") => {
                html::collect_html_entities(path, source, file_id, &mut storage)
            }
            Some("css") => css::collect_css_entities(path, source, file_id, &mut storage, 1),
            Some("sql") => sql::collect_sql_entities(path, source, file_id, &mut storage),
            _ => {}
        }
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
    fn indexes_github_actions_workflow_as_structural_source_proof() {
        let dir = tempfile::tempdir().expect("temp dir");
        let workflow_dir = dir.path().join(".github").join("workflows");
        std::fs::create_dir_all(&workflow_dir).expect("workflow dir");
        let path = workflow_dir.join("ci.yml");
        let yaml = r#"name: CI
on:
  push:
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Cargo test
        run: cargo test
      - run: cargo fmt
"#;
        std::fs::write(&path, yaml).expect("write workflow");

        let storage = index_structural_file(&path).expect("index workflow");

        assert_eq!(storage.files[0].language, "github_actions_workflow");
        assert!(
            storage
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::MODULE
                    && node.serialized_name == "CI"
                    && node.start_line == Some(1)
                    && node.start_col == Some(7)
                    && node.end_col == Some(8)),
            "workflow name should be an exact source-span node"
        );
        let build = storage
            .nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::FUNCTION
                    && node
                        .canonical_id
                        .as_deref()
                        .is_some_and(|value| value.contains("github-actions:job:"))
                    && node.serialized_name == "build"
            })
            .expect("build job node");
        assert_eq!(build.start_line, Some(5));
        assert_eq!(build.start_col, Some(3));
        assert_eq!(build.end_col, Some(7));
        assert_exact_step_span(&storage, "- uses: actions/checkout@v4", 8, 7);
        assert_exact_step_span(&storage, "- name: Cargo test", 9, 7);
        assert_exact_step_span(&storage, "- run: cargo fmt", 11, 7);
        assert!(
            storage
                .edges
                .iter()
                .any(|edge| edge.kind == EdgeKind::MEMBER)
        );
        assert!(!is_structural_candidate_path(Path::new("openapi.yaml")));
    }

    #[test]
    fn indexes_docker_compose_services_as_structural_source_proof() {
        let source = r#"name: demo-stack
services:
  web:
    image: nginx:1.27
    ports:
      - "8080:80"
    environment:
      RACK_ENV: production
      FEATURE_FLAG: "true"
    volumes:
      - ./site:/usr/share/nginx/html:ro
  worker:
    build: ./worker
    environment:
      - QUEUE=critical
    volumes:
      - worker-cache:/cache
"#;

        let storage =
            index_structural_source(Path::new("docker/demo-compose.yml"), source).unwrap();

        assert_eq!(storage.files[0].language, "docker_compose");
        assert!(
            storage.nodes.iter().any(|node| {
                node.kind == NodeKind::MODULE
                    && node.serialized_name == "demo-stack"
                    && node.start_line == Some(1)
                    && node.start_col == Some(7)
                    && node.end_col == Some(16)
            }),
            "compose stack name should be an exact source-span node"
        );
        assert_compose_node_span(&storage, NodeKind::FUNCTION, "web", 3, 3);
        assert_compose_node_span(&storage, NodeKind::FUNCTION, "worker", 12, 3);
        assert_compose_node_span(&storage, NodeKind::ANNOTATION, "image: nginx:1.27", 4, 5);
        assert_compose_node_span(&storage, NodeKind::ANNOTATION, "build: ./worker", 13, 5);
        assert_compose_node_span(&storage, NodeKind::ANNOTATION, "- \"8080:80\"", 6, 7);
        assert_compose_node_span(&storage, NodeKind::ANNOTATION, "RACK_ENV", 8, 7);
        assert_compose_node_span(&storage, NodeKind::ANNOTATION, "FEATURE_FLAG", 9, 7);
        assert_compose_node_span(&storage, NodeKind::ANNOTATION, "QUEUE", 15, 9);
        assert_compose_node_span(
            &storage,
            NodeKind::ANNOTATION,
            "- ./site:/usr/share/nginx/html:ro",
            11,
            7,
        );
        assert_compose_node_span(
            &storage,
            NodeKind::ANNOTATION,
            "- worker-cache:/cache",
            17,
            7,
        );
        assert!(
            storage
                .edges
                .iter()
                .any(|edge| edge.kind == EdgeKind::MEMBER)
        );
    }

    #[test]
    fn docker_compose_admission_is_path_scoped_not_generic_yaml() {
        assert!(is_structural_candidate_path(Path::new(
            "docker/retrieval-compose.yml"
        )));
        assert!(is_structural_candidate_path(Path::new("compose.yaml")));
        assert!(is_structural_candidate_path(Path::new(
            "docker-compose.override.yml"
        )));
        assert!(is_structural_candidate_path(Path::new(
            r"deploy\compose.yaml"
        )));
        assert!(is_structural_candidate_path(Path::new(
            ".github/workflows/ci.yml"
        )));
        assert!(!is_structural_candidate_path(Path::new("openapi.yaml")));
        assert!(!is_structural_candidate_path(Path::new(
            "docs/workflow.yml"
        )));

        let storage = index_structural_source(Path::new("compose.yml"), "openapi: 3.1.0\n")
            .expect("index unsupported compose-shaped yaml");
        assert!(
            storage.nodes.iter().all(|node| {
                !node
                    .canonical_id
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("docker-compose:")
            }),
            "unsupported compose-shaped yaml should not invent compose anchors"
        );
    }

    #[test]
    fn docker_compose_requires_services_section_to_emit_anchors() {
        let storage =
            index_structural_source(Path::new("compose.yaml"), "name: api\nopenapi: 3.1.0\n")
                .expect("index admitted non-compose yaml");
        assert!(
            storage.nodes.iter().all(|node| {
                !node
                    .canonical_id
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("docker-compose:")
            }),
            "compose-looking yaml without services must not invent compose anchors"
        );
    }

    #[test]
    fn cargo_manifest_admission_is_basename_scoped_not_generic_toml() {
        assert!(is_structural_candidate_path(Path::new("Cargo.toml")));
        assert!(is_structural_candidate_path(Path::new(
            "crates/codestory-cli/Cargo.toml"
        )));
        assert!(!is_structural_candidate_path(Path::new("config.toml")));
        assert!(!is_structural_candidate_path(Path::new(
            ".cargo/config.toml"
        )));
        assert!(!is_structural_candidate_path(Path::new("Cargo.lock")));
    }

    #[test]
    fn indexes_cargo_manifest_source_proof_anchors() {
        let source = r#"[workspace] # workspace table
members = ["crates/api", "crates/cli"] # "not-a-member"

[package]
name = "demo"

[dependencies] # direct dependencies only
serde = "1"
tokio = { version = "1" }

[dev-dependencies]
pretty_assertions = "1"

[build-dependencies]
cc = "1"

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[workspace.dependencies]
anyhow = "1"

[dependencies.tracing]
version = "0.1"

[features]
default = ["serde"]

[patch.crates-io]
serde = { git = "https://example.invalid/serde" }

[replace]
replace_serde = { path = "vendor/serde" }
"#;

        let storage = index_structural_source(Path::new("Cargo.toml"), source).unwrap();

        assert_eq!(storage.files[0].language, "cargo_manifest");
        assert_manifest_node_span(&storage, NodeKind::MODULE, "crates/api", 2, 13);
        assert_manifest_node_span(&storage, NodeKind::MODULE, "crates/cli", 2, 27);
        assert_manifest_node_span(&storage, NodeKind::PACKAGE, "demo", 5, 9);
        assert_manifest_node_span(&storage, NodeKind::ANNOTATION, "serde", 8, 1);
        assert_manifest_node_span(&storage, NodeKind::ANNOTATION, "tokio", 9, 1);
        assert_manifest_node_span(&storage, NodeKind::ANNOTATION, "pretty_assertions", 12, 1);
        assert_manifest_node_span(&storage, NodeKind::ANNOTATION, "cc", 15, 1);
        for ignored in [
            "libc",
            "anyhow",
            "tracing",
            "default",
            "not-a-member",
            "replace_serde",
        ] {
            assert!(
                !storage
                    .nodes
                    .iter()
                    .any(|node| node.serialized_name == ignored),
                "unsupported Cargo table should not emit {ignored}"
            );
        }
    }

    #[test]
    fn unnamed_github_actions_workflow_uses_jobs_source_anchor() {
        let yaml = r#"on:
  push:
jobs:
  test:
    steps:
      - run: cargo test
"#;

        let storage = index_structural_source(Path::new(".github/workflows/unnamed.yml"), yaml)
            .expect("index workflow");

        assert!(
            !storage
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::MODULE && node.serialized_name == "unnamed"),
            "workflow module must not be derived from the filename"
        );
        let workflow = storage
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::MODULE && node.serialized_name == "jobs:")
            .expect("jobs source anchor");
        assert_eq!(workflow.start_line, Some(3));
        assert_eq!(workflow.start_col, Some(1));
        assert_eq!(workflow.end_col, Some(5));
    }

    #[test]
    fn github_actions_workflow_name_span_uses_scalar_value_not_key_text() {
        assert_workflow_name_span(
            r#"name: "name"
on:
  push:
jobs:
  test:
    steps:
      - run: cargo test
"#,
            "name",
            8,
            11,
        );
        assert_workflow_name_span(
            r#"name: me
on:
  push:
jobs:
  test:
    steps:
      - run: cargo test
"#,
            "me",
            7,
            8,
        );
    }

    fn assert_workflow_name_span(
        source: &str,
        expected_name: &str,
        expected_start_col: u32,
        expected_end_col: u32,
    ) {
        let storage = index_structural_source(Path::new(".github/workflows/name.yml"), source)
            .expect("index workflow");
        let workflow = storage
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::MODULE && node.serialized_name == expected_name)
            .unwrap_or_else(|| panic!("missing workflow node {expected_name}"));
        assert_eq!(workflow.start_line, Some(1));
        assert_eq!(workflow.start_col, Some(expected_start_col));
        assert_eq!(workflow.end_col, Some(expected_end_col));
    }

    fn assert_exact_step_span(
        storage: &IntermediateStorage,
        source_anchor: &str,
        line: u32,
        col: u32,
    ) {
        let node = storage
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::ANNOTATION && node.serialized_name == source_anchor)
            .unwrap_or_else(|| panic!("missing exact step anchor {source_anchor}"));
        assert_eq!(node.start_line, Some(line));
        assert_eq!(node.start_col, Some(col));
        assert_eq!(
            node.end_col,
            Some(col + source_anchor.len() as u32 - 1),
            "step span must cover only source text"
        );
    }

    fn assert_compose_node_span(
        storage: &IntermediateStorage,
        kind: NodeKind,
        source_anchor: &str,
        line: u32,
        col: u32,
    ) {
        let node = storage
            .nodes
            .iter()
            .find(|node| node.kind == kind && node.serialized_name == source_anchor)
            .unwrap_or_else(|| panic!("missing compose source anchor {source_anchor}"));
        assert_eq!(node.start_line, Some(line));
        assert_eq!(node.start_col, Some(col));
        assert_eq!(
            node.end_col,
            Some(col + source_anchor.len() as u32 - 1),
            "compose span must cover only source text"
        );
    }

    fn assert_manifest_node_span(
        storage: &IntermediateStorage,
        kind: NodeKind,
        source_anchor: &str,
        line: u32,
        col: u32,
    ) {
        let node = storage
            .nodes
            .iter()
            .find(|node| node.kind == kind && node.serialized_name == source_anchor)
            .unwrap_or_else(|| panic!("missing Cargo manifest source anchor {source_anchor}"));
        assert_eq!(node.start_line, Some(line));
        assert_eq!(node.start_col, Some(col));
        assert_eq!(
            node.end_col,
            Some(col + source_anchor.len() as u32 - 1),
            "Cargo manifest span must cover only source text"
        );
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
