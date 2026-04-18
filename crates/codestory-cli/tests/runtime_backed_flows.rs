use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

fn copy_tictactoe_workspace() -> tempfile::TempDir {
    let temp = tempdir().expect("create temp dir");
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crates dir")
        .join("codestory-indexer")
        .join("tests")
        .join("fixtures")
        .join("tictactoe");

    for entry in fs::read_dir(&fixtures).expect("read fixtures") {
        let entry = entry.expect("fixture entry");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let target = temp.path().join(entry.file_name());
        fs::copy(&path, &target).expect("copy fixture");
    }

    temp
}

fn run_cli(workspace: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command.args(args);
    command.arg("--project").arg(workspace);
    command.output().expect("run codestory-cli")
}

#[test]
#[ignore = "builds indexed runtime fixtures; run explicitly when touching CLI/runtime read-command flows"]
fn read_commands_support_explicit_auto_refresh_after_indexing() {
    let workspace = copy_tictactoe_workspace();

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let search = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            "TicTacToe",
            "--refresh",
            "auto",
            "--format",
            "json",
        ],
    );
    assert!(
        search.status.success(),
        "search command failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
    assert!(
        json["indexed_symbol_hits"]
            .as_array()
            .is_some_and(|hits| !hits.is_empty()),
        "auto-refresh search should still return indexed symbol hits"
    );
}

#[test]
#[ignore = "builds indexed runtime fixtures; run explicitly when touching CLI/runtime read-command flows"]
fn symbol_query_file_filter_resolves_expected_fixture() {
    let workspace = copy_tictactoe_workspace();

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let symbol = run_cli(
        workspace.path(),
        &[
            "symbol",
            "--query",
            "TicTacToe",
            "--file",
            "rust_tictactoe.rs",
            "--format",
            "json",
        ],
    );
    assert!(
        symbol.status.success(),
        "symbol command failed: {}",
        String::from_utf8_lossy(&symbol.stderr)
    );

    let json: Value = serde_json::from_slice(&symbol.stdout).expect("parse symbol json");
    let resolved_path = json["resolution"]["resolved"]["file_path"]
        .as_str()
        .expect("resolved file path");
    assert!(
        resolved_path.contains("rust_tictactoe.rs"),
        "file filter should resolve to the requested fixture, got {resolved_path}"
    );
}
