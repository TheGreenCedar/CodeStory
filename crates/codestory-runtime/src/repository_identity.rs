use serde::Serialize;
use std::path::Path;
use std::process::Command;

pub const REPOSITORY_IDENTITY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryIdentityReport {
    pub project: String,
    pub root_derived_project_id: String,
    pub canonical_repository_id: Option<String>,
    pub repository_identity_schema_version: u32,
    pub normalized_repository_identity: Option<String>,
    pub git_remote: Option<String>,
    pub git_tree: Option<String>,
    pub cache_schema_version: u32,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
    pub freshness_inputs: Vec<String>,
}

pub fn inspect_repository_identity(project_root: &Path) -> RepositoryIdentityReport {
    let root_derived_project_id = codestory_retrieval::project_id_for_root(project_root);
    let remote = git_output(project_root, &["config", "--get", "remote.origin.url"]).ok();
    let tree = git_output(project_root, &["rev-parse", "HEAD^{tree}"]).ok();
    let dirty = git_output(project_root, &["status", "--porcelain"])
        .map(|status| !status.trim().is_empty())
        .unwrap_or(true);
    let normalized = remote.as_deref().and_then(normalize_repository_identity);
    let canonical_repository_id = normalized.as_deref().map(canonical_repository_id);
    let (portable_reuse_eligible, portable_reuse_reason) =
        portable_reuse_status(normalized.as_deref(), tree.as_deref(), dirty);

    RepositoryIdentityReport {
        project: project_root.to_string_lossy().to_string(),
        root_derived_project_id,
        canonical_repository_id,
        repository_identity_schema_version: REPOSITORY_IDENTITY_SCHEMA_VERSION,
        normalized_repository_identity: normalized,
        git_remote: remote,
        git_tree: tree,
        cache_schema_version: codestory_store::CURRENT_SCHEMA_VERSION,
        portable_reuse_eligible,
        portable_reuse_reason,
        freshness_inputs: vec![
            "git_tree".into(),
            "cache_schema_version".into(),
            "sidecar_input_hash".into(),
        ],
    }
}

fn portable_reuse_status(
    normalized: Option<&str>,
    tree: Option<&str>,
    dirty: bool,
) -> (bool, String) {
    if normalized.is_none() {
        return (false, "git_remote_missing".into());
    }
    if tree.is_none() {
        return (false, "git_tree_unavailable".into());
    }
    if dirty {
        return (false, "git_worktree_dirty".into());
    }
    (true, "eligible".into())
}

fn canonical_repository_id(normalized_repository_identity: &str) -> String {
    let mut state = 0xcbf29ce484222325_u64;
    mix_str(&mut state, "codestory-repository-identity");
    mix_u32(&mut state, REPOSITORY_IDENTITY_SCHEMA_VERSION);
    mix_str(&mut state, normalized_repository_identity);
    format!("repo-v{REPOSITORY_IDENTITY_SCHEMA_VERSION}-{state:016x}")
}

fn normalize_repository_identity(remote: &str) -> Option<String> {
    let value = remote
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .replace('\\', "/");
    if value.is_empty() {
        return None;
    }

    let without_scheme = value
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(value.as_str());
    let without_user = without_scheme
        .split_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(without_scheme);
    let scp_like = without_user.find(':').is_some_and(|colon| {
        without_user[..colon].find('/').is_none() && without_user[colon + 1..].contains('/')
    });
    let normalized = if scp_like {
        without_user.replacen(':', "/", 1)
    } else {
        without_user.to_string()
    };
    let normalized = normalized
        .trim_start_matches('/')
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn git_output(project: &Path, args: &[&str]) -> Result<String, ()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn mix_u32(state: &mut u64, value: u32) {
    for byte in value.to_le_bytes() {
        *state ^= u64::from(byte);
        *state = state.wrapping_mul(0x00000100000001B3);
    }
}

fn mix_str(state: &mut u64, value: &str) {
    for byte in value.as_bytes() {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(0x00000100000001B3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn normalizes_common_git_remote_forms() {
        let https = normalize_repository_identity("https://github.com/TheGreenCedar/CodeStory.git")
            .expect("https remote");
        let ssh = normalize_repository_identity("git@github.com:TheGreenCedar/CodeStory.git")
            .expect("ssh remote");

        assert_eq!(https, "github.com/thegreencedar/codestory");
        assert_eq!(https, ssh);
    }

    #[test]
    fn canonical_identity_uses_repo_identity_not_tree_state() {
        let Some(project) = git_project() else {
            return;
        };

        let first = inspect_repository_identity(project.path());
        fs::write(project.path().join("lib.rs"), "pub fn changed() {}\n").expect("write source");
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "change"]);
        let second = inspect_repository_identity(project.path());

        assert_eq!(
            first.canonical_repository_id,
            second.canonical_repository_id
        );
        assert_ne!(first.git_tree, second.git_tree);
        assert!(first.portable_reuse_eligible);
        assert!(second.portable_reuse_eligible);
    }

    fn git_project() -> Option<tempfile::TempDir> {
        if Command::new("git").arg("--version").output().is_err() {
            return None;
        }
        let project = tempdir().expect("project");
        git(project.path(), &["init"]);
        git(
            project.path(),
            &["config", "user.email", "codestory@example.invalid"],
        );
        git(project.path(), &["config", "user.name", "CodeStory Test"]);
        git(
            project.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/TheGreenCedar/CodeStory.git",
            ],
        );
        fs::write(project.path().join("lib.rs"), "pub fn run() {}\n").expect("write source");
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "init"]);
        Some(project)
    }

    fn git(project: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
