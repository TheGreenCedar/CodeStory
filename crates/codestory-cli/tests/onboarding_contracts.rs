use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

#[test]
fn readme_keeps_dual_track_onboarding() {
    let root = repo_root();
    let readme = fs::read_to_string(root.join("README.md")).expect("README should exist");
    assert!(readme.contains("Use CodeStory"));
    assert!(readme.contains("Hack on CodeStory"));
    assert!(readme.contains("docs/architecture/overview.md"));
    assert!(readme.contains("docs/architecture/runtime-execution-path.md"));
    assert!(readme.contains("docs/contributors/debugging.md"));
    assert!(readme.contains("docs/contributors/testing-matrix.md"));

    for path in [
        "docs/architecture/overview.md",
        "docs/architecture/runtime-execution-path.md",
        "docs/architecture/subsystems/contracts.md",
        "docs/architecture/subsystems/workspace.md",
        "docs/architecture/subsystems/indexer.md",
        "docs/architecture/subsystems/runtime.md",
        "docs/architecture/subsystems/store.md",
        "docs/architecture/subsystems/cli.md",
        "docs/contributors/getting-started.md",
        "docs/contributors/debugging.md",
        "docs/contributors/testing-matrix.md",
        "docs/decision-log.md",
    ] {
        assert!(
            root.join(path).exists(),
            "expected onboarding doc to exist: {path}"
        );
    }
}
