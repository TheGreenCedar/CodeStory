use anyhow::{Context, Result};
use codestory_contracts::api::{NodeKind, SearchHit};
use codestory_runtime::{compare_ranked_hits, symbol_name_match_rank};
use std::fs;
use std::path::Path;

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

pub(crate) fn search_hit_matches_file_filter(
    project_root: &Path,
    hit: &SearchHit,
    fragment: &str,
) -> bool {
    file_filter_match_bucket(project_root, hit, fragment) > 0
}

pub(crate) fn file_filter_match_bucket(project_root: &Path, hit: &SearchHit, fragment: &str) -> u8 {
    let Some(file_path) = hit.file_path.as_deref() else {
        return 0;
    };

    let absolute = normalize_path_fragment(file_path);
    let relative = normalize_path_fragment(&crate::display::relative_path(project_root, file_path));
    let fragment = normalize_path_fragment(fragment);
    let fragment = fragment.trim_matches('/').to_string();
    if fragment.is_empty() {
        return 0;
    }

    if relative == fragment || absolute == fragment {
        return 4;
    }

    if relative.ends_with(&format!("/{fragment}")) || absolute.ends_with(&format!("/{fragment}")) {
        return 3;
    }

    if relative
        .rsplit('/')
        .next()
        .is_some_and(|file_name| file_name == fragment)
    {
        return 2;
    }

    if relative.contains(&fragment) || absolute.contains(&fragment) {
        return 1;
    }

    0
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
