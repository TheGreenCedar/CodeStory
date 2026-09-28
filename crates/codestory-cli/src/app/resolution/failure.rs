use crate::{args::SearchHitOutput, display, runtime};
use codestory_contracts::api::{ApiError, ApiErrorDetails, CommandFailureEnvelope};
use std::{
    ffi::{OsStr, OsString},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(in crate::app) struct StructuredCommandFailure {
    pub(in crate::app) envelope: CommandFailureEnvelope,
    pub(in crate::app) output_file: Option<PathBuf>,
    pub(in crate::app) markdown: Option<String>,
}

impl std::fmt::Display for StructuredCommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.envelope.error.message)
    }
}

impl std::error::Error for StructuredCommandFailure {}

pub(in crate::app) fn command_failure_envelope(
    code: impl Into<String>,
    failed_layer: impl Into<String>,
    message: impl Into<String>,
    context: serde_json::Value,
) -> CommandFailureEnvelope {
    CommandFailureEnvelope::new(ApiError::with_details(
        code,
        message,
        ApiErrorDetails {
            cause_code: None,
            failed_layer: Some(failed_layer.into()),
            project: None,
            next_commands: Vec::new(),
            minimum_next: Vec::new(),
            full_repair: Vec::new(),
            readiness: None,
            embedding_capacity: None,
            embedding_retry: None,
            disk_space: None,
            coverage_gaps: Vec::new(),
        },
    ))
    .with_context(context)
}

pub(in crate::app) fn generic_command_failure(error: &anyhow::Error) -> CommandFailureEnvelope {
    command_failure_envelope(
        "command_failed",
        "command",
        error.to_string(),
        serde_json::json!({
            "causes": error.chain().skip(1).map(ToString::to_string).collect::<Vec<_>>()
        }),
    )
}

pub(in crate::app) fn command_failure_message(error: &anyhow::Error) -> String {
    if runtime::api_error_in_chain(error).is_some() {
        format!("{error:#}")
    } else {
        error.to_string()
    }
}

pub(in crate::app) fn json_output_requested(args: &[OsString]) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == OsStr::new("--format") && pair[1] == OsStr::new("json"))
        || args.iter().any(|arg| arg == OsStr::new("--format=json"))
        || args
            .iter()
            .any(|arg| arg == OsStr::new("verify-indexed-direct-calls"))
}

pub(in crate::app) fn requested_output_file(args: &[OsString]) -> Option<&Path> {
    args.iter()
        .find_map(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix("--output-file="))
                .filter(|path| !path.is_empty())
                .map(Path::new)
        })
        .or_else(|| {
            args.windows(2).find_map(|pair| {
                (pair[0] == OsStr::new("--output-file")
                    && !pair[1].to_string_lossy().starts_with('-'))
                .then(|| Path::new(&pair[1]))
            })
        })
}

pub(in crate::app) fn emit_command_failure(
    envelope: &CommandFailureEnvelope,
    output_file: Option<&Path>,
) {
    let json = serde_json::to_string_pretty(envelope)
        .expect("the command failure envelope is always JSON-serializable");
    if let Some(path) = output_file
        && fs::write(path, format!("{json}\n")).is_ok()
    {
        return;
    }
    println!("{json}");
}

/// The evidence lines a failing command owes a human reading the default
/// output: the same `context.causes` chain and `next_action`/`next_commands`
/// guidance the JSON envelope already carries.
pub(in crate::app) fn command_failure_details_markdown(
    envelope: &CommandFailureEnvelope,
) -> String {
    let mut markdown = String::new();
    let error = &envelope.error;
    let _ = writeln!(markdown, "code: {}", error.code);
    if let Some(details) = error.details.as_deref()
        && let Some(layer) = details.failed_layer.as_deref()
    {
        let _ = writeln!(markdown, "failed_layer: {layer}");
    }
    if let Some(causes) = envelope
        .context
        .as_ref()
        .and_then(|context| context.get("causes"))
        .and_then(serde_json::Value::as_array)
        && !causes.is_empty()
    {
        let _ = writeln!(markdown, "causes:");
        for cause in causes {
            let cause = cause
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| cause.to_string());
            let _ = writeln!(markdown, "- {cause}");
        }
    }
    if let Some(details) = error.details.as_deref() {
        for action in &details.minimum_next {
            let _ = writeln!(markdown, "next_action: {action}");
        }
        if details.next_commands.len() > details.minimum_next.len() {
            let _ = writeln!(markdown, "next_commands:");
            for command in details
                .next_commands
                .iter()
                .skip(details.minimum_next.len())
            {
                let _ = writeln!(markdown, "- `{command}`");
            }
        }
    }
    markdown
}

/// The complete markdown document for a failed command, written to
/// `--output-file` when the run did not ask for JSON.
pub(in crate::app) fn render_command_failure_markdown(envelope: &CommandFailureEnvelope) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Command Error");
    let _ = writeln!(markdown, "message: {}", envelope.error.message);
    markdown.push_str(&command_failure_details_markdown(envelope));
    markdown
}

pub(in crate::app) fn quote_command_path(path: &Path) -> String {
    display::quote_command_path(path)
}

pub(in crate::app) fn quote_command_value(value: &str) -> String {
    display::quote_command_value(value)
}

pub(in crate::app) fn quote_command_argument_value(value: &str) -> String {
    display::quote_command_argument_value(value)
}

#[derive(serde::Serialize)]
pub(crate) struct CliErrorOutput {
    pub(in crate::app::resolution) error: CliErrorBody,
}

#[derive(serde::Serialize)]
pub(in crate::app) struct CliErrorBody {
    pub(in crate::app::resolution) code: &'static str,
    pub(in crate::app::resolution) failed_layer: &'static str,
    pub(in crate::app::resolution) message: String,
    pub(in crate::app::resolution) query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::app::resolution) file_filter: Option<String>,
    pub(in crate::app::resolution) alternatives: Vec<SearchHitOutput>,
    pub(in crate::app::resolution) layer_notes: Vec<String>,
    pub(in crate::app::resolution) next_commands: Vec<String>,
}

pub(in crate::app) const CLI_ERROR_MARKDOWN_ALTERNATIVE_LIMIT: usize = 10;
