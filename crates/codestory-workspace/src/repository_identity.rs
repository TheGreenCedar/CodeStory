use serde::Serialize;
use std::path::Path;
use std::process::Command;

pub const REPOSITORY_IDENTITY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryIdentity {
    pub canonical_repository_id: Option<String>,
    pub repository_identity_schema_version: u32,
    pub normalized_repository_identity: Option<String>,
    pub git_remote: Option<String>,
    pub git_tree: Option<String>,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarProjectIdentity {
    pub project_id: String,
    pub canonical_repository_id: Option<String>,
    pub root_derived_project_id: String,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

pub fn inspect_repository_identity(project_root: &Path) -> RepositoryIdentity {
    let remote = git_output(project_root, &["config", "--get", "remote.origin.url"]).ok();
    let tree = git_output(project_root, &["rev-parse", "HEAD^{tree}"]).ok();
    let dirty = git_output(project_root, &["status", "--porcelain"])
        .map(|status| !status.trim().is_empty())
        .unwrap_or(true);
    let normalized = remote.as_deref().and_then(normalize_repository_identity);
    let canonical_repository_id = normalized.as_deref().map(canonical_repository_id);
    let (portable_reuse_eligible, portable_reuse_reason) =
        portable_reuse_status(normalized.as_deref(), tree.as_deref(), dirty);

    RepositoryIdentity {
        canonical_repository_id,
        repository_identity_schema_version: REPOSITORY_IDENTITY_SCHEMA_VERSION,
        normalized_repository_identity: normalized,
        git_remote: remote,
        git_tree: tree,
        portable_reuse_eligible,
        portable_reuse_reason,
    }
}

pub fn sidecar_project_identity(
    project_root: &Path,
    root_derived_project_id: String,
) -> SidecarProjectIdentity {
    let identity = inspect_repository_identity(project_root);
    let project_id = if identity.portable_reuse_eligible {
        identity
            .canonical_repository_id
            .clone()
            .unwrap_or_else(|| root_derived_project_id.clone())
    } else {
        root_derived_project_id.clone()
    };

    SidecarProjectIdentity {
        project_id,
        canonical_repository_id: identity.canonical_repository_id,
        root_derived_project_id,
        portable_reuse_eligible: identity.portable_reuse_eligible,
        portable_reuse_reason: identity.portable_reuse_reason,
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
    let value = remote.trim().replace('\\', "/");
    if value.is_empty() {
        return None;
    }

    if let Some((_, rest)) = value.split_once("://") {
        return normalize_url_repository_identity(rest);
    }

    let without_user = strip_userinfo(&value);
    let scp_like = without_user.find(':').is_some_and(|colon| {
        without_user[..colon].find('/').is_none() && without_user[colon + 1..].contains('/')
    });
    let normalized = if scp_like {
        without_user.replacen(':', "/", 1)
    } else {
        without_user.to_string()
    };
    normalize_repository_path(&normalized)
}

fn normalize_url_repository_identity(rest: &str) -> Option<String> {
    let rest = strip_userinfo(rest);
    let (authority, path) = rest.split_once('/')?;
    let host = authority
        .split_once(':')
        .map_or(authority, |(host, _)| host);
    normalize_repository_path(&format!("{host}/{path}"))
}

fn strip_userinfo(value: &str) -> &str {
    value.split_once('@').map(|(_, rest)| rest).unwrap_or(value)
}

fn normalize_repository_path(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let normalized = lower
        .trim_start_matches('/')
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .to_string();
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
        for (remote, expected) in [
            (
                "https://github.com/TheGreenCedar/CodeStory.git",
                "github.com/thegreencedar/codestory",
            ),
            (
                "ssh://git@github.com/TheGreenCedar/CodeStory.git",
                "github.com/thegreencedar/codestory",
            ),
            (
                "ssh://git@github.com:22/TheGreenCedar/CodeStory.git",
                "github.com/thegreencedar/codestory",
            ),
            (
                "git@github.com:TheGreenCedar/CodeStory.git",
                "github.com/thegreencedar/codestory",
            ),
            (
                "https://github.com/TheGreenCedar/CodeStory.git/",
                "github.com/thegreencedar/codestory",
            ),
            (
                "HTTPS://GITHUB.COM/TheGreenCedar/CodeStory.GIT",
                "github.com/thegreencedar/codestory",
            ),
        ] {
            assert_eq!(
                normalize_repository_identity(remote).as_deref(),
                Some(expected),
                "remote: {remote}"
            );
        }
    }

    #[test]
    fn portable_reuse_eligibility_fails_closed_without_identity_or_clean_tree() {
        assert_eq!(
            portable_reuse_status(None, Some("tree"), false),
            (false, "git_remote_missing".into())
        );
        assert_eq!(
            portable_reuse_status(Some("github.com/org/repo"), None, false),
            (false, "git_tree_unavailable".into())
        );
        assert_eq!(
            portable_reuse_status(Some("github.com/org/repo"), Some("tree"), true),
            (false, "git_worktree_dirty".into())
        );
    }

    #[test]
    fn sidecar_project_identity_uses_canonical_id_only_when_clean_and_identifiable() {
        let Some(project) = git_project() else {
            return;
        };

        let clean = sidecar_project_identity(project.path(), "root-id".into());
        assert!(clean.portable_reuse_eligible);
        assert_eq!(clean.project_id, clean.canonical_repository_id.unwrap());

        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        let dirty = sidecar_project_identity(project.path(), "root-id".into());
        assert!(!dirty.portable_reuse_eligible);
        assert_eq!(dirty.portable_reuse_reason, "git_worktree_dirty");
        assert_eq!(dirty.project_id, "root-id");
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
