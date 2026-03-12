use codestory_contracts::api::{GroundingBudgetDto, TrailDirection};
use std::path::Path;

use crate::args::CliTrailMode;

pub(crate) fn clean_path_string(path: &str) -> String {
    let mut stringified = path.replace('\\', "/");
    if stringified.starts_with("//?/") {
        stringified = stringified[4..].to_string();
    }
    stringified
}

pub(crate) fn relative_path(project_root: &Path, raw: &str) -> String {
    let path = Path::new(raw);
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy();
    clean_path_string(&relative)
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
