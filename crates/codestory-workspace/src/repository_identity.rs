use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Version of the repository identity hashing contract.
pub const REPOSITORY_IDENTITY_SCHEMA_VERSION: u32 = 1;

/// Shared project identity contract version.
pub const PROJECT_IDENTITY_SCHEMA_VERSION: u32 = 2;
const PROJECT_IDENTITY_OBSERVATION_CACHE_TTL: Duration = Duration::from_secs(1);

static PROJECT_IDENTITY_OBSERVATION_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, (Instant, ProjectIdentityV2)>>,
> = OnceLock::new();

/// Git-derived identity used to decide whether portable sidecar cache reuse is
/// safe for a project root.
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

/// Stable logical, workspace, and artifact identities for a project root.
///
/// `project_id` identifies the repository independently of checkout state when
/// a canonical repository identity is available. `workspace_id` always scopes
/// to one canonical root. `artifact_scope_id` fails closed to that workspace
/// whenever portable reuse is not eligible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectIdentityV2 {
    pub project_identity_schema_version: u32,
    pub project_id: String,
    pub workspace_id: String,
    pub artifact_scope_id: String,
    pub canonical_repository_id: Option<String>,
    pub legacy_raw_root_project_id: Option<String>,
    pub normalized_root_project_id_alias: Option<String>,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

/// Project id decision for sidecar artifacts.
///
/// Clean, identifiable Git repositories use a stable repository-derived id.
/// Dirty, missing, or non-Git roots fall back to the root-derived id so cached
/// sidecars do not cross an unsafe freshness boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarProjectIdentity {
    pub project_id: String,
    pub canonical_repository_id: Option<String>,
    pub root_derived_project_id: String,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

/// Inspect Git remote, tree, and dirtiness for portable cache reuse.
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

/// Resolve the shared V2 identity contract for a project root.
pub fn project_identity_v2(project_root: &Path) -> ProjectIdentityV2 {
    let repository_identity = inspect_repository_identity(project_root);
    let root_identity = workspace_root_identity(project_root);
    let project_id = repository_identity
        .canonical_repository_id
        .clone()
        .unwrap_or_else(|| root_identity.workspace_id.clone());
    let artifact_scope_id = if repository_identity.portable_reuse_eligible {
        project_id.clone()
    } else {
        root_identity.workspace_id.clone()
    };

    ProjectIdentityV2 {
        project_identity_schema_version: PROJECT_IDENTITY_SCHEMA_VERSION,
        project_id,
        workspace_id: root_identity.workspace_id,
        artifact_scope_id,
        canonical_repository_id: repository_identity.canonical_repository_id,
        legacy_raw_root_project_id: root_identity.legacy_raw_root_project_id,
        normalized_root_project_id_alias: root_identity.normalized_root_project_id_alias,
        portable_reuse_eligible: repository_identity.portable_reuse_eligible,
        portable_reuse_reason: repository_identity.portable_reuse_reason,
    }
}

/// Resolve identity for repeated observational status reads.
///
/// Mutating/indexing paths must use `project_identity_v2` so dirtiness changes
/// are observed immediately.
pub fn cached_project_identity_v2(project_root: &Path) -> ProjectIdentityV2 {
    let key = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let cache = PROJECT_IDENTITY_OBSERVATION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_at, identity)) = cache.get(&key)
        && cached_at.elapsed() < PROJECT_IDENTITY_OBSERVATION_CACHE_TTL
    {
        return identity.clone();
    }
    let identity = project_identity_v2(project_root);
    cache.insert(key, (Instant::now(), identity.clone()));
    identity
}

/// Return the canonical-root FNV identity used to scope one workspace.
pub fn workspace_id_for_root(project_root: &Path) -> String {
    workspace_root_identity(project_root).workspace_id
}

/// Choose the sidecar project id while preserving the fallback reason.
pub fn sidecar_project_identity(
    project_root: &Path,
    root_derived_project_id: String,
) -> SidecarProjectIdentity {
    let identity = project_identity_v2(project_root);
    let project_id = if identity.portable_reuse_eligible {
        identity.project_id
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceRootIdentity {
    workspace_id: String,
    legacy_raw_root_project_id: Option<String>,
    normalized_root_project_id_alias: Option<String>,
}

fn workspace_root_identity(project_root: &Path) -> WorkspaceRootIdentity {
    let raw_root = project_root.to_string_lossy();
    let canonical_root = fs::canonicalize(project_root)
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    workspace_root_identity_from_text(&raw_root, &canonical_root)
}

fn workspace_root_identity_from_text(
    raw_root: &str,
    canonical_root: &str,
) -> WorkspaceRootIdentity {
    let workspace_id = fnv1a_hex(canonical_root.as_bytes());
    let legacy_raw_root_project_id = fnv_alias(raw_root, &workspace_id);
    let normalized_root_project_id_alias =
        fnv_alias(&normalize_root_identity_text(canonical_root), &workspace_id);

    WorkspaceRootIdentity {
        workspace_id,
        legacy_raw_root_project_id,
        normalized_root_project_id_alias,
    }
}

fn fnv_alias(root: &str, workspace_id: &str) -> Option<String> {
    let legacy_id = fnv1a_hex(root.as_bytes());
    (legacy_id != workspace_id).then_some(legacy_id)
}

fn normalize_root_identity_text(root: &str) -> String {
    let trimmed = root.trim();
    let windows_style = trimmed.contains('\\')
        || trimmed.starts_with("//")
        || trimmed.as_bytes().get(1) == Some(&b':');
    let mut normalized = trimmed.replace('\\', "/");

    if normalized
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/unc/"))
    {
        normalized = format!("//{}", &normalized[8..]);
    } else if normalized.starts_with("//?/") {
        normalized.drain(..4);
    }

    let preserve_unc = normalized.starts_with("//");
    let mut collapsed = String::with_capacity(normalized.len());
    let mut previous_was_separator = false;
    for ch in normalized.chars() {
        if ch == '/' {
            if !previous_was_separator {
                collapsed.push(ch);
            }
            previous_was_separator = true;
        } else {
            collapsed.push(ch);
            previous_was_separator = false;
        }
    }
    if preserve_unc && !collapsed.starts_with("//") {
        collapsed.insert(0, '/');
    }

    while collapsed.len() > 1 && collapsed.ends_with('/') && !is_windows_drive_root(&collapsed) {
        collapsed.pop();
    }

    if windows_style {
        collapsed.make_ascii_lowercase();
    }
    collapsed
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1..] == *b":/"
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

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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

    #[test]
    fn project_id_is_stable_across_dirty_transitions() {
        let Some(project) = git_project() else {
            return;
        };

        let clean = project_identity_v2(project.path());
        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        let dirty = project_identity_v2(project.path());

        assert_eq!(clean.project_id, dirty.project_id);
        assert_eq!(clean.workspace_id, dirty.workspace_id);
        assert!(clean.portable_reuse_eligible);
        assert!(!dirty.portable_reuse_eligible);
    }

    #[test]
    fn worktrees_share_project_id_but_not_workspace_id() {
        let Some(project) = git_project() else {
            return;
        };
        let worktree_parent = tempdir().expect("worktree parent");
        let worktree = worktree_parent.path().join("linked-worktree");
        git(
            project.path(),
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().expect("worktree path"),
                "HEAD",
            ],
        );

        let first = project_identity_v2(project.path());
        let second = project_identity_v2(&worktree);

        assert_eq!(first.project_id, second.project_id);
        assert_ne!(first.workspace_id, second.workspace_id);
        assert_eq!(first.artifact_scope_id, second.artifact_scope_id);
    }

    #[test]
    fn workspace_id_matches_existing_canonical_root_fnv_contract() {
        let project = tempdir().expect("project");
        let canonical = fs::canonicalize(project.path()).expect("canonical project root");
        let expected = fnv1a_hex(canonical.to_string_lossy().as_bytes());

        assert_eq!(workspace_id_for_root(project.path()), expected);
        assert_eq!(project_identity_v2(project.path()).workspace_id, expected);
    }

    #[test]
    fn canonicalization_equates_supported_path_spellings() {
        let project = tempdir().expect("project");
        let dotted = project.path().join(".");

        assert_eq!(
            workspace_id_for_root(project.path()),
            workspace_id_for_root(&dotted)
        );
    }

    #[test]
    fn windows_normalization_is_an_alias_not_the_workspace_id() {
        let identity =
            workspace_root_identity_from_text(r"C:\Source\CodeStory\", r"\\?\C:\Source\CodeStory\");
        let existing_canonical_id = fnv1a_hex(r"\\?\C:\Source\CodeStory\".as_bytes());
        let normalized_alias = fnv1a_hex("c:/source/codestory".as_bytes());

        assert_eq!(identity.workspace_id, existing_canonical_id);
        assert_eq!(
            identity.normalized_root_project_id_alias.as_deref(),
            Some(normalized_alias.as_str())
        );
        assert_eq!(
            normalize_root_identity_text(r"\\?\C:\Source\CodeStory\"),
            "c:/source/codestory"
        );
    }

    #[test]
    fn artifact_scope_fails_closed_when_worktree_becomes_dirty() {
        let Some(project) = git_project() else {
            return;
        };

        let clean = project_identity_v2(project.path());
        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        let dirty = project_identity_v2(project.path());

        assert_eq!(clean.artifact_scope_id, clean.project_id);
        assert_eq!(dirty.artifact_scope_id, dirty.workspace_id);
        assert_ne!(clean.artifact_scope_id, dirty.artifact_scope_id);
        assert_eq!(dirty.portable_reuse_reason, "git_worktree_dirty");
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
