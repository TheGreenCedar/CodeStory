//! CLI JSON contracts for `retrieval bootstrap` storage repair output.

use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn run_bootstrap(project: &std::path::Path, extra_args: &[&str]) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command.args(["retrieval", "bootstrap", "--project"]);
    command.arg(project);
    command.args(extra_args);
    let output = command.output().expect("run retrieval bootstrap");
    assert!(
        output.status.success(),
        "bootstrap failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse bootstrap json")
}

fn create_valid_cache_with_cli(project: &std::path::Path, cache: &std::path::Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .args(["index", "--project"])
        .arg(project)
        .args(["--cache-dir"])
        .arg(cache)
        .args(["--refresh", "full"])
        .output()
        .expect("run index to create valid cache");
    assert!(
        output.status.success(),
        "index failed while creating valid cache: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bootstrap_json_includes_storage_repair_fields() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");

    let json = run_bootstrap(
        project.path(),
        &["--skip-compose", "--wait-secs", "0", "--format", "json"],
    );
    let repair = &json["storage_repair"];
    assert!(
        repair.is_object(),
        "expected storage_repair object: {repair}"
    );
    for field in [
        "qdrant_reachable",
        "removed_invalid_dirs",
        "migrated_legacy_stub_markers",
        "pruned_collections",
        "protected_collections",
        "collections_seen",
        "prune_candidates",
        "overflow_protected",
        "scan_errors",
        "sources_scanned",
        "prune_suppressed_reason",
    ] {
        assert!(
            repair.get(field).is_some(),
            "storage_repair missing `{field}`: {repair}"
        );
    }
    assert!(
        json.get("embed_reachable").is_some(),
        "bootstrap output missing embed_reachable: {json}"
    );
    assert!(
        json.get("embed_detail").is_some(),
        "bootstrap output missing embed_detail: {json}"
    );
    assert!(repair["scan_errors"].is_array());
    assert!(
        repair["prune_suppressed_reason"].is_null()
            || repair["prune_suppressed_reason"].is_string(),
        "prune_suppressed_reason must be null or string: {}",
        repair["prune_suppressed_reason"]
    );
}

#[test]
fn bootstrap_json_surfaces_prune_suppressed_reason_on_scan_errors() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");
    let cache = tempdir().expect("cache");
    create_valid_cache_with_cli(project.path(), cache.path());
    let corrupt_subdir = cache.path().join("deadbeefdeadbeef");
    fs::create_dir_all(&corrupt_subdir).expect("corrupt subdir");
    fs::write(corrupt_subdir.join("codestory.db"), b"not sqlite").expect("corrupt hashed db");

    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command.args([
        "retrieval",
        "bootstrap",
        "--project",
        project.path().to_str().expect("utf8 path"),
        "--cache-dir",
        cache.path().to_str().expect("utf8 cache"),
        "--skip-compose",
        "--wait-secs",
        "0",
        "--format",
        "json",
    ]);
    let output = command.output().expect("bootstrap with corrupt cache");
    assert!(
        output.status.success(),
        "bootstrap failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("parse json");
    let repair = &json["storage_repair"];
    assert!(
        repair["scan_errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty()),
        "expected scan_errors: {repair}"
    );
    assert_eq!(
        repair["prune_suppressed_reason"].as_str(),
        Some("protection_scan_error")
    );
    assert_eq!(repair["pruned_collections"].as_u64(), Some(0));
}
