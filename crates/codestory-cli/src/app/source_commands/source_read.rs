use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::rendering::build_query_resolution_output_with_runtime;
use super::super::resolution::{quote_command_value, resolve_source_target_or_emit_ambiguity};
use super::affected_rendering::render_files_markdown;
use crate::args;
use crate::args::{
    FilesCommand, ProjectArgs, QueryCommand, QueryOutput, SnippetCommand, SnippetJsonOutput,
};
use crate::explore;
use crate::output::{
    RenderedPublicOutput, emit_public_operation, render_query_markdown, render_snippet_markdown,
};
use crate::runtime;
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error};
use anyhow::{Context, Result, bail};
use codestory_contracts::api::IndexedFilesRequest;
use std::io::IsTerminal;

pub(in crate::app) fn run_snippet(cmd: SnippetCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "snippet")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "snippet")?;

    let file_filter = cmd.target.file_filter();
    let operation = if cmd.target.query.is_some() {
        "graph_assisted"
    } else {
        "graph"
    };
    let colorize = cmd.format == args::OutputFormat::Markdown
        && cmd.output_file.is_none()
        && std::io::stdout().is_terminal();
    let operation = runtime.run_public_operation(operation, || {
        let target = resolve_source_target_or_emit_ambiguity(
            &runtime,
            cmd.target.selection()?,
            file_filter.as_deref(),
            cmd.format,
            cmd.output_file.as_deref(),
        )?;
        let target = if cmd.function_body {
            runtime::prefer_function_body_target(&runtime.project_root, target)
        } else {
            target
        };
        let context = if cmd.function_body {
            runtime
                .browser
                .snippet_function_body_context(target.selected.node_id.clone(), cmd.context)
        } else {
            runtime
                .browser
                .snippet_context(target.selected.node_id.clone(), cmd.context)
        }
        .map_err(map_api_error)?;
        let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
        let verification_targets = resolution.resolved.verification_targets.clone();
        let markdown = render_snippet_markdown(
            &runtime.project_root,
            &target,
            &context,
            colorize,
            &verification_targets,
        );
        let output = SnippetJsonOutput {
            resolution,
            snippet: &context,
            verification_targets,
        };
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(in crate::app) fn run_query(cmd: QueryCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "query")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    if let Some(sql) = cmd.sql.as_deref() {
        bail!(
            "CodeStory `query` uses the graph-query DSL, not SQL. \
             Use syntax like `search(query: 'AppController') | limit(5)` or \
             `trail(symbol: 'AppController') | filter(kind: function)`. \
             For raw symbol discovery, use `search --query {}`. \
             Unsupported SQL received: {}",
            quote_command_value(sql),
            sql
        );
    }
    let query = cmd
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .context("Query cannot be empty.")?;
    let ast =
        codestory_runtime::parse_graph_query(query).map_err(|error| anyhow::anyhow!("{error}"))?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "query")?;
    let operation = runtime.run_public_operation("graph", || {
        let items = runtime
            .browser
            .query(&ast)
            .map_err(map_api_error)?
            .iter()
            .map(|item| explore::browser_query_item_to_output(&runtime.project_root, item))
            .collect();
        let output = QueryOutput {
            query: query.to_string(),
            ast: ast.clone(),
            items,
        };
        let markdown = render_query_markdown(&output);
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(in crate::app) fn run_files(cmd: FilesCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "files")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let project = ProjectArgs {
        project: cmd.project.clone(),
        cache_dir: cmd.cache_dir.clone(),
    };
    let runtime = RuntimeContext::new(&project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "files")?;
    let operation = runtime.run_public_operation("graph", || {
        let output = runtime
            .browser
            .indexed_files(IndexedFilesRequest {
                path_contains: cmd.path.clone(),
                language: cmd.language.clone(),
                role: cmd.role.map(Into::into),
                limit: Some(cmd.limit),
            })
            .map_err(map_api_error)?;
        let markdown = render_files_markdown(&output);
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}
