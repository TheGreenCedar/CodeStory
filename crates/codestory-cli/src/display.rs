use codestory_contracts::api::{GroundingBudgetDto, SearchHit, TrailDirection};
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

pub(crate) fn relative_path(project_root: &Path, raw: &str) -> String {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return clean_path_string(raw);
    }

    if let Ok(relative) = path.strip_prefix(project_root) {
        return clean_path_string(&relative.to_string_lossy());
    }

    let normalized_root = clean_path_string(&project_root.to_string_lossy()).to_ascii_lowercase();
    let normalized_raw = clean_path_string(raw);
    let normalized_raw_lower = normalized_raw.to_ascii_lowercase();

    if let Some(remainder) = normalized_raw_lower
        .strip_prefix(&format!("{normalized_root}/"))
        .and_then(|_| {
            normalized_raw.strip_prefix(&(clean_path_string(&project_root.to_string_lossy()) + "/"))
        })
    {
        return remainder.to_string();
    }

    normalized_raw
}

pub(crate) fn format_search_hit_target(project_root: &Path, hit: &SearchHit) -> String {
    let mut out = format!("{} [{}]", hit.display_name, format_kind(hit.kind));
    if let Some(path) = hit.file_path.as_deref() {
        out.push(' ');
        out.push_str(&relative_path(project_root, path));
    }
    if let Some(line) = hit.line {
        out.push(':');
        out.push_str(&line.to_string());
    }
    out
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
