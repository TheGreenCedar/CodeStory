use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};
use tempfile::tempdir;

fn write_tiny_rust_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "tiny-browser-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");

    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"pub mod runtime;

pub struct AppController {
    project_name: String,
}

impl AppController {
    pub fn new(project_name: impl Into<String>) -> Self {
        Self {
            project_name: project_name.into(),
        }
    }

    pub fn open_project(&self) -> String {
        open_project(&self.project_name)
    }

    pub fn run_indexing(&self) -> usize {
        run_indexing(&self.open_project())
    }
}

pub fn open_project(project_name: &str) -> String {
    runtime::normalize_project(project_name)
}

pub fn run_indexing(project_path: &str) -> usize {
    runtime::schedule_index(project_path)
}
"#,
    )
    .expect("write lib.rs");
    fs::write(
        src.join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("workspace:{project_name}")
}

pub fn schedule_index(project_path: &str) -> usize {
    super::open_project(project_path).len()
}
"#,
    )
    .expect("write runtime.rs");
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

fn run_cli_with_stdin(
    workspace: &Path,
    cache_dir: &Path,
    args: &[&str],
    stdin: &str,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn codestory-cli");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait codestory-cli")
}

fn run_cli_with_embedding_env(
    workspace: &Path,
    cache_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env_remove("CODESTORY_EMBED_RUNTIME_MODE")
        .env_remove("CODESTORY_EMBED_BACKEND")
        .env_remove("CODESTORY_EMBED_PROFILE")
        .env_remove("CODESTORY_EMBED_MODEL_ID")
        .env_remove("CODESTORY_EMBED_TRUNCATE_DIM")
        .env_remove("CODESTORY_EMBED_EXPECTED_DIM")
        .env_remove("CODESTORY_EMBED_LLAMACPP_URL");
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("run codestory-cli")
}

fn run_cli_json(workspace: &Path, cache_dir: &Path, args: &[&str]) -> Value {
    let output = run_cli(workspace, cache_dir, args);
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse json output")
}

fn run_cli_json_with_embedding_env(
    workspace: &Path,
    cache_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Value {
    let output = run_cli_with_embedding_env(workspace, cache_dir, args, envs);
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse json output")
}

fn check_with_name<'a>(doctor: &'a Value, name: &str) -> &'a Value {
    doctor["checks"]
        .as_array()
        .expect("doctor checks")
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("doctor check `{name}` missing: {doctor:#}"))
}

fn doctor_next_commands(doctor: &Value) -> Vec<&str> {
    doctor["next_commands"]
        .as_array()
        .expect("doctor next commands")
        .iter()
        .map(|command| command.as_str().expect("next command string"))
        .collect()
}

fn assert_no_agent_proof_commands(commands: &[&str], context: &str) {
    let joined = commands.join("\n");
    for blocked in ["packet", "ground", "search", "context"] {
        assert!(
            !joined.contains(&format!("codestory-cli {blocked} ")),
            "{context} should stop before `{blocked}` proof/navigation commands: {joined}"
        );
    }
}

fn clean_test_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn install_fake_managed_embeddings(cache_dir: &Path) {
    let root = cache_dir.join("managed-embeddings");
    let model_path = root
        .join("models")
        .join("bge-base-en-v1.5-onnx-qdrant")
        .join("model_optimized.onnx");
    let runtime_model_path = root
        .join("models")
        .join("bge-base-en-v1.5-onnx-qdrant")
        .join("model_optimized_cls_pool.onnx");
    let tokenizer_path = root
        .join("models")
        .join("bge-base-en-v1.5-onnx-qdrant")
        .join("tokenizer.json");
    fs::create_dir_all(model_path.parent().expect("model parent")).expect("create model dir");
    fs::write(&model_path, b"fake model").expect("write fake model");
    fs::write(&runtime_model_path, b"fake pooled model").expect("write fake pooled model");
    fs::write(&tokenizer_path, b"fake tokenizer").expect("write fake tokenizer");
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "backend": "onnx",
            "provider": "directml",
            "onnx_model_asset": "model_optimized.onnx",
            "onnx_model_path": clean_test_path(&runtime_model_path),
            "onnx_source_model_path": clean_test_path(&model_path),
            "onnx_pooled_output": "sentence_embedding",
            "onnx_tokenizer_asset": "tokenizer.json",
            "onnx_tokenizer_path": clean_test_path(&tokenizer_path),
        }))
        .expect("manifest json"),
    )
    .expect("write fake managed manifest");
}

fn run_stdio_request(workspace: &Path, cache_dir: &Path, request: &str) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("serve")
        .arg("--stdio")
        .arg("--refresh")
        .arg("none")
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdio server");

    {
        let stdin = child.stdin.as_mut().expect("stdio stdin");
        stdin
            .write_all(request.as_bytes())
            .expect("write stdio request");
        stdin.write_all(b"\n").expect("finish stdio request");
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for stdio server");
    assert!(
        output.status.success(),
        "stdio server failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("stdio server should write one response, got {stdout:?}"));
    serde_json::from_str(line).expect("parse stdio response")
}

fn string_field<'a>(value: &'a Value, path: &[&str]) -> &'a str {
    value_at_path(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("expected string at path {path:?}"))
}

fn array_is_non_empty(value: &Value, path: &[&str]) -> bool {
    value_at_path(value, path)
        .as_array()
        .is_some_and(|items| !items.is_empty())
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut current = value;
    for key in path {
        current = value_at_path_key(current, key, path);
    }
    current
}

fn value_at_path_key<'a>(value: &'a Value, key: &str, path: &[&str]) -> &'a Value {
    match value {
        Value::Object(fields) => fields
            .get(key)
            .unwrap_or_else(|| panic!("missing key {key:?} at path {path:?}")),
        Value::Array(items) => {
            let index = key
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("expected array index at path {path:?}"));
            items
                .get(index)
                .unwrap_or_else(|| panic!("missing index {index} at path {path:?}"))
        }
        _ => panic!("cannot descend into non-container at path {path:?}, key {key:?}"),
    }
}

fn search_dir_for_storage(storage_path: &Path) -> PathBuf {
    let parent = storage_path.parent().expect("storage parent");
    let stem = storage_path
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("storage file stem");
    parent.join(format!("{stem}.search"))
}

fn find_index_freshness(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(map) => {
            for key in ["index_freshness", "freshness"] {
                if let Some(candidate) = map.get(key)
                    && (freshness_count(
                        candidate,
                        &["changed_file_count", "changed_count", "changed"],
                    )
                    .is_some()
                        || candidate.get("not_checked_reason").is_some()
                        || candidate.get("not_checked").is_some())
                {
                    return Some(candidate);
                }
            }
            map.values().find_map(find_index_freshness)
        }
        Value::Array(items) => items.iter().find_map(find_index_freshness),
        _ => None,
    }
}

fn freshness_count(value: &Value, aliases: &[&str]) -> Option<u64> {
    aliases
        .iter()
        .find_map(|alias| value.get(*alias).and_then(Value::as_u64))
}

fn assert_stale_freshness_counts(value: &Value, context: &str) {
    let freshness = find_index_freshness(value)
        .unwrap_or_else(|| panic!("{context} should include an index freshness signal: {value:#}"));
    assert_eq!(
        freshness_count(
            freshness,
            &["changed_file_count", "changed_count", "changed"]
        ),
        Some(1),
        "{context} freshness should report one changed file: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["new_file_count", "new_count", "new", "added_count", "added"]
        ),
        Some(1),
        "{context} freshness should report one new file: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &[
                "removed_file_count",
                "removed_count",
                "removed",
                "deleted_count",
                "deleted"
            ]
        ),
        Some(1),
        "{context} freshness should report one removed file: {freshness:#}"
    );
}

fn write_openapi_workspace(root: &Path) {
    fs::write(
        root.join("openapi.json"),
        r#"{
  "openapi": "3.1.0",
  "paths": {
    "/api/users": {
      "get": {
        "operationId": "listUsers"
      }
    }
  }
}"#,
    )
    .expect("write openapi fixture");
}

fn write_investigation_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "investigation-browser-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");

    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"pub mod evidence;
pub mod router;

pub fn run_investigation(payload: &str) -> &'static str {
    let event = evidence::parse_investigation_event(payload);
    router::route_investigation_event(event)
}
"#,
    )
    .expect("write lib.rs");
    fs::write(
        src.join("evidence.rs"),
        r#"pub const INVESTIGATION_LITERAL: &str = "CODESTORY_INVESTIGATION_LITERAL";

pub fn parse_investigation_event(payload: &str) -> &'static str {
    let _marker = INVESTIGATION_LITERAL;
    if payload.is_empty() {
        "empty"
    } else {
        "routed"
    }
}
"#,
    )
    .expect("write evidence.rs");
    fs::write(
        src.join("router.rs"),
        r#"pub fn route_investigation_event(event: &str) -> &'static str {
    match event {
        "routed" => "source-read-with-citation",
        _ => "gap-recorded",
    }
}
"#,
    )
    .expect("write router.rs");
}

#[test]
fn read_commands_report_changed_openapi_index_freshness() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_openapi_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("openapi.json"),
        r#"{
  "openapi": "3.1.0",
  "paths": {
    "/api/users": {
      "post": {
        "operationId": "createUser"
      }
    }
  }
}"#,
    )
    .expect("modify indexed openapi fixture");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "stale",
        "changed indexed OpenAPI schema should make freshness stale: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["changed_file_count", "changed_count", "changed"]
        ),
        Some(1),
        "changed indexed OpenAPI schema should be counted as changed: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["new_file_count", "new_count", "new", "added_count", "added"]
        ),
        Some(0),
        "modified OpenAPI schema should not be counted as new: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &[
                "removed_file_count",
                "removed_count",
                "removed",
                "deleted_count",
                "deleted"
            ]
        ),
        Some(0),
        "modified OpenAPI schema should not be counted as removed: {freshness:#}"
    );
}

#[test]
fn non_openapi_json_does_not_keep_freshness_stale() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    fs::write(
        workspace.path().join("skills-lock.json"),
        r#"{"skills":[]}"#,
    )
    .expect("write non-openapi json");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "fresh",
        "non-OpenAPI JSON that indexes to no symbols should not keep freshness stale: {freshness:#}"
    );
}

#[test]
fn new_openapi_with_late_paths_marks_freshness_stale() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    let filler = "x".repeat(70 * 1024);
    fs::write(
        workspace.path().join("late-openapi.yaml"),
        format!(
            "openapi: 3.1.0\ninfo:\n  title: Late Paths\n  version: 1.0.0\ncomponents:\n  schemas:\n    Filler:\n      description: \"{filler}\"\npaths:\n  /api/late:\n    get:\n      operationId: getLate\n"
        ),
    )
    .expect("write late OpenAPI fixture");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "stale",
        "new OpenAPI schema with late paths should make freshness stale: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["new_file_count", "new_count", "new", "added_count", "added"]
        ),
        Some(1),
        "new OpenAPI schema with late paths should be counted as new: {freshness:#}"
    );
}

#[test]
fn doctor_next_commands_stop_at_index_repair_when_inventory_is_stale() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    fs::write(
        workspace.path().join("late-openapi.yaml"),
        "openapi: 3.1.0\ninfo:\n  title: Late Paths\n  version: 1.0.0\npaths:\n  /api/late:\n    get:\n      operationId: getLate\n",
    )
    .expect("write late OpenAPI fixture");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "stale",
        "doctor should report stale freshness before recommending repair: {doctor:#}"
    );

    let next_commands = doctor_next_commands(&doctor);
    let joined = next_commands.join("\n");
    assert!(
        joined.contains("codestory-cli index")
            && joined.contains("--refresh incremental")
            && joined.contains("codestory-cli doctor"),
        "stale doctor should recommend index repair then doctor recheck: {doctor:#}"
    );
    assert!(
        !joined.contains("retrieval status") && !joined.contains("retrieval index"),
        "stale doctor should stop before retrieval repair commands: {doctor:#}"
    );
    assert_no_agent_proof_commands(&next_commands, "stale doctor");

    let markdown = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "markdown"],
    );
    assert!(
        markdown.status.success(),
        "doctor markdown failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&markdown.stdout),
        String::from_utf8_lossy(&markdown.stderr)
    );
    let markdown = String::from_utf8_lossy(&markdown.stdout);
    assert!(
        markdown
            .contains("readiness: local_navigation=repair_index agent_packet_search=repair_index"),
        "doctor markdown should show split readiness with index repair first:\n{markdown}"
    );
}

#[test]
fn doctor_next_commands_stop_at_retrieval_repair_when_sidecar_is_not_full() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let index = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    let index_next_commands = doctor_next_commands(&index);
    let index_next_commands_joined = index_next_commands.join("\n");
    assert!(
        index_next_commands_joined.contains("codestory-cli retrieval status")
            && index_next_commands_joined.contains("codestory-cli retrieval index"),
        "index output should recommend sidecar repair when sidecar retrieval is not full: {index:#}"
    );
    assert_no_agent_proof_commands(&index_next_commands, "index output");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_ne!(
        freshness["status"], "stale",
        "sidecar repair test needs fresh enough inventory: {doctor:#}"
    );
    assert_ne!(
        doctor["retrieval_mode"], "full",
        "sidecar repair test needs mandatory sidecar retrieval to be not full: {doctor:#}"
    );

    let next_commands = doctor_next_commands(&doctor);
    let joined = next_commands.join("\n");
    assert!(
        joined.contains("codestory-cli retrieval status")
            && joined.contains("codestory-cli retrieval index")
            && joined.contains("--refresh full")
            && joined.contains("codestory-cli doctor"),
        "doctor should recommend retrieval repair before packet/search: {doctor:#}"
    );
    assert!(
        !joined.contains("codestory-cli index "),
        "fresh doctor should not send users back to index repair: {doctor:#}"
    );
    assert_no_agent_proof_commands(&next_commands, "sidecar doctor");

    let markdown = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "markdown"],
    );
    assert!(
        markdown.status.success(),
        "doctor markdown failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&markdown.stdout),
        String::from_utf8_lossy(&markdown.stderr)
    );
    let markdown = String::from_utf8_lossy(&markdown.stdout);
    assert!(
        markdown.contains("readiness: local_navigation=ready agent_packet_search=repair_retrieval"),
        "doctor markdown should show split readiness with retrieval repair for agent use:\n{markdown}"
    );
}

#[test]
fn doctor_reports_current_and_stored_semantic_doc_embedding_contract() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let index = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
        &[
            ("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
            ("CODESTORY_EMBED_PROFILE", "bge-small-en-v1.5"),
        ],
    );
    assert!(
        index["summary"]["retrieval"]["semantic_doc_count"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "hash-mode index should persist semantic docs for doctor to report"
    );

    let doctor = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
        &[
            ("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
            ("CODESTORY_EMBED_PROFILE", "bge-small-en-v1.5"),
        ],
    );
    let current = &doctor["retrieval"]["current_embedding"];
    for field in ["profile", "model_id", "backend", "dimension", "doc_shape"] {
        assert!(
            current.get(field).is_some(),
            "doctor should report current embedding `{field}` metadata: {doctor:#}"
        );
    }

    let stored = &doctor["retrieval"]["stored_embedding"];
    for field in ["doc_count", "cache_key", "dimension", "doc_shape"] {
        assert!(
            stored.get(field).is_some(),
            "doctor should report stored semantic-doc `{field}` metadata: {doctor:#}"
        );
    }

    assert_ne!(
        doctor["retrieval_mode"], "full",
        "legacy hash semantic readiness must not make mandatory sidecar retrieval look full: {doctor:#}"
    );
    assert!(
        doctor["sidecar_retrieval"]["retrieval_mode"].is_string(),
        "doctor should expose first-class sidecar retrieval mode: {doctor:#}"
    );
    let sidecar_check = check_with_name(&doctor, "sidecar_retrieval");
    assert_eq!(
        sidecar_check["status"], "warn",
        "doctor should warn when mandatory sidecar retrieval is unavailable despite legacy semantic docs: {doctor:#}"
    );
    let next_commands = doctor["next_commands"]
        .as_array()
        .expect("doctor next commands");
    let first_next_commands = next_commands
        .iter()
        .take(3)
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        first_next_commands.contains("retrieval status")
            && first_next_commands.contains("retrieval index")
            && !first_next_commands.contains("packet"),
        "doctor should recommend sidecar status/index repair before packet/search when mode is not full: {doctor:#}"
    );
}

#[test]
fn doctor_warns_when_stored_semantic_doc_profile_differs_from_current_config() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
        &[
            ("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
            ("CODESTORY_EMBED_PROFILE", "bge-small-en-v1.5"),
        ],
    );

    let doctor = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
        &[
            ("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
            ("CODESTORY_EMBED_PROFILE", "bge-base-en-v1.5"),
        ],
    );
    let semantic_contract = check_with_name(&doctor, "semantic_contract");
    assert_eq!(
        semantic_contract["status"], "warn",
        "doctor should warn when stored semantic-doc metadata mismatches current embedding config: {doctor:#}"
    );
    let message = semantic_contract["message"]
        .as_str()
        .expect("semantic contract message");
    assert!(
        message.contains("bge-small-en-v1.5") && message.contains("bge-base-en-v1.5"),
        "mismatch warning should name stored and current profiles: {message}"
    );
}

#[test]
fn doctor_keeps_missing_llamacpp_endpoint_explicit() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let doctor = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
        &[
            ("CODESTORY_EMBED_BACKEND", "llamacpp"),
            (
                "CODESTORY_EMBED_LLAMACPP_URL",
                "http://127.0.0.1:9/v1/embeddings",
            ),
        ],
    );

    assert_eq!(
        doctor["retrieval"]["fallback_reason"], "missing_embedding_runtime",
        "missing llama.cpp endpoint should stay a typed retrieval fallback: {doctor:#}"
    );
    let fallback_message = doctor["retrieval"]["fallback_message"]
        .as_str()
        .expect("fallback message");
    assert!(
        fallback_message.contains("llama.cpp")
            && fallback_message.contains("127.0.0.1:9/v1/embeddings"),
        "missing endpoint should name llama.cpp and the configured URL: {fallback_message}"
    );
}

#[test]
fn doctor_redacts_sensitive_llamacpp_url_output() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let doctor = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
        &[
            ("CODESTORY_EMBED_BACKEND", "llamacpp"),
            (
                "CODESTORY_EMBED_LLAMACPP_URL",
                "https://user:secret@example.test/v1/embeddings?token=abc#frag",
            ),
        ],
    );

    let serialized = doctor.to_string();
    assert!(
        serialized.contains("https://example.test/v1/embeddings"),
        "doctor should retain scheme/host/path for configured endpoints: {doctor:#}"
    );
    for secret_fragment in ["user:secret", "token=abc", "#frag"] {
        assert!(
            !serialized.contains(secret_fragment),
            "doctor output should redact sensitive URL fragments `{secret_fragment}`: {doctor:#}"
        );
    }

    let managed = check_with_name(&doctor, "managed_embeddings");
    assert!(
        managed["message"]
            .as_str()
            .is_some_and(|message| message.contains("https://example.test/v1/embeddings")),
        "managed embedding status should use redacted endpoint display: {managed:#}"
    );
    let env_row = doctor["environment"]
        .as_array()
        .expect("doctor environment")
        .iter()
        .find(|row| row["name"] == "CODESTORY_EMBED_LLAMACPP_URL")
        .unwrap_or_else(|| panic!("missing URL env row: {doctor:#}"));
    assert_eq!(
        env_row["message"], "set to `https://example.test/v1/embeddings`",
        "doctor URL env row should redact userinfo, query, and fragment: {env_row:#}"
    );
}

#[test]
fn setup_embeddings_dry_run_reports_pinned_managed_assets() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let setup = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["setup", "embeddings", "--dry-run", "--format", "json"],
        &[],
    );

    assert_eq!(
        setup["dry_run"], true,
        "setup should report dry-run mode: {setup:#}"
    );
    assert_eq!(
        setup["backend"], "onnx",
        "setup should select ONNX: {setup:#}"
    );
    assert!(
        setup["model"]["url"]
            .as_str()
            .is_some_and(|value| value.contains("Qdrant/bge-base-en-v1.5-onnx-Q")),
        "setup should pin the managed BGE-base ONNX model: {setup:#}"
    );
    assert_eq!(
        setup["model"]["name"], "model_optimized.onnx",
        "setup should report the pinned optimized ONNX graph: {setup:#}"
    );
    assert_eq!(
        setup["runtime_model"]["name"], "model_optimized_cls_pool.onnx",
        "setup should report the derived pooled ONNX runtime graph: {setup:#}"
    );
    assert!(
        setup["tokenizer"]["url"]
            .as_str()
            .is_some_and(|value| value.contains("Qdrant/bge-base-en-v1.5-onnx-Q")),
        "setup should pin the matching tokenizer: {setup:#}"
    );
    assert!(
        !cache_dir
            .path()
            .join("managed-embeddings")
            .join("downloads")
            .exists(),
        "dry-run setup must not create download artifacts"
    );
    let next_commands = setup["next_commands"]
        .as_array()
        .expect("setup next commands")
        .iter()
        .map(|value| value.as_str().expect("next command"))
        .collect::<Vec<_>>();
    assert_eq!(
        next_commands,
        vec![
            format!(
                "codestory-cli doctor --project \"{}\" --cache-dir \"{}\"",
                clean_test_path(workspace.path()),
                clean_test_path(cache_dir.path())
            ),
            format!(
                "codestory-cli index --project \"{}\" --cache-dir \"{}\" --refresh full",
                clean_test_path(workspace.path()),
                clean_test_path(cache_dir.path())
            ),
        ],
        "setup follow-up commands should preserve project and cache args: {setup:#}"
    );
}

#[test]
fn doctor_reports_managed_embedding_setup_state() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let doctor = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
        &[],
    );

    let managed = check_with_name(&doctor, "managed_embeddings");
    let status = managed["status"].as_str().expect("managed status");
    let message = managed["message"].as_str().expect("managed message");
    match status {
        "info" => assert!(
            message.contains("setup embeddings"),
            "doctor should name the setup command when managed assets are missing: {doctor:#}"
        ),
        "ok" => assert!(
            message.contains("Managed ONNX embeddings are installed"),
            "doctor should report globally available managed assets for isolated cache dirs: {doctor:#}"
        ),
        _ => panic!("managed setup state should be informational or installed: {doctor:#}"),
    }
}

#[test]
fn doctor_does_not_autostart_installed_managed_embeddings() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    install_fake_managed_embeddings(cache_dir.path());

    let doctor = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
        &[],
    );

    let managed = check_with_name(&doctor, "managed_embeddings");
    assert_eq!(
        managed["status"], "warn",
        "installed but invalid managed assets should fail runtime verification: {doctor:#}"
    );
    assert!(
        managed["message"]
            .as_str()
            .is_some_and(|message| message.contains("failed runtime verification")),
        "managed status should explain that installed assets did not pass runtime verification: {doctor:#}"
    );
    assert!(
        !cache_dir
            .path()
            .join("managed-embeddings")
            .join("logs")
            .exists(),
        "doctor should not create managed embedding logs or start a server: {doctor:#}"
    );
}

#[test]
fn index_dry_run_does_not_autostart_installed_managed_embeddings() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    install_fake_managed_embeddings(cache_dir.path());

    let dry_run = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["index", "--dry-run", "--format", "json"],
        &[],
    );

    assert_eq!(dry_run["dry_run"]["files_to_index"], 3);
    assert!(
        !cache_dir
            .path()
            .join("managed-embeddings")
            .join("logs")
            .exists(),
        "index --dry-run should not create managed embedding logs or start a server: {dry_run:#}"
    );
}

#[test]
fn tiny_workspace_browser_loop_works_from_existing_cache() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    let tests = workspace.path().join("tests");
    fs::create_dir_all(&tests).expect("create tests dir");
    fs::write(
        tests.join("app_controller_test.rs"),
        r#"use tiny_browser_fixture::AppController;

#[test]
fn app_controller_opens_project() {
    let controller = AppController::new("demo");
    assert!(controller.open_project().contains("demo"));
}
"#,
    )
    .expect("write integration test");

    let search_dir_snapshot =
        index_tiny_workspace_for_browser_loop(workspace.path(), cache_dir.path());

    assert_doctor_reports_existing_cache_health(workspace.path(), cache_dir.path());

    assert_ground_reads_existing_cache(workspace.path(), cache_dir.path());

    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "AppController",
        None,
    );
    assert_product_search_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        "AppController",
    );

    assert_symbol_reads_focus_node(workspace.path(), cache_dir.path(), &node_id);

    assert_trail_reads_focus_node(workspace.path(), cache_dir.path(), &node_id);

    assert_snippet_reads_focus_node(workspace.path(), cache_dir.path(), &node_id);

    let bookmark_id = add_and_assert_bookmark_focus(workspace.path(), cache_dir.path(), &node_id);

    assert_context_bookmark_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        &bookmark_id,
    );

    assert_explore_outputs_focus_context(workspace.path(), cache_dir.path(), &node_id);

    assert_files_and_affected_read_existing_cache(workspace.path(), cache_dir.path());

    assert_query_search_fails_closed_without_full_sidecars(workspace.path(), cache_dir.path());
    assert_packet_builds_broad_task_contract(workspace.path(), cache_dir.path());
    assert_context_id_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        &node_id,
    );
    remove_and_assert_bookmark_gone(workspace.path(), cache_dir.path(), &bookmark_id);
    assert_stdio_context_id_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        &node_id,
    );

    search_dir_snapshot.assert_unchanged();
}

struct SearchDirSnapshot {
    path: PathBuf,
    modified: SystemTime,
}

fn index_tiny_workspace_for_browser_loop(workspace: &Path, cache_dir: &Path) -> SearchDirSnapshot {
    let index = run_cli_json(
        workspace,
        cache_dir,
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index["summary"]["stats"]["node_count"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "index should discover symbols in the tiny workspace"
    );
    let storage_path = PathBuf::from(string_field(&index, &["storage_path"]));
    SearchDirSnapshot::capture(search_dir_for_storage(&storage_path))
}

fn assert_doctor_reports_existing_cache_health(workspace: &Path, cache_dir: &Path) {
    let doctor = run_cli_json(workspace, cache_dir, &["doctor", "--format", "json"]);
    assert!(
        doctor["checks"]
            .as_array()
            .is_some_and(|checks| !checks.is_empty()),
        "doctor should emit checks"
    );
    assert!(
        doctor["environment"]
            .as_array()
            .expect("doctor environment")
            .iter()
            .any(|row| row["name"] == "CODESTORY_EMBED_MODEL_ID"),
        "doctor should expose the embedding model-id env var documented by .codestory.toml"
    );
}

fn assert_ground_reads_existing_cache(workspace: &Path, cache_dir: &Path) {
    let ground = run_cli_json(
        workspace,
        cache_dir,
        &["ground", "--refresh", "none", "--format", "json"],
    );
    assert!(
        array_is_non_empty(&ground, &["root_symbols"]) || array_is_non_empty(&ground, &["files"]),
        "ground should return project grounding data"
    );
}

fn ground_symbol_node_id_from_existing_cache(
    workspace: &Path,
    cache_dir: &Path,
    symbol: &str,
    file_path_suffix: Option<&str>,
) -> String {
    let ground = run_cli_json(
        workspace,
        cache_dir,
        &["ground", "--refresh", "none", "--format", "json"],
    );
    let mut symbols = Vec::new();
    if let Some(root_symbols) = ground["root_symbols"].as_array() {
        symbols.extend(root_symbols.iter());
    }
    if let Some(files) = ground["files"].as_array() {
        for file in files {
            if let Some(file_symbols) = file["symbols"].as_array() {
                symbols.extend(file_symbols.iter());
            }
        }
    }
    let label_prefix = format!("{symbol} @ ");
    let hit = symbols
        .into_iter()
        .find(|item| {
            let Some(label) = item["label"].as_str() else {
                return false;
            };
            label.starts_with(&label_prefix)
                && file_path_suffix.is_none_or(|suffix| label.contains(&suffix.replace('\\', "/")))
        })
        .unwrap_or_else(|| panic!("ground should find {symbol} in the existing cache: {ground:#}"));
    string_field(hit, &["id"]).to_string()
}

fn assert_product_search_fails_closed_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    query: &str,
) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "search",
            "--query",
            query,
            "--repo-text",
            "off",
            "--limit",
            "5",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "product search should fail closed without full sidecars"
    );
    assert_sidecar_failure_output(output);
}

fn assert_sidecar_failure_output(output: std::process::Output) {
    let failure = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        failure.contains("sidecar retrieval")
            && (failure.contains("expected mode=full")
                || failure.contains("retrieval_manifest_missing")),
        "command should explain the mandatory sidecar gate, got: {failure}"
    );
}

fn assert_symbol_reads_focus_node(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let symbol = run_cli_json(
        workspace,
        cache_dir,
        &[
            "symbol",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        string_field(&symbol, &["symbol", "node", "display_name"]),
        "AppController"
    );
}

fn assert_trail_reads_focus_node(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let trail = run_cli_json(
        workspace,
        cache_dir,
        &[
            "trail",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        array_is_non_empty(&trail, &["trail", "trail", "nodes"]),
        "trail should return at least the focus node"
    );

    let trail_story = run_cli_json(
        workspace,
        cache_dir,
        &[
            "trail",
            &format!("--id={node_id}"),
            "--story",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        string_field(&trail_story, &["trail", "story", "summary"]).contains("Story trail"),
        "trail --story JSON should include a readable story summary"
    );
    assert!(
        trail_story["trail"]["story"]["entry_points"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "trail --story should describe entry points"
    );
    assert!(
        trail_story["trail"]["story"]["test_scope"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item
                .as_str()
                .is_some_and(|text| text.contains("tests and benches excluded")))),
        "trail --story should make test scope explicit"
    );
}

fn assert_snippet_reads_focus_node(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let snippet = run_cli_json(
        workspace,
        cache_dir,
        &[
            "snippet",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        string_field(&snippet, &["snippet", "snippet"]).contains("AppController"),
        "snippet should include source for AppController"
    );
}

fn add_and_assert_bookmark_focus(workspace: &Path, cache_dir: &Path, node_id: &str) -> String {
    let bookmark = run_cli_json(
        workspace,
        cache_dir,
        &[
            "bookmark",
            "add",
            &format!("--id={node_id}"),
            "--comment",
            "entry point under review",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    let bookmark_id = string_field(&bookmark, &["bookmark", "bookmark", "id"]).to_string();
    assert_eq!(
        string_field(&bookmark, &["bookmark", "bookmark", "node_id"]),
        node_id
    );
    assert_eq!(
        string_field(&bookmark, &["category", "name"]),
        "Investigation"
    );

    let bookmarks = run_cli_json(
        workspace,
        cache_dir,
        &["bookmark", "list", "--format", "json"],
    );
    assert!(
        bookmarks["bookmarks"]
            .as_array()
            .expect("bookmarks")
            .iter()
            .any(|bookmark| bookmark["bookmark"]["id"] == bookmark_id),
        "bookmark list should include the saved focus"
    );
    bookmark_id
}

fn assert_context_bookmark_fails_closed_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    bookmark_id: &str,
) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "context",
            "--bookmark",
            bookmark_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "context --bookmark should fail closed without full sidecars"
    );
    assert_sidecar_failure_output(output);
}

fn assert_explore_outputs_focus_context(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let explore = run_cli_json(
        workspace,
        cache_dir,
        &[
            "explore",
            &format!("--id={node_id}"),
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        string_field(&explore, &["profile", "requested"]),
        "default",
        "explore should preserve the default profile unless --profile is explicit"
    );
    assert_eq!(
        string_field(&explore, &["profile", "caller_scope"]),
        "production-only",
        "default explore should keep production-only caller scope"
    );
    assert_eq!(
        string_field(&explore, &["search", "selected", "display_name"]),
        "AppController"
    );
    assert_eq!(
        string_field(&explore, &["status", "refresh"]),
        "none",
        "explore should report the read-command refresh mode"
    );
    assert!(
        array_is_non_empty(&explore, &["status", "next_commands"]),
        "explore JSON should include target-aware next commands"
    );
    assert!(
        explore["status"]["layer_notes"]
            .as_array()
            .expect("layer notes")
            .iter()
            .any(|note| note
                .as_str()
                .is_some_and(|note| note.starts_with("query_resolution:"))),
        "explore status should preserve the query-resolution layer"
    );
    assert!(
        explore["status"]["layer_notes"]
            .as_array()
            .expect("layer notes")
            .iter()
            .any(|note| note
                .as_str()
                .is_some_and(|note| note.starts_with("snippet_context:"))),
        "explore status should preserve the snippet-context layer"
    );
    assert!(
        array_is_non_empty(&explore, &["trail", "trail", "nodes"]),
        "explore JSON should include trail detail"
    );
    assert!(
        string_field(&explore, &["relationship_evidence", "map_source"]).contains("trail_context"),
        "explore JSON should expose relationship evidence source"
    );
    assert!(
        string_field(&explore, &["snippet", "snippet"]).contains("AppController"),
        "explore JSON should include snippet detail"
    );
    assert!(
        array_is_non_empty(&explore, &["source_packet", "files"]),
        "explore JSON should include grouped source packet files"
    );
    assert!(
        array_is_non_empty(&explore, &["source_packet", "notes"]),
        "explore JSON should include source packet budget/coverage notes"
    );

    let profiled_explore = run_cli_json(
        workspace,
        cache_dir,
        &[
            "explore",
            &format!("--id={node_id}"),
            "--profile",
            "test-impact",
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        string_field(&profiled_explore, &["profile", "requested"]),
        "test-impact",
        "explicit explore profiles should be reflected in JSON output"
    );
    assert_eq!(
        string_field(&profiled_explore, &["profile", "caller_scope"]),
        "include-tests-and-benches",
        "test-impact profile should include test and bench neighbors"
    );
    assert!(
        profiled_explore["profile"]["depth"]
            .as_u64()
            .unwrap_or_default()
            >= 4,
        "test-impact profile should raise the default depth floor"
    );

    let explore_markdown = run_cli(
        workspace,
        cache_dir,
        &[
            "explore",
            &format!("--id={node_id}"),
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "markdown",
        ],
    );
    assert!(
        explore_markdown.status.success(),
        "explore markdown failed: {}",
        String::from_utf8_lossy(&explore_markdown.stderr)
    );
    let explore_markdown = String::from_utf8_lossy(&explore_markdown.stdout);
    for expected in [
        "# Explore",
        "status:",
        "profile:",
        "search:",
        "results:",
        "resolution:",
        "navigation:",
        "relationship evidence:",
        "symbol:",
        "trail:",
        "snippet:",
        "source packet:",
        "snippet_context:",
        "semantic_runtime:",
        "output_write:",
    ] {
        assert!(
            explore_markdown.contains(expected),
            "explore markdown should contain `{expected}`:\n{explore_markdown}"
        );
    }
}

fn assert_files_and_affected_read_existing_cache(workspace: &Path, cache_dir: &Path) {
    let files = run_cli_json(
        workspace,
        cache_dir,
        &[
            "files",
            "--role",
            "test",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        files["summary"]["language_counts"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["language"] == "rust"
                && item["support_mode"] == "parser_backed_graph"
                && item["evidence_tier"] == "graph_fidelity"
                && item["claim_label"] == "parser-backed graph, fidelity-gated")),
        "files JSON should include language counts with support tiers: {files:#}"
    );
    assert!(
        files["summary"]["framework_route_coverage"]
            .as_array()
            .is_some_and(
                |items| items.iter().any(|item| item["framework"] == "express"
                    && item["promotable"] == true
                    && item["fixture_status"].is_string()
                    && item["unsupported_patterns"].is_array())
                    && items.iter().any(|item| item["framework"] == "nextjs"
                        && item["confidence_floor"] == "file_convention"
                        && item["known_gaps"].is_array())
                    && items.iter().any(|item| item["framework"] == "gin"
                        && item["handler_link_support"] == "not_claimed_text_only")
            ),
        "files JSON should include framework route coverage matrix: {files:#}"
    );
    assert!(
        files["files"]
            .as_array()
            .expect("files")
            .iter()
            .any(|file| file["path"]
                .as_str()
                .is_some_and(|path| path.contains("app_controller_test.rs"))
                && file["role"] == "test"),
        "files --role test should list inferred test files: {files:#}"
    );

    let files_markdown = run_cli(
        workspace,
        cache_dir,
        &[
            "files",
            "--language",
            "rust",
            "--limit",
            "3",
            "--refresh",
            "none",
            "--format",
            "markdown",
        ],
    );
    assert!(
        files_markdown.status.success(),
        "files markdown failed: {}",
        String::from_utf8_lossy(&files_markdown.stderr)
    );
    let files_markdown = String::from_utf8_lossy(&files_markdown.stdout);
    assert!(
        files_markdown.contains("# indexed files")
            && files_markdown.contains("languages:")
            && files_markdown.contains("rust=")
            && files_markdown.contains("[parser_backed_graph; graph_fidelity]")
            && files_markdown.contains("language_support_claims:")
            && files_markdown.contains("parser-backed graph, fidelity-gated")
            && files_markdown.contains("coverage:")
            && files_markdown.contains("framework route coverage:"),
        "files markdown should summarize inventory, support tiers, and coverage:\n{files_markdown}"
    );

    let affected = run_cli_json(
        workspace,
        cache_dir,
        &[
            "affected",
            "src/lib.rs",
            "--depth",
            "2",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(affected["matched_file_count"], 1);
    assert!(
        affected["impacted_symbols"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["graph_depth"].is_number()
                    && item["reason"].is_string()
                    && item["confidence"].is_string()
            })),
        "affected JSON should expand changed files to symbols with graph evidence: {affected:#}"
    );
    if let Some(tests) = affected["impacted_tests"].as_array()
        && !tests.is_empty()
    {
        assert!(
            tests.iter().all(|item| {
                item["graph_depth"].is_number()
                    && item["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("focused test hint"))
                    && item["confidence"].is_string()
            }),
            "affected test hints should expose graph evidence and caveat language: {affected:#}"
        );
    }

    let affected_stdin = run_cli_with_stdin(
        workspace,
        cache_dir,
        &[
            "affected",
            "--stdin",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
        "src/runtime.rs\n",
    );
    assert!(
        affected_stdin.status.success(),
        "affected --stdin failed: {}",
        String::from_utf8_lossy(&affected_stdin.stderr)
    );
    let affected_stdin: Value =
        serde_json::from_slice(&affected_stdin.stdout).expect("parse affected stdin json");
    assert_eq!(affected_stdin["changed_paths"][0], "src/runtime.rs");
    assert_eq!(affected_stdin["matched_file_count"], 1);

    let affected_markdown = run_cli(
        workspace,
        cache_dir,
        &[
            "affected",
            "src/lib.rs",
            "--refresh",
            "none",
            "--format",
            "markdown",
        ],
    );
    assert!(
        affected_markdown.status.success(),
        "affected markdown failed: {}",
        String::from_utf8_lossy(&affected_markdown.stderr)
    );
    let affected_markdown = String::from_utf8_lossy(&affected_markdown.stdout);
    assert!(
        affected_markdown.contains("# affected analysis")
            && affected_markdown.contains("matched files:")
            && affected_markdown.contains("impacted symbols:")
            && affected_markdown.contains("): "),
        "affected markdown should summarize impact:\n{affected_markdown}"
    );

    run_git(workspace, &["init"]);
    run_git(workspace, &["add", "."]);
    run_git(
        workspace,
        &[
            "-c",
            "user.email=codestory@example.test",
            "-c",
            "user.name=CodeStory Test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ],
    );
    fs::write(
        workspace.join("src/runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("workspace:{project_name}")
}

pub fn schedule_index(project_path: &str) -> usize {
    super::open_project(project_path).len()
}

pub fn changed_after_index() -> bool {
    true
}
"#,
    )
    .expect("modify runtime fixture for git diff fallback");
    let affected_git = run_cli_json(
        workspace,
        cache_dir,
        &["affected", "--refresh", "none", "--format", "json"],
    );
    assert!(
        affected_git["changed_paths"]
            .as_array()
            .expect("changed paths")
            .iter()
            .any(|path| path == "src/runtime.rs"),
        "affected should default to git diff --name-only HEAD: {affected_git:#}"
    );
}

fn run_git(workspace: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_query_search_fails_closed_without_full_sidecars(workspace: &Path, cache_dir: &Path) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "query",
            "search(query: 'AppController') | limit(1)",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "query search DSL should fail closed without full sidecars"
    );
    assert_sidecar_failure_output(output);
}

fn assert_context_id_fails_closed_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    node_id: &str,
) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "context",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "context --id should fail closed without full sidecars"
    );
    assert_sidecar_failure_output(output);
}

fn remove_and_assert_bookmark_gone(workspace: &Path, cache_dir: &Path, bookmark_id: &str) {
    let removed = run_cli_json(
        workspace,
        cache_dir,
        &["bookmark", "remove", bookmark_id, "--format", "json"],
    );
    assert_eq!(string_field(&removed, &["removed_id"]), bookmark_id);
    let bookmarks_after_remove = run_cli_json(
        workspace,
        cache_dir,
        &["bookmark", "list", "--format", "json"],
    );
    assert!(
        bookmarks_after_remove["bookmarks"]
            .as_array()
            .expect("bookmarks after remove")
            .iter()
            .all(|bookmark| bookmark["bookmark"]["id"] != bookmark_id),
        "bookmark remove should persistently delete the saved focus"
    );
}

fn assert_packet_builds_broad_task_contract(workspace: &Path, cache_dir: &Path) {
    let args = [
        "packet",
        "--question",
        "Explain how AppController routes project opening through normalize_project",
        "--budget",
        "tiny",
        "--task-class",
        "architecture-explanation",
        "--format",
        "json",
    ];
    let output = run_cli(workspace, cache_dir, &args);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stderr}\n{stdout}");
        assert!(
            combined.contains("sidecar retrieval")
                || combined.contains("retrieval sidecar")
                || combined.contains("mandatory"),
            "packet without full sidecars should fail closed with a sidecar diagnostic, got stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        return;
    }
    let packet: Value = serde_json::from_slice(&output.stdout).expect("parse packet json output");
    assert_eq!(
        packet["budget"]["requested"], "tiny",
        "packet should honor the requested budget: {packet:#}"
    );
    assert_eq!(
        packet["plan"]["task_class"], "architecture_explanation",
        "packet should expose the planner task class: {packet:#}"
    );
    assert!(
        array_is_non_empty(&packet, &["plan", "queries"]),
        "packet should expose planned retrieval queries: {packet:#}"
    );
    assert!(
        packet
            .pointer("/sufficiency/status")
            .and_then(Value::as_str)
            .is_some(),
        "packet should expose sufficiency status: {packet:#}"
    );
    assert!(
        array_is_non_empty(&packet, &["answer", "retrieval_trace", "steps"]),
        "packet should include the underlying retrieval trace: {packet:#}"
    );
    assert_eq!(
        packet["answer"]["retrieval_version"], "sidecar",
        "successful packet output must come from mandatory sidecar retrieval: {packet:#}"
    );
}

fn assert_stdio_context_id_fails_closed_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    node_id: &str,
) {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "context",
            "arguments": {
                "id": node_id
            }
        }
    })
    .to_string();
    let stdio = run_stdio_request(workspace, cache_dir, &request);
    assert_eq!(
        stdio["result"]["isError"], true,
        "stdio context --id should return a tool error without full sidecars: {stdio:#}"
    );
    let message = stdio["result"]["structuredContent"]["message"].as_str();
    assert!(
        message.is_some_and(|message| message.contains("sidecar retrieval")),
        "stdio context --id should fail closed without full sidecars: {stdio:#}"
    );
}

impl SearchDirSnapshot {
    fn capture(path: PathBuf) -> Self {
        let modified = fs::metadata(&path)
            .expect("search dir metadata before read commands")
            .modified()
            .expect("search dir modified before read commands");
        Self { path, modified }
    }

    fn assert_unchanged(&self) {
        let after = fs::metadata(&self.path)
            .expect("search dir metadata after read commands")
            .modified()
            .expect("search dir modified after read commands");
        assert_eq!(
            self.modified, after,
            "read commands should not recreate the persisted search directory"
        );
    }
}

#[test]
fn bookmarks_degrade_gracefully_after_reindex_removes_target() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    assert_product_search_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        "normalize_project",
    );
    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "normalize_project",
        Some("src/runtime.rs"),
    );

    let bookmark = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "bookmark",
            "add",
            &format!("--id={node_id}"),
            "--comment",
            "target that will disappear",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    let bookmark_id = string_field(&bookmark, &["bookmark", "bookmark", "id"]).to_string();

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("src").join("runtime.rs"),
        r#"pub fn replacement_runtime_entry(project_name: &str) -> String {
    format!("replacement:{project_name}")
}
"#,
    )
    .expect("remove bookmarked symbol");

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "incremental", "--format", "json"],
    );

    let bookmarks = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["bookmark", "list", "--format", "json"],
    );
    let listed = bookmarks["bookmarks"].as_array().expect("bookmarks");
    assert!(
        listed.is_empty()
            || listed
                .iter()
                .all(|bookmark| bookmark["stale"].as_bool() == Some(true)),
        "bookmark list should prune removed nodes or mark stale rows without crashing: {bookmarks:#}"
    );

    let context = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "context",
            "--bookmark",
            &bookmark_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !context.status.success(),
        "context --bookmark should not silently ignore a removed bookmark"
    );
    let failure = format!(
        "{}{}",
        String::from_utf8_lossy(&context.stdout),
        String::from_utf8_lossy(&context.stderr)
    );
    assert!(
        failure.contains("Bookmark not found") || failure.contains("is stale"),
        "context --bookmark should explain stale or missing bookmark focus, got: {failure}"
    );
}

#[test]
fn read_commands_report_stale_index_freshness_without_refreshing_cache() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    fs::write(
        workspace.path().join("src").join("removed_after_index.rs"),
        "pub fn removed_after_index() {}\n",
    )
    .expect("write pre-index file");

    let index = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    let storage_path = PathBuf::from(string_field(&index, &["storage_path"]));
    let search_dir = search_dir_for_storage(&storage_path);
    let storage_before = fs::metadata(&storage_path)
        .expect("storage metadata before read")
        .modified()
        .expect("storage modified before read");
    let search_dir_before = fs::metadata(&search_dir)
        .expect("search dir metadata before read")
        .modified()
        .expect("search dir modified before read");

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("changed:{project_name}")
}
"#,
    )
    .expect("modify indexed file after indexing");
    fs::write(
        workspace.path().join("src").join("new_after_index.rs"),
        "pub fn new_after_index() {}\n",
    )
    .expect("write new file after indexing");
    fs::remove_file(workspace.path().join("src").join("removed_after_index.rs"))
        .expect("remove indexed file after indexing");

    assert_product_search_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        "AppController",
    );

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    assert_stale_freshness_counts(&doctor, "doctor");

    let storage_after = fs::metadata(&storage_path)
        .expect("storage metadata after read")
        .modified()
        .expect("storage modified after read");
    let search_dir_after = fs::metadata(&search_dir)
        .expect("search dir metadata after read")
        .modified()
        .expect("search dir modified after read");
    assert_eq!(
        storage_before, storage_after,
        "read freshness checks should not mutate the SQLite cache"
    );
    assert_eq!(
        search_dir_before, search_dir_after,
        "read freshness checks should not recreate the persisted search directory"
    );
}

#[test]
fn context_json_reports_deep_trace_by_default() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_investigation_workspace(workspace.path());

    let index = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index["summary"]["stats"]["node_count"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "index should discover investigation fixture symbols"
    );

    assert_product_search_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        "parse_investigation_event",
    );
    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "parse_investigation_event",
        None,
    );

    let context = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "context",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !context.status.success(),
        "context --id should fail closed without full sidecars"
    );
    assert_sidecar_failure_output(context);
}
