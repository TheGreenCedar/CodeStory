use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn ready_command_emits_compact_verdicts_and_filters_goal() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    run_cli(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    let json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["ready", "--format", "json"],
    );
    let json: Value = serde_json::from_str(&json_text).expect("ready json");
    let verdicts = json["verdicts"]
        .as_array()
        .expect("ready verdicts should be an array");
    assert_eq!(verdicts.len(), 2);
    assert_eq!(verdicts[0]["goal"], "local_navigation");
    assert!(
        verdicts[0]["minimum_next"][0]
            .as_str()
            .expect("minimum next command")
            .contains("codestory-cli")
    );

    let command_text = json_text.replace("\\\\", "\\");
    assert!(
        !command_text.contains("\\\\?\\") && !command_text.contains("//?/"),
        "ready commands should use normalized human paths: {json_text}"
    );

    let local_json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["ready", "--goal", "local", "--format", "json"],
    );
    let local_json: Value = serde_json::from_str(&local_json_text).expect("ready local json");
    let local_verdicts = local_json["verdicts"]
        .as_array()
        .expect("local ready verdicts");
    assert_eq!(local_verdicts.len(), 1);
    assert_eq!(local_verdicts[0]["goal"], "local_navigation");
    assert!(
        local_json["local_refresh"].is_null(),
        "plain ready should not report a wait-fresh action: {local_json_text}"
    );

    let wait_json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "ready",
            "--goal",
            "local",
            "--wait-fresh",
            "--format",
            "json",
        ],
    );
    let wait_json: Value = serde_json::from_str(&wait_json_text).expect("ready wait json");
    assert_eq!(wait_json["verdicts"][0]["status"], "ready");
    assert_eq!(wait_json["local_refresh"]["state"], "refreshed");
    assert_eq!(wait_json["local_refresh"]["reason"], "already_fresh");
    assert_eq!(
        wait_json["verdicts"][0]["sidecar"]["degraded_reason"], "retrieval_manifest_missing",
        "local wait-fresh must not bootstrap retrieval sidecars: {wait_json_text}"
    );

    let markdown = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["ready", "--goal", "agent", "--format", "markdown"],
    );
    assert!(markdown.contains("# Readiness"));
    assert!(markdown.contains("agent_packet_search"));
    assert!(markdown.contains("minimum_next:"));
    assert!(markdown.contains("full_repair:"));
    assert!(markdown.contains("codestory-cli ready --goal agent --repair --project"));
    assert!(markdown.contains("--run-id"));
    assert!(markdown.contains("shared-agent"));
    assert!(markdown.contains("codestory-cli retrieval status --project"));
    assert!(markdown.contains("codestory-cli doctor --project"));
    assert!(markdown.contains("--format markdown"));
    assert!(
        !markdown.contains("codestory-cli index --project"),
        "fresh-index agent readiness should not recommend a full core reindex: {markdown}"
    );

    let agent_json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["ready", "--goal", "agent", "--format", "json"],
    );
    let agent_json: Value = serde_json::from_str(&agent_json_text).expect("ready agent json");
    let agent = &agent_json["verdicts"][0];
    assert_eq!(agent["status"], "repair_retrieval");
    assert_eq!(
        agent["sidecar"]["degraded_reason"],
        "retrieval_manifest_missing"
    );
    assert!(
        agent["minimum_next"]
            .as_array()
            .is_some_and(
                |commands| commands.iter().any(|command| command
                    .as_str()
                    .is_some_and(|text| text.contains("ready --goal agent --repair")
                        && text.contains("--run-id")
                        && text.contains("shared-agent")))
            ),
        "missing sidecars should point at the agent-owned repair command: {agent_json_text}"
    );
    assert!(
        agent["full_repair"]
            .as_array()
            .is_some_and(
                |commands| commands
                    .iter()
                    .any(|command| command
                        .as_str()
                        .is_some_and(|text| text.contains("retrieval status")
                            && text.contains("--format json")))
            ),
        "missing sidecars should finish with status proof guidance: {agent_json_text}"
    );
    assert_eq!(
        agent_json["readiness_lanes"]["local_default"]["profile"],
        "local"
    );
    assert_eq!(
        agent_json["readiness_lanes"]["agent_packet_search"]["profile"],
        "agent"
    );
    assert_eq!(
        agent_json["readiness_lanes"]["agent_packet_search"]["run_id"],
        "shared-agent"
    );

    let explicit_agent_json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "ready",
            "--goal",
            "agent",
            "--run-id",
            "isolated-proof",
            "--format",
            "json",
        ],
    );
    let explicit_agent_json: Value =
        serde_json::from_str(&explicit_agent_json_text).expect("explicit ready agent json");
    assert_eq!(
        explicit_agent_json["verdicts"][0]["sidecar"]["run_id"],
        "isolated-proof"
    );
    assert_eq!(
        explicit_agent_json["readiness_lanes"]["agent_packet_search"]["run_id"],
        "isolated-proof"
    );
    assert_eq!(
        explicit_agent_json["verdicts"][0]["sidecar"]["run_id"],
        explicit_agent_json["readiness_lanes"]["agent_packet_search"]["run_id"],
        "explicit ready agent verdict and rendered lane should use one selected runtime status: {explicit_agent_json_text}"
    );
    assert_eq!(
        explicit_agent_json["verdicts"][0]["sidecar"]["retrieval_mode"],
        explicit_agent_json["readiness_lanes"]["agent_packet_search"]["sidecar_mode"],
        "explicit ready agent verdict and rendered lane should agree on full-vs-degraded state: {explicit_agent_json_text}"
    );
    assert!(
        explicit_agent_json["verdicts"][0]["minimum_next"]
            .as_array()
            .is_some_and(
                |commands| commands.iter().any(|command| command
                    .as_str()
                    .is_some_and(|text| text.contains("ready --goal agent --repair")
                        && text.contains("--run-id")
                        && text.contains("isolated-proof")))
            ),
        "explicit run id should stay in agent repair guidance: {explicit_agent_json_text}"
    );
    assert!(
        explicit_agent_json["verdicts"][0]["full_repair"]
            .as_array()
            .is_some_and(
                |commands| commands
                    .iter()
                    .any(|command| command.as_str().is_some_and(|text| text
                        .contains("retrieval status")
                        && text.contains("--profile agent")
                        && text.contains("--run-id")
                        && text.contains("isolated-proof")))
            ),
        "explicit run id should stay in agent status proof guidance: {explicit_agent_json_text}"
    );
}

#[test]
fn ready_repair_indexes_fresh_workspace_for_local_navigation() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["ready", "--goal", "local", "--repair", "--format", "json"],
    );
    let json: Value = serde_json::from_str(&json_text).expect("ready repair json");
    let verdicts = json["verdicts"]
        .as_array()
        .expect("ready repair verdicts should be an array");

    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0]["goal"], "local_navigation");
    assert_eq!(verdicts[0]["status"], "ready");
    assert!(
        verdicts[0]["minimum_next"][0]
            .as_str()
            .expect("minimum next command")
            .contains("codestory-cli ground --project"),
        "repaired local readiness should point at grounding, not another index repair: {json_text}"
    );
}

#[test]
fn ready_wait_fresh_refreshes_stale_local_graph_once() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    run_cli(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    fs::write(
        workspace.path().join("src").join("lib.rs"),
        r#"pub fn entry_point() -> String {
    helper("ready")
}

pub fn added_after_index() -> String {
    helper("fresh")
}

fn helper(value: &str) -> String {
    format!("ready:{value}")
}
"#,
    )
    .expect("make index stale");

    let stale_json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["ready", "--goal", "local", "--format", "json"],
    );
    let stale_json: Value = serde_json::from_str(&stale_json_text).expect("stale ready json");
    assert_eq!(stale_json["verdicts"][0]["status"], "repair_index");
    assert_eq!(stale_json["verdicts"][0]["index"]["status"], "stale");

    let wait_json_text = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "ready",
            "--goal",
            "local",
            "--wait-fresh",
            "--format",
            "json",
        ],
    );
    let wait_json: Value = serde_json::from_str(&wait_json_text).expect("wait fresh json");
    assert_eq!(wait_json["verdicts"][0]["status"], "ready");
    assert_eq!(wait_json["verdicts"][0]["index"]["status"], "fresh");
    assert_eq!(wait_json["local_refresh"]["state"], "refreshed");
    assert_eq!(wait_json["local_refresh"]["reason"], "refreshed");
    assert_eq!(
        wait_json["verdicts"][0]["sidecar"]["retrieval_mode"],
        "unavailable"
    );
    assert_eq!(
        wait_json["verdicts"][0]["sidecar"]["degraded_reason"], "retrieval_manifest_missing",
        "wait-fresh should leave packet/search sidecars separately gated: {wait_json_text}"
    );
}

fn run_cli(workspace: &Path, cache_dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
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
name = "ready-command-fixture"
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
    helper("ready")
}

fn helper(value: &str) -> String {
    format!("ready:{value}")
}
"#,
    )
    .expect("write lib.rs");
}
