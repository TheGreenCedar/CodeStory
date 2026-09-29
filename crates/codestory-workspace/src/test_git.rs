//! Isolated git invocations for test fixtures.
//!
//! Every fixture git runs through [`git_command`]: child-scoped environment
//! only, never `set_var`, never ambient configuration. The child gets
//! `-c core.hooksPath=/dev/null`, null global/system config,
//! `GIT_CONFIG_NOSYSTEM=1`, an empty fixture-owned HOME/XDG, and every
//! inherited `GIT_*` variable removed — so a developer's global config,
//! credential helpers, or `GIT_DIR` state can never leak into a fixture and
//! a fixture can never execute a hook installed outside the fixture tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::{TempDir, tempdir};

/// The fixture-owned directory the child git treats as HOME and XDG root.
pub fn isolated_home(root: &Path) -> PathBuf {
    root.join(".codestory-test-home")
}

/// Build a git invocation that cannot see ambient git state.
pub fn git_command(root: &Path, args: &[&str]) -> Command {
    let home = isolated_home(root);
    fs::create_dir_all(&home).expect("isolated git home");
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg("user.name=CodeStory Test")
        .arg("-c")
        .arg("user.email=test@example.invalid")
        .arg("-C")
        .arg(root)
        .args(args);
    for (key, _) in std::env::vars() {
        if key.starts_with("GIT_") {
            command.env_remove(&key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join("xdg-config"))
        .env("XDG_CACHE_HOME", home.join("xdg-cache"));
    command
}

/// Run git in `root` and require success.
pub fn git(root: &Path, args: &[&str]) {
    let output = git_command(root, args)
        .output()
        .expect("run isolated fixture git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Run git in `root` and return trimmed stdout.
pub fn git_stdout(root: &Path, args: &[&str]) -> String {
    let output = git_command(root, args)
        .output()
        .expect("run isolated fixture git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8 git output")
        .trim()
        .to_string()
}

/// Whether `git status --porcelain` reports work.
pub fn git_is_dirty(root: &Path) -> bool {
    !git_stdout(root, &["--no-optional-locks", "status", "--porcelain"]).is_empty()
}

/// A committed fixture repository. Git is a hard development dependency:
/// its absence fails the fixture loudly instead of vacuously passing.
pub fn git_project(remote_url: &str) -> TempDir {
    assert!(
        Command::new("git").arg("--version").output().is_ok(),
        "git is a hard development dependency and must be installed"
    );
    let project = tempdir().expect("project");
    git(project.path(), &["init"]);
    git(project.path(), &["remote", "add", "origin", remote_url]);
    fs::write(project.path().join("lib.rs"), "pub fn run() {}\n").expect("write source");
    git(project.path(), &["add", "."]);
    git(project.path(), &["commit", "-m", "init"]);
    project
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hostile hooksPath reachable through every config channel the child
    /// sees must never execute: the builder pins `core.hooksPath=/dev/null`
    /// and nullifies global/system config, so the sentinel stays unwritten.
    /// The hostile path is seeded in the local config as well so that
    /// weakening either isolation layer is detectable.
    #[test]
    fn fixture_git_never_executes_global_hooks() {
        let project = tempdir().expect("project");
        let home = isolated_home(project.path());
        let hooks = home.join("hostile-hooks");
        fs::create_dir_all(&hooks).expect("create hostile hook dir");
        let marker = project.path().join("global-hook-ran");
        let hook = hooks.join("post-commit");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch \"{}\"\n", marker.display()),
        )
        .expect("write hostile hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("chmod hook");
        }
        let hostile_config = format!("[core]\n\thooksPath = {}\n", hooks.display());
        fs::write(home.join(".gitconfig"), &hostile_config).expect("write hostile global config");
        fs::create_dir_all(home.join("xdg-config/git")).expect("create xdg config dir");
        fs::write(home.join("xdg-config/git/config"), &hostile_config)
            .expect("write hostile xdg config");

        git(project.path(), &["init"]);
        git(
            project.path(),
            &[
                "config",
                "core.hooksPath",
                hooks.to_str().expect("UTF-8 hooks dir"),
            ],
        );
        git(project.path(), &["commit", "--allow-empty", "-m", "probe"]);

        assert!(
            !marker.exists(),
            "a configured hook executed during an isolated fixture git"
        );
    }

    /// Mechanical proof that no workspace test reaches git except through
    /// this module: `Command::new("git"` may appear only here, and no test
    /// may mutate the process environment.
    #[test]
    fn workspace_tests_spawn_git_only_through_the_isolated_builder() {
        let mut violations = Vec::new();
        for (file, source) in [
            ("atomic_file.rs", include_str!("atomic_file.rs")),
            (
                "filesystem_observer.rs",
                include_str!("filesystem_observer.rs"),
            ),
            ("lib.rs", include_str!("lib.rs")),
            ("locking.rs", include_str!("locking.rs")),
            ("owned_deletion.rs", include_str!("owned_deletion.rs")),
            ("paths.rs", include_str!("paths.rs")),
            ("repo_metadata.rs", include_str!("repo_metadata.rs")),
            ("repository_hooks.rs", include_str!("repository_hooks.rs")),
            (
                "repository_identity.rs",
                include_str!("repository_identity.rs"),
            ),
            ("source_freshness.rs", include_str!("source_freshness.rs")),
        ] {
            if source.contains("Command::new(\"git\")") {
                violations.push(format!("{file}: bare git spawn outside test_git"));
            }
            if source.contains("env::set_var") || source.contains("env::remove_var") {
                violations.push(format!("{file}: mutates the process environment"));
            }
        }
        assert!(violations.is_empty(), "{violations:?}");
    }
}
