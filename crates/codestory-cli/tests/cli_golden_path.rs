use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
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

fn run_stdio_request(
    workspace: &Path,
    cache_dir: &Path,
    env_root: &Path,
    marker: &Path,
    request: &str,
) -> Value {
    let mut fake_agent_dir = env_root.join("bin");
    if cfg!(target_os = "windows") {
        fake_agent_dir = env_root.join("appdata").join("npm");
        fs::create_dir_all(&fake_agent_dir).expect("create fake npm dir");
        fs::write(
            fake_agent_dir.join("codex.cmd"),
            "@echo off\r\necho spawned > \"%CODESTORY_FAKE_AGENT_MARKER%\"\r\nexit /b 7\r\n",
        )
        .expect("write fake codex.cmd");
    } else {
        fs::create_dir_all(&fake_agent_dir).expect("create fake bin dir");
        let fake_agent = fake_agent_dir.join("codex");
        fs::write(
            &fake_agent,
            "#!/bin/sh\necho spawned > \"$CODESTORY_FAKE_AGENT_MARKER\"\nexit 7\n",
        )
        .expect("write fake codex");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&fake_agent)
                .expect("fake codex metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake_agent, permissions).expect("chmod fake codex");
        }
    }

    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut path_entries = vec![fake_agent_dir.clone()];
    path_entries.extend(std::env::split_paths(&path));
    let path = std::env::join_paths(path_entries).expect("join PATH");

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
        .env("CODESTORY_FAKE_AGENT_MARKER", marker)
        .env("APPDATA", env_root.join("appdata"))
        .env("USERPROFILE", env_root.join("home"))
        .env("LOCALAPPDATA", env_root.join("localappdata"))
        .env("PATH", path)
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
    let mut current = value;
    for key in path {
        current = match current {
            Value::Object(fields) => fields
                .get(*key)
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
        };
    }
    current
        .as_str()
        .unwrap_or_else(|| panic!("expected string at path {path:?}"))
}

fn array_is_non_empty(value: &Value, path: &[&str]) -> bool {
    let mut current = value;
    for key in path {
        current = match current {
            Value::Object(fields) => fields
                .get(*key)
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
        };
    }
    current.as_array().is_some_and(|items| !items.is_empty())
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

fn trace_has_step(value: &Value, kind: &str) -> bool {
    value["retrieval_trace"]["steps"]
        .as_array()
        .expect("retrieval trace steps")
        .iter()
        .any(|step| step["kind"] == kind)
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
fn tiny_workspace_browser_loop_works_from_existing_cache() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

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
        "index should discover symbols in the tiny workspace"
    );
    let storage_path = PathBuf::from(string_field(&index, &["storage_path"]));
    let search_dir = search_dir_for_storage(&storage_path);
    let search_dir_before = fs::metadata(&search_dir)
        .expect("search dir metadata before read commands")
        .modified()
        .expect("search dir modified before read commands");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
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

    let ground = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["ground", "--refresh", "none", "--format", "json"],
    );
    assert!(
        array_is_non_empty(&ground, &["root_symbols"]) || array_is_non_empty(&ground, &["files"]),
        "ground should return project grounding data"
    );

    let search = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "search",
            "--query",
            "AppController",
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
        array_is_non_empty(&search, &["indexed_symbol_hits"]),
        "search should find AppController in the existing cache"
    );
    assert!(
        search["retrieval"]["stored_embedding"]["doc_count"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "search should preserve stored semantic-doc contract metadata"
    );
    let node_id = string_field(&search, &["indexed_symbol_hits", "0", "node_id"]);

    let symbol = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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

    let trail = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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

    let snippet = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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

    let query = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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
        array_is_non_empty(&query, &["items"]),
        "query should read from the existing cache"
    );

    let ask = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "ask",
            "What does AppController do?",
            &format!("--focus-id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
            "--agent-command",
            "definitely-not-codestory-local-agent",
        ],
    );
    assert!(
        array_is_non_empty(&ask, &["sections"]),
        "ask should return a DB-first answer packet"
    );
    assert!(
        array_is_non_empty(&ask, &["retrieval_trace", "steps"]),
        "ask should include a retrieval trace"
    );
    let local_agent_step = ask["retrieval_trace"]["steps"]
        .as_array()
        .expect("trace steps")
        .iter()
        .find(|step| step["kind"] == "local_agent")
        .expect("local agent trace step should be present");
    assert_eq!(local_agent_step["status"], "skipped");
    assert!(
        local_agent_step["input"]
            .as_array()
            .expect("local agent input fields")
            .iter()
            .any(|field| field["key"] == "requested" && field["value"] == "false"),
        "CLI ask should not request local-agent execution without --with-local-agent"
    );
    assert!(
        local_agent_step["output"]
            .as_array()
            .expect("local agent output fields")
            .iter()
            .any(|field| field["key"] == "state" && field["value"] == "disabled"),
        "trace should make the disabled local-agent state explicit"
    );

    let stdio_env = tempdir().expect("stdio env dir");
    let marker = stdio_env.path().join("fake-agent-spawned.txt");
    let stdio = run_stdio_request(
        workspace.path(),
        cache_dir.path(),
        stdio_env.path(),
        &marker,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ask","arguments":{"prompt":"What does AppController do?"}}}"#,
    );
    assert!(
        stdio["result"]["retrieval_trace"]["steps"]
            .as_array()
            .expect("stdio ask trace steps")
            .iter()
            .any(|step| {
                step["kind"] == "local_agent"
                    && step["status"] == "skipped"
                    && step["input"]
                        .as_array()
                        .expect("local agent input")
                        .iter()
                        .any(|field| field["key"] == "requested" && field["value"] == "false")
            }),
        "stdio ask should leave local-agent execution disabled by omission"
    );
    assert!(
        !marker.exists(),
        "stdio ask without an explicit local-agent request should not spawn a default codex command"
    );

    let search_dir_after = fs::metadata(&search_dir)
        .expect("search dir metadata after read commands")
        .modified()
        .expect("search dir modified after read commands");
    assert_eq!(
        search_dir_before, search_dir_after,
        "read commands should not recreate the persisted search directory"
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

    let search = run_cli_json(
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
    assert_stale_freshness_counts(&search, "search --refresh none");

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
fn ask_investigate_json_reports_bounded_trace_without_changing_plain_ask() {
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

    let plain_ask = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "ask",
            "Where is INVESTIGATION_LITERAL used?",
            "--refresh",
            "none",
            "--format",
            "json",
            "--agent-command",
            "definitely-not-codestory-local-agent",
        ],
    );
    assert!(
        trace_has_step(&plain_ask, "search"),
        "plain ask should keep the existing DB-first trace"
    );
    assert!(
        plain_ask["retrieval_trace"]["annotations"]
            .as_array()
            .is_none_or(|annotations| {
                !annotations.iter().any(|annotation| {
                    annotation
                        .as_str()
                        .is_some_and(|value| value.contains("investigate"))
                })
            }),
        "plain ask should not silently opt into the new investigation mode"
    );

    let investigated = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "ask",
            "--investigate",
            "Where is INVESTIGATION_LITERAL used and what source was checked?",
            "--refresh",
            "none",
            "--format",
            "json",
            "--agent-command",
            "definitely-not-codestory-local-agent",
        ],
    );
    assert!(
        trace_has_step(&investigated, "search"),
        "investigation starts with the current search ranking"
    );
    assert!(
        trace_has_step(&investigated, "query_expansion")
            || trace_has_step(&investigated, "repo_text_fallback"),
        "weak first hits should trigger query expansion or exact-symbol/file fallback"
    );
    assert!(
        trace_has_step(&investigated, "trail") || trace_has_step(&investigated, "neighborhood"),
        "investigation should record bounded graph expansion"
    );
    assert!(
        trace_has_step(&investigated, "source_read"),
        "investigation should record bounded source/snippet reads"
    );
    assert!(
        array_is_non_empty(&investigated, &["citations"]),
        "investigation should return cited evidence"
    );
    assert!(
        investigated["retrieval_trace"]["annotations"]
            .as_array()
            .expect("trace annotations")
            .iter()
            .any(|annotation| {
                annotation.as_str().is_some_and(|value| {
                    let value = value.to_ascii_lowercase();
                    value.contains("what i checked") || value.contains("investigation")
                })
            }),
        "investigation JSON should expose the named mode or what-I-checked trace"
    );

    let local_agent_step = investigated["retrieval_trace"]["steps"]
        .as_array()
        .expect("trace steps")
        .iter()
        .find(|step| step["kind"] == "local_agent")
        .expect("local agent trace step");
    assert_eq!(local_agent_step["status"], "skipped");
    assert!(
        local_agent_step["input"]
            .as_array()
            .expect("local agent input fields")
            .iter()
            .any(|field| field["key"] == "requested" && field["value"] == "false"),
        "ask --investigate must not request local-agent execution by default"
    );
}
