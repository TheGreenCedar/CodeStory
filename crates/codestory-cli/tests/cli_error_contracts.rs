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
fn ambiguous_query_lists_ranked_alternatives_and_next_steps() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_ambiguous_rust_workspace(workspace.path());

    let index = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&index.stdout),
        String::from_utf8_lossy(&index.stderr)
    );

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
