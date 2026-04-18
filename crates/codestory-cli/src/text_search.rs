use anyhow::{Context, Result};
use codestory_contracts::api::{NodeId, SearchHit, SearchHitOrigin};
use std::fs;
use std::path::Path;

pub(crate) fn looks_like_text_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }

    let word_count = trimmed.split_whitespace().count();
    let has_text_punctuation = query
        .chars()
        .any(|ch| matches!(ch, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\''));
    if (word_count > 1 && has_text_punctuation) || trimmed.len() > 28 || word_count >= 4 {
        return true;
    }

    if word_count < 2 {
        return false;
    }

    trimmed.split_whitespace().any(|term| {
        matches!(
            term.to_ascii_lowercase().as_str(),
            "how"
                | "what"
                | "why"
                | "where"
                | "when"
                | "which"
                | "who"
                | "does"
                | "do"
                | "is"
                | "are"
                | "should"
                | "can"
        )
    })
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
    let terms = text_query_terms(&normalized_query);

    let mut line_match = None;
    for (index, line) in contents.lines().enumerate() {
        let normalized_line = line.to_ascii_lowercase();
        if normalized_line.contains(&normalized_query)
            || (terms.len() >= 2 && terms.iter().all(|term| normalized_line.contains(term)))
        {
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

fn text_query_terms(query: &str) -> Vec<&str> {
    query
        .split_whitespace()
        .map(|term| term.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_'))
        .filter(|term| term.len() >= 3)
        .filter(|term| {
            !matches!(
                *term,
                "how"
                    | "what"
                    | "why"
                    | "where"
                    | "when"
                    | "which"
                    | "who"
                    | "does"
                    | "do"
                    | "is"
                    | "are"
                    | "should"
                    | "can"
                    | "the"
                    | "this"
                    | "that"
                    | "with"
                    | "from"
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn natural_language_queries_trigger_repo_text_without_punctuation() {
        assert!(looks_like_text_query("how does cli work"));
        assert!(looks_like_text_query("what stores grounding snapshots"));
        assert!(!looks_like_text_query("WorkspaceIndexer"));
        assert!(!looks_like_text_query("AppController open_project"));
    }

    #[test]
    fn repo_text_scan_can_match_term_sets_without_full_phrase() {
        let dir = tempdir().expect("temp dir");
        let file = dir.path().join("README.md");
        fs::write(
            &file,
            "This guide explains how the cli runtime executes the search workflow.\n",
        )
        .expect("write fixture");

        let hit = scan_file_text_hit(dir.path(), &file, "how does cli work", 0).expect("text hit");

        assert_eq!(hit.line, Some(1));
        assert_eq!(hit.origin, SearchHitOrigin::TextMatch);
    }
}
