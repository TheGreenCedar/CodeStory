use codestory_workspace::filesystem_observer::ObservedScopeFilter;
use codestory_workspace::{
    RepositoryTrackingDigest, WorkspaceInventoryOutcome, WorkspaceManifest,
    observe_repository_tracking_digest,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=CodeStory Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .expect("run fixture git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn check_transition(linked: bool) {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().canonicalize().unwrap();
    let original = parent.join("original");
    fs::create_dir(&original).unwrap();
    git(&original, &["init", "--quiet"]);
    fs::write(original.join("Main.java"), "class Main {}\n").unwrap();
    git(&original, &["add", "Main.java"]);
    git(&original, &["commit", "--quiet", "-m", "fixture"]);
    let root = if linked {
        let worktree = parent.join("linked");
        git(
            &original,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                worktree.to_str().unwrap(),
            ],
        );
        worktree
    } else {
        original
    };
    let source = root.join("build/X.java");
    fs::create_dir(source.parent().unwrap()).unwrap();
    fs::write(&source, "class X {}\n").unwrap();
    let before = WorkspaceManifest::open(root.clone())
        .unwrap()
        .source_inventory()
        .unwrap();
    assert_eq!(before.outcome, WorkspaceInventoryOutcome::Complete);
    assert!(!before.files.contains(&source));
    let before_tracking = before.repository_tracking_digest.clone().unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let index = PathBuf::from(git(&root, &["rev-parse", "--absolute-git-dir"])).join("index");
    let index_before = fs::read(&index).unwrap();
    git(&root, &["add", "build/X.java"]);
    let after = WorkspaceManifest::open(root.clone())
        .unwrap()
        .source_inventory()
        .unwrap();
    assert_eq!(after.outcome, WorkspaceInventoryOutcome::Complete);
    assert!(after.files.contains(&source));
    let after_tracking = after.repository_tracking_digest.clone().unwrap();
    assert_eq!(source_bytes, fs::read(&source).unwrap());
    assert_ne!(index_before, fs::read(&index).unwrap());
    if linked {
        assert!(!index.starts_with(&root));
    }
    println!(
        "linked={linked}; before_admitted=false; after_admitted=true; source_bytes_unchanged=true; index_outside_root={}",
        !index.starts_with(&root)
    );
    assert!(!ObservedScopeFilter::source_default().admits(&root, &index));
    assert_ne!(before_tracking, after_tracking);
    assert_eq!(
        observe_repository_tracking_digest(&root).unwrap(),
        after_tracking,
        "current tracking observation must match the exact inventory witness (linked={linked})"
    );
}

#[test]
fn root_index_tracking_transition_requires_more_than_source_epoch() {
    check_transition(false);
}
#[test]
fn linked_index_tracking_transition_requires_more_than_source_epoch() {
    check_transition(true);
}

#[test]
fn tracked_recovery_receipt_covers_gitignored_sources_outside_build() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "--quiet"]);
    fs::write(root.join("Main.java"), "class Main {}\n").unwrap();
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    git(&root, &["add", "Main.java", ".gitignore"]);
    git(&root, &["commit", "--quiet", "-m", "fixture"]);
    let source = root.join("ignored/X.java");
    fs::create_dir(source.parent().unwrap()).unwrap();
    fs::write(&source, "class X {}\n").unwrap();

    let before = WorkspaceManifest::open(root.clone())
        .unwrap()
        .source_inventory()
        .unwrap();
    assert!(!before.files.contains(&source));
    git(&root, &["add", "--force", "ignored/X.java"]);
    let after = WorkspaceManifest::open(root.clone())
        .unwrap()
        .source_inventory()
        .unwrap();
    assert!(after.files.contains(&source));
    assert_ne!(
        before.repository_tracking_digest,
        after.repository_tracking_digest
    );
}

#[test]
fn no_repository_empty_repository_and_unknown_are_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    assert_eq!(
        observe_repository_tracking_digest(&root).unwrap(),
        RepositoryTrackingDigest::NoRepository
    );

    git(&root, &["init", "--quiet"]);
    assert!(matches!(
        observe_repository_tracking_digest(&root).unwrap(),
        RepositoryTrackingDigest::Present(_)
    ));

    let unknown = temp.path().join("unknown");
    fs::create_dir(&unknown).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join(".git"), unknown.join(".git")).unwrap();
    #[cfg(unix)]
    assert!(observe_repository_tracking_digest(&unknown).is_err());
}
