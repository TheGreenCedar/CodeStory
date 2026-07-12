//! CLI JSON contracts for `retrieval bootstrap` storage repair output.

use serde_json::Value;
use std::fs;
use tempfile::tempdir;

fn run_bootstrap(project: &std::path::Path, extra_args: &[&str]) -> Value {
    let mut command = test_support::cli_command();
    command.env("CODESTORY_EMBED_ALLOW_CPU", "1");
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

fn run_status(project: &std::path::Path, extra_args: &[&str]) -> Value {
    let mut command = test_support::cli_command();
    command.env("CODESTORY_EMBED_ALLOW_CPU", "1");
    command.args(["retrieval", "status", "--project"]);
    command.arg(project);
    command.args(extra_args);
    let output = command.output().expect("run retrieval status");
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse status json")
}

fn run_down(project: &std::path::Path, extra_args: &[&str]) {
    let mut command = test_support::cli_command();
    command.env("CODESTORY_EMBED_ALLOW_CPU", "1");
    command.args(["retrieval", "down", "--project"]);
    command.arg(project);
    command.args(extra_args);
    let output = command.output().expect("run retrieval down");
    assert!(
        output.status.success(),
        "down failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_digest_image_pins(json: &Value) {
    for field in ["qdrant", "embed"] {
        let image = json[field]
            .as_str()
            .unwrap_or_else(|| panic!("sidecar_images.{field} should be a string: {json}"));
        assert!(
            image.contains("@sha256:"),
            "sidecar_images.{field} must include a digest pin: {image}"
        );
    }
}

fn create_valid_cache_with_cli(project: &std::path::Path, cache: &std::path::Path) {
    let output = test_support::cli_command()
        .env("CODESTORY_EMBED_ALLOW_CPU", "1")
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

fn write_embed_model_fixture(project: &std::path::Path) {
    let model_dir = project.join("models").join("gguf").join("bge-base-en-v1.5");
    fs::create_dir_all(&model_dir).expect("model dir");
    fs::write(
        model_dir.join("bge-base-en-v1.5.Q8_0.gguf"),
        b"model placeholder",
    )
    .expect("model file");
}

#[test]
fn bootstrap_json_includes_storage_repair_fields() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");
    write_embed_model_fixture(project.path());

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
    assert_digest_image_pins(&json["sidecar_state"]["sidecar_images"]);
    assert!(repair["scan_errors"].is_array());
    assert!(
        repair["prune_suppressed_reason"].is_null()
            || repair["prune_suppressed_reason"].is_string(),
        "prune_suppressed_reason must be null or string: {}",
        repair["prune_suppressed_reason"]
    );
}

#[test]
fn bootstrap_then_status_reports_manifest_missing_before_indexing() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");
    write_embed_model_fixture(project.path());
    let cache = tempdir().expect("cache");
    let cache_arg = cache.path().to_str().expect("utf8 cache");

    let bootstrap = run_bootstrap(
        project.path(),
        &[
            "--cache-dir",
            cache_arg,
            "--skip-compose",
            "--wait-secs",
            "0",
            "--format",
            "json",
        ],
    );
    assert!(
        bootstrap["storage_repair"].is_object(),
        "bootstrap output missing storage repair: {bootstrap}"
    );
    assert_digest_image_pins(&bootstrap["sidecar_state"]["sidecar_images"]);

    let status = run_status(
        project.path(),
        &["--cache-dir", cache_arg, "--format", "json"],
    );
    assert_digest_image_pins(&status["sidecar_images"]);
    assert!(
        status["readiness_broker"].is_object(),
        "retrieval status should include durable broker parity: {status}"
    );
    assert!(
        status["readiness_broker"]["gpu_proof"].is_object(),
        "retrieval status broker should include GPU proof shape: {status}"
    );
    assert_eq!(
        status["degraded_reason"].as_str(),
        Some("retrieval_manifest_missing"),
        "status should report manifest-missing shape before indexing: {status}"
    );
    assert_ne!(
        status["retrieval_mode"].as_str(),
        Some("full"),
        "manifest-missing status must not report full mode before retrieval index: {status}"
    );
    assert!(
        status["manifest_contract"].is_null(),
        "manifest-missing status must not derive a manifest contract: {status}"
    );
    assert_eq!(
        status["repair"]["reason"].as_str(),
        Some("retrieval_manifest_missing"),
        "manifest-missing status should expose a typed repair reason: {status}"
    );
    assert!(
        status["repair"]["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval bootstrap")),
        "manifest-missing status should expose an actionable repair command: {status}"
    );
    assert!(
        status["repair"]["full_repair"]
            .as_array()
            .is_some_and(
                |commands| commands
                    .iter()
                    .any(|command| command
                        .as_str()
                        .is_some_and(|text| text.contains("retrieval index")
                            && text.contains("--refresh full")))
            ),
        "manifest-missing status should include full sidecar rebuild guidance: {status}"
    );
}

#[test]
fn agent_profile_bootstrap_status_and_down_are_project_isolated() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");
    write_embed_model_fixture(project.path());
    let cache = tempdir().expect("cache");
    let cache_arg = cache.path().to_str().expect("utf8 cache");

    let bootstrap = run_bootstrap(
        project.path(),
        &[
            "--cache-dir",
            cache_arg,
            "--profile",
            "agent",
            "--run-id",
            "contract-a",
            "--skip-compose",
            "--wait-secs",
            "0",
            "--format",
            "json",
        ],
    );
    let state = &bootstrap["sidecar_state"];
    assert_eq!(state["owner"].as_str(), Some("codestory"));
    assert_eq!(state["profile"].as_str(), Some("agent"));
    let namespace = state["namespace"].as_str().expect("namespace");
    assert!(namespace.starts_with("codestory-agent-"));
    assert!(
        namespace.ends_with("-contract-a"),
        "agent namespace should carry the run id: {namespace}"
    );
    assert_eq!(state["run_id"].as_str(), Some("contract-a"));
    assert!(state.get("lexical_http_port").is_none());
    assert!(
        state["qdrant_http_port"]
            .as_u64()
            .is_some_and(|port| port > 0)
    );
    assert!(
        state["qdrant_grpc_port"]
            .as_u64()
            .is_some_and(|port| port > 0)
    );
    assert!(
        state["embed_http_port"]
            .as_u64()
            .is_some_and(|port| port > 0)
    );
    assert_ne!(state["qdrant_http_port"].as_u64(), Some(6333));
    assert_ne!(state["qdrant_grpc_port"].as_u64(), Some(6334));
    assert_ne!(state["embed_http_port"].as_u64(), Some(8080));
    assert!(
        state["cleanup_command"]
            .as_str()
            .is_some_and(|command| command.contains("--profile agent")),
        "agent state should name the cleanup path: {state}"
    );
    let status = run_status(
        project.path(),
        &[
            "--cache-dir",
            cache_arg,
            "--profile",
            "agent",
            "--run-id",
            "contract-a",
            "--format",
            "json",
        ],
    );
    let ownership = &status["ownership"];
    assert_eq!(ownership["profile"].as_str(), Some("agent"));
    assert_eq!(ownership["namespace"].as_str(), Some(namespace));
    assert!(ownership["ports"].get("lexical_http").is_none());
    let state_path =
        std::path::PathBuf::from(ownership["state_file"].as_str().expect("state file"));
    assert!(state_path.is_file(), "state file should exist before down");

    run_down(
        project.path(),
        &[
            "--cache-dir",
            cache_arg,
            "--profile",
            "agent",
            "--run-id",
            "contract-a",
        ],
    );
    assert!(
        !state_path.exists(),
        "down should remove only the owned state file"
    );
}

#[test]
fn agent_profile_status_repair_hints_are_profile_aware() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");
    let cache = tempdir().expect("cache");
    let cache_arg = cache.path().to_str().expect("utf8 cache");

    let status = run_status(
        project.path(),
        &[
            "--cache-dir",
            cache_arg,
            "--profile",
            "agent",
            "--run-id",
            "repair-a",
            "--format",
            "json",
        ],
    );

    let repair = &status["repair"];
    assert_eq!(
        repair["reason"].as_str(),
        Some("retrieval_manifest_missing")
    );
    assert!(
        repair["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("--profile agent")
                && command.contains("--run-id repair-a")),
        "agent status repair command should keep the profile/run id: {status}"
    );
    let full_repair = repair["full_repair"]
        .as_array()
        .expect("full repair commands");
    assert!(
        full_repair
            .iter()
            .all(|command| command.as_str().is_some_and(
                |text| text.contains("--profile agent") && text.contains("--run-id repair-a")
            )),
        "agent full repair commands should keep the profile/run id: {status}"
    );
}

#[test]
fn run_id_status_implies_agent_profile() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");
    let cache = tempdir().expect("cache");
    let cache_arg = cache.path().to_str().expect("utf8 cache");

    let status = run_status(
        project.path(),
        &[
            "--cache-dir",
            cache_arg,
            "--run-id",
            "implicit-agent",
            "--format",
            "json",
        ],
    );

    let ownership = &status["ownership"];
    assert_eq!(ownership["profile"].as_str(), Some("agent"));
    assert!(
        ownership["namespace"]
            .as_str()
            .is_some_and(|namespace| namespace.ends_with("-implicit-agent")),
        "status --run-id should inspect the named agent namespace: {status}"
    );
    let repair = &status["repair"];
    assert!(
        repair["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("--profile agent")
                && command.contains("--run-id implicit-agent")),
        "status --run-id repair command should keep the inferred agent run id: {status}"
    );
}

#[test]
fn bootstrap_json_surfaces_prune_suppressed_reason_on_scan_errors() {
    let project = tempdir().expect("project");
    fs::write(project.path().join("lib.rs"), "pub fn main() {}\n").expect("source");
    write_embed_model_fixture(project.path());
    let cache = tempdir().expect("cache");
    create_valid_cache_with_cli(project.path(), cache.path());
    let corrupt_subdir = cache.path().join("deadbeefdeadbeef");
    fs::create_dir_all(&corrupt_subdir).expect("corrupt subdir");
    fs::write(corrupt_subdir.join("codestory.db"), b"not sqlite").expect("corrupt hashed db");

    let mut command = test_support::cli_command();
    command.env("CODESTORY_EMBED_ALLOW_CPU", "1");
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
mod test_support;
