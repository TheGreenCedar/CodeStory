use anyhow::{Context, Result};
use codestory_contracts::api::{NodeKind, SearchHit};
use codestory_runtime::{compare_ranked_hits, symbol_name_match_rank};
use std::fs;

pub(crate) fn compare_resolution_hits(
    query: &str,
    left: &SearchHit,
    right: &SearchHit,
) -> std::cmp::Ordering {
    compare_ranked_hits(
        left,
        right,
        resolution_rank(query, left),
        resolution_rank(query, right),
    )
}

pub(crate) fn resolution_rank(query: &str, hit: &SearchHit) -> (u8, u8, u8, u8, u8) {
    let rank = symbol_name_match_rank(query, &hit.display_name);

    (
        rank.exact_display,
        rank.exact_terminal,
        declaration_anchor_bucket(hit),
        resolution_kind_bucket(hit.kind),
        rank.exact_leading,
    )
}

pub(crate) fn search_hit_matches_file_filter(hit: &SearchHit, fragment: &str) -> bool {
    let Some(file_path) = hit.file_path.as_deref() else {
        return false;
    };

    let file_path = normalize_path_fragment(file_path);
    let fragment = normalize_path_fragment(fragment);
    file_path.contains(&fragment)
}

fn resolution_kind_bucket(kind: NodeKind) -> u8 {
    if matches!(
        kind,
        NodeKind::MODULE
            | NodeKind::NAMESPACE
            | NodeKind::PACKAGE
            | NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    ) {
        return 2;
    }

    if matches!(
        kind,
        NodeKind::FUNCTION
            | NodeKind::METHOD
            | NodeKind::MACRO
            | NodeKind::FIELD
            | NodeKind::VARIABLE
            | NodeKind::GLOBAL_VARIABLE
            | NodeKind::CONSTANT
            | NodeKind::ENUM_CONSTANT
    ) {
        return 1;
    }

    0
}

fn normalize_path_fragment(value: &str) -> String {
    crate::display::clean_path_string(value).to_ascii_lowercase()
}

fn declaration_anchor_bucket(hit: &SearchHit) -> u8 {
    if matches!(
        hit.kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    ) && !hit_is_impl_anchor(hit)
    {
        return 1;
    }

    0
}

fn hit_is_impl_anchor(hit: &SearchHit) -> bool {
    let Some(file_path) = hit.file_path.as_deref() else {
        return false;
    };
    let Some(line) = hit.line else {
        return false;
    };
    let Ok(contents) = read_file_contents_for_resolution(file_path) else {
        return false;
    };
    let Some(source_line) = contents.lines().nth(line.saturating_sub(1) as usize) else {
        return false;
    };
    let trimmed = source_line.trim_start();
    trimmed.starts_with("impl ") || trimmed.starts_with("unsafe impl ")
}

fn read_file_contents_for_resolution(path: &str) -> Result<String> {
    if let Ok(contents) = fs::read_to_string(path) {
        return Ok(contents);
    }

    #[cfg(windows)]
    if let Some(stripped) = path.strip_prefix(r"\\?\")
        && let Ok(contents) = fs::read_to_string(stripped)
    {
        return Ok(contents);
    }

    fs::read_to_string(path).with_context(|| format!("Failed to read file `{path}`"))
}
