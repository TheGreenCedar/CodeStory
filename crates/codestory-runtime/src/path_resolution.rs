use super::{ApiError, AppController};
use std::io;
use std::path::{Path, PathBuf};

pub(super) fn resolve_project_file_path(
    controller: &AppController,
    path: &str,
    allow_missing_leaf: bool,
) -> Result<PathBuf, ApiError> {
    let root = controller.require_project_root()?;
    resolve_project_file_path_from_root(&root, path, allow_missing_leaf)
}

/// Resolve one user-supplied file path against a project root without opening,
/// activating, or otherwise mutating project state.
#[doc(hidden)]
pub fn resolve_project_file_path_from_root(
    root: &Path,
    path: &str,
    allow_missing_leaf: bool,
) -> Result<PathBuf, ApiError> {
    let root = root
        .canonicalize()
        .map_err(|e| ApiError::internal(format!("Failed to resolve project root: {e}")))?;

    let raw = PathBuf::from(path);
    if raw
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ApiError::invalid_argument(
            "Refusing to access file outside project root: parent path components are not allowed.",
        ));
    }
    let candidate = if raw.is_absolute() {
        raw
    } else {
        root.join(raw)
    };

    let mut existing_ancestor = candidate.as_path();
    let mut missing_suffix = Vec::new();
    let resolved_ancestor = loop {
        match existing_ancestor.canonicalize() {
            Ok(canonical) => break canonical,
            Err(canonical_error) => match std::fs::symlink_metadata(existing_ancestor) {
                Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => {
                    if !allow_missing_leaf {
                        return Err(ApiError::not_found(format!(
                            "File not found: {}",
                            candidate.display()
                        )));
                    }
                    let Some(file_name) = existing_ancestor.file_name() else {
                        return Err(ApiError::invalid_argument(format!(
                            "Invalid file path: {}",
                            candidate.display()
                        )));
                    };
                    missing_suffix.push(file_name.to_os_string());
                    let Some(parent) = existing_ancestor.parent() else {
                        return Err(ApiError::invalid_argument(format!(
                            "Invalid file path: {}",
                            candidate.display()
                        )));
                    };
                    existing_ancestor = parent;
                }
                Ok(_) => {
                    return Err(ApiError::invalid_argument(format!(
                        "Refusing unresolved existing file path ancestor {}: {canonical_error}",
                        existing_ancestor.display()
                    )));
                }
                Err(metadata_error) => {
                    return Err(ApiError::invalid_argument(format!(
                        "Refusing file path with an unresolvable ancestor {}: canonicalize failed with {canonical_error}; metadata failed with {metadata_error}",
                        existing_ancestor.display()
                    )));
                }
            },
        }
    };

    // Containment is established against the nearest existing ancestor before
    // any missing suffix is accepted. This catches an existing symlink ancestor
    // that redirects a nested deleted/renamed path outside the project.
    if !resolved_ancestor.starts_with(&root) {
        return Err(ApiError::invalid_argument(
            "Refusing to access file outside project root.",
        ));
    }

    let mut resolved = resolved_ancestor;
    for component in missing_suffix.iter().rev() {
        resolved.push(component);
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn nested_missing_suffix_is_resolved_from_nearest_existing_ancestor() {
        let project = tempdir().expect("project");
        std::fs::create_dir(project.path().join("src")).expect("create source directory");

        let resolved =
            resolve_project_file_path_from_root(project.path(), "src/deleted/parent/old.rs", true)
                .expect("resolve nested deleted path");

        assert_eq!(
            resolved,
            project
                .path()
                .canonicalize()
                .expect("canonical project")
                .join("src/deleted/parent/old.rs")
        );
    }

    #[test]
    fn parent_and_absolute_outside_paths_are_rejected() {
        let project = tempdir().expect("project");
        let outside = tempdir().expect("outside");

        for path in [
            "../outside.rs".to_string(),
            outside
                .path()
                .join("outside.rs")
                .to_string_lossy()
                .to_string(),
        ] {
            let error = resolve_project_file_path_from_root(project.path(), &path, true)
                .expect_err("outside path must fail");
            assert_eq!(error.code, "invalid_argument");
            assert!(error.message.contains("outside project root"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn existing_symlink_ancestor_escape_is_rejected_for_nested_missing_path() {
        let project = tempdir().expect("project");
        let outside = tempdir().expect("outside");
        std::os::unix::fs::symlink(outside.path(), project.path().join("escape"))
            .expect("create escaping symlink");

        let error = resolve_project_file_path_from_root(
            project.path(),
            "escape/deleted/parent/old.rs",
            true,
        )
        .expect_err("symlink escape must fail");

        assert_eq!(error.code, "invalid_argument");
        assert!(error.message.contains("outside project root"));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_ancestor_is_not_peeled_as_a_missing_suffix() {
        let project = tempdir().expect("project");
        std::os::unix::fs::symlink("absent-target", project.path().join("dangling"))
            .expect("create dangling symlink");

        let error = resolve_project_file_path_from_root(
            project.path(),
            "dangling/deleted/parent/old.rs",
            true,
        )
        .expect_err("dangling ancestor must fail closed");

        assert_eq!(error.code, "invalid_argument");
        assert!(
            error
                .message
                .contains("unresolved existing file path ancestor")
        );
        assert!(error.message.contains("dangling"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loop_ancestor_is_not_peeled_as_a_missing_suffix() {
        let project = tempdir().expect("project");
        std::os::unix::fs::symlink("loop-b", project.path().join("loop-a"))
            .expect("create first loop link");
        std::os::unix::fs::symlink("loop-a", project.path().join("loop-b"))
            .expect("create second loop link");

        let error = resolve_project_file_path_from_root(
            project.path(),
            "loop-a/deleted/parent/old.rs",
            true,
        )
        .expect_err("symlink loop ancestor must fail closed");

        assert_eq!(error.code, "invalid_argument");
        assert!(error.message.contains("unresolvable ancestor"));
        assert!(error.message.contains("loop-a"));
    }
}
