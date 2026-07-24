use crate::{args::SearchHitOutput, display, runtime};
use codestory_contracts::api::{ApiError, ApiErrorDetails, CommandFailureEnvelope};
use std::{
    ffi::{OsStr, OsString},
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
