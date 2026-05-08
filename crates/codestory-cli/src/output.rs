use anyhow::{Context, Result, bail};
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, AgentResponseBlockDto, AgentRetrievalPolicyModeDto,
    AgentRetrievalPresetDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, GraphArtifactDto, GroundingSnapshotDto, NodeDetailsDto,
    RepoTextScanStatsDto, RetrievalFallbackReasonDto, RetrievalModeDto, RetrievalStateDto,
    SearchHit, SnippetContextDto, SymbolContextDto, TrailContextDto, TrailStoryDto,
};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::args::{
    CliTrailMode, DoctorOutput, ExplainOutput, IndexDryRunOutput, IndexOutput, OutputFormat,
    QueryOutput, SearchHitOutput, SearchOutput, TrailCommand,
};
use crate::display::{
    clean_path_string, default_trail_direction, format_budget, format_direction, format_kind,
    format_trail_mode, relative_path,
};
use crate::runtime::ResolvedTarget;

const EVIDENCE_PREVIEW_LIMIT: usize = 3;

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

pub(crate) fn validate_output_file_parent(path: &Path) -> Result<()> {
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
    Ok(())
}

fn write_output_file(path: &Path, content: &str) -> Result<()> {
    validate_output_file_parent(path)?;
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
    if explain {
        append_ground_evidence_packet(&mut markdown, project_root, snapshot);
    }
    if !explain && !snapshot.recommended_queries.is_empty() {
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

pub(crate) fn render_explain_markdown(project_root: &Path, output: &ExplainOutput<'_>) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Repo Explanation");
    let _ = writeln!(markdown, "root: `{}`", clean_path_string(output.project));
    let _ = writeln!(markdown, "refresh: `{}`", output.refresh);
    let _ = writeln!(markdown, "workflow: {}", output.workflow.join(" -> "));
    if let Some(retrieval) = output.retrieval {
        let _ = writeln!(markdown, "retrieval: {}", render_retrieval_state(retrieval));
    }
    let _ = writeln!(
        markdown,
        "coverage: files {}/{} symbols {}/{}",
        output.grounding.coverage.represented_files,
        output.grounding.coverage.total_files,
        output.grounding.coverage.represented_symbols,
        output.grounding.coverage.total_symbols
    );
    let _ = writeln!(markdown, "prompt: `{}`", output.prompt);

    if !output.anchors.is_empty() {
        let _ = writeln!(markdown, "anchors:");
        for anchor in output.anchors.iter().take(EVIDENCE_PREVIEW_LIMIT * 2) {
            let _ = writeln!(markdown, "- {}", render_search_hit_output(anchor));
        }
    }

    if !output.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }

    let _ = writeln!(markdown);
    markdown.push_str(&render_agent_answer_markdown(project_root, output.answer));
    markdown
}

pub(crate) fn render_search_markdown(project_root: &Path, output: &SearchOutput) -> String {
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
    if let Some(stats) = output.repo_text_stats.as_ref() {
        append_repo_text_scan_stats(&mut markdown, stats);
    }
    if output.explain {
        append_search_evidence_packet(&mut markdown, project_root, output);
    }
    if !output.suggestions.is_empty() {
        let _ = writeln!(markdown, "did_you_mean:");
        for hit in &output.suggestions {
            let _ = writeln!(markdown, "- {}", render_search_hit_output(hit));
        }
    }
    if !output.explain && !output.query_hints.is_empty() {
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
        if !output.explain {
            append_search_hit_why(&mut markdown, hit);
        }
    }
    let _ = writeln!(markdown, "repo_text_hits: {}", output.repo_text_hits.len());
    for hit in &output.repo_text_hits {
        let _ = writeln!(markdown, "- {}", render_search_hit_output(hit));
        if !output.explain {
            append_search_hit_why(&mut markdown, hit);
        }
        if let Some(excerpt) = hit.excerpt.as_deref() {
            let _ = writeln!(markdown, "  excerpt: {}", excerpt);
        }
    }
    markdown
}

fn append_search_hit_why(markdown: &mut String, hit: &SearchHitOutput) {
    if hit.why.is_empty() {
        return;
    }
    for why in &hit.why {
        let _ = writeln!(markdown, "  why: {why}");
    }
}

fn append_repo_text_scan_stats(markdown: &mut String, stats: &RepoTextScanStatsDto) {
    let _ = writeln!(
        markdown,
        "repo_text_scan: {}",
        repo_text_scan_summary(stats)
    );
    if stats.truncated {
        if let Some(reason) = stats.reason.as_deref() {
            let _ = writeln!(markdown, "repo_text_scan_reason: {reason}");
        }
        if let Some(action) = stats.action.as_deref() {
            let _ = writeln!(markdown, "repo_text_scan_action: {action}");
        }
    }
}

fn repo_text_scan_summary(stats: &RepoTextScanStatsDto) -> String {
    format!(
        "files={}/{} bytes={}/{} duration_ms={}/{} skipped_large={} truncated={}",
        stats.scanned_file_count,
        stats.file_cap,
        stats.scanned_byte_count,
        stats.byte_cap,
        stats.duration_ms,
        stats.time_cap_ms,
        stats.skipped_large_file_count,
        stats.truncated
    )
}

fn append_ground_evidence_packet(
    markdown: &mut String,
    project_root: &Path,
    snapshot: &GroundingSnapshotDto,
) {
    let (confidence, reasons) = ground_confidence(snapshot);
    let _ = writeln!(
        markdown,
        "short_finding: grounding snapshot represents {}/{} files and {}/{} symbols.",
        snapshot.coverage.represented_files,
        snapshot.coverage.total_files,
        snapshot.coverage.represented_symbols,
        snapshot.coverage.total_symbols
    );
    let _ = writeln!(
        markdown,
        "confidence: {confidence} - {}",
        reasons.join("; ")
    );

    let _ = writeln!(markdown, "what_was_checked:");
    let _ = writeln!(
        markdown,
        "- project `{}` with `{}` budget",
        clean_path_string(&snapshot.root),
        format_budget(snapshot.budget)
    );
    let _ = writeln!(
        markdown,
        "- graph stats nodes={} edges={} files={} errors={}",
        snapshot.stats.node_count,
        snapshot.stats.edge_count,
        snapshot.stats.file_count,
        snapshot.stats.error_count
    );
    if let Some(retrieval) = snapshot.retrieval.as_ref() {
        let _ = writeln!(
            markdown,
            "- retrieval state: {}",
            render_retrieval_state(retrieval)
        );
    } else {
        let _ = writeln!(markdown, "- retrieval state: unavailable");
    }

    let mut gaps = ground_gap_notes(snapshot);
    if gaps.is_empty() {
        gaps.push("No coverage gaps or retrieval fallbacks were reported.".to_string());
    }
    let _ = writeln!(markdown, "gaps_uncertainty:");
    for gap in gaps {
        let _ = writeln!(markdown, "- {gap}");
    }

    let _ = writeln!(markdown, "citations:");
    if snapshot.root_symbols.is_empty() {
        for file in snapshot.files.iter().take(EVIDENCE_PREVIEW_LIMIT) {
            let _ = writeln!(
                markdown,
                "- `{}` [{}] represented_symbols={}/{}",
                relative_path(project_root, &file.file_path),
                file.language.as_deref().unwrap_or("unknown"),
                file.represented_symbol_count,
                file.symbol_count
            );
        }
        if snapshot.files.is_empty() {
            let _ = writeln!(markdown, "- none from grounding snapshot");
        }
    } else {
        for symbol in snapshot.root_symbols.iter().take(EVIDENCE_PREVIEW_LIMIT) {
            let node_ref = symbol
                .node_ref
                .as_deref()
                .map(|value| format!(" ref=`{value}`"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- [{}] {} [{}]{}",
                symbol.id.0,
                symbol.label,
                format_kind(symbol.kind),
                node_ref
            );
        }
    }

    let _ = writeln!(markdown, "next_commands:");
    if snapshot.recommended_queries.is_empty() {
        let _ = writeln!(
            markdown,
            "- `codestory-cli ground --project {} --why`",
            quoted_cli_arg(&clean_path_string(&project_root.to_string_lossy()))
        );
    } else {
        for command in snapshot
            .recommended_queries
            .iter()
            .take(EVIDENCE_PREVIEW_LIMIT)
        {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
}

fn append_search_evidence_packet(
    markdown: &mut String,
    project_root: &Path,
    output: &SearchOutput,
) {
    let total_hits = output.indexed_symbol_hits.len() + output.repo_text_hits.len();
    let _ = writeln!(
        markdown,
        "short_finding: found {total_hits} direct hits for `{}` (indexed_symbol_hits={} repo_text_hits={}).",
        output.query,
        output.indexed_symbol_hits.len(),
        output.repo_text_hits.len()
    );
    let (confidence, reasons) = search_confidence(output);
    let _ = writeln!(
        markdown,
        "confidence: {confidence} - {}",
        reasons.join("; ")
    );

    let _ = writeln!(markdown, "what_was_checked:");
    let _ = writeln!(
        markdown,
        "- indexed symbol search with limit_per_source={}",
        output.limit_per_source
    );
    let _ = writeln!(
        markdown,
        "- repo text search {}",
        if output.repo_text_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    if let Some(stats) = output.repo_text_stats.as_ref() {
        let _ = writeln!(
            markdown,
            "- repo text scan caps: {}",
            repo_text_scan_summary(stats)
        );
    }
    let _ = writeln!(
        markdown,
        "- retrieval state: {}",
        render_retrieval_state(&output.retrieval)
    );

    let mut gaps = search_gap_notes(output);
    if gaps.is_empty() {
        gaps.push("No search gaps were reported for this query.".to_string());
    }
    let _ = writeln!(markdown, "gaps_uncertainty:");
    for gap in gaps {
        let _ = writeln!(markdown, "- {gap}");
    }

    let _ = writeln!(markdown, "citations:");
    let mut wrote_citation = false;
    for hit in output
        .indexed_symbol_hits
        .iter()
        .chain(output.repo_text_hits.iter())
        .take(EVIDENCE_PREVIEW_LIMIT)
    {
        wrote_citation = true;
        let _ = writeln!(markdown, "- {}", render_search_hit_output(hit));
    }
    if !wrote_citation {
        let _ = writeln!(markdown, "- none");
    }

    let _ = writeln!(markdown, "next_commands:");
    if output.query_hints.is_empty() {
        if let Some(hit) = output
            .indexed_symbol_hits
            .iter()
            .chain(output.repo_text_hits.iter())
            .find(|hit| hit.resolvable)
        {
            let project = quoted_project_arg(project_root);
            let _ = writeln!(
                markdown,
                "- `codestory-cli symbol --project {project} --id {}`",
                hit.node_id
            );
            let _ = writeln!(
                markdown,
                "- `codestory-cli ask --project {project} --focus-id {} {}`",
                hit.node_id,
                quoted_cli_arg("Explain this symbol")
            );
        } else {
            let _ = writeln!(
                markdown,
                "- `codestory-cli search --project {} --query {} --why`",
                quoted_project_arg(project_root),
                quoted_cli_arg(&output.query)
            );
        }
    } else {
        for hint in output.query_hints.iter().take(EVIDENCE_PREVIEW_LIMIT) {
            let _ = writeln!(markdown, "- {hint}");
        }
    }
}

fn append_agent_evidence_packet(
    markdown: &mut String,
    project_root: &Path,
    answer: &AgentAnswerDto,
) {
    let (confidence, reasons) = agent_confidence(answer);
    let _ = writeln!(
        markdown,
        "confidence: {confidence} - {}",
        reasons.join("; ")
    );

    let _ = writeln!(markdown, "what_was_checked:");
    let _ = writeln!(
        markdown,
        "- retrieval plan: profile={} policy={} latency_ms={} steps={}",
        format_agent_profile(answer.retrieval_trace.resolved_profile),
        format_agent_policy(answer.retrieval_trace.policy_mode),
        answer.retrieval_trace.total_latency_ms,
        answer.retrieval_trace.steps.len()
    );
    let checked_stages = answer
        .retrieval_trace
        .steps
        .iter()
        .take(EVIDENCE_PREVIEW_LIMIT + 3)
        .map(|step| {
            format!(
                "{}:{}",
                format_agent_step_kind(step.kind),
                format_agent_step_status(step.status)
            )
        })
        .collect::<Vec<_>>();
    if checked_stages.is_empty() {
        let _ = writeln!(markdown, "- no retrieval steps were recorded");
    } else {
        let _ = writeln!(markdown, "- checked stages: {}", checked_stages.join(", "));
    }
    let remaining_steps = answer
        .retrieval_trace
        .steps
        .len()
        .saturating_sub(EVIDENCE_PREVIEW_LIMIT + 3);
    if remaining_steps > 0 {
        let _ = writeln!(
            markdown,
            "- plus {remaining_steps} more stages in the JSON/bundle trace"
        );
    }

    let mut gaps = agent_gap_notes(answer);
    if gaps.is_empty() {
        gaps.push("No explicit gaps were recorded in the retrieval trace.".to_string());
    }
    let _ = writeln!(markdown, "gaps_uncertainty:");
    for gap in gaps {
        let _ = writeln!(markdown, "- {gap}");
    }

    let _ = writeln!(markdown, "citations:");
    if answer.citations.is_empty() {
        let _ = writeln!(markdown, "- none");
    } else {
        for citation in answer.citations.iter().take(EVIDENCE_PREVIEW_LIMIT) {
            let _ = writeln!(
                markdown,
                "- {}",
                render_agent_citation(project_root, citation, false)
            );
        }
    }

    let _ = writeln!(markdown, "next_commands:");
    if let Some(citation) = answer.citations.iter().find(|citation| citation.resolvable) {
        let project = quoted_project_arg(project_root);
        let _ = writeln!(
            markdown,
            "- `codestory-cli symbol --project {project} --id {}`",
            citation.node_id.0
        );
        let _ = writeln!(
            markdown,
            "- `codestory-cli trail --project {project} --id {}`",
            citation.node_id.0
        );
    } else {
        let _ = writeln!(
            markdown,
            "- `codestory-cli search --project {} --query {} --why`",
            quoted_project_arg(project_root),
            quoted_cli_arg(&answer.prompt.replace('\n', " "))
        );
    }
}

fn ground_confidence(snapshot: &GroundingSnapshotDto) -> (&'static str, Vec<String>) {
    let mut limiting = ground_gap_notes(snapshot);
    let has_no_content =
        snapshot.coverage.total_files == 0 || snapshot.coverage.represented_files == 0;
    let confidence = if snapshot.stats.error_count > 0 || has_no_content {
        "low"
    } else if limiting.is_empty() {
        "high"
    } else {
        "medium"
    };
    if limiting.is_empty() {
        limiting.push("complete represented coverage with no fallback signal".to_string());
    }
    (confidence, limiting)
}

fn ground_gap_notes(snapshot: &GroundingSnapshotDto) -> Vec<String> {
    let mut gaps = Vec::new();
    if snapshot.stats.error_count > 0 {
        gaps.push(format!(
            "index reported {} errors",
            snapshot.stats.error_count
        ));
    }
    if snapshot.coverage.total_files == 0 {
        gaps.push("no files were indexed".to_string());
    } else if snapshot.coverage.represented_files < snapshot.coverage.total_files {
        gaps.push(format!(
            "represented files are partial: {}/{}",
            snapshot.coverage.represented_files, snapshot.coverage.total_files
        ));
    }
    if snapshot.coverage.total_symbols == 0 {
        gaps.push("no symbols were indexed".to_string());
    } else if snapshot.coverage.represented_symbols < snapshot.coverage.total_symbols {
        gaps.push(format!(
            "represented symbols are partial: {}/{}",
            snapshot.coverage.represented_symbols, snapshot.coverage.total_symbols
        ));
    }
    if snapshot.coverage.compressed_files > 0 {
        gaps.push(format!(
            "{} files are compressed in the packet",
            snapshot.coverage.compressed_files
        ));
    }
    if let Some(retrieval) = snapshot.retrieval.as_ref() {
        append_retrieval_gap_notes(&mut gaps, retrieval);
    } else {
        gaps.push("retrieval state is unavailable".to_string());
    }
    gaps
}

fn search_confidence(output: &SearchOutput) -> (&'static str, Vec<String>) {
    let mut reasons = search_gap_notes(output);
    let top_score = output
        .indexed_symbol_hits
        .iter()
        .chain(output.repo_text_hits.iter())
        .map(|hit| hit.score)
        .fold(0.0_f32, |left, right| left.max(right));
    let total_hits = output.indexed_symbol_hits.len() + output.repo_text_hits.len();
    let confidence = if total_hits == 0 || top_score < 0.35 {
        "low"
    } else if !output.indexed_symbol_hits.is_empty()
        && top_score >= 0.75
        && output.retrieval.fallback_reason.is_none()
        && output.retrieval.semantic_ready
    {
        "high"
    } else {
        "medium"
    };
    if reasons.is_empty() {
        reasons.push(format!(
            "top hit score {top_score:.2} with indexed evidence available"
        ));
    }
    (confidence, reasons)
}

fn search_gap_notes(output: &SearchOutput) -> Vec<String> {
    let mut gaps = Vec::new();
    append_retrieval_gap_notes(&mut gaps, &output.retrieval);
    if output.indexed_symbol_hits.is_empty() && output.repo_text_hits.is_empty() {
        gaps.push("no indexed symbol or repo-text hits matched".to_string());
    } else if output.indexed_symbol_hits.is_empty() {
        gaps.push(
            "only repo-text hits matched; resolve a concrete identifier before graph browsing"
                .to_string(),
        );
    }
    if !output.repo_text_enabled {
        gaps.push("repo text fallback was disabled".to_string());
    }
    if let Some(stats) = output.repo_text_stats.as_ref()
        && stats.truncated
    {
        gaps.push(
            stats
                .reason
                .clone()
                .unwrap_or_else(|| "repo-text scan hit a configured cap".to_string()),
        );
        if let Some(action) = stats.action.clone() {
            gaps.push(action);
        }
    }
    if !output.suggestions.is_empty() {
        gaps.push(format!(
            "{} query suggestions may indicate ambiguity or spelling drift",
            output.suggestions.len()
        ));
    }
    if let Some(top_score) = output
        .indexed_symbol_hits
        .iter()
        .chain(output.repo_text_hits.iter())
        .map(|hit| hit.score)
        .max_by(|left, right| left.total_cmp(right))
        && top_score < 0.5
    {
        gaps.push(format!("top hit score is weak: {top_score:.2}"));
    }
    gaps
}

fn agent_confidence(answer: &AgentAnswerDto) -> (&'static str, Vec<String>) {
    let mut reasons = agent_gap_notes(answer);
    let top_score = answer
        .citations
        .iter()
        .map(|citation| citation.score)
        .fold(0.0_f32, |left, right| left.max(right));
    let has_problem_step = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status != AgentRetrievalStepStatusDto::Ok);
    let confidence = if answer.citations.is_empty()
        || has_problem_step
        || answer.retrieval_trace.sla_missed
        || top_score < 0.35
    {
        "low"
    } else if top_score >= 0.75 && reasons.is_empty() {
        "high"
    } else {
        "medium"
    };
    if reasons.is_empty() {
        reasons.push(format!(
            "{} citations with top retrieval score {top_score:.2}",
            answer.citations.len()
        ));
    }
    (confidence, reasons)
}

fn agent_gap_notes(answer: &AgentAnswerDto) -> Vec<String> {
    let mut gaps = Vec::new();
    if answer.citations.is_empty() {
        gaps.push("no citations were returned".to_string());
    }
    if answer.retrieval_trace.sla_missed {
        let target = answer
            .retrieval_trace
            .sla_target_ms
            .map(|value| format!(" target_ms={value}"))
            .unwrap_or_default();
        gaps.push(format!(
            "retrieval SLA missed: latency_ms={}{}",
            answer.retrieval_trace.total_latency_ms, target
        ));
    }
    for annotation in answer
        .retrieval_trace
        .annotations
        .iter()
        .filter(|annotation| is_gap_annotation(annotation))
        .take(EVIDENCE_PREVIEW_LIMIT)
    {
        gaps.push(format!("trace annotation: {annotation}"));
    }
    for step in answer
        .retrieval_trace
        .steps
        .iter()
        .filter(|step| step.status != AgentRetrievalStepStatusDto::Ok)
        .take(EVIDENCE_PREVIEW_LIMIT)
    {
        gaps.push(format!(
            "retrieval step issue: {}",
            render_agent_step_summary(step)
        ));
    }
    if answer
        .citations
        .iter()
        .all(|citation| citation.retrieval_score_breakdown.is_none())
        && !answer.citations.is_empty()
    {
        gaps.push("detailed citation score breakdowns are unavailable; use JSON/bundle trace for full evidence".to_string());
    }
    gaps
}

fn append_retrieval_gap_notes(gaps: &mut Vec<String>, retrieval: &RetrievalStateDto) {
    if let Some(reason) = retrieval.fallback_reason {
        gaps.push(format!(
            "retrieval fallback: {}",
            format_retrieval_fallback_reason(reason)
        ));
    }
    if let Some(message) = retrieval.fallback_message.as_deref() {
        gaps.push(format!("retrieval note: {}", message.replace('\n', " ")));
    }
    if !retrieval.semantic_ready {
        gaps.push("semantic retrieval is not ready".to_string());
    }
}

fn render_agent_step_summary(step: &AgentRetrievalStepDto) -> String {
    let message = step
        .message
        .as_deref()
        .map(|value| format!(" message=\"{}\"", value.replace('"', "\\\"")))
        .unwrap_or_default();
    format!(
        "{} status={} duration_ms={}{}",
        format_agent_step_kind(step.kind),
        format_agent_step_status(step.status),
        step.duration_ms,
        message
    )
}

fn render_agent_citation(
    project_root: &Path,
    citation: &AgentCitationDto,
    include_breakdown: bool,
) -> String {
    let file = citation
        .file_path
        .as_deref()
        .map(|path| relative_path(project_root, path))
        .unwrap_or_else(|| "-".to_string());
    let line = citation
        .line
        .map(|line| format!(":{line}"))
        .unwrap_or_default();
    let mut out = format!(
        "[{}] {} [{}] {}{} origin={} resolvable={} score={:.3}",
        citation.node_id.0,
        citation.display_name,
        format_kind(citation.kind),
        file,
        line,
        citation.origin.as_str(),
        citation.resolvable,
        citation.score
    );
    if include_breakdown && let Some(breakdown) = citation.retrieval_score_breakdown.as_ref() {
        let _ = write!(
            out,
            " why lexical={:.3} semantic={:.3} graph={:.3} total={:.3}",
            breakdown.lexical, breakdown.semantic, breakdown.graph, breakdown.total
        );
    }
    out
}

fn is_gap_annotation(annotation: &str) -> bool {
    let lower = annotation.to_ascii_lowercase();
    [
        "fallback",
        "gap",
        "low confidence",
        "missing",
        "no relevant",
        "skipped",
        "truncated",
        "uncertain",
        "unavailable",
        "weak",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn format_retrieval_fallback_reason(reason: RetrievalFallbackReasonDto) -> &'static str {
    match reason {
        RetrievalFallbackReasonDto::DisabledByConfig => "disabled_by_config",
        RetrievalFallbackReasonDto::MissingEmbeddingRuntime => "missing_embedding_runtime",
        RetrievalFallbackReasonDto::MissingSemanticDocs => "missing_semantic_docs",
    }
}

fn format_agent_step_kind(kind: AgentRetrievalStepKindDto) -> &'static str {
    match kind {
        AgentRetrievalStepKindDto::Search => "search",
        AgentRetrievalStepKindDto::SemanticQueryEmbedding => "semantic_query_embedding",
        AgentRetrievalStepKindDto::SemanticCandidateRetrieval => "semantic_candidate_retrieval",
        AgentRetrievalStepKindDto::HybridRerank => "hybrid_rerank",
        AgentRetrievalStepKindDto::QueryExpansion => "query_expansion",
        AgentRetrievalStepKindDto::RepoTextFallback => "repo_text_fallback",
        AgentRetrievalStepKindDto::TrailFilterOptions => "trail_filter_options",
        AgentRetrievalStepKindDto::Neighborhood => "neighborhood",
        AgentRetrievalStepKindDto::Trail => "trail",
        AgentRetrievalStepKindDto::NodeDetails => "node_details",
        AgentRetrievalStepKindDto::NodeOccurrences => "node_occurrences",
        AgentRetrievalStepKindDto::EdgeOccurrences => "edge_occurrences",
        AgentRetrievalStepKindDto::SourceRead => "source_read",
        AgentRetrievalStepKindDto::MermaidSynthesis => "mermaid_synthesis",
        AgentRetrievalStepKindDto::AnswerSynthesis => "answer_synthesis",
    }
}

fn format_agent_step_status(status: AgentRetrievalStepStatusDto) -> &'static str {
    match status {
        AgentRetrievalStepStatusDto::Ok => "ok",
        AgentRetrievalStepStatusDto::Error => "error",
        AgentRetrievalStepStatusDto::Skipped => "skipped",
        AgentRetrievalStepStatusDto::Truncated => "truncated",
    }
}

fn format_agent_profile(profile: AgentRetrievalPresetDto) -> &'static str {
    match profile {
        AgentRetrievalPresetDto::Architecture => "architecture",
        AgentRetrievalPresetDto::Callflow => "callflow",
        AgentRetrievalPresetDto::Inheritance => "inheritance",
        AgentRetrievalPresetDto::Impact => "impact",
        AgentRetrievalPresetDto::Investigate => "investigate",
    }
}

fn format_agent_policy(policy: AgentRetrievalPolicyModeDto) -> &'static str {
    match policy {
        AgentRetrievalPolicyModeDto::LatencyFirst => "latency_first",
        AgentRetrievalPolicyModeDto::CompletenessFirst => "completeness_first",
    }
}

fn quoted_project_arg(project_root: &Path) -> String {
    quoted_cli_arg(&clean_path_string(&project_root.to_string_lossy()))
}

fn quoted_cli_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
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
    let _ = writeln!(markdown, "mode: {}", agent_answer_mode_label(answer));
    append_agent_evidence_packet(&mut markdown, project_root, answer);
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
            let _ = writeln!(
                markdown,
                "- {}",
                render_agent_citation(project_root, citation, true)
            );
        }
    }
    markdown
}

fn agent_answer_mode_label(answer: &AgentAnswerDto) -> &'static str {
    let annotations = &answer.retrieval_trace.annotations;
    let repo_explain = annotations
        .iter()
        .any(|annotation| annotation.starts_with("mode=repo_explain_"));

    if repo_explain {
        "DB-first repo explanation packet assembled from indexed evidence"
    } else {
        "DB-first retrieval packet assembled from indexed evidence"
    }
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
    let attention = output
        .checks
        .iter()
        .filter(|check| matches!(check.status.as_str(), "warn" | "error"))
        .collect::<Vec<_>>();
    if !attention.is_empty() {
        let _ = writeln!(markdown, "attention:");
        for check in attention {
            let _ = writeln!(
                markdown,
                "- {} [{}]: {}",
                check.name, check.status, check.message
            );
        }
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

pub(crate) fn render_trail_story_markdown(
    project_root: &Path,
    target: &ResolvedTarget,
    context: &TrailContextDto,
    _cmd: &TrailCommand,
    story: &TrailStoryDto,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Trail Story");
    append_resolution(&mut markdown, project_root, target);
    let _ = writeln!(
        markdown,
        "focus: {}",
        render_node(project_root, &context.focus)
    );
    let _ = writeln!(markdown, "summary: {}", story.summary);

    append_story_list(&mut markdown, "## Entry Points", &story.entry_points);

    let _ = writeln!(markdown, "\n## Core Flow");
    if story.core_flow.is_empty() {
        let _ = writeln!(markdown, "- no graph edges were returned for this focus");
    } else {
        for step in &story.core_flow {
            let _ = writeln!(
                markdown,
                "- [{}] {} {} {} (certainty={}). {}",
                step.edge_id, step.source, step.relation, step.target, step.certainty, step.note
            );
        }
    }

    append_story_list(&mut markdown, "## Side Effects", &story.side_effects);
    append_story_list(&mut markdown, "## Uncertainty", &story.uncertainty);
    append_story_list(&mut markdown, "## Tests", &story.test_scope);
    append_story_list(&mut markdown, "## Gaps And Limits", &story.limits);
    markdown
}

fn append_story_list(markdown: &mut String, title: &str, items: &[String]) {
    let _ = writeln!(markdown, "\n{title}");
    if items.is_empty() {
        let _ = writeln!(markdown, "- none");
    } else {
        for item in items {
            let _ = writeln!(markdown, "- {item}");
        }
    }
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
    let _ = writeln!(
        markdown,
        "context: requested_lines={} max_snippet_bytes={}",
        context.requested_context,
        context.max_snippet_bytes.unwrap_or_default()
    );
    if context.snippet_truncated {
        let _ = writeln!(
            markdown,
            "snippet_truncated: true (max_snippet_bytes={})",
            context.max_snippet_bytes.unwrap_or_default()
        );
    }
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

pub(crate) fn render_retrieval_state(state: &RetrievalStateDto) -> String {
    let mode = match state.mode {
        RetrievalModeDto::Hybrid => "hybrid",
        RetrievalModeDto::Symbolic => "symbolic",
    };
    let mut out = format!("{mode} semantic_docs={}", state.semantic_doc_count);
    if let Some(model) = state.embedding_model.as_deref() {
        let _ = write!(out, " model={model}");
    }
    if let Some(reason) = state.fallback_reason {
        let reason = format_retrieval_fallback_reason(reason);
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

fn render_search_hit_output(hit: &SearchHitOutput) -> String {
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
        AgentAnswerDto, AgentCitationDto, AgentResponseBlockDto, AgentResponseSectionDto,
        AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto, AgentRetrievalStepDto,
        AgentRetrievalStepKindDto, AgentRetrievalStepStatusDto, AgentRetrievalTraceDto, EdgeId,
        EdgeKind, GraphEdgeDto, GraphNodeDto, GraphResponse, GroundingBudgetDto,
        GroundingCoverageDto, GroundingFileDigestDto, GroundingSnapshotDto,
        GroundingSymbolDigestDto, NodeDetailsDto, NodeId, NodeKind, RetrievalFallbackReasonDto,
        RetrievalModeDto, RetrievalScoreBreakdownDto, RetrievalStateDto, SearchHitOrigin,
        StorageStatsDto, TrailContextDto, TrailStoryDto, TrailStoryStepDto,
    };
    use serde_json::json;
    use std::path::Path;
    use tempfile::tempdir;

    fn assert_evidence_packet_shape(markdown: &str, intro_labels: &[&str]) {
        let lower = markdown.to_ascii_lowercase();
        let mut missing = Vec::new();

        if !intro_labels.iter().any(|label| lower.contains(label)) {
            missing.push(format!("one of {intro_labels:?}"));
        }
        for required in ["confidence:", "what_was_checked:", "gaps_uncertainty:"] {
            if !lower.contains(required) {
                missing.push(required.to_string());
            }
        }
        if !lower.contains("citations:") && !lower.contains("## citations") {
            missing.push("citations: or ## citations".to_string());
        }
        if !lower.contains("next_commands:") && !lower.contains("query_hints:") {
            missing.push("next_commands: or query_hints:".to_string());
        }

        assert!(
            missing.is_empty(),
            "Markdown evidence packet is missing {missing:?}:\n{markdown}"
        );
    }

    fn assert_order(markdown: &str, first: &str, second: &str) {
        let first_index = markdown
            .find(first)
            .unwrap_or_else(|| panic!("missing `{first}` in:\n{markdown}"));
        let second_index = markdown
            .find(second)
            .unwrap_or_else(|| panic!("missing `{second}` in:\n{markdown}"));
        assert!(
            first_index < second_index,
            "expected `{first}` before `{second}` in:\n{markdown}"
        );
    }

    fn sample_retrieval() -> RetrievalStateDto {
        RetrievalStateDto {
            mode: RetrievalModeDto::Hybrid,
            hybrid_configured: true,
            semantic_ready: true,
            semantic_doc_count: 12,
            embedding_model: Some("bge-small-en-v1.5".to_string()),
            current_embedding: None,
            stored_embedding: None,
            fallback_reason: None,
            fallback_message: None,
        }
    }

    fn sample_storage_stats() -> StorageStatsDto {
        StorageStatsDto {
            node_count: 3,
            edge_count: 2,
            file_count: 1,
            error_count: 0,
        }
    }

    fn sample_node_details(id: &str, display_name: &str) -> NodeDetailsDto {
        NodeDetailsDto {
            id: NodeId(id.to_string()),
            kind: NodeKind::FUNCTION,
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
            kind: NodeKind::FUNCTION,
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

    fn sample_graph_node_with_file(
        id: &str,
        label: &str,
        file_path: &str,
        depth: u32,
    ) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: label.to_string(),
            kind: NodeKind::FUNCTION,
            depth,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: Some(file_path.to_string()),
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

    fn sample_graph_edge_with_certainty(
        id: &str,
        source: &str,
        target: &str,
        certainty: &str,
        confidence: f32,
    ) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind: EdgeKind::CALL,
            confidence: Some(confidence),
            certainty: Some(certainty.to_string()),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    fn sample_trail_command(include_tests: bool) -> TrailCommand {
        TrailCommand {
            project: crate::args::ProjectArgs {
                project: Path::new("C:/repo").to_path_buf(),
                cache_dir: None,
            },
            target: crate::args::TargetArgs {
                id: None,
                query: Some("handle_request".to_string()),
                file: None,
                choose: None,
            },
            mode: CliTrailMode::Neighborhood,
            depth: Some(2),
            direction: None,
            max_nodes: 24,
            include_tests,
            show_utility_calls: false,
            hide_speculative: false,
            story: true,
            layout: crate::args::CliLayout::Horizontal,
            refresh: crate::args::RefreshMode::None,
            format: OutputFormat::Markdown,
            output_file: None,
            mermaid: false,
        }
    }

    fn sample_resolved_target() -> ResolvedTarget {
        ResolvedTarget {
            selector: crate::args::QuerySelectorOutput::Query,
            requested: "handle_request".to_string(),
            file_filter: None,
            selected: SearchHit {
                node_id: NodeId("handle".to_string()),
                display_name: "handle_request".to_string(),
                kind: NodeKind::FUNCTION,
                file_path: Some("C:/repo/src/request.rs".to_string()),
                line: Some(10),
                score: 1.0,
                origin: SearchHitOrigin::IndexedSymbol,
                resolvable: true,
                score_breakdown: None,
            },
            alternatives: Vec::new(),
        }
    }

    fn sample_story_trail_context() -> TrailContextDto {
        let mut focus = sample_node_details("handle", "handle_request");
        focus.file_path = Some("C:/repo/src/request.rs".to_string());
        focus.start_line = Some(10);
        let mut uncertain =
            sample_graph_edge_with_certainty("edge-hook", "handle", "hook", "uncertain", 0.32);
        uncertain.candidate_targets = vec![NodeId("candidate-one".to_string())];

        TrailContextDto {
            focus,
            trail: GraphResponse {
                center_id: NodeId("handle".to_string()),
                nodes: vec![
                    sample_graph_node_with_file(
                        "handle",
                        "handle_request",
                        "C:/repo/src/request.rs",
                        0,
                    ),
                    sample_graph_node_with_file(
                        "validate",
                        "validate_request",
                        "C:/repo/src/request.rs",
                        1,
                    ),
                    sample_graph_node_with_file(
                        "profile",
                        "load_profile",
                        "C:/repo/src/profile.rs",
                        2,
                    ),
                    sample_graph_node_with_file(
                        "audit",
                        "write_audit_log",
                        "C:/repo/src/audit.rs",
                        1,
                    ),
                    sample_graph_node_with_file(
                        "hook",
                        "dynamic_plugin_hook",
                        "C:/repo/src/plugin.rs",
                        1,
                    ),
                    sample_graph_node_with_file(
                        "test",
                        "test_request_flow",
                        "C:/repo/tests/request_flow.rs",
                        1,
                    ),
                ],
                edges: vec![
                    sample_graph_edge_with_certainty(
                        "edge-validate",
                        "handle",
                        "validate",
                        "certain",
                        0.99,
                    ),
                    sample_graph_edge_with_certainty(
                        "edge-profile",
                        "validate",
                        "profile",
                        "probable",
                        0.72,
                    ),
                    sample_graph_edge_with_certainty(
                        "edge-audit",
                        "handle",
                        "audit",
                        "certain",
                        0.94,
                    ),
                    uncertain,
                    sample_graph_edge_with_certainty(
                        "edge-test",
                        "test",
                        "handle",
                        "certain",
                        0.95,
                    ),
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
            story: None,
        }
    }

    fn sample_trail_story(include_tests: bool) -> TrailStoryDto {
        TrailStoryDto {
            summary: "Story trail around `handle_request` found 6 nodes and 5 edges; mode=neighborhood direction=both tests=included utility_calls=hidden truncated=false.".to_string(),
            entry_points: vec![
                "focus: handle_request [function] `src/request.rs`".to_string(),
                "entry: test_request_flow [function] `tests/request_flow.rs`".to_string(),
            ],
            core_flow: vec![
                TrailStoryStepDto {
                    edge_id: "edge-validate".to_string(),
                    source: "handle_request [function] `src/request.rs`".to_string(),
                    relation: "calls".to_string(),
                    target: "validate_request [function] `src/request.rs`".to_string(),
                    certainty: "certain".to_string(),
                    note: "certain call edge confidence=0.99".to_string(),
                },
                TrailStoryStepDto {
                    edge_id: "edge-profile".to_string(),
                    source: "validate_request [function] `src/request.rs`".to_string(),
                    relation: "calls".to_string(),
                    target: "load_profile [function] `src/profile.rs`".to_string(),
                    certainty: "probable".to_string(),
                    note: "probable call edge confidence=0.72".to_string(),
                },
                TrailStoryStepDto {
                    edge_id: "edge-hook".to_string(),
                    source: "handle_request [function] `src/request.rs`".to_string(),
                    relation: "calls".to_string(),
                    target: "dynamic_plugin_hook [function] `src/plugin.rs`".to_string(),
                    certainty: "uncertain".to_string(),
                    note: "uncertain call edge confidence=0.32 candidate_targets=1".to_string(),
                },
                TrailStoryStepDto {
                    edge_id: "edge-speculative".to_string(),
                    source: "handle_request [function] `src/request.rs`".to_string(),
                    relation: "calls".to_string(),
                    target: "experimental_hook [function] `src/plugin.rs`".to_string(),
                    certainty: "speculative".to_string(),
                    note: "speculative call edge confidence=0.21".to_string(),
                },
                TrailStoryStepDto {
                    edge_id: "edge-missing".to_string(),
                    source: "handle_request [function] `src/request.rs`".to_string(),
                    relation: "calls".to_string(),
                    target: "legacy_dispatch [function] `src/legacy.rs`".to_string(),
                    certainty: "missing certainty metadata".to_string(),
                    note: "missing certainty metadata call edge".to_string(),
                },
            ],
            side_effects: vec![
                "possible side-effect candidate [edge-audit] handle_request [function] `src/request.rs` calls write_audit_log [function] `src/audit.rs` (certainty=certain)".to_string(),
            ],
            uncertainty: vec![
                "[edge-profile] validate_request [function] `src/request.rs` calls load_profile [function] `src/profile.rs` is probable. probable call edge confidence=0.72".to_string(),
                "[edge-hook] handle_request [function] `src/request.rs` calls dynamic_plugin_hook [function] `src/plugin.rs` is uncertain. uncertain call edge confidence=0.32 candidate_targets=1".to_string(),
                "[edge-speculative] handle_request [function] `src/request.rs` calls experimental_hook [function] `src/plugin.rs` is speculative. speculative call edge confidence=0.21".to_string(),
                "[edge-missing] handle_request [function] `src/request.rs` calls legacy_dispatch [function] `src/legacy.rs` is missing certainty metadata. missing certainty metadata call edge".to_string(),
            ],
            test_scope: if include_tests {
                vec![
                    "tests and benches included by --include-tests".to_string(),
                    "1 test-like node(s) present: test_request_flow [function] `tests/request_flow.rs`".to_string(),
                    "utility/helper calls hidden by default; pass --show-utility-calls to include them".to_string(),
                ]
            } else {
                vec![
                    "tests and benches excluded by default production-only scope; pass --include-tests to include them".to_string(),
                    "1 test-like node(s) present: test_request_flow [function] `tests/request_flow.rs`".to_string(),
                    "utility/helper calls hidden by default; pass --show-utility-calls to include them".to_string(),
                ]
            },
            limits: vec![
                "trail not truncated; max_nodes=24 omitted_edge_count=0".to_string(),
            ],
        }
    }

    fn sample_search_hit() -> crate::args::SearchHitOutput {
        crate::args::SearchHitOutput {
            number: Some(1),
            node_id: "node-build-packet".to_string(),
            node_ref: Some("src/lib.rs:7:build_packet".to_string()),
            display_name: "build_packet".to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some("C:/repo/src/lib.rs".to_string()),
            line: Some(7),
            score: 0.91,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.7,
                semantic: 0.1,
                graph: 0.11,
                total: 0.91,
            }),
            duplicate_of: None,
            excerpt: None,
            why: vec![
                "matched symbol name and semantic evidence".to_string(),
                "can be passed to symbol, trail, snippet, explore, or ask as a focus id"
                    .to_string(),
            ],
        }
    }

    fn sample_agent_answer_with_annotations(annotations: Vec<String>) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "answer-1".to_string(),
            prompt: "How does this repo fit together?".to_string(),
            summary: "The repository is described from indexed evidence.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Use the evidence packet and citations.".to_string(),
                }],
            }],
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "request-1".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Investigate,
                policy_mode: AgentRetrievalPolicyModeDto::CompletenessFirst,
                total_latency_ms: 15,
                sla_target_ms: Some(500),
                sla_missed: false,
                annotations,
                steps: Vec::new(),
            },
        }
    }

    #[test]
    fn ask_markdown_labels_repo_explain_modes() {
        let db_first =
            sample_agent_answer_with_annotations(vec!["mode=repo_explain_db_first".to_string()]);
        let db_markdown = render_agent_answer_markdown(Path::new("C:/repo"), &db_first);
        assert!(
            db_markdown
                .contains("mode: DB-first repo explanation packet assembled from indexed evidence"),
            "{db_markdown}"
        );
    }

    #[test]
    fn ask_markdown_contract_includes_evidence_packet_shape() {
        let answer = AgentAnswerDto {
            answer_id: "answer-1".to_string(),
            prompt: "How does packet output work?".to_string(),
            summary: "Packet output is assembled from retrieved CLI evidence.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Use the output renderer and keep claims tied to citations."
                        .to_string(),
                }],
            }],
            citations: vec![AgentCitationDto {
                node_id: NodeId("node-render".to_string()),
                display_name: "render_agent_answer_markdown".to_string(),
                kind: NodeKind::FUNCTION,
                file_path: Some("C:/repo/src/output.rs".to_string()),
                line: Some(552),
                score: 0.87,
                origin: SearchHitOrigin::IndexedSymbol,
                resolvable: true,
                subgraph_id: None,
                evidence_edge_ids: Vec::new(),
                retrieval_score_breakdown: None,
            }],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "request-1".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 15,
                sla_target_ms: Some(500),
                sla_missed: false,
                annotations: vec!["semantic retrieval ready".to_string()],
                steps: vec![AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Ok,
                    duration_ms: 4,
                    input: Vec::new(),
                    output: Vec::new(),
                    message: Some("checked indexed symbols".to_string()),
                }],
            },
        };

        let markdown = render_agent_answer_markdown(Path::new("C:/repo"), &answer);

        assert_evidence_packet_shape(&markdown, &["summary:", "answer:"]);
        assert_order(&markdown, "confidence:", "what_was_checked:");
        assert_order(&markdown, "what_was_checked:", "gaps_uncertainty:");
        assert_order(&markdown, "gaps_uncertainty:", "citations:");
        assert!(
            !markdown.contains("request_id="),
            "ask markdown should keep raw request ids in JSON/bundles:\n{markdown}"
        );
        assert!(
            !markdown.contains("checked indexed symbols"),
            "ask markdown should summarize normal step messages instead of dumping trace detail:\n{markdown}"
        );
    }

    #[test]
    fn search_why_markdown_contract_includes_evidence_packet_shape() {
        let output = crate::args::SearchOutput {
            query: "packet output".to_string(),
            retrieval: sample_retrieval(),
            freshness: None,
            limit_per_source: 1,
            repo_text_mode: crate::args::RepoTextMode::Auto,
            repo_text_enabled: true,
            explain: true,
            query_hints: vec![
                "codestory-cli ask --project C:/repo \"How does packet output work?\"".to_string(),
            ],
            suggestions: Vec::new(),
            indexed_symbol_hits: vec![sample_search_hit()],
            repo_text_hits: Vec::new(),
            repo_text_stats: Some(RepoTextScanStatsDto {
                scanned_file_count: 12,
                scanned_byte_count: 4096,
                skipped_large_file_count: 1,
                file_cap: 2000,
                byte_cap: 33_554_432,
                time_cap_ms: 500,
                duration_ms: 7,
                truncated: false,
                reason: None,
                action: None,
            }),
        };

        let markdown = render_search_markdown(Path::new("C:/repo"), &output);

        assert_evidence_packet_shape(&markdown, &["short_finding:", "summary:"]);
        assert_order(&markdown, "short_finding:", "confidence:");
        assert!(
            markdown.contains("repo text scan caps: files=12/2000 bytes=4096/33554432"),
            "{markdown}"
        );
        assert!(
            !markdown.contains("query_hints:"),
            "search --why should not duplicate packet next_commands as legacy query_hints:\n{markdown}"
        );
        assert!(
            !markdown.contains("why:"),
            "search --why should not duplicate packet evidence as legacy per-hit why lines:\n{markdown}"
        );
    }

    #[test]
    fn ground_why_markdown_contract_includes_evidence_packet_shape() {
        let snapshot = GroundingSnapshotDto {
            root: "C:/repo".to_string(),
            budget: GroundingBudgetDto::Balanced,
            generated_at_epoch_ms: 0,
            stats: sample_storage_stats(),
            retrieval: Some(sample_retrieval()),
            coverage: GroundingCoverageDto {
                total_files: 1,
                represented_files: 1,
                total_symbols: 1,
                represented_symbols: 1,
                compressed_files: 0,
            },
            root_symbols: vec![GroundingSymbolDigestDto {
                id: NodeId("node-build-packet".to_string()),
                node_ref: Some("src/lib.rs:7:build_packet".to_string()),
                label: "build_packet".to_string(),
                kind: NodeKind::FUNCTION,
                line: Some(7),
                member_count: None,
                summary: Some("Builds the evidence packet.".to_string()),
                edge_digest: Vec::new(),
            }],
            files: vec![GroundingFileDigestDto {
                file_path: "C:/repo/src/lib.rs".to_string(),
                language: Some("rust".to_string()),
                symbol_count: 1,
                represented_symbol_count: 1,
                compressed: false,
                symbols: Vec::new(),
            }],
            coverage_buckets: Vec::new(),
            notes: vec!["No fallback was needed.".to_string()],
            recommended_queries: vec![
                "codestory-cli search --project C:/repo --query packet --why".to_string(),
            ],
        };

        let markdown = render_ground_markdown(Path::new("C:/repo"), &snapshot, true);

        assert_evidence_packet_shape(&markdown, &["short_finding:", "summary:"]);
        assert_order(&markdown, "short_finding:", "confidence:");
        assert!(
            !markdown.contains("why:"),
            "ground --why should use the redesigned packet instead of the legacy why block:\n{markdown}"
        );
        assert!(
            !markdown.contains("recommended_queries:"),
            "ground --why should not duplicate packet next_commands as legacy recommended_queries:\n{markdown}"
        );
    }

    #[test]
    fn ask_markdown_surfaces_low_confidence_trace_gaps() {
        let answer = AgentAnswerDto {
            answer_id: "answer-1".to_string(),
            prompt: "Where did retrieval fail?".to_string(),
            summary: "Retrieval was incomplete.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "The answer is limited by skipped source reads.".to_string(),
                }],
            }],
            citations: vec![AgentCitationDto {
                node_id: NodeId("node-weak".to_string()),
                display_name: "weak_hit".to_string(),
                kind: NodeKind::FUNCTION,
                file_path: Some("C:/repo/src/lib.rs".to_string()),
                line: Some(9),
                score: 0.21,
                origin: SearchHitOrigin::IndexedSymbol,
                resolvable: true,
                subgraph_id: None,
                evidence_edge_ids: Vec::new(),
                retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                    lexical: 0.1,
                    semantic: 0.06,
                    graph: 0.05,
                    total: 0.21,
                }),
            }],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "request-low".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Investigate,
                policy_mode: AgentRetrievalPolicyModeDto::CompletenessFirst,
                total_latency_ms: 650,
                sla_target_ms: Some(500),
                sla_missed: true,
                annotations: vec!["weak hits after fallback".to_string()],
                steps: vec![
                    AgentRetrievalStepDto {
                        kind: AgentRetrievalStepKindDto::Search,
                        status: AgentRetrievalStepStatusDto::Ok,
                        duration_ms: 4,
                        input: Vec::new(),
                        output: Vec::new(),
                        message: Some("normal search detail".to_string()),
                    },
                    AgentRetrievalStepDto {
                        kind: AgentRetrievalStepKindDto::SourceRead,
                        status: AgentRetrievalStepStatusDto::Skipped,
                        duration_ms: 1,
                        input: Vec::new(),
                        output: Vec::new(),
                        message: Some("source reads skipped by budget".to_string()),
                    },
                ],
            },
        };

        let markdown = render_agent_answer_markdown(Path::new("C:/repo"), &answer);

        assert!(markdown.contains("confidence: low"), "{markdown}");
        assert!(
            markdown.contains("retrieval SLA missed: latency_ms=650 target_ms=500"),
            "{markdown}"
        );
        assert!(
            markdown.contains("trace annotation: weak hits after fallback"),
            "{markdown}"
        );
        assert!(
            markdown.contains("retrieval step issue: source_read status=skipped"),
            "{markdown}"
        );
        assert!(
            !markdown.contains("normal search detail"),
            "normal step messages should stay in JSON/bundles:\n{markdown}"
        );
    }

    #[test]
    fn search_why_markdown_surfaces_fallback_and_zero_hits() {
        let mut retrieval = sample_retrieval();
        retrieval.semantic_ready = false;
        retrieval.semantic_doc_count = 0;
        retrieval.fallback_reason = Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime);
        retrieval.fallback_message = Some("embedding runtime unavailable".to_string());
        let output = crate::args::SearchOutput {
            query: "packet output".to_string(),
            retrieval,
            freshness: None,
            limit_per_source: 1,
            repo_text_mode: crate::args::RepoTextMode::Off,
            repo_text_enabled: false,
            explain: true,
            query_hints: Vec::new(),
            suggestions: Vec::new(),
            indexed_symbol_hits: Vec::new(),
            repo_text_hits: Vec::new(),
            repo_text_stats: None,
        };

        let markdown = render_search_markdown(Path::new("C:/repo"), &output);

        assert!(markdown.contains("confidence: low"), "{markdown}");
        assert!(
            markdown.contains("retrieval fallback: missing_embedding_runtime"),
            "{markdown}"
        );
        assert!(
            markdown.contains("retrieval note: embedding runtime unavailable"),
            "{markdown}"
        );
        assert!(
            markdown.contains("semantic retrieval is not ready"),
            "{markdown}"
        );
        assert!(
            markdown.contains("no indexed symbol or repo-text hits matched"),
            "{markdown}"
        );
        assert!(
            markdown.contains("repo text fallback was disabled"),
            "{markdown}"
        );
        assert!(markdown.contains("citations:\n- none"), "{markdown}");
    }

    #[test]
    fn ground_why_markdown_surfaces_fallback_and_partial_coverage() {
        let mut retrieval = sample_retrieval();
        retrieval.semantic_ready = false;
        retrieval.semantic_doc_count = 0;
        retrieval.fallback_reason = Some(RetrievalFallbackReasonDto::MissingSemanticDocs);
        let snapshot = GroundingSnapshotDto {
            root: "C:/repo".to_string(),
            budget: GroundingBudgetDto::Strict,
            generated_at_epoch_ms: 0,
            stats: StorageStatsDto {
                node_count: 1,
                edge_count: 0,
                file_count: 4,
                error_count: 2,
            },
            retrieval: Some(retrieval),
            coverage: GroundingCoverageDto {
                total_files: 4,
                represented_files: 1,
                total_symbols: 5,
                represented_symbols: 2,
                compressed_files: 1,
            },
            root_symbols: Vec::new(),
            files: vec![GroundingFileDigestDto {
                file_path: "C:/repo/src/lib.rs".to_string(),
                language: Some("rust".to_string()),
                symbol_count: 5,
                represented_symbol_count: 2,
                compressed: true,
                symbols: Vec::new(),
            }],
            coverage_buckets: Vec::new(),
            notes: Vec::new(),
            recommended_queries: Vec::new(),
        };

        let markdown = render_ground_markdown(Path::new("C:/repo"), &snapshot, true);

        assert!(markdown.contains("confidence: low"), "{markdown}");
        assert!(markdown.contains("index reported 2 errors"), "{markdown}");
        assert!(
            markdown.contains("represented files are partial: 1/4"),
            "{markdown}"
        );
        assert!(
            markdown.contains("represented symbols are partial: 2/5"),
            "{markdown}"
        );
        assert!(markdown.contains("1 files are compressed"), "{markdown}");
        assert!(
            markdown.contains("retrieval fallback: missing_semantic_docs"),
            "{markdown}"
        );
        assert!(
            markdown.contains("semantic retrieval is not ready"),
            "{markdown}"
        );
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
    fn trail_story_markdown_includes_required_sections_and_textual_uncertainty() {
        let project_root = Path::new("C:/repo");
        let context = sample_story_trail_context();
        let cmd = sample_trail_command(true);
        let story = sample_trail_story(true);
        let markdown = render_trail_story_markdown(
            project_root,
            &sample_resolved_target(),
            &context,
            &cmd,
            &story,
        );

        assert_order(&markdown, "# Trail Story", "## Entry Points");
        assert_order(&markdown, "## Entry Points", "## Core Flow");
        assert_order(&markdown, "## Core Flow", "## Side Effects");
        assert_order(&markdown, "## Side Effects", "## Uncertainty");
        assert_order(&markdown, "## Uncertainty", "## Tests");
        assert!(markdown.contains("handle_request [function]"));
        assert!(markdown.contains("validate_request"));
        assert!(markdown.contains("write_audit_log"));
        assert!(markdown.contains("certainty=certain"));
        assert!(markdown.contains("certainty=probable"));
        assert!(markdown.contains("certainty=uncertain"));
        assert!(
            markdown.contains("candidate_targets=1"),
            "uncertainty should explain why a low-confidence edge remains textual:\n{markdown}"
        );
    }

    #[test]
    fn trail_story_markdown_matches_stable_snapshot() {
        let project_root = Path::new("C:/repo");
        let context = sample_story_trail_context();
        let cmd = sample_trail_command(true);
        let story = sample_trail_story(true);
        let markdown = render_trail_story_markdown(
            project_root,
            &sample_resolved_target(),
            &context,
            &cmd,
            &story,
        );

        let expected = r#"# Trail Story
resolved_query: `handle_request` -> [handle] handle_request [function] src/request.rs:10 score=1.00 origin=indexed_symbol ref=`src/request.rs:10:handle_request`
focus: [handle] handle_request [function] src/request.rs:10
summary: Story trail around `handle_request` found 6 nodes and 5 edges; mode=neighborhood direction=both tests=included utility_calls=hidden truncated=false.

## Entry Points
- focus: handle_request [function] `src/request.rs`
- entry: test_request_flow [function] `tests/request_flow.rs`

## Core Flow
- [edge-validate] handle_request [function] `src/request.rs` calls validate_request [function] `src/request.rs` (certainty=certain). certain call edge confidence=0.99
- [edge-profile] validate_request [function] `src/request.rs` calls load_profile [function] `src/profile.rs` (certainty=probable). probable call edge confidence=0.72
- [edge-hook] handle_request [function] `src/request.rs` calls dynamic_plugin_hook [function] `src/plugin.rs` (certainty=uncertain). uncertain call edge confidence=0.32 candidate_targets=1
- [edge-speculative] handle_request [function] `src/request.rs` calls experimental_hook [function] `src/plugin.rs` (certainty=speculative). speculative call edge confidence=0.21
- [edge-missing] handle_request [function] `src/request.rs` calls legacy_dispatch [function] `src/legacy.rs` (certainty=missing certainty metadata). missing certainty metadata call edge

## Side Effects
- possible side-effect candidate [edge-audit] handle_request [function] `src/request.rs` calls write_audit_log [function] `src/audit.rs` (certainty=certain)

## Uncertainty
- [edge-profile] validate_request [function] `src/request.rs` calls load_profile [function] `src/profile.rs` is probable. probable call edge confidence=0.72
- [edge-hook] handle_request [function] `src/request.rs` calls dynamic_plugin_hook [function] `src/plugin.rs` is uncertain. uncertain call edge confidence=0.32 candidate_targets=1
- [edge-speculative] handle_request [function] `src/request.rs` calls experimental_hook [function] `src/plugin.rs` is speculative. speculative call edge confidence=0.21
- [edge-missing] handle_request [function] `src/request.rs` calls legacy_dispatch [function] `src/legacy.rs` is missing certainty metadata. missing certainty metadata call edge

## Tests
- tests and benches included by --include-tests
- 1 test-like node(s) present: test_request_flow [function] `tests/request_flow.rs`
- utility/helper calls hidden by default; pass --show-utility-calls to include them

## Gaps And Limits
- trail not truncated; max_nodes=24 omitted_edge_count=0
"#;

        assert_eq!(markdown, expected);
    }

    #[test]
    fn trail_story_reports_side_effects_and_test_scope() {
        let included = sample_trail_story(true);
        assert!(
            included
                .side_effects
                .iter()
                .any(|item| item.contains("write_audit_log")),
            "story should name likely side-effect calls: {included:#?}"
        );
        assert!(
            included
                .test_scope
                .iter()
                .any(|item| item.contains("tests and benches included")),
            "include-tests story should say tests are included: {included:#?}"
        );
        assert!(
            included
                .test_scope
                .iter()
                .any(|item| item.contains("test_request_flow")),
            "include-tests story should name rendered test-like nodes: {included:#?}"
        );

        let excluded = sample_trail_story(false);
        assert!(
            excluded
                .test_scope
                .iter()
                .any(|item| item.contains("tests and benches excluded")),
            "production-scope story should say tests are excluded: {excluded:#?}"
        );
    }

    #[test]
    fn trail_story_handles_single_node_without_edges() {
        let story = TrailStoryDto {
            summary: "Story trail around `A` found 1 node and 0 edges; mode=neighborhood direction=both tests=excluded utility_calls=hidden truncated=false.".to_string(),
            entry_points: vec![
                "focus: A [function]".to_string(),
                "no graph entry edges were returned for this focus".to_string(),
            ],
            core_flow: Vec::new(),
            side_effects: vec![
                "none detected from conservative edge-kind and target-name heuristics; inspect snippets for runtime effects".to_string(),
            ],
            uncertainty: vec!["no rendered trail edges to evaluate for certainty".to_string()],
            test_scope: vec![
                "tests and benches excluded by default production-only scope; pass --include-tests to include them".to_string(),
                "no test-like nodes are present in the rendered trail".to_string(),
            ],
            limits: vec![
                "trail not truncated; max_nodes=24 omitted_edge_count=0".to_string(),
                "no edges were returned, so core flow is limited to the focus node".to_string(),
            ],
        };

        assert!(story.core_flow.is_empty());
        assert!(
            story
                .entry_points
                .iter()
                .any(|item| item.contains("no graph entry edges"))
        );
        assert!(
            story
                .limits
                .iter()
                .any(|item| item.contains("no edges were returned"))
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
            story: None,
        };

        let dot = render_trail_dot(Path::new("C:/repo"), &context);

        assert!(dot.contains("\"a\" [label=\"A\\n[function]\"];"));
        assert!(dot.contains("\"a\" -> \"b\" [label=\"call\"];"));
    }
}
