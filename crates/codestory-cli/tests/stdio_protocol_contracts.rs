use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

struct StdioFixture {
    workspace: TempDir,
    cache_dir: TempDir,
    hash_embeddings: bool,
    latest_release_version: String,
    disable_installed_cli_probe: bool,
    sidecar_policy_state: Option<String>,
    sidecar_last_repair_command: Option<String>,
    dirty_marker_path: Option<PathBuf>,
    dirty_marker_project_root: Option<PathBuf>,
    local_refresh_timeout_ms: Option<u64>,
}

struct StdioServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

struct RemoveDirOnDrop(PathBuf);

impl Drop for RemoveDirOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Drop for StdioServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_tiny_rust_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "tiny-stdio-contract-fixture"
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
        r#"pub mod alpha;
pub mod beta;
pub mod runtime;

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
}

pub fn open_project(project_name: &str) -> String {
    runtime::normalize_project(project_name)
}
"#,
    )
    .expect("write lib.rs");
    fs::write(
        src.join("alpha.rs"),
        r#"pub fn configure() -> usize {
    1
}
"#,
    )
    .expect("write alpha.rs");
    fs::write(
        src.join("beta.rs"),
        r#"pub fn configure() -> usize {
    2
}
"#,
    )
    .expect("write beta.rs");
    fs::write(
        src.join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("workspace:{project_name}")
}
"#,
    )
    .expect("write runtime.rs");
}

fn indexed_fixture() -> StdioFixture {
    indexed_fixture_with_embedding_mode(true)
}

fn write_dirty_marker_fixture(fixture: &StdioFixture, name: &str, marker: Value) -> PathBuf {
    let marker_path = fixture.cache_dir.path().join(name);
    thread::sleep(Duration::from_millis(25));
    fs::write(&marker_path, marker.to_string()).expect("write dirty marker");
    marker_path
}

fn refresh_fixture_index(fixture: &StdioFixture) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command
        .arg("index")
        .arg("--refresh")
        .arg("incremental")
        .arg("--format")
        .arg("json")
        .arg("--project")
        .arg(fixture.workspace.path())
        .arg("--cache-dir")
        .arg(fixture.cache_dir.path());
    apply_fixture_embedding_env(&mut command, fixture.hash_embeddings);
    let output = command.output().expect("run index refresh");
    assert!(
        output.status.success(),
        "index refresh failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn indexed_fixture_with_embedding_mode(hash_embeddings: bool) -> StdioFixture {
    let workspace = tempfile::tempdir().expect("workspace dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command
        .arg("index")
        .arg("--refresh")
        .arg("full")
        .arg("--format")
        .arg("json")
        .arg("--project")
        .arg(workspace.path())
        .arg("--cache-dir")
        .arg(cache_dir.path());
    apply_fixture_embedding_env(&mut command, hash_embeddings);
    let output = command.output().expect("run index");
    assert!(
        output.status.success(),
        "index failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    StdioFixture {
        workspace,
        cache_dir,
        hash_embeddings,
        latest_release_version: env!("CARGO_PKG_VERSION").to_string(),
        disable_installed_cli_probe: false,
        sidecar_policy_state: None,
        sidecar_last_repair_command: None,
        dirty_marker_path: None,
        dirty_marker_project_root: None,
        local_refresh_timeout_ms: None,
    }
}

fn unindexed_fixture() -> StdioFixture {
    let workspace = tempfile::tempdir().expect("workspace dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    StdioFixture {
        workspace,
        cache_dir,
        hash_embeddings: true,
        latest_release_version: env!("CARGO_PKG_VERSION").to_string(),
        disable_installed_cli_probe: false,
        sidecar_policy_state: None,
        sidecar_last_repair_command: None,
        dirty_marker_path: None,
        dirty_marker_project_root: None,
        local_refresh_timeout_ms: None,
    }
}

fn full_retrieval_fixture() -> Result<Option<StdioFixture>, String> {
    if !env_flag("CODESTORY_STDIO_FULL_RETRIEVAL_TESTS") {
        return Ok(None);
    }
    let fixture = indexed_fixture_with_embedding_mode(false);
    let output = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("retrieval")
        .arg("index")
        .arg("--refresh")
        .arg("none")
        .arg("--format")
        .arg("json")
        .arg("--project")
        .arg(fixture.workspace.path())
        .arg("--cache-dir")
        .arg(fixture.cache_dir.path())
        .output()
        .expect("run retrieval index");
    if !output.status.success() {
        return Err(format!(
            "full-retrieval stdio contract setup failed: retrieval index failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let status = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("retrieval")
        .arg("status")
        .arg("--format")
        .arg("json")
        .arg("--project")
        .arg(fixture.workspace.path())
        .arg("--cache-dir")
        .arg(fixture.cache_dir.path())
        .output()
        .expect("run retrieval status");
    if !status.status.success() {
        return Err(format!(
            "full-retrieval stdio contract setup failed: retrieval status failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    let status_json: Value = serde_json::from_slice(&status.stdout)
        .map_err(|error| format!("full-retrieval status emitted invalid json: {error}"))?;
    if status_json["retrieval_mode"] != json!("full") {
        return Err(format!(
            "full-retrieval stdio contract setup failed: retrieval status was not full: {status_json:#}"
        ));
    }
    Ok(Some(fixture))
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn apply_fixture_embedding_env(command: &mut Command, hash_embeddings: bool) {
    if hash_embeddings {
        command.env("CODESTORY_EMBED_RUNTIME_MODE", "hash");
    }
}

fn spawn_stdio_server(fixture: &StdioFixture) -> StdioServer {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command
        .arg("serve")
        .arg("--stdio")
        .arg("--refresh")
        .arg("none")
        .arg("--project")
        .arg(fixture.workspace.path())
        .arg("--cache-dir")
        .arg(fixture.cache_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_fixture_embedding_env(&mut command, fixture.hash_embeddings);
    command.env(
        "CODESTORY_LATEST_RELEASE_VERSION",
        &fixture.latest_release_version,
    );
    if fixture.disable_installed_cli_probe {
        command.env("CODESTORY_DISABLE_INSTALLED_CLI_PROBE", "1");
    }
    if let Some(state) = &fixture.sidecar_policy_state {
        command.env("CODESTORY_PLUGIN_SIDECAR_POLICY_STATE", state);
        command.env(
            "CODESTORY_PLUGIN_SIDECAR_POLICY_PATH",
            fixture.cache_dir.path().join("plugin-sidecar-policy.json"),
        );
        command.env(
            "CODESTORY_PLUGIN_SIDECAR_ENABLE_COMMAND",
            "node codestory-mcp.cjs sidecar-policy enable",
        );
        command.env(
            "CODESTORY_PLUGIN_SIDECAR_DISABLE_COMMAND",
            "node codestory-mcp.cjs sidecar-policy disable",
        );
    }
    if let Some(command_text) = &fixture.sidecar_last_repair_command {
        command.env("CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_STATE", "completed");
        command.env("CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_COMMAND", command_text);
    }
    if let Some(path) = &fixture.dirty_marker_path {
        command.env("CODESTORY_PLUGIN_DIRTY_MARKER_PATH", path);
    }
    if let Some(root) = &fixture.dirty_marker_project_root {
        command.env("CODESTORY_PLUGIN_DIRTY_MARKER_PROJECT_ROOT", root);
    }
    if let Some(timeout_ms) = fixture.local_refresh_timeout_ms {
        command.env(
            "CODESTORY_STDIO_LOCAL_REFRESH_TIMEOUT_MS",
            timeout_ms.to_string(),
        );
    }
    let mut child = command.spawn().expect("spawn stdio server");

    let stdin = child.stdin.take().expect("stdio stdin");
    let stdout = BufReader::new(child.stdout.take().expect("stdio stdout"));
    StdioServer {
        child,
        stdin,
        stdout,
    }
}

fn send_json(server: &mut StdioServer, request: Value) -> Value {
    send_line(server, &request.to_string())
}

fn send_line(server: &mut StdioServer, line: &str) -> Value {
    writeln!(server.stdin, "{line}").expect("write request line");
    server.stdin.flush().expect("flush request line");

    let mut response = String::new();
    let bytes = server
        .stdout
        .read_line(&mut response)
        .expect("read response line");
    assert!(bytes > 0, "stdio server closed before responding");
    serde_json::from_str(response.trim()).expect("parse response json")
}

fn assert_success_envelope(response: &Value, id: Value) -> &Value {
    assert_eq!(response.get("jsonrpc"), Some(&json!("2.0")));
    assert_eq!(response.get("id"), Some(&id));
    assert!(
        response.get("error").is_none(),
        "success response should not include error: {response}"
    );
    response.get("result").expect("success result")
}

fn assert_tool_success(response: &Value, id: Value) -> &Value {
    let result = assert_success_envelope(response, id);
    assert!(
        result.get("isError").and_then(Value::as_bool) != Some(true),
        "successful tools/call should not be marked as an error: {response}"
    );
    assert_tool_text_content(result, response);
    result
        .get("structuredContent")
        .expect("tools/call success should include structuredContent")
}

fn assert_structured_citations_have_no_evidence(value: &Value) {
    fn visit(value: &Value, citation_count: &mut usize) {
        match value {
            Value::Object(map) => {
                if map.contains_key("node_id")
                    && map.contains_key("display_name")
                    && map.contains_key("score")
                {
                    *citation_count += 1;
                    assert!(
                        map.get("evidence_edge_ids")
                            .and_then(Value::as_array)
                            .is_none_or(|edges| edges.is_empty()),
                        "citation should omit evidence edge ids when include_evidence=false: {value}"
                    );
                    assert!(
                        map.get("retrieval_score_breakdown")
                            .is_none_or(Value::is_null),
                        "citation should omit retrieval score breakdown when include_evidence=false: {value}"
                    );
                }
                for child in map.values() {
                    visit(child, citation_count);
                }
            }
            Value::Array(items) => {
                for child in items {
                    visit(child, citation_count);
                }
            }
            _ => {}
        }
    }

    let mut citation_count = 0;
    visit(value, &mut citation_count);
    assert!(
        citation_count > 0,
        "test fixture should return citations to prove evidence stripping: {value}"
    );
}

fn assert_tool_error(response: &Value, id: Value) -> &Value {
    let result = assert_success_envelope(response, id);
    assert_eq!(
        result.get("isError").and_then(Value::as_bool),
        Some(true),
        "tools/call execution errors should be returned as CallToolResult errors: {response}"
    );
    assert_tool_text_content(result, response);
    result
        .get("structuredContent")
        .expect("tools/call error should include structuredContent")
}

fn assert_tool_text_content<'a>(result: &'a Value, response: &Value) -> &'a str {
    result["content"]
        .as_array()
        .and_then(|content| content.first())
        .and_then(|content| {
            (content["type"] == "text")
                .then(|| content["text"].as_str())
                .flatten()
        })
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| panic!("tools/call result should include text content: {response}"))
}

fn assert_error_envelope(response: &Value, id: Value) -> &Value {
    assert_eq!(response.get("jsonrpc"), Some(&json!("2.0")));
    assert_eq!(response.get("id"), Some(&id));
    assert!(
        response.get("result").is_none(),
        "error response should not include result: {response}"
    );
    let error = response.get("error").expect("error object");
    assert!(
        error.get("code").and_then(Value::as_i64).is_some(),
        "error should include numeric code: {response}"
    );
    assert!(
        error.get("message").and_then(Value::as_str).is_some(),
        "error should include message: {response}"
    );
    error
}

fn assert_error_code(error: &Value, code: i64) {
    assert_eq!(
        error.get("code").and_then(Value::as_i64),
        Some(code),
        "unexpected JSON-RPC error code: {error}"
    );
}

fn sorted_field_values<'a>(items: &'a Value, array_field: &str, field: &str) -> Vec<&'a str> {
    let mut values: Vec<_> = items[array_field]
        .as_array()
        .unwrap_or_else(|| panic!("{array_field} should be an array: {items}"))
        .iter()
        .map(|item| {
            item[field].as_str().unwrap_or_else(|| {
                panic!("{array_field} item should include string {field}: {item}")
            })
        })
        .collect();
    values.sort_unstable();
    values
}

fn tool_by_name<'a>(tools: &'a Value, name: &str) -> &'a Value {
    tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("missing tool {name}: {tools}"))
}

fn tool_input_schema<'a>(tools: &'a Value, name: &str) -> &'a Value {
    tool_by_name(tools, name)
        .get("inputSchema")
        .unwrap_or_else(|| panic!("tool {name} should include inputSchema: {tools}"))
}

fn tool_output_schema<'a>(tools: &'a Value, name: &str) -> &'a Value {
    tool_by_name(tools, name)
        .get("outputSchema")
        .unwrap_or_else(|| panic!("tool {name} should include outputSchema: {tools}"))
}

fn required_fields(schema: &Value) -> BTreeSet<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("schema should include required fields: {schema}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("required field should be a string: {schema}"))
        })
        .collect()
}

fn schema_property<'a>(schema: &'a Value, name: &str) -> &'a Value {
    schema
        .pointer(&format!("/properties/{name}"))
        .unwrap_or_else(|| panic!("schema should include property {name}: {schema}"))
}

fn assert_schema_enum_values(schema: &Value, pointer: &str, expected: &[&str]) {
    let values: BTreeSet<_> = schema
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("schema should include enum array at {pointer}: {schema}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("enum values should be strings at {pointer}: {schema}"))
        })
        .collect();
    for expected_value in expected {
        assert!(
            values.contains(expected_value),
            "schema enum {pointer} should include {expected_value}: {schema}"
        );
    }
}

fn contains_key_recursive(value: &Value, names: &[&str]) -> bool {
    match value {
        Value::Object(map) => {
            map.keys().any(|key| names.contains(&key.as_str()))
                || map
                    .values()
                    .any(|child| contains_key_recursive(child, names))
        }
        Value::Array(values) => values
            .iter()
            .any(|child| contains_key_recursive(child, names)),
        _ => false,
    }
}

fn contains_bool_recursive(value: &Value, names: &[&str], expected: bool) -> bool {
    match value {
        Value::Object(map) => {
            map.iter().any(|(key, child)| {
                names.contains(&key.as_str()) && child.as_bool() == Some(expected)
            }) || map
                .values()
                .any(|child| contains_bool_recursive(child, names, expected))
        }
        Value::Array(values) => values
            .iter()
            .any(|child| contains_bool_recursive(child, names, expected)),
        _ => false,
    }
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

fn assert_fresh_freshness_counts(value: &Value, context: &str) {
    let freshness = find_index_freshness(value)
        .unwrap_or_else(|| panic!("{context} should include an index freshness signal: {value:#}"));
    assert_eq!(
        freshness.get("status").and_then(Value::as_str),
        Some("fresh"),
        "{context} freshness should be fresh after reindex: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["changed_file_count", "changed_count", "changed"]
        ),
        Some(0),
        "{context} freshness should report no changed files: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["new_file_count", "new_count", "new", "added_count", "added"]
        ),
        Some(0),
        "{context} freshness should report no new files: {freshness:#}"
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
        "{context} freshness should report no removed files: {freshness:#}"
    );
}

fn assert_allowed_surface(
    status: &Value,
    surface: &str,
    expected_allowed: bool,
    expected_goal: &str,
    expected_status: &str,
) {
    let surface_status = status
        .pointer(&format!("/allowed_surfaces/{surface}"))
        .unwrap_or_else(|| panic!("status should include allowed_surfaces.{surface}: {status}"));
    assert_eq!(
        surface_status["allowed"],
        json!(expected_allowed),
        "unexpected allowed state for {surface}: {surface_status}"
    );
    assert_eq!(
        surface_status["readiness_goal"],
        json!(expected_goal),
        "unexpected readiness goal for {surface}: {surface_status}"
    );
    assert_eq!(
        surface_status["status"],
        json!(expected_status),
        "unexpected readiness status for {surface}: {surface_status}"
    );
    assert!(
        surface_status["summary"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "surface status should include a readiness summary for {surface}: {surface_status}"
    );
    if expected_allowed {
        assert!(
            surface_status["blocked_reason"].is_null(),
            "allowed surface {surface} should not include a blocked reason: {surface_status}"
        );
    } else {
        assert!(
            surface_status["blocked_reason"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "blocked surface {surface} should explain why it is blocked: {surface_status}"
        );
        assert!(
            surface_status["minimum_next"]
                .as_array()
                .is_some_and(|commands| !commands.is_empty()),
            "blocked surface {surface} should include minimum repair guidance: {surface_status}"
        );
    }
}

fn string_values_recursive<'a>(value: &'a Value, strings: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => strings.push(text),
        Value::Array(values) => {
            for child in values {
                string_values_recursive(child, strings);
            }
        }
        Value::Object(map) => {
            for child in map.values() {
                string_values_recursive(child, strings);
            }
        }
        _ => {}
    }
}

fn json_resource_content(result: &Value, uri: &str) -> Value {
    let content = result["contents"]
        .as_array()
        .expect("resource contents")
        .iter()
        .find(|content| content["uri"] == uri)
        .unwrap_or_else(|| panic!("resource read should include content for {uri}: {result}"));
    assert_eq!(content["mimeType"], "application/json");
    let text = content["text"]
        .as_str()
        .unwrap_or_else(|| panic!("resource {uri} content should include JSON text: {content}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("resource {uri} should be parseable JSON: {error}\n{text}"))
}

fn write_active_repair_status_fixture(
    fixture: &StdioFixture,
    run_id: &str,
    phase: &str,
) -> (PathBuf, RemoveDirOnDrop) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_millis() as i64;
    write_repair_status_fixture(fixture, run_id, phase, now)
}

fn write_abandoned_repair_status_fixture(
    fixture: &StdioFixture,
    run_id: &str,
    phase: &str,
) -> (PathBuf, RemoveDirOnDrop) {
    let updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_millis() as i64
        - 60_000;
    write_repair_status_fixture(fixture, run_id, phase, updated_at)
}

fn write_repair_status_fixture(
    fixture: &StdioFixture,
    run_id: &str,
    phase: &str,
    updated_at_epoch_ms: i64,
) -> (PathBuf, RemoveDirOnDrop) {
    let canonical_root =
        fs::canonicalize(fixture.workspace.path()).expect("canonical fixture root");
    let sidecar = codestory_retrieval::sidecar_runtime_for_project_with_run_id(
        &canonical_root,
        codestory_retrieval::SidecarProfile::Agent,
        Some(run_id),
    );
    let status_path = sidecar
        .layout
        .state_file
        .with_file_name("ready-repair-status.json");
    let status_dir = status_path
        .parent()
        .expect("repair status parent")
        .to_path_buf();
    fs::create_dir_all(&status_dir).expect("create repair status dir");
    let project_root = canonical_root
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    fs::write(
        &status_path,
        json!({
            "schema_version": 1,
            "status": "repairing",
            "project_root": project_root,
            "profile": "agent",
            "run_id": run_id,
            "namespace": sidecar.namespace,
            "compose_project": sidecar.compose_project,
            "phase": phase,
            "pid": std::process::id(),
            "started_at_epoch_ms": updated_at_epoch_ms,
            "updated_at_epoch_ms": updated_at_epoch_ms
        })
        .to_string(),
    )
    .expect("write repair status fixture");
    (status_path, RemoveDirOnDrop(status_dir))
}

fn continuation_uris_for(node_id: &str) -> Vec<String> {
    ["symbol", "snippet", "references", "trail"]
        .iter()
        .map(|kind| format!("codestory://{kind}/{node_id}"))
        .collect()
}

fn assert_continuation_links(value: &Value, node_id: &str, context: &str) {
    let mut strings = Vec::new();
    string_values_recursive(value, &mut strings);
    for expected in continuation_uris_for(node_id) {
        assert!(
            strings.iter().any(|candidate| *candidate == expected),
            "{context} should expose continuation link {expected}: {value}"
        );
    }
}

fn has_safety_metadata(tool: &Value) -> bool {
    let Some(metadata) = tool.get("annotations").or_else(|| tool.get("metadata")) else {
        return false;
    };
    let text = metadata.to_string().to_ascii_lowercase();
    text.contains("write")
        || text.contains("system")
        || text.contains("destructive")
        || text.contains("danger")
        || text.contains("mutation")
        || text.contains("safety")
}

fn assert_read_only_tool_metadata(tool: &Value) {
    let name = tool["name"].as_str().expect("tool name");
    let annotations = tool
        .get("annotations")
        .unwrap_or_else(|| panic!("{name} should include MCP-style annotations: {tool}"));
    let safety = tool
        .get("safety")
        .or_else(|| tool.get("metadata"))
        .unwrap_or_else(|| panic!("{name} should include safety metadata: {tool}"));

    assert!(
        annotations.get("readOnlyHint").and_then(Value::as_bool) == Some(true)
            || contains_bool_recursive(safety, &["readOnly", "read_only"], true),
        "{name} should declare read-only behavior: {tool}"
    );
    assert!(
        annotations.get("destructiveHint").and_then(Value::as_bool) == Some(false)
            || contains_bool_recursive(safety, &["destructive", "destructiveHint"], false),
        "{name} should declare non-destructive behavior: {tool}"
    );
    assert!(
        annotations.get("idempotentHint").and_then(Value::as_bool) == Some(true)
            || contains_bool_recursive(safety, &["idempotent", "idempotentHint"], true),
        "{name} should declare idempotent behavior: {tool}"
    );
    assert!(
        contains_bool_recursive(tool, &["localOnly", "local_only"], true)
            || contains_bool_recursive(tool, &["openWorld", "open_world"], false),
        "{name} should declare local-only or open-world=false behavior: {tool}"
    );
}

fn assert_local_plugin_mutation_tool_metadata(tool: &Value) {
    let name = tool["name"].as_str().expect("tool name");
    let annotations = tool
        .get("annotations")
        .unwrap_or_else(|| panic!("{name} should include MCP-style annotations: {tool}"));
    let safety = tool
        .get("safety")
        .or_else(|| tool.get("metadata"))
        .unwrap_or_else(|| panic!("{name} should include safety metadata: {tool}"));

    assert_eq!(
        annotations.get("readOnlyHint").and_then(Value::as_bool),
        Some(false),
        "{name} should declare local config writes: {tool}"
    );
    assert_eq!(
        annotations.get("destructiveHint").and_then(Value::as_bool),
        Some(false),
        "{name} should declare non-destructive behavior: {tool}"
    );
    assert_eq!(
        annotations.get("idempotentHint").and_then(Value::as_bool),
        Some(true),
        "{name} should declare idempotent behavior: {tool}"
    );
    assert!(
        contains_bool_recursive(safety, &["localOnly", "local_only"], true)
            && contains_bool_recursive(safety, &["openWorld", "open_world"], false),
        "{name} should declare local-only closed-world behavior: {tool}"
    );
    assert_eq!(
        safety.get("mutation").and_then(Value::as_str),
        Some("local_plugin_configuration"),
        "{name} should label the plugin-local mutation: {tool}"
    );
}

#[test]
fn initialize_preserves_id_and_reports_server_info_and_capabilities() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "contract-test", "version": "0"}
            }
        }),
    );

    let result = assert_success_envelope(&response, json!("init-1"));
    assert_eq!(
        result.get("protocolVersion"),
        Some(&json!("2024-11-05")),
        "initialize should echo the requested protocol version: {response}"
    );
    assert!(
        result
            .pointer("/serverInfo/name")
            .or_else(|| result.pointer("/name"))
            .and_then(Value::as_str)
            .is_some_and(|name| name == "codestory"),
        "initialize should report codestory server info: {response}"
    );
    assert_eq!(
        result.get("version"),
        Some(&json!(env!("CARGO_PKG_VERSION"))),
        "initialize top-level version should match the CLI package version: {response}"
    );
    assert_eq!(
        result.pointer("/serverInfo/version"),
        Some(&json!(env!("CARGO_PKG_VERSION"))),
        "initialize serverInfo version should match the CLI package version: {response}"
    );
    assert!(
        result.get("capabilities").is_some(),
        "initialize should report server capabilities: {response}"
    );
}

#[test]
fn stdio_starts_without_prebuilt_index_and_reports_status() {
    let fixture = unindexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let init = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "init-unindexed",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "contract-test", "version": "0"}
            }
        }),
    );
    assert_success_envelope(&init, json!("init-unindexed"));

    let status_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-unindexed",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let status_result = assert_success_envelope(&status_response, json!("status-unindexed"));
    let status = json_resource_content(status_result, "codestory://status");

    assert_eq!(status["readiness"][0]["status"], json!("ready"));
    assert_allowed_surface(&status, "ground", true, "local_navigation", "ready");
    assert_allowed_surface(&status, "search", false, "agent_packet_search", "blocked");
}

#[test]
fn notification_messages_do_not_produce_responses() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    writeln!(
        server.stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .expect("write initialized notification");
    server
        .stdin
        .flush()
        .expect("flush initialized notification");

    let response = send_json(
        &mut server,
        json!({"jsonrpc": "2.0", "id": "after-notification", "method": "tools/list"}),
    );

    let result = assert_success_envelope(&response, json!("after-notification"));
    assert!(
        result["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "the next request should receive the first response after a notification: {response}"
    );
}

#[test]
fn tool_catalog_keeps_stable_browser_and_setup_tool_names() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let tools = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": "catalog-tools", "method": "tools/list"}),
        ),
        json!("catalog-tools"),
    )
    .clone();

    let tool_names = sorted_field_values(&tools, "tools", "name");
    assert_eq!(
        tool_names,
        vec![
            "affected",
            "callees",
            "callers",
            "context",
            "definition",
            "files",
            "get_node",
            "ground",
            "neighbors",
            "packet",
            "query_subgraph",
            "references",
            "repair_all",
            "search",
            "shortest_path",
            "sidecar_setup",
            "snippet",
            "symbol",
            "symbols",
            "trace",
            "trail",
        ],
        "stdio browser/setup tool names should stay stable: {tools}"
    );
    assert!(
        !tool_names.iter().any(|name| name.starts_with("codestory_")),
        "stdio tool names should stay agent-facing and avoid shell/file mutation surfaces: {tool_names:?}"
    );
    let packet_description = tool_by_name(&tools, "packet")["description"]
        .as_str()
        .expect("packet description");
    assert!(
        packet_description.contains("broad structural questions")
            && packet_description.contains("graph/sidecar evidence")
            && packet_description.contains("truncation")
            && packet_description.contains("follow-up commands")
            && packet_description.contains("before source snippets"),
        "packet description should route broad questions to proof-bearing packet evidence first: {packet_description}"
    );
    let search_description = tool_by_name(&tools, "search")["description"]
        .as_str()
        .expect("search description");
    assert!(
        search_description.contains("Discover candidate")
            && search_description.contains("packet before snippet/source reads"),
        "search description should label discovery before source proof reads: {search_description}"
    );
    let ground_description = tool_by_name(&tools, "ground")["description"]
        .as_str()
        .expect("ground description");
    assert!(
        ground_description.contains("compact repository map")
            && ground_description.contains("before packet/search")
            && ground_description.contains("codestory://grounding"),
        "ground description should connect the tool to orientation and the grounding resource: {ground_description}"
    );
    let files_description = tool_by_name(&tools, "files")["description"]
        .as_str()
        .expect("files description");
    assert!(
        files_description.contains("indexed files")
            && files_description.contains("locally fresh index")
            && files_description.contains("refreshes local graph")
            && files_description.contains("never bootstraps sidecars"),
        "files description should make the local-refresh boundary explicit: {files_description}"
    );
    let affected_description = tool_by_name(&tools, "affected")["description"]
        .as_str()
        .expect("affected description");
    assert!(
        affected_description.contains("changed paths")
            && affected_description.contains("locally fresh index")
            && affected_description.contains("never discovers git changes")
            && affected_description.contains("refreshes local graph")
            && affected_description.contains("never bootstraps sidecars"),
        "affected description should make the explicit-path local-refresh boundary clear: {affected_description}"
    );
    let snippet_description = tool_by_name(&tools, "snippet")["description"]
        .as_str()
        .expect("snippet description");
    assert!(
        snippet_description.contains("after packet, search, or graph evidence"),
        "snippet description should not be the first stop for broad structural questions: {snippet_description}"
    );

    for tool in tools["tools"].as_array().expect("tools array") {
        let name = tool["name"].as_str().expect("tool name");
        if matches!(name, "repair_all" | "sidecar_setup") {
            assert_local_plugin_mutation_tool_metadata(tool);
        } else {
            assert_read_only_tool_metadata(tool);
        }
        let looks_like_write_or_system_tool = [
            "write", "edit", "delete", "remove", "create", "update", "patch", "open_", "launch",
            "shell", "exec", "system", "fs.",
        ]
        .iter()
        .any(|needle| name.contains(needle));
        assert!(
            !looks_like_write_or_system_tool || has_safety_metadata(tool),
            "write/system-looking tool {name} must include explicit safety metadata before it can appear in the read-only catalog: {tool}"
        );
    }
}

#[test]
fn tool_catalog_input_schemas_capture_stable_arguments() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let tools = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": "catalog-inputs", "method": "tools/list"}),
        ),
        json!("catalog-inputs"),
    )
    .clone();

    let search = tool_input_schema(&tools, "search");
    assert_eq!(
        search["type"], "object",
        "search schema should be object: {search}"
    );
    assert!(
        required_fields(search).contains("query"),
        "search.query should be required: {search}"
    );
    assert_eq!(
        schema_property(search, "query")["type"],
        "string",
        "search.query should be a string: {search}"
    );
    let repo_text = schema_property(search, "repo_text");
    assert_schema_enum_values(search, "/properties/repo_text/enum", &["auto", "off", "on"]);
    assert_eq!(
        repo_text.get("default"),
        Some(&json!("auto")),
        "search.repo_text should default to auto: {search}"
    );
    let search_limit = schema_property(search, "limit");
    assert!(
        matches!(search_limit["type"].as_str(), Some("integer" | "number")),
        "search.limit should be numeric: {search}"
    );
    assert_eq!(
        search_limit.get("default"),
        Some(&json!(10)),
        "search.limit should document the stdio default: {search}"
    );
    assert_eq!(
        search_limit.get("minimum"),
        Some(&json!(1)),
        "search.limit should document a lower bound: {search}"
    );
    assert_eq!(
        search_limit.get("maximum"),
        Some(&json!(50)),
        "search.limit should document the bounded default search page: {search}"
    );

    let packet = tool_input_schema(&tools, "packet");
    assert_eq!(
        packet["type"], "object",
        "packet schema should be object: {packet}"
    );
    assert!(
        required_fields(packet).contains("question"),
        "packet.question should be required: {packet}"
    );
    assert_eq!(
        schema_property(packet, "question")["type"],
        "string",
        "packet.question should be a string: {packet}"
    );
    assert_schema_enum_values(
        packet,
        "/properties/budget/enum",
        &["tiny", "compact", "standard", "deep"],
    );
    assert_eq!(
        schema_property(packet, "budget").get("default"),
        Some(&json!("compact")),
        "packet.budget should document the stdio default: {packet}"
    );
    assert_schema_enum_values(
        packet,
        "/properties/task_class/enum",
        &[
            "architecture_explanation",
            "bug_localization",
            "change_impact",
            "route_tracing",
            "symbol_ownership",
            "data_flow",
            "edit_planning",
        ],
    );
    assert_eq!(
        schema_property(packet, "include_evidence").get("default"),
        Some(&json!(true)),
        "packet.include_evidence should document the stdio default: {packet}"
    );

    let sidecar_setup = tool_input_schema(&tools, "sidecar_setup");
    assert_schema_enum_values(
        sidecar_setup,
        "/properties/action/enum",
        &["status", "enable", "disable", "ask", "repair"],
    );
    assert_eq!(
        schema_property(sidecar_setup, "action").get("default"),
        Some(&json!("status")),
        "sidecar_setup.action should default to the cheap status probe: {sidecar_setup}"
    );

    let ground = tool_input_schema(&tools, "ground");
    assert_schema_enum_values(
        ground,
        "/properties/budget/enum",
        &["strict", "balanced", "max"],
    );
    assert_eq!(
        schema_property(ground, "budget").get("default"),
        Some(&json!("balanced")),
        "ground.budget should document the stdio default: {ground}"
    );

    let files = tool_input_schema(&tools, "files");
    assert_eq!(
        schema_property(files, "path")["type"],
        "string",
        "files.path should be a string filter: {files}"
    );
    assert_eq!(
        schema_property(files, "language")["type"],
        "string",
        "files.language should be a string filter: {files}"
    );
    assert_schema_enum_values(
        files,
        "/properties/role/enum",
        &["source", "test", "generated", "vendor", "unknown"],
    );
    let files_limit = schema_property(files, "limit");
    assert_eq!(
        files_limit.get("default"),
        Some(&json!(500)),
        "files.limit should document the CLI-backed default: {files}"
    );
    assert_eq!(
        files_limit.get("maximum"),
        Some(&json!(5000)),
        "files.limit should document the runtime clamp: {files}"
    );

    let affected = tool_input_schema(&tools, "affected");
    assert_eq!(
        schema_property(affected, "changed_paths")["type"],
        "array",
        "affected.changed_paths should be an array: {affected}"
    );
    assert_eq!(
        schema_property(affected, "changed_paths")["items"]["type"],
        "string",
        "affected.changed_paths should contain strings: {affected}"
    );
    let change_records = schema_property(affected, "change_records");
    assert_eq!(
        change_records["type"], "array",
        "affected.change_records should be an array: {affected}"
    );
    let change_record = change_records
        .get("items")
        .unwrap_or_else(|| panic!("change_records should describe item schema: {affected}"));
    assert!(
        required_fields(change_record).contains("path")
            && required_fields(change_record).contains("kind"),
        "affected.change_records should require path and kind: {affected}"
    );
    assert_schema_enum_values(
        change_record,
        "/properties/kind/enum",
        &[
            "added",
            "modified",
            "deleted",
            "renamed",
            "copied",
            "untracked",
            "unknown",
        ],
    );
    let affected_depth = schema_property(affected, "depth");
    assert_eq!(
        affected_depth.get("default"),
        Some(&json!(2)),
        "affected.depth should document the runtime default: {affected}"
    );
    assert_eq!(
        affected_depth.get("minimum"),
        Some(&json!(1)),
        "affected.depth should document the lower bound: {affected}"
    );
    assert_eq!(
        affected_depth.get("maximum"),
        Some(&json!(8)),
        "affected.depth should document the runtime clamp: {affected}"
    );
    assert_eq!(
        schema_property(affected, "filter")["type"],
        "string",
        "affected.filter should be a string: {affected}"
    );
    let affected_any_of = affected["anyOf"].as_array().unwrap_or_else(|| {
        panic!("affected should require paths or records via anyOf: {affected}")
    });
    assert!(
        affected_any_of
            .iter()
            .any(|branch| required_fields(branch).contains("changed_paths"))
            && affected_any_of
                .iter()
                .any(|branch| required_fields(branch).contains("change_records")),
        "affected should require explicit changed_paths or change_records: {affected}"
    );

    for name in ["symbol", "definition", "references", "snippet"] {
        let schema = tool_input_schema(&tools, name);
        let required = required_fields(schema);
        assert!(
            !required.contains("query") && !required.contains("id"),
            "{name} should allow either query or id without requiring both: {schema}"
        );
        assert_eq!(
            schema_property(schema, "query")["type"],
            "string",
            "{name}.query should be a string: {schema}"
        );
        assert_eq!(
            schema_property(schema, "id")["type"],
            "string",
            "{name}.id should be a string node id: {schema}"
        );
        assert!(
            schema_property(schema, "choose").get("minimum").is_some(),
            "{name}.choose should document the 1-based lower bound: {schema}"
        );
    }

    let symbols = tool_input_schema(&tools, "symbols");
    let symbols_limit = schema_property(symbols, "limit");
    assert!(
        matches!(symbols_limit["type"].as_str(), Some("integer" | "number")),
        "symbols.limit should be numeric: {symbols}"
    );
    assert_eq!(
        symbols_limit.get("default"),
        Some(&json!(300)),
        "symbols.limit should document the root-symbol browse default: {symbols}"
    );
    assert_eq!(
        symbols_limit.get("minimum"),
        Some(&json!(1)),
        "symbols.limit should document a lower bound: {symbols}"
    );
    assert_eq!(
        symbols_limit.get("maximum"),
        Some(&json!(2000)),
        "symbols.limit should document the stdio hard cap: {symbols}"
    );

    let trail = tool_input_schema(&tools, "trail");
    assert!(
        !required_fields(trail).contains("query") && !required_fields(trail).contains("id"),
        "trail should allow either query or id without requiring both: {trail}"
    );
    assert_eq!(schema_property(trail, "id")["type"], "string");
    assert!(
        schema_property(trail, "choose").get("minimum").is_some(),
        "trail.choose should document the 1-based lower bound: {trail}"
    );
    assert_schema_enum_values(
        trail,
        "/properties/direction/enum",
        &["both", "incoming", "outgoing"],
    );
    assert_eq!(
        schema_property(trail, "direction").get("default"),
        Some(&json!("both")),
        "trail.direction should document the stdio default: {trail}"
    );
    assert_eq!(
        schema_property(trail, "depth").get("default"),
        Some(&json!(2)),
        "trail.depth should document the stdio default: {trail}"
    );
    assert_eq!(
        schema_property(trail, "max_nodes").get("maximum"),
        Some(&json!(120)),
        "trail.max_nodes should document the stdio hard cap: {trail}"
    );
    assert_eq!(
        schema_property(trail, "story")["type"],
        "boolean",
        "trail.story should be a boolean opt-in: {trail}"
    );
    assert_eq!(
        schema_property(trail, "story").get("default"),
        Some(&json!(false)),
        "trail.story should document the stdio default: {trail}"
    );
    for name in ["callers", "callees"] {
        let alias = tool_input_schema(&tools, name);
        assert_eq!(
            schema_property(alias, "depth").get("default"),
            Some(&json!(1)),
            "{name}.depth should document the bounded alias default: {alias}"
        );
        assert_eq!(
            schema_property(alias, "max_nodes").get("maximum"),
            Some(&json!(120)),
            "{name}.max_nodes should document the stdio hard cap: {alias}"
        );
    }
    let trace = tool_input_schema(&tools, "trace");
    assert_eq!(
        schema_property(trace, "story").get("default"),
        Some(&json!(true)),
        "trace.story should default to readable output: {trace}"
    );
    assert_eq!(
        schema_property(trace, "max_nodes").get("maximum"),
        Some(&json!(120)),
        "trace.max_nodes should document the stdio hard cap: {trace}"
    );

    let context = tool_input_schema(&tools, "context");
    assert!(
        !required_fields(context).contains("query")
            && !required_fields(context).contains("id")
            && !required_fields(context).contains("bookmark"),
        "context should require exactly one target through anyOf rather than a single prompt: {context}"
    );
    assert_eq!(
        schema_property(context, "query")["type"],
        "string",
        "context.query should be a string: {context}"
    );
    assert_eq!(
        schema_property(context, "id")["type"],
        "string",
        "context.id should be a string node id: {context}"
    );
    assert_eq!(
        schema_property(context, "bookmark")["type"],
        "string",
        "context.bookmark should be a string bookmark id: {context}"
    );
    assert_eq!(
        schema_property(context, "max_results").get("default"),
        Some(&json!(8)),
        "context.max_results should document the stdio default: {context}"
    );
}

#[test]
fn tool_catalog_exposes_output_schemas_for_stable_dto_backed_tools() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let tools = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": "catalog-outputs", "method": "tools/list"}),
        ),
        json!("catalog-outputs"),
    )
    .clone();

    for name in [
        "affected",
        "callees",
        "callers",
        "context",
        "definition",
        "files",
        "ground",
        "packet",
        "references",
        "search",
        "snippet",
        "symbol",
        "symbols",
        "trace",
        "trail",
    ] {
        let tool = tool_by_name(&tools, name);
        let output_schema = tool
            .get("outputSchema")
            .unwrap_or_else(|| panic!("{name} should expose outputSchema: {tool}"));
        assert_eq!(
            output_schema["type"], "object",
            "{name} outputSchema should describe the stdio result shape: {tool}"
        );
        if name == "symbols" {
            assert_eq!(
                schema_property(output_schema, "symbols")["type"],
                "array",
                "symbols outputSchema should wrap symbol arrays in an object: {tool}"
            );
        }
        if name == "context" {
            assert_eq!(
                schema_property(output_schema, "packet_id")["type"],
                "string",
                "context outputSchema should expose context packet terminology: {tool}"
            );
            assert_eq!(
                schema_property(output_schema, "target")["type"],
                "string",
                "context outputSchema should expose a resolved target label: {tool}"
            );
            assert!(
                output_schema.pointer("/properties/answer_id").is_none()
                    && output_schema.pointer("/properties/prompt").is_none(),
                "context outputSchema should not expose answer/prompt DTO names: {tool}"
            );
        }
        if name == "packet" {
            assert_eq!(
                schema_property(output_schema, "packet_id")["type"],
                "string",
                "packet outputSchema should expose a stable packet id: {tool}"
            );
            for field in [
                "plan",
                "answer",
                "budget",
                "sufficiency",
                "retrieval_trace_summary",
            ] {
                assert!(
                    required_fields(output_schema).contains(field),
                    "packet outputSchema should require {field}: {tool}"
                );
            }
        }
        if name == "ground" {
            assert_eq!(
                schema_property(output_schema, "root")["type"],
                "string",
                "ground outputSchema should expose the project root: {tool}"
            );
            assert_schema_enum_values(
                output_schema,
                "/properties/budget/enum",
                &["strict", "balanced", "max"],
            );
            for field in ["stats", "coverage", "root_symbols", "files"] {
                assert!(
                    required_fields(output_schema).contains(field),
                    "ground outputSchema should require grounding DTO field {field}: {tool}"
                );
            }
        }
        if name == "files" {
            for field in ["project_root", "usable", "summary", "files"] {
                assert!(
                    output_schema["anyOf"]
                        .as_array()
                        .is_some_and(|any_of| any_of
                            .iter()
                            .any(|branch| required_fields(branch).contains(field))),
                    "files outputSchema should accept successful DTO field {field}: {tool}"
                );
            }
            let file_schema = output_schema
                .pointer("/properties/files/items")
                .unwrap_or_else(|| panic!("files outputSchema should describe file rows: {tool}"));
            assert_eq!(
                schema_property(file_schema, "path")["type"],
                "string",
                "file rows should expose project-relative paths: {tool}"
            );
            assert_schema_enum_values(
                file_schema,
                "/properties/role/enum",
                &["source", "test", "generated", "vendor", "unknown"],
            );
        }
        if name == "affected" {
            for field in [
                "project_root",
                "changed_paths",
                "change_records",
                "matched_files",
                "matched_file_count",
                "depth",
                "impacted_symbols",
                "impacted_tests",
            ] {
                assert!(
                    output_schema["anyOf"]
                        .as_array()
                        .is_some_and(|any_of| any_of
                            .iter()
                            .any(|branch| required_fields(branch).contains(field))),
                    "affected outputSchema should accept successful DTO field {field}: {tool}"
                );
            }
            assert_eq!(
                schema_property(output_schema, "changed_paths")["items"]["type"],
                "string",
                "affected outputSchema should expose changed path strings: {tool}"
            );
            let record_schema = output_schema
                .pointer("/properties/change_records/items")
                .unwrap_or_else(|| {
                    panic!("affected outputSchema should describe change records: {tool}")
                });
            assert_schema_enum_values(
                record_schema,
                "/properties/kind/enum",
                &[
                    "added",
                    "modified",
                    "deleted",
                    "renamed",
                    "copied",
                    "untracked",
                    "unknown",
                ],
            );
        }
    }

    let search_hit_schema = tool_output_schema(&tools, "search")
        .pointer("/properties/hits/items")
        .unwrap_or_else(|| panic!("search outputSchema should describe hit items: {tools}"));
    let search_output_schema = tool_output_schema(&tools, "search");
    assert_eq!(
        schema_property(search_output_schema, "search_plan")["type"],
        json!(["object", "null"]),
        "search outputSchema should allow optional SearchPlan DTOs: {search_output_schema}"
    );
    assert_eq!(
        schema_property(search_output_schema, "retrieval_shadow")["type"],
        json!(["object", "null"]),
        "search outputSchema should expose optional retrieval_shadow DTOs: {search_output_schema}"
    );
    assert!(
        schema_property(search_output_schema, "code")["type"] == "string"
            && schema_property(search_output_schema, "message")["type"] == "string",
        "search outputSchema should also admit typed API errors returned as tool errors: {search_output_schema}"
    );
    assert!(
        required_fields(search_output_schema).is_empty(),
        "search outputSchema should not globally require success-only fields because tool errors reuse the same outputSchema: {search_output_schema}"
    );
    assert!(
        search_output_schema["anyOf"]
            .as_array()
            .is_some_and(|any_of| {
                any_of
                    .iter()
                    .any(|branch| required_fields(branch).contains("code"))
                    && any_of
                        .iter()
                        .any(|branch| required_fields(branch).contains("query"))
            }),
        "search outputSchema should accept either search results or typed API errors: {search_output_schema}"
    );
    assert!(
        !required_fields(search_hit_schema).contains("match_quality"),
        "SearchHit.match_quality is optional and must not be required: {search_hit_schema}"
    );
    assert_eq!(
        schema_property(search_hit_schema, "match_quality")["type"],
        "string",
        "SearchHit outputSchema should still advertise optional match_quality: {search_hit_schema}"
    );

    let related_hit_schema = tool_output_schema(&tools, "symbol")
        .pointer("/properties/related_hits/items")
        .unwrap_or_else(|| {
            panic!("symbol outputSchema should describe related hit items: {tools}")
        });
    assert!(
        !required_fields(related_hit_schema).contains("match_quality"),
        "symbol related hits reuse SearchHit and must tolerate omitted match_quality: {related_hit_schema}"
    );

    let snippet = tool_output_schema(&tools, "snippet");
    for field in ["scope", "requested_context", "snippet_truncated"] {
        assert!(
            required_fields(snippet).contains(field),
            "snippet outputSchema should require emitted DTO field {field}: {snippet}"
        );
        let _ = schema_property(snippet, field);
    }
    assert_schema_enum_values(
        snippet,
        "/properties/scope/enum",
        &["line_context", "function_body"],
    );
}

#[test]
fn resource_template_and_prompt_catalog_names_are_snapshot_stable() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let resources = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": "catalog-resources", "method": "resources/list"}),
        ),
        json!("catalog-resources"),
    )
    .clone();
    assert_eq!(
        sorted_field_values(&resources, "resources", "uri"),
        vec![
            "codestory://agent-guide",
            "codestory://grounding",
            "codestory://project",
            "codestory://status",
            "codestory://symbols/root",
        ],
        "resource catalog should stay compact and stable: {resources}"
    );

    let templates = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": "catalog-templates", "method": "resources/templates/list"}),
        ),
        json!("catalog-templates"),
    )
    .clone();
    assert_eq!(
        sorted_field_values(&templates, "resourceTemplates", "uriTemplate"),
        vec![
            "codestory://references/{node_id}",
            "codestory://snippet/{node_id}",
            "codestory://symbol/{node_id}",
            "codestory://trail/{node_id}",
        ],
        "resource template catalog should stay compact and stable: {templates}"
    );

    let prompts = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": "catalog-prompts", "method": "prompts/list"}),
        ),
        json!("catalog-prompts"),
    )
    .clone();
    assert_eq!(
        sorted_field_values(&prompts, "prompts", "name"),
        vec!["explain_symbol", "impact_analysis", "trace_callflow"],
        "prompt catalog should stay compact and stable: {prompts}"
    );

    let explain_symbol = assert_success_envelope(
        &send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": "prompt-explain-symbol",
                "method": "prompts/get",
                "params": {"name": "explain_symbol"}
            }),
        ),
        json!("prompt-explain-symbol"),
    )
    .clone();
    assert_eq!(
        explain_symbol["description"],
        "Explain a symbol using definition, references, and snippet context.",
        "prompts/get should return the human prompt description: {explain_symbol}"
    );
}

#[test]
fn transcript_lists_tools_resources_templates_and_prompts() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let tools = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        ),
        json!(1),
    )
    .clone();
    let tool_names: Vec<_> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for expected in [
        "ground",
        "files",
        "affected",
        "search",
        "symbol",
        "trail",
        "definition",
        "references",
        "symbols",
        "snippet",
        "context",
    ] {
        assert!(
            tool_names.contains(&expected),
            "tools/list should include {expected}: {tools}"
        );
    }

    let resources = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"}),
        ),
        json!(2),
    )
    .clone();
    assert!(
        resources["resources"]
            .as_array()
            .expect("resources array")
            .iter()
            .any(|resource| resource["uri"] == "codestory://project"),
        "resources/list should include the project resource: {resources}"
    );
    for expected in ["codestory://status", "codestory://agent-guide"] {
        assert!(
            resources["resources"]
                .as_array()
                .expect("resources array")
                .iter()
                .any(|resource| resource["uri"] == expected),
            "resources/list should include {expected}: {resources}"
        );
    }

    let templates = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": 3, "method": "resources/templates/list"}),
        ),
        json!(3),
    )
    .clone();
    assert!(
        templates["resourceTemplates"]
            .as_array()
            .expect("resource templates array")
            .iter()
            .any(|template| template["uriTemplate"] == "codestory://symbol/{node_id}"),
        "resources/templates/list should include symbol template: {templates}"
    );

    let prompts = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": 4, "method": "prompts/list"}),
        ),
        json!(4),
    )
    .clone();
    assert!(
        prompts["prompts"]
            .as_array()
            .expect("prompts array")
            .iter()
            .any(|prompt| prompt["name"] == "explain_symbol"),
        "prompts/list should include explain_symbol: {prompts}"
    );
}

#[test]
fn ground_tool_returns_budgeted_grounding_snapshot() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-strict",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"budget": "strict"}
            }
        }),
    );

    let snapshot = assert_tool_success(&response, json!("ground-strict"));
    assert_eq!(
        snapshot["budget"],
        json!("strict"),
        "ground tool should honor the requested grounding budget: {snapshot}"
    );
    assert!(
        snapshot["root"]
            .as_str()
            .is_some_and(|root| !root.is_empty())
            && snapshot
                .pointer("/stats/node_count")
                .and_then(Value::as_u64)
                > Some(0)
            && snapshot
                .pointer("/coverage/represented_files")
                .and_then(Value::as_u64)
                > Some(0),
        "ground tool should return a populated grounding snapshot: {snapshot}"
    );

    let default_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-default",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {}}
        }),
    );
    let default_snapshot = assert_tool_success(&default_response, json!("ground-default"));
    assert_eq!(
        default_snapshot["budget"],
        json!("balanced"),
        "ground tool should default to the existing grounding resource budget: {default_snapshot}"
    );

    let bad_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-bad-budget",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"budget": "huge"}
            }
        }),
    );
    let error = assert_tool_error(&bad_response, json!("ground-bad-budget"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("ground.budget")),
        "ground tool should fail closed on unknown budgets: {bad_response}"
    );
}

#[test]
fn files_tool_lists_indexed_files_without_sidecars() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "files-source",
            "method": "tools/call",
            "params": {
                "name": "files",
                "arguments": {
                    "path": "src/",
                    "language": "rust",
                    "role": "source",
                    "limit": 2
                }
            }
        }),
    );

    let result = assert_tool_success(&response, json!("files-source"));
    assert!(
        result["usable"].as_bool() == Some(true),
        "files tool should report a usable indexed fixture: {result}"
    );
    assert!(
        result
            .pointer("/summary/visible_file_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count <= 2),
        "files tool should respect the requested cap: {result}"
    );
    let files = result["files"]
        .as_array()
        .unwrap_or_else(|| panic!("files tool should return file rows: {result}"));
    assert!(
        !files.is_empty()
            && files.iter().all(|file| file["path"]
                .as_str()
                .is_some_and(|path| path.contains("src/")))
            && files.iter().all(|file| file["language"] == json!("rust"))
            && files.iter().all(|file| file["role"] == json!("source")),
        "files tool should apply path/language/role filters: {result}"
    );

    let bad_role = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "files-bad-role",
            "method": "tools/call",
            "params": {
                "name": "files",
                "arguments": {"role": "workspace"}
            }
        }),
    );
    let error = assert_tool_error(&bad_role, json!("files-bad-role"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("files.role")),
        "files tool should fail closed on unknown roles: {bad_role}"
    );
}

#[test]
fn affected_tool_maps_explicit_changed_paths_without_sidecars() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-runtime",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "changed_paths": ["src/runtime.rs"],
                    "change_records": [
                        {
                            "path": "src/runtime.rs",
                            "kind": "modified",
                            "status": "M"
                        }
                    ],
                    "depth": 2
                }
            }
        }),
    );

    let result = assert_tool_success(&response, json!("affected-runtime"));
    assert_eq!(
        result["changed_paths"],
        json!(["src/runtime.rs"]),
        "affected should preserve explicit changed paths: {result}"
    );
    assert_eq!(
        result["change_records"][0]["kind"],
        json!("modified"),
        "affected should preserve explicit change records: {result}"
    );
    assert_eq!(
        result["matched_file_count"],
        json!(1),
        "affected should match the indexed changed file: {result}"
    );
    assert_eq!(
        result["matched_files"][0]["path"],
        json!("src/runtime.rs"),
        "affected should expose matched file rows: {result}"
    );
    assert!(
        result["impacted_symbols"]
            .as_array()
            .is_some_and(|symbols| !symbols.is_empty()),
        "affected should expand matched files to impacted symbols: {result}"
    );
}

#[test]
fn affected_tool_rejects_invalid_arguments_without_transport_crash() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let bad_paths = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-bad-paths",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"changed_paths": "src/runtime.rs"}
            }
        }),
    );
    let error = assert_tool_error(&bad_paths, json!("affected-bad-paths"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("affected.changed_paths")),
        "affected should fail closed on malformed path input: {bad_paths}"
    );

    let bad_record = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-bad-record",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "change_records": [
                        {"path": "src/runtime.rs", "kind": "touched"}
                    ]
                }
            }
        }),
    );
    let error = assert_tool_error(&bad_record, json!("affected-bad-record"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("affected.change_records")),
        "affected should fail closed on malformed change records: {bad_record}"
    );
}

#[test]
fn transcript_reads_project_resource() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "project-resource",
            "method": "resources/read",
            "params": {"uri": "codestory://project"}
        }),
    );

    let result = assert_success_envelope(&response, json!("project-resource"));
    let content = result["contents"]
        .as_array()
        .expect("resource contents")
        .first()
        .expect("first resource content");
    assert_eq!(content["uri"], "codestory://project");
    assert_eq!(content["mimeType"], "application/json");
    let text = content["text"].as_str().expect("project resource text");
    let project: Value = serde_json::from_str(text).expect("project resource json text");
    assert!(
        project
            .get("project_root")
            .or_else(|| project.get("root"))
            .is_some(),
        "project resource should include a project root field: {project}"
    );
}

#[test]
fn resources_read_status_reports_browser_readiness_and_next_calls() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-resource",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );

    let result = assert_success_envelope(&response, json!("status-resource"));
    let status = json_resource_content(result, "codestory://status");
    assert_eq!(
        status["server_version"],
        json!(env!("CARGO_PKG_VERSION")),
        "status should identify the serving package version: {status}"
    );
    assert_eq!(
        status["cli_version"],
        json!(env!("CARGO_PKG_VERSION")),
        "status should identify the active CLI version: {status}"
    );
    assert!(
        status["source_checkout_version"].is_null()
            || status["source_checkout_version"]
                .as_str()
                .is_some_and(|version| !version.is_empty()),
        "status should distinguish source checkout version from active runtime version: {status}"
    );
    assert!(
        status["sidecar_contract_version"].is_number(),
        "status should expose the sidecar contract version: {status}"
    );
    assert!(
        status["sidecar_retrieval"]["sidecar_contract_version"].is_number(),
        "sidecar status should expose the sidecar contract version: {status}"
    );
    assert!(
        status["server_executable"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
            || status["warnings"]
                .as_array()
                .is_some_and(|warnings| !warnings.is_empty()),
        "status should expose server_executable or an explicit warning: {status}"
    );
    assert!(
        status["server_executable_sha256"]
            .as_str()
            .is_some_and(|sha256| sha256.len() == 64),
        "status should expose the active server executable checksum: {status}"
    );
    assert_eq!(
        status["runtime_boundary"]["restart_required_for_runtime_change"],
        json!(true),
        "status should make the MCP restart boundary explicit: {status}"
    );
    assert_eq!(
        status["plugin_runtime"]["cli_source"],
        json!("direct_cli_launch"),
        "direct cargo stdio tests should label the non-plugin launch boundary: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["runtime_source"],
        json!("direct_cli_launch"),
        "runtime truth should group the launch source classification: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["launcher_source"], status["plugin_runtime"]["cli_source"],
        "runtime truth should reuse plugin runtime launch evidence: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["sidecar_policy"],
        json!("unmanaged"),
        "direct stdio status should make unmanaged sidecar policy explicit: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["sidecar_status"]["mode"],
        status["readiness_lanes"]["agent_packet_search"]["sidecar_mode"],
        "runtime truth should reuse the selected agent readiness lane mode: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["sidecar_status"]["degraded_reason"],
        status["readiness_lanes"]["agent_packet_search"]["degraded_reason"],
        "runtime truth should reuse the selected agent readiness lane reason: {status}"
    );
    assert!(
        status
            .get("project_root")
            .or_else(|| status.get("root"))
            .and_then(Value::as_str)
            .is_some_and(|root| !root.is_empty()),
        "status should include project root: {status}"
    );
    assert!(
        contains_key_recursive(
            &status,
            &["cache_path", "cache_dir", "storage_path", "storage"]
        ),
        "status should include cache/storage path information: {status}"
    );
    assert!(
        contains_key_recursive(&status, &["retrieval_mode", "retrieval"])
            || contains_bool_recursive(&status, &["not_ready", "notReady"], true),
        "status should include retrieval mode or an explicit not-ready state: {status}"
    );
    assert_ne!(
        status["retrieval_mode"], "full",
        "hash-mode indexed fixture must not report mandatory sidecar retrieval as full: {status}"
    );
    assert_eq!(
        status["local_refresh"]["state"],
        json!("refreshed"),
        "fresh local graph state should be explicit even when sidecar retrieval is unavailable: {status}"
    );
    assert_eq!(
        status["local_refresh"]["blocks_local_surfaces"],
        json!(false),
        "fresh local graph state should not block local graph surfaces: {status}"
    );
    assert!(
        status["sidecar_retrieval"]["retrieval_mode"].is_string(),
        "status should expose sidecar retrieval diagnostics: {status}"
    );
    assert_eq!(
        status["legacy_semantic_diagnostics"]["diagnostic_only"],
        json!(true),
        "legacy semantic readiness should be nested as diagnostic-only: {status}"
    );
    assert!(
        contains_key_recursive(
            &status,
            &[
                "semantic",
                "semantic_readiness",
                "semantic_ready",
                "semantic_doc_count",
                "doc_count",
                "fallback",
                "fallback_reason",
            ],
        ),
        "status should include semantic readiness/doc count/fallback information: {status}"
    );
    let next_call_text = status["recommended_next_calls"].to_string();
    let readiness = status["readiness"]
        .as_array()
        .unwrap_or_else(|| panic!("status should include readiness verdicts: {status}"));
    let readiness_lanes = status["readiness_lanes"]
        .as_object()
        .unwrap_or_else(|| panic!("status should include readiness lanes: {status}"));
    let local_default = readiness_lanes
        .get("local_default")
        .unwrap_or_else(|| panic!("status should include local_default lane: {status}"));
    assert_eq!(
        local_default["profile"],
        json!("local"),
        "local/default lane should report the local profile: {status}"
    );
    assert!(
        local_default["sidecar_mode"].is_string(),
        "local/default lane should report sidecar mode: {status}"
    );
    assert!(
        local_default["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("--profile local")),
        "local/default lane should expose a local-scoped next command: {status}"
    );
    let agent_lane = readiness_lanes
        .get("agent_packet_search")
        .unwrap_or_else(|| panic!("status should include agent_packet_search lane: {status}"));
    assert_eq!(
        agent_lane["status"],
        json!("blocked"),
        "agent lane should report blocked packet/search readiness: {status}"
    );
    assert_eq!(
        agent_lane["profile"],
        json!("agent"),
        "agent lane must not collapse to local when no agent run exists: {status}"
    );
    assert!(
        agent_lane["run_id"]
            .as_str()
            .is_some_and(|run_id| !run_id.is_empty()),
        "agent lane should report a non-empty agent run id: {status}"
    );
    assert!(
        agent_lane["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("ready --goal agent --repair")),
        "agent lane should expose the agent-scoped next command: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["readiness_lanes"]["local_graph"]["status"],
        json!("ready"),
        "runtime truth should include the local graph readiness lane: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["readiness_lanes"]["local_graph"]["refresh_state"],
        json!("refreshed"),
        "runtime truth should include local refresh state: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["readiness_lanes"]["agent_packet_search"]["status"],
        json!("blocked"),
        "runtime truth should include the agent packet/search readiness lane: {status}"
    );
    for surface in [
        "ground",
        "files",
        "symbol",
        "definition",
        "get_node",
        "callers",
        "callees",
        "neighbors",
        "shortest_path",
        "query_subgraph",
        "symbols",
        "trace",
        "trail",
        "references",
        "snippet",
        "affected",
    ] {
        assert_allowed_surface(&status, surface, true, "local_navigation", "ready");
    }
    for surface in ["packet", "search", "context"] {
        assert_allowed_surface(&status, surface, false, "agent_packet_search", "blocked");
        assert_eq!(
            status
                .pointer(&format!("/allowed_surfaces/{surface}/repair_reason"))
                .and_then(Value::as_str),
            Some("retrieval_manifest_missing"),
            "blocked agent surface should expose typed sidecar repair reason: {status}"
        );
    }
    assert!(
        readiness
            .iter()
            .any(|verdict| verdict["goal"] == "agent_packet_search"
                && verdict["minimum_next"]
                    .as_array()
                    .is_some_and(|commands| !commands.is_empty())
                && verdict["full_repair"]
                    .as_array()
                    .is_some_and(|commands| !commands.is_empty())),
        "status should expose agent readiness with minimum_next/full_repair: {status}"
    );
    assert!(
        !next_call_text.contains("\"tool\":\"packet\"")
            && !next_call_text.contains("\"tool\":\"search\""),
        "status should recommend repair, not packet/search calls, when mode is not full: {status}"
    );
    assert!(
        !next_call_text.contains("codestory-cli index --project")
            && !next_call_text.contains("retrieval bootstrap")
            && !next_call_text.contains("retrieval index")
            && !next_call_text.contains("\"tool\":\"repair_all\"")
            && next_call_text.contains("codestory://status")
            && next_call_text.contains("codestory://agent-guide")
            && next_call_text.contains("not persisted for this host"),
        "status should block MCP sidecar repair without repeating a fresh core index when mode is not full and policy is unmanaged: {status}"
    );
    assert!(
        !next_call_text.contains("\"method\":\"cli\""),
        "status should not expose CLI as the normal user-facing repair method: {status}"
    );
    assert!(
        status
            .get("recommended_next_calls")
            .or_else(|| status.get("recommended_calls"))
            .or_else(|| status.get("next_calls"))
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty()),
        "status should include recommended next calls: {status}"
    );
}

#[test]
fn resources_read_status_reports_active_agent_repair_phase() {
    let fixture = indexed_fixture();
    let (status_path, _cleanup) =
        write_active_repair_status_fixture(&fixture, "issue-661-proof", "Qdrant finalize");
    assert!(status_path.exists(), "repair status fixture should exist");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-repairing",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );

    let result = assert_success_envelope(&response, json!("status-repairing"));
    let status = json_resource_content(result, "codestory://status");
    let agent_lane = &status["readiness_lanes"]["agent_packet_search"];
    assert_eq!(agent_lane["status"], json!("repairing"), "{status}");
    assert_eq!(agent_lane["profile"], json!("agent"), "{status}");
    assert_eq!(agent_lane["run_id"], json!("issue-661-proof"), "{status}");
    assert_eq!(agent_lane["phase"], json!("Qdrant finalize"), "{status}");
    assert!(
        agent_lane["namespace"]
            .as_str()
            .is_some_and(|namespace| namespace.contains("issue-661-proof")),
        "repairing status should include the active namespace: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["readiness_lanes"]["agent_packet_search"]["status"],
        json!("repairing"),
        "{status}"
    );
    assert_eq!(
        status["runtime_truth"]["readiness_lanes"]["agent_packet_search"]["phase"],
        json!("Qdrant finalize"),
        "{status}"
    );
    assert_eq!(
        status["runtime_truth"]["sidecar_status"]["phase"],
        json!("Qdrant finalize"),
        "{status}"
    );
    assert!(
        agent_lane["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval status")
                && command.contains("--profile agent")
                && command.contains("--run-id")
                && command.contains("issue-661-proof")),
        "repairing lane should point at status proof, not a second repair: {status}"
    );
}

#[test]
fn resources_read_status_reports_abandoned_agent_repair_actions() {
    let fixture = indexed_fixture();
    let (status_path, _cleanup) =
        write_abandoned_repair_status_fixture(&fixture, "aborted-run", "Embedding documents");
    assert!(status_path.exists(), "repair status fixture should exist");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-abandoned-repair",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-abandoned-repair"));
    let status = json_resource_content(result, "codestory://status");
    assert_eq!(status["sidecar_setup"]["active_repair"], Value::Null);
    assert_eq!(
        status["sidecar_setup"]["abandoned_repair"]["status"],
        json!("abandoned"),
        "{status}"
    );
    assert_eq!(
        status["sidecar_setup"]["abandoned_repair"]["run_id"],
        json!("aborted-run"),
        "{status}"
    );
    assert!(
        status["sidecar_setup"]["abandoned_repair"]["inspect_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval status")
                && command.contains("--run-id")
                && command.contains("aborted-run")),
        "abandoned repair should include a bounded inspect command: {status}"
    );
    assert!(
        status["sidecar_setup"]["abandoned_repair"]["cleanup_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval down")
                && command.contains("--run-id")
                && command.contains("aborted-run")),
        "abandoned repair should include an explicit cleanup command: {status}"
    );

    // The explicit MCP repair action retries past abandoned records. Do not call
    // it from this contract test: it intentionally launches a real repair
    // worker, which belongs in the live MCP proof lane rather than the cheap
    // status-shape suite.
}

#[test]
fn resources_read_status_prompts_before_sidecar_repair_when_policy_is_ask() {
    let mut fixture = indexed_fixture();
    fixture.sidecar_policy_state = Some("ask".to_string());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-sidecar-ask",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-sidecar-ask"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["sidecar_setup"]["state"], json!("ask"));
    assert_eq!(status["sidecar_setup"]["prompt_required"], json!(true));
    assert_eq!(status["sidecar_setup"]["auto_repair"], json!(false));
    assert_eq!(
        status["sidecar_setup"]["repair_mode"],
        json!("consent_required")
    );
    assert!(
        status["sidecar_setup"]["prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("may start or download retrieval sidecars")),
        "{status}"
    );
    let next_call_text = status["recommended_next_calls"].to_string();
    assert!(next_call_text.contains("host/confirm"), "{status}");
    assert!(
        next_call_text.contains("\"tool\":\"sidecar_setup\""),
        "{status}"
    );
    assert!(next_call_text.contains("\"action\":\"enable\""), "{status}");
    assert!(
        next_call_text.contains("\"action\":\"disable\""),
        "{status}"
    );
    assert!(
        next_call_text.contains("confirm_next")
            && next_call_text.contains("\"tool\":\"sidecar_setup\"")
            && next_call_text.contains("\"action\":\"repair\""),
        "ask policy should include sidecar_setup repair only after consent: {status}"
    );
    assert!(
        next_call_text.contains("decline_next"),
        "ask policy should include a decline path: {status}"
    );
    assert!(
        !next_call_text.contains("\"tool\":\"repair_all\""),
        "ask policy should not recommend raw repair_all before consent: {status}"
    );
    assert!(
        !next_call_text.contains("\"method\":\"cli\""),
        "ask policy should not expose CLI as the normal consent path: {status}"
    );
}

#[test]
fn resources_read_status_blocks_unmanaged_session_repair_without_persisted_policy() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-sidecar-unmanaged",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-sidecar-unmanaged"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["sidecar_setup"]["state"], json!("unmanaged"));
    assert_eq!(
        status["sidecar_setup"]["repair_mode"],
        json!("explicit_mcp_unmanaged")
    );
    let next_call_text = status["recommended_next_calls"].to_string();
    assert!(
        next_call_text.contains("not persisted for this host"),
        "{status}"
    );
    assert!(
        !next_call_text.contains("\"tool\":\"repair_all\""),
        "unmanaged policy should block MCP repair until policy is persisted: {status}"
    );
    assert_eq!(
        status["allowed_surfaces"]["repair_all"]["status"],
        json!("repair_unmanaged"),
        "{status}"
    );
    assert!(
        next_call_text.contains("\"uri\":\"codestory://status\"")
            && next_call_text.contains("\"uri\":\"codestory://agent-guide\""),
        "{status}"
    );
}

#[test]
fn resources_read_status_recommends_explicit_repair_when_policy_enabled() {
    let mut fixture = indexed_fixture();
    fixture.sidecar_policy_state = Some("enabled".to_string());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-sidecar-enabled",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-sidecar-enabled"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["sidecar_setup"]["state"], json!("enabled"));
    assert_eq!(status["sidecar_setup"]["auto_repair"], json!(false));
    assert_eq!(
        status["sidecar_setup"]["status_triggered_repair"],
        json!(false)
    );
    assert_eq!(
        status["sidecar_setup"]["explicit_repair_enabled"],
        json!(true)
    );
    assert_eq!(
        status["sidecar_setup"]["repair_mode"],
        json!("explicit_mcp")
    );
    let sidecar_repair_command = status["sidecar_setup"]["next_repair_command"]
        .as_str()
        .expect("sidecar setup next repair command");
    assert!(
        sidecar_repair_command.contains("--run-id")
            && sidecar_repair_command.contains("shared-agent"),
        "sidecar setup should point at the shared agent run id: {status}"
    );
    let next_call_text = status["recommended_next_calls"].to_string();
    assert_eq!(
        status["status_resource_auto_repair"],
        Value::Null,
        "status reads must not spawn sidecar repair: {status}"
    );
    assert!(
        next_call_text.contains("\"tool\":\"sidecar_setup\"")
            && next_call_text.contains("\"action\":\"repair\"")
            && !next_call_text.contains("\"tool\":\"repair_all\""),
        "enabled policy should recommend explicit sidecar_setup repair: {status}"
    );
    assert!(
        status["readiness_broker"]["project_id"]
            .as_str()
            .is_some_and(|project_id| project_id.starts_with("codestory-")),
        "status should expose the durable readiness broker snapshot: {status}"
    );
    assert_eq!(
        status["readiness_broker"]["resources"]["native_embedding_runtime"]["scope"],
        json!("machine"),
        "{status}"
    );
    assert!(
        next_call_text.contains("\"uri\":\"codestory://status\""),
        "enabled policy should include status readback after explicit repair: {status}"
    );
    assert!(
        !next_call_text.contains("\"method\":\"cli\""),
        "enabled policy should not expose CLI as the normal repair path: {status}"
    );
}

#[test]
fn resources_read_status_reports_abandoned_repair_without_starting_when_policy_enabled() {
    let mut fixture = indexed_fixture();
    fixture.sidecar_policy_state = Some("enabled".to_string());
    let (status_path, _cleanup) = write_abandoned_repair_status_fixture(
        &fixture,
        codestory_retrieval::DEFAULT_AGENT_RUN_ID,
        "graph artifact",
    );
    assert!(status_path.exists(), "repair status fixture should exist");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-sidecar-enabled-abandoned",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-sidecar-enabled-abandoned"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["sidecar_setup"]["state"], json!("enabled"));
    assert_eq!(
        status["sidecar_setup"]["abandoned_repair"]["status"],
        json!("abandoned"),
        "{status}"
    );
    assert_eq!(
        status["status_resource_auto_repair"],
        Value::Null,
        "status reads must not spawn repair while abandoned state is present: {status}"
    );
    assert!(
        status["recommended_next_calls"]
            .to_string()
            .contains("\"tool\":\"sidecar_setup\""),
        "enabled policy should leave repair as an explicit MCP action: {status}"
    );
    assert_eq!(
        status["readiness_broker"]["reconciliation"]["status"],
        json!("observed"),
        "{status}"
    );
}

#[test]
fn resources_read_status_suppresses_auto_repair_when_policy_disabled() {
    let mut fixture = indexed_fixture();
    fixture.sidecar_policy_state = Some("disabled".to_string());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-sidecar-disabled",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-sidecar-disabled"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["sidecar_setup"]["state"], json!("disabled"));
    assert_eq!(status["sidecar_setup"]["auto_repair"], json!(false));
    assert_eq!(status["sidecar_setup"]["repair_mode"], json!("disabled"));
    let next_call_text = status["recommended_next_calls"].to_string();
    assert!(
        next_call_text.contains("CodeStory packet/search repair is disabled"),
        "{status}"
    );
    assert!(
        next_call_text.contains("\"tool\":\"sidecar_setup\""),
        "{status}"
    );
    assert!(next_call_text.contains("\"action\":\"enable\""), "{status}");
    assert!(
        !next_call_text.contains("ready --goal agent --repair"),
        "disabled policy should not recommend sidecar repair: {status}"
    );
    assert!(
        !next_call_text.contains("\"method\":\"cli\""),
        "disabled policy should not expose CLI as the normal recovery path: {status}"
    );
}

#[test]
fn sidecar_setup_status_marks_old_last_repair_as_stale() {
    let mut fixture = indexed_fixture();
    fixture.sidecar_policy_state = Some("enabled".to_string());
    fixture.sidecar_last_repair_command = Some(
        r#""C:\\Users\\alber\\.codex\\plugins\\data\\codestory-TheGreenCedar\\codestory-cli\\0.12.3\\bin\\codestory-cli.exe" ready --goal agent --repair"#
            .to_string(),
    );
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-stale-last-repair",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-stale-last-repair"));
    let status = json_resource_content(result, "codestory://status");
    assert_eq!(
        status["sidecar_setup"]["last_repair"]["current"],
        json!(false)
    );
    assert!(
        status["sidecar_setup"]["last_repair"]["stale_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("last_repair_cli_version_mismatch")),
        "old last_repair command should be marked stale: {status}"
    );
}

#[test]
fn tools_call_sidecar_setup_updates_plugin_policy_without_cli_user_steps() {
    let mut fixture = indexed_fixture();
    fixture.sidecar_policy_state = Some("ask".to_string());
    let policy_path = fixture.cache_dir.path().join("plugin-sidecar-policy.json");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-enable",
            "method": "tools/call",
            "params": {
                "name": "sidecar_setup",
                "arguments": {"action": "enable"}
            }
        }),
    );
    let setup = assert_tool_success(&response, json!("sidecar-setup-enable"));
    assert_eq!(
        setup["state"],
        json!("enabled"),
        "sidecar_setup enable should report the updated policy state immediately: {setup}"
    );
    assert_eq!(
        setup["mcp_control"]["repair"],
        json!({"method": "tools/call", "tool": "sidecar_setup", "arguments": {"action": "repair"}}),
        "sidecar_setup should expose the MCP repair call, not a user CLI step: {setup}"
    );

    let policy: Value = serde_json::from_str(
        &fs::read_to_string(&policy_path)
            .unwrap_or_else(|error| panic!("read sidecar policy {policy_path:?}: {error}")),
    )
    .unwrap_or_else(|error| panic!("sidecar policy should be json: {error}"));
    assert_eq!(policy["state"], json!("enabled"), "{policy}");
    assert!(
        policy["updated_at"]
            .as_str()
            .is_some_and(|value| value.starts_with("unix:")),
        "sidecar policy should record an update timestamp: {policy}"
    );

    let repair_all_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "repair-all-compat",
            "method": "tools/call",
            "params": {
                "name": "repair_all",
                "arguments": {}
            }
        }),
    );
    let repair_all = assert_tool_success(&repair_all_response, json!("repair-all-compat"));
    assert_eq!(repair_all["deprecated"], json!(true));
    assert_eq!(repair_all["canonical_tool"], json!("sidecar_setup"));
    assert_eq!(
        repair_all["canonical_arguments"],
        json!({"action": "repair"})
    );
    assert_eq!(
        repair_all["mode"],
        json!("background"),
        "repair_all compatibility alias must not run foreground repair: {repair_all}"
    );

    let repair_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-repair",
            "method": "tools/call",
            "params": {
                "name": "sidecar_setup",
                "arguments": {"action": "repair"}
            }
        }),
    );
    let repair = assert_tool_success(&repair_response, json!("sidecar-setup-repair"));
    assert!(
        repair["status"] == json!("started") || repair["status"] == json!("already_running"),
        "sidecar_setup repair should return a bounded repair state: {repair}"
    );
    assert_eq!(
        repair["mode"],
        json!("background"),
        "sidecar_setup repair should not wait for full repair inside the MCP request: {repair}"
    );
    assert!(
        repair["next_status_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval status")
                && command.contains("--profile agent")
                && command.contains(codestory_retrieval::DEFAULT_AGENT_RUN_ID)),
        "sidecar_setup repair should point to cheap status inspection: {repair}"
    );
    if repair["status"] == json!("started") {
        assert!(
            repair["broker_reconciliation"]["status"]
                .as_str()
                .is_some_and(|status| matches!(status, "clean" | "abandoned_cleaned")),
            "sidecar_setup repair should reconcile broker state before spawning: {repair}"
        );
    }

    let status_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-status",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&status_response, json!("sidecar-setup-status"));
    let status = json_resource_content(result, "codestory://status");
    assert_eq!(
        status["sidecar_setup"]["state"],
        json!("enabled"),
        "{status}"
    );
    let next_call_text = status["recommended_next_calls"].to_string();
    assert_eq!(
        status["status_resource_auto_repair"],
        Value::Null,
        "status should not start another repair after explicit repair: {status}"
    );
    assert!(
        status["sidecar_setup"]["active_repair"]["status"] == json!("repairing")
            || status["readiness_broker"]["operations"]
                .as_array()
                .is_some_and(|operations| !operations.is_empty()),
        "status should report Rust-owned repair through setup or broker state after explicit repair: {status}"
    );
    assert!(
        next_call_text.contains("\"uri\":\"codestory://status\""),
        "status should recommend status readback after explicit repair: {status}"
    );
}

#[test]
fn tools_call_sidecar_setup_reports_active_shared_agent_repair_without_waiting() {
    let fixture = indexed_fixture();
    let (status_path, _cleanup) = write_active_repair_status_fixture(
        &fixture,
        codestory_retrieval::DEFAULT_AGENT_RUN_ID,
        "Qdrant finalize",
    );
    assert!(status_path.exists(), "repair status fixture should exist");
    thread::sleep(Duration::from_millis(25));
    fs::write(
        fixture.workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("active-repair:{project_name}")
}
"#,
    )
    .expect("make local graph stale while active repair is running");
    let mut server = spawn_stdio_server(&fixture);

    let status_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-status-active",
            "method": "tools/call",
            "params": {
                "name": "sidecar_setup",
                "arguments": {"action": "status"}
            }
        }),
    );
    let setup = assert_tool_success(&status_response, json!("sidecar-setup-status-active"));
    assert_eq!(
        setup["active_repair"]["status"],
        json!("repairing"),
        "sidecar_setup status should surface the active shared-agent repair: {setup}"
    );
    assert_eq!(
        setup["active_repair"]["run_id"],
        json!(codestory_retrieval::DEFAULT_AGENT_RUN_ID),
        "{setup}"
    );
    let status_resource = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-status-active-resource",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let status_result = assert_success_envelope(
        &status_resource,
        json!("sidecar-setup-status-active-resource"),
    );
    let status = json_resource_content(status_result, "codestory://status");
    assert_eq!(
        status["local_refresh"]["state"],
        json!("refreshing"),
        "active repair should compact stale local refresh chatter into a refreshing lane: {status}"
    );
    assert_eq!(
        status["local_refresh"]["reason"],
        json!("active_ready_repair:Qdrant finalize"),
        "{status}"
    );
    assert_eq!(
        status["effective_index_freshness"]["status"],
        json!("stale"),
        "maintainer JSON should still expose stale freshness detail while agent status stays compact: {status}"
    );

    let repair_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-repair-active",
            "method": "tools/call",
            "params": {
                "name": "sidecar_setup",
                "arguments": {"action": "repair"}
            }
        }),
    );
    let repair = assert_tool_success(&repair_response, json!("sidecar-setup-repair-active"));
    assert_eq!(
        repair["status"],
        json!("already_running"),
        "sidecar_setup repair should not spawn or wait when shared-agent repair is active: {repair}"
    );
    assert_eq!(
        repair["phase"],
        json!("Qdrant finalize"),
        "already-running response should preserve current repair phase: {repair}"
    );
    assert!(
        repair["next_status_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval status")
                && command.contains("--profile agent")
                && command.contains(codestory_retrieval::DEFAULT_AGENT_RUN_ID)),
        "already-running response should point to cheap status inspection: {repair}"
    );
}

#[test]
fn tools_call_sidecar_setup_reports_active_agent_repair_non_default_without_spawning_default() {
    let fixture = indexed_fixture();
    let run_id = "non-default-active";
    let (status_path, _cleanup) =
        write_active_repair_status_fixture(&fixture, run_id, "Embedding documents");
    assert!(status_path.exists(), "repair status fixture should exist");
    thread::sleep(Duration::from_millis(25));
    fs::write(
        fixture.workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("non-default-active-repair:{project_name}")
}
"#,
    )
    .expect("make local graph stale while non-default active repair is running");
    let mut server = spawn_stdio_server(&fixture);

    let status_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-status-non-default-active",
            "method": "tools/call",
            "params": {
                "name": "sidecar_setup",
                "arguments": {"action": "status"}
            }
        }),
    );
    let setup = assert_tool_success(
        &status_response,
        json!("sidecar-setup-status-non-default-active"),
    );
    assert_eq!(
        setup["active_repair"]["run_id"],
        json!(run_id),
        "sidecar_setup status should surface non-default active repair lanes: {setup}"
    );
    assert!(
        setup["next_repair_command"]
            .as_str()
            .is_some_and(|command| command.contains("ready --goal agent --repair")
                && command.contains(codestory_retrieval::DEFAULT_AGENT_RUN_ID)
                && !command.contains(run_id)),
        "normal repair command should remain shared-agent when no user action is taken: {setup}"
    );

    let status_resource = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-status-non-default-resource",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let status_result = assert_success_envelope(
        &status_resource,
        json!("sidecar-setup-status-non-default-resource"),
    );
    let status = json_resource_content(status_result, "codestory://status");
    assert_eq!(
        status["local_refresh"]["reason"],
        json!("active_ready_repair:Embedding documents"),
        "status should not start local refresh while any project agent repair is active: {status}"
    );
    assert_eq!(
        status["sidecar_setup"]["active_repair"]["run_id"],
        json!(run_id),
        "runtime truth should expose the non-default active repair: {status}"
    );

    let repair_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "sidecar-setup-repair-non-default-active",
            "method": "tools/call",
            "params": {
                "name": "sidecar_setup",
                "arguments": {"action": "repair"}
            }
        }),
    );
    let repair = assert_tool_success(
        &repair_response,
        json!("sidecar-setup-repair-non-default-active"),
    );
    assert_eq!(
        repair["status"],
        json!("already_running"),
        "sidecar_setup repair should not spawn shared-agent while another run_id is active: {repair}"
    );
    assert_eq!(repair["run_id"], json!(run_id), "{repair}");
    assert!(
        repair["next_status_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval status")
                && command.contains("--profile agent")
                && command.contains("--run-id")
                && command.contains(run_id)
                && !command.contains(codestory_retrieval::DEFAULT_AGENT_RUN_ID)),
        "already-running response should inspect the active non-default run: {repair}"
    );
}

#[test]
fn resources_read_status_reports_dirty_marker_as_stale_local_index() {
    let mut fixture = indexed_fixture();
    let marker_path = write_dirty_marker_fixture(
        &fixture,
        "dirty-marker.json",
        json!({
            "schema_version": 1,
            "project_root": fixture.workspace.path().to_string_lossy(),
            "dirty": true,
            "updated_at": "2026-06-25T00:00:00.000Z",
            "source": "test-hook",
            "path_sample": ["src/runtime.rs"]
        }),
    );
    fixture.dirty_marker_path = Some(marker_path.clone());
    fixture.dirty_marker_project_root = Some(fixture.workspace.path().to_path_buf());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-dirty-marker",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-dirty-marker"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["dirty_marker"]["status"], json!("dirty_stale"));
    assert_eq!(status["dirty_marker"]["dirty"], json!(true));
    assert_eq!(
        status["dirty_marker"]["reason"],
        json!("dirty_marker_newer_than_index")
    );
    assert_eq!(
        status["index_freshness"]["status"],
        json!("fresh"),
        "computed inventory freshness should remain visible: {status}"
    );
    assert_eq!(
        status["effective_index_freshness"]["status"],
        json!("stale")
    );
    assert_eq!(status["local_refresh"]["state"], json!("skipped"));
    assert_eq!(
        status["local_refresh"]["blocks_local_surfaces"],
        json!(true)
    );
    assert_eq!(status["readiness"][0]["status"], json!("repair_index"));
    assert_allowed_surface(&status, "ground", false, "local_navigation", "repair_index");
    assert_allowed_surface(&status, "packet", false, "agent_packet_search", "blocked");
}

#[test]
fn resources_read_status_uses_full_storage_state_for_dirty_marker_freshness() {
    let mut fixture = indexed_fixture();
    let marker_path = write_dirty_marker_fixture(
        &fixture,
        "dirty-marker-wal-indexed.json",
        json!({
            "schema_version": 1,
            "project_root": fixture.workspace.path().to_string_lossy(),
            "dirty": true,
            "updated_at": "2026-06-25T00:00:00.000Z",
            "source": "test-hook",
            "path_sample": ["src/runtime.rs"]
        }),
    );
    thread::sleep(Duration::from_millis(1200));
    refresh_fixture_index(&fixture);
    fixture.dirty_marker_path = Some(marker_path.clone());
    fixture.dirty_marker_project_root = Some(fixture.workspace.path().to_path_buf());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-dirty-marker-indexed",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-dirty-marker-indexed"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["dirty_marker"]["status"], json!("dirty_indexed"));
    assert_eq!(status["dirty_marker"]["dirty"], json!(true));
    assert_eq!(
        status["dirty_marker"]["blocks_local_surfaces"],
        json!(false)
    );
    assert_fresh_freshness_counts(&status, "dirty marker older than full storage state");
    assert_allowed_surface(&status, "ground", true, "local_navigation", "ready");
    assert_allowed_surface(&status, "packet", false, "agent_packet_search", "blocked");
}

#[test]
fn resources_read_status_reports_unknown_dirty_marker_without_blocking_local_index() {
    let mut fixture = indexed_fixture();
    let marker_path = fixture.cache_dir.path().join("dirty-marker-invalid.json");
    fs::write(&marker_path, "{not-json").expect("write invalid marker");
    fixture.dirty_marker_path = Some(marker_path);
    fixture.dirty_marker_project_root = Some(fixture.workspace.path().to_path_buf());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-unknown-marker",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-unknown-marker"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["dirty_marker"]["status"], json!("unknown"));
    assert_eq!(
        status["dirty_marker"]["blocks_local_surfaces"],
        json!(false)
    );
    assert!(
        status["dirty_marker"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("marker_json_error")),
        "unknown marker should explain the parse failure: {status}"
    );
    assert_fresh_freshness_counts(&status, "status with unknown dirty marker");
    assert_allowed_surface(&status, "ground", true, "local_navigation", "ready");
    assert_allowed_surface(&status, "packet", false, "agent_packet_search", "blocked");
}

#[test]
fn resources_read_status_dirty_marker_fail_open_matrix() {
    let cases = [
        ("missing", None, json!("missing"), None, None),
        (
            "schema",
            Some(json!({
                "schema_version": 99,
                "project_root": "__PROJECT_ROOT__",
                "dirty": true,
                "updated_at": "2026-06-25T00:00:00.000Z",
                "source": "test-hook",
                "path_sample": []
            })),
            json!("unknown"),
            Some(json!("schema_version_unsupported")),
            None,
        ),
        (
            "root",
            Some(json!({
                "schema_version": 1,
                "project_root": "C:/different/project",
                "dirty": true,
                "updated_at": "2026-06-25T00:00:00.000Z",
                "source": "test-hook",
                "path_sample": []
            })),
            json!("unknown"),
            Some(json!("project_root_mismatch")),
            None,
        ),
        (
            "clean",
            Some(json!({
                "schema_version": 1,
                "project_root": "__PROJECT_ROOT__",
                "dirty": false,
                "updated_at": "2026-06-25T00:00:00.000Z",
                "source": "test-hook",
                "path_sample": []
            })),
            json!("clean"),
            None,
            Some(json!(false)),
        ),
    ];

    for (name, marker, expected_status, expected_reason, expected_dirty) in cases {
        let mut fixture = indexed_fixture();
        let marker_path = fixture
            .cache_dir
            .path()
            .join(format!("dirty-marker-{name}.json"));
        if let Some(mut marker) = marker {
            if marker["project_root"] == json!("__PROJECT_ROOT__") {
                marker["project_root"] = json!(fixture.workspace.path().to_string_lossy());
            }
            fs::write(&marker_path, marker.to_string()).expect("write marker");
        }
        fixture.dirty_marker_path = Some(marker_path);
        fixture.dirty_marker_project_root = Some(fixture.workspace.path().to_path_buf());
        let mut server = spawn_stdio_server(&fixture);

        let response = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": format!("status-dirty-marker-{name}"),
                "method": "resources/read",
                "params": {"uri": "codestory://status"}
            }),
        );
        let result =
            assert_success_envelope(&response, json!(format!("status-dirty-marker-{name}")));
        let status = json_resource_content(result, "codestory://status");

        assert_eq!(
            status["dirty_marker"]["status"], expected_status,
            "{name}: {status}"
        );
        assert_eq!(
            status["dirty_marker"]["blocks_local_surfaces"],
            json!(false),
            "{name}: {status}"
        );
        if let Some(reason) = expected_reason {
            assert_eq!(status["dirty_marker"]["reason"], reason, "{name}: {status}");
        }
        if let Some(dirty) = expected_dirty {
            assert_eq!(status["dirty_marker"]["dirty"], dirty, "{name}: {status}");
        }
        assert_fresh_freshness_counts(&status, name);
        assert_allowed_surface(&status, "ground", true, "local_navigation", "ready");
        assert_allowed_surface(&status, "packet", false, "agent_packet_search", "blocked");
    }
}

#[test]
fn resources_read_status_blocks_all_surfaces_when_active_cli_is_stale() {
    let mut fixture = indexed_fixture();
    fixture.latest_release_version = "999.0.0".to_string();
    fixture.disable_installed_cli_probe = true;
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-stale-cli",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-stale-cli"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(
        status["readiness"][0]["status"],
        json!("repair_setup"),
        "stale active CLI should be a hard setup repair state: {status}"
    );
    let setup = &status["readiness"][0]["setup"];
    assert_eq!(setup["active_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(setup["latest_version"], "999.0.0");
    assert!(
        setup["active_path"]
            .as_str()
            .is_some_and(|path| path.contains("codestory-cli")),
        "setup snapshot should expose active executable path: {status}"
    );
    for surface in [
        "ground",
        "files",
        "packet",
        "search",
        "context",
        "repair_all",
    ] {
        let surface_status = &status["allowed_surfaces"][surface];
        assert_eq!(surface_status["allowed"], json!(false));
        assert_eq!(surface_status["status"], json!("repair_setup"));
        assert_eq!(surface_status["repair_reason"], json!("stale_active_cli"));
    }
    let next_call_text = status["recommended_next_calls"].to_string();
    assert!(
        next_call_text.contains("install-codestory.ps1") && next_call_text.contains("999.0.0"),
        "stale active CLI repair should be an installer command, not a prompt: {status}"
    );
}

#[test]
fn tool_calls_block_all_surfaces_when_active_cli_is_stale() {
    let mut fixture = indexed_fixture();
    fixture.latest_release_version = "999.0.0".to_string();
    fixture.disable_installed_cli_probe = true;
    let mut server = spawn_stdio_server(&fixture);

    for (tool, arguments) in [
        ("ground", json!({})),
        ("search", json!({"query": "AppController"})),
        ("repair_all", json!({})),
    ] {
        let response = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": format!("stale-{tool}"),
                "method": "tools/call",
                "params": {
                    "name": tool,
                    "arguments": arguments
                }
            }),
        );
        let error = assert_tool_error(&response, json!(format!("stale-{tool}")));
        assert_eq!(
            error.pointer("/code").and_then(Value::as_str),
            Some("codestory_tool_blocked")
        );
        assert_eq!(
            error.pointer("/status").and_then(Value::as_str),
            Some("repair_setup")
        );
        assert_eq!(
            error.pointer("/repair_reason").and_then(Value::as_str),
            Some("stale_active_cli")
        );
        let setup = error
            .pointer("/setup")
            .unwrap_or_else(|| panic!("blocked stale CLI tool should include setup: {response}"));
        assert_eq!(setup["active_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(setup["latest_version"], "999.0.0");
        let minimum_next = error
            .pointer("/minimum_next")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("blocked stale CLI tool should include minimum_next: {response}")
            });
        assert!(
            minimum_next.iter().all(|call| call
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|method| method != "cli")),
            "blocked stale CLI tool should not expose CLI as the normal repair method: {response}"
        );
        assert!(
            minimum_next.iter().any(|call| call
                .get("debug_command")
                .and_then(Value::as_str)
                .is_some_and(
                    |text| text.contains("install-codestory.ps1") && text.contains("999.0.0")
                )),
            "blocked stale CLI tool should keep installer detail as debug metadata: {response}"
        );
    }
}

#[test]
fn background_local_refresh_status_refreshes_long_lived_stale_index_with_bounded_latency() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let warmup = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-freshness-warmup",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let warmup_result = assert_success_envelope(&warmup, json!("status-freshness-warmup"));
    let warmup_status = json_resource_content(warmup_result, "codestory://status");
    assert_fresh_freshness_counts(&warmup_status, "warm codestory://status");

    thread::sleep(Duration::from_millis(25));
    fs::write(
        fixture.workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("changed:{project_name}")
}
"#,
    )
    .expect("modify indexed file after indexing");
    fs::write(
        fixture
            .workspace
            .path()
            .join("src")
            .join("new_after_index.rs"),
        "pub fn new_after_index() {}\n",
    )
    .expect("write new file after indexing");
    fs::remove_file(fixture.workspace.path().join("src").join("alpha.rs"))
        .expect("remove indexed file after indexing");

    let refreshed = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-freshness-after-mutation",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let refreshed_result =
        assert_success_envelope(&refreshed, json!("status-freshness-after-mutation"));
    let refreshed_status = json_resource_content(refreshed_result, "codestory://status");
    assert_fresh_freshness_counts(&refreshed_status, "codestory://status after mutation");
    assert_eq!(
        refreshed_status["local_refresh"]["reason"],
        json!("refreshed"),
        "long-lived status must not return the cached warm freshness result after source mutation: {refreshed_status}"
    );
    assert_eq!(
        refreshed_status["local_refresh"]["blocks_local_surfaces"],
        json!(false),
        "successful local refresh should keep local graph surfaces usable: {refreshed_status}"
    );
    assert_eq!(
        refreshed_status["allowed_surfaces"]["ground"]["allowed"],
        json!(true),
        "fresh local graph should allow local graph surfaces: {refreshed_status}"
    );
    assert_eq!(
        refreshed_status["allowed_surfaces"]["packet"]["status"],
        json!("blocked"),
        "packet/search should stay gated by the agent retrieval lane after local refresh: {refreshed_status}"
    );
    let status_next_call_text = refreshed_status["recommended_next_calls"].to_string();
    assert!(
        !status_next_call_text.contains("\"tool\":\"packet\"")
            && !status_next_call_text.contains("\"tool\":\"search\""),
        "local freshness repair should not recommend packet/search calls while sidecars are unavailable: {refreshed_status}"
    );

    let mut elapsed = Vec::new();
    let mut last_status = refreshed_status;
    for index in 0..12 {
        let started = Instant::now();
        let response = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": format!("status-freshness-{index}"),
                "method": "resources/read",
                "params": {"uri": "codestory://status"}
            }),
        );
        elapsed.push(started.elapsed());
        let result = assert_success_envelope(&response, json!(format!("status-freshness-{index}")));
        last_status = json_resource_content(result, "codestory://status");
    }

    assert_fresh_freshness_counts(&last_status, "cached codestory://status after refresh");
    assert!(
        matches!(
            last_status["local_refresh"]["reason"].as_str(),
            Some("refreshed" | "already_fresh")
        ),
        "status should stay fresh without stale cache masking after the bounded refresh: {last_status}"
    );
    elapsed.sort_unstable();
    let median = elapsed[elapsed.len() / 2];
    let p95 = elapsed[(elapsed.len() * 95).div_ceil(100) - 1];
    assert!(
        median < Duration::from_millis(250),
        "warm status freshness check median should stay under 250ms for a small repo, got median={median:?}, p95={p95:?}"
    );
    assert!(
        p95 < Duration::from_secs(1),
        "warm status freshness check p95 should stay under 1s for a small repo, got median={median:?}, p95={p95:?}"
    );

    let mut index_command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    index_command
        .arg("index")
        .arg("--refresh")
        .arg("full")
        .arg("--format")
        .arg("json")
        .arg("--project")
        .arg(fixture.workspace.path())
        .arg("--cache-dir")
        .arg(fixture.cache_dir.path());
    apply_fixture_embedding_env(&mut index_command, fixture.hash_embeddings);
    let output = index_command
        .output()
        .expect("rerun index after stale status");
    assert!(
        output.status.success(),
        "reindex failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let refreshed = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-freshness-after-reindex",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let result = assert_success_envelope(&refreshed, json!("status-freshness-after-reindex"));
    let refreshed_status = json_resource_content(result, "codestory://status");
    assert_fresh_freshness_counts(&refreshed_status, "codestory://status after reindex");
}

#[test]
fn ground_tool_returns_degraded_local_refresh_when_stale_refresh_budget_expires() {
    let mut fixture = indexed_fixture();
    fixture.local_refresh_timeout_ms = Some(0);

    thread::sleep(Duration::from_millis(25));
    fs::write(
        fixture.workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("budget-expired:{project_name}")
}
"#,
    )
    .expect("modify indexed file after indexing");

    let mut server = spawn_stdio_server(&fixture);
    let started = Instant::now();
    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-refresh-budget-expired",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"budget": "strict"}
            }
        }),
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "ground should return degraded local-refresh guidance before an MCP tool timeout, got {elapsed:?}: {response}"
    );

    let error = assert_tool_error(&response, json!("ground-refresh-budget-expired"));
    assert_eq!(
        error.pointer("/code").and_then(Value::as_str),
        Some("codestory_tool_blocked")
    );
    assert_eq!(
        error.pointer("/tool").and_then(Value::as_str),
        Some("ground")
    );
    assert_eq!(
        error.pointer("/readiness_goal").and_then(Value::as_str),
        Some("local_navigation")
    );
    assert_eq!(
        error.pointer("/status").and_then(Value::as_str),
        Some("repair_index")
    );
    assert_eq!(
        error
            .pointer("/local_refresh/state")
            .and_then(Value::as_str),
        Some("refreshing")
    );
    assert_eq!(
        error
            .pointer("/local_refresh/readiness_status")
            .and_then(Value::as_str),
        Some("repair_index")
    );
    assert_eq!(
        error
            .pointer("/local_refresh/blocks_local_surfaces")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        error
            .pointer("/local_refresh/reason")
            .and_then(Value::as_str),
        Some("refresh_timeout")
    );
    assert_ne!(
        error
            .pointer("/sidecar/retrieval_mode")
            .and_then(Value::as_str),
        Some("full"),
        "local refresh degradation must not claim packet/search sidecar readiness: {error}"
    );
    assert!(
        error
            .pointer("/minimum_next")
            .and_then(Value::as_array)
            .is_some_and(|commands| !commands.is_empty()),
        "blocked ground should include compact next-step guidance: {error}"
    );
}

#[test]
fn tools_call_local_graph_refreshes_long_lived_index_after_source_mutation() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let ground_before = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-ground-before",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"budget": "strict"}
            }
        }),
    );
    let ground_before = assert_tool_success(&ground_before, json!("tool-refresh-ground-before"));
    let node_count_before = ground_before
        .pointer("/stats/node_count")
        .and_then(Value::as_u64)
        .expect("ground before mutation node count");

    let files_before = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-files-before",
            "method": "tools/call",
            "params": {
                "name": "files",
                "arguments": {"path": "src/runtime.rs", "limit": 5}
            }
        }),
    );
    let files_before = assert_tool_success(&files_before, json!("tool-refresh-files-before"));
    assert_eq!(
        files_before.pointer("/summary/visible_file_count"),
        Some(&json!(1)),
        "files tool should work before mutation: {files_before}"
    );

    thread::sleep(Duration::from_millis(25));
    fs::write(
        fixture
            .workspace
            .path()
            .join("src")
            .join("live_tool_added.rs"),
        "pub fn stdio_tool_added_after_mutation() -> usize {\n    7\n}\n",
    )
    .expect("write file after stdio server startup");

    let files_after = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-files-after",
            "method": "tools/call",
            "params": {
                "name": "files",
                "arguments": {"path": "live_tool_added.rs", "limit": 5}
            }
        }),
    );
    let files_after = assert_tool_success(&files_after, json!("tool-refresh-files-after"));
    assert!(
        files_after["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file["path"]
                .as_str()
                .is_some_and(|path| path.contains("live_tool_added.rs")))),
        "files tool should refresh the local graph before serving post-mutation evidence: {files_after}"
    );

    let status_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-status",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let status_result = assert_success_envelope(&status_response, json!("tool-refresh-status"));
    let status = json_resource_content(status_result, "codestory://status");
    assert_fresh_freshness_counts(&status, "codestory://status after local graph tool refresh");
    assert_eq!(
        status["local_refresh"]["reason"],
        json!("refreshed"),
        "tool dispatch should have refreshed the long-lived server before status was reread: {status}"
    );
    assert_eq!(
        status["allowed_surfaces"]["search"]["status"],
        json!("blocked"),
        "local graph refresh must not make packet/search readiness claims: {status}"
    );

    let ground_after = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-ground-after",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"budget": "strict"}
            }
        }),
    );
    let ground_after = assert_tool_success(&ground_after, json!("tool-refresh-ground-after"));
    let node_count_after = ground_after
        .pointer("/stats/node_count")
        .and_then(Value::as_u64)
        .expect("ground after mutation node count");
    assert!(
        node_count_after > node_count_before,
        "ground should serve refreshed graph stats after mutation; before={node_count_before}, after={node_count_after}, snapshot={ground_after}"
    );

    let symbol_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-symbol",
            "method": "tools/call",
            "params": {
                "name": "symbol",
                "arguments": {"query": "stdio_tool_added_after_mutation"}
            }
        }),
    );
    let symbol = assert_tool_success(&symbol_response, json!("tool-refresh-symbol"));
    let node_id = symbol
        .pointer("/node/id")
        .and_then(Value::as_str)
        .or_else(|| {
            symbol
                .pointer("/resolution/resolved/node_id")
                .and_then(Value::as_str)
        })
        .unwrap_or_else(|| panic!("symbol should resolve the post-mutation function: {symbol}"))
        .to_string();

    for (tool, id) in [
        ("snippet", "tool-refresh-snippet"),
        ("trail", "tool-refresh-trail"),
        ("trace", "tool-refresh-trace"),
    ] {
        let response = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": tool,
                    "arguments": {"id": node_id, "depth": 1, "max_nodes": 20}
                }
            }),
        );
        let result = assert_tool_success(&response, json!(id));
        assert!(
            result
                .to_string()
                .contains("stdio_tool_added_after_mutation"),
            "{tool} should serve refreshed graph evidence for the post-mutation symbol: {result}"
        );
    }

    let affected_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-affected",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "changed_paths": ["src/live_tool_added.rs"],
                    "change_records": [
                        {
                            "path": "src/live_tool_added.rs",
                            "kind": "added",
                            "status": "A"
                        }
                    ]
                }
            }
        }),
    );
    let affected = assert_tool_success(&affected_response, json!("tool-refresh-affected"));
    assert_eq!(
        affected["matched_file_count"],
        json!(1),
        "affected should use the refreshed local graph for the added file: {affected}"
    );

    let search_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "tool-refresh-search-still-blocked",
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "stdio_tool_added_after_mutation"}
            }
        }),
    );
    let search_error =
        assert_tool_error(&search_response, json!("tool-refresh-search-still-blocked"));
    assert_eq!(
        search_error.pointer("/code").and_then(Value::as_str),
        Some("codestory_tool_blocked"),
        "packet/search readiness should remain separately gated after local graph refresh: {search_response}"
    );
}

#[test]
fn resources_read_agent_guide_describes_default_browser_loop_and_safety() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "agent-guide-resource",
            "method": "resources/read",
            "params": {"uri": "codestory://agent-guide"}
        }),
    );

    let result = assert_success_envelope(&response, json!("agent-guide-resource"));
    let guide = json_resource_content(result, "codestory://agent-guide");
    assert!(
        guide
            .get("default_browser_loop")
            .or_else(|| guide.get("recommended_call_sequence"))
            .or_else(|| guide.get("recommended_next_calls"))
            .and_then(Value::as_array)
            .is_some_and(|calls| {
                calls
                    .iter()
                    .any(|call| call["uri"] == json!("codestory://status"))
            })
            && guide
                .get("readiness_lanes")
                .and_then(Value::as_array)
                .is_some_and(|lanes| lanes.len() >= 2),
        "agent guide should include a concise default browser loop or call sequence: {guide}"
    );
    let local_lane = guide["readiness_lanes"]
        .as_array()
        .and_then(|lanes| {
            lanes
                .iter()
                .find(|lane| lane["readiness_goal"] == json!("local_navigation"))
        })
        .unwrap_or_else(|| panic!("agent guide should include local_navigation lane: {guide}"));
    let local_surfaces = local_lane["surfaces"]
        .as_array()
        .unwrap_or_else(|| panic!("local lane should list surfaces: {guide}"));
    for expected in [
        "ground",
        "files",
        "symbol",
        "definition",
        "get_node",
        "callers",
        "callees",
        "neighbors",
        "shortest_path",
        "query_subgraph",
        "symbols",
        "snippet",
        "references",
        "trace",
        "trail",
        "affected",
    ] {
        assert!(
            local_surfaces.iter().any(|surface| surface == expected),
            "local lane should include {expected}: {guide}"
        );
    }
    assert!(
        !local_surfaces.iter().any(|surface| surface == "context"),
        "context is sidecar-backed and should not be in the local lane: {guide}"
    );
    let agent_lane = guide["readiness_lanes"]
        .as_array()
        .and_then(|lanes| {
            lanes
                .iter()
                .find(|lane| lane["readiness_goal"] == json!("agent_packet_search"))
        })
        .unwrap_or_else(|| panic!("agent guide should include agent_packet_search lane: {guide}"));
    let agent_surfaces = agent_lane["surfaces"]
        .as_array()
        .unwrap_or_else(|| panic!("agent lane should list surfaces: {guide}"));
    for expected in ["packet", "search", "context"] {
        assert!(
            agent_surfaces.iter().any(|surface| surface == expected),
            "agent lane should include {expected}: {guide}"
        );
    }
    let mut strings = Vec::new();
    string_values_recursive(&guide, &mut strings);
    for expected in [
        "ground",
        "packet",
        "search",
        "context",
        "definition",
        "snippet",
    ] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "agent guide should recommend {expected} in its call sequence: {guide}"
        );
    }
    let guide_text = strings.join("\n").to_ascii_lowercase();
    let unconditional_sequence_text = guide
        .get("recommended_call_sequence")
        .and_then(Value::as_array)
        .map(|calls| Value::Array(calls.clone()).to_string())
        .unwrap_or_default();
    assert!(
        !unconditional_sequence_text.contains("\"tool\":\"packet\"")
            && !unconditional_sequence_text.contains("\"tool\":\"search\""),
        "packet/search should not be unconditional normal next steps: {guide}"
    );
    assert!(
        guide_text.contains("allowed_surfaces.packet.allowed")
            && guide_text.contains("allowed_surfaces.search.allowed")
            && guide_text.contains("allowed_surfaces.context.allowed"),
        "agent guide should make packet/search/context conditional on status allowed_surfaces: {guide}"
    );
    assert!(
        guide_text.contains("repo-text hits as navigation clues"),
        "agent guide should treat repo-text hits as navigation clues: {guide}"
    );
    assert!(
        guide_text.contains("search hits as discovery clues")
            && guide_text.contains("proof-bearing sidecar, graph, or source evidence"),
        "agent guide should distinguish discovery clues from proof-bearing evidence: {guide}"
    );
    assert!(
        guide_text.contains("unsafe to claim") && guide_text.contains("follow_up_commands"),
        "agent guide should name unsafe-to-claim and follow-up states: {guide}"
    );
    assert!(
        guide_text.contains("direct_source_reads")
            && guide_text.contains("missing, stale, or degraded index/sidecar state"),
        "agent guide should name the direct source-read fallback: {guide}"
    );
    assert!(
        guide_text.contains("ground")
            && guide_text.contains("files")
            && guide_text.contains("definition")
            && guide_text.contains("get_node")
            && guide_text.contains("neighbors")
            && guide_text.contains("shortest_path")
            && guide_text.contains("query_subgraph")
            && guide_text.contains("symbols")
            && guide_text.contains("affected")
            && guide_text.contains("local_navigation"),
        "agent guide should record local navigation surfaces: {guide}"
    );
    assert!(
        !guide_text.contains("files, affected, cache identity, retrieval status"),
        "agent guide should not describe allowed files/affected surfaces as deferred: {guide}"
    );
    assert!(
        !guide_text.contains("repo-text hits as evidence"),
        "agent guide should not present repo-text hits as evidence: {guide}"
    );
    assert!(
        contains_key_recursive(&guide, &["safety_notes", "safety"])
            || strings.iter().any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("read-only") || value.contains("non-destructive")
            }),
        "agent guide should include safety notes: {guide}"
    );
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn resources_read_status_keeps_dirty_marker_separate_from_full_sidecar_readiness() {
    let Some(mut fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}"))
    else {
        return;
    };
    let marker_path = write_dirty_marker_fixture(
        &fixture,
        "dirty-marker-full-sidecar.json",
        json!({
            "schema_version": 1,
            "project_root": fixture.workspace.path().to_string_lossy(),
            "dirty": true,
            "updated_at": "2026-06-25T00:00:00.000Z",
            "source": "test-hook",
            "path_sample": ["src/runtime.rs"]
        }),
    );
    fixture.dirty_marker_path = Some(marker_path);
    fixture.dirty_marker_project_root = Some(fixture.workspace.path().to_path_buf());
    let mut server = spawn_stdio_server(&fixture);

    let status_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "full-retrieval-dirty-marker-status",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let status_result = assert_success_envelope(
        &status_response,
        json!("full-retrieval-dirty-marker-status"),
    );
    let status = json_resource_content(status_result, "codestory://status");

    assert_eq!(status["dirty_marker"]["status"], json!("dirty_stale"));
    assert_allowed_surface(&status, "ground", false, "local_navigation", "repair_index");
    for surface in ["packet", "search", "context"] {
        assert_allowed_surface(&status, surface, true, "agent_packet_search", "ready");
    }
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn transcript_calls_search_tool() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let status_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "full-retrieval-status",
            "method": "resources/read",
            "params": {"uri": "codestory://status"}
        }),
    );
    let status_result = assert_success_envelope(&status_response, json!("full-retrieval-status"));
    let status = json_resource_content(status_result, "codestory://status");
    for surface in ["packet", "search", "context"] {
        assert_allowed_surface(&status, surface, true, "agent_packet_search", "ready");
    }

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "AppController"}
            }
        }),
    );

    let result = assert_tool_success(&response, json!(10));
    assert!(
        result["indexed_symbol_hits"]
            .as_array()
            .is_some_and(|hits| hits
                .iter()
                .any(|hit| hit["display_name"] == "AppController")),
        "search tool should return AppController hit: {result}"
    );
    let app_controller_hit = result["indexed_symbol_hits"]
        .as_array()
        .expect("indexed symbol hits")
        .iter()
        .find(|hit| hit["display_name"] == "AppController")
        .unwrap_or_else(|| panic!("missing AppController hit: {result}"));
    assert_eq!(
        app_controller_hit["match_quality"],
        json!("exact"),
        "stdio search hits should satisfy the advertised match_quality schema: {app_controller_hit}"
    );
    let app_controller_id = app_controller_hit["node_id"]
        .as_str()
        .expect("AppController node id")
        .to_string();

    let snippet_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "snippet-schema-fields",
            "method": "tools/call",
            "params": {
                "name": "snippet",
                "arguments": {"id": app_controller_id}
            }
        }),
    );
    let snippet_result = assert_tool_success(&snippet_response, json!("snippet-schema-fields"));
    assert_eq!(
        snippet_result["scope"],
        json!("line_context"),
        "stdio snippet should emit its scope: {snippet_result}"
    );
    assert_eq!(
        snippet_result["requested_context"],
        json!(4),
        "stdio snippet should emit requested_context: {snippet_result}"
    );
    assert!(
        snippet_result["snippet_truncated"].is_boolean(),
        "stdio snippet should emit snippet_truncated: {snippet_result}"
    );

    let symbol_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "symbol-related-hits",
            "method": "tools/call",
            "params": {
                "name": "symbol",
                "arguments": {"query": "configure", "choose": 1}
            }
        }),
    );
    let symbol_result = assert_tool_success(&symbol_response, json!("symbol-related-hits"));
    let related_hits = symbol_result["related_hits"]
        .as_array()
        .unwrap_or_else(|| panic!("symbol related_hits should be an array: {symbol_result}"));
    assert!(
        related_hits
            .iter()
            .any(|hit| hit.get("match_quality").is_none()),
        "stdio symbol related_hits should exercise optional match_quality omission: {symbol_result}"
    );

    let symbols_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "symbols-structured",
            "method": "tools/call",
            "params": {
                "name": "symbols",
                "arguments": {"limit": 2}
            }
        }),
    );

    let symbols_result = assert_tool_success(&symbols_response, json!("symbols-structured"));
    let symbols = symbols_result["symbols"].as_array().unwrap_or_else(|| {
        panic!("symbols tool should return an object with symbols: {symbols_result}")
    });
    assert!(
        !symbols.is_empty() && symbols.len() <= 2,
        "symbols tool should respect the requested cap: {symbols_result}"
    );
}

#[test]
fn search_tool_fails_closed_without_full_retrieval_sidecars() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "search-requires-full-retrieval",
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "AppController"}
            }
        }),
    );

    let error = assert_tool_error(&response, json!("search-requires-full-retrieval"));
    assert_eq!(
        error.pointer("/code").and_then(Value::as_str),
        Some("codestory_tool_blocked"),
        "stdio search should be blocked by readiness before dispatch: {response}"
    );
    assert_eq!(
        error.pointer("/readiness_goal").and_then(Value::as_str),
        Some("agent_packet_search")
    );
    assert_eq!(
        error.pointer("/status").and_then(Value::as_str),
        Some("blocked")
    );
    assert_eq!(
        error.pointer("/failed_layer").and_then(Value::as_str),
        Some("retrieval_sidecar")
    );
    assert_eq!(
        error.pointer("/repair_reason").and_then(Value::as_str),
        Some("retrieval_manifest_missing")
    );
    assert_eq!(
        error
            .pointer("/sidecar/degraded_reason")
            .and_then(Value::as_str),
        Some("retrieval_manifest_missing")
    );
    let minimum_next = error["minimum_next"]
        .as_array()
        .unwrap_or_else(|| panic!("stdio search error should include minimum_next: {response}"));
    assert_eq!(
        minimum_next.len(),
        1,
        "stdio search readiness error should expose exactly one canonical minimum repair: {response}"
    );
    assert!(
        minimum_next.iter().any(|call| call
            .get("tool")
            .and_then(Value::as_str)
            .is_some_and(|tool| tool == "sidecar_setup")
            && call.pointer("/arguments/action").and_then(Value::as_str) == Some("repair")
            && call
                .get("debug_commands")
                .and_then(Value::as_array)
                .is_some_and(|commands| !commands.is_empty())),
        "stdio search readiness error should point at MCP-managed agent repair: {response}"
    );
    let full_repair = error["full_repair"]
        .as_array()
        .unwrap_or_else(|| panic!("stdio search error should include full_repair: {response}"));
    assert!(
        full_repair
            .iter()
            .all(|call| !call.to_string().contains("\"method\":\"cli\"")
                && call
                    .get("debug_command")
                    .and_then(Value::as_str)
                    .is_none_or(|text| !text.contains("codestory-cli index"))),
        "stdio search sidecar errors should not repeat core index repair commands: {response}"
    );
    assert!(
        full_repair.iter().any(
            |call| call.to_string().contains("codestory-cli retrieval status")
                && call.to_string().contains("--format json")
        ),
        "stdio search error should include sidecar status proof debug command: {response}"
    );
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn context_tool_maps_target_id_to_deep_browser_request() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let search_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "context-focus-search",
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "AppController"}
            }
        }),
    );
    let search_result = assert_tool_success(&search_response, json!("context-focus-search"));
    let node_id = search_result["indexed_symbol_hits"]
        .as_array()
        .expect("indexed symbol hits")
        .iter()
        .find(|hit| hit["display_name"] == "AppController")
        .and_then(|hit| hit["node_id"].as_str())
        .unwrap_or_else(|| panic!("missing AppController node id: {search_result}"))
        .to_string();

    let context_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "context-focus",
            "method": "tools/call",
            "params": {
                "name": "context",
                "arguments": {
                    "id": node_id,
                    "max_results": 4
                }
            }
        }),
    );

    let packet = assert_tool_success(&context_response, json!("context-focus"));
    assert_eq!(
        packet.pointer("/retrieval_trace/resolved_profile"),
        Some(&json!("investigate")),
        "stdio context should use the investigation preset by default: {packet}"
    );
    assert!(
        packet
            .get("summary")
            .and_then(Value::as_str)
            .is_some_and(|summary| summary.contains("DB-first retrieval")),
        "stdio context should return the DB-first labeled packet after local-agent removal: {packet}"
    );
    assert!(
        !packet.to_string().contains("local_agent"),
        "stdio context should not leak removed local-agent fields: {packet}"
    );
    let neighborhood_step = packet
        .pointer("/retrieval_trace/steps")
        .and_then(Value::as_array)
        .and_then(|steps| steps.iter().find(|step| step["kind"] == "neighborhood"))
        .unwrap_or_else(|| panic!("missing neighborhood step in context trace: {packet}"));
    assert!(
        neighborhood_step
            .get("input")
            .and_then(Value::as_array)
            .is_some_and(|fields| fields
                .iter()
                .any(|field| field["key"] == "center_id" && field["value"] == node_id)),
        "stdio context.id should seed the browser focus node: {neighborhood_step}"
    );
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn packet_tool_returns_budgeted_sufficiency_contract() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "packet-contract",
            "method": "tools/call",
            "params": {
                "name": "packet",
                "arguments": {
                    "question": "Explain how AppController routes repository indexing",
                    "budget": "tiny",
                    "task_class": "architecture_explanation"
                }
            }
        }),
    );

    let packet = assert_tool_success(&response, json!("packet-contract"));
    let text = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("stdio packet should include readable text content: {response}"));
    assert!(
        text.contains("packet_id:") && text.contains("sufficiency:"),
        "stdio packet text should summarize packet identity and sufficiency: {text}"
    );
    for expected in [
        "budget:",
        "truncated:",
        "unsafe_to_claim:",
        "pagination:",
        "repo_content_boundary:",
        "gaps:",
        "open_next:",
        "follow_up_commands:",
    ] {
        assert!(
            text.contains(expected),
            "stdio packet text should name {expected}: {text}"
        );
    }
    assert!(
        !text.trim_start().starts_with('{'),
        "stdio packet text should be a digest, not duplicated JSON: {text}"
    );
    assert!(
        !text.contains("\"retrieval_trace\""),
        "stdio packet text should leave full traces in structuredContent only: {text}"
    );
    assert_eq!(
        packet["question"], "Explain how AppController routes repository indexing",
        "stdio packet should preserve the requested question: {packet}"
    );
    assert_eq!(
        packet["budget"]["requested"], "tiny",
        "stdio packet should honor the requested budget: {packet}"
    );
    let packet_bytes = serde_json::to_vec(packet)
        .expect("serialize packet content")
        .len();
    let used_output_bytes = packet
        .pointer("/budget/used/output_bytes")
        .and_then(Value::as_u64)
        .expect("packet budget should include output byte usage");
    let max_output_bytes = packet
        .pointer("/budget/limits/max_output_bytes")
        .and_then(Value::as_u64)
        .expect("packet budget should include output byte limit");
    assert!(
        used_output_bytes <= max_output_bytes,
        "packet should fit inside its advertised output budget: {packet}"
    );
    assert!(
        packet_bytes <= max_output_bytes as usize,
        "stdio structured packet should fit inside its advertised output budget: {packet}"
    );
    assert_eq!(
        packet["plan"]["task_class"], "architecture_explanation",
        "stdio packet should expose the planner task class: {packet}"
    );
    assert!(
        packet["plan"]["queries"]
            .as_array()
            .is_some_and(|queries| !queries.is_empty()),
        "stdio packet should expose planned retrieval queries: {packet}"
    );
    assert!(
        packet
            .pointer("/answer/retrieval_trace/steps")
            .and_then(Value::as_array)
            .is_some_and(|steps| !steps.is_empty()),
        "stdio packet should expose the underlying retrieval trace: {packet}"
    );
    assert!(
        packet
            .pointer("/sufficiency/status")
            .and_then(Value::as_str)
            .is_some(),
        "stdio packet should include a sufficiency status: {packet}"
    );
    assert!(
        packet
            .pointer("/retrieval_trace_summary/source_read_steps")
            .and_then(Value::as_u64)
            .is_some(),
        "stdio packet should include retrieval trace summary counters: {packet}"
    );

    let repeated_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "packet-contract-repeat",
            "method": "tools/call",
            "params": {
                "name": "packet",
                "arguments": {
                    "question": "Explain how AppController routes repository indexing",
                    "budget": "tiny",
                    "task_class": "architecture_explanation"
                }
            }
        }),
    );
    let repeated_packet = assert_tool_success(&repeated_response, json!("packet-contract-repeat"));
    assert_eq!(
        repeated_packet, packet,
        "identical stdio packet requests should reuse the warm packet response without changing the structured payload"
    );
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn structured_packet_and_context_honor_include_evidence_false() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let packet_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "packet-no-evidence",
            "method": "tools/call",
            "params": {
                "name": "packet",
                "arguments": {
                    "question": "Explain how AppController routes repository indexing",
                    "budget": "tiny",
                    "task_class": "architecture_explanation",
                    "include_evidence": false
                }
            }
        }),
    );
    let packet = assert_tool_success(&packet_response, json!("packet-no-evidence"));
    assert_structured_citations_have_no_evidence(packet);

    let context_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "context-no-evidence",
            "method": "tools/call",
            "params": {
                "name": "context",
                "arguments": {
                    "query": "AppController",
                    "include_evidence": false
                }
            }
        }),
    );
    let context = assert_tool_success(&context_response, json!("context-no-evidence"));
    assert_structured_citations_have_no_evidence(context);
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn search_tool_exposes_continuation_links_and_clamps_tiny_payloads() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "search-continuations",
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "AppController", "limit": 500}
            }
        }),
    );

    let result = assert_tool_success(&response, json!("search-continuations"));
    assert!(
        result["limit_per_source"]
            .as_u64()
            .is_some_and(|limit| limit <= 50),
        "search limit should be clamped to the documented max: {result}"
    );
    let response_size = serde_json::to_vec(&response)
        .expect("serialize response")
        .len();
    assert!(
        response_size < 64 * 1024,
        "tiny fixture search response should stay bounded, got {response_size} bytes: {result}"
    );
    let hits = result["indexed_symbol_hits"]
        .as_array()
        .expect("indexed symbol hits");
    assert!(
        hits.len() <= 50,
        "search indexed hits should respect the documented page cap: {result}"
    );
    let hit = hits
        .iter()
        .find(|hit| hit["display_name"] == "AppController")
        .unwrap_or_else(|| panic!("missing AppController hit: {result}"));
    let node_id = hit["node_id"].as_str().expect("hit node id");
    assert_continuation_links(hit, node_id, "search hit");
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn search_tool_does_not_offer_symbol_links_for_non_resolvable_repo_text_hits() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "repo-text-continuations",
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {
                    "query": "workspace:{project_name}",
                    "repo_text": "on",
                    "limit": 10
                }
            }
        }),
    );

    let result = assert_tool_success(&response, json!("repo-text-continuations"));
    let repo_text_hits = result["repo_text_hits"].as_array().expect("repo text hits");
    let non_resolvable_hit = repo_text_hits
        .iter()
        .find(|hit| hit["resolvable"] == json!(false))
        .unwrap_or_else(|| panic!("expected a non-resolvable repo-text hit: {result}"));
    assert!(
        non_resolvable_hit.get("links").is_none(),
        "non-resolvable repo-text hits should not advertise symbol/snippet/trail continuations: {non_resolvable_hit}"
    );
    assert_eq!(
        non_resolvable_hit["trust"], "untrusted_repo_evidence",
        "repo-text hits should carry the trust-boundary marker: {non_resolvable_hit}"
    );
    assert!(
        non_resolvable_hit.get("untrusted_repo_excerpt").is_some(),
        "repo-text hits with excerpts should expose the labeled excerpt field: {non_resolvable_hit}"
    );
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn definition_tool_exposes_symbol_snippet_references_and_trail_links() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "definition-continuations",
            "method": "tools/call",
            "params": {
                "name": "definition",
                "arguments": {"query": "AppController"}
            }
        }),
    );

    let result = assert_tool_success(&response, json!("definition-continuations"));
    let node_id = result
        .pointer("/definition/node_id")
        .and_then(Value::as_str)
        .or_else(|| {
            result
                .pointer("/resolution/resolved/node_id")
                .and_then(Value::as_str)
        })
        .expect("definition result node id");
    assert_continuation_links(result, node_id, "definition result");
}

#[test]
#[ignore = "live full-retrieval stdio success contract; set CODESTORY_STDIO_FULL_RETRIEVAL_TESTS=1 after preparing real sidecars"]
fn symbol_tool_reports_ambiguous_targets_and_choose_resolves_displayed_number() {
    let Some(fixture) = full_retrieval_fixture().unwrap_or_else(|error| panic!("{error}")) else {
        return;
    };
    let mut server = spawn_stdio_server(&fixture);

    let ambiguous = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ambiguous-symbol",
            "method": "tools/call",
            "params": {
                "name": "symbol",
                "arguments": {"query": "configure"}
            }
        }),
    );
    let error = assert_tool_error(&ambiguous, json!("ambiguous-symbol"));
    assert_eq!(
        error.pointer("/code").and_then(Value::as_str),
        Some("ambiguous_target"),
        "stdio symbol ambiguity should expose structured error data: {ambiguous}"
    );
    let alternatives = error
        .pointer("/alternatives")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ambiguous alternatives: {ambiguous}"));
    assert!(alternatives.len() >= 2);
    let second_id = alternatives[1]
        .get("node_id")
        .and_then(Value::as_str)
        .expect("second alternative node id")
        .to_string();

    let chosen = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "chosen-symbol",
            "method": "tools/call",
            "params": {
                "name": "symbol",
                "arguments": {"query": "configure", "choose": 2}
            }
        }),
    );
    let result = assert_tool_success(&chosen, json!("chosen-symbol"));
    assert_eq!(
        result.pointer("/node/id").and_then(Value::as_str),
        Some(second_id.as_str()),
        "stdio symbol choose should resolve displayed alternative #2: {chosen}"
    );
}

#[test]
fn unknown_method_returns_jsonrpc_error() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 20, "method": "codestory/nope"}),
    );

    let error = assert_error_envelope(&response, json!(20));
    assert_error_code(error, -32601);
    let message = error["message"]
        .as_str()
        .expect("error message")
        .to_ascii_lowercase();
    assert!(
        message.contains("method not found") || message.contains("unknown method"),
        "unknown method message should be stable: {response}"
    );
}

#[test]
fn invalid_json_returns_parse_error_with_null_id() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_line(
        &mut server,
        r#"{"jsonrpc":"2.0","id":21,"method":"tools/list""#,
    );

    let error = assert_error_envelope(&response, Value::Null);
    assert_error_code(error, -32700);
    let message = error["message"]
        .as_str()
        .expect("error message")
        .to_ascii_lowercase();
    assert!(
        message.contains("parse error") || message.contains("json"),
        "invalid JSON message should mention parsing: {response}"
    );
}

#[test]
fn oversized_stdio_frame_returns_structured_protocol_error() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let oversized = "x".repeat(1024 * 1024 + 1);

    let response = send_line(&mut server, &oversized);

    let error = assert_error_envelope(&response, Value::Null);
    assert_error_code(error, -32700);
    assert_eq!(
        error.pointer("/data/code").and_then(Value::as_str),
        Some("stdio_frame_too_large"),
        "oversized frame should use a structured protocol error: {response}"
    );
    assert_eq!(
        error
            .pointer("/data/max_frame_bytes")
            .and_then(Value::as_u64),
        Some(1024 * 1024),
        "oversized frame error should report the configured byte limit: {response}"
    );
    assert!(
        error
            .pointer("/data/line_bytes")
            .and_then(Value::as_u64)
            .is_some_and(|bytes| bytes > 1024 * 1024),
        "oversized frame error should report observed line bytes: {response}"
    );

    let follow_up = send_json(
        &mut server,
        json!({"jsonrpc": "2.0", "id": "after-oversized", "method": "initialize"}),
    );
    assert_success_envelope(&follow_up, json!("after-oversized"));
}

#[test]
fn bad_tool_call_args_return_jsonrpc_error() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": 22,
            "method": "tools/call",
            "params": {"arguments": {"query": "AppController"}}
        }),
    );

    let error = assert_error_envelope(&response, json!(22));
    assert_error_code(error, -32602);
    assert!(
        error["message"]
            .as_str()
            .expect("error message")
            .contains("tool"),
        "bad tools/call args should name the tool problem: {response}"
    );
}

#[test]
fn not_found_resource_returns_jsonrpc_error() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": 23,
            "method": "resources/read",
            "params": {"uri": "codestory://missing/resource"}
        }),
    );

    let error = assert_error_envelope(&response, json!(23));
    assert_error_code(error, -32602);
    let message = error["message"].as_str().expect("error message");
    assert!(
        message.contains("unknown resource") || message.contains("not found"),
        "not-found resource message should be stable: {response}"
    );
}

#[test]
fn unknown_prompt_returns_jsonrpc_error() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": 24,
            "method": "prompts/get",
            "params": {"name": "not_a_prompt"}
        }),
    );

    let error = assert_error_envelope(&response, json!(24));
    assert_error_code(error, -32602);
    assert!(
        error["message"]
            .as_str()
            .expect("error message")
            .contains("Unknown prompt"),
        "unknown prompt message should identify the missing prompt: {response}"
    );
}
