use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn report_command_help_names_markdown_and_json_exports() {
    let output = test_support::cli_command()
        .arg("report")
        .arg("--help")
        .output()
        .expect("run report help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help output is utf8");
    assert!(stdout.contains("Generate a repo report or machine graph export"));
    assert!(stdout.contains("--format <FORMAT>"));
    assert!(stdout.contains("--output-file <PATH>"));
    assert!(stdout.contains("--limit <N>"));
    assert!(stdout.contains("--profile <PROFILE>"));
}

#[test]
fn report_handoff_profile_renders_handoff_header_and_json_metadata() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    run_cli(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    let markdown = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "report",
            "--profile",
            "handoff",
            "--limit",
            "3",
            "--format",
            "markdown",
        ],
    );
    assert!(markdown.contains("## Read This First / Agent Handoff"));
    assert!(markdown.contains("readiness agent_packet_search"));
    assert!(markdown.contains("## Suggested Follow-up Queries"));
    assert!(
        !markdown.contains("## Repo Summary"),
        "handoff profile should trim default report sections:\n{markdown}"
    );

    let json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["report", "--limit", "3", "--format", "json"],
    );
    let json: Value = serde_json::from_str(&json_text).expect("report json");
    assert!(
        json.pointer("/metadata/handoff/readiness/0").is_some(),
        "report json should include metadata.handoff.readiness: {json}"
    );
    assert!(
        json.pointer("/metadata/handoff/next_command")
            .and_then(Value::as_str)
            .is_some_and(|command| command.contains("codestory-cli")),
        "report json should include a handoff next command: {json}"
    );
}

fn run_cli(workspace: &Path, cache_dir: &Path, args: &[&str]) -> String {
    let output = test_support::cli_command()
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .output()
        .expect("run codestory-cli");
    assert!(
        output.status.success(),
        "command failed: {args:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout utf8")
}

fn write_tiny_rust_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "report-handoff-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(
        src.join("lib.rs"),
        r#"pub fn entry_point() -> String {
    helper("report")
}

fn helper(value: &str) -> String {
    format!("handoff:{value}")
}
"#,
    )
    .expect("write lib.rs");
}
mod test_support;
