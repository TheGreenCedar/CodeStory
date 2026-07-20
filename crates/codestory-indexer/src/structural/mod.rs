//! Structural entity collectors with exact source-span anchors.
//!
//! Structural collectors cover source formats where CodeStory can extract
//! useful evidence without claiming parser-backed language support. They emit
//! file rows, nodes, edges, occurrences, and callable projection state with
//! conservative certainty. Callers should present this output as structural
//! source proof, not as semantic resolution or full graph coverage.

mod blanking;
mod cargo_manifest;
mod common;
pub(crate) mod css;
mod docker_compose;
mod generic;
mod github_actions;
mod html;
mod sql;

pub(crate) use blanking::byte_offset_line_col;
pub use blanking::{
    EmbeddedRegion, EmbeddedRegionKind, blank_non_script_regions, blank_outside_regions,
    extract_embedded_regions,
};
/// Return the structural language label stored for `path`.
pub fn structural_language_name(path: &Path) -> &'static str {
    common::structural_language_name(path)
}

use crate::intermediate_storage::IntermediateStorage;
use anyhow::Result;
use codestory_contracts::graph::NodeId;
use codestory_contracts::language_support::{
    is_cargo_manifest_file_path, is_docker_compose_file_path, is_github_actions_workflow_path,
};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const MAX_STRUCTURAL_SOURCE_BYTES: u64 = 1024 * 1024;
pub const MAX_STRUCTURAL_UNITS_PER_FILE: usize = 2048;

#[derive(Debug, thiserror::Error)]
pub enum StructuralCollectionError {
    #[error("structural source is binary or not valid UTF-8")]
    Binary,
    #[error("{0}")]
    Malformed(String),
    #[error(
        "structural source exceeds the {MAX_STRUCTURAL_SOURCE_BYTES}-byte collector limit: {0} bytes"
    )]
    SourceByteLimit(u64),
    #[error(
        "structural source exceeds the {MAX_STRUCTURAL_UNITS_PER_FILE}-unit collector limit: {0} units"
    )]
    UnitLimit(usize),
}

/// Return whether `path` is routed to a structural collector.
pub fn is_structural_candidate_path(path: &Path) -> bool {
    is_structural_format_path(path)
}

/// Return whether `path` has a dedicated or generic structural format route.
pub fn is_structural_format_path(path: &Path) -> bool {
    let path_text = path.to_string_lossy();
    codestory_contracts::language_support::is_structural_source_path(path_text.as_ref())
}

/// Read and structurally index a file from disk.
pub fn index_structural_file(path: &Path) -> Result<IntermediateStorage> {
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 > MAX_STRUCTURAL_SOURCE_BYTES {
        return Err(StructuralCollectionError::SourceByteLimit(bytes.len() as u64).into());
    }
    let source = decode_structural_source(bytes.clone())?;
    let source_content_hash = format!("{:x}", Sha256::digest(&bytes));
    let storage = index_structural_source(path, &source)?;
    finalize_structural_storage(path, &source, &source_content_hash, storage)
}

pub(crate) fn decode_structural_source(
    bytes: Vec<u8>,
) -> std::result::Result<String, StructuralCollectionError> {
    if bytes.iter().any(|byte| *byte == 0)
        || bytes
            .iter()
            .any(|byte| *byte < 0x09 || (*byte > 0x0d && *byte < 0x20))
    {
        return Err(StructuralCollectionError::Binary);
    }
    String::from_utf8(bytes).map_err(|_| StructuralCollectionError::Binary)
}

pub(crate) fn structural_producer(path: &Path) -> Option<&'static str> {
    let path_text = path.to_string_lossy();
    if is_github_actions_workflow_path(path_text.as_ref()) {
        return Some("structural_github_actions_workflow_collector");
    }
    if is_docker_compose_file_path(path_text.as_ref()) {
        return Some("structural_docker_compose_collector");
    }
    if is_cargo_manifest_file_path(path_text.as_ref()) {
        return Some("structural_cargo_manifest_collector");
    }
    match structural_extension(path).as_deref() {
        Some("html" | "htm") => Some("structural_html_collector"),
        Some("css") => Some("structural_css_collector"),
        Some("sql") => Some("structural_sql_collector"),
        Some("md" | "markdown" | "mdx") => Some("structural_markdown_collector"),
        Some("yml" | "yaml") => Some("structural_yaml_collector"),
        Some("toml") => Some("structural_toml_collector"),
        Some("json") => Some("structural_json_collector"),
        Some("zsh" | "ksh" | "command") => Some("structural_shell_collector"),
        Some("ps1" | "psm1") => Some("structural_powershell_collector"),
        _ => None,
    }
}

pub(crate) fn finalize_structural_storage(
    path: &Path,
    source: &str,
    source_content_hash: &str,
    mut storage: IntermediateStorage,
) -> Result<IntermediateStorage> {
    let producer = structural_producer(path)
        .ok_or_else(|| anyhow::anyhow!("unsupported structural collector path"))?;
    let file = storage
        .files
        .first()
        .ok_or_else(|| anyhow::anyhow!("structural collector emitted no file projection"))?
        .clone();
    let structural_unit_ids = storage
        .structural_unit_node_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    storage.file_content_hashes.clear();
    storage.structural_text_units.clear();
    storage.structural_text_projections.clear();
    let mut units_by_node = std::collections::BTreeMap::new();
    for node in storage.nodes.iter().filter(|node| {
        node.kind != codestory_contracts::graph::NodeKind::FILE
            && node
                .file_node_id
                .is_some_and(|file_id| file_id.0 == file.id)
            && structural_unit_ids.contains(&node.id)
    }) {
        let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) =
            (node.start_line, node.start_col, node.end_line, node.end_col)
        else {
            return Err(anyhow::anyhow!(
                "structural evidence node {} has no exact source span",
                node.id.0
            ));
        };
        let exact_source = exact_source_range_bytes(
            source, start_line, start_col, end_line, end_col,
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "structural evidence node {} has an invalid source span",
                node.id.0
            )
        })?;
        let mut content_hasher = Sha256::new();
        hash_part(
            &mut content_hasher,
            b"codestory-structural-text-unit-content-v1",
        );
        hash_part(
            &mut content_hasher,
            &codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION.to_le_bytes(),
        );
        hash_part(&mut content_hasher, producer.as_bytes());
        hash_part(&mut content_hasher, b"structural_text");
        hash_part(&mut content_hasher, b"source_range_only");
        hash_part(&mut content_hasher, file.language.as_bytes());
        hash_part(&mut content_hasher, &(node.kind as i32).to_le_bytes());
        hash_part(&mut content_hasher, source_content_hash.as_bytes());
        hash_part(&mut content_hasher, file.file_role.as_str().as_bytes());
        for coordinate in [start_line, start_col, end_line, end_col] {
            hash_part(&mut content_hasher, &coordinate.to_le_bytes());
        }
        hash_part(&mut content_hasher, exact_source);

        let content_hash = format!("{:x}", content_hasher.finalize());
        let mut placement_hasher = Sha256::new();
        hash_part(
            &mut placement_hasher,
            b"codestory-structural-text-unit-placement-v1",
        );
        hash_part(&mut placement_hasher, &file.id.to_le_bytes());
        hash_part(&mut placement_hasher, &node.id.0.to_le_bytes());
        hash_part(&mut placement_hasher, content_hash.as_bytes());
        for coordinate in [start_line, start_col, end_line, end_col] {
            hash_part(&mut placement_hasher, &coordinate.to_le_bytes());
        }
        units_by_node.insert(
            node.id,
            codestory_store::StructuralTextUnit {
                node_id: node.id,
                file_id: file.id,
                placement_id: format!("{:x}", placement_hasher.finalize()),
                content_hash,
                source_content_hash: source_content_hash.to_string(),
                descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
                producer: producer.to_string(),
                evidence_tier: "structural_text".to_string(),
                resolution: "source_range_only".to_string(),
                language: file.language.clone(),
                kind: node.kind,
                start_line,
                start_col,
                end_line,
                end_col,
                file_role: file.file_role,
            },
        );
    }
    storage.structural_text_units = units_by_node.into_values().collect();
    storage.structural_unit_node_ids.sort_unstable();
    storage.structural_unit_node_ids.dedup();
    storage.structural_text_projections = vec![codestory_store::StructuralTextProjection {
        file_id: file.id,
        source_content_hash: source_content_hash.to_string(),
        descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer: producer.to_string(),
        language: file.language.clone(),
        file_role: file.file_role,
        unit_count: storage.structural_text_units.len() as u64,
        unit_digest: codestory_store::structural_text_unit_digest(&storage.structural_text_units),
    }];
    storage
        .file_content_hashes
        .push(codestory_store::FileContentHash {
            file_id: file.id,
            content_hash: source_content_hash.to_string(),
        });
    Ok(storage)
}

fn hash_part(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn exact_source_range_bytes(
    source: &str,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) -> Option<&[u8]> {
    if start_line == 0
        || start_col == 0
        || end_line < start_line
        || (end_line == start_line && end_col < start_col)
    {
        return None;
    }
    let bytes = source.as_bytes();
    let mut line_starts = vec![0usize];
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            line_starts.push(index + 1);
        }
    }
    let start_base = *line_starts.get(start_line as usize - 1)?;
    let end_base = *line_starts.get(end_line as usize - 1)?;
    let start = start_base.checked_add(start_col as usize - 1)?;
    let end_exclusive = end_base.checked_add(end_col as usize)?;
    if start >= end_exclusive || end_exclusive > bytes.len() {
        return None;
    }
    source.is_char_boundary(start).then_some(())?;
    source.is_char_boundary(end_exclusive).then_some(())?;
    Some(&bytes[start..end_exclusive])
}

/// Add CSS entities extracted from an embedded template style block.
pub fn collect_embedded_style_css(
    path: &Path,
    style_source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    line_offset: u32,
    first_line_col: u32,
) {
    css::collect_css_entities(
        path,
        style_source,
        file_id,
        storage,
        line_offset,
        first_line_col.saturating_sub(1) as usize,
    );
}

/// Structurally index source text for an already admitted path.
///
/// The returned storage is ready to merge into a projection batch and includes
/// callable projection state derived from the structural edges.
pub fn index_structural_source(
    path: &Path,
    source: &str,
) -> std::result::Result<IntermediateStorage, StructuralCollectionError> {
    if source.len() as u64 > MAX_STRUCTURAL_SOURCE_BYTES {
        return Err(StructuralCollectionError::SourceByteLimit(
            source.len() as u64
        ));
    }
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
            Some("css") => css::collect_css_entities(path, source, file_id, &mut storage, 1, 0),
            Some("sql") => sql::collect_sql_entities(path, source, file_id, &mut storage),
            Some("md" | "markdown" | "mdx") => {
                generic::collect_markdown_entities(path, source, file_id, &mut storage)?
            }
            Some("yml" | "yaml") => {
                generic::collect_yaml_entities(path, source, file_id, &mut storage)?
            }
            Some("toml") => generic::collect_toml_entities(path, source, file_id, &mut storage)?,
            Some("json") => generic::collect_json_entities(path, source, file_id, &mut storage)?,
            Some("zsh" | "ksh" | "command") => {
                generic::collect_shell_entities(path, source, file_id, &mut storage)?
            }
            Some("ps1" | "psm1") => {
                generic::collect_powershell_entities(path, source, file_id, &mut storage)?
            }
            _ => {}
        }
    }
    if storage.structural_unit_node_ids.len() > MAX_STRUCTURAL_UNITS_PER_FILE {
        return Err(StructuralCollectionError::UnitLimit(
            storage.structural_unit_node_ids.len(),
        ));
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
    fn structural_families_emit_only_explicit_exact_source_anchors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fixtures: &[(&str, &str, &str, &[&str])] = &[
            (
                ".github/workflows/ci.yml",
                "name: CI\njobs:\n  build:\n    steps:\n      - uses: actions/checkout@v4\n      - run: cargo test\n",
                "structural_github_actions_workflow_collector",
                &[
                    "CI",
                    "build",
                    "- uses: actions/checkout@v4",
                    "- run: cargo test",
                ],
            ),
            (
                "docker-compose.yml",
                "name: demo\nservices:\n  web:\n    image: nginx:1.27\n    ports:\n      - \"8080:80\"\n",
                "structural_docker_compose_collector",
                &["demo", "web", "image: nginx:1.27", "- \"8080:80\""],
            ),
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/api\"]\n[package]\nname = \"demo\"\n[dependencies]\nserde = \"1\"\n",
                "structural_cargo_manifest_collector",
                &["crates/api", "demo", "serde"],
            ),
            (
                "web/index.html",
                "<main id=\"app\">\n<style>.shell { --accent: blue; }</style>\n<script type=\"module\">const boot = () => 1;</script>\n</main>\n",
                "structural_html_collector",
                &["<main", "app", "shell", "--accent", "<script"],
            ),
            (
                "web/styles.css",
                ".shell, #app { color: red; --accent: blue; }\n",
                "structural_css_collector",
                &["shell", "app", "--accent"],
            ),
            (
                "db/schema.sql",
                "CREATE SCHEMA app;\nCREATE TABLE app.users (id INTEGER, email TEXT);\nCREATE INDEX users_email_idx ON app.users (email);\nCREATE VIEW app.active_users AS SELECT * FROM app.users;\nCREATE FUNCTION app.touch_users() RETURNS void AS 'SELECT 1';\n",
                "structural_sql_collector",
                &[
                    "app",
                    "app.users",
                    "id",
                    "email",
                    "users_email_idx",
                    "app.active_users",
                    "app.touch_users",
                ],
            ),
            (
                "docs/guide.mdx",
                "# Guide\n\n```rust\n# Hidden\n[hidden]: ./hidden.md\n```\n\n[api]: ./api.md\n",
                "structural_markdown_collector",
                &["Guide", "api", "rust"],
            ),
            (
                "config/service.yaml",
                "service:\n  literal: |\n    text: [not, a, flow\n    url: https://example.com\n  url: https://example.com\n  endpoints:\n    - https://example.com\n",
                "structural_yaml_collector",
                &["service", "literal", "url", "endpoints"],
            ),
            (
                "config/service.toml",
                "[server]\ndescription = \"\"\"\nfake = \"not a key\"\n[hidden]\n\"\"\"\nhost = \"127.0.0.1\"\n",
                "structural_toml_collector",
                &["server", "description", "host"],
            ),
            (
                "config/service.json",
                "{\"app\":{\"enabled\":true},\"count\":1}\n",
                "structural_json_collector",
                &["app", "enabled", "count"],
            ),
            (
                "scripts/setup.zsh",
                "cat <<'EOF'\nfake() { echo hidden; }\nsource ./hidden.zsh\nEOF\nautoload compinit\nfunction deploy { echo ok; }\nsource ./env.zsh\n",
                "structural_shell_collector",
                &["compinit", "deploy", "./env.zsh"],
            ),
            (
                "scripts/build.ps1",
                "<#\nfunction Invoke-Hidden { }\nImport-Module Hidden\n#>\nfunction Invoke-Build { }\nImport-Module Pester\n. ./common.ps1\n",
                "structural_powershell_collector",
                &["Invoke-Build", "Pester", "./common.ps1"],
            ),
        ];

        for &(relative, source, producer, expected_anchors) in fixtures {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(&path, source).expect("write structural fixture");
            let first = index_structural_file(&path).expect("index structural fixture");
            let second = index_structural_file(&path).expect("repeat structural fixture");

            assert!(!first.structural_text_units.is_empty(), "{relative}");
            assert_eq!(first.structural_text_units, second.structural_text_units);
            assert_eq!(
                first.structural_text_projections,
                second.structural_text_projections
            );
            assert_eq!(first.structural_text_projections.len(), 1);
            let projection = &first.structural_text_projections[0];
            assert_eq!(projection.producer, producer);
            assert_eq!(
                projection.descriptor_version,
                codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION
            );
            assert_eq!(
                projection.unit_count,
                first.structural_text_units.len() as u64
            );
            assert_eq!(
                projection.unit_digest,
                codestory_store::structural_text_unit_digest(&first.structural_text_units)
            );
            let mut actual_anchors = Vec::new();
            for unit in &first.structural_text_units {
                assert_eq!(unit.producer, producer);
                assert_eq!(unit.evidence_tier, "structural_text");
                assert_eq!(unit.resolution, "source_range_only");
                assert_eq!(unit.source_content_hash, projection.source_content_hash);
                assert_eq!(unit.content_hash.len(), 64);
                assert_eq!(unit.placement_id.len(), 64);
                assert_eq!(unit.file_role, first.files[0].file_role);
                let exact = exact_source_range_bytes(
                    source,
                    unit.start_line,
                    unit.start_col,
                    unit.end_line,
                    unit.end_col,
                )
                .expect("unit exact source span");
                let exact = std::str::from_utf8(exact).expect("UTF-8 structural source span");
                assert!(
                    expected_anchors.contains(&exact),
                    "{relative} emitted fabricated or unexpected unit slice {exact:?}"
                );
                actual_anchors.push(exact.to_string());
            }
            actual_anchors.sort();
            let mut expected_anchors = expected_anchors
                .iter()
                .map(|anchor| (*anchor).to_string())
                .collect::<Vec<_>>();
            expected_anchors.sort();
            assert_eq!(actual_anchors, expected_anchors, "{relative}");
        }
    }

    #[test]
    fn shell_literal_heredoc_operators_do_not_hide_later_anchors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fixtures = [
            (
                "double-quoted",
                "printf '%s\\n' \"<<NOT_A_HEREDOC\"\nfunction after_double { echo ok; }\nsource ./after-double.zsh\n",
                ["after_double", "./after-double.zsh"],
            ),
            (
                "single-quoted",
                "printf '%s\\n' '<<NOT_A_HEREDOC'\nfunction after_single { echo ok; }\nsource ./after-single.zsh\n",
                ["after_single", "./after-single.zsh"],
            ),
            (
                "escaped",
                "printf '%s\\n' \\<<NOT_A_HEREDOC\nfunction after_escape { echo ok; }\nsource ./after-escape.zsh\n",
                ["after_escape", "./after-escape.zsh"],
            ),
        ];

        for (name, source, expected) in fixtures {
            let path = dir.path().join(format!("{name}.zsh"));
            std::fs::write(&path, source).expect("write shell fixture");
            let storage = index_structural_file(&path).expect("index shell fixture");
            let mut actual = storage
                .structural_text_units
                .iter()
                .map(|unit| {
                    exact_source_range_bytes(
                        source,
                        unit.start_line,
                        unit.start_col,
                        unit.end_line,
                        unit.end_col,
                    )
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .expect("unit exact source span")
                    .to_string()
                })
                .collect::<Vec<_>>();
            actual.sort();
            let mut expected = expected.map(str::to_string).to_vec();
            expected.sort();
            assert_eq!(actual, expected, "{name}");
        }
    }

    #[test]
    fn shell_real_quoted_heredocs_hide_body_anchors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("quoted-heredocs.zsh");
        let source = "cat <<'SINGLE'\nfunction hidden_single { echo hidden; }\nsource ./hidden-single.zsh\nSINGLE\ncat <<\"DOUBLE\"\nfunction hidden_double { echo hidden; }\nsource ./hidden-double.zsh\nDOUBLE\ncat <<-'TABBED'\n\tfunction hidden_tabbed { echo hidden; }\n\tsource ./hidden-tabbed.zsh\n\tTABBED\nfunction visible { echo ok; }\nsource ./visible.zsh\n";
        std::fs::write(&path, source).expect("write shell fixture");

        let storage = index_structural_file(&path).expect("index shell fixture");
        let mut actual = storage
            .structural_text_units
            .iter()
            .map(|unit| {
                exact_source_range_bytes(
                    source,
                    unit.start_line,
                    unit.start_col,
                    unit.end_line,
                    unit.end_col,
                )
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .expect("unit exact source span")
                .to_string()
            })
            .collect::<Vec<_>>();
        actual.sort();
        assert_eq!(actual, ["./visible.zsh", "visible"]);
    }

    #[test]
    fn generic_structural_routing_keeps_specialized_and_parser_backed_precedence() {
        assert_eq!(
            structural_producer(Path::new(".github/workflows/ci.yaml")),
            Some("structural_github_actions_workflow_collector")
        );
        assert_eq!(
            structural_producer(Path::new("deploy/docker-compose.yml")),
            Some("structural_docker_compose_collector")
        );
        assert_eq!(
            structural_producer(Path::new("crates/app/Cargo.toml")),
            Some("structural_cargo_manifest_collector")
        );
        #[cfg(windows)]
        assert_eq!(
            structural_producer(Path::new("crates/app/CARGO.TOML")),
            Some("structural_cargo_manifest_collector")
        );
        #[cfg(not(windows))]
        assert_eq!(
            structural_producer(Path::new("crates/app/CARGO.TOML")),
            Some("structural_toml_collector")
        );
        assert_eq!(
            structural_producer(Path::new("config/settings.yaml")),
            Some("structural_yaml_collector")
        );
        assert_eq!(
            structural_producer(Path::new("config/settings.toml")),
            Some("structural_toml_collector")
        );
        assert!(!is_structural_candidate_path(Path::new("scripts/run.sh")));
        assert!(!is_structural_candidate_path(Path::new("scripts/run.bash")));
        assert!(is_structural_candidate_path(Path::new("scripts/run.zsh")));
        assert!(is_structural_candidate_path(Path::new("scripts/run.ps1")));
        assert_eq!(
            codestory_contracts::language_support::parser_backed_language_name_for_path(Some(
                "scripts/run.sh"
            )),
            Some("bash")
        );
    }

    #[test]
    fn structural_format_routing_defers_relative_exclusion_to_the_shared_policy() {
        for path in [
            "vendor/config.json",
            r"third_party\docs\guide.md",
            "generated/settings.yaml",
            r"target\release\metadata.json",
            "config/package-lock.json",
            r"config\pnpm-lock.yaml",
            "config/.env.production.json",
            r"secrets\deploy.ps1",
            "web/app.min.json",
            r"docs\guide.generated.md",
        ] {
            assert!(
                is_structural_format_path(Path::new(path)),
                "fixture must have a structural extension: {path}"
            );
            assert!(
                codestory_contracts::language_support::structural_source_path_exclusion(path)
                    .is_some(),
                "missing policy exclusion: {path}"
            );
            assert!(
                is_structural_candidate_path(Path::new(path)),
                "format routing should remain independent of policy: {path}"
            );
        }
        assert!(is_structural_candidate_path(Path::new(
            "config/package.json"
        )));
        assert!(is_structural_candidate_path(Path::new(
            r"docs\architecture\guide.md"
        )));
    }

    #[test]
    fn generic_collectors_reject_malformed_binary_and_over_bound_input_without_units() {
        for (path, source) in [
            ("config.json", "{\"unterminated\":"),
            ("config.toml", "[table\nkey = 1"),
            ("config.yaml", "items: [one, two\n"),
        ] {
            assert!(
                matches!(
                    index_structural_source(Path::new(path), source),
                    Err(StructuralCollectionError::Malformed(_))
                ),
                "{path} should reject malformed syntax"
            );
        }
        assert!(matches!(
            decode_structural_source(vec![0xff, 0xfe]),
            Err(StructuralCollectionError::Binary)
        ));
        assert!(matches!(
            decode_structural_source(b"key\0value".to_vec()),
            Err(StructuralCollectionError::Binary)
        ));

        let oversized = "x".repeat(MAX_STRUCTURAL_SOURCE_BYTES as usize + 1);
        assert!(matches!(
            index_structural_source(Path::new("guide.md"), &oversized),
            Err(StructuralCollectionError::SourceByteLimit(_))
        ));

        let mut keys = String::from("{");
        for index in 0..=MAX_STRUCTURAL_UNITS_PER_FILE {
            if index > 0 {
                keys.push(',');
            }
            keys.push_str(&format!("\"key{index}\":{index}"));
        }
        keys.push('}');
        assert!(matches!(
            index_structural_source(Path::new("many.json"), &keys),
            Err(StructuralCollectionError::UnitLimit(_))
        ));
    }

    #[test]
    fn moved_generic_structural_source_keeps_content_identity_and_changes_placement() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source = "# Guide\n\n[api]: ./api.md\n";
        let first_path = dir.path().join("first/guide.md");
        let second_path = dir.path().join("second/guide.md");
        for path in [&first_path, &second_path] {
            std::fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            std::fs::write(path, source).expect("write markdown fixture");
        }
        let first = index_structural_file(&first_path).expect("index first markdown");
        let second = index_structural_file(&second_path).expect("index second markdown");
        let mut first_content = first
            .structural_text_units
            .iter()
            .map(|unit| &unit.content_hash)
            .collect::<Vec<_>>();
        let mut second_content = second
            .structural_text_units
            .iter()
            .map(|unit| &unit.content_hash)
            .collect::<Vec<_>>();
        first_content.sort();
        second_content.sort();
        assert_eq!(first_content, second_content);
        assert!(
            first
                .structural_text_units
                .iter()
                .zip(&second.structural_text_units)
                .all(|(left, right)| {
                    left.node_id != right.node_id && left.placement_id != right.placement_id
                })
        );
    }

    #[test]
    fn path_embedding_collectors_keep_content_identity_out_of_graph_placement() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fixtures = [
            (
                "workflow",
                ".github/workflows/first.yml",
                ".github/workflows/second.yml",
                "name: CI\njobs:\n  build:\n    runs-on: ubuntu-latest\n",
            ),
            (
                "cargo",
                "first/Cargo.toml",
                "second/Cargo.toml",
                "[package]\nname = \"demo\"\n[dependencies]\nserde = \"1\"\n",
            ),
            (
                "compose",
                "first/docker-compose.yml",
                "second/docker-compose.yml",
                "services:\n  web:\n    image: nginx\n",
            ),
            (
                "html",
                "first/index.html",
                "second/index.html",
                "<main id=\"app\"><script>const boot = 1;</script></main>\n",
            ),
            (
                "css",
                "first/styles.css",
                "second/styles.css",
                ".card { color: red; }\n",
            ),
        ];

        for (label, first_relative, second_relative, source) in fixtures {
            let first_path = dir.path().join(first_relative);
            let second_path = dir.path().join(second_relative);
            for path in [&first_path, &second_path] {
                std::fs::create_dir_all(path.parent().expect("fixture parent"))
                    .expect("create fixture parent");
                std::fs::write(path, source).expect("write structural fixture");
            }

            let first = index_structural_file(&first_path).expect("index first fixture");
            let second = index_structural_file(&second_path).expect("index second fixture");
            let mut first_content = first
                .structural_text_units
                .iter()
                .map(|unit| unit.content_hash.clone())
                .collect::<Vec<_>>();
            let mut second_content = second
                .structural_text_units
                .iter()
                .map(|unit| unit.content_hash.clone())
                .collect::<Vec<_>>();
            first_content.sort();
            second_content.sort();
            assert_eq!(first_content, second_content, "{label}");
            assert!(first.structural_text_units.iter().all(|left| {
                second.structural_text_units.iter().all(|right| {
                    left.node_id != right.node_id && left.placement_id != right.placement_id
                })
            }));
        }
    }

    #[test]
    fn duplicate_exact_bytes_at_distinct_spans_have_distinct_descriptor_and_placement_identity() {
        let source = "services:\n  web:\n    ports:\n      - shared:/data\n      - shared:/data\n";
        let collected = index_structural_source(Path::new("docker-compose.yml"), source)
            .expect("collect duplicate structural anchors");
        let storage = finalize_structural_storage(
            Path::new("docker-compose.yml"),
            source,
            &format!("{:x}", Sha256::digest(source.as_bytes())),
            collected,
        )
        .expect("finalize duplicate structural anchors");
        let duplicates = storage
            .structural_text_units
            .iter()
            .filter(|unit| {
                exact_source_range_bytes(
                    source,
                    unit.start_line,
                    unit.start_col,
                    unit.end_line,
                    unit.end_col,
                ) == Some(b"- shared:/data".as_slice())
            })
            .collect::<Vec<_>>();
        assert_eq!(duplicates.len(), 2);
        assert_ne!(duplicates[0].content_hash, duplicates[1].content_hash);
        assert_ne!(duplicates[0].node_id, duplicates[1].node_id);
        assert_ne!(duplicates[0].placement_id, duplicates[1].placement_id);
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
        assert!(is_structural_candidate_path(Path::new("openapi.yaml")));
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
    fn docker_compose_admission_precedes_generic_yaml() {
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
        assert!(is_structural_candidate_path(Path::new("openapi.yaml")));
        assert!(is_structural_candidate_path(Path::new("docs/workflow.yml")));
        assert_eq!(
            structural_producer(Path::new("docs/workflow.yml")),
            Some("structural_yaml_collector")
        );

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
    fn cargo_manifest_admission_precedes_generic_toml() {
        assert!(is_structural_candidate_path(Path::new("Cargo.toml")));
        assert!(is_structural_candidate_path(Path::new(
            "crates/codestory-cli/Cargo.toml"
        )));
        assert!(is_structural_candidate_path(Path::new("config.toml")));
        assert!(is_structural_candidate_path(Path::new(
            ".cargo/config.toml"
        )));
        assert!(!is_structural_candidate_path(Path::new("Cargo.lock")));
        assert_eq!(
            structural_producer(Path::new("config.toml")),
            Some("structural_toml_collector")
        );
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
                file_content_hashes: &[],
                nodes: &projected.nodes,
                structural_text_units: &projected.structural_text_units,
                structural_text_projections: &projected.structural_text_projections,
                structural_text_cache_writes: &[],
                edges: &projected.edges,
                occurrences: &projected.occurrences,
                component_access: &projected.component_access,
                callable_projection_states: &projected.callable_projection_states,
            })?;
        Ok(())
    }

    #[test]
    fn finalized_html_keeps_delegated_parser_descendants_out_of_structural_units() {
        let source = "<main id=\"app\"><script type=\"module\">function boot() { return 1; }</script></main>";
        let collected = index_structural_source(Path::new("index.html"), source)
            .expect("collect HTML structural units");
        let storage = finalize_structural_storage(
            Path::new("index.html"),
            source,
            &format!("{:x}", Sha256::digest(source.as_bytes())),
            collected,
        )
        .expect("finalize HTML structural units");
        let unit_ids = storage
            .structural_text_units
            .iter()
            .map(|unit| unit.node_id)
            .collect::<HashSet<_>>();
        let delegated_boot = storage
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::FUNCTION && node.serialized_name.contains("boot"))
            .expect("delegated parser function");
        assert!(!unit_ids.contains(&delegated_boot.id));
        let mut anchors = storage
            .structural_text_units
            .iter()
            .map(|unit| {
                std::str::from_utf8(
                    exact_source_range_bytes(
                        source,
                        unit.start_line,
                        unit.start_col,
                        unit.end_line,
                        unit.end_col,
                    )
                    .expect("exact HTML unit span"),
                )
                .expect("UTF-8 HTML unit")
                .to_string()
            })
            .collect::<Vec<_>>();
        anchors.sort();
        assert_eq!(anchors, vec!["<main", "<script", "app"]);
    }

    #[test]
    fn zero_unit_structural_file_publishes_a_complete_projection() -> anyhow::Result<()> {
        let source = "/* deliberately contains no CSS anchors */\n";
        let collected = index_structural_source(Path::new("empty.css"), source)?;
        let projected = finalize_structural_storage(
            Path::new("empty.css"),
            source,
            &format!("{:x}", Sha256::digest(source.as_bytes())),
            collected,
        )?;
        assert!(projected.structural_text_units.is_empty());
        assert_eq!(projected.structural_text_projections.len(), 1);
        assert_eq!(projected.structural_text_projections[0].unit_count, 0);

        let mut storage = Storage::new_in_memory()?;
        storage
            .projections()
            .flush_projection_batch(ProjectionBatch {
                files: &projected.files,
                file_content_hashes: &projected.file_content_hashes,
                nodes: &projected.nodes,
                structural_text_units: &projected.structural_text_units,
                structural_text_projections: &projected.structural_text_projections,
                structural_text_cache_writes: &[],
                edges: &projected.edges,
                occurrences: &projected.occurrences,
                component_access: &projected.component_access,
                callable_projection_states: &projected.callable_projection_states,
            })?;
        let publication = codestory_store::IndexPublicationRecord {
            generation: 1,
            generation_id: "zero-unit-generation".into(),
            run_id: "zero-unit-run".into(),
            mode: codestory_store::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        let manifest = storage.publish_structural_text_unit_generation(&publication)?;
        assert_eq!(manifest.unit_count, 0);
        assert_eq!(manifest.projection_count, 1);
        storage.validate_structural_text_unit_publication(&publication)?;
        Ok(())
    }
}
