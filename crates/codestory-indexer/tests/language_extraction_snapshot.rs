//! Byte-exact projection snapshots for languages whose extraction rules have
//! been moved out of the indexer god file into `src/languages/<lang>.rs`.
//!
//! `fidelity_regression` and `tictactoe_language_coverage` are threshold tests:
//! they assert minimum counts and the presence of required names, so a move
//! that silently *added* nodes, dropped a callsite marker, changed a qualified
//! name, or lost a resolved owner still passes them. This file pins the whole
//! rendered projection of the same two fixtures instead, which is what the S3
//! contract actually requires ("outputs equal before and after the move").
//!
//! Two indexing paths are captured on purpose, because they exercise different
//! halves of a language's rules:
//!
//! * the `fidelity_lab` fixture through the full store pipeline
//!   (`WorkspaceIndexer::run_incremental`), which runs call resolution and so
//!   covers the manual receiver-call engine and its resolved owners; and
//! * the `tictactoe` fixture through raw `index_file`, which covers the
//!   tree-sitter rule file, occurrences, and component access without any
//!   resolution on top.
//!
//! Sibling packages (#1677-#1691) add one `SnapshotCase` row plus two golden
//! files; they do not change this harness. To produce a new golden, run the
//! failing assertion and copy its `left` value into the fixture — the goldens
//! are never written by the test itself, so a regression cannot bless itself.

use anyhow::{Result, anyhow};
use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{AccessKind, Edge, Node, NodeId};
use codestory_indexer::{IndexResult, WorkspaceIndexer, get_language_for_ext, index_file};
use codestory_store::Store as Storage;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

struct SnapshotCase {
    /// Registry language name, used only for failure messages.
    language: &'static str,
    /// Extension routed through the language registry.
    extension: &'static str,
    fidelity_filename: &'static str,
    fidelity_source: &'static str,
    fidelity_golden: &'static str,
    tictactoe_filename: &'static str,
    tictactoe_source: &'static str,
    tictactoe_golden: &'static str,
}

const CASES: &[SnapshotCase] = &[
    SnapshotCase {
        language: "kotlin",
        extension: "kt",
        fidelity_filename: "fidelity.kt",
        fidelity_source: include_str!("fixtures/fidelity_lab/kotlin_fidelity_lab.kt"),
        fidelity_golden: include_str!("fixtures/language_snapshots/kotlin_fidelity.txt"),
        tictactoe_filename: "game.kt",
        tictactoe_source: include_str!("fixtures/tictactoe/kotlin_tictactoe.kt"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/kotlin_tictactoe.txt"),
    },
    SnapshotCase {
        language: "java",
        extension: "java",
        fidelity_filename: "fidelity.java",
        fidelity_source: include_str!("fixtures/fidelity_lab/java_fidelity_lab.java"),
        fidelity_golden: include_str!("fixtures/language_snapshots/java_fidelity.txt"),
        tictactoe_filename: "game.java",
        tictactoe_source: include_str!("fixtures/tictactoe/java_tictactoe.java"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/java_tictactoe.txt"),
    },
    SnapshotCase {
        language: "cpp",
        extension: "cpp",
        fidelity_filename: "fidelity.cpp",
        fidelity_source: include_str!("fixtures/fidelity_lab/cpp_fidelity_lab.cpp"),
        fidelity_golden: include_str!("fixtures/language_snapshots/cpp_fidelity.txt"),
        tictactoe_filename: "game.cpp",
        tictactoe_source: include_str!("fixtures/tictactoe/cpp_tictactoe.cpp"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/cpp_tictactoe.txt"),
    },
    SnapshotCase {
        language: "c",
        extension: "c",
        fidelity_filename: "fidelity.c",
        fidelity_source: include_str!("fixtures/fidelity_lab/c_fidelity_lab.c"),
        fidelity_golden: include_str!("fixtures/language_snapshots/c_fidelity.txt"),
        tictactoe_filename: "game.c",
        tictactoe_source: include_str!("fixtures/tictactoe/c_tictactoe.c"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/c_tictactoe.txt"),
    },
    SnapshotCase {
        language: "javascript",
        extension: "js",
        fidelity_filename: "fidelity.js",
        fidelity_source: include_str!("fixtures/fidelity_lab/javascript_fidelity_lab.js"),
        fidelity_golden: include_str!("fixtures/language_snapshots/javascript_fidelity.txt"),
        tictactoe_filename: "game.js",
        tictactoe_source: include_str!("fixtures/tictactoe/javascript_tictactoe.js"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/javascript_tictactoe.txt"),
    },
    SnapshotCase {
        language: "typescript",
        extension: "ts",
        fidelity_filename: "fidelity.ts",
        fidelity_source: include_str!("fixtures/fidelity_lab/typescript_fidelity_lab.ts"),
        fidelity_golden: include_str!("fixtures/language_snapshots/typescript_fidelity.txt"),
        tictactoe_filename: "game.ts",
        tictactoe_source: include_str!("fixtures/tictactoe/typescript_tictactoe.ts"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/typescript_tictactoe.txt"),
    },
    // TSX is the one row whose fixtures are new rather than reused: the
    // `fidelity_lab` and `tictactoe` corpora only carried a `.ts` file, and a
    // `.ts` fixture proves nothing about the TSX grammar, the `tsx.graph.scm`
    // rule file, or the JSX usage edges that only a `.tsx` extension turns on.
    SnapshotCase {
        language: "tsx",
        extension: "tsx",
        fidelity_filename: "fidelity.tsx",
        fidelity_source: include_str!("fixtures/fidelity_lab/tsx_fidelity_lab.tsx"),
        fidelity_golden: include_str!("fixtures/language_snapshots/tsx_fidelity.txt"),
        tictactoe_filename: "game.tsx",
        tictactoe_source: include_str!("fixtures/tictactoe/tsx_tictactoe.tsx"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/tsx_tictactoe.txt"),
    },
    SnapshotCase {
        language: "python",
        extension: "py",
        fidelity_filename: "fidelity.py",
        fidelity_source: include_str!("fixtures/fidelity_lab/python_fidelity_lab.py"),
        fidelity_golden: include_str!("fixtures/language_snapshots/python_fidelity.txt"),
        tictactoe_filename: "game.py",
        tictactoe_source: include_str!("fixtures/tictactoe/python_tictactoe.py"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/python_tictactoe.txt"),
    },
    SnapshotCase {
        language: "rust",
        extension: "rs",
        fidelity_filename: "fidelity.rs",
        fidelity_source: include_str!("fixtures/fidelity_lab/rust_fidelity_lab.rs"),
        fidelity_golden: include_str!("fixtures/language_snapshots/rust_fidelity.txt"),
        tictactoe_filename: "game.rs",
        tictactoe_source: include_str!("fixtures/tictactoe/rust_tictactoe.rs"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/rust_tictactoe.txt"),
    },
    SnapshotCase {
        language: "go",
        extension: "go",
        fidelity_filename: "fidelity.go",
        fidelity_source: include_str!("fixtures/fidelity_lab/go_fidelity_lab.go"),
        fidelity_golden: include_str!("fixtures/language_snapshots/go_fidelity.txt"),
        tictactoe_filename: "game.go",
        tictactoe_source: include_str!("fixtures/tictactoe/go_tictactoe.go"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/go_tictactoe.txt"),
    },
    SnapshotCase {
        language: "ruby",
        extension: "rb",
        fidelity_filename: "fidelity.rb",
        fidelity_source: include_str!("fixtures/fidelity_lab/ruby_fidelity_lab.rb"),
        fidelity_golden: include_str!("fixtures/language_snapshots/ruby_fidelity.txt"),
        tictactoe_filename: "game.rb",
        tictactoe_source: include_str!("fixtures/tictactoe/ruby_tictactoe.rb"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/ruby_tictactoe.txt"),
    },
    SnapshotCase {
        language: "php",
        extension: "php",
        fidelity_filename: "fidelity.php",
        fidelity_source: include_str!("fixtures/fidelity_lab/php_fidelity_lab.php"),
        fidelity_golden: include_str!("fixtures/language_snapshots/php_fidelity.txt"),
        tictactoe_filename: "game.php",
        tictactoe_source: include_str!("fixtures/tictactoe/php_tictactoe.php"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/php_tictactoe.txt"),
    },
    SnapshotCase {
        language: "csharp",
        extension: "cs",
        fidelity_filename: "fidelity.cs",
        fidelity_source: include_str!("fixtures/fidelity_lab/csharp_fidelity_lab.cs"),
        fidelity_golden: include_str!("fixtures/language_snapshots/csharp_fidelity.txt"),
        tictactoe_filename: "game.cs",
        tictactoe_source: include_str!("fixtures/tictactoe/csharp_tictactoe.cs"),
        tictactoe_golden: include_str!("fixtures/language_snapshots/csharp_tictactoe.txt"),
    },
];

#[test]
fn moved_language_rules_keep_the_fidelity_lab_projection_byte_identical() -> Result<()> {
    for case in CASES {
        let (nodes, edges, root) =
            index_through_store(case.fidelity_filename, case.fidelity_source)?;
        let rendered = render_resolved_projection(&nodes, &edges, &root);
        assert_eq!(
            rendered.trim_end(),
            case.fidelity_golden.trim_end(),
            "{} fidelity_lab projection changed; the S3 move must be output-equal",
            case.language
        );
    }
    Ok(())
}

#[test]
fn moved_language_rules_keep_the_tictactoe_projection_byte_identical() -> Result<()> {
    for case in CASES {
        let result = index_raw(
            case.extension,
            case.tictactoe_filename,
            case.tictactoe_source,
        )?;
        let rendered = render_index_result(&result);
        assert_eq!(
            rendered.trim_end(),
            case.tictactoe_golden.trim_end(),
            "{} tictactoe projection changed; the S3 move must be output-equal",
            case.language
        );
    }
    Ok(())
}

/// The golden files must actually describe the fixture, not an empty run.
///
/// A snapshot harness whose fixture silently stopped producing a graph would
/// otherwise keep comparing two empty strings forever.
#[test]
fn snapshot_goldens_are_non_trivial() {
    for case in CASES {
        let fidelity_edges = case
            .fidelity_golden
            .lines()
            .filter(|line| line.starts_with("edge "))
            .count();
        let tictactoe_edges = case
            .tictactoe_golden
            .lines()
            .filter(|line| line.starts_with("edge "))
            .count();
        assert!(
            case.fidelity_golden
                .lines()
                .filter(|line| line.starts_with("node "))
                .count()
                >= 10
                && fidelity_edges >= 5,
            "{} fidelity golden is too small to prove anything",
            case.language
        );
        assert!(
            case.tictactoe_golden
                .lines()
                .filter(|line| line.starts_with("node "))
                .count()
                >= 15
                && tictactoe_edges >= 12,
            "{} tictactoe golden is too small to prove anything",
            case.language
        );
    }
}

/// Index one file through the full store pipeline.
///
/// The third element is the temporary root, which appears inside canonical ids
/// and file-node names and must be normalized out before comparison.
fn index_through_store(filename: &str, contents: &str) -> Result<(Vec<Node>, Vec<Edge>, String)> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join(filename);
    fs::write(&file_path, contents)?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![file_path],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let errors = storage.get_errors(None)?;
    anyhow::ensure!(
        errors.is_empty(),
        "indexing errors for `{filename}`: {errors:?}"
    );

    Ok((
        storage.get_nodes()?,
        storage.get_edges()?,
        root.to_string_lossy().replace('\\', "/"),
    ))
}

fn index_raw(extension: &str, filename: &str, source: &str) -> Result<IndexResult> {
    let language_config = get_language_for_ext(extension)
        .ok_or_else(|| anyhow!("no language mapping for extension `{extension}`"))?;
    index_file(Path::new(filename), source, &language_config, None, None)
        .map_err(|error| anyhow!("indexing failed for `{filename}`: {error}"))
}

/// Node identity independent of assignment order.
///
/// Snapshotting raw `NodeId`s would make the golden depend on hash-map
/// iteration order rather than on the extraction rules.
fn node_label(nodes: &HashMap<NodeId, &Node>, id: NodeId) -> String {
    match nodes.get(&id) {
        Some(node) => format!(
            "{:?}:{}@{}",
            node.kind,
            node.serialized_name,
            node.start_line.unwrap_or(0)
        ),
        None => format!("<missing:{}>", id.0),
    }
}

/// Replace the run-specific temporary root so the golden is stable, then sort.
///
/// Normalization has to happen before the sort: the temporary directory name is
/// random, and sorting raw lines would let it decide the order of any two lines
/// that only differ inside a path.
fn emit_sorted(lines: Vec<String>, temp_root: &str, out: &mut String) {
    let mut lines = lines
        .into_iter()
        .map(|line| {
            let line = line.replace('\\', "/");
            if temp_root.is_empty() {
                line
            } else {
                line.replace(temp_root, "<ROOT>")
            }
        })
        .collect::<Vec<_>>();
    lines.sort();
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
}

fn render_nodes(nodes: &[Node], temp_root: &str, out: &mut String) {
    let by_id: HashMap<NodeId, &Node> = nodes.iter().map(|node| (node.id, node)).collect();
    let lines = nodes
        .iter()
        .map(|node| {
            format!(
                "node {:?} name={} qn={} canonical={} line={} col={} end={}:{} file={}",
                node.kind,
                node.serialized_name,
                node.qualified_name.as_deref().unwrap_or("-"),
                node.canonical_id.as_deref().unwrap_or("-"),
                node.start_line.unwrap_or(0),
                node.start_col.unwrap_or(0),
                node.end_line.unwrap_or(0),
                node.end_col.unwrap_or(0),
                node.file_node_id
                    .map(|id| node_label(&by_id, id))
                    .unwrap_or_else(|| "-".to_string()),
            )
        })
        .collect::<Vec<_>>();
    emit_sorted(lines, temp_root, out);
}

fn render_edges(nodes: &[Node], edges: &[Edge], temp_root: &str, out: &mut String) {
    let by_id: HashMap<NodeId, &Node> = nodes.iter().map(|node| (node.id, node)).collect();
    let lines = edges
        .iter()
        .map(|edge| {
            format!(
                "edge {:?} src={} dst={} resolved_src={} resolved_dst={} line={} confidence={} certainty={} callsite={} candidates=[{}]",
                edge.kind,
                node_label(&by_id, edge.source),
                node_label(&by_id, edge.target),
                edge.resolved_source
                    .map(|id| node_label(&by_id, id))
                    .unwrap_or_else(|| "-".to_string()),
                edge.resolved_target
                    .map(|id| node_label(&by_id, id))
                    .unwrap_or_else(|| "-".to_string()),
                edge.line.unwrap_or(0),
                edge.confidence
                    .map(|value| format!("{value:.4}"))
                    .unwrap_or_else(|| "-".to_string()),
                edge.certainty
                    .map(|value| format!("{value:?}"))
                    .unwrap_or_else(|| "-".to_string()),
                edge.callsite_identity.as_deref().unwrap_or("-"),
                {
                    let mut candidates = edge
                        .candidate_targets
                        .iter()
                        .map(|id| node_label(&by_id, *id))
                        .collect::<Vec<_>>();
                    candidates.sort();
                    candidates.join(",")
                },
            )
        })
        .collect::<Vec<_>>();
    let mut rendered = String::new();
    emit_sorted(lines, temp_root, &mut rendered);
    out.push_str(&normalize_callsite_hashes(&rendered));
}

/// Replace the 64-bit hash components of `callsite_identity` with dense tokens.
///
/// The identity is `<file-hash>:<line>:<col>:<callsite-hash>` and both hashes
/// are derived from the file's *absolute* path, which is a fresh temporary
/// directory on every run. Masking the raw values keeps the golden stable while
/// preserving everything the contract cares about: how many distinct callsite
/// identities exist, which edges share one (a collision would show up as a
/// repeated token), and the line and column they carry.
fn normalize_callsite_hashes(rendered: &str) -> String {
    let mut assigned: Vec<String> = Vec::new();
    let mut out = String::new();
    for line in rendered.lines() {
        let Some((head, rest)) = line.split_once("callsite=") else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let (identity, tail) = rest.split_once(' ').unwrap_or((rest, ""));
        let masked = identity
            .split(':')
            .map(|component| {
                // A syntax marker rides the last component as `<hash>|syntax:...`;
                // mask the hash and keep the marker verbatim, because the marker
                // is exactly the per-language fact this snapshot must pin.
                let (number, suffix) = match component.split_once('|') {
                    Some((number, suffix)) => (number, Some(suffix)),
                    None => (component, None),
                };
                let masked = match number.parse::<i64>() {
                    Ok(value) if value.abs() >= 1_000_000_000 => {
                        let index = match assigned.iter().position(|seen| seen == number) {
                            Some(index) => index,
                            None => {
                                assigned.push(number.to_string());
                                assigned.len() - 1
                            }
                        };
                        format!("h{index}")
                    }
                    _ => number.to_string(),
                };
                match suffix {
                    Some(suffix) => format!("{masked}|{suffix}"),
                    None => masked,
                }
            })
            .collect::<Vec<_>>()
            .join(":");
        out.push_str(head);
        out.push_str("callsite=");
        out.push_str(&masked);
        if !tail.is_empty() {
            out.push(' ');
            out.push_str(tail);
        }
        out.push('\n');
    }
    out
}

fn render_resolved_projection(nodes: &[Node], edges: &[Edge], temp_root: &str) -> String {
    let mut out = String::new();
    render_nodes(nodes, temp_root, &mut out);
    render_edges(nodes, edges, temp_root, &mut out);
    out
}

fn render_index_result(result: &IndexResult) -> String {
    let mut out = String::new();
    render_nodes(&result.nodes, "", &mut out);
    render_edges(&result.nodes, &result.edges, "", &mut out);

    let by_id: HashMap<NodeId, &Node> = result.nodes.iter().map(|node| (node.id, node)).collect();

    let files = result
        .files
        .iter()
        .map(|file| {
            format!(
                "file path={} language={} lines={} complete={} role={:?}",
                file.path.to_string_lossy().replace('\\', "/"),
                file.language,
                file.line_count,
                file.complete,
                file.file_role,
            )
        })
        .collect::<Vec<_>>();
    emit_sorted(files, "", &mut out);

    let occurrences = result
        .occurrences
        .iter()
        .map(|occurrence| {
            format!(
                "occurrence element={} kind={:?} span={}:{}-{}:{}",
                node_label(&by_id, NodeId(occurrence.element_id)),
                occurrence.kind,
                occurrence.location.start_line,
                occurrence.location.start_col,
                occurrence.location.end_line,
                occurrence.location.end_col,
            )
        })
        .collect::<Vec<_>>();
    emit_sorted(occurrences, "", &mut out);

    let access = result
        .component_access
        .iter()
        .map(|(id, access): &(NodeId, AccessKind)| {
            format!("access node={} kind={access:?}", node_label(&by_id, *id))
        })
        .collect::<Vec<_>>();
    emit_sorted(access, "", &mut out);

    let anchors = result
        .impl_anchor_node_ids
        .iter()
        .map(|id| format!("impl_anchor node={}", node_label(&by_id, *id)))
        .collect::<Vec<_>>();
    emit_sorted(anchors, "", &mut out);

    let states = result
        .callable_projection_states
        .iter()
        .map(|state| {
            let mut line = String::new();
            let _ = write!(line, "callable_state {state:?}");
            line
        })
        .collect::<Vec<_>>();
    emit_sorted(states, "", &mut out);

    out
}
