use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn write_tiny_rust_workspace(root: &Path) {
    fs::create_dir_all(root.join("src")).expect("create src dir");
    fs::write(
        root.join("src").join("lib.rs"),
        r#"pub struct AppController;

pub fn open_project() -> AppController {
    AppController
}
"#,
    )
    .expect("write lib.rs");
}

fn write_ambiguous_rust_workspace(root: &Path) {
    fs::create_dir_all(root.join("src")).expect("create src dir");
    fs::write(
        root.join("src").join("lib.rs"),
        r#"pub mod alpha;
pub mod beta;
"#,
    )
    .expect("write lib.rs");
    fs::write(
        root.join("src").join("alpha.rs"),
        r#"pub fn configure() -> usize {
    1
}
"#,
    )
    .expect("write alpha.rs");
    fs::write(
        root.join("src").join("beta.rs"),
        r#"pub fn configure() -> usize {
    2
}
"#,
    )
    .expect("write beta.rs");
}

fn run_cli(workspace: &Path, cache_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash");
    command.output().expect("run codestory-cli")
}

fn assert_fails_with(output: std::process::Output, expected: &[&str]) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for needle in expected {
        assert!(
            combined.contains(needle),
            "expected output to contain {needle:?}\ncombined output:\n{combined}"
        );
    }
}

fn assert_success(output: &std::process::Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn index_workspace(workspace: &Path, cache_dir: &Path) {
    let index = run_cli(
        workspace,
        cache_dir,
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert_success(&index, "index command failed");
}

fn parse_stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout should be JSON, got parse error: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn ambiguous_error_alternatives(json: &Value) -> &Vec<Value> {
    json.pointer("/error/alternatives")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ambiguous JSON should expose /error/alternatives: {json:#}"))
}

#[test]
fn read_command_without_cache_reports_recovery_command() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "search",
            "--query",
            "AppController",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );

    assert_fails_with(
        output,
        &[
            "No indexed files are available",
            "codestory-cli index",
            "--refresh full",
            "--refresh incremental",
        ],
    );
}

#[test]
fn missing_output_parent_is_rejected_before_runtime_cache_creation() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let missing_output = cache_dir.path().join("missing").join("search.json");
    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "search",
            "--query",
            "AppController",
            "--refresh",
            "none",
            "--format",
            "json",
            "--output-file",
            missing_output.to_str().expect("utf-8 temp path"),
        ],
    );

    assert_fails_with(output, &["Output parent directory does not exist"]);
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "output preflight should happen before runtime cache creation"
    );
}

#[test]
fn non_trail_dot_format_is_rejected_before_runtime_cache_creation() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "search",
            "--query",
            "AppController",
            "--refresh",
            "none",
            "--format",
            "dot",
        ],
    );

    assert_fails_with(
        output,
        &[
            "--format dot is only supported by `trail`",
            "`search` supports markdown and json",
        ],
    );
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "format validation should happen before runtime cache creation"
    );
}

#[test]
fn trail_story_output_conflicts_are_rejected_before_runtime_cache_creation() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let mermaid = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "trail",
            "--query",
            "AppController",
            "--story",
            "--mermaid",
            "--refresh",
            "none",
        ],
    );
    assert_fails_with(mermaid, &["--story cannot be combined with --mermaid"]);
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "story/mermaid validation should happen before runtime cache creation"
    );

    let dot = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "trail",
            "--query",
            "AppController",
            "--story",
            "--format",
            "dot",
            "--refresh",
            "none",
        ],
    );
    assert_fails_with(dot, &["--story cannot be combined with --format dot"]);
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "story/dot validation should happen before runtime cache creation"
    );
}

#[test]
fn ambiguous_query_lists_ranked_alternatives_and_next_steps() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_ambiguous_rust_workspace(workspace.path());

    index_workspace(workspace.path(), cache_dir.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "symbol",
            "--query",
            "configure",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );

    assert_fails_with(
        output,
        &[
            "Query `configure` is ambiguous",
            "Top equally ranked matches",
            "alpha.rs",
            "beta.rs",
            "resolve the exact `--id` from `search` output",
        ],
    );
}

#[test]
fn ambiguous_symbol_json_includes_numbered_alternatives_with_stable_refs() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_ambiguous_rust_workspace(workspace.path());
    index_workspace(workspace.path(), cache_dir.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "symbol",
            "--query",
            "configure",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );

    assert!(
        !output.status.success(),
        "ambiguous query must not silently resolve a tied target\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_stdout_json(&output);
    assert_eq!(
        json.pointer("/error/code").and_then(Value::as_str),
        Some("ambiguous_target"),
        "ambiguous JSON should carry a machine-readable error code: {json:#}"
    );
    assert_eq!(
        json.pointer("/error/failed_layer").and_then(Value::as_str),
        Some("query_resolution"),
        "ambiguous JSON should identify the failed layer: {json:#}"
    );
    assert!(
        json.pointer("/error/layer_notes")
            .and_then(Value::as_array)
            .is_some_and(|notes| notes.iter().any(|note| note
                .as_str()
                .is_some_and(|note| note.starts_with("query_resolution:")))),
        "ambiguous JSON should preserve layer notes: {json:#}"
    );
    assert!(
        json.pointer("/error/layer_notes")
            .and_then(Value::as_array)
            .is_some_and(|notes| notes
                .iter()
                .all(|note| !note.as_str().unwrap_or_default().contains("search --file"))),
        "ambiguous JSON should not imply `search --file` exists: {json:#}"
    );
    assert!(
        json.pointer("/resolution/resolved").is_none(),
        "ambiguous JSON should not include a hidden resolved target: {json:#}"
    );

    let alternatives = ambiguous_error_alternatives(&json);
    assert!(
        alternatives.len() >= 2,
        "ambiguous JSON should expose at least the tied alternatives: {json:#}"
    );
    for (offset, alternative) in alternatives.iter().take(2).enumerate() {
        assert_eq!(
            alternative.get("number").and_then(Value::as_u64),
            Some((offset + 1) as u64),
            "alternatives should be numbered in displayed 1-based order: {json:#}"
        );
        assert!(
            alternative.get("node_id").and_then(Value::as_str).is_some(),
            "alternative should include stable node_id: {alternative:#}"
        );
        assert!(
            alternative
                .get("node_ref")
                .and_then(Value::as_str)
                .is_some(),
            "alternative should include stable node_ref: {alternative:#}"
        );
    }

    let filtered = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "symbol",
            "--query",
            "configure",
            "--file",
            "src",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    let filtered_json = parse_stdout_json(&filtered);
    assert!(
        filtered_json
            .pointer("/error/next_commands")
            .and_then(Value::as_array)
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .is_some_and(|value| value.contains("--file \"src\"")))),
        "filtered ambiguity next command should preserve the file filter: {filtered_json:#}"
    );

    let explore = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "explore",
            "--query",
            "configure",
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !explore.status.success(),
        "ambiguous explore query must fail without silently choosing a target"
    );
    let explore_json = parse_stdout_json(&explore);
    assert_eq!(
        explore_json
            .pointer("/error/failed_layer")
            .and_then(Value::as_str),
        Some("query_resolution"),
        "explore ambiguity should preserve the failed query-resolution layer: {explore_json:#}"
    );
}

#[test]
fn choose_flag_resolves_by_displayed_alternative_number_when_available() {
    let help = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .args(["symbol", "--help"])
        .output()
        .expect("run symbol help");
    assert_success(&help, "symbol --help failed");
    let help_text = String::from_utf8_lossy(&help.stdout);
    if !help_text.contains("--choose") {
        return;
    }

    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_ambiguous_rust_workspace(workspace.path());
    index_workspace(workspace.path(), cache_dir.path());

    let ambiguous = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "symbol",
            "--query",
            "configure",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !ambiguous.status.success(),
        "baseline ambiguous query should fail before --choose is applied"
    );
    let ambiguous_json = parse_stdout_json(&ambiguous);
    let second_alternative = ambiguous_error_alternatives(&ambiguous_json)
        .iter()
        .find(|alternative| alternative.get("number").and_then(Value::as_u64) == Some(2))
        .expect("displayed alternative #2");
    let expected_node_id = second_alternative
        .get("node_id")
        .and_then(Value::as_str)
        .expect("alternative #2 node_id")
        .to_string();

    let chosen = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "symbol",
            "--query",
            "configure",
            "--choose",
            "2",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_success(&chosen, "--choose 2 should resolve deterministically");
    let chosen_json = parse_stdout_json(&chosen);
    assert_eq!(
        chosen_json
            .pointer("/resolution/resolved/node_id")
            .and_then(Value::as_str),
        Some(expected_node_id.as_str()),
        "--choose should resolve the node id displayed as alternative #2"
    );
}
