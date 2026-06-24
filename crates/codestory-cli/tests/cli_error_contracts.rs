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

fn write_many_ambiguous_rust_workspace(root: &Path, count: usize) {
    fs::create_dir_all(root.join("src")).expect("create src dir");
    let mut lib = String::new();
    for index in 1..=count {
        lib.push_str(&format!("pub mod candidate_{index};\n"));
        fs::write(
            root.join("src").join(format!("candidate_{index}.rs")),
            format!(
                r#"pub fn configure() -> usize {{
    {index}
}}
"#
            ),
        )
        .expect("write candidate module");
    }
    fs::write(root.join("src").join("lib.rs"), lib).expect("write lib.rs");
}

fn write_many_ambiguous_rust_workspace_under_dir(root: &Path, dir: &str, count: usize) {
    let source_dir = root.join("src").join(dir);
    fs::create_dir_all(&source_dir).expect("create nested source dir");
    let mut lib = String::new();
    for index in 1..=count {
        lib.push_str(&format!(
            "#[path = \"{dir}/candidate_{index}.rs\"]\npub mod candidate_{index};\n"
        ));
        fs::write(
            source_dir.join(format!("candidate_{index}.rs")),
            format!(
                r#"pub fn configure() -> usize {{
    {index}
}}
"#
            ),
        )
        .expect("write nested candidate module");
    }
    fs::write(root.join("src").join("lib.rs"), lib).expect("write lib.rs");
}

fn write_drill_friction_workspace(root: &Path) {
    fs::create_dir_all(root.join("src").join("collections")).expect("create collections dir");
    fs::create_dir_all(root.join("src").join("components")).expect("create components dir");

    fs::write(
        root.join("src").join("collections").join("Posts.ts"),
        r#"export const Posts = {
  slug: 'posts',
  fields: [{ name: 'title', type: 'text' }],
}
"#,
    )
    .expect("write Posts.ts");
    fs::write(
        root.join("src").join("collections").join("Comments.ts"),
        r#"export const Comments = {
  slug: 'comments',
  fields: [{ name: 'body', type: 'textarea' }],
}
"#,
    )
    .expect("write Comments.ts");
    fs::write(
        root.join("src").join("payload-types.ts"),
        r#"export interface PayloadTypes {
  posts: Posts
  comments: Comments
}

export interface Posts {
  id: string
}

export interface Comments {
  id: string
}
"#,
    )
    .expect("write payload-types.ts");
    fs::write(
        root.join("src")
            .join("components")
            .join("ElsewhereFeed.tsx"),
        r#"export type ElsewhereFeedProps = {
  profiles: string[]
}

export function ElsewhereFeed(props: ElsewhereFeedProps) {
  return <section>{props.profiles.length}</section>
}
"#,
    )
    .expect("write ElsewhereFeed.tsx");
    fs::write(
        root.join("src").join("main.rs"),
        r#"pub fn run_index() {}
pub fn run_index_once() {}

#[cfg(test)]
mod tests {
    #[test]
    fn test_rust_tauri_command_registration_indexes_command_symbol_and_boundary() {}
}
"#,
    )
    .expect("write main.rs");
    fs::write(
        root.join("src").join("browser.rs"),
        r#"pub struct ReadOnlyBrowserService;

impl ReadOnlyBrowserService {
    pub fn snippet_context(&self) {}
    pub fn trail_context(&self) {}
}
"#,
    )
    .expect("write browser.rs");
    fs::write(
        root.join("src").join("grounding.rs"),
        r#"pub struct AppController;

impl AppController {
    pub fn snippet_context(&self) {}
    pub fn trail_context(&self) {}
}
"#,
    )
    .expect("write grounding.rs");
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

fn git(workspace: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .expect("run git");
    assert_success(&output, &format!("git {} failed", args.join(" ")));
}

fn json_string<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected string at {pointer}: {value:#}"))
}

fn normalized_path(value: &str) -> String {
    value.replace('\\', "/")
}

fn ambiguous_error_alternatives(json: &Value) -> &Vec<Value> {
    json.pointer("/error/alternatives")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ambiguous JSON should expose /error/alternatives: {json:#}"))
}

#[test]
fn top_level_help_names_command_purposes() {
    let help = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("--help")
        .output()
        .expect("run top-level help");
    assert_success(&help, "codestory-cli --help failed");
    let help_text = String::from_utf8_lossy(&help.stdout);

    for (command, purpose) in [
        ("index", "Build or refresh the repository index."),
        ("search", "Find symbols and repo text evidence."),
        (
            "packet",
            "Answer a broad repository question with evidence.",
        ),
        ("doctor", "Check cache, index, and retrieval health."),
        (
            "smoke",
            "Run a machine-readable smoke profile for CI and agent images.",
        ),
        ("setup", "Install or check local setup assets."),
        ("cache", "Prepare or inspect local cache artifacts."),
        ("symbol", "Inspect a symbol by query or id."),
        (
            "explore",
            "Open the terminal explorer or print an exploration packet.",
        ),
    ] {
        assert!(
            help_text
                .lines()
                .any(|line| line.trim_start().starts_with(command) && line.contains(purpose)),
            "top-level help should show {command:?} purpose {purpose:?}, not only command names:\n{help_text}"
        );
    }
}

#[test]
fn smoke_ci_agent_json_runs_tiny_repo_profile() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["smoke", "--profile", "ci-agent", "--format", "json"],
    );
    assert_success(&output, "smoke ci-agent failed");

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse smoke json");
    assert_eq!(json_string(&json, "/profile"), "ci-agent");
    assert_eq!(json_string(&json, "/status"), "pass");

    let checked = json["checked_surfaces"]
        .as_array()
        .expect("checked surfaces array");
    for surface in ["index", "ground", "symbol", "affected"] {
        assert!(
            checked
                .iter()
                .any(|item| item["surface"] == surface && item["status"] == "pass"),
            "smoke should pass required surface {surface}: {json:#}"
        );
    }
    assert!(
        checked
            .iter()
            .any(|item| item["surface"] == "sidecar_full_mode")
            || json["skipped_optional_surfaces"]
                .as_array()
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item["surface"] == "sidecar_full_mode")),
        "sidecar full mode should be checked or explicitly skipped: {json:#}"
    );
}

#[test]
fn smoke_ci_agent_invalid_project_emits_json_failure() {
    let workspace = tempdir().expect("workspace dir");
    let missing = workspace.path().join("missing");

    let output = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .args([
            "smoke",
            "--profile",
            "ci-agent",
            "--format",
            "json",
            "--project",
        ])
        .arg(&missing)
        .output()
        .expect("run codestory-cli");
    assert!(
        !output.status.success(),
        "invalid project smoke should fail, stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("parse smoke json");
    assert_eq!(json_string(&json, "/profile"), "ci-agent");
    assert_eq!(json_string(&json, "/status"), "fail");
    assert_eq!(json_string(&json, "/checked_surfaces/0/surface"), "project");
    assert!(
        json["repair_hints"]
            .as_array()
            .is_some_and(|hints| !hints.is_empty()),
        "failure JSON should include repair hints: {json:#}"
    );
}

#[test]
fn cache_identity_json_reports_canonical_contract_fields() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    git(workspace.path(), &["init"]);
    git(
        workspace.path(),
        &["config", "user.email", "codestory@example.invalid"],
    );
    git(workspace.path(), &["config", "user.name", "CodeStory Test"]);
    git(
        workspace.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:TheGreenCedar/CodeStory.git",
        ],
    );
    git(workspace.path(), &["add", "."]);
    git(workspace.path(), &["commit", "-m", "init"]);

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["cache", "identity", "--format", "json"],
    );
    assert_success(&output, "cache identity should succeed");
    let json = parse_stdout_json(&output);

    assert_eq!(
        json["normalized_repository_identity"],
        "github.com/thegreencedar/codestory"
    );
    assert_eq!(json["repository_identity_schema_version"], 1);
    assert_eq!(json["portable_reuse_eligible"], true);
    assert!(
        json["canonical_repository_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("repo-v1-"))
    );
    assert!(json["root_derived_project_id"].as_str().is_some());
    assert!(json["git_tree"].as_str().is_some());
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
            "use markdown or json",
        ],
    );
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "format validation should happen before runtime cache creation"
    );
}

#[test]
fn query_sql_flag_reports_graph_dsl_guidance_before_runtime_cache_creation() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "query",
            "--sql",
            "select path, kind, name from symbols",
            "--refresh",
            "none",
        ],
    );

    assert_fails_with(
        output,
        &[
            "uses the graph-query DSL, not SQL",
            "search(query: 'AppController') | limit(5)",
            "For raw symbol discovery, use `search --query",
        ],
    );
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "SQL guardrail should fail before runtime cache creation"
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
fn snippet_lines_alias_sets_context_for_agent_guesses() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    index_workspace(workspace.path(), cache_dir.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query",
            "AppController",
            "--lines",
            "12",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );

    assert_success(&output, "snippet --lines alias should succeed");
    let json = parse_stdout_json(&output);
    assert_eq!(
        json.pointer("/snippet/requested_context")
            .and_then(Value::as_u64),
        Some(12),
        "--lines should feed the same context field as --context: {json:#}"
    );
}

#[test]
fn graph_query_resolution_handles_real_drill_friction_patterns() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_drill_friction_workspace(workspace.path());
    index_workspace(workspace.path(), cache_dir.path());

    let posts = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query",
            "Posts",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_success(&posts, "Posts should resolve to the collection config");
    let posts_json = parse_stdout_json(&posts);
    assert!(
        normalized_path(json_string(&posts_json, "/snippet/path"))
            .ends_with("src/collections/Posts.ts"),
        "Posts should choose the source collection config, not generated payload types: {posts_json:#}"
    );

    let comments = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "trail",
            "--query",
            "comments",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_success(
        &comments,
        "lowercase comments should resolve to the collection config",
    );
    let comments_json = parse_stdout_json(&comments);
    assert_eq!(
        json_string(&comments_json, "/trail/focus/display_name"),
        "Comments",
        "comments should choose the collection anchor: {comments_json:#}"
    );

    let guessed_page = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query",
            "ElsewherePage",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !guessed_page.status.success(),
        "guessed ElsewherePage should not silently resolve to semantic neighbors"
    );
    let guessed_combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&guessed_page.stdout),
        String::from_utf8_lossy(&guessed_page.stderr)
    );
    assert!(
        guessed_combined.contains("No symbol matched query `ElsewherePage`")
            && guessed_combined.contains("search --project")
            && !guessed_combined.contains("ambiguous_target"),
        "guessed names should produce a scoped no-match recovery, not ambiguity: {guessed_combined}"
    );

    let run_index = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query",
            "run_index_command",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_success(
        &run_index,
        "run_index_command should resolve to production run_index",
    );
    let run_index_json = parse_stdout_json(&run_index);
    assert_eq!(
        json_string(&run_index_json, "/snippet/node/display_name"),
        "run_index",
        "command-shaped guesses should prefer production entrypoints over tests: {run_index_json:#}"
    );

    let snippet_context = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query",
            "snippet_context",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_success(
        &snippet_context,
        "snippet_context should prefer implementation over facade",
    );
    let snippet_context_json = parse_stdout_json(&snippet_context);
    assert!(
        normalized_path(json_string(&snippet_context_json, "/snippet/path"))
            .ends_with("src/grounding.rs"),
        "facade method names should resolve to implementation files: {snippet_context_json:#}"
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
fn ambiguous_query_prioritizes_next_commands_and_caps_human_alternatives() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_many_ambiguous_rust_workspace(workspace.path(), 12);

    index_workspace(workspace.path(), cache_dir.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["symbol", "--query", "configure", "--refresh", "none"],
    );

    assert!(
        !output.status.success(),
        "ambiguous query must fail before a target is selected\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let next_commands = combined
        .find("Next commands:")
        .unwrap_or_else(|| panic!("missing Next commands in:\n{combined}"));
    let alternatives = combined
        .find("Top equally ranked matches")
        .unwrap_or_else(|| panic!("missing alternatives heading in:\n{combined}"));
    assert!(
        next_commands < alternatives,
        "Next commands should precede alternatives in the human diagnostic:\n{combined}"
    );
    assert!(
        combined.contains("Top equally ranked matches (showing 10 of 12):"),
        "human diagnostic should explain the displayed cap:\n{combined}"
    );
    let displayed_alternatives = combined
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.contains(" id=`")
                && trimmed
                    .split_once('.')
                    .is_some_and(|(number, _)| number.chars().all(|ch| ch.is_ascii_digit()))
        })
        .count();
    assert_eq!(
        displayed_alternatives, 10,
        "human diagnostic should display at most 10 alternatives:\n{combined}"
    );

    let json_output = run_cli(
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
        !json_output.status.success(),
        "ambiguous JSON query should fail without selecting a target"
    );
    let json = parse_stdout_json(&json_output);
    assert_eq!(
        ambiguous_error_alternatives(&json).len(),
        12,
        "structured ambiguity JSON should keep every tied alternative: {json:#}"
    );
}

#[test]
fn ambiguous_query_quotes_shell_sensitive_file_filters_in_next_commands() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_many_ambiguous_rust_workspace_under_dir(workspace.path(), "$hidden path", 2);
    index_workspace(workspace.path(), cache_dir.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "symbol",
            "--query",
            "configure",
            "--file",
            "$hidden path",
            "--refresh",
            "none",
        ],
    );

    assert!(
        !output.status.success(),
        "filtered ambiguous query should fail before selecting a target\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("--file '$hidden path'"),
        "human diagnostic should single-quote shell-sensitive file filters:\n{combined}"
    );
    assert!(
        !combined.contains("--file \"$hidden path\""),
        "human diagnostic should not double-quote shell-sensitive file filters:\n{combined}"
    );

    let json_output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "symbol",
            "--query",
            "configure",
            "--file",
            "$hidden path",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !json_output.status.success(),
        "filtered ambiguous JSON query should fail before selecting a target"
    );
    let json = parse_stdout_json(&json_output);
    assert_eq!(
        ambiguous_error_alternatives(&json).len(),
        2,
        "filtered ambiguity JSON should keep every tied alternative: {json:#}"
    );
    assert!(
        json.pointer("/error/next_commands")
            .and_then(Value::as_array)
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .is_some_and(|value| value.contains("--file '$hidden path'")))),
        "structured next commands should single-quote shell-sensitive file filters: {json:#}"
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
        json.pointer("/error/layer_notes")
            .and_then(Value::as_array)
            .is_some_and(|notes| notes.iter().any(|note| note
                .as_str()
                .is_some_and(|note| note.contains("--query \"configure\"")))),
        "ambiguous JSON layer note should include the searched query when it names search: {json:#}"
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
fn ambiguous_query_writes_output_file_even_on_failure() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_many_ambiguous_rust_workspace(workspace.path(), 12);
    index_workspace(workspace.path(), cache_dir.path());
    let output_file = cache_dir.path().join("ambiguous-snippet.md");

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query",
            "configure",
            "--refresh",
            "none",
            "--output-file",
            output_file.to_str().expect("utf-8 temp path"),
        ],
    );

    assert!(
        !output.status.success(),
        "ambiguous snippet should still fail after writing diagnostics"
    );
    let diagnostic = fs::read_to_string(&output_file).expect("read ambiguity output file");
    assert!(diagnostic.contains("# Command Error"), "{diagnostic}");
    assert!(
        diagnostic.contains("code: ambiguous_target"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("alternatives: 12"), "{diagnostic}");
    assert!(
        diagnostic.contains(
            "showing: 10 of 12; use `--format json` or `search` to inspect all alternatives"
        ),
        "{diagnostic}"
    );
    assert!(
        !diagnostic.contains("Top equally ranked matches"),
        "markdown output-file diagnostics should not duplicate the embedded human alternatives from the message:\n{diagnostic}"
    );
    let next_commands = diagnostic
        .find("next_commands:")
        .unwrap_or_else(|| panic!("missing next_commands in:\n{diagnostic}"));
    let alternatives = diagnostic
        .find("alternatives: 12")
        .unwrap_or_else(|| panic!("missing alternatives heading in:\n{diagnostic}"));
    assert!(
        next_commands < alternatives,
        "markdown output-file diagnostics should put next commands before alternatives:\n{diagnostic}"
    );
    let visible_alternative_rows = diagnostic
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            let numbered_runtime_row = trimmed.contains(" id=`")
                && trimmed
                    .split_once('.')
                    .is_some_and(|(number, _)| number.chars().all(|ch| ch.is_ascii_digit()));
            let structured_markdown_row =
                trimmed.starts_with("- [") && trimmed.contains("configure");
            numbered_runtime_row || structured_markdown_row
        })
        .count();
    assert_eq!(
        visible_alternative_rows, 10,
        "markdown output-file diagnostics should render exactly one capped alternatives section:\n{diagnostic}"
    );

    let json_output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query",
            "configure",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !json_output.status.success(),
        "ambiguous JSON snippet should fail without selecting a target"
    );
    let json = parse_stdout_json(&json_output);
    assert_eq!(
        ambiguous_error_alternatives(&json).len(),
        12,
        "structured ambiguity JSON should keep every tied alternative: {json:#}"
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
