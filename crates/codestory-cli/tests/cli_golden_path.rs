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

fn clean_test_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn install_fake_managed_embeddings(cache_dir: &Path) {
    let root = cache_dir.join("managed-embeddings");
    let server_dir = root.join("llama").join("b9058").join("fake");
    let model_path = root
        .join("models")
        .join("bge-base-en-v1.5-gguf")
        .join("bge-base-en-v1.5-q8_0.gguf");
    fs::create_dir_all(&server_dir).expect("create fake server dir");
    fs::create_dir_all(model_path.parent().expect("model parent")).expect("create model dir");
    let server_name = if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    fs::copy(
        env!("CARGO_BIN_EXE_codestory-cli"),
        server_dir.join(server_name),
    )
    .expect("copy fake server executable");
    fs::write(&model_path, b"fake model").expect("write fake model");
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "llama_release": "b9058",
            "llama_asset": "fake",
            "llama_path": clean_test_path(&server_dir),
            "llama_variant": "cpu",
            "model_asset": "bge-base-en-v1.5-q8_0.gguf",
            "model_path": clean_test_path(&model_path),
            "model_quant": "q8_0",
            "endpoint": "http://127.0.0.1:8080/v1/embeddings",
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

fn trace_field_value<'a>(step: &'a Value, bucket: &str, key: &str) -> Option<&'a str> {
    step[bucket].as_array()?.iter().find_map(|field| {
        (field["key"] == key)
            .then(|| field["value"].as_str())
            .flatten()
    })
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
    assert!(
        setup["llama"]["url"]
            .as_str()
            .is_some_and(|value| value.contains("ggml-org/llama.cpp")),
        "setup should pin a llama.cpp release asset: {setup:#}"
    );
    let expected_variant = if cfg!(target_os = "macos") {
        "cpu"
    } else {
        "vulkan"
    };
    assert_eq!(
        setup["llama_variant"], expected_variant,
        "setup should default to Vulkan where a pinned asset exists and report CPU only as fallback: {setup:#}"
    );
    assert!(
        setup["model"]["url"]
            .as_str()
            .is_some_and(|value| value.contains("CompendiumLabs/bge-base-en-v1.5-gguf")),
        "setup should pin the managed BGE-base GGUF model: {setup:#}"
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
fn doctor_reports_missing_managed_assets_before_setup() {
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
    assert_eq!(
        managed["status"], "info",
        "missing managed assets should be informational before setup: {doctor:#}"
    );
    assert!(
        managed["message"]
            .as_str()
            .is_some_and(|message| message.contains("setup embeddings")),
        "doctor should name the setup command when managed assets are missing: {doctor:#}"
    );
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
    assert!(
        ["ok", "warn"].contains(&managed["status"].as_str().unwrap_or_default()),
        "installed managed assets should be inspected without being treated as missing: {doctor:#}"
    );
    assert!(
        !cache_dir
            .path()
            .join("managed-embeddings")
            .join("logs")
            .exists(),
        "doctor should not create managed llama-server logs or start the fake server: {doctor:#}"
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

    let trail_story = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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

    let bookmark = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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
        workspace.path(),
        cache_dir.path(),
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

    let bookmarked_ask = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "ask",
            "What does this bookmark focus on?",
            "--bookmark",
            &bookmark_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        bookmarked_ask["retrieval_trace"]["steps"]
            .as_array()
            .expect("bookmark ask trace steps")
            .iter()
            .any(|step| step["input"].as_array().is_some_and(|fields| fields
                .iter()
                .any(|field| field["key"] == "has_focus" && field["value"] == "true"))),
        "ask --bookmark should explicitly seed focused retrieval"
    );
    assert!(
        bookmarked_ask["retrieval_trace"]["annotations"]
            .as_array()
            .expect("bookmark ask trace annotations")
            .iter()
            .any(|annotation| {
                let Some(annotation) = annotation.as_str() else {
                    return false;
                };
                annotation.contains(&format!("bookmark_focus id={bookmark_id}"))
                    && annotation.contains("comment=`entry point under review`")
            }),
        "ask --bookmark should preserve bookmark identity in the retrieval trace"
    );

    let explore = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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
        string_field(&explore, &["snippet", "snippet"]).contains("AppController"),
        "explore JSON should include snippet detail"
    );

    let explore_markdown = run_cli(
        workspace.path(),
        cache_dir.path(),
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
        "search:",
        "results:",
        "resolution:",
        "navigation:",
        "symbol:",
        "trail:",
        "snippet:",
        "snippet_context:",
        "semantic_runtime:",
        "output_write:",
    ] {
        assert!(
            explore_markdown.contains(expected),
            "explore markdown should contain `{expected}`:\n{explore_markdown}"
        );
    }

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
    assert!(
        ask["retrieval_trace"]["steps"]
            .as_array()
            .expect("trace steps")
            .iter()
            .all(|step| step["kind"] != "local_agent"),
        "CLI ask should not include removed local-agent trace steps"
    );

    let removed = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["bookmark", "remove", &bookmark_id, "--format", "json"],
    );
    assert_eq!(string_field(&removed, &["removed_id"]), bookmark_id);
    let bookmarks_after_remove = run_cli_json(
        workspace.path(),
        cache_dir.path(),
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

    let stdio = run_stdio_request(
        workspace.path(),
        cache_dir.path(),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ask","arguments":{"prompt":"What does AppController do?"}}}"#,
    );
    let stdio_result = &stdio["result"]["structuredContent"];
    assert!(
        stdio_result["retrieval_trace"]["steps"]
            .as_array()
            .expect("stdio ask trace steps")
            .iter()
            .all(|step| step["kind"] != "local_agent"),
        "stdio ask should not include removed local-agent trace steps"
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
fn bookmarks_degrade_gracefully_after_reindex_removes_target() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    let search = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "search",
            "--query",
            "normalize_project",
            "--repo-text",
            "off",
            "--limit",
            "10",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    let target = search["indexed_symbol_hits"]
        .as_array()
        .expect("indexed symbol hits")
        .iter()
        .find(|hit| {
            hit["display_name"] == "normalize_project"
                && hit["file_path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("src/runtime.rs"))
        })
        .unwrap_or_else(|| panic!("normalize_project hit should exist: {search:#}"));
    let node_id = target["node_id"].as_str().expect("node id");

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

    let ask = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "ask",
            "What did this bookmark point to?",
            "--bookmark",
            &bookmark_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !ask.status.success(),
        "ask --bookmark should not silently ignore a removed bookmark"
    );
    let failure = format!(
        "{}{}",
        String::from_utf8_lossy(&ask.stdout),
        String::from_utf8_lossy(&ask.stderr)
    );
    assert!(
        failure.contains("Bookmark not found") || failure.contains("is stale"),
        "ask --bookmark should explain stale or missing bookmark focus, got: {failure}"
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
    let trace_steps = investigated["retrieval_trace"]["steps"]
        .as_array()
        .expect("trace steps");
    if let Some(trail_step) = trace_steps.iter().find(|step| step["kind"] == "trail") {
        assert!(
            trace_field_value(trail_step, "input", "max_nodes").is_some()
                || trace_field_value(trail_step, "output", "max_nodes").is_some(),
            "trail trace should expose the active max_nodes cap: {trail_step}"
        );
    }
    let source_step = trace_steps
        .iter()
        .find(|step| step["kind"] == "source_read")
        .expect("source_read step");
    if source_step["status"] == "ok" {
        let max_source_bytes = trace_field_value(source_step, "output", "max_source_bytes")
            .and_then(|value| value.parse::<u64>().ok())
            .expect("max_source_bytes output");
        let snippet_bytes = trace_field_value(source_step, "output", "snippet_bytes")
            .and_then(|value| value.parse::<u64>().ok())
            .expect("snippet_bytes output");
        assert!(
            snippet_bytes <= max_source_bytes,
            "source_read should report snippet bytes within cap: {source_step}"
        );
    }
    if let Some(repo_text_step) = trace_steps
        .iter()
        .find(|step| step["kind"] == "repo_text_fallback")
    {
        assert!(
            trace_field_value(repo_text_step, "output", "file_cap").is_some(),
            "repo-text fallback trace should expose scan caps: {repo_text_step}"
        );
    }
    let synthesis_step = trace_steps
        .iter()
        .find(|step| step["kind"] == "answer_synthesis")
        .expect("answer synthesis step");
    assert!(
        trace_field_value(synthesis_step, "output", "graph_artifact_byte_cap").is_some(),
        "answer synthesis should expose graph artifact bundle caps: {synthesis_step}"
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

    assert!(
        trace_steps.iter().all(|step| step["kind"] != "local_agent"),
        "ask --investigate should not include removed local-agent trace steps"
    );
}
