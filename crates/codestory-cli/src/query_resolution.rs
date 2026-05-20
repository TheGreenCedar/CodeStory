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

pub(crate) fn resolution_rank(query: &str, hit: &SearchHit) -> (u8, u8, u8, u8, u8, u8, u8) {
    resolution_rank_with_project_root(None, query, hit)
}

pub(crate) fn resolution_rank_with_project_root(
    project_root: Option<&Path>,
    query: &str,
    hit: &SearchHit,
) -> (u8, u8, u8, u8, u8, u8, u8) {
    let rank = symbol_name_match_rank(query, &hit.display_name);

    (
        rank.exact_display,
        rank.exact_terminal,
        type_definition_line_bucket(project_root, query, hit),
        callable_definition_line_bucket(project_root, query, hit),
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

fn type_definition_line_bucket(project_root: Option<&Path>, query: &str, hit: &SearchHit) -> u8 {
    if !matches!(
        hit.kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    ) {
        return 0;
    }

    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display == 0 && rank.exact_terminal == 0 && rank.exact_leading == 0 {
        return 0;
    }

    let Some(file_path) = hit.file_path.as_deref() else {
        return 0;
    };
    let Some(line) = hit.line else {
        return 0;
    };
    let Ok(contents) = read_file_contents_for_resolution(project_root, file_path) else {
        return 0;
    };
    let Some(source_line) = contents.lines().nth(line.saturating_sub(1) as usize) else {
        return 0;
    };
    let trimmed = source_line.split("//").next().unwrap_or(source_line).trim();
    let expected = codestory_runtime::terminal_symbol_segment(query);
    let tokens = trimmed
        .split(|ch: char| ch.is_whitespace() || ch == ':' || ch == ';' || ch == '{')
        .map(|token| token.trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let Some(keyword_index) = tokens
        .iter()
        .position(|token| matches!(*token, "class" | "struct" | "interface" | "enum" | "union"))
    else {
        return 0;
    };
    let Some(type_name) = tokens.get(keyword_index + 1).copied() else {
        return 0;
    };
    if !type_name.eq_ignore_ascii_case(&expected) {
        return 0;
    }
    if trimmed.contains('{') || !trimmed.ends_with(';') {
        2
    } else {
        0
    }
}

fn callable_definition_line_bucket(
    project_root: Option<&Path>,
    query: &str,
    hit: &SearchHit,
) -> u8 {
    if !matches!(
        hit.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    ) {
        return 0;
    }

    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display == 0 && rank.exact_terminal == 0 && rank.exact_leading == 0 {
        return 0;
    }

    let Some(file_path) = hit.file_path.as_deref() else {
        return 0;
    };
    let Some(line) = hit.line else {
        return 0;
    };
    let Ok(contents) = read_file_contents_for_resolution(project_root, file_path) else {
        return 0;
    };
    let line_index = line.saturating_sub(1) as usize;
    let Some(source_line) = contents.lines().nth(line_index) else {
        return 0;
    };
    let trimmed = source_line.split("//").next().unwrap_or(source_line).trim();
    let expected = codestory_runtime::terminal_symbol_segment(query);
    if expected.is_empty() || !line_contains_symbol_name(trimmed, &expected) {
        return 0;
    }
    let signature_window = contents
        .lines()
        .skip(line_index)
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");
    if looks_like_callable_declaration(&signature_window) {
        return 0;
    }
    if !looks_like_callable_definition(&signature_window) {
        return 0;
    }

    2
}

fn line_contains_symbol_name(line: &str, expected: &str) -> bool {
    line.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|token| token.eq_ignore_ascii_case(expected))
}

fn looks_like_callable_declaration(line: &str) -> bool {
    let brace = line.find('{');
    let semicolon = line.find(';');
    let before_body = brace.map(|index| &line[..index]).unwrap_or(line);
    matches!(
        (brace, semicolon),
        (Some(brace), Some(semicolon)) if semicolon < brace
    ) || matches!((brace, semicolon), (None, Some(_)))
        || before_body.contains("= 0;")
}

fn looks_like_callable_definition(line: &str) -> bool {
    let brace = line.find('{');
    let semicolon = line.find(';');
    matches!(
        (brace, semicolon),
        (Some(brace), Some(semicolon)) if brace < semicolon
    ) || matches!((brace, semicolon), (Some(_), None))
}

fn hit_is_impl_anchor(hit: &SearchHit) -> bool {
    let Some(file_path) = hit.file_path.as_deref() else {
        return false;
    };
    let Some(line) = hit.line else {
        return false;
    };
    let Ok(contents) = read_file_contents_for_resolution(None, file_path) else {
        return false;
    };
    let Some(source_line) = contents.lines().nth(line.saturating_sub(1) as usize) else {
        return false;
    };
    let trimmed = source_line.trim_start();
    trimmed.starts_with("impl ") || trimmed.starts_with("unsafe impl ")
}

fn read_file_contents_for_resolution(project_root: Option<&Path>, path: &str) -> Result<String> {
    let raw_path = Path::new(path);
    let joined_path;
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else if let Some(root) = project_root {
        joined_path = root.join(raw_path);
        joined_path.as_path()
    } else {
        raw_path
    };

    if let Ok(contents) = fs::read_to_string(candidate) {
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
