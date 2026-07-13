use serde::Serialize;
use std::path::Path;

pub use codestory_workspace::REPOSITORY_IDENTITY_V2_SCHEMA_VERSION as REPOSITORY_IDENTITY_SCHEMA_VERSION;

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryIdentityReport {
    pub project: String,
    pub project_identity_schema_version: u32,
    pub project_id: String,
    pub workspace_id: String,
    pub artifact_scope_id: String,
    pub root_derived_project_id: String,
    pub canonical_repository_id: Option<String>,
    pub repository_identity_schema_version: u32,
    pub normalized_repository_identity: Option<String>,
    pub legacy_alias_disposition: String,
    pub legacy_project_id: Option<String>,
    pub git_remote: Option<String>,
    pub git_tree: Option<String>,
    pub cache_schema_version: u32,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
    pub freshness_inputs: Vec<String>,
}

pub fn inspect_repository_identity(project_root: &Path) -> RepositoryIdentityReport {
    let root_derived_project_id = codestory_retrieval::project_id_for_root(project_root);
    let repository = codestory_workspace::inspect_repository_identity_v2(project_root);
    let identity = codestory_workspace::project_identity_v3(project_root);
    let legacy_alias_disposition = if identity.legacy_canonical_repository_id.is_some() {
        "safe"
    } else {
        "unavailable_without_provenance"
    };
    RepositoryIdentityReport {
        project: project_root.to_string_lossy().to_string(),
        project_identity_schema_version: identity.project_identity_schema_version,
        project_id: identity.project_id,
        workspace_id: identity.workspace_id,
        artifact_scope_id: identity.artifact_scope_id,
        root_derived_project_id,
        canonical_repository_id: repository.canonical_repository_id,
        repository_identity_schema_version: repository.repository_identity_schema_version,
        normalized_repository_identity: repository.normalized_repository_identity,
        legacy_alias_disposition: legacy_alias_disposition.to_string(),
        legacy_project_id: identity.legacy_canonical_repository_id,
        git_remote: repository.git_remote,
        git_tree: repository.git_tree,
        cache_schema_version: codestory_store::CURRENT_SCHEMA_VERSION,
        portable_reuse_eligible: repository.portable_reuse_eligible,
        portable_reuse_reason: repository.portable_reuse_reason,
        freshness_inputs: vec![
            "git_tree".into(),
            "cache_schema_version".into(),
            "sidecar_input_hash".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

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
        assert_eq!(first.project_id, second.project_id);
        assert_eq!(first.workspace_id, second.workspace_id);
        assert_ne!(first.git_tree, second.git_tree);
        assert_eq!(
            first.project_identity_schema_version,
            codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION
        );
        assert_eq!(
            first.legacy_alias_disposition,
            "unavailable_without_provenance"
        );
        assert!(first.legacy_project_id.is_none());
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
