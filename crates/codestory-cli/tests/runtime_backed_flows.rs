use fs4::fs_std::FileExt;
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

fn run_cli_with_cache(workspace: &Path, cache_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut command = test_support::cli_command();
    command.args(args);
    command
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir);
    command.output().expect("run codestory-cli")
}

fn publish_schema_29_projection_fixture(workspace: &Path, cache_dir: &Path) -> PathBuf {
    fs::create_dir_all(cache_dir).expect("create explicit cache");
    let index = run_cli_with_cache(
        workspace,
        cache_dir,
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "fixture index failed: stderr={} stdout={}",
        String::from_utf8_lossy(&index.stderr),
        String::from_utf8_lossy(&index.stdout)
    );
    let storage_path = cache_dir.join("codestory.db");
    let connection = rusqlite::Connection::open(&storage_path).expect("open indexed fixture");
    let structural_counts = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM structural_text_unit),
                (SELECT COUNT(*) FROM structural_text_projection),
                (SELECT COUNT(*) FROM structural_text_artifact_cache)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("read structural fixture counts");
    assert_eq!(structural_counts, (0, 0, 0));
    connection
        .execute_batch(
            "DELETE FROM structural_text_unit_publication;
             ALTER TABLE index_publication RENAME TO index_publication_v30;
             CREATE TABLE index_publication (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                generation INTEGER NOT NULL CHECK (generation > 0),
                generation_id TEXT NOT NULL UNIQUE CHECK (length(generation_id) > 0),
                run_id TEXT NOT NULL CHECK (length(run_id) > 0),
                mode TEXT NOT NULL CHECK (mode IN ('full', 'incremental')),
                published_at_epoch_ms INTEGER NOT NULL CHECK (published_at_epoch_ms >= 0)
             );
             INSERT INTO index_publication SELECT * FROM index_publication_v30;
             DROP TABLE index_publication_v30;
             PRAGMA user_version = 29;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .expect("downgrade projection fixture");
    drop(connection);
    storage_path
}

fn remove_workspace_source(workspace: &Path) {
    for entry in fs::read_dir(workspace).expect("list workspace source") {
        let path = entry.expect("workspace entry").path();
        if path.is_dir() {
            fs::remove_dir_all(path).expect("remove source directory");
        } else {
            fs::remove_file(path).expect("remove source file");
        }
    }
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

#[test]
#[ignore = "builds a schema-29 indexed fixture and executes the projection-only CLI writer"]
fn republish_projections_cli_uses_stored_core_after_all_source_is_removed() {
    let workspace = copy_tictactoe_workspace();
    let cache = tempdir().expect("explicit cache");
    let storage_path = publish_schema_29_projection_fixture(workspace.path(), cache.path());
    remove_workspace_source(workspace.path());

    let output = run_cli_with_cache(
        workspace.path(),
        cache.path(),
        &["retrieval", "republish-projections", "--format", "json"],
    );

    assert!(
        output.status.success(),
        "projection republish failed: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("parse republish output");
    assert_eq!(json["publication"]["mode"], "semantic_projection");
    assert!(
        json["symbol_document_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    let connection = rusqlite::Connection::open(&storage_path).expect("open republished core");
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .expect("read migrated schema"),
        30
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT unit_count, projection_count
                 FROM structural_text_unit_publication WHERE id = 1 AND complete = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("read explicit empty structural publication"),
        (0, 0)
    );
}

#[test]
#[ignore = "builds a schema-29 indexed fixture and verifies CLI writer-lock ordering"]
fn republish_projections_cli_acquires_writer_lock_before_schema_migration() {
    let workspace = copy_tictactoe_workspace();
    let cache = tempdir().expect("explicit cache");
    let storage_path = publish_schema_29_projection_fixture(workspace.path(), cache.path());
    remove_workspace_source(workspace.path());
    let bytes_before = fs::read(&storage_path).expect("read schema-29 bytes");
    let lock_path = storage_path.with_extension("index-writer.lock");
    let writer_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open writer lock");
    assert!(
        writer_lock
            .try_lock_exclusive()
            .expect("acquire writer lock"),
        "test must own the writer lock"
    );

    let output = run_cli_with_cache(
        workspace.path(),
        cache.path(),
        &["retrieval", "republish-projections", "--format", "json"],
    );

    assert!(
        !output.status.success(),
        "locked writer unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stderr.contains("cache_busy")
            || stderr.contains("writer lock")
            || stdout.contains("cache_busy")
            || stdout.contains("writer lock"),
        "unexpected CLI error: stderr={stderr} stdout={stdout}"
    );
    assert_eq!(
        fs::read(&storage_path).expect("read unchanged schema-29 bytes"),
        bytes_before,
        "CLI touched the database before acquiring the writer lock"
    );
    let connection = rusqlite::Connection::open(&storage_path).expect("inspect locked fixture");
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .expect("read retained schema"),
        29
    );
}

mod test_support;
