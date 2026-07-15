mod test_support;

use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn cli_state_stays_out_of_controlled_user_cache_fallback() {
    let project = tempfile::tempdir().expect("project");
    std::fs::write(project.path().join("lib.rs"), "pub fn isolated() {}\n")
        .expect("project source");
    let decoy = test_support::os_user_root();
    // Windows initializes these platform base directories while resolving the
    // overridden user profile. Include that inert setup in the baseline so the
    // invariant detects CodeStory state, not OS directory normalization.
    std::fs::create_dir_all(decoy.join("home").join("AppData").join("Roaming"))
        .expect("decoy roaming app data");
    std::fs::create_dir_all(decoy.join("home").join("AppData").join("Local"))
        .expect("decoy local profile app data");
    std::fs::create_dir_all(decoy.join("local-app-data")).expect("decoy local app data");
    std::fs::create_dir_all(decoy.join("app-data")).expect("decoy app data");
    std::fs::create_dir_all(decoy.join("xdg-cache")).expect("decoy XDG cache");
    std::fs::create_dir_all(decoy.join("xdg-data")).expect("decoy XDG data");
    std::fs::write(decoy.join("sentinel.txt"), b"unchanged").expect("decoy sentinel");

    // Some Windows hosts create PowerShell startup-profile metadata while a
    // retrieval command gathers machine information. Warm that platform-owned
    // cache, then prove the warmup left no CodeStory-shaped user state before
    // taking the invariant baseline.
    let warm = cli_command_with_decoy(&decoy)
        .args(["retrieval", "status", "--project"])
        .arg(project.path())
        .args([
            "--profile",
            "agent",
            "--run-id",
            "test-state-isolation-warmup",
            "--format",
            "json",
        ])
        .output()
        .expect("warm platform profile state");
    assert!(warm.status.success(), "warmup status command must succeed");
    let warmed = inventory(&decoy);
    assert!(
        warmed.iter().all(|(path, _)| is_expected_decoy_entry(path)),
        "warmup leaked state into the decoy user root: {warmed:?}"
    );
    let before = inventory(&decoy);

    let output = cli_command_with_decoy(&decoy)
        .args(["retrieval", "status", "--project"])
        .arg(project.path())
        .args([
            "--profile",
            "agent",
            "--run-id",
            "test-state-isolation",
            "--format",
            "json",
        ])
        .output()
        .expect("run isolated retrieval status");

    assert!(
        output.status.success(),
        "isolated status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(inventory(&decoy), before, "decoy user cache changed");
}

fn cli_command_with_decoy(decoy: &Path) -> Command {
    let mut command = test_support::cli_command();
    command
        .env("HOME", decoy.join("home"))
        .env("USERPROFILE", decoy.join("home"))
        .env("XDG_CACHE_HOME", decoy.join("xdg-cache"))
        .env("XDG_DATA_HOME", decoy.join("xdg-data"))
        .env("LOCALAPPDATA", decoy.join("local-app-data"))
        .env("APPDATA", decoy.join("app-data"));
    command
}

fn is_expected_decoy_entry(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    matches!(
        normalized.as_str(),
        "app-data"
            | "home"
            | "home/AppData"
            | "home/AppData/Local"
            | "home/AppData/Roaming"
            | "local-app-data"
            | "sentinel.txt"
            | "xdg-cache"
            | "xdg-data"
    ) || normalized.starts_with("home/AppData/Local/Microsoft")
}

#[test]
fn integration_cli_processes_use_the_isolated_command_helper() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let cli_binary_env = ["CARGO_BIN_EXE_", "codestory-cli"].concat();
    for entry in std::fs::read_dir(&tests).expect("integration test directory") {
        let entry = entry.expect("integration test entry");
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("integration test source");
        assert!(
            !source.contains(&cli_binary_env),
            "{} accesses the CLI binary outside the isolated test helper",
            path.display()
        );
    }
}

fn inventory(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut entries = Vec::new();
    collect_inventory(root, root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn collect_inventory(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
    let mut children: Vec<_> = std::fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read inventory {}: {error}", path.display()))
        .map(|entry| entry.expect("inventory entry"))
        .collect();
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        let relative = path.strip_prefix(root).expect("relative inventory path");
        if child.file_type().expect("inventory file type").is_dir() {
            entries.push((relative.to_path_buf(), None));
            collect_inventory(root, &path, entries);
        } else {
            entries.push((
                relative.to_path_buf(),
                Some(std::fs::read(&path).expect("inventory file")),
            ));
        }
    }
}
