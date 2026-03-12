use anyhow::{Context, Result};
use codestory_contracts::api::{
    GroundingSnapshotDto, NodeDetailsDto, SearchHit, SnippetContextDto, SymbolContextDto,
    TrailContextDto,
};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::IndexOutput;
use crate::args::{CliTrailMode, OutputFormat, TrailCommand};
use crate::display::{
    clean_path_string, default_trail_direction, format_budget, format_direction, format_kind,
    format_trail_mode, relative_path,
};
use crate::runtime::ResolvedTarget;

pub(crate) fn emit<T: Serialize>(format: OutputFormat, value: &T, markdown: String) -> Result<()> {
    match format {
        OutputFormat::Markdown => {
            println!("{markdown}");
            Ok(())
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).context("Failed to serialize JSON output")?
            );
            Ok(())
        }
    }
}

pub(crate) fn render_index_markdown(output: &IndexOutput<'_>) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Index");
    let _ = writeln!(markdown, "project: `{}`", clean_path_string(output.project));
    let _ = writeln!(
        markdown,
        "storage: `{}`",
        clean_path_string(output.storage_path)
    );
    let _ = writeln!(markdown, "refresh: `{}`", output.refresh);
    let _ = writeln!(
        markdown,
        "stats: nodes={} edges={} files={} errors={}",
        output.summary.stats.node_count,
        output.summary.stats.edge_count,
        output.summary.stats.file_count,
        output.summary.stats.error_count
    );
    if let Some(timings) = output.phase_timings {
        let _ = writeln!(
            markdown,
            "timings_ms: parse={} flush={} resolve={} cleanup={} cache_refresh={}",
            timings.parse_index_ms,
            timings.projection_flush_ms,
            timings.edge_resolution_ms,
            timings.cleanup_ms,
            timings.cache_refresh_ms.unwrap_or(0)
        );
        let _ = writeln!(
            markdown,
            "resolution: calls {}->{}, imports {}->{}",
            timings.unresolved_calls_start,
            timings.unresolved_calls_end,
            timings.unresolved_imports_start,
            timings.unresolved_imports_end
        );
        append_optional_timings_line(
            &mut markdown,
            "staged_publish_ms",
            &[
                ("deferred_indexes", timings.deferred_indexes_ms),
                ("summary_snapshot", timings.summary_snapshot_ms),
                ("detail_snapshot", timings.detail_snapshot_ms),
                ("publish", timings.publish_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "setup_ms",
            &[
                (
                    "existing_projection_ids",
                    timings.setup_existing_projection_ids_ms,
                ),
                ("seed_symbol_table", timings.setup_seed_symbol_table_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "flush_breakdown_ms",
            &[
                ("files", timings.flush_files_ms),
                ("nodes", timings.flush_nodes_ms),
                ("edges", timings.flush_edges_ms),
                ("occurrences", timings.flush_occurrences_ms),
                ("component_access", timings.flush_component_access_ms),
                ("callable_projection", timings.flush_callable_projection_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_ms",
            &[
                ("override_count", timings.resolution_override_count_ms),
                ("unresolved_counts", timings.resolution_unresolved_counts_ms),
                ("calls", timings.resolution_calls_ms),
                ("imports", timings.resolution_imports_ms),
                ("cleanup", timings.resolution_cleanup_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_indexes_ms",
            &[
                ("call_candidate", timings.resolution_call_candidate_index_ms),
                (
                    "import_candidate",
                    timings.resolution_import_candidate_index_ms,
                ),
                ("call_semantic", timings.resolution_call_semantic_index_ms),
                (
                    "import_semantic",
                    timings.resolution_import_semantic_index_ms,
                ),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_detail_ms",
            &[
                (
                    "call_semantic_candidates",
                    timings.resolution_call_semantic_candidates_ms,
                ),
                (
                    "import_semantic_candidates",
                    timings.resolution_import_semantic_candidates_ms,
                ),
                ("call_compute", timings.resolution_call_compute_ms),
                ("import_compute", timings.resolution_import_compute_ms),
                ("call_apply", timings.resolution_call_apply_ms),
                ("import_apply", timings.resolution_import_apply_ms),
                ("overrides", timings.resolution_override_resolution_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "resolution_semantic_requests",
            &[
                ("call_rows", timings.resolution_call_semantic_requests),
                (
                    "call_unique",
                    timings.resolution_call_semantic_unique_requests,
                ),
                (
                    "call_skipped",
                    timings.resolution_call_semantic_skipped_requests,
                ),
                ("import_rows", timings.resolution_import_semantic_requests),
                (
                    "import_unique",
                    timings.resolution_import_semantic_unique_requests,
                ),
                (
                    "import_skipped",
                    timings.resolution_import_semantic_skipped_requests,
                ),
            ],
        );
    }
    markdown
}

fn append_optional_timings_line(
    markdown: &mut String,
    label: &str,
    entries: &[(&str, Option<u32>)],
) {
    let rendered = entries
        .iter()
        .filter_map(|(name, value)| value.map(|value| format!("{name}={value}")))
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        return;
    }
    let _ = writeln!(markdown, "{label}: {}", rendered.join(" "));
}

pub(crate) fn render_ground_markdown(
    project_root: &Path,
    snapshot: &GroundingSnapshotDto,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Grounding Snapshot");
    let _ = writeln!(markdown, "root: `{}`", clean_path_string(&snapshot.root));
    let _ = writeln!(markdown, "budget: `{}`", format_budget(snapshot.budget));
    let _ = writeln!(
        markdown,
        "coverage: files {}/{} symbols {}/{} compressed_files={}",
        snapshot.coverage.represented_files,
        snapshot.coverage.total_files,
        snapshot.coverage.represented_symbols,
        snapshot.coverage.total_symbols,
        snapshot.coverage.compressed_files
    );
    let _ = writeln!(
        markdown,
        "stats: nodes={} edges={} files={} errors={}",
        snapshot.stats.node_count,
        snapshot.stats.edge_count,
        snapshot.stats.file_count,
        snapshot.stats.error_count
    );
    if !snapshot.recommended_queries.is_empty() {
        let _ = writeln!(
            markdown,
            "recommended_queries: {}",
            snapshot.recommended_queries.join(", ")
        );
    }
    if !snapshot.notes.is_empty() {
        let _ = writeln!(markdown, "notes:");
        for note in &snapshot.notes {
            let _ = writeln!(markdown, "- {note}");
        }
    }
    let _ = writeln!(markdown, "root_symbols:");
    for symbol in &snapshot.root_symbols {
        let _ = writeln!(markdown, "- {}", render_ground_symbol(symbol));
    }
    let _ = writeln!(markdown, "files:");
    for file in &snapshot.files {
        let language = file.language.as_deref().unwrap_or("unknown");
        let status = if file.compressed {
            "compressed"
        } else {
            "full"
        };
        let focus = if file.symbols.is_empty() {
            "no indexed symbols".to_string()
        } else {
            file.symbols
                .iter()
                .map(render_ground_symbol)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let _ = writeln!(
            markdown,
            "- `{}` [{}] symbols {}/{} {} | {}",
            relative_path(project_root, &file.file_path),
            language,
            file.represented_symbol_count,
            file.symbol_count,
            status,
            focus
        );
    }
    if !snapshot.coverage_buckets.is_empty() {
        let _ = writeln!(markdown, "coverage_buckets:");
        for bucket in &snapshot.coverage_buckets {
            let sample_paths = if bucket.sample_paths.is_empty() {
                "no sample paths".to_string()
            } else {
                bucket.sample_paths.join(", ")
            };
            let _ = writeln!(
                markdown,
                "- `{}` files={} symbols={} samples={}",
                bucket.label, bucket.file_count, bucket.symbol_count, sample_paths
            );
        }
    }
    markdown
}

pub(crate) fn render_search_markdown(
    project_root: &Path,
    query: &str,
    hits: &[SearchHit],
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Search");
    let _ = writeln!(markdown, "query: `{query}`");
    let _ = writeln!(markdown, "hits: {}", hits.len());
    for hit in hits {
        let _ = writeln!(markdown, "- {}", render_search_hit(project_root, hit));
    }
    markdown
}

pub(crate) fn render_symbol_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &SymbolContextDto,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Symbol");
    append_resolution(&mut markdown, project_root, target);
    let _ = writeln!(
        markdown,
        "focus: {}",
        render_node(project_root, &context.node)
    );
    let _ = writeln!(markdown, "children: {}", context.children.len());
    for child in &context.children {
        let _ = writeln!(
            markdown,
            "- [{}] {} [{}]{}",
            child.id.0,
            child.label,
            format_kind(child.kind),
            if child.has_children { " children" } else { "" }
        );
    }
    if !context.edge_digest.is_empty() {
        let _ = writeln!(markdown, "edge_digest:");
        for edge in &context.edge_digest {
            let _ = writeln!(markdown, "- {edge}");
        }
    }
    if !context.related_hits.is_empty() {
        let _ = writeln!(markdown, "related_hits:");
        for hit in &context.related_hits {
            let _ = writeln!(markdown, "- {}", render_search_hit(project_root, hit));
        }
    }
    markdown
}

pub(crate) fn render_trail_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &TrailContextDto,
    cmd: &TrailCommand,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Trail");
    append_resolution(&mut markdown, project_root, target);
    let _ = writeln!(
        markdown,
        "focus: {}",
        render_node(project_root, &context.focus)
    );
    let _ = writeln!(
        markdown,
        "mode: {} direction: {} depth: {} nodes: {} edges: {} truncated: {}",
        format_trail_mode(cmd.mode),
        format_direction(
            cmd.direction
                .map(Into::into)
                .unwrap_or_else(|| default_trail_direction(cmd.mode))
        ),
        cmd.depth.unwrap_or(match cmd.mode {
            CliTrailMode::Neighborhood => 2,
            CliTrailMode::Referenced | CliTrailMode::Referencing => 0,
        }),
        context.trail.nodes.len(),
        context.trail.edges.len(),
        context.trail.truncated
    );

    let labels = context
        .trail
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.label.clone()))
        .collect::<HashMap<_, _>>();

    let _ = writeln!(markdown, "nodes:");
    for node in &context.trail.nodes {
        let file = node
            .file_path
            .as_deref()
            .map(|value| relative_path(project_root, value))
            .unwrap_or_else(|| "-".to_string());
        let _ = writeln!(
            markdown,
            "- [{}] {} [{}] depth={} file={}",
            node.id.0,
            node.label,
            format_kind(node.kind),
            node.depth,
            file
        );
    }

    let _ = writeln!(markdown, "edges:");
    for edge in &context.trail.edges {
        let source = labels
            .get(&edge.source)
            .map(String::as_str)
            .unwrap_or(&edge.source.0);
        let target = labels
            .get(&edge.target)
            .map(String::as_str)
            .unwrap_or(&edge.target.0);
        let certainty = edge
            .certainty
            .as_deref()
            .map(|value| format!(" certainty={value}"))
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- [{}] {} -{}-> {}{}",
            edge.id.0,
            source,
            format!("{:?}", edge.kind).to_lowercase(),
            target,
            certainty
        );
    }
    markdown
}

pub(crate) fn render_snippet_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &SnippetContextDto,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Snippet");
    append_resolution(&mut markdown, project_root, target);
    let _ = writeln!(
        markdown,
        "focus: {}",
        render_node(project_root, &context.node)
    );
    let _ = writeln!(
        markdown,
        "path: `{}`:{}",
        relative_path(project_root, &context.path),
        context.line
    );
    let _ = writeln!(markdown, "{}", context.snippet);
    markdown
}

fn append_resolution(markdown: &mut String, project_root: &Path, target: &ResolvedTarget) {
    if target.requested.starts_with("id:") {
        return;
    }
    let _ = writeln!(
        markdown,
        "resolved_query: `{}` -> {}",
        target.requested,
        render_search_hit(project_root, &target.selected)
    );
    if target.alternatives.len() > 1 {
        let alternatives = target
            .alternatives
            .iter()
            .skip(1)
            .take(3)
            .map(|hit| render_search_hit(project_root, hit))
            .collect::<Vec<_>>();
        if !alternatives.is_empty() {
            let _ = writeln!(markdown, "alternate_hits:");
            for hit in alternatives {
                let _ = writeln!(markdown, "- {hit}");
            }
        }
    }
}

fn render_node(project_root: &Path, node: &NodeDetailsDto) -> String {
    let mut out = format!(
        "[{}] {} [{}]",
        node.id.0,
        node.display_name,
        format_kind(node.kind)
    );
    if let Some(path) = node.file_path.as_deref() {
        let _ = write!(out, " {}", relative_path(project_root, path));
    }
    if let Some(line) = node.start_line {
        let _ = write!(out, ":{line}");
    }
    out
}

fn render_search_hit(project_root: &Path, hit: &SearchHit) -> String {
    let mut out = format!(
        "[{}] {} [{}]",
        hit.node_id.0,
        hit.display_name,
        format_kind(hit.kind)
    );
    if let Some(path) = hit.file_path.as_deref() {
        let _ = write!(out, " {}", relative_path(project_root, path));
    }
    if let Some(line) = hit.line {
        let _ = write!(out, ":{line}");
    }
    let _ = write!(out, " score={:.2}", hit.score);
    out
}

fn render_ground_symbol(symbol: &codestory_contracts::api::GroundingSymbolDigestDto) -> String {
    let mut out = format!(
        "[{}] {} [{}]",
        symbol.id.0,
        symbol.label,
        format_kind(symbol.kind)
    );
    if let Some(line) = symbol.line {
        let _ = write!(out, " line={line}");
    }
    if let Some(member_count) = symbol.member_count {
        let _ = write!(out, " members={member_count}");
    }
    if !symbol.edge_digest.is_empty() {
        let _ = write!(out, " edges={}", symbol.edge_digest.join("; "));
    }
    out
}
