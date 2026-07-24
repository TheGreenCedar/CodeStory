use anyhow::{Context, Result, bail};
use codestory_contracts::api::{
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    AffectedFollowUpInvocationDto, CommandFailureEnvelope, FrameworkRouteCoverageDto,
    IndexedFilesRequest, TrailContextDto,
};
use std::{
    fmt::Write as _,
    io::{IsTerminal, Read},
};

use crate::{
    args::{
        self, AffectedChangeSource, AffectedCommand, AffectedStdinFormat, CliDirection,
        CliTrailMode, FilesCommand, ProjectArgs, QueryCommand, QueryOutput, QueryResolutionOutput,
        SnippetCommand, SnippetJsonOutput, SymbolCommand, SymbolJsonOutput, SymbolWorkflowCommand,
        TrailCommand, TrailJsonOutput, build_trail_request,
    },
    explore,
    output::{
        RenderedPublicOutput, emit_public_operation, render_query_markdown,
        render_snippet_markdown, render_symbol_markdown, render_symbol_mermaid, render_trail_dot,
        render_trail_markdown, render_trail_mermaid, render_trail_story_markdown,
    },
    runtime::{self, AmbiguousTargetError, RuntimeContext, ensure_index_ready, map_api_error},
};

use super::{
    artifacts::{ensure_dot_only_for_trail, preflight_output_file},
    rendering::{
        build_query_resolution_output_from_occurrences, build_query_resolution_output_with_runtime,
    },
    resolution::{
        StructuredCommandFailure, command_failure_envelope, quote_command_argument_value,
        quote_command_value, resolve_source_target_or_emit_ambiguity,
        resolve_target_or_emit_ambiguity, structured_ambiguous_target_failure,
    },
};

pub(super) fn run_symbol(cmd: SymbolCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "symbol")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "symbol")?;

    let file_filter = cmd.target.file_filter();
    let operation = if cmd.target.query.is_some() {
        "graph_assisted"
    } else {
        "graph"
    };
    let operation = runtime.run_public_operation(operation, || {
        let target = resolve_target_or_emit_ambiguity(
            &runtime,
            cmd.target.selection()?,
            file_filter.as_deref(),
            cmd.format,
            cmd.output_file.as_deref(),
        )?;
        let context = runtime
            .browser
            .symbol_context(target.selected.node_id.clone())
            .map_err(map_api_error)?;
        let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
        if cmd.mermaid {
            return Ok(RenderedPublicOutput::text(render_symbol_mermaid(&context)));
        }
        let verification_targets = resolution.resolved.verification_targets.clone();
        let markdown = render_symbol_markdown(
            &runtime.project_root,
            &target,
            &context,
            &verification_targets,
        );
        let output = SymbolJsonOutput {
            resolution,
            symbol: &context,
            verification_targets,
        };
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

#[derive(serde::Serialize)]
pub(super) struct SymbolWorkflowOutput<'a> {
    workflow: &'static str,
    project_root: &'a str,
    resolution: QueryResolutionOutput,
    symbol: &'a codestory_contracts::api::SymbolContextDto,
    direct_callers: &'a [codestory_runtime::SymbolWorkflowNode],
    transitive_callers: &'a [codestory_runtime::SymbolWorkflowNode],
    impacted_files: &'a [String],
    impacted_routes: &'a [codestory_runtime::SymbolWorkflowRoute],
    likely_tests: &'a [codestory_runtime::SymbolWorkflowTest],
    caps: &'a codestory_runtime::SymbolWorkflowCaps,
    unknowns: &'a [String],
    next_commands: &'a [String],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    affected: Option<&'a codestory_contracts::api::AffectedAnalysisDto>,
    trail: &'a TrailContextDto,
}

pub(super) fn run_symbol_workflow(
    mode: codestory_runtime::SymbolWorkflowMode,
    cmd: SymbolWorkflowCommand,
) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, mode.label())?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, mode.label())?;

    let file_filter = cmd.target.file_filter();
    let operation_name = if cmd.target.query.is_some() {
        "graph_assisted"
    } else {
        "graph"
    };
    let operation = runtime.run_public_operation(operation_name, || {
        let target = match cmd.target.selection()? {
            args::TargetSelection::Id(id) => codestory_runtime::TargetSelection::Id(id),
            args::TargetSelection::Query { query, choose } => {
                codestory_runtime::TargetSelection::Query { query, choose }
            }
        };
        let response = match runtime
            .browser
            .symbol_workflow(codestory_runtime::SymbolWorkflowRequest {
                mode,
                target,
                file_filter: file_filter.clone(),
                depth: cmd.depth,
                max_nodes: cmd.max_nodes,
                include_tests: cmd.include_tests,
            })
            .map_err(map_api_error)?
        {
            codestory_runtime::SymbolWorkflowOutcome::Complete(response) => *response,
            codestory_runtime::SymbolWorkflowOutcome::Ambiguous(ambiguous) => {
                return structured_ambiguous_target_failure(
                    &runtime,
                    AmbiguousTargetError {
                        query: ambiguous.query,
                        file_filter: ambiguous.file_filter,
                        alternatives: ambiguous.alternatives,
                        message: ambiguous.message,
                    },
                    cmd.format,
                    cmd.output_file.as_deref(),
                );
            }
            codestory_runtime::SymbolWorkflowOutcome::Rejected(message) => bail!(message),
        };
        let resolution_target =
            runtime::ResolvedTarget::from_runtime(response.resolution.target.clone());
        let resolution = build_query_resolution_output_from_occurrences(
            &runtime.project_root,
            &resolution_target,
            &response.resolution.occurrences,
        );
        let output = SymbolWorkflowOutput {
            workflow: response.workflow,
            project_root: &response.project_root,
            resolution,
            symbol: &response.symbol,
            direct_callers: &response.direct_callers,
            transitive_callers: &response.transitive_callers,
            impacted_files: &response.impacted_files,
            impacted_routes: &response.impacted_routes,
            likely_tests: &response.likely_tests,
            caps: &response.caps,
            unknowns: &response.unknowns,
            next_commands: &response.next_commands,
            affected: response.affected.as_ref(),
            trail: &response.trail,
        };
        let markdown = render_symbol_workflow_markdown(mode, &output);
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(super) fn render_symbol_workflow_markdown(
    mode: codestory_runtime::SymbolWorkflowMode,
    output: &SymbolWorkflowOutput<'_>,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# {}", mode.title());
    let _ = writeln!(
        markdown,
        "symbol: {} [{}]",
        output.symbol.node.display_name, output.symbol.node.id.0
    );
    if let Some(path) = output.symbol.node.file_path.as_deref() {
        let line = output
            .symbol
            .node
            .start_line
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        let _ = writeln!(markdown, "source: {path}{line}");
    }
    let _ = writeln!(
        markdown,
        "caps: caller_depth={} caller_max_nodes={} affected_depth={} impacted_symbols<=200 impacted_routes<=100",
        output.caps.caller_depth, output.caps.caller_max_nodes, output.caps.affected_depth
    );

    append_symbol_workflow_nodes(&mut markdown, "direct_callers", output.direct_callers);
    append_symbol_workflow_nodes(
        &mut markdown,
        "transitive_callers",
        output.transitive_callers,
    );
    append_symbol_workflow_strings(&mut markdown, "impacted_files", output.impacted_files);

    let _ = writeln!(markdown, "impacted_routes:");
    if output.impacted_routes.is_empty() {
        let _ = writeln!(markdown, "- none");
    } else {
        for route in output.impacted_routes {
            let location = route
                .file_path
                .as_deref()
                .map(|path| {
                    route
                        .line
                        .map(|line| format!(" {path}:{line}"))
                        .unwrap_or_else(|| format!(" {path}"))
                })
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- {} {} -> {} [{}]{}",
                route.method, route.path, route.display_name, route.confidence, location
            );
            let _ = writeln!(markdown, "  reason: {}", route.reason);
        }
    }

    let _ = writeln!(markdown, "likely_tests:");
    if output.likely_tests.is_empty() {
        let _ = writeln!(markdown, "- none");
    } else {
        for test in output.likely_tests {
            let _ = writeln!(
                markdown,
                "- {} confidence={} graph_depth={} impacted_symbols={}",
                test.path, test.confidence, test.graph_depth, test.impacted_symbol_count
            );
            let _ = writeln!(markdown, "  reason: {}", test.reason);
        }
    }

    append_symbol_workflow_strings(&mut markdown, "unknowns", output.unknowns);
    append_symbol_workflow_strings(&mut markdown, "next_commands", output.next_commands);
    markdown
}

pub(super) fn append_symbol_workflow_nodes(
    markdown: &mut String,
    label: &str,
    nodes: &[codestory_runtime::SymbolWorkflowNode],
) {
    let _ = writeln!(markdown, "{label}:");
    if nodes.is_empty() {
        let _ = writeln!(markdown, "- none");
        return;
    }
    for node in nodes {
        let location = node
            .file_path
            .as_deref()
            .map(|path| format!(" {path}"))
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- [{}] {} ({}) depth={}{}",
            node.node_id.0, node.display_name, node.kind, node.depth, location
        );
    }
}

pub(super) fn append_symbol_workflow_strings(markdown: &mut String, label: &str, items: &[String]) {
    let _ = writeln!(markdown, "{label}:");
    if items.is_empty() {
        let _ = writeln!(markdown, "- none");
        return;
    }
    for item in items {
        let _ = writeln!(markdown, "- {item}");
    }
}

pub(super) fn run_trail(cmd: TrailCommand) -> Result<()> {
    preflight_output_file(cmd.output_file.as_deref())?;
    if cmd.story && cmd.mermaid {
        bail!("--story cannot be combined with --mermaid; use markdown or json output");
    }
    if cmd.story && cmd.format == args::OutputFormat::Dot {
        bail!("--story cannot be combined with --format dot; use markdown or json output");
    }
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "trail")?;

    let file_filter = cmd.target.file_filter();
    let operation = if cmd.target.query.is_some() {
        "graph_assisted"
    } else {
        "graph"
    };
    let operation = runtime.run_public_operation(operation, || {
        let target = resolve_target_or_emit_ambiguity(
            &runtime,
            cmd.target.selection()?,
            file_filter.as_deref(),
            cmd.format,
            cmd.output_file.as_deref(),
        )?;
        let request = build_trail_request(&target.selected.node_id, &cmd);
        let context = runtime
            .browser
            .trail_context(request)
            .map_err(map_api_error)?;
        let resolution = build_query_resolution_output_with_runtime(&runtime, &target);
        if cmd.mermaid {
            return Ok(RenderedPublicOutput::text(render_trail_mermaid(&context)));
        }
        if cmd.format == args::OutputFormat::Dot {
            return Ok(RenderedPublicOutput::text(render_trail_dot(
                &runtime.project_root,
                &context,
            )));
        }
        let notes = trail_guidance_notes(&context);
        let mut markdown = if let Some(story) = context.story.as_ref() {
            render_trail_story_markdown(&runtime.project_root, &target, &context, &cmd, story)
        } else {
            render_trail_markdown(&runtime.project_root, &target, &context, &cmd)
        };
        if !notes.is_empty() {
            let _ = writeln!(markdown, "notes:");
            for note in &notes {
                let _ = writeln!(markdown, "- {note}");
            }
        }
        let output = TrailJsonOutput {
            resolution,
            trail: &context,
            notes,
        };
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(super) fn run_callers(mut cmd: TrailCommand) -> Result<()> {
    cmd.mode = CliTrailMode::Referencing;
    cmd.direction = Some(CliDirection::Incoming);
    run_trail(cmd)
}

pub(super) fn run_callees(mut cmd: TrailCommand) -> Result<()> {
    cmd.mode = CliTrailMode::Referenced;
    cmd.direction = Some(CliDirection::Outgoing);
    run_trail(cmd)
}

pub(super) fn run_trace(mut cmd: TrailCommand) -> Result<()> {
    if !cmd.mermaid && cmd.format != args::OutputFormat::Dot {
        cmd.story = true;
    }
    run_trail(cmd)
}

pub(super) fn trail_guidance_notes(
    context: &codestory_contracts::api::TrailContextDto,
) -> Vec<String> {
    if !context.trail.edges.is_empty() || context.trail.nodes.len() > 1 {
        return Vec::new();
    }
    if context.focus.file_path.is_none() {
        return Vec::new();
    }
    vec![format!(
        "No graph edges were indexed for `{}`. For object/config exports, use `snippet --id {}` or `explore --id {}` to inspect fields, hooks, access rules, and imports.",
        context.focus.display_name, context.focus.id.0, context.focus.id.0
    )]
}

pub(super) fn run_snippet(cmd: SnippetCommand) -> Result<()> {
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

pub(super) fn run_query(cmd: QueryCommand) -> Result<()> {
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

pub(super) fn run_files(cmd: FilesCommand) -> Result<()> {
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

pub(super) fn run_affected(cmd: AffectedCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "affected")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "affected")?;
    let change_records =
        affected_change_records(&cmd).map_err(|error| affected_discovery_error(&cmd, error))?;
    let operation = runtime.run_observational_public_operation("affected", || {
        let output = runtime
            .browser
            .affected_analysis(AffectedAnalysisRequest {
                input: AffectedAnalysisInput::ChangeRecords(change_records.clone()),
                depth: Some(cmd.depth),
                filter: cmd.filter.clone(),
            })
            .map_err(map_api_error)?;
        let markdown = render_affected_markdown(&output);
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(super) fn affected_change_records(
    cmd: &AffectedCommand,
) -> Result<Vec<AffectedChangeRecordDto>> {
    let mut records = cmd
        .paths
        .iter()
        .map(|path| affected_path_record(path, AffectedChangeKindDto::Unknown, "path"))
        .collect::<Vec<_>>();
    if cmd.stdin {
        let mut input = Vec::new();
        std::io::stdin()
            .read_to_end(&mut input)
            .context("Failed to read changed paths from stdin")?;
        let input = path_text_from_bytes(&input, "stdin")?;
        match cmd.stdin_format {
            AffectedStdinFormat::Path => {
                records.extend(input.lines().filter(|line| !line.is_empty()).map(|path| {
                    affected_path_record(path, AffectedChangeKindDto::Unknown, "stdin")
                }))
            }
            AffectedStdinFormat::NameStatus => {
                records.extend(parse_git_name_status_records(&input)?);
            }
        }
    }
    if !records.is_empty() {
        dedupe_affected_change_records(&mut records);
        return Ok(records);
    }
    let output = affected_git_change_output(cmd)?;
    if !output.status.success() {
        bail!(
            "git change discovery failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut records = match cmd.changes {
        AffectedChangeSource::Untracked => parse_git_nul_path_records(
            &output.stdout,
            AffectedChangeKindDto::Untracked,
            "??",
            "git_ls_files",
        )?,
        AffectedChangeSource::Head
        | AffectedChangeSource::Staged
        | AffectedChangeSource::Unstaged => parse_git_name_status_records_z(&output.stdout)?,
    };
    dedupe_affected_change_records(&mut records);
    Ok(records)
}

pub(super) fn affected_git_change_output(cmd: &AffectedCommand) -> Result<std::process::Output> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(&cmd.project.project);
    match cmd.changes {
        AffectedChangeSource::Head => {
            command
                .arg("diff")
                .arg("--name-status")
                .arg("-z")
                .arg("HEAD");
        }
        AffectedChangeSource::Staged => {
            command
                .arg("diff")
                .arg("--cached")
                .arg("--name-status")
                .arg("-z");
        }
        AffectedChangeSource::Unstaged => {
            command.arg("diff").arg("--name-status").arg("-z");
        }
        AffectedChangeSource::Untracked => {
            command
                .arg("ls-files")
                .arg("-z")
                .arg("--others")
                .arg("--exclude-standard");
        }
    }
    command
        .output()
        .context("Failed to run git change discovery")
}

#[derive(Debug)]
pub(super) struct UnsupportedNonUtf8Path {
    source: &'static str,
}

impl std::fmt::Display for UnsupportedNonUtf8Path {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "unsupported_non_utf8_path: {} returned a path that cannot be represented in UTF-8",
            self.source
        )
    }
}

impl std::error::Error for UnsupportedNonUtf8Path {}

pub(super) fn affected_discovery_error(
    cmd: &AffectedCommand,
    error: anyhow::Error,
) -> anyhow::Error {
    let Some(unsupported) = error.downcast_ref::<UnsupportedNonUtf8Path>() else {
        return error;
    };
    StructuredCommandFailure {
        envelope: unsupported_non_utf8_path_envelope(unsupported),
        output_file: cmd.output_file.clone(),
        markdown: None,
    }
    .into()
}

pub(super) fn unsupported_non_utf8_path_envelope(
    error: &UnsupportedNonUtf8Path,
) -> CommandFailureEnvelope {
    command_failure_envelope(
        "unsupported_non_utf8_path",
        "git_change_discovery",
        error.to_string(),
        serde_json::json!({"source": error.source}),
    )
}

pub(super) fn nul_delimited_git_fields(input: &[u8]) -> Result<Vec<&[u8]>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if input.last() != Some(&0) {
        bail!("git NUL-delimited path output is missing its terminator");
    }
    let fields = input[..input.len() - 1]
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>();
    if fields.iter().any(|field| field.is_empty()) {
        bail!("git NUL-delimited path output contains an empty field");
    }
    Ok(fields)
}

pub(super) fn path_text_from_bytes(bytes: &[u8], source: &'static str) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| anyhow::Error::new(UnsupportedNonUtf8Path { source }))
}

pub(super) fn parse_git_nul_path_records(
    input: &[u8],
    kind: AffectedChangeKindDto,
    status: &str,
    source: &'static str,
) -> Result<Vec<AffectedChangeRecordDto>> {
    nul_delimited_git_fields(input)?
        .into_iter()
        .map(|field| {
            path_text_from_bytes(field, source)
                .map(|path| affected_path_record(&path, kind.clone(), status))
        })
        .collect()
}

pub(super) fn parse_git_name_status_records_z(
    input: &[u8],
) -> Result<Vec<AffectedChangeRecordDto>> {
    let fields = nul_delimited_git_fields(input)?;
    let mut records = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = std::str::from_utf8(fields[index])
            .context("git name-status status is not valid UTF-8")?;
        index += 1;
        let kind = affected_change_kind_from_status(status);
        let previous_path = if matches!(
            kind,
            AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied
        ) {
            let field = fields
                .get(index)
                .context("git name-status rename/copy record is missing the previous path")?;
            index += 1;
            Some(path_text_from_bytes(field, "git_name_status")?)
        } else {
            None
        };
        let field = fields
            .get(index)
            .context("git name-status record is missing the path")?;
        index += 1;
        records.push(AffectedChangeRecordDto {
            path: path_text_from_bytes(field, "git_name_status")?,
            kind,
            status: status.to_string(),
            previous_path,
        });
    }
    Ok(records)
}

pub(super) fn parse_git_name_status_records(input: &str) -> Result<Vec<AffectedChangeRecordDto>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(parse_git_name_status_record)
        .collect()
}

pub(super) fn parse_git_name_status_record(line: &str) -> Result<AffectedChangeRecordDto> {
    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() == 1 {
        return Ok(affected_path_record(
            parts[0],
            AffectedChangeKindDto::Unknown,
            "path",
        ));
    }
    let status = parts[0];
    let kind = affected_change_kind_from_status(status);
    let (previous_path, path) = if matches!(
        kind,
        AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied
    ) {
        let previous = parts
            .get(1)
            .copied()
            .filter(|path| !path.is_empty())
            .context("git name-status rename/copy row is missing the previous path")?;
        let current = parts
            .get(2)
            .copied()
            .filter(|path| !path.is_empty())
            .context("git name-status rename/copy row is missing the current path")?;
        (Some(previous.to_string()), current)
    } else {
        let path = parts
            .get(1)
            .copied()
            .filter(|path| !path.is_empty())
            .context("git name-status row is missing the path")?;
        (None, path)
    };
    Ok(AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status: status.to_string(),
        previous_path,
    })
}

pub(super) fn affected_path_record(
    path: &str,
    kind: AffectedChangeKindDto,
    status: &str,
) -> AffectedChangeRecordDto {
    AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status: status.to_string(),
        previous_path: None,
    }
}

pub(super) fn affected_change_kind_from_status(status: &str) -> AffectedChangeKindDto {
    match status.chars().next().unwrap_or_default() {
        'A' => AffectedChangeKindDto::Added,
        'M' | 'T' | 'U' => AffectedChangeKindDto::Modified,
        'D' => AffectedChangeKindDto::Deleted,
        'R' => AffectedChangeKindDto::Renamed,
        'C' => AffectedChangeKindDto::Copied,
        '?' => AffectedChangeKindDto::Untracked,
        _ => AffectedChangeKindDto::Unknown,
    }
}

pub(super) fn dedupe_affected_change_records(records: &mut Vec<AffectedChangeRecordDto>) {
    records.retain(|record| !record.path.is_empty());
    records.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.previous_path.cmp(&right.previous_path))
            .then(left.status.cmp(&right.status))
    });
    records.dedup_by(|left, right| {
        left.path == right.path
            && left.previous_path == right.previous_path
            && left.status == right.status
    });
}

pub(super) fn render_files_markdown(output: &codestory_contracts::api::IndexedFilesDto) -> String {
    let mut markdown = String::new();
    markdown.push_str("# indexed files\n\n");
    render_files_summary(&mut markdown, output);
    render_framework_route_coverage(&mut markdown, output);
    render_source_policy_exclusions(&mut markdown, output);
    render_indexed_file_rows(&mut markdown, output);
    markdown
}

pub(super) fn render_files_summary(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    let status = if output.usable { "usable" } else { "empty" };
    let _ = writeln!(
        markdown,
        "- index: {status}; whole index files: {}; indexed: {}; incomplete: {}; error files: {}; policy exclusions: {}; filtered files: {}; visible rows: {}; truncated: {}",
        output.summary.file_count,
        output.summary.indexed_file_count,
        output.summary.incomplete_file_count,
        output.summary.error_file_count,
        output.summary.policy_exclusion_count,
        output.summary.filtered_file_count,
        output.summary.visible_file_count,
        output.summary.truncated
    );
    if !output.summary.language_counts.is_empty() {
        let languages = output
            .summary
            .language_counts
            .iter()
            .map(|entry| {
                format!(
                    "{}={} [{}; {}]",
                    entry.language, entry.file_count, entry.support_mode, entry.evidence_tier
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- languages: {languages}");
        let claim_labels = output
            .summary
            .language_counts
            .iter()
            .map(|entry| format!("{}={}", entry.language, entry.claim_label))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- language_support_claims: {claim_labels}");
    }
    if !output.summary.incomplete_reason_counts.is_empty() {
        let reasons = output
            .summary
            .incomplete_reason_counts
            .iter()
            .map(|entry| format!("{}={} ({})", entry.reason, entry.file_count, entry.detail))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(markdown, "- incomplete_reasons: {reasons}");
    }
    for note in &output.summary.coverage_notes {
        let _ = writeln!(markdown, "- coverage: {note}");
    }
}

pub(super) fn render_source_policy_exclusions(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    if output.policy_exclusions.is_empty() {
        return;
    }
    markdown.push_str(
        "\nverified policy exclusions (source inventory only; no graph or semantic coverage):\n",
    );
    for exclusion in &output.policy_exclusions {
        let _ = writeln!(
            markdown,
            "- {} ({:?}, {} bytes, {} structural units, policy={} byte_cap={} unit_cap={}, core={}/{})",
            exclusion.path,
            exclusion.role,
            exclusion.observed_size,
            exclusion.observed_unit_count,
            exclusion.policy_version,
            exclusion.byte_cap,
            exclusion.structural_unit_cap,
            exclusion.core_generation_id,
            exclusion.core_run_id,
        );
    }
}

pub(super) fn render_framework_route_coverage(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    if !output.summary.framework_route_coverage.is_empty() {
        markdown.push_str("\nframework route coverage:\n");
        for entry in &output.summary.framework_route_coverage {
            let _ = writeln!(markdown, "{}", framework_route_coverage_row(entry));
        }
    }
}

pub(super) fn framework_route_coverage_row(entry: &FrameworkRouteCoverageDto) -> String {
    format!(
        "- {} ({}) status={} coverage_evidence={} confidence_floor={} handler_link={} promotable={} unsupported={} known_gaps={}",
        entry.framework,
        entry.language,
        entry.status,
        entry.coverage_evidence,
        entry.confidence_floor,
        entry.handler_link_support,
        entry.promotable,
        joined_or_none_recorded(&entry.unsupported_patterns),
        joined_or_none_recorded(&entry.known_gaps)
    )
}

pub(super) fn joined_or_none_recorded(values: &[String]) -> String {
    if values.is_empty() {
        "none recorded".to_string()
    } else {
        values.join("; ")
    }
}

pub(super) fn render_indexed_file_rows(
    markdown: &mut String,
    output: &codestory_contracts::api::IndexedFilesDto,
) {
    markdown.push_str("\nfiles:\n");
    for file in &output.files {
        let markers = [
            (!file.indexed).then_some("not-indexed"),
            (!file.complete).then_some("incomplete"),
            (file.error_count > 0).then_some("errors"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let marker = if markers.is_empty() {
            String::new()
        } else {
            format!(" [{}]", markers.join(", "))
        };
        let _ = writeln!(
            markdown,
            "- {} ({}, {:?}, {} lines){}",
            file.path, file.language, file.role, file.line_count, marker
        );
    }
    if output.summary.truncated {
        markdown.push_str("- ... truncated by limit\n");
    }
}

pub(super) fn render_affected_markdown(
    output: &codestory_contracts::api::AffectedAnalysisDto,
) -> String {
    let mut markdown = String::new();
    markdown.push_str("# affected analysis\n\n");
    render_affected_summary(&mut markdown, output);
    render_affected_matched_files(&mut markdown, output);
    render_affected_routes(&mut markdown, output);
    render_affected_tests(&mut markdown, output);
    render_affected_symbols(&mut markdown, output);
    render_affected_footer(&mut markdown, output);
    markdown
}

pub(super) fn render_affected_summary(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    let _ = writeln!(
        markdown,
        "- matched files: {}; depth: {}; impacted symbols: {}; impacted routes: {}; impacted tests: {}",
        output.matched_file_count,
        output.depth,
        output.impacted_symbols.len(),
        output.impacted_routes.len(),
        output.impacted_tests.len()
    );
    let _ = writeln!(
        markdown,
        "- completeness: complete={} confidence={} direct={} propagated={} uncovered={} unavailable={} truncated={}",
        output.completeness.complete,
        output.completeness.confidence,
        output.completeness.direct_impact_count,
        output.completeness.propagated_impact_count,
        output.completeness.uncovered_input_count,
        output.completeness.unavailable_evidence_count,
        output.completeness.truncated
    );
    let _ = writeln!(
        markdown,
        "- bounds: requested_depth={} maximum_depth={} visited_nodes={} visited_edges={} symbol_limit={} route_limit={}",
        output.bounds.requested_depth,
        output.bounds.maximum_depth,
        output.bounds.visited_node_count,
        output.bounds.visited_edge_count,
        output.bounds.impacted_symbol_limit,
        output.bounds.impacted_route_limit
    );
    if !output.changed_paths.is_empty() {
        markdown.push_str("- changed paths:\n");
        for path in &output.changed_paths {
            let _ = writeln!(markdown, "  - {path}");
        }
    }
    if !output.change_records.is_empty() {
        markdown.push_str("- change records:\n");
        for record in &output.change_records {
            let previous = record
                .previous_path
                .as_deref()
                .map(|path| format!(" previous={path}"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "  - {:?} {} status={}{}",
                record.kind, record.path, record.status, previous
            );
        }
    }
    for note in &output.notes {
        let _ = writeln!(markdown, "- note: {note}");
    }
}

pub(super) fn render_affected_matched_files(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.matched_files.is_empty() {
        markdown.push_str("\nmatched files:\n");
        for file in &output.matched_files {
            let mut markers = Vec::new();
            if !file.complete {
                markers.push("incomplete".to_string());
            }
            if file.error_count > 0 {
                markers.push(format!("errors={}", file.error_count));
            }
            if let Some(kind) = file.change_kind.as_ref() {
                markers.push(format!("change={kind:?}"));
            }
            if let Some(status) = file.change_status.as_deref() {
                markers.push(format!("status={status}"));
            }
            if let Some(previous_path) = file.previous_path.as_deref() {
                markers.push(format!("previous={previous_path}"));
            }
            let marker = if markers.is_empty() {
                String::new()
            } else {
                format!(" ({})", markers.join(", "))
            };
            let _ = writeln!(markdown, "- {} [{:?}]{marker}", file.path, file.role);
        }
    }
    if !output.unmatched_paths.is_empty() {
        markdown.push_str("\nunmatched paths:\n");
        for path in &output.unmatched_paths {
            let mut markers = vec![format!("classification={:?}", path.classification)];
            if let Some(kind) = path.change_kind.as_ref() {
                markers.push(format!("change={kind:?}"));
            }
            if let Some(status) = path.change_status.as_deref() {
                markers.push(format!("status={status}"));
            }
            if let Some(previous_path) = path.previous_path.as_deref() {
                markers.push(format!("previous={previous_path}"));
            }
            let marker = if markers.is_empty() {
                String::new()
            } else {
                format!(" ({})", markers.join(", "))
            };
            let _ = writeln!(markdown, "- {}{marker}: {}", path.path, path.reason);
        }
    }
}

pub(super) fn render_affected_routes(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.impacted_routes.is_empty() {
        markdown.push_str("\nimpacted routes:\n");
        for route in output.impacted_routes.iter().take(30) {
            let handler = route
                .route
                .handler
                .as_ref()
                .map(|handler| format!(" handler={}", handler.display_name))
                .unwrap_or_default();
            let framework = route
                .route
                .framework
                .as_deref()
                .map(|framework| format!(" framework={framework}"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- d{} {} {}{}{} [{}]: {}",
                route.graph_depth,
                route.route.method,
                route.route.path,
                framework,
                handler,
                route.confidence,
                route.reason
            );
        }
    }
}

pub(super) fn render_affected_tests(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.impacted_tests.is_empty() {
        markdown.push_str("\nlikely impacted tests:\n");
        for test in &output.impacted_tests {
            let _ = writeln!(
                markdown,
                "- d{} {} ({} symbols, {}): {}",
                test.graph_depth,
                test.path,
                test.impacted_symbol_count,
                test.confidence,
                test.reason
            );
        }
    }
}

pub(super) fn render_affected_symbols(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    markdown.push_str("\nimpacted symbols:\n");
    for symbol in output.impacted_symbols.iter().take(40) {
        let location = symbol
            .file_path
            .as_deref()
            .map(|path| match symbol.line {
                Some(line) => format!("{path}:{line}"),
                None => path.to_string(),
            })
            .unwrap_or_else(|| "unknown".to_string());
        let _ = writeln!(
            markdown,
            "- d{} {} [{:?}] at {} ({}, {}): {}",
            symbol.graph_depth,
            symbol.display_name,
            symbol.kind,
            location,
            symbol.node_id.0,
            symbol.confidence,
            symbol.reason
        );
    }
    if output.impacted_symbols.len() > 40 {
        let _ = writeln!(
            markdown,
            "- ... {} more symbols omitted",
            output.impacted_symbols.len() - 40
        );
    }
}

pub(super) fn render_affected_invocation(invocation: &AffectedFollowUpInvocationDto) -> String {
    std::iter::once(invocation.program.clone())
        .chain(
            invocation
                .args
                .iter()
                .map(|arg| quote_command_argument_value(arg)),
        )
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn render_affected_footer(
    markdown: &mut String,
    output: &codestory_contracts::api::AffectedAnalysisDto,
) {
    if !output.blind_spots.is_empty() {
        markdown.push_str("\nblind spots:\n");
        for blind_spot in &output.blind_spots {
            let _ = writeln!(markdown, "- {blind_spot}");
        }
    }
    if !output.follow_ups.is_empty() {
        markdown.push_str("\nfollow-ups:\n");
        for follow_up in &output.follow_ups {
            let invocation = follow_up
                .invocation
                .as_ref()
                .map(render_affected_invocation)
                .map(|invocation| format!(" invocation=`{invocation}`"))
                .unwrap_or_default();
            let _ = writeln!(
                markdown,
                "- {} [{}]: {}{}",
                follow_up.action, follow_up.confidence, follow_up.reason, invocation
            );
        }
    }
}
