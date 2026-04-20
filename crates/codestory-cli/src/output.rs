use anyhow::{Context, Result, bail};
use codestory_contracts::api::{
    AgentAnswerDto, AgentResponseBlockDto, GraphArtifactDto, GroundingSnapshotDto, NodeDetailsDto,
    RetrievalFallbackReasonDto, RetrievalModeDto, RetrievalStateDto, SearchHit, SnippetContextDto,
    SymbolContextDto, TrailContextDto,
};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::args::{
    CliTrailMode, DoctorOutput, IndexDryRunOutput, IndexOutput, OutputFormat, QueryOutput,
    SearchOutput, TrailCommand,
};
use crate::display::{
    clean_path_string, default_trail_direction, format_budget, format_direction, format_kind,
    format_trail_mode, relative_path,
};
use crate::runtime::ResolvedTarget;

pub(crate) fn emit<T: Serialize>(
    format: OutputFormat,
    value: &T,
    markdown: String,
    output_file: Option<&Path>,
) -> Result<()> {
    let content = render_output_content(format, value, &markdown)?;
    if let Some(path) = output_file {
        write_output_file(path, &content)?;
    } else {
        print!("{content}");
    }
    Ok(())
}

pub(crate) fn emit_text(content: String, output_file: Option<&Path>) -> Result<()> {
    let mut content = content;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    if let Some(path) = output_file {
        write_output_file(path, &content)?;
    } else {
        print!("{content}");
    }
    Ok(())
}

fn render_output_content<T: Serialize>(
    format: OutputFormat,
    value: &T,
    markdown: &str,
) -> Result<String> {
    let mut content = match format {
        OutputFormat::Markdown => markdown.to_string(),
        OutputFormat::Json => {
            serde_json::to_string_pretty(value).context("Failed to serialize JSON output")?
        }
        OutputFormat::Dot => bail!("--format dot is only supported by `trail`"),
    };
    if !content.ends_with('\n') {
        content.push('\n');
    }
    Ok(content)
}

fn write_output_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        && !parent.exists()
    {
        bail!(
            "Output parent directory does not exist: {}",
            clean_path_string(&parent.to_string_lossy())
        );
    }

    let file = File::create(path).with_context(|| {
        format!(
            "Failed to create output file {}",
            clean_path_string(&path.to_string_lossy())
        )
    })?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(content.as_bytes())
        .with_context(|| format!("Failed to write output file {}", path.display()))?;
    writer
        .flush()
        .with_context(|| format!("Failed to flush output file {}", path.display()))?;
    Ok(())
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
    if !output.summary.members.is_empty() {
        let _ = writeln!(markdown, "members:");
        for member in &output.summary.members {
            let _ = writeln!(
                markdown,
                "- `{}` files={} nodes={} edges={}",
                clean_path_string(&member.path),
                member.file_count.unwrap_or(member.indexed_files),
                member.node_count.unwrap_or(0),
                member.edge_count.unwrap_or(0)
            );
        }
    }
    let _ = writeln!(
        markdown,
        "retrieval: {}",
        render_retrieval_state(output.retrieval)
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
            "semantic_ms",
            &[
                ("doc_build", timings.semantic_doc_build_ms),
                ("embedding", timings.semantic_embedding_ms),
                ("db_upsert", timings.semantic_db_upsert_ms),
                ("reload", timings.semantic_reload_ms),
            ],
        );
        append_optional_timings_line(
            &mut markdown,
            "semantic_docs",
            &[
                ("reused", timings.semantic_docs_reused),
                ("embedded", timings.semantic_docs_embedded),
                ("pending", timings.semantic_docs_pending),
                ("stale", timings.semantic_docs_stale),
            ],
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
    if let Some(summary) = output.summary_generation {
        let _ = writeln!(
            markdown,
            "summaries: generated={} reused={} skipped={} endpoint={}",
            summary.generated, summary.reused, summary.skipped, summary.endpoint
        );
    }
    if !output.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    markdown
}

pub(crate) fn render_index_dry_run_markdown(output: &IndexDryRunOutput<'_>) -> String {
    let dry_run = output.dry_run;
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Index Dry Run");
    let _ = writeln!(markdown, "project: `{}`", clean_path_string(&dry_run.root));
    let _ = writeln!(
        markdown,
        "storage: `{}`",
        clean_path_string(&dry_run.storage_path)
    );
    let _ = writeln!(markdown, "refresh: `{:?}`", dry_run.refresh);
    let _ = writeln!(
        markdown,
        "plan: would index {} files, remove {} files",
        dry_run.files_to_index, dry_run.files_to_remove
    );
    if !dry_run.members.is_empty() {
        let _ = writeln!(markdown, "members:");
        for member in &dry_run.members {
            let _ = write!(
                markdown,
                "- `{}` files_to_index={} indexed_files={}",
                clean_path_string(&member.path),
                member.files_to_index,
                member.indexed_files
            );
            if member.file_count.is_some()
                || member.node_count.is_some()
                || member.edge_count.is_some()
            {
                let _ = write!(
                    markdown,
                    " files={} nodes={} edges={}",
                    member.file_count.unwrap_or(member.indexed_files),
                    member.node_count.unwrap_or(0),
                    member.edge_count.unwrap_or(0)
                );
            }
            let _ = writeln!(markdown);
        }
    }
    if !dry_run.sample_files_to_index.is_empty() {
        let _ = writeln!(markdown, "sample_files_to_index:");
        for path in &dry_run.sample_files_to_index {
            let _ = writeln!(markdown, "- `{}`", clean_path_string(path));
        }
    }
    if !dry_run.sample_file_ids_to_remove.is_empty() {
        let ids = dry_run
            .sample_file_ids_to_remove
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "sample_file_ids_to_remove: {ids}");
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
    explain: bool,
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
    if let Some(retrieval) = snapshot.retrieval.as_ref() {
        let _ = writeln!(markdown, "retrieval: {}", render_retrieval_state(retrieval));
    }
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
    if explain {
        let _ = writeln!(markdown, "why:");
        if let Some(retrieval) = snapshot.retrieval.as_ref() {
            let _ = writeln!(
                markdown,
                "- retrieval_mode: {}",
                render_retrieval_state(retrieval)
            );
        }
        let _ = writeln!(
            markdown,
            "- coverage: represented {} of {} files and {} of {} symbols",
            snapshot.coverage.represented_files,
            snapshot.coverage.total_files,
            snapshot.coverage.represented_symbols,
            snapshot.coverage.total_symbols
        );
        if !snapshot.recommended_queries.is_empty() {
            let _ = writeln!(
                markdown,
                "- query_hints: {}",
                snapshot.recommended_queries.join(", ")
            );
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

pub(crate) fn render_search_markdown(_project_root: &Path, output: &SearchOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Search");
    let _ = writeln!(markdown, "query: `{}`", output.query);
    let _ = writeln!(
        markdown,
        "retrieval: {}",
        render_retrieval_state(&output.retrieval)
    );
    let _ = writeln!(markdown, "limit_per_source: {}", output.limit_per_source);
    let _ = writeln!(
        markdown,
        "repo_text: {} ({})",
        match output.repo_text_mode {
            crate::args::RepoTextMode::Auto => "auto",
            crate::args::RepoTextMode::On => "on",
            crate::args::RepoTextMode::Off => "off",
        },
        if output.repo_text_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    if !output.suggestions.is_empty() {
        let _ = writeln!(markdown, "did_you_mean:");
        for hit in &output.suggestions {
            let _ = writeln!(markdown, "- {}", render_search_hit_output(hit));
        }
    }
    if output.explain && !output.query_hints.is_empty() {
        let _ = writeln!(markdown, "query_hints:");
        for hint in &output.query_hints {
            let _ = writeln!(markdown, "- {hint}");
        }
    }
    let _ = writeln!(
        markdown,
        "indexed_symbol_hits: {}",
        output.indexed_symbol_hits.len()
    );
    for hit in &output.indexed_symbol_hits {
        let _ = writeln!(markdown, "- {}", render_search_hit_output(hit));
        append_search_hit_why(&mut markdown, hit);
    }
    let _ = writeln!(markdown, "repo_text_hits: {}", output.repo_text_hits.len());
    for hit in &output.repo_text_hits {
        let _ = writeln!(markdown, "- {}", render_search_hit_output(hit));
        append_search_hit_why(&mut markdown, hit);
        if let Some(excerpt) = hit.excerpt.as_deref() {
            let _ = writeln!(markdown, "  excerpt: {}", excerpt);
        }
    }
    markdown
}

fn append_search_hit_why(markdown: &mut String, hit: &crate::args::SearchHitOutput) {
    if hit.why.is_empty() {
        return;
    }
    for why in &hit.why {
        let _ = writeln!(markdown, "  why: {why}");
    }
}

pub(crate) fn render_agent_answer_markdown(project_root: &Path, answer: &AgentAnswerDto) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Ask");
    let _ = writeln!(markdown, "prompt: `{}`", answer.prompt.replace('\n', " "));
    let _ = writeln!(markdown, "summary: {}", answer.summary);
    let _ = writeln!(
        markdown,
        "retrieval_version: `{}`",
        answer.retrieval_version
    );
    let _ = writeln!(
        markdown,
        "trace: profile={:?} policy={:?} latency_ms={} steps={}",
        answer.retrieval_trace.resolved_profile,
        answer.retrieval_trace.policy_mode,
        answer.retrieval_trace.total_latency_ms,
        answer.retrieval_trace.steps.len()
    );
    for section in &answer.sections {
        let _ = writeln!(markdown, "\n## {}", section.title);
        for block in &section.blocks {
            match block {
                AgentResponseBlockDto::Markdown { markdown: block } => {
                    markdown.push_str(block);
                    if !block.ends_with('\n') {
                        markdown.push('\n');
                    }
                }
                AgentResponseBlockDto::Mermaid { graph_id } => {
                    if let Some(GraphArtifactDto::Mermaid { mermaid_syntax, .. }) =
                        answer.graphs.iter().find(|graph| match graph {
                            GraphArtifactDto::Mermaid { id, .. } => id == graph_id,
                            GraphArtifactDto::Uml { .. } => false,
                        })
                    {
                        let _ = writeln!(markdown, "```mermaid");
                        markdown.push_str(mermaid_syntax);
                        if !mermaid_syntax.ends_with('\n') {
                            markdown.push('\n');
                        }
                        let _ = writeln!(markdown, "```");
                    }
                }
            }
        }
    }
    if !answer.citations.is_empty() {
        let _ = writeln!(markdown, "\n## Citations");
        for citation in &answer.citations {
            let file = citation
                .file_path
                .as_deref()
                .map(|path| relative_path(project_root, path))
                .unwrap_or_else(|| "-".to_string());
            let line = citation
                .line
                .map(|line| format!(":{line}"))
                .unwrap_or_default();
            let _ = write!(
                markdown,
                "- [{}] {} [{}] {}{} score={:.3}",
                citation.node_id.0,
                citation.display_name,
                format_kind(citation.kind),
                file,
                line,
                citation.score
            );
            if let Some(breakdown) = citation.retrieval_score_breakdown.as_ref() {
                let _ = write!(
                    markdown,
                    " why lexical={:.3} semantic={:.3} graph={:.3} total={:.3}",
                    breakdown.lexical, breakdown.semantic, breakdown.graph, breakdown.total
                );
            }
            let _ = writeln!(markdown);
        }
    }
    markdown
}

pub(crate) fn render_doctor_markdown(output: &DoctorOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Doctor");
    let _ = writeln!(markdown, "project: `{}`", output.project);
    let _ = writeln!(markdown, "storage: `{}`", output.storage_path);
    let _ = writeln!(
        markdown,
        "stats: nodes={} edges={} files={} errors={}",
        output.stats.node_count,
        output.stats.edge_count,
        output.stats.file_count,
        output.stats.error_count
    );
    if let Some(retrieval) = output.retrieval.as_ref() {
        let _ = writeln!(markdown, "retrieval: {}", render_retrieval_state(retrieval));
    }
    let _ = writeln!(markdown, "checks:");
    for check in &output.checks {
        let _ = writeln!(
            markdown,
            "- {} [{}]: {}",
            check.name, check.status, check.message
        );
    }
    let _ = writeln!(markdown, "environment:");
    for item in &output.environment {
        let _ = writeln!(
            markdown,
            "- {} [{}]: {}",
            item.name, item.status, item.message
        );
    }
    if !output.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
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
    if let Some(summary) = context.summary.as_deref() {
        let _ = writeln!(markdown, "summary: {summary}");
    }
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

pub(crate) fn render_query_markdown(output: &QueryOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Query");
    let _ = writeln!(markdown, "query: `{}`", output.query);
    let _ = writeln!(markdown, "items: {}", output.items.len());
    for item in &output.items {
        let mut line = format!(
            "- [{}] {} [{}]",
            item.node_id,
            item.display_name,
            format_kind(item.kind)
        );
        if let Some(path) = item.file_path.as_deref() {
            let _ = write!(line, " {path}");
        }
        if let Some(line_no) = item.line {
            let _ = write!(line, ":{line_no}");
        }
        if let Some(depth) = item.depth {
            let _ = write!(line, " depth={depth}");
        }
        if let Some(node_ref) = item.node_ref.as_deref() {
            let _ = write!(line, " ref=`{node_ref}`");
        }
        let _ = write!(line, " source={}", item.source);
        let _ = writeln!(markdown, "{line}");
    }
    markdown
}

pub(crate) fn render_symbol_mermaid(context: &SymbolContextDto) -> String {
    let mut mermaid = String::new();
    let _ = writeln!(mermaid, "flowchart LR");
    let root = mermaid_node_id(&context.node.id.0);
    let _ = writeln!(
        mermaid,
        "  {}[\"{}\\n[{}]\"]",
        root,
        escape_mermaid_label(&context.node.display_name),
        format_kind(context.node.kind)
    );
    for child in &context.children {
        let child_id = mermaid_node_id(&child.id.0);
        let _ = writeln!(
            mermaid,
            "  {}[\"{}\\n[{}]\"]",
            child_id,
            escape_mermaid_label(&child.label),
            format_kind(child.kind)
        );
        let _ = writeln!(mermaid, "  {} --> {}", root, child_id);
    }
    mermaid
}

pub(crate) fn render_trail_mermaid(context: &TrailContextDto) -> String {
    let mut mermaid = String::new();
    let _ = writeln!(mermaid, "flowchart LR");
    for node in &context.trail.nodes {
        let _ = writeln!(
            mermaid,
            "  {}[\"{}\\n[{}]\"]",
            mermaid_node_id(&node.id.0),
            escape_mermaid_label(&node.label),
            format_kind(node.kind)
        );
    }
    for edge in &context.trail.edges {
        let label = format!("{:?}", edge.kind).to_lowercase();
        let _ = writeln!(
            mermaid,
            "  {} -->|{}| {}",
            mermaid_node_id(&edge.source.0),
            escape_mermaid_label(&label),
            mermaid_node_id(&edge.target.0)
        );
    }
    mermaid
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
        let edge_kind = format!("{:?}", edge.kind).to_lowercase();
        let (connector, certainty) =
            render_trail_edge_notation(&edge_kind, edge.certainty.as_deref());
        let _ = writeln!(
            markdown,
            "- [{}] {} {} {}{}",
            edge.id.0, source, connector, target, certainty
        );
    }
    markdown
}

pub(crate) fn render_trail_dot(_project_root: &Path, context: &TrailContextDto) -> String {
    let mut dot = String::new();
    let _ = writeln!(dot, "digraph codestory_trail {{");
    let _ = writeln!(dot, "  rankdir=LR;");
    for node in &context.trail.nodes {
        let _ = writeln!(
            dot,
            "  \"{}\" [label=\"{}\\n[{}]\"];",
            escape_dot(&node.id.0),
            escape_dot(&node.label),
            format_kind(node.kind)
        );
    }
    for edge in &context.trail.edges {
        let _ = writeln!(
            dot,
            "  \"{}\" -> \"{}\" [label=\"{}\"];",
            escape_dot(&edge.source.0),
            escape_dot(&edge.target.0),
            escape_dot(&format!("{:?}", edge.kind).to_lowercase())
        );
    }
    let _ = writeln!(dot, "}}");
    dot
}

pub(crate) fn render_snippet_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &SnippetContextDto,
    colorize: bool,
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
    let fence = snippet_fence(&context.snippet);
    let _ = writeln!(markdown, "{fence}{}", snippet_language(&context.path));
    let snippet = if colorize {
        ansi_highlight_snippet(&context.path, &context.snippet)
    } else {
        context.snippet.clone()
    };
    let _ = writeln!(markdown, "{snippet}");
    let _ = writeln!(markdown, "{fence}");
    markdown
}

fn append_resolution(markdown: &mut String, project_root: &Path, target: &ResolvedTarget) {
    if matches!(target.selector, crate::args::QuerySelectorOutput::Id) {
        let _ = writeln!(
            markdown,
            "resolved_id: `{}` -> {}",
            target.requested,
            render_search_hit(project_root, &target.selected)
        );
        return;
    }
    if let Some(file_filter) = target.file_filter.as_deref() {
        let _ = writeln!(
            markdown,
            "file_filter: `{}`",
            clean_path_string(file_filter)
        );
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

fn render_retrieval_state(state: &RetrievalStateDto) -> String {
    let mode = match state.mode {
        RetrievalModeDto::Hybrid => "hybrid",
        RetrievalModeDto::Symbolic => "symbolic",
    };
    let mut out = format!("{mode} semantic_docs={}", state.semantic_doc_count);
    if let Some(model) = state.embedding_model.as_deref() {
        let _ = write!(out, " model={model}");
    }
    if let Some(reason) = state.fallback_reason {
        let reason = match reason {
            RetrievalFallbackReasonDto::DisabledByConfig => "disabled_by_config",
            RetrievalFallbackReasonDto::MissingEmbeddingRuntime => "missing_embedding_runtime",
            RetrievalFallbackReasonDto::MissingSemanticDocs => "missing_semantic_docs",
        };
        let _ = write!(out, " fallback={reason}");
    }
    if let Some(message) = state.fallback_message.as_deref() {
        let _ = write!(out, " note={}", message.replace('\n', " "));
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
    let _ = write!(out, " origin={}", hit.origin.as_str());
    if let Some(node_ref) = node_ref(
        project_root,
        hit.file_path.as_deref(),
        hit.line,
        &hit.display_name,
    ) {
        let _ = write!(out, " ref=`{node_ref}`");
    }
    out
}

fn render_search_hit_output(hit: &crate::args::SearchHitOutput) -> String {
    let mut out = format!(
        "[{}] {} [{}]",
        hit.node_id,
        hit.display_name,
        format_kind(hit.kind)
    );
    if let Some(path) = hit.file_path.as_deref() {
        let _ = write!(out, " {}", path);
    }
    if let Some(line) = hit.line {
        let _ = write!(out, ":{line}");
    }
    let _ = write!(out, " score={:.2}", hit.score);
    let _ = write!(out, " origin={}", hit.origin.as_str());
    if let Some(node_ref) = hit.node_ref.as_deref() {
        let _ = write!(out, " ref=`{node_ref}`");
    }
    if hit.duplicate_of.is_some() {
        let _ = write!(out, " (see above)");
    }
    out
}

pub(crate) fn node_ref(
    project_root: &Path,
    file_path: Option<&str>,
    line: Option<u32>,
    display_name: &str,
) -> Option<String> {
    let file_path = file_path?;
    let line = line?;
    Some(format!(
        "{}:{line}:{display_name}",
        relative_path(project_root, file_path)
    ))
}

fn render_trail_edge_notation(edge_kind: &str, certainty: Option<&str>) -> (String, String) {
    let normalized = certainty.map(|value| value.to_ascii_lowercase());
    let connector = match normalized.as_deref() {
        Some("probable") => format!("~{edge_kind}~>"),
        Some("uncertain" | "speculative") => format!("?{edge_kind}?>"),
        _ => format!("-{edge_kind}->"),
    };
    let suffix = certainty
        .map(|value| format!(" certainty={value}"))
        .unwrap_or_else(|| " [unresolved]".to_string());
    (connector, suffix)
}

fn escape_dot(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn mermaid_node_id(value: &str) -> String {
    let mut out = String::from("n");
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn escape_mermaid_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn ansi_highlight_snippet(path: &str, snippet: &str) -> String {
    let language = snippet_language(path);
    if language.is_empty() {
        return snippet.to_string();
    }
    snippet
        .lines()
        .map(|line| ansi_highlight_line(language, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ansi_highlight_line(language: &str, line: &str) -> String {
    let comment_marker = match language {
        "python" | "ruby" | "toml" | "yaml" => Some("#"),
        "rust" | "typescript" | "tsx" | "javascript" | "jsx" | "go" | "java" | "kotlin"
        | "csharp" | "cpp" | "php" | "swift" => Some("//"),
        _ => None,
    };
    let Some(marker) = comment_marker else {
        return ansi_highlight_code(language, line);
    };
    if let Some(index) = line.find(marker) {
        let (code, comment) = line.split_at(index);
        return format!(
            "{}\x1b[90m{}\x1b[0m",
            ansi_highlight_code(language, code),
            comment
        );
    }
    ansi_highlight_code(language, line)
}

fn ansi_highlight_code(language: &str, code: &str) -> String {
    let mut out = String::new();
    let mut chars = code.chars().peekable();
    while let Some(ch) = chars.next() {
        if matches!(ch, '"' | '\'' | '`') {
            out.push_str("\x1b[32m");
            out.push(ch);
            let quote = ch;
            let mut escaped = false;
            for next in chars.by_ref() {
                out.push(next);
                if escaped {
                    escaped = false;
                    continue;
                }
                if next == '\\' {
                    escaped = true;
                    continue;
                }
                if next == quote {
                    break;
                }
            }
            out.push_str("\x1b[0m");
            continue;
        }

        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut word = String::new();
            word.push(ch);
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_alphanumeric() || next == '_' {
                    word.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if is_language_keyword(language, &word) {
                let _ = write!(out, "\x1b[1;34m{word}\x1b[0m");
            } else {
                out.push_str(&word);
            }
            continue;
        }

        out.push(ch);
    }
    out
}

fn is_language_keyword(language: &str, word: &str) -> bool {
    match language {
        "rust" => matches!(
            word,
            "as" | "async"
                | "await"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "else"
                | "enum"
                | "fn"
                | "for"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "match"
                | "mod"
                | "mut"
                | "pub"
                | "return"
                | "self"
                | "struct"
                | "trait"
                | "type"
                | "use"
                | "where"
                | "while"
        ),
        "typescript" | "tsx" | "javascript" | "jsx" => matches!(
            word,
            "async"
                | "await"
                | "class"
                | "const"
                | "else"
                | "export"
                | "extends"
                | "for"
                | "from"
                | "function"
                | "if"
                | "import"
                | "interface"
                | "let"
                | "new"
                | "return"
                | "type"
                | "var"
                | "while"
        ),
        "python" => matches!(
            word,
            "async"
                | "await"
                | "class"
                | "def"
                | "elif"
                | "else"
                | "except"
                | "for"
                | "from"
                | "if"
                | "import"
                | "in"
                | "lambda"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
        ),
        _ => matches!(
            word,
            "class"
                | "const"
                | "else"
                | "enum"
                | "for"
                | "func"
                | "function"
                | "if"
                | "import"
                | "interface"
                | "return"
                | "struct"
                | "type"
                | "var"
                | "while"
        ),
    }
}

fn snippet_language(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" => "javascript",
        "jsx" => "jsx",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "cs" => "csharp",
        "cpp" | "cc" | "cxx" => "cpp",
        "h" | "hpp" => "cpp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "json" => "json",
        "toml" => "toml",
        "md" => "markdown",
        "yml" | "yaml" => "yaml",
        _ => "",
    }
}

fn snippet_fence(snippet: &str) -> &'static str {
    if snippet.contains("```") {
        "````"
    } else {
        "```"
    }
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
    if let Some(summary) = symbol.summary.as_deref() {
        let _ = write!(out, " summary=\"{}\"", summary.replace('"', "\\\""));
    }
    if !symbol.edge_digest.is_empty() {
        let _ = write!(out, " edges={}", symbol.edge_digest.join("; "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto, GraphResponse, NodeDetailsDto, NodeId,
        TrailContextDto,
    };
    use serde_json::json;
    use std::path::Path;
    use tempfile::tempdir;

    fn sample_node_details(id: &str, display_name: &str) -> NodeDetailsDto {
        NodeDetailsDto {
            id: NodeId(id.to_string()),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            display_name: display_name.to_string(),
            serialized_name: display_name.to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: None,
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
            member_access: None,
        }
    }

    fn sample_graph_node(id: &str, label: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: label.to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            depth: 0,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        }
    }

    fn sample_graph_edge(id: &str, source: &str, target: &str) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind: EdgeKind::CALL,
            confidence: None,
            certainty: None,
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    #[test]
    fn render_output_content_uses_selected_format() {
        let markdown = render_output_content(OutputFormat::Markdown, &json!({"ok": true}), "hello")
            .expect("render markdown");
        assert_eq!(markdown, "hello\n");

        let json_output =
            render_output_content(OutputFormat::Json, &json!({"ok": true}), "ignored")
                .expect("render json");
        assert!(json_output.contains("\"ok\": true"));
    }

    #[test]
    fn render_output_content_rejects_dot_without_trail_renderer() {
        let error = render_output_content(OutputFormat::Dot, &json!({"ok": true}), "ignored")
            .expect_err("generic emit should reject dot");

        assert!(
            error
                .to_string()
                .contains("--format dot is only supported by `trail`"),
            "{error:#}"
        );
    }

    #[test]
    fn write_output_file_rejects_missing_parent_directory() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("missing").join("out.md");

        let error = write_output_file(&path, "content").expect_err("missing parent should fail");

        assert!(
            error
                .to_string()
                .contains("Output parent directory does not exist"),
            "{error:#}"
        );
    }

    #[test]
    fn trail_edge_notation_reflects_certainty() {
        assert_eq!(
            render_trail_edge_notation("call", Some("certain")),
            ("-call->".to_string(), " certainty=certain".to_string())
        );
        assert_eq!(
            render_trail_edge_notation("call", Some("definite")),
            ("-call->".to_string(), " certainty=definite".to_string())
        );
        assert_eq!(
            render_trail_edge_notation("call", Some("probable")),
            ("~call~>".to_string(), " certainty=probable".to_string())
        );
        assert_eq!(
            render_trail_edge_notation("call", Some("uncertain")),
            ("?call?>".to_string(), " certainty=uncertain".to_string())
        );
        assert_eq!(
            render_trail_edge_notation("call", Some("speculative")),
            ("?call?>".to_string(), " certainty=speculative".to_string())
        );
        assert_eq!(
            render_trail_edge_notation("call", None),
            ("-call->".to_string(), " [unresolved]".to_string())
        );
    }

    #[test]
    fn render_trail_dot_emits_graphviz_nodes_and_edges() {
        let context = TrailContextDto {
            focus: sample_node_details("a", "A"),
            trail: GraphResponse {
                center_id: NodeId("a".to_string()),
                nodes: vec![sample_graph_node("a", "A"), sample_graph_node("b", "B")],
                edges: vec![sample_graph_edge("edge-1", "a", "b")],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        };

        let dot = render_trail_dot(Path::new("C:/repo"), &context);

        assert!(dot.contains("\"a\" [label=\"A\\n[function]\"];"));
        assert!(dot.contains("\"a\" -> \"b\" [label=\"call\"];"));
    }
}
