use codestory_contracts::api::{GroundingBudgetDto, TrailDirection};
use std::path::Path;

use crate::args::CliTrailMode;

pub(crate) fn clean_path_string(path: &str) -> String {
    let mut stringified = path.replace('\\', "/");
    if let Some(stripped) = stringified.strip_prefix("//?/UNC/") {
        stringified = format!("//{stripped}");
    } else if stringified.starts_with("//?/") {
        stringified = stringified[4..].to_string();
    }
    stringified
}

pub(crate) fn quote_command_path(path: &Path) -> String {
    let value = clean_path_string(&path.to_string_lossy());
    quote_command_argument_value(&value)
}

pub(crate) fn quote_command_value(value: &str) -> String {
    quote_shell_single_quoted_value(value)
}

pub(crate) fn quote_command_argument_value(value: &str) -> String {
    if command_value_needs_single_quotes(value) {
        quote_command_value(value)
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

fn command_value_needs_single_quotes(value: &str) -> bool {
    value.chars().any(|ch| matches!(ch, '$' | '`' | '\'' | '"'))
}

#[cfg(windows)]
fn quote_shell_single_quoted_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn quote_shell_single_quoted_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn relative_path(project_root: &Path, raw: &str) -> String {
    let normalized_raw = clean_path_string(raw);
    codestory_workspace::workspace_relative_path(project_root, Path::new(&normalized_raw))
        .map(|relative| clean_path_string(&relative.to_string_lossy()))
        .unwrap_or(normalized_raw)
}

pub(crate) fn format_kind(kind: codestory_contracts::api::NodeKind) -> String {
    format!("{kind:?}").to_lowercase()
}

pub(crate) fn format_budget(budget: GroundingBudgetDto) -> &'static str {
    match budget {
        GroundingBudgetDto::Strict => "strict",
        GroundingBudgetDto::Balanced => "balanced",
        GroundingBudgetDto::Max => "max",
    }
}

pub(crate) fn format_trail_mode(mode: CliTrailMode) -> &'static str {
    match mode {
        CliTrailMode::Neighborhood => "neighborhood",
        CliTrailMode::Referenced => "referenced",
        CliTrailMode::Referencing => "referencing",
    }
}

pub(crate) fn format_direction(direction: TrailDirection) -> &'static str {
    match direction {
        TrailDirection::Incoming => "incoming",
        TrailDirection::Outgoing => "outgoing",
        TrailDirection::Both => "both",
    }
}

pub(crate) fn default_trail_direction(mode: CliTrailMode) -> TrailDirection {
    match mode {
        CliTrailMode::Neighborhood => TrailDirection::Both,
        CliTrailMode::Referenced => TrailDirection::Outgoing,
        CliTrailMode::Referencing => TrailDirection::Incoming,
    }
}
