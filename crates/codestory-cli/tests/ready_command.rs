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
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .is_some_and(|text| text.contains("ready --goal agent --repair")))),
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
