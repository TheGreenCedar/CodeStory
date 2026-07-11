use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Output;
use tempfile::tempdir;

fn write_tiny_project(root: &Path) {
    fs::write(root.join("lib.rs"), "pub fn main() {}\n").expect("write tiny project");
}

fn run_cli(project: &Path, cache_dir: &Path, args: &[&str]) -> Output {
    let mut command = test_support::cli_command();
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
    assert!(
        stdout.contains("sidecar_images:") && stdout.contains("@sha256:"),
        "sidecar status should expose digest-pinned images:\n{stdout}"
    );
}

#[test]
fn sidecar_inventory_reports_read_only_dry_run_json() {
    let project = tempdir().expect("project");
    let cache_dir = tempdir().expect("cache");
    write_tiny_project(project.path());

    let output = run_cli(
        project.path(),
        cache_dir.path(),
        &["sidecar", "inventory", "--format", "json"],
    );
    assert_success(&output, "sidecar inventory failed");

    let json = stdout_json(&output);
    assert_eq!(json["dry_run"].as_bool(), Some(true));
    assert!(json["namespaces"].is_array());
    assert_eq!(
        json["generation_retention"]["dry_run"].as_bool(),
        Some(true)
    );
    assert_eq!(
        json["generation_retention"]["pruning_suppressed"].as_bool(),
        Some(true)
    );
    assert!(
        json["generation_retention"]["errors"]
            .as_array()
            .is_some_and(|errors| errors.iter().any(|error| error
                .as_str()
                .is_some_and(|error| error.contains("active retrieval manifest is unavailable"))))
    );
}

#[test]
fn retrieval_inventory_has_human_output() {
    let project = tempdir().expect("project");
    let cache_dir = tempdir().expect("cache");
    write_tiny_project(project.path());

    let output = run_cli(
        project.path(),
        cache_dir.path(),
        &["retrieval", "inventory", "--format", "markdown"],
    );
    assert_success(&output, "retrieval inventory failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("# Retrieval sidecar inventory"),
        "inventory should have a human-readable heading:\n{stdout}"
    );
    assert!(stdout.contains("dry_run"));
    assert!(stdout.contains("generation_retention_pruning_suppressed: true"));
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
    assert!(combined.contains("codestory-cli sidecar inventory"));
    assert!(combined.contains("codestory-cli retrieval status"));
    assert!(
        !cache_dir.path().join("codestory.db").exists(),
        "unknown sidecar subcommand should fail before runtime cache creation"
    );
}
mod test_support;
