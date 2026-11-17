use gix::bstr::ByteSlice;
use gix::objs::TreeRefIter;
use gix::{ObjectId, Repository};
use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const DEFAULT_MAX_COMMITS: usize = 150;
const MAX_FILES_PER_COMMIT: usize = 30;
const MAX_COUPLED_PER_FILE: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct CochangeEntry {
    pub path: String,
    pub count: u32,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelatedTestOrFixture {
    pub path: String,
    pub score: f32,
    pub is_fixture: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CochangeIndex {
    entries: HashMap<String, Vec<CochangeEntry>>,
}

impl CochangeIndex {
    pub fn build(root: &Path, max_commits: usize) -> Option<Self> {
        let repo = open_isolated_repo(root)?;
        let head_id = repo.head_id().ok()?.detach();
        let mut file_counts = HashMap::<String, u32>::new();
        let mut pair_counts = HashMap::<String, HashMap<String, u32>>::new();

        for changed in walk_commit_file_sets(&repo, head_id, max_commits, None) {
            if changed.is_empty() || changed.len() > MAX_FILES_PER_COMMIT {
                continue;
            }
            for file in &changed {
                *file_counts.entry(file.clone()).or_insert(0) += 1;
            }
            for left in &changed {
                let row = pair_counts.entry(left.clone()).or_default();
                for right in &changed {
                    if left != right {
                        *row.entry(right.clone()).or_insert(0) += 1;
                    }
                }
            }
        }

        let mut entries = HashMap::new();
        for (source, coupled) in pair_counts {
            let total = file_counts.get(&source).copied().unwrap_or(1).max(1);
            let mut list = coupled
                .into_iter()
                .map(|(path, count)| CochangeEntry {
                    path,
                    count,
                    confidence: count as f32 / total as f32,
                })
                .collect::<Vec<_>>();
            sort_and_truncate_cochange(&mut list);
            if !list.is_empty() {
                entries.insert(source, list);
            }
        }
        Some(Self { entries })
    }

    pub fn coupled_files(&self, path: &str) -> &[CochangeEntry] {
        let normalized = normalize_rel_path(path);
        self.entries
            .get(&normalized)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Fast targeted co-change lookup for a single project-relative path `source_path`.
/// Only diffs commits where `source_path`'s blob OID actually changed.
pub fn coupled_to_path(root: &Path, source_path: &str, max_commits: usize) -> Vec<CochangeEntry> {
    let normalized = normalize_rel_path(source_path);
    if normalized.is_empty() {
        return Vec::new();
    }
    let Some(repo) = open_isolated_repo(root) else {
        return Vec::new();
    };
    let Ok(head_id) = repo.head_id().map(|id| id.detach()) else {
        return Vec::new();
    };
    let mut source_commits = 0u32;
    let mut counts = HashMap::<String, u32>::new();
    for changed in walk_commit_file_sets(&repo, head_id, max_commits, Some(&normalized)) {
        if changed.len() <= 1 || changed.len() > MAX_FILES_PER_COMMIT {
            continue;
        }
        source_commits += 1;
        for other in changed {
            if other != normalized {
                *counts.entry(other).or_insert(0) += 1;
            }
        }
    }
    if source_commits == 0 {
        return Vec::new();
    }
    let mut list = counts
        .into_iter()
        .map(|(path, count)| CochangeEntry {
            path,
            count,
            confidence: count as f32 / source_commits as f32,
        })
        .collect::<Vec<_>>();
    sort_and_truncate_cochange(&mut list);
    list
}

/// Ranks related test and fixture files for `source_path` by combining git
/// co-change history with structural sibling/directory conventions.
pub fn find_related_tests_and_fixtures(
    root: &Path,
    source_path: &str,
    max_results: usize,
) -> Vec<RelatedTestOrFixture> {
    find_related_tests_and_fixtures_in_paths(root, source_path, None, &[], max_results)
}

/// Ranks related test, fixture, and coupled configuration files for `source_path`,
/// optionally scanning `repo_paths` across the whole repository to locate cross-tree
/// tests and declarative fixtures (`tests/responses/test_fileresponse.py`,
/// `tests/specs/task/.../__test__.jsonc`, `*.golden*`).
pub fn find_related_tests_and_fixtures_in_paths(
    root: &Path,
    source_path: &str,
    repo_paths: Option<&[String]>,
    extra_terms: &[&str],
    max_results: usize,
) -> Vec<RelatedTestOrFixture> {
    if max_results == 0 {
        return Vec::new();
    }
    let normalized = normalize_rel_path(source_path);
    if normalized.is_empty() {
        return Vec::new();
    }
    let mut scores = HashMap::<String, f32>::new();

    for entry in coupled_to_path(root, &normalized, DEFAULT_MAX_COMMITS) {
        if root.join(&entry.path).is_file() {
            let weight = if is_test_or_fixture_path(&entry.path) {
                1.35
            } else {
                0.90
            };
            let score = entry.confidence * weight + (entry.count.min(5) as f32 * 0.15);
            scores
                .entry(entry.path)
                .and_modify(|existing| *existing = existing.max(score))
                .or_insert(score);
        }
    }

    for (candidate, structural_score) in structural_test_and_fixture_candidates(root, &normalized) {
        if candidate != normalized {
            scores
                .entry(candidate)
                .and_modify(|existing| *existing += structural_score * 0.5)
                .or_insert(structural_score);
        }
    }

    if let Some(paths) = repo_paths {
        for (candidate, repo_score) in
            repo_wide_test_and_fixture_candidates(&normalized, paths, extra_terms)
        {
            if candidate != normalized {
                scores
                    .entry(candidate)
                    .and_modify(|existing| *existing = existing.max(repo_score) + 0.15)
                    .or_insert(repo_score);
            }
        }
    }

    let mut ranked = scores
        .into_iter()
        .map(|(path, score)| RelatedTestOrFixture {
            is_fixture: is_fixture_only_path(&path),
            path,
            score,
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    ranked.truncate(max_results);
    ranked
}

pub fn is_test_or_fixture_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(&lower);
    is_fixture_only_path(&lower)
        || file_name.ends_with("_test.go")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_test.py")
        || (file_name.starts_with("test_") && file_name.ends_with(".py"))
        || file_name == "tests.py"
        || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.contains("/__tests__/")
}

pub fn is_fixture_only_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(&lower);
    file_name == "__test__.jsonc"
        || file_name.contains(".golden")
        || file_name.ends_with(".snap")
        || lower.starts_with("testdata/")
        || lower.contains("/testdata/")
        || lower.starts_with("fixtures/")
        || lower.contains("/fixtures/")
        || lower.starts_with("test_data/")
        || lower.contains("/test_data/")
        || lower.starts_with("tests/specs/")
        || lower.contains("/tests/specs/")
}

fn split_identifier_tokens(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in raw.chars() {
        if !ch.is_ascii_alphanumeric() {
            if current.len() >= 3 {
                out.push(current.to_ascii_lowercase());
            }
            current.clear();
            prev_lower = false;
            continue;
        }
        if prev_lower && ch.is_ascii_uppercase() && current.len() >= 3 {
            out.push(current.to_ascii_lowercase());
            current.clear();
        }
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        current.push(ch);
    }
    if current.len() >= 3 {
        out.push(current.to_ascii_lowercase());
    }
    out
}

fn is_generic_path_token(tok: &str) -> bool {
    matches!(
        tok,
        "src"
            | "pkg"
            | "lib"
            | "libs"
            | "cli"
            | "app"
            | "internal"
            | "mod"
            | "main"
            | "index"
            | "init"
            | "utils"
            | "util"
            | "common"
            | "helpers"
            | "helper"
            | "test"
            | "tests"
            | "spec"
            | "specs"
            | "testdata"
            | "fixtures"
            | "golden"
            | "json"
            | "jsonc"
            | "yaml"
            | "toml"
            | "rust"
            | "python"
            | "golang"
    )
}

fn repo_wide_test_and_fixture_candidates(
    source_path: &str,
    repo_paths: &[String],
    extra_terms: &[&str],
) -> Vec<(String, f32)> {
    let mut source_tokens = BTreeSet::new();
    let parts = source_path.split('/').collect::<Vec<_>>();
    let dir_parts = &parts[..parts.len().saturating_sub(1)];
    for seg in dir_parts.iter().rev().take(3) {
        for tok in split_identifier_tokens(seg) {
            if !is_generic_path_token(&tok) {
                source_tokens.insert(tok);
            }
        }
    }
    if let Some(stem) = Path::new(source_path).file_stem().and_then(|s| s.to_str()) {
        for tok in split_identifier_tokens(stem) {
            if !is_generic_path_token(&tok) {
                source_tokens.insert(tok);
            }
        }
    }
    let mut query_tokens = BTreeSet::new();
    for term in extra_terms {
        for tok in split_identifier_tokens(term) {
            if !is_generic_path_token(&tok) && tok.len() >= 4 {
                query_tokens.insert(tok);
            }
        }
    }
    if source_tokens.is_empty() && query_tokens.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for candidate in repo_paths {
        if candidate == source_path || !is_test_or_fixture_path(candidate) {
            continue;
        }
        let lower = candidate.to_ascii_lowercase();
        let file_name = lower.rsplit('/').next().unwrap_or(&lower);
        let mut score = 0.0_f32;
        let mut matched_source = 0usize;
        let mut matched_query = 0usize;
        for tok in &source_tokens {
            if lower.contains(tok.as_str()) {
                matched_source += 1;
                score += if file_name.contains(tok.as_str()) {
                    0.55
                } else {
                    0.40
                };
            }
        }
        for tok in &query_tokens {
            if lower.contains(tok.as_str()) {
                matched_query += 1;
                score += if file_name.contains(tok.as_str()) {
                    0.50
                } else {
                    0.35
                };
            }
        }
        if matched_source + matched_query >= 2 || (matched_source >= 1 && matched_query >= 1) {
            if is_fixture_only_path(&lower) {
                score += 0.25;
            }
            out.push((candidate.clone(), score));
        }
    }
    out.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    out.truncate(8);
    out
}

fn structural_test_and_fixture_candidates(root: &Path, source_path: &str) -> Vec<(String, f32)> {
    let rel = Path::new(source_path);
    let Some(stem) = rel.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let base_stem = stem
        .strip_suffix("_test")
        .or_else(|| stem.strip_prefix("test_"))
        .or_else(|| stem.strip_suffix(".test"))
        .or_else(|| stem.strip_suffix(".spec"))
        .unwrap_or(stem);
    let parent = rel.parent().unwrap_or_else(|| Path::new(""));
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |candidate: String, score: f32| {
        let normalized = normalize_rel_path(&candidate);
        if !normalized.is_empty()
            && normalized != source_path
            && root.join(&normalized).is_file()
            && seen.insert(normalized.clone())
        {
            out.push((normalized, score));
        }
    };

    let join_parent = |name: &str| -> String {
        if parent.as_os_str().is_empty() {
            name.to_string()
        } else {
            format!(
                "{}/{name}",
                parent.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/")
            )
        }
    };

    for sibling in [
        format!("{base_stem}_test.go"),
        format!("{base_stem}_test.rs"),
        format!("test_{base_stem}.py"),
        format!("{base_stem}_test.py"),
        format!("{base_stem}.test.ts"),
        format!("{base_stem}.test.tsx"),
        format!("{base_stem}.spec.ts"),
        format!("{base_stem}.spec.tsx"),
        format!("{base_stem}.test.js"),
        format!("{base_stem}.spec.js"),
        "__test__.jsonc".to_string(),
        "tests.py".to_string(),
    ] {
        let score = if sibling == "__test__.jsonc" || sibling.contains(base_stem) {
            0.95
        } else {
            0.70
        };
        push(join_parent(&sibling), score);
    }

    let abs_parent = root.join(parent);
    if let Ok(entries) = std::fs::read_dir(&abs_parent) {
        for entry in entries.flatten().take(64) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.is_file() {
                let lower = name.to_ascii_lowercase();
                if lower == "__test__.jsonc"
                    || lower.contains(".golden")
                    || (lower.contains(&base_stem.to_ascii_lowercase())
                        && is_test_or_fixture_path(name))
                {
                    push(join_parent(name), 0.90);
                }
            } else if path.is_dir()
                && matches!(
                    name.to_ascii_lowercase().as_str(),
                    "testdata" | "fixtures" | "test_data" | "__tests__" | "tests"
                )
                && let Ok(sub_entries) = std::fs::read_dir(&path)
            {
                for sub in sub_entries.flatten().take(32) {
                    let sub_path = sub.path();
                    if sub_path.is_file()
                        && let Ok(rel_sub) = sub_path.strip_prefix(root)
                    {
                        let rel_str = rel_sub
                            .to_string_lossy()
                            .replace(std::path::MAIN_SEPARATOR, "/");
                        let sub_name = sub_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        let score = if sub_name.contains(&base_stem.to_ascii_lowercase())
                            || sub_name == "__test__.jsonc"
                        {
                            0.88
                        } else {
                            0.65
                        };
                        push(rel_str, score);
                    }
                }
            }
        }
    }

    out
}

fn sort_and_truncate_cochange(list: &mut Vec<CochangeEntry>) {
    list.sort_by(|left, right| {
        Reverse(left.count)
            .cmp(&Reverse(right.count))
            .then_with(|| right.confidence.total_cmp(&left.confidence))
            .then_with(|| left.path.cmp(&right.path))
    });
    list.truncate(MAX_COUPLED_PER_FILE);
}

fn normalize_rel_path(path: &str) -> String {
    path.trim_start_matches("./")
        .replace('\\', "/")
        .trim_matches('/')
        .to_string()
}

fn open_isolated_repo(root: &Path) -> Option<Repository> {
    gix::open_opts(root, gix::open::Options::isolated()).ok()
}

fn walk_commit_file_sets(
    repo: &Repository,
    head_id: ObjectId,
    max_commits: usize,
    required_path: Option<&str>,
) -> Vec<BTreeSet<String>> {
    let mut out = Vec::new();
    let mut current = Some(head_id);
    let mut visited = 0usize;
    let required_parts = required_path
        .map(|path| path.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>());

    while let Some(commit_id) = current {
        if visited >= max_commits {
            break;
        }
        visited += 1;
        let Ok(commit_obj) = repo.find_object(commit_id) else {
            break;
        };
        let Ok(commit) = commit_obj.try_into_commit() else {
            break;
        };
        let Ok(commit_ref) = commit.decode() else {
            break;
        };
        let mut parents = commit_ref.parents();
        let first_parent = parents.next();
        if parents.next().is_some() {
            // Skip merge commits; follow first parent.
            current = first_parent;
            continue;
        }
        let Ok(tree_id) = commit.tree_id() else {
            break;
        };
        let tree_oid = tree_id.detach();
        let parent_tree_oid = first_parent.and_then(|parent_id| {
            repo.find_object(parent_id)
                .ok()?
                .try_into_commit()
                .ok()?
                .tree_id()
                .ok()
                .map(|id| id.detach())
        });

        if let Some(parts) = &required_parts {
            let now_oid = lookup_path_oid(repo, Some(tree_oid), parts);
            let prev_oid = lookup_path_oid(repo, parent_tree_oid, parts);
            if now_oid == prev_oid {
                current = first_parent;
                continue;
            }
        }

        let mut changed = BTreeSet::new();
        diff_trees(
            repo,
            parent_tree_oid,
            Some(tree_oid),
            "",
            &mut changed,
            MAX_FILES_PER_COMMIT + 1,
        );
        if !changed.is_empty() && changed.len() <= MAX_FILES_PER_COMMIT {
            out.push(changed);
        }
        current = first_parent;
    }
    out
}

fn lookup_path_oid(repo: &Repository, tree_id: Option<ObjectId>, parts: &[&str]) -> Option<ObjectId> {
    let mut current = tree_id?;
    let hash_kind = repo.object_hash();
    for (index, part) in parts.iter().enumerate() {
        let obj = repo.find_object(current).ok()?;
        let mut found = None;
        for entry in TreeRefIter::from_bytes(&obj.data, hash_kind).flatten() {
            if entry.filename.to_str().ok() == Some(*part) {
                found = Some((entry.oid.to_owned(), entry.mode.is_tree()));
                break;
            }
        }
        let (oid, is_tree) = found?;
        if index + 1 == parts.len() {
            return Some(oid);
        }
        if !is_tree {
            return None;
        }
        current = oid;
    }
    None
}

fn diff_trees(
    repo: &Repository,
    old_tree: Option<ObjectId>,
    new_tree: Option<ObjectId>,
    prefix: &str,
    changed: &mut BTreeSet<String>,
    limit: usize,
) {
    if old_tree == new_tree || changed.len() >= limit {
        return;
    }
    let old_entries = read_tree_entries(repo, old_tree);
    let new_entries = read_tree_entries(repo, new_tree);
    let mut names = BTreeSet::new();
    for name in old_entries.keys().chain(new_entries.keys()) {
        names.insert(name.as_str());
    }
    for name in names {
        if changed.len() >= limit {
            break;
        }
        let rel = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        match (old_entries.get(name), new_entries.get(name)) {
            (Some((old_oid, old_is_tree)), Some((new_oid, new_is_tree))) => {
                if old_oid == new_oid && old_is_tree == new_is_tree {
                    continue;
                }
                if *old_is_tree || *new_is_tree {
                    diff_trees(
                        repo,
                        (*old_is_tree).then_some(*old_oid),
                        (*new_is_tree).then_some(*new_oid),
                        &rel,
                        changed,
                        limit,
                    );
                } else {
                    changed.insert(rel);
                }
            }
            (Some((old_oid, true)), None) => {
                diff_trees(repo, Some(*old_oid), None, &rel, changed, limit);
            }
            (None, Some((new_oid, true))) => {
                diff_trees(repo, None, Some(*new_oid), &rel, changed, limit);
            }
            (Some(_), None) | (None, Some(_)) => {
                changed.insert(rel);
            }
            (None, None) => {}
        }
    }
}

fn read_tree_entries(
    repo: &Repository,
    tree_id: Option<ObjectId>,
) -> HashMap<String, (ObjectId, bool)> {
    let Some(id) = tree_id else {
        return HashMap::new();
    };
    let Ok(obj) = repo.find_object(id) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for entry in TreeRefIter::from_bytes(&obj.data, repo.object_hash()).flatten() {
        if let Ok(name) = entry.filename.to_str() {
            map.insert(name.to_string(), (entry.oid.to_owned(), entry.mode.is_tree()));
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn run_git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git command should run");
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn cochange_tracks_coupled_commits_and_ranks_fixtures() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        run_git(root, &["init"]);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests/fixtures")).unwrap();

        for i in 1..=3 {
            fs::write(root.join("src/engine.rs"), format!("pub fn v{i}() {{}}\n")).unwrap();
            fs::write(
                root.join("tests/fixtures/__test__.jsonc"),
                format!("{{\"v\": {i}}}\n"),
            )
            .unwrap();
            run_git(root, &["add", "."]);
            run_git(root, &["commit", "-m", &format!("c{i}")]);
        }

        let coupled = coupled_to_path(root, "src/engine.rs", 50);
        assert_eq!(coupled.len(), 1);
        assert_eq!(coupled[0].path, "tests/fixtures/__test__.jsonc");
        assert_eq!(coupled[0].count, 3);
        assert!((coupled[0].confidence - 1.0).abs() < f32::EPSILON);

        let index = CochangeIndex::build(root, 50).expect("cochange index");
        assert_eq!(
            index.coupled_files("src/engine.rs")[0].path,
            "tests/fixtures/__test__.jsonc"
        );

        let related = find_related_tests_and_fixtures(root, "src/engine.rs", 5);
        assert!(!related.is_empty());
        assert_eq!(related[0].path, "tests/fixtures/__test__.jsonc");
        assert!(related[0].is_fixture);
    }
}
