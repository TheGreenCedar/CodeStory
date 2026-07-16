use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
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

fn broad_metadata_workspace() -> tempfile::TempDir {
    let temp = tempdir().expect("create broad metadata fixture");
    fs::write(temp.path().join("metadata.rs"), "// METADATA_ANCHOR\n")
        .expect("write broad metadata fixture");
    temp
}

fn run_cli(workspace: &Path, args: &[&str]) -> std::process::Output {
    let mut command = test_support::cli_command();
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

fn publish_zero_dense_agent_fixture(workspace: &Path) {
    let project = fs::canonicalize(workspace).expect("canonical project fixture");
    let process_cache = fs::canonicalize(test_support::test_state_root().join("stdio-cache"))
        .expect("canonical CLI cache root");
    let storage_path = process_cache
        .join(codestory_workspace::workspace_id_v3_for_root(&project))
        .join("codestory.db");
    let runtime = codestory_retrieval::with_test_cache_root(&process_cache, || {
        codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
            Some(&project),
            codestory_retrieval::SidecarProfile::Agent,
        )
    });
    codestory_retrieval::test_support::publish_zero_dense_pinned_query_fixture(
        &project,
        &storage_path,
        &runtime,
    )
    .expect("publish strict zero-dense agent fixture");
}

fn assert_publication_metadata(json: &Value, surface: &str, retrieval_expected: bool) {
    let metadata = json
        .pointer("/_meta/codestory_publication")
        .unwrap_or_else(|| panic!("{surface} JSON omitted publication metadata: {json}"));
    assert_eq!(metadata["served_from"], "complete_publication", "{surface}");
    assert_eq!(
        metadata["publication"], metadata["core_publication"],
        "{surface}"
    );
    assert!(
        metadata["core_publication"]["generation_id"].is_string(),
        "{surface}"
    );
    assert!(
        metadata["operation"]["operation_id"].is_string(),
        "{surface}"
    );
    assert!(
        metadata["operation"]["attempt"].as_u64().is_some(),
        "{surface}"
    );
    if retrieval_expected {
        assert!(metadata["retrieval_publication"].is_object(), "{surface}");
        assert_eq!(
            metadata["retrieval_publication"]["core_generation_id"],
            metadata["core_publication"]["generation_id"],
            "{surface}"
        );
        assert_eq!(
            metadata["retrieval_publication"]["core_run_id"],
            metadata["core_publication"]["run_id"],
            "{surface}"
        );
    }
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
    assert_publication_metadata(&json, "search", true);
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
    assert_publication_metadata(&json, "symbol", false);
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
    assert_publication_metadata(&json, "query", false);
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
    assert_publication_metadata(&trail_json, "trail", false);
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

#[test]
#[ignore = "builds indexed runtime fixtures; run explicitly when touching CLI publication metadata"]
fn graph_cli_json_surfaces_share_publication_metadata() {
    let workspace = copy_tictactoe_workspace();
    index_workspace(workspace.path());
    let query = run_cli(workspace.path(), &["ground", "--format", "json"]);
    assert!(
        query.status.success(),
        "query command failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    let query_json: Value = serde_json::from_slice(&query.stdout).expect("parse grounding JSON");
    let node_id = query_json["root_symbols"][0]["id"]
        .as_str()
        .expect("grounding node id");
    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "symbol",
            vec!["symbol", "--id", node_id, "--format", "json"],
        ),
        ("trail", vec!["trail", "--id", node_id, "--format", "json"]),
        (
            "snippet",
            vec!["snippet", "--id", node_id, "--format", "json"],
        ),
        (
            "explore",
            vec!["explore", "--id", node_id, "--no-tui", "--format", "json"],
        ),
        (
            "impact",
            vec!["impact", "--id", node_id, "--format", "json"],
        ),
        (
            "test-map",
            vec!["test-map", "--id", node_id, "--format", "json"],
        ),
        ("files", vec!["files", "--format", "json"]),
        (
            "affected",
            vec!["affected", "rust_tictactoe.rs", "--format", "json"],
        ),
    ];

    for (surface, args) in cases {
        let output = run_cli(workspace.path(), &args);
        assert!(
            output.status.success(),
            "{surface} command failed: stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let json: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("parse {surface} JSON: {error}"));
        assert_publication_metadata(&json, surface, false);
    }
}

#[test]
#[ignore = "builds full retrieval fixtures; run explicitly when touching broad CLI publication metadata"]
fn broad_cli_json_surfaces_share_core_and_retrieval_publications() {
    let workspace = broad_metadata_workspace();
    index_workspace(workspace.path());
    publish_zero_dense_agent_fixture(workspace.path());
    let cases: &[(&str, &[&str])] = &[
        (
            "search",
            &[
                "search",
                "--query",
                "METADATA_ANCHOR",
                "--repo-text",
                "on",
                "--format",
                "json",
            ],
        ),
        (
            "packet",
            &[
                "packet",
                "--question",
                "Where is METADATA_ANCHOR defined?",
                "--format",
                "json",
            ],
        ),
    ];

    for (surface, args) in cases {
        let output = run_cli(workspace.path(), args);
        assert!(
            output.status.success(),
            "{surface} command failed: stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let json: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("parse {surface} JSON: {error}"));
        assert_publication_metadata(&json, surface, true);
    }

    let output_dir = tempdir().expect("drill output dir");
    let output_dir_arg = output_dir.path().to_string_lossy().to_string();
    let drill = run_cli(
        workspace.path(),
        &[
            "drill",
            "--anchors",
            "METADATA_ANCHOR",
            "--question",
            "Where is METADATA_ANCHOR defined?",
            "--output-dir",
            &output_dir_arg,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        drill.status.success(),
        "drill command failed: {}",
        String::from_utf8_lossy(&drill.stderr)
    );
    let json: Value = serde_json::from_slice(&drill.stdout).expect("parse drill JSON");
    assert_publication_metadata(&json, "drill", true);
    let summary: Value = serde_json::from_slice(
        &fs::read(output_dir.path().join("drill-summary.json")).expect("read drill summary"),
    )
    .expect("parse drill summary JSON");
    assert_publication_metadata(&summary, "drill summary", true);
}
mod test_support;
