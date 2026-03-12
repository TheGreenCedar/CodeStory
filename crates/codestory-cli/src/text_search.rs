use anyhow::{Context, Result};
use codestory_contracts::api::{NodeId, SearchHit, SearchHitOrigin};
use std::fs;
use std::path::Path;

pub(crate) fn looks_like_text_query(query: &str) -> bool {
    let word_count = query.split_whitespace().count();
    let has_text_punctuation = query
        .chars()
        .any(|ch| matches!(ch, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\''));
    (word_count > 1 && has_text_punctuation) || query.len() > 28
}

pub(crate) fn scan_repo_text_hits(
    project_root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    if query.trim().is_empty() || limit == 0 {
        return Ok(hits);
    }
    scan_repo_text_hits_inner(project_root, project_root, query, limit, &mut hits)?;
    Ok(hits)
}

fn scan_repo_text_hits_inner(
    project_root: &Path,
    dir: &Path,
    query: &str,
    limit: usize,
    hits: &mut Vec<SearchHit>,
) -> Result<()> {
    if hits.len() >= limit {
        return Ok(());
    }

    for entry in
        fs::read_dir(dir).with_context(|| format!("Failed to read directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if is_ignored_search_dir(&path) {
                continue;
            }
            scan_repo_text_hits_inner(project_root, &path, query, limit, hits)?;
            if hits.len() >= limit {
                break;
            }
            continue;
        }

        if let Some(hit) = scan_file_text_hit(project_root, &path, query, hits.len()) {
            hits.push(hit);
            if hits.len() >= limit {
                break;
            }
        }
    }

    Ok(())
}

fn is_ignored_search_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "node_modules" | ".next" | "dist")
    )
}

fn scan_file_text_hit(
    project_root: &Path,
    path: &Path,
    query: &str,
    rank: usize,
) -> Option<SearchHit> {
    let metadata = path.metadata().ok()?;
    if metadata.len() > 1_000_000 {
        return None;
    }

    let contents = fs::read_to_string(path).ok()?;
    let normalized_query = query.trim().to_ascii_lowercase();
    if normalized_query.is_empty() {
        return None;
    }

    let mut line_match = None;
    for (index, line) in contents.lines().enumerate() {
        if line.to_ascii_lowercase().contains(&normalized_query) {
            line_match = Some((index + 1).min(u32::MAX as usize) as u32);
            break;
        }
    }
    let line = line_match?;
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let hash_hex = crate::runtime::fnv1a_hex(format!("{relative}:{line}").as_bytes());
    let node_id_raw = i64::from_str_radix(&hash_hex[..15], 16).ok()?;

    Some(SearchHit {
        node_id: NodeId(node_id_raw.to_string()),
        display_name: relative.clone(),
        kind: codestory_contracts::api::NodeKind::FILE,
        file_path: Some(path.to_string_lossy().to_string()),
        line: Some(line),
        score: 500.0 - rank as f32,
        origin: SearchHitOrigin::TextMatch,
        resolvable: false,
    })
}
