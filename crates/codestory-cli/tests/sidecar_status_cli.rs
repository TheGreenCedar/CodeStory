use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

fn write_tiny_project(root: &Path) {
    fs::write(root.join("lib.rs"), "pub fn main() {}\n").expect("write tiny project");
}

fn run_cli(project: &Path, cache_dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command.args(args);
    command.arg("--project").arg(project);
    command.arg("--cache-dir").arg(cache_dir);
    command.output().expect("run codestory-cli")
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout should be JSON: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn sidecar_status_aliases_retrieval_status_json() {
    let project = tempdir().expect("project");
    let cache_dir = tempdir().expect("cache");
    write_tiny_project(project.path());

    let retrieval = run_cli(
        project.path(),
        cache_dir.path(),
        &["retrieval", "status", "--format", "json"],
    );
    assert_success(&retrieval, "retrieval status failed");

    let sidecar = run_cli(
        project.path(),
        cache_dir.path(),
        &["sidecar", "status", "--format", "json"],
    );
    assert_success(&sidecar, "sidecar status failed");

    assert_eq!(stdout_json(&sidecar), stdout_json(&retrieval));
}

#[test]
fn sidecar_status_uses_retrieval_status_human_output() {
    let project = tempdir().expect("project");
    let cache_dir = tempdir().expect("cache");
    write_tiny_project(project.path());

    let output = run_cli(
        project.path(),
        cache_dir.path(),
        &["sidecar", "status", "--format", "markdown"],
    );
    assert_success(&output, "sidecar status failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("# Retrieval status"),
        "sidecar status should reuse retrieval status output:\n{stdout}"
    );
}

#[test]
fn unknown_sidecar_subcommand_suggests_status_without_runtime_side_effects() {
    let project = tempdir().expect("project");
    let cache_dir = tempdir().expect("cache");
    write_tiny_project(project.path());

    let output = run_cli(project.path(), cache_dir.path(), &["sidecar", "frobnicate"]);
    assert!(
        !output.status.success(),
        "unknown sidecar subcommand should fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("unknown sidecar subcommand `frobnicate`"));
    assert!(combined.contains("codestory-cli sidecar status"));
    assert!(combined.contains("codestory-cli retrieval status"));
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "unknown sidecar subcommand should fail before runtime cache creation"
    );
}
