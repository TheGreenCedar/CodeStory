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

fn index_workspace(workspace: &Path) {
    let index = run_cli(
        workspace,
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );
}

#[test]
#[ignore = "builds indexed runtime fixtures; run explicitly when touching CLI/runtime read-command flows"]
fn read_commands_support_explicit_auto_refresh_after_indexing() {
    let workspace = copy_tictactoe_workspace();
    index_workspace(workspace.path());

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
    index_workspace(workspace.path());

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

#[test]
#[ignore = "builds indexed runtime fixtures; run explicitly when touching CLI/runtime read-command flows"]
fn query_command_runs_search_filter_limit_pipeline() {
    let workspace = copy_tictactoe_workspace();
    index_workspace(workspace.path());

    let query = run_cli(
        workspace.path(),
        &[
            "query",
            "search(query: 'check_winner') | filter(kind: function) | limit(2)",
            "--format",
            "json",
        ],
    );
    assert!(
        query.status.success(),
        "query command failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );

    let json: Value = serde_json::from_slice(&query.stdout).expect("parse query json");
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2, "limit should cap filtered items");
    assert!(
        items.iter().all(|item| item["kind"] == "FUNCTION"),
        "filter(kind: function) should keep only function hits: {items:?}"
    );
    assert!(
        items.iter().all(|item| item["source"] == "search"),
        "query search operation should mark item provenance"
    );
}

#[test]
#[ignore = "builds indexed runtime fixtures; run explicitly when touching CLI/runtime read-command flows"]
fn query_symbol_prefers_same_exact_target_as_symbol_command() {
    let workspace = copy_tictactoe_workspace();
    index_workspace(workspace.path());

    let symbol = run_cli(
        workspace.path(),
        &["symbol", "--query", "_select_player", "--format", "json"],
    );
    assert!(
        symbol.status.success(),
        "symbol command failed: {}",
        String::from_utf8_lossy(&symbol.stderr)
    );
    let symbol_json: Value = serde_json::from_slice(&symbol.stdout).expect("parse symbol json");
    let expected_id = symbol_json["resolution"]["resolved"]["node_id"]
        .as_str()
        .expect("resolved node id");

    let query = run_cli(
        workspace.path(),
        &[
            "query",
            "symbol('_select_player') | limit(1)",
            "--format",
            "json",
        ],
    );
    assert!(
        query.status.success(),
        "query command failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let query_json: Value = serde_json::from_slice(&query.stdout).expect("parse query json");
    let actual_id = query_json["items"][0]["node_id"]
        .as_str()
        .expect("query item node id");

    assert_eq!(
        actual_id, expected_id,
        "query symbol() should resolve the same exact target as symbol --query"
    );
}

#[test]
#[ignore = "builds indexed runtime fixtures; run explicitly when touching CLI/runtime read-command flows"]
fn trail_command_default_width_matches_query_dsl_trail() {
    let workspace = copy_tictactoe_workspace();
    index_workspace(workspace.path());

    let trail = run_cli(
        workspace.path(),
        &["trail", "--query", "_select_player", "--format", "json"],
    );
    assert!(
        trail.status.success(),
        "trail command failed: {}",
        String::from_utf8_lossy(&trail.stderr)
    );
    let trail_json: Value = serde_json::from_slice(&trail.stdout).expect("parse trail json");
    let trail_nodes = trail_json["trail"]["trail"]["nodes"]
        .as_array()
        .expect("trail nodes");

    let query = run_cli(
        workspace.path(),
        &[
            "query",
            "trail(symbol: '_select_player') | limit(120)",
            "--format",
            "json",
        ],
    );
    assert!(
        query.status.success(),
        "query command failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let query_json: Value = serde_json::from_slice(&query.stdout).expect("parse query json");
    let query_items = query_json["items"].as_array().expect("query items");

    assert_eq!(
        trail_nodes.len(),
        query_items.len(),
        "trail --query and query trail(...) should expose the same default graph width"
    );
}
