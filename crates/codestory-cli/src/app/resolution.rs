use super::*;

#[derive(serde::Serialize)]
pub(crate) struct CliErrorOutput {
    error: CliErrorBody,
}

#[derive(serde::Serialize)]
pub(super) struct CliErrorBody {
    code: &'static str,
    failed_layer: &'static str,
    message: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file_filter: Option<String>,
    alternatives: Vec<SearchHitOutput>,
    layer_notes: Vec<String>,
    next_commands: Vec<String>,
}

pub(super) const CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT: usize = 10;

pub(crate) fn resolve_target_or_emit_ambiguity(
    runtime: &RuntimeContext,
    target: args::TargetSelection,
    file_filter: Option<&str>,
    format: args::OutputFormat,
    output_file: Option<&std::path::Path>,
) -> Result<runtime::ResolvedTarget> {
    match resolve_target(runtime, target, file_filter) {
        Ok(target) => Ok(target),
        Err(error) => {
            if let Some(ambiguous) = error.downcast_ref::<AmbiguousTargetError>() {
                return structured_ambiguous_target_failure(
                    runtime,
                    ambiguous.clone(),
                    format,
                    output_file,
                );
            }
            Err(error)
        }
    }
}

pub(super) fn resolve_source_target_or_emit_ambiguity(
    runtime: &RuntimeContext,
    target: args::TargetSelection,
    file_filter: Option<&str>,
    format: args::OutputFormat,
    output_file: Option<&std::path::Path>,
) -> Result<runtime::ResolvedTarget> {
    match resolve_source_target(runtime, target, file_filter) {
        Ok(target) => Ok(target),
        Err(error) => {
            if let Some(ambiguous) = error.downcast_ref::<AmbiguousTargetError>() {
                return structured_ambiguous_target_failure(
                    runtime,
                    ambiguous.clone(),
                    format,
                    output_file,
                );
            }
            Err(error)
        }
    }
}

pub(super) fn structured_ambiguous_target_failure<T>(
    runtime: &RuntimeContext,
    ambiguous: AmbiguousTargetError,
    format: args::OutputFormat,
    output_file: Option<&Path>,
) -> Result<T> {
    let output = build_ambiguous_target_error_output(&runtime.project_root, &ambiguous);
    let markdown = (format != args::OutputFormat::Json).then(|| render_cli_error_markdown(&output));
    Err(StructuredCommandFailure {
        envelope: ambiguous_command_failure(&output, &runtime.project_root),
        output_file: output_file.map(Path::to_path_buf),
        markdown,
    }
    .into())
}

pub(super) fn ambiguous_command_failure(
    output: &CliErrorOutput,
    project_root: &Path,
) -> CommandFailureEnvelope {
    let message = cli_error_markdown_message(output).to_string();
    CommandFailureEnvelope::new(ApiError::with_details(
        output.error.code,
        message,
        ApiErrorDetails {
            cause_code: None,
            failed_layer: Some(output.error.failed_layer.to_string()),
            project: Some(display::clean_path_string(&project_root.to_string_lossy())),
            next_commands: output.error.next_commands.clone(),
            minimum_next: output.error.next_commands.iter().take(1).cloned().collect(),
            full_repair: output.error.next_commands.clone(),
            readiness: None,
            embedding_capacity: None,
            embedding_retry: None,
            coverage_gaps: Vec::new(),
        },
    ))
    .with_context(serde_json::json!({
        "query": output.error.query,
        "file_filter": output.error.file_filter,
        "alternatives": output.error.alternatives,
        "layer_notes": output.error.layer_notes,
    }))
}

pub(super) fn render_cli_error_markdown(output: &CliErrorOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Command Error");
    let _ = writeln!(markdown, "code: {}", output.error.code);
    let _ = writeln!(markdown, "failed_layer: {}", output.error.failed_layer);
    let _ = writeln!(markdown, "message: {}", cli_error_markdown_message(output));
    let _ = writeln!(markdown, "query: `{}`", output.error.query);
    if let Some(file_filter) = output.error.file_filter.as_deref() {
        let _ = writeln!(markdown, "file_filter: `{file_filter}`");
    }
    if !output.error.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.error.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    let _ = writeln!(
        markdown,
        "alternatives: {}",
        output.error.alternatives.len()
    );
    if output.error.alternatives.len() > CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT {
        let _ = writeln!(
            markdown,
            "showing: {} of {}; use `--format json` or `search` to inspect all alternatives",
            CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT,
            output.error.alternatives.len()
        );
    }
    for alternative in output
        .error
        .alternatives
        .iter()
        .take(CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT)
    {
        let location = alternative
            .file_path
            .as_deref()
            .map(|path| {
                alternative
                    .line
                    .map(|line| format!(" {path}:{line}"))
                    .unwrap_or_else(|| format!(" {path}"))
            })
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- [{}] {} [{}]{} score={:.2} match={}",
            alternative.node_id,
            alternative.display_name,
            display::format_kind(alternative.kind),
            location,
            alternative.score,
            match alternative.match_quality {
                SearchMatchQualityDto::Exact => "exact",
                SearchMatchQualityDto::NormalizedExact => "normalized_exact",
                SearchMatchQualityDto::Prefix => "prefix",
                SearchMatchQualityDto::Fuzzy => "fuzzy",
                SearchMatchQualityDto::SemanticSuggestion => "semantic_suggestion",
                SearchMatchQualityDto::RepoText => "repo_text",
            }
        );
    }
    if !output.error.layer_notes.is_empty() {
        let _ = writeln!(markdown, "layer_notes:");
        for note in &output.error.layer_notes {
            let _ = writeln!(markdown, "- {note}");
        }
    }
    markdown
}

pub(super) fn cli_error_markdown_message(output: &CliErrorOutput) -> &str {
    if output.error.code == "ambiguous_target" {
        output
            .error
            .message
            .lines()
            .next()
            .unwrap_or(&output.error.message)
    } else {
        &output.error.message
    }
}

pub(crate) fn build_ambiguous_target_error_output(
    project_root: &std::path::Path,
    ambiguous: &AmbiguousTargetError,
) -> CliErrorOutput {
    let alternatives = ambiguous
        .alternatives
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            build_numbered_search_hit_output(project_root, hit, Some(&ambiguous.query), index + 1)
        })
        .collect::<Vec<_>>();
    let project = quote_command_path(project_root);
    let file_clause = ambiguous
        .file_filter
        .as_deref()
        .map(|file_filter| format!(" --file {}", quote_command_argument_value(file_filter)))
        .unwrap_or_default();
    let mut next_commands = vec![format!(
        "codestory-cli symbol --project {project} --query {}{} --choose 1",
        quote_command_argument_value(&ambiguous.query),
        file_clause
    )];
    if let Some(first) = ambiguous.alternatives.first() {
        next_commands.push(format!(
            "codestory-cli symbol --project {project} --id {}",
            first.node_id.0
        ));
        if let Some(path) = first.file_path.as_deref() {
            next_commands.push(format!(
                "codestory-cli symbol --project {project} --query {} --file {}",
                quote_command_argument_value(&ambiguous.query),
                quote_command_argument_value(&crate::display::relative_path(project_root, path))
            ));
        }
    }

    CliErrorOutput {
        error: CliErrorBody {
            code: "ambiguous_target",
            failed_layer: "query_resolution",
            message: ambiguous.message.clone(),
            query: ambiguous.query.clone(),
            file_filter: ambiguous
                .file_filter
                .as_deref()
                .map(crate::display::clean_path_string),
            alternatives,
            layer_notes: vec![
                format!(
                    "query_resolution: `{}` matched multiple equally ranked symbols",
                    ambiguous.query
                ),
                format!(
                    "search: inspect alternatives with `codestory-cli search --project {project} --query {}`, then rerun this command with --choose, --id, or --file",
                    quote_command_argument_value(&ambiguous.query)
                ),
            ],
            next_commands,
        },
    }
}
