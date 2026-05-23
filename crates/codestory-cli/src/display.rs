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
    let normalized_root = clean_path_string(&project_root.to_string_lossy());
    let normalized_raw = clean_path_string(raw);
    let normalized_root = normalized_root.trim_end_matches('/');
    let normalized_raw_lower = normalized_raw.to_ascii_lowercase();
    let normalized_root_lower = normalized_root.to_ascii_lowercase();

    if normalized_raw_lower == normalized_root_lower {
        return String::new();
    }
    if let Some(remainder) = normalized_raw_lower.strip_prefix(&format!("{normalized_root_lower}/"))
    {
        let start = normalized_raw.len() - remainder.len();
        return normalized_raw[start..].to_string();
    }

    let path = Path::new(raw);
    if !path.is_absolute() {
        return normalized_raw;
    }

    if let Ok(relative) = path.strip_prefix(project_root) {
        return clean_path_string(&relative.to_string_lossy());
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
