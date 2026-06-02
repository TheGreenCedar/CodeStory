//! Build per-project Zoekt shard directories (lexical index + optional remote index).

use crate::config::ZOEKT_REAL_VERSION_PIN;
use anyhow::{Context, Result};
use codestory_store::FileRole;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const LEXICAL_INDEX_FILE: &str = "lexical-index.jsonl";
const SHARD_META_FILE: &str = "shard-meta.json";
const STUB_MARKER: &str = ".zoekt-stub";

const MAX_FILE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LexicalIndexEntry {
    path: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShardMeta {
    version: String,
    project_id: String,
    file_count: u32,
    lexical_hash: Option<String>,
    indexed_at_epoch_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalInputFingerprint {
    pub file_count: u32,
    pub hash: String,
}

/// Populate `shards/<project_id>/` with a searchable lexical index; remove stub marker on success.
pub fn build_zoekt_shard(
    project_root: &Path,
    zoekt_data_dir: &Path,
    project_id: &str,
    zoekt_http_reachable: bool,
) -> Result<bool> {
    let shard_dir = zoekt_data_dir.join("shards").join(project_id);
    std::fs::create_dir_all(&shard_dir)
        .with_context(|| format!("create zoekt shard dir {}", shard_dir.display()))?;

    let entries = collect_lexical_entries(project_root)?;
    if entries.is_empty() {
        return Ok(false);
    }
    let lexical_hash = lexical_entries_hash(&entries);

    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let mut writer = std::fs::File::create(&index_path)
        .with_context(|| format!("create {}", index_path.display()))?;
    use std::io::Write;
    for entry in &entries {
        let line = serde_json::to_string(entry).context("serialize lexical index entry")?;
        writeln!(writer, "{line}").context("write lexical index line")?;
    }

    let version = ZOEKT_REAL_VERSION_PIN.to_string();
    let meta = ShardMeta {
        version: version.clone(),
        project_id: project_id.to_string(),
        file_count: entries.len() as u32,
        lexical_hash: Some(lexical_hash),
        indexed_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
    };
    std::fs::write(
        shard_dir.join(SHARD_META_FILE),
        serde_json::to_string_pretty(&meta).context("serialize shard meta")?,
    )
    .context("write shard meta")?;

    let stub = shard_dir.join(STUB_MARKER);
    if stub.is_file() {
        std::fs::remove_file(&stub).context("remove zoekt stub marker")?;
    }

    let _ = zoekt_http_reachable;
    Ok(true)
}

pub fn shard_has_lexical_index(shard_dir: &Path) -> bool {
    shard_dir.join(LEXICAL_INDEX_FILE).is_file() && !shard_dir.join(STUB_MARKER).is_file()
}

pub fn shard_matches_lexical_input(
    zoekt_data_dir: &Path,
    sidecar_generation: &str,
    expected_file_count: u32,
    expected_hash: &str,
) -> bool {
    let shard_dir = shard_dir_for(zoekt_data_dir, sidecar_generation);
    if !shard_has_lexical_index(&shard_dir) {
        return false;
    }
    let Ok(body) = std::fs::read_to_string(shard_dir.join(SHARD_META_FILE)) else {
        return false;
    };
    let Ok(meta) = serde_json::from_str::<ShardMeta>(&body) else {
        return false;
    };
    meta.version == ZOEKT_REAL_VERSION_PIN
        && meta.project_id == sidecar_generation
        && meta.file_count == expected_file_count
        && meta.lexical_hash.as_deref() == Some(expected_hash)
}

pub fn lexical_input_fingerprint(project_root: &Path) -> Result<LexicalInputFingerprint> {
    let entries = collect_lexical_entries(project_root)?;
    Ok(LexicalInputFingerprint {
        file_count: entries.len().min(u32::MAX as usize) as u32,
        hash: lexical_entries_hash(&entries),
    })
}

fn lexical_entries_hash(entries: &[LexicalIndexEntry]) -> String {
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(b"codestory-zoekt-lexical-v1");
    hasher.update(ZOEKT_REAL_VERSION_PIN.as_bytes());
    for entry in entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([0]);
        hasher.update(entry.content.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub fn search_lexical_index(
    shard_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<LexicalHit>> {
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    if !index_path.is_file() {
        return Ok(Vec::new());
    }
    let tokens = lexical_query_tokens(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let command_tokens = command_query_tokens(query);
    let content = std::fs::read_to_string(&index_path).context("read lexical index")?;
    let entries = content
        .lines()
        .filter_map(|line| serde_json::from_str::<LexicalIndexEntry>(line).ok())
        .collect::<Vec<_>>();
    let token_frequencies = token_document_frequencies(&entries, &tokens);
    let token_weights = token_frequencies
        .iter()
        .zip(tokens.iter())
        .map(|(frequency, token)| {
            let mut weight = lexical_token_weight(*frequency, entries.len());
            if command_tokens
                .iter()
                .any(|command_token| command_token == token)
            {
                weight *= 2.0;
            }
            weight
        })
        .collect::<Vec<_>>();
    let total_weight = token_weights.iter().sum::<f32>();
    let required_weight = required_lexical_match_weight(tokens.len(), total_weight);
    let mut hits = Vec::new();
    for entry in entries {
        let path_lower = entry.path.to_ascii_lowercase();
        let content_lower = entry.content.to_ascii_lowercase();
        let token_match = lexical_token_match(&tokens, &token_weights, &path_lower, &content_lower);
        if token_match.matched_weight >= required_weight
            && broad_query_path_gate(tokens.len(), &token_match)
        {
            let score = score_lexical_match(&entry.path, &token_match);
            hits.push(LexicalHit {
                path: entry.path,
                score,
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn lexical_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| token.len() >= 2)
        .map(str::to_ascii_lowercase)
        .filter(|token| !LEXICAL_STOP_WORDS.contains(&token.as_str()))
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
    tokens
}

fn command_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut in_backticks = false;
    let mut current = String::new();
    for ch in query.chars() {
        if ch == '`' {
            if in_backticks {
                push_command_tokens(&current, &mut tokens);
                current.clear();
            }
            in_backticks = !in_backticks;
            continue;
        }
        if in_backticks {
            current.push(ch);
        }
    }
    tokens
}

fn push_command_tokens(command: &str, tokens: &mut Vec<String>) {
    for token in command
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .map(|token| token.trim_start_matches('-').to_ascii_lowercase())
        .filter(|token| token.len() >= 2)
        .filter(|token| token != "codex")
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
}

const LEXICAL_STOP_WORDS: &[&str] = &[
    "about", "after", "and", "are", "cite", "does", "explain", "file", "files", "flow", "flows",
    "for", "from", "how", "into", "level", "path", "source", "sources", "support", "that", "the",
    "through", "top", "what", "where", "which", "with",
];

fn token_document_frequencies(entries: &[LexicalIndexEntry], tokens: &[String]) -> Vec<usize> {
    tokens
        .iter()
        .map(|token| {
            entries
                .iter()
                .filter(|entry| {
                    let path_lower = entry.path.to_ascii_lowercase();
                    let content_lower = entry.content.to_ascii_lowercase();
                    path_lower.contains(token.as_str()) || content_lower.contains(token.as_str())
                })
                .count()
        })
        .collect()
}

fn lexical_token_weight(document_frequency: usize, document_count: usize) -> f32 {
    let rarity = ((document_count as f32 + 1.0) / (document_frequency as f32 + 1.0)).ln();
    (1.0 + rarity).clamp(0.25, 5.0)
}

fn required_lexical_match_weight(token_count: usize, total_weight: f32) -> f32 {
    if token_count <= 3 {
        return total_weight;
    }
    (total_weight * 0.28).max(2.5)
}

#[derive(Debug, Clone, Copy)]
struct LexicalTokenMatch {
    matched_weight: f32,
    path_weight: f32,
    content_weight: f32,
    total_weight: f32,
    meaningful_path_weight: f32,
}

fn lexical_token_match(
    tokens: &[String],
    token_weights: &[f32],
    path_lower: &str,
    content_lower: &str,
) -> LexicalTokenMatch {
    let mut matched_weight = 0.0;
    let mut path_weight = 0.0;
    let mut content_weight = 0.0;
    let mut total_weight = 0.0;
    let mut meaningful_path_weight = 0.0;
    for (token, weight) in tokens.iter().zip(token_weights.iter().copied()) {
        total_weight += weight;
        let path_factor = path_match_factor(path_lower, token);
        let path_match = path_factor > 0.0;
        let content_match = content_lower.contains(token.as_str());
        if path_match || content_match {
            matched_weight += weight;
        }
        if path_match {
            path_weight += weight * path_factor;
            if path_factor >= 1.0 && weight >= 1.5 {
                meaningful_path_weight += weight;
            }
        }
        if content_match {
            content_weight += weight;
        }
    }
    LexicalTokenMatch {
        matched_weight,
        path_weight,
        content_weight,
        total_weight,
        meaningful_path_weight,
    }
}

fn broad_query_path_gate(token_count: usize, token_match: &LexicalTokenMatch) -> bool {
    token_count < 8 || token_match.meaningful_path_weight > 0.0
}

fn path_match_factor(path_lower: &str, token: &str) -> f32 {
    if path_lower.split('/').any(|segment| segment == token) {
        return 1.8;
    }
    if path_lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .flat_map(|part| part.split('_'))
        .any(|part| part == token)
    {
        return 1.0;
    }
    if path_lower.contains(token) {
        return 0.35;
    }
    0.0
}

#[derive(Debug, Clone)]
pub struct LexicalHit {
    pub path: String,
    pub score: f32,
}

fn score_lexical_match(path: &str, token_match: &LexicalTokenMatch) -> f32 {
    let coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.matched_weight / token_match.total_weight
    };
    let mut score = 0.20_f32
        + coverage * 0.25
        + token_match.path_weight * 0.09
        + token_match.content_weight * 0.035;
    let path_lower = path.replace('\\', "/").to_ascii_lowercase();
    if path_lower.contains("/src/") || path_lower.starts_with("src/") {
        score += 0.04;
    }
    score *= lexical_file_role_multiplier(FileRole::classify_path(Path::new(path)));
    score.min(0.99)
}

fn lexical_file_role_multiplier(file_role: FileRole) -> f32 {
    match file_role {
        FileRole::Entrypoint => 1.08,
        FileRole::Source => 1.0,
        FileRole::Test => 0.68,
        FileRole::Docs => 0.72,
        FileRole::Benchmark => 0.64,
        FileRole::Generated => 0.55,
        FileRole::Vendor => 0.45,
    }
}

fn collect_lexical_entries(project_root: &Path) -> Result<Vec<LexicalIndexEntry>> {
    let mut entries = Vec::new();
    collect_lexical_entries_inner(project_root, project_root, &mut entries)?;
    Ok(entries)
}

fn collect_lexical_entries_inner(
    project_root: &Path,
    dir: &Path,
    entries: &mut Vec<LexicalIndexEntry>,
) -> Result<()> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(_) => return Ok(()),
    };
    let mut dir_entries = read_dir.flatten().collect::<Vec<_>>();
    dir_entries.sort_by_key(|entry| entry.path());

    for entry in dir_entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if should_skip_dir(&name) {
                continue;
            }
            collect_lexical_entries_inner(project_root, &path, entries)?;
            continue;
        }
        if !should_index_file(&name) {
            continue;
        }
        let metadata = entry.metadata().ok();
        if metadata
            .as_ref()
            .and_then(|meta| meta.len().try_into().ok())
            .is_some_and(|len: usize| len > MAX_FILE_BYTES)
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(LexicalIndexEntry { path: rel, content });
    }
    Ok(())
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | "dist" | "build" | ".codestory" | "__pycache__"
    )
}

fn should_index_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(lower.as_str(), "lib.rs" | "mod.rs" | "main.rs")
        || lower.ends_with(".rs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".py")
        || lower.ends_with(".go")
        || lower.ends_with(".java")
        || lower.ends_with(".c")
        || lower.ends_with(".cpp")
        || lower.ends_with(".h")
        || lower.ends_with(".hpp")
        || lower.ends_with(".cs")
        || lower.ends_with(".md")
}

pub fn shard_dir_for(zoekt_data_dir: &Path, project_id: &str) -> PathBuf {
    zoekt_data_dir.join("shards").join(project_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lexical_index_finds_repo_relative_paths() {
        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("lib.rs"),
            "pub fn extension_service() {}",
        )
        .expect("write");
        let zoekt_root = TempDir::new().expect("zoekt");
        build_zoekt_shard(project.path(), zoekt_root.path(), "abc123", false).expect("build");
        let shard = shard_dir_for(zoekt_root.path(), "abc123");
        assert!(shard_has_lexical_index(&shard));
        let hits = search_lexical_index(&shard, "extension", 8).expect("search");
        assert!(hits.iter().any(|hit| hit.path == "lib.rs"));
    }

    #[test]
    fn shard_match_requires_current_lexical_hash_metadata() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn alpha() {}").expect("write");
        let zoekt_root = TempDir::new().expect("zoekt");
        let fingerprint = lexical_input_fingerprint(project.path()).expect("fingerprint");
        build_zoekt_shard(project.path(), zoekt_root.path(), "generation", false).expect("build");

        assert!(shard_matches_lexical_input(
            zoekt_root.path(),
            "generation",
            fingerprint.file_count,
            &fingerprint.hash
        ));
        assert!(!shard_matches_lexical_input(
            zoekt_root.path(),
            "generation",
            fingerprint.file_count,
            "not-the-current-hash"
        ));
    }

    #[test]
    fn lexical_search_scores_all_matches_before_truncating() {
        let zoekt_root = TempDir::new().expect("zoekt");
        let shard = zoekt_root.path();
        let weak = LexicalIndexEntry {
            path: "src/a_weak.rs".into(),
            content: "handler mentioned once".into(),
        };
        let strong = LexicalIndexEntry {
            path: "src/z_strong_handler.rs".into(),
            content: "handler handler handler".into(),
        };
        let lines = [weak, strong]
            .into_iter()
            .map(|entry| serde_json::to_string(&entry).expect("serialize"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(shard.join(LEXICAL_INDEX_FILE), lines).expect("write index");

        let hits = search_lexical_index(shard, "handler", 1).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/z_strong_handler.rs");
    }

    #[test]
    fn lexical_index_does_not_stop_at_legacy_smoke_cap() {
        let project = TempDir::new().expect("project");
        let src = project.path().join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        for index in 0..4_100 {
            std::fs::write(
                src.join(format!("file_{index:04}.ts")),
                format!("export const symbol_{index:04} = {index};\n"),
            )
            .expect("write source file");
        }

        let zoekt_root = TempDir::new().expect("zoekt");
        build_zoekt_shard(project.path(), zoekt_root.path(), "large", false).expect("build");
        let shard = shard_dir_for(zoekt_root.path(), "large");
        let hits = search_lexical_index(&shard, "symbol_4099", 4).expect("search");

        assert!(
            hits.iter().any(|hit| hit.path == "src/file_4099.ts"),
            "large-repo lexical shard should include files after the old 4096-file cap"
        );
    }

    #[test]
    fn lexical_search_tie_breaks_by_path() {
        let zoekt_root = TempDir::new().expect("zoekt");
        let shard = zoekt_root.path();
        let later = LexicalIndexEntry {
            path: "src/b.rs".into(),
            content: "handler".into(),
        };
        let earlier = LexicalIndexEntry {
            path: "src/a.rs".into(),
            content: "handler".into(),
        };
        let lines = [later, earlier]
            .into_iter()
            .map(|entry| serde_json::to_string(&entry).expect("serialize"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(shard.join(LEXICAL_INDEX_FILE), lines).expect("write index");

        let hits = search_lexical_index(shard, "handler", 2).expect("search");
        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs",]
        );
    }

    #[test]
    fn lexical_search_uses_partial_matching_for_broad_prompts() {
        let zoekt_root = TempDir::new().expect("zoekt");
        let shard = zoekt_root.path();
        let source = LexicalIndexEntry {
            path: "workspace/app/src/event_processor_with_jsonl_output.rs".into(),
            content: "jsonl event output request runtime turn start".into(),
        };
        let test = LexicalIndexEntry {
            path: "workspace/app/tests/event_processor_with_json_output.rs".into(),
            content: "json event output test approval fixture".into(),
        };
        let unrelated = LexicalIndexEntry {
            path: "workspace/core/src/session.rs".into(),
            content: "session bookkeeping".into(),
        };
        let generic_agent_doc = LexicalIndexEntry {
            path: ".agents/skills/review/SKILL.md".into(),
            content: "request json cli runtime thread turn start event output".into(),
        };
        let generated_schema = LexicalIndexEntry {
            path: "workspace/app-protocol/schema/typescript/v2/CommandRequestParams.ts".into(),
            content: "app server command request turn start request".into(),
        };
        let lines = [test, unrelated, generic_agent_doc, generated_schema, source]
            .into_iter()
            .map(|entry| serde_json::to_string(&entry).expect("serialize"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(shard.join(LEXICAL_INDEX_FILE), lines).expect("write index");

        let hits = search_lexical_index(
            shard,
            "Explain how `app request --json` flows from CLI into runtime thread turn start JSONL event output",
            4,
        )
        .expect("search");

        assert!(!hits.is_empty());
        assert_eq!(
            hits.first().map(|hit| hit.path.as_str()),
            Some("workspace/app/src/event_processor_with_jsonl_output.rs")
        );
        assert!(
            hits.iter()
                .all(|hit| hit.path != "workspace/core/src/session.rs")
        );
        assert!(
            hits.iter()
                .all(|hit| hit.path != ".agents/skills/review/SKILL.md")
        );
    }
}
