//! Runtime side of the workspace path identity seam.
//!
//! `codestory-agent` owns the [`WorkspacePathIdentity`] trait and can never
//! link `codestory-workspace`; this module is where the runtime closes the
//! seam over the real filesystem probe. Every packet-sufficiency construction
//! site threads [`RuntimeWorkspacePathIdentity`] explicitly — the seam has no
//! `Default` and no `Option` fallback, so a construction site that forgets it
//! is a compile error rather than a packet that silently compares paths with
//! the wrong oracle.

use codestory_agent::workspace_path_identity::WorkspacePathIdentity;
use std::path::Path;

/// The runtime's [`WorkspacePathIdentity`]: two paths name the same workspace
/// file exactly when [`codestory_workspace::same_workspace_path`] says so.
pub(crate) struct RuntimeWorkspacePathIdentity;

impl WorkspacePathIdentity for RuntimeWorkspacePathIdentity {
    fn same_workspace_path(&self, left: &Path, right: &Path) -> bool {
        codestory_workspace::same_workspace_path(left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// The seam is only honest if the injected adapter and the direct call are
    /// the same oracle. Run both over a tempdir matrix of path spellings —
    /// identical, dot-segment relative forms, trailing separators, case
    /// differences, a symlink, genuinely different files, and a missing file —
    /// and require equal verdicts on every row, plus the absolute verdict on
    /// the rows whose answer is platform-independent.
    #[test]
    fn runtime_adapter_matches_direct_same_workspace_path_over_spelling_matrix() {
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        fs::create_dir(root.join("sub")).expect("create subdirectory");
        fs::write(root.join("Alpha.rs"), "alpha").expect("write Alpha.rs");
        fs::write(root.join("beta.rs"), "beta").expect("write beta.rs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("Alpha.rs"), root.join("link.rs"))
            .expect("create symlink to Alpha.rs");

        let trailing_separator_spelling = {
            let mut spelling = root.join("Alpha.rs").into_os_string();
            spelling.push(std::path::MAIN_SEPARATOR.to_string());
            PathBuf::from(spelling)
        };

        // (label, left spelling, right spelling, verdict when platform-independent)
        let mut matrix: Vec<(&str, PathBuf, PathBuf, Option<bool>)> = vec![
            (
                "identical absolute spellings",
                root.join("Alpha.rs"),
                root.join("Alpha.rs"),
                Some(true),
            ),
            (
                "dot-segment relative form of the same file",
                root.join("sub").join("..").join("Alpha.rs"),
                root.join("Alpha.rs"),
                Some(true),
            ),
            (
                "trailing separator on one spelling",
                trailing_separator_spelling,
                root.join("Alpha.rs"),
                None,
            ),
            (
                "case-differing spelling of the same name",
                root.join("alpha.rs"),
                root.join("Alpha.rs"),
                None,
            ),
            (
                "genuinely different files",
                root.join("Alpha.rs"),
                root.join("beta.rs"),
                Some(false),
            ),
            (
                "missing file compared through normalized spelling",
                root.join("missing.rs"),
                root.join("missing.rs"),
                Some(true),
            ),
            (
                "missing file never matches an existing one",
                root.join("missing.rs"),
                root.join("Alpha.rs"),
                Some(false),
            ),
        ];
        #[cfg(unix)]
        matrix.push((
            "symlink spelling of the same file",
            root.join("link.rs"),
            root.join("Alpha.rs"),
            Some(true),
        ));

        for (label, left, right, expected) in matrix {
            let direct = codestory_workspace::same_workspace_path(&left, &right);
            let injected = RuntimeWorkspacePathIdentity.same_workspace_path(&left, &right);
            assert_eq!(
                injected, direct,
                "adapter and direct same_workspace_path must agree for {label}: \
                 {left:?} vs {right:?}"
            );
            if let Some(expected) = expected {
                assert_eq!(direct, expected, "direct verdict for {label}");
            }
        }
    }
}
