mod test_support;

use fs4::fs_std::FileExt as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
    latest_release_version: Option<String>,
    disable_release_probe: bool,
    disable_installed_cli_probe: bool,
    plugin_data_dir: Option<PathBuf>,
    plugin_cli_source: Option<String>,
    dirty_marker_path: Option<PathBuf>,
    dirty_marker_project_root: Option<PathBuf>,
    local_refresh_timeout_ms: Option<u64>,
}

struct StdioServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
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
    let workspace = tempfile::tempdir().expect("workspace dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let mut command = test_support::cli_command();
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
    allow_explicit_cpu_embeddings(&mut command);
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
        latest_release_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        disable_release_probe: false,
        disable_installed_cli_probe: false,
        plugin_data_dir: None,
        plugin_cli_source: None,
        dirty_marker_path: None,
        dirty_marker_project_root: None,
        local_refresh_timeout_ms: None,
    }
}

fn write_dirty_marker_fixture(fixture: &StdioFixture, name: &str, marker: Value) -> PathBuf {
    let marker_path = fixture.cache_dir.path().join(name);
    thread::sleep(Duration::from_millis(25));
    fs::write(&marker_path, marker.to_string()).expect("write dirty marker");
    marker_path
}

fn write_live_local_refresh(fixture: &StdioFixture) -> u32 {
    let project_root = fs::canonicalize(fixture.workspace.path())
        .expect("canonical workspace")
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_millis() as i64;
    let pid = std::process::id();
    fs::write(
        fixture.cache_dir.path().join("local-refresh.lock"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "project_root": project_root,
            "pid": pid,
            "started_at_epoch_ms": now,
            "token": format!("test:{pid}:{now}")
        }))
        .expect("serialize refresh lock"),
    )
    .expect("write refresh lock");
    fs::write(
        fixture.cache_dir.path().join("local-refresh-status.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "status": "refreshing",
            "project_root": project_root,
            "phase": "incremental_index",
            "pid": pid,
            "started_at_epoch_ms": now,
            "updated_at_epoch_ms": now,
            "last_failure_reason": null
        }))
        .expect("serialize refresh status"),
    )
    .expect("write refresh status");
    pid
}

fn refresh_fixture_index(fixture: &StdioFixture) {
    let mut command = test_support::cli_command();
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
    allow_explicit_cpu_embeddings(&mut command);
    let output = command.output().expect("run index refresh");
    assert!(
        output.status.success(),
        "index refresh failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn unindexed_fixture() -> StdioFixture {
    let workspace = tempfile::tempdir().expect("workspace dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    StdioFixture {
        workspace,
        cache_dir,
        latest_release_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        disable_release_probe: false,
        disable_installed_cli_probe: false,
        plugin_data_dir: None,
        plugin_cli_source: None,
        dirty_marker_path: None,
        dirty_marker_project_root: None,
        local_refresh_timeout_ms: None,
    }
}

fn write_managed_cli_fixture(plugin_data: &Path, version: &str) -> PathBuf {
    let version_dir = plugin_data.join("codestory-cli").join(version);
    let bin_dir = version_dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("create managed CLI fixture dir");
    let executable = bin_dir.join(if cfg!(windows) {
        "codestory-cli.exe"
    } else {
        "codestory-cli"
    });
    let content = format!("managed CLI fixture {version}");
    fs::write(&executable, content.as_bytes()).expect("write managed CLI fixture");
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    fs::write(
        version_dir.join("manifest.json"),
        json!({
            "path": format!("bin/{}", executable.file_name().unwrap().to_string_lossy()),
            "sha256": sha256,
            "version": version
        })
        .to_string(),
    )
    .expect("write managed CLI fixture manifest");
    executable
}

fn allow_explicit_cpu_embeddings(command: &mut Command) {
    command.env("CODESTORY_EMBED_ALLOW_CPU", "1");
}

fn spawn_stdio_server(fixture: &StdioFixture) -> StdioServer {
    let state_root = fixture.cache_dir.path().join("test-state");
    let mut command = test_support::cli_command();
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
        .stderr(Stdio::piped())
        .env("CODESTORY_CACHE_ROOT", state_root.join("cache"))
        .env("CODESTORY_STDIO_CACHE_ROOT", state_root.join("stdio-cache"))
        .env("CODESTORY_PLUGIN_DATA", state_root.join("plugin-data"));
    allow_explicit_cpu_embeddings(&mut command);
    if let Some(version) = &fixture.latest_release_version {
        command.env("CODESTORY_LATEST_RELEASE_VERSION", version);
    }
    if fixture.disable_release_probe {
        command.env("CODESTORY_DISABLE_RELEASE_PROBE", "1");
    }
    if fixture.disable_installed_cli_probe {
        command.env("CODESTORY_DISABLE_INSTALLED_CLI_PROBE", "1");
    }
    if let Some(plugin_data) = &fixture.plugin_data_dir {
        command.env("CODESTORY_PLUGIN_DATA", plugin_data);
    }
    if let Some(source) = &fixture.plugin_cli_source {
        command.env("CODESTORY_PLUGIN_CLI_SOURCE", source);
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

fn spawn_multi_project_stdio_server(cache_root: &Path) -> StdioServer {
    let mut child = test_support::cli_command()
        .arg("serve")
        .arg("--stdio")
        .arg("--multi-project")
        .arg("--refresh")
        .arg("full")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CODESTORY_EMBED_ALLOW_CPU", "1")
        .env("CODESTORY_STDIO_CACHE_ROOT", cache_root)
        .env("CODESTORY_PLUGIN_MULTI_PROJECT", "1")
        .spawn()
        .expect("spawn multi-project stdio server");
    let stdin = child.stdin.take().expect("multi-project stdio stdin");
    let stdout = BufReader::new(child.stdout.take().expect("multi-project stdio stdout"));
    StdioServer {
        child,
        stdin,
        stdout,
    }
}

fn send_json(server: &mut StdioServer, request: Value) -> Value {
    send_line(server, &request.to_string())
}

fn initialize_stdio_server(server: &mut StdioServer, id: &str) {
    let response = send_json(
        server,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "contract-test", "version": "0"}
            }
        }),
    );
    assert_success_envelope(&response, json!(id));
}

fn send_line(server: &mut StdioServer, line: &str) -> Value {
    writeln!(server.stdin, "{line}").expect("write request line");
    server.stdin.flush().expect("flush request line");
    read_json(server)
}

fn read_json(server: &mut StdioServer) -> Value {
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

fn assert_search_repaired_before_terminal_model_absence(
    server: &mut StdioServer,
    error: &Value,
    search_generations: &Path,
    id: &str,
) {
    assert_eq!(error["code"], json!("codestory_unavailable"));
    assert_eq!(error["cause_code"], json!("native_model_not_embedded"));
    assert_eq!(error["retry_tool"], Value::Null);
    assert!(
        search_generations.is_dir(),
        "search repair must complete before the terminal package limitation is reported"
    );
    let ground_id = format!("{id}-local-ground");
    let ground = send_json(
        server,
        json!({
            "jsonrpc": "2.0",
            "id": ground_id,
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {"budget": "strict"}}
        }),
    );
    assert_tool_success(&ground, json!(ground_id));
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
        surface_status.get("summary"),
        None,
        "ordinary surface {surface} must reference, not clone, its verdict"
    );
    let verdict = status["readiness"]
        .as_array()
        .and_then(|readiness| {
            readiness
                .iter()
                .find(|verdict| verdict["goal"] == expected_goal)
        })
        .unwrap_or_else(|| panic!("missing canonical readiness verdict {expected_goal}: {status}"));
    assert_eq!(
        verdict["status"],
        json!(expected_status),
        "unexpected canonical readiness status for {surface}: {verdict}"
    );
    assert!(
        verdict["summary"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "canonical verdict should include a readiness summary for {surface}: {verdict}"
    );
    if expected_allowed {
        assert_eq!(verdict["status"], "ready");
    } else {
        assert_eq!(
            verdict.get("minimum_next"),
            None,
            "normal status must not ask the user to prepare retrieval manually: {verdict}"
        );
    }
}

fn assert_activation_surface(status: &Value, surface: &str) {
    let surface_status = status
        .pointer(&format!("/allowed_surfaces/{surface}"))
        .unwrap_or_else(|| panic!("status should include allowed_surfaces.{surface}: {status}"));
    assert_eq!(surface_status["allowed"], json!(true));
    assert_eq!(surface_status["activation_required"], json!(true));
    assert_eq!(surface_status["readiness_goal"], json!("local_navigation"));
    assert!(surface_status.get("failed_layer").is_none());
    assert_eq!(
        status["readiness"][0]["status"],
        json!("unavailable"),
        "callable bootstrap surfaces must not misreport the publication as ready: {status}"
    );
}

fn assert_ground_activation_call(status: &Value) {
    let calls = status["recommended_next_calls"]
        .as_array()
        .expect("status should include recommended next calls");
    assert_eq!(
        calls.len(),
        1,
        "local activation should require one call: {status}"
    );
    assert_eq!(calls[0]["method"], json!("tools/call"));
    assert_eq!(calls[0]["tool"], json!("ground"));
    assert_eq!(calls[0]["arguments"]["project"], status["project_root"]);
    assert_eq!(calls[0]["arguments"]["budget"], json!("balanced"));
    assert_eq!(calls[0]["activation_required"], json!(true));
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
        .find(|content| {
            content["uri"] == uri
                || content["uri"]
                    .as_str()
                    .is_some_and(|candidate| candidate.starts_with(&format!("{uri}?project=")))
        })
        .unwrap_or_else(|| panic!("resource read should include content for {uri}: {result}"));
    assert_eq!(content["mimeType"], "application/json");
    let text = content["text"]
        .as_str()
        .unwrap_or_else(|| panic!("resource {uri} content should include JSON text: {content}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("resource {uri} should be parseable JSON: {error}\n{text}"))
}

fn strict_resource_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("encode resource component");
        }
    }
    encoded
}

fn project_resource_uri(base_uri: &str, project: &Path) -> String {
    format!(
        "{base_uri}?project={}",
        strict_resource_component(project.to_string_lossy().as_ref())
    )
}

fn assert_tool_safety_metadata(tool: &Value) {
    let name = tool["name"].as_str().expect("tool name");
    let observational = name == "status";
    let annotations = tool
        .get("annotations")
        .unwrap_or_else(|| panic!("{name} should include MCP-style annotations: {tool}"));
    let safety = tool
        .get("safety")
        .or_else(|| tool.get("metadata"))
        .unwrap_or_else(|| panic!("{name} should include safety metadata: {tool}"));

    assert!(
        annotations.get("readOnlyHint").and_then(Value::as_bool) == Some(observational)
            && safety.get("readOnly").and_then(Value::as_bool) == Some(observational),
        "{name} should distinguish observation from managed activation: {tool}"
    );
    assert_eq!(
        safety.get("effect").and_then(Value::as_str),
        Some(if observational {
            "read_only"
        } else {
            "managed_activation"
        }),
        "{name} should label its effect truthfully: {tool}"
    );
    assert_eq!(
        safety.get("activatesProject").and_then(Value::as_bool),
        Some(!observational),
        "{name} should declare whether it activates managed local state: {tool}"
    );
    assert_eq!(
        safety.get("writesRepository").and_then(Value::as_bool),
        Some(false),
        "{name} must not edit repository source: {tool}"
    );
    assert_eq!(
        safety.get("requiresConfirmation").and_then(Value::as_bool),
        Some(false),
        "{name} should not ask the user to confirm managed local preparation: {tool}"
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
    assert_eq!(
        safety.get("localOnly").and_then(Value::as_bool),
        Some(observational),
        "{name} should reserve local-only for the observational status tool: {tool}"
    );
    assert_eq!(
        safety.get("openWorld").and_then(Value::as_bool),
        Some(!observational),
        "{name} should disclose automatic managed downloads: {tool}"
    );
    assert_eq!(
        annotations.get("openWorldHint").and_then(Value::as_bool),
        Some(!observational),
        "{name} annotations should match managed network behavior: {tool}"
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
fn stdio_status_observes_unbuilt_index_and_ground_activates_it() {
    let fixture = unindexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let status_uri = project_resource_uri("codestory://status", fixture.workspace.path());

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
            "params": {"uri": status_uri}
        }),
    );
    let status_result = assert_success_envelope(&status_response, json!("status-unindexed"));
    let status = json_resource_content(status_result, "codestory://status");

    assert_eq!(status["readiness"][0]["status"], json!("unavailable"));
    assert!(status["index_publication"].is_null());
    assert_eq!(
        status["readiness"][0]["index"]["indexed_file_count"],
        json!(0)
    );
    for surface in ["ground", "files", "affected"] {
        assert_activation_surface(&status, surface);
    }
    assert_allowed_surface(&status, "symbol", false, "local_navigation", "unavailable");
    assert_allowed_surface(
        &status,
        "search",
        false,
        "agent_packet_search",
        "unavailable",
    );
    assert_ground_activation_call(&status);

    let ground = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-unindexed",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {"budget": "strict"}}
        }),
    );
    let grounding = assert_tool_success(&ground, json!("ground-unindexed"));
    assert!(
        grounding["stats"]["file_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "first ground call should return a non-empty repository map: {grounding}"
    );

    let refreshed = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-indexed-after-ground",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let refreshed = json_resource_content(
        assert_success_envelope(&refreshed, json!("status-indexed-after-ground")),
        "codestory://status",
    );
    assert_eq!(refreshed["readiness"][0]["status"], json!("ready"));
    assert_allowed_surface(&refreshed, "ground", true, "local_navigation", "ready");
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
fn multi_project_stdio_routes_interleaved_requests_by_explicit_project() {
    let first = tempfile::tempdir().expect("first workspace");
    let second = tempfile::tempdir().expect("second workspace");
    let cache_root = tempfile::tempdir().expect("multi-project cache root");
    write_tiny_rust_workspace(first.path());
    write_tiny_rust_workspace(second.path());
    fs::write(
        first.path().join("src").join("first_only.rs"),
        "pub fn first_only() {}\n",
    )
    .expect("write first-only source");
    fs::write(
        second.path().join("src").join("second_only.rs"),
        "pub fn second_only() {}\n",
    )
    .expect("write second-only source");

    let mut server = spawn_multi_project_stdio_server(cache_root.path());
    let tools = assert_success_envelope(
        &send_json(
            &mut server,
            json!({"jsonrpc": "2.0", "id": "multi-tools", "method": "tools/list"}),
        ),
        json!("multi-tools"),
    )
    .clone();
    assert!(
        tools["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .all(|tool| {
                tool.pointer("/inputSchema/required")
                    .and_then(Value::as_array)
                    .is_some_and(|required| required.contains(&json!("project")))
            }),
        "every MCP tool must require explicit project routing: {tools}"
    );

    let missing = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "multi-missing-project",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {"budget": "strict"}}
        }),
    );
    assert_eq!(
        assert_tool_error(&missing, json!("multi-missing-project"))["code"],
        json!("project_required")
    );
    let relative = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "multi-relative-project",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"project": ".", "budget": "strict"}
            }
        }),
    );
    assert_eq!(
        assert_tool_error(&relative, json!("multi-relative-project"))["code"],
        json!("project_required")
    );
    let unavailable = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "multi-unavailable-project",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"project": first.path().join("missing"), "budget": "strict"}
            }
        }),
    );
    assert_eq!(
        assert_tool_error(&unavailable, json!("multi-unavailable-project"))["code"],
        json!("project_unavailable")
    );

    let ground_request = |id: &str, project: &Path| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"project": project, "budget": "strict"}
            }
        })
    };
    writeln!(
        server.stdin,
        "{}",
        ground_request("multi-first", first.path())
    )
    .expect("queue first project request");
    writeln!(
        server.stdin,
        "{}",
        ground_request("multi-second", second.path())
    )
    .expect("queue second project request");
    server.stdin.flush().expect("flush interleaved requests");
    let first_response = read_json(&mut server);
    let second_response = read_json(&mut server);
    let first_snapshot = assert_tool_success(&first_response, json!("multi-first")).clone();
    let second_snapshot = assert_tool_success(&second_response, json!("multi-second")).clone();

    let first_again = {
        let response = send_json(
            &mut server,
            ground_request("multi-first-again", first.path()),
        );
        assert_tool_success(&response, json!("multi-first-again")).clone()
    };

    let first_root = fs::canonicalize(first.path()).expect("canonical first workspace");
    let second_root = fs::canonicalize(second.path()).expect("canonical second workspace");
    assert_eq!(
        PathBuf::from(first_snapshot["root"].as_str().expect("first root")),
        first_root
    );
    assert_eq!(
        PathBuf::from(second_snapshot["root"].as_str().expect("second root")),
        second_root
    );
    assert_eq!(first_snapshot["root"], first_again["root"]);
    assert_ne!(first_snapshot["root"], second_snapshot["root"]);

    let affected_request = |id: &str, project: &Path, path: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"project": project, "paths": [path]}
            }
        })
    };
    let read_affected = |server: &mut StdioServer, id: &str, project: &Path, path: &str| {
        let response = send_json(server, affected_request(id, project, path));
        assert_tool_success(&response, json!(id)).clone()
    };
    let first_affected = read_affected(
        &mut server,
        "multi-first-affected",
        first.path(),
        "src/first_only.rs",
    );
    let second_affected = read_affected(
        &mut server,
        "multi-second-affected",
        second.path(),
        "src/second_only.rs",
    );
    let first_affected_again = read_affected(
        &mut server,
        "multi-first-affected-again",
        first.path(),
        "src/first_only.rs",
    );
    assert_eq!(
        first_affected["project_root"],
        first_affected_again["project_root"]
    );
    assert_ne!(
        first_affected["project_root"],
        second_affected["project_root"]
    );
    assert_eq!(
        first_affected["changed_paths"],
        json!(["src/first_only.rs"])
    );
    assert_eq!(
        second_affected["changed_paths"],
        json!(["src/second_only.rs"])
    );
    assert_eq!(
        first_affected_again["changed_paths"],
        json!(["src/first_only.rs"])
    );

    let status_request = |id: &str, project: &Path| {
        let uri = project_resource_uri("codestory://status", project);
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "resources/read",
            "params": {"uri": uri}
        })
    };
    let read_status = |server: &mut StdioServer, id: &str, project: &Path| {
        let response = send_json(server, status_request(id, project));
        json_resource_content(
            assert_success_envelope(&response, json!(id)),
            "codestory://status",
        )
    };
    let first_status = read_status(&mut server, "multi-first-status", first.path());
    let second_status = read_status(&mut server, "multi-second-status", second.path());
    let first_status_again = read_status(&mut server, "multi-first-status-again", first.path());
    for (status, expected_root) in [
        (&first_status, &first_root),
        (&second_status, &second_root),
        (&first_status_again, &first_root),
    ] {
        assert_eq!(
            fs::canonicalize(
                status["project_root"]
                    .as_str()
                    .expect("status project root")
            )
            .expect("canonical status project root"),
            *expected_root,
            "status crossed project roots: {status}"
        );
    }
    assert_ne!(first_status["storage_path"], second_status["storage_path"]);
    for pointer in ["/project_root", "/storage_path", "/retrieval_mode"] {
        assert_eq!(
            first_status.pointer(pointer),
            first_status_again.pointer(pointer),
            "A/B/A status identity drifted at {pointer}"
        );
    }

    let first_symbol = assert_tool_success(
        &send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": "multi-first-symbol",
                "method": "tools/call",
                "params": {
                    "name": "symbol",
                    "arguments": {"project": first.path(), "query": "first_only"}
                }
            }),
        ),
        json!("multi-first-symbol"),
    )
    .clone();
    let first_node_id = first_symbol
        .pointer("/node/id")
        .or_else(|| first_symbol.pointer("/resolution/resolved/node_id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("first project should resolve first_only: {first_symbol}"));
    let wrong_project_uri = format!(
        "codestory://symbol/{}?project={}",
        strict_resource_component(first_node_id),
        strict_resource_component(second.path().to_string_lossy().as_ref())
    );
    let wrong_project = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "multi-first-node-through-second",
            "method": "resources/read",
            "params": {"uri": wrong_project_uri}
        }),
    );
    assert!(
        wrong_project.get("error").is_some()
            || wrong_project
                .pointer("/result/contents/0/text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("not found")),
        "a node selected from the first repository must not resolve through the second: {wrong_project}"
    );
}

#[test]
fn multi_project_packet_repairs_keep_operation_identity_project_scoped() {
    let projects = (0..3)
        .map(|index| {
            let project = tempfile::tempdir().expect("project workspace");
            write_tiny_rust_workspace(project.path());
            fs::write(
                project
                    .path()
                    .join("src")
                    .join(format!("project_{index}.rs")),
                format!("pub fn project_{index}() {{}}\n"),
            )
            .expect("write project-specific source");
            project
        })
        .collect::<Vec<_>>();
    let cache_root = tempfile::tempdir().expect("multi-project cache root");
    let writer_locks = projects
        .iter()
        .map(|project| {
            let root = fs::canonicalize(project.path()).expect("canonical project root");
            let cache_dir = cache_root
                .path()
                .join(codestory_workspace::workspace_id_v3_for_root(&root));
            fs::create_dir_all(&cache_dir).expect("create project cache dir");
            let lock_path = cache_dir
                .join("codestory.db")
                .with_extension("index-writer.lock");
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)
                .expect("open project writer lock");
            assert!(
                file.try_lock_exclusive().expect("take project writer lock"),
                "test must own the project writer lock"
            );
            file
        })
        .collect::<Vec<_>>();
    let mut server = spawn_multi_project_stdio_server(cache_root.path());
    let packet_request = |id: &str, project: &Path| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "packet",
                "arguments": {
                    "project": project,
                    "question": "How does AppController open a project?"
                }
            }
        })
    };

    let mut operation_ids = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        let id = format!("multi-packet-{index}");
        let response = send_json(&mut server, packet_request(&id, project.path()));
        let error = assert_tool_error(&response, json!(id));
        assert_eq!(error["code"], json!("codestory_preparing"));
        assert_eq!(error["cause_code"], json!("cache_busy"));
        assert_eq!(error["retry_tool"], json!("packet"));
        assert!(error["retry_after_ms"].as_u64().is_some());
        operation_ids.push(
            error["operation"]["operation_id"]
                .as_str()
                .expect("project activation operation id")
                .to_string(),
        );
    }
    assert_eq!(
        operation_ids.iter().collect::<BTreeSet<_>>().len(),
        3,
        "each project needs an independent activation operation"
    );

    let retry = send_json(
        &mut server,
        packet_request("multi-packet-first-retry", projects[0].path()),
    );
    let retry_error = assert_tool_error(&retry, json!("multi-packet-first-retry"));
    assert_eq!(
        retry_error["operation"]["operation_id"],
        json!(operation_ids[0]),
        "retrying one project must not adopt another project's operation"
    );

    drop(writer_locks);
    for (index, project) in projects.iter().enumerate() {
        let id = format!("multi-ground-after-lock-{index}");
        let response = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "ground",
                    "arguments": {"project": project.path(), "budget": "strict"}
                }
            }),
        );
        let snapshot = assert_tool_success(&response, json!(id));
        assert_eq!(
            fs::canonicalize(snapshot["root"].as_str().expect("grounding root"))
                .expect("canonical grounding root"),
            fs::canonicalize(project.path()).expect("canonical expected root")
        );
    }
}

#[test]
fn tool_catalog_keeps_stable_product_tool_names() {
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
            "search",
            "shortest_path",
            "snippet",
            "status",
            "symbol",
            "symbols",
            "trace",
            "trail",
        ],
        "stdio product tool names should stay stable: {tools}"
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
            && packet_description.contains("repository evidence")
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
            && ground_description.contains("orientation")
            && ground_description.contains("managed retrieval preparation"),
        "ground description should connect the tool to orientation and automatic preparation: {ground_description}"
    );
    let files_description = tool_by_name(&tools, "files")["description"]
        .as_str()
        .expect("files description");
    assert!(
        files_description.contains("indexed files")
            && files_description.contains("locally fresh index")
            && files_description.contains("refreshes the repository map")
            && files_description.contains("does not wait for broad search"),
        "files description should make the local-refresh boundary explicit: {files_description}"
    );
    let affected_description = tool_by_name(&tools, "affected")["description"]
        .as_str()
        .expect("affected description");
    assert!(
        affected_description.contains("last complete local index")
            && affected_description.contains("preserving bounded stale and error evidence")
            && affected_description.contains("Cold or partial state may trigger managed indexing")
            && affected_description.contains("Never discovers git changes")
            && affected_description.contains("does not wait for broad search"),
        "affected description should state its last-complete and activation boundary: {affected_description}"
    );
    let snippet_description = tool_by_name(&tools, "snippet")["description"]
        .as_str()
        .expect("snippet description");
    assert!(
        snippet_description.contains("after packet, search, or graph evidence"),
        "snippet description should not be the first stop for broad structural questions: {snippet_description}"
    );

    for tool in tools["tools"].as_array().expect("tools array") {
        assert_tool_safety_metadata(tool);
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
        Some(&json!(100)),
        "files.limit should document the compact agent default: {files}"
    );
    assert_eq!(
        files_limit.get("maximum"),
        Some(&json!(500)),
        "files.limit should document the stdio hard cap: {files}"
    );

    let affected = tool_input_schema(&tools, "affected");
    assert_eq!(
        schema_property(affected, "paths")["type"],
        "array",
        "affected.paths should be the preferred array input: {affected}"
    );
    assert_eq!(
        schema_property(affected, "paths")["items"]["type"],
        "string",
        "affected.paths should contain strings: {affected}"
    );
    for field in ["paths", "changed_paths"] {
        assert_eq!(
            schema_property(affected, field)["items"]["minLength"],
            json!(1),
            "affected.{field} items should reject empty strings in the exposed contract: {affected}"
        );
    }
    for field in ["paths", "changed_paths", "change_records"] {
        let property = schema_property(affected, field);
        assert_eq!(
            property.get("minItems"),
            Some(&json!(1)),
            "affected.{field} should require at least one entry: {affected}"
        );
        assert_eq!(
            property.get("maxItems"),
            Some(&json!(200)),
            "affected.{field} should expose the adapter hard cap: {affected}"
        );
    }
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
    assert!(
        affected.get("anyOf").is_none(),
        "affected exact-one input contract should not be described as anyOf: {affected}"
    );
    let affected_one_of = affected["oneOf"].as_array().unwrap_or_else(|| {
        panic!("affected should require exactly one path source via oneOf: {affected}")
    });
    assert!(
        affected_one_of
            .iter()
            .any(|branch| required_fields(branch).contains("paths"))
            && affected_one_of
                .iter()
                .any(|branch| required_fields(branch).contains("changed_paths"))
            && affected_one_of
                .iter()
                .any(|branch| required_fields(branch).contains("change_records")),
        "affected should require exactly one of paths, changed_paths, or change_records: {affected}"
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
    let snippet = tool_input_schema(&tools, "snippet");
    assert_schema_enum_values(
        snippet,
        "/properties/scope/enum",
        &["function_body", "line_context"],
    );
    assert_eq!(
        schema_property(snippet, "scope").get("default"),
        Some(&json!("line_context"))
    );
    for field in ["context", "lines"] {
        assert_eq!(
            schema_property(snippet, field).get("maximum"),
            Some(&json!(200)),
            "snippet.{field} should expose the bounded source window: {snippet}"
        );
    }
    assert_eq!(
        schema_property(snippet, "function_body")["type"],
        "boolean",
        "snippet.function_body should preserve the documented CLI selector: {snippet}"
    );

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
            for field in ["stats", "coverage", "orientation", "root_symbols", "files"] {
                assert!(
                    required_fields(output_schema).contains(field),
                    "ground outputSchema should require grounding DTO field {field}: {tool}"
                );
            }
            let orientation = schema_property(output_schema, "orientation");
            assert_eq!(
                orientation["additionalProperties"],
                json!(false),
                "ground orientation outputSchema should reject fields outside the DTO: {tool}"
            );
            for field in [
                "confidence",
                "total_root_candidates",
                "evaluated_root_candidates",
                "candidate_entrypoint_roots",
                "selected_entrypoint_roots",
                "candidate_subsystems",
                "selected_subsystems",
                "uncertainty",
            ] {
                assert!(
                    required_fields(orientation).contains(field),
                    "ground orientation outputSchema should require DTO field {field}: {tool}"
                );
            }
            assert_schema_enum_values(
                orientation,
                "/properties/confidence/enum",
                &["strong", "partial", "weak"],
            );
            assert_schema_enum_values(
                orientation,
                "/properties/uncertainty/items/enum",
                &[
                    "bounded_candidate_window",
                    "no_entrypoint_evidence",
                    "entrypoint_evidence_omitted",
                    "limited_subsystem_breadth",
                    "compressed_presentation",
                ],
            );
        }
        if name == "files" {
            for field in [
                "project_root",
                "usable",
                "summary",
                "files",
                "policy_exclusions",
            ] {
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
            let exclusion_schema = output_schema
                .pointer("/properties/policy_exclusions/items")
                .unwrap_or_else(|| {
                    panic!("files outputSchema should describe policy exclusions: {tool}")
                });
            for field in [
                "path",
                "content_hash",
                "observed_size",
                "observed_unit_count",
                "policy_version",
                "byte_cap",
                "structural_unit_cap",
                "project_id",
                "workspace_id",
                "core_generation_id",
                "core_run_id",
                "graph_coverage",
                "semantic_coverage",
            ] {
                assert!(
                    required_fields(exclusion_schema).contains(field),
                    "policy exclusion schema should require {field}: {tool}"
                );
            }
        }
        if name == "affected" {
            for field in [
                "project_root",
                "changed_paths",
                "change_records",
                "matched_files",
                "uncovered_inputs",
                "matched_file_count",
                "depth",
                "impacted_symbols",
                "impacted_tests",
                "bounds",
                "completeness",
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
            let unmatched_schema = output_schema
                .pointer("/properties/unmatched_paths/items")
                .unwrap_or_else(|| {
                    panic!("affected outputSchema should describe unmatched paths: {tool}")
                });
            assert_schema_enum_values(
                unmatched_schema,
                "/properties/classification/enum",
                &[
                    "valid_uncovered",
                    "missing",
                    "expected_deleted",
                    "rename_unresolved",
                    "stale_index",
                    "malformed",
                    "unavailable_evidence",
                ],
            );
            assert!(
                output_schema.pointer("/properties/next_commands").is_none(),
                "affected outputSchema should not duplicate structured follow-ups as command strings: {tool}"
            );
            let follow_up_schema = output_schema
                .pointer("/properties/follow_ups/items")
                .unwrap_or_else(|| panic!("affected should describe follow-up actions: {tool}"));
            let invocation_schema = schema_property(follow_up_schema, "invocation");
            assert_eq!(
                schema_property(invocation_schema, "program")["type"],
                "string",
                "affected follow-up invocations should expose a program: {tool}"
            );
            assert_eq!(
                schema_property(invocation_schema, "args")["items"]["type"],
                "string",
                "affected follow-up invocations should expose an argv array: {tool}"
            );
            let completeness_schema = schema_property(output_schema, "completeness");
            assert_eq!(
                schema_property(completeness_schema, "truncation_reasons")["items"]["type"],
                "string",
                "affected completeness should describe field-specific truncation reasons: {tool}"
            );
        }
    }

    let search_hit_schema = tool_output_schema(&tools, "search")
        .pointer("/properties/hits/items")
        .unwrap_or_else(|| panic!("search outputSchema should describe hit items: {tools}"));
    let search_output_schema = tool_output_schema(&tools, "search");
    assert_eq!(
        schema_property(search_output_schema, "counts")["type"],
        json!("object"),
        "search outputSchema should expose compact source counts: {search_output_schema}"
    );
    for removed_diagnostic in [
        "search_plan",
        "retrieval_shadow",
        "suggestions",
        "indexed_symbol_hits",
        "repo_text_hits",
    ] {
        assert!(
            search_output_schema["properties"]
                .get(removed_diagnostic)
                .is_none(),
            "search outputSchema should omit duplicated or diagnostic field {removed_diagnostic}: {search_output_schema}"
        );
    }
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

    let citation_schema = tool_output_schema(&tools, "context")
        .pointer("/properties/citations/items")
        .unwrap_or_else(|| panic!("context outputSchema should describe agent citations: {tools}"));
    for (surface, schema) in [
        ("search hit", search_hit_schema),
        ("agent citation", citation_schema),
    ] {
        for field in [
            "evidence_tier",
            "evidence_producer",
            "resolution_status",
            "eligible_for_sufficiency",
        ] {
            assert!(
                !required_fields(schema).contains(field),
                "{surface} evidence field {field} must remain optional: {schema}"
            );
        }
        assert_schema_enum_values(
            schema,
            "/properties/evidence_tier/enum",
            &[
                "exact_source",
                "structural_text",
                "resolved_graph",
                "lexical_source",
                "symbol_doc",
                "component_report",
                "dense_semantic",
                "synthetic_source_scan",
                "generated_summary",
            ],
        );
        assert_schema_enum_values(
            schema,
            "/properties/resolution_status/enum",
            &[
                "resolved",
                "source_range_only",
                "unresolved",
                "diagnostic_only",
            ],
        );
        assert_eq!(
            schema_property(schema, "evidence_producer")["type"],
            "string",
            "{surface} outputSchema should expose the evidence producer: {schema}"
        );
        assert_eq!(
            schema_property(schema, "eligible_for_sufficiency")["type"],
            "boolean",
            "{surface} outputSchema should expose the sufficiency flag: {schema}"
        );
    }

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
    for field in [
        "max_snippet_bytes",
        "range_source",
        "fallback_reason",
        "truncation_guidance",
    ] {
        assert!(
            !required_fields(snippet).contains(field),
            "snippet outputSchema should keep conditionally emitted DTO field {field} optional: {snippet}"
        );
        let _ = schema_property(snippet, field);
    }
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
        vec!["codestory://agent-guide"],
        "only static project-free resources belong in resources/list: {resources}"
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
            "codestory://grounding{?project}",
            "codestory://project{?project}",
            "codestory://references/{node_id}{?project}",
            "codestory://snippet/{node_id}{?project}",
            "codestory://status{?project}",
            "codestory://symbol/{node_id}{?project}",
            "codestory://symbols/root{?project}",
            "codestory://trail/{node_id}{?project}",
        ],
        "every advertised repository-reading resource template should carry an explicit project selector: {templates}"
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
    assert_eq!(
        sorted_field_values(&resources, "resources", "uri"),
        vec!["codestory://agent-guide"],
        "resources/list should contain only static project-free resources: {resources}"
    );

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
            .any(|template| {
                template["uriTemplate"] == "codestory://symbol/{node_id}{?project}"
            }),
        "resources/templates/list should include a project-bound symbol template: {templates}"
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
    let orientation = snapshot["orientation"]
        .as_object()
        .unwrap_or_else(|| panic!("ground should return typed orientation evidence: {snapshot}"));
    for field in [
        "confidence",
        "total_root_candidates",
        "evaluated_root_candidates",
        "candidate_entrypoint_roots",
        "selected_entrypoint_roots",
        "candidate_subsystems",
        "selected_subsystems",
        "uncertainty",
    ] {
        assert!(
            orientation.contains_key(field),
            "ground orientation should emit DTO field {field}: {snapshot}"
        );
    }

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
fn snippet_tool_exact_id_navigates_structural_evidence_but_query_stays_typed() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let ground_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-structural-snippet",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {"budget": "balanced"}}
        }),
    );
    let grounding = assert_tool_success(&ground_response, json!("ground-structural-snippet"));
    let node_id = grounding["root_symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            grounding["files"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|file| file["symbols"].as_array().into_iter().flatten()),
        )
        .find(|symbol| {
            symbol["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("tiny-stdio-contract-fixture @ "))
        })
        .and_then(|symbol| symbol["id"].as_str())
        .unwrap_or_else(|| panic!("grounding should expose the Cargo package: {grounding:#}"))
        .to_string();

    let snippet_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "snippet-structural-id",
            "method": "tools/call",
            "params": {"name": "snippet", "arguments": {"id": node_id}}
        }),
    );
    let snippet = assert_tool_success(&snippet_response, json!("snippet-structural-id"));
    assert!(
        snippet["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("Cargo.toml"))
            && snippet["snippet"]
                .as_str()
                .is_some_and(|source| source.contains("[package]")),
        "stdio snippet should navigate the exact structural source range: {snippet:#}"
    );

    let query_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "snippet-structural-query",
            "method": "tools/call",
            "params": {
                "name": "snippet",
                "arguments": {"query": "tiny-stdio-contract-fixture"}
            }
        }),
    );
    let error = assert_tool_error(&query_response, json!("snippet-structural-query"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("No symbol matched query")),
        "stdio query snippet should retain typed graph filtering: {error:#}"
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
fn affected_tool_maps_preferred_paths_without_sidecars() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let mut changed_paths = vec!["src/runtime.rs".to_string()];
    changed_paths.extend((0..60).map(|index| format!("src/generated-{index}.rs")));

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-runtime",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "paths": changed_paths,
                    "depth": 2
                }
            }
        }),
    );

    let result = assert_tool_success(&response, json!("affected-runtime"));
    assert_eq!(
        result["counts"]["changed_paths"],
        json!(61),
        "affected should preserve the original changed-path count: {result}"
    );
    assert_eq!(result["changed_paths"].as_array().map(Vec::len), Some(50));
    assert_eq!(result["truncated"], json!(true));
    assert_eq!(result["completeness"]["complete"], json!(false));
    assert_eq!(result["completeness"]["truncated"], json!(true));
    assert!(
        result["completeness"]["truncation_reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.iter().any(|reason| {
                reason.as_str().is_some_and(|reason| {
                    reason.contains("changed_paths response total 61")
                        && reason.contains("stdio limit 50")
                })
            })),
        "transport truncation should degrade nested completeness with field totals: {result}"
    );
    assert_eq!(
        result["change_records"][0]["kind"],
        json!("unknown"),
        "affected should normalize simple paths into change records: {result}"
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
    assert!(result["bounds"]["visited_node_count"].is_number());
    assert!(result["completeness"]["complete"].is_boolean());
    let text = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("affected compact text");
    assert!(text.contains("tool: affected"));
    assert!(text.contains("matched_file_count: 1"));
    assert!(text.contains("count.changed_paths: 61"));
    assert!(text.contains("structuredContent: available"));
    assert!(
        text.len() < 4 * 1024,
        "tool text should stay compact while structuredContent carries the bounded result"
    );
}

#[test]
fn affected_tool_matches_existing_alias_by_native_file_identity() {
    let fixture = indexed_fixture();
    let alias_dir = fixture.workspace.path().join("target");
    fs::create_dir_all(&alias_dir).expect("create excluded alias dir");
    fs::hard_link(
        fixture.workspace.path().join("src/runtime.rs"),
        alias_dir.join("runtime-alias.rs"),
    )
    .expect("create hard-link alias");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-native-alias",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "changed_paths": ["target/runtime-alias.rs"],
                    "depth": 1
                }
            }
        }),
    );

    let result = assert_tool_success(&response, json!("affected-native-alias"));
    assert_eq!(
        result["matched_file_count"],
        json!(1),
        "an excluded alias should match the indexed file by native identity: {result}"
    );
    assert_eq!(result["matched_files"][0]["path"], json!("src/runtime.rs"));
}

#[test]
fn affected_tool_uses_previous_rename_identity_as_bounded_graph_seed() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-rename-previous",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "change_records": [{
                        "path": "src/runtime-renamed.rs",
                        "previous_path": "src/runtime.rs",
                        "kind": "renamed",
                        "status": "R"
                    }],
                    "depth": 1
                }
            }
        }),
    );

    let result = assert_tool_success(&response, json!("affected-rename-previous"));
    assert_eq!(result["matched_file_count"], json!(0));
    assert_eq!(result["matched_files"], json!([]));
    assert_eq!(
        result["unmatched_paths"][0]["classification"],
        json!("rename_unresolved")
    );
    assert_eq!(
        result["unmatched_paths"][0]["change_kind"],
        json!("renamed")
    );
    assert!(
        result["unmatched_paths"][0]["evidence"]
            .as_array()
            .is_some_and(|evidence| evidence.iter().any(|item| {
                item.as_str()
                    .is_some_and(|text| text.contains("previous indexed identity"))
            })),
        "previous identity should stay bounded evidence rather than current-path coverage: {result}"
    );
    assert!(
        result["impacted_symbols"]
            .as_array()
            .is_some_and(|symbols| !symbols.is_empty()),
        "previous rename identity should still seed bounded graph impact: {result}"
    );
}

#[test]
fn affected_tool_emits_no_generic_follow_up_for_complete_fresh_analysis() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-complete",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": ["src/runtime.rs"]}
            }
        }),
    );
    let result = assert_tool_success(&response, json!("affected-complete"));
    assert_eq!(result["completeness"]["complete"], json!(true));
    assert_eq!(result["completeness"]["truncated"], json!(false));
    assert!(result.get("next_commands").is_none());
    assert_eq!(result["follow_ups"], json!([]));
}

#[test]
fn affected_tool_classifies_existing_svg_as_valid_uncovered_without_reindex() {
    let fixture = indexed_fixture();
    fs::write(
        fixture.workspace.path().join("desk-asset.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#,
    )
    .expect("write SVG asset");
    let mut server = spawn_stdio_server(&fixture);
    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-svg",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": ["desk-asset.svg"]}
            }
        }),
    );
    let result = assert_tool_success(&response, json!("affected-svg"));
    assert_eq!(
        result["unmatched_paths"][0]["classification"],
        json!("valid_uncovered")
    );
    assert_eq!(
        result["uncovered_inputs"][0]["classification"],
        json!("valid_uncovered")
    );
    assert!(result.get("next_commands").is_none());
    assert_eq!(
        result["follow_ups"][0]["action"],
        json!("inspect_graph_boundary")
    );
    assert!(result["follow_ups"][0].get("invocation").is_none());
    assert!(
        !result.to_string().contains("--refresh full")
            && !result.to_string().contains("doctor --project"),
        "valid uncovered assets should not recommend reindex or doctor: {result}"
    );
}

#[test]
fn affected_tool_classifies_directory_as_malformed() {
    let fixture = indexed_fixture();
    fs::create_dir(fixture.workspace.path().join("asset-directory"))
        .expect("create directory input");
    let mut server = spawn_stdio_server(&fixture);
    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-directory",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": ["asset-directory"]}
            }
        }),
    );
    let result = assert_tool_success(&response, json!("affected-directory"));
    assert_eq!(
        result["unmatched_paths"][0]["classification"],
        json!("malformed")
    );
    assert!(
        result["unmatched_paths"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("regular file")),
        "directories must not become positive valid_uncovered evidence: {result}"
    );
}

#[test]
fn affected_tool_aborts_outside_root_resolution_before_classification() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-outside-root",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": ["../outside.rs"]}
            }
        }),
    );
    let error = assert_tool_error(&response, json!("affected-outside-root"));
    assert_eq!(error["code"], json!("invalid_argument"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("outside project root")),
        "outside-root resolution should abort instead of producing an input class: {response}"
    );
}

#[test]
fn affected_tool_observes_stale_source_without_refreshing_away_the_evidence() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let fresh_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-before-stale",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": ["src/runtime.rs"]}
            }
        }),
    );
    let fresh = assert_tool_success(&fresh_response, json!("affected-before-stale"));
    assert_eq!(fresh["completeness"]["complete"], json!(true));

    fs::write(
        fixture.workspace.path().join("src/runtime.rs"),
        "pub fn changed_after_publication() -> bool { true }\n",
    )
    .expect("modify indexed source");
    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-stale",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": ["src/runtime.rs"]}
            }
        }),
    );
    let result = assert_tool_success(&response, json!("affected-stale"));
    assert_eq!(result["completeness"]["complete"], json!(false));
    assert!(
        result["uncovered_inputs"]
            .as_array()
            .is_some_and(|inputs| inputs.iter().any(|input| {
                input["path"] == "src/runtime.rs" && input["classification"] == "stale_index"
            })),
        "affected must retain and explain exact stale-source evidence: {result}"
    );
    assert!(result.get("next_commands").is_none());
    assert!(
        result["follow_ups"].as_array().is_some_and(|follow_ups| {
            follow_ups.iter().any(|follow_up| {
                follow_up["action"] == "refresh_stale_index"
                    && follow_up["invocation"]["program"] == "codestory-cli"
                    && follow_up["invocation"]["args"]
                        .as_array()
                        .is_some_and(|args| {
                            args.windows(2)
                                .any(|pair| pair[0] == "--refresh" && pair[1] == "incremental")
                        })
            })
        }),
        "stale source evidence should produce one structured incremental repair: {result}"
    );
    assert!(
        !result.to_string().contains("--refresh full")
            && !result.to_string().contains("doctor --project"),
        "stale source evidence must not produce generic repair commands: {result}"
    );
}

#[test]
fn affected_tool_preserves_rename_and_delete_status_evidence() {
    let fixture = indexed_fixture();
    fs::rename(
        fixture.workspace.path().join("src/runtime.rs"),
        fixture.workspace.path().join("src/renamed_runtime.rs"),
    )
    .expect("rename indexed source");
    fs::remove_file(fixture.workspace.path().join("src/beta.rs")).expect("remove indexed source");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-rename-delete",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "change_records": [
                        {
                            "path": "src/renamed_runtime.rs",
                            "previous_path": "src/runtime.rs",
                            "kind": "renamed",
                            "status": "R100"
                        },
                        {
                            "path": "src/beta.rs",
                            "kind": "deleted",
                            "status": "D"
                        }
                    ],
                    "depth": 2
                }
            }
        }),
    );
    let result = assert_tool_success(&response, json!("affected-rename-delete"));
    assert_eq!(result["matched_file_count"], json!(1));
    assert_eq!(result["change_records"][0]["kind"], json!("renamed"));
    assert_eq!(result["change_records"][0]["status"], json!("R100"));
    assert_eq!(
        result["change_records"][0]["previous_path"],
        json!("src/runtime.rs")
    );
    assert_eq!(result["change_records"][1]["kind"], json!("deleted"));
    assert_eq!(result["change_records"][1]["status"], json!("D"));
    let matched = result["matched_files"]
        .as_array()
        .expect("matched file evidence");
    assert!(matched.iter().all(|file| file["path"] != "src/runtime.rs"));
    assert!(matched.iter().any(|file| {
        file["path"] == "src/beta.rs"
            && file["change_kind"] == "deleted"
            && file["change_status"] == "D"
    }));
    assert!(
        result["uncovered_inputs"]
            .as_array()
            .is_some_and(|inputs| inputs
                .iter()
                .filter(|input| { input["classification"] == "stale_index" })
                .count()
                == 2),
        "rename and delete must retain exact stale publication evidence: {result}"
    );
    assert!(
        result["impacted_symbols"]
            .as_array()
            .is_some_and(|symbols| symbols.iter().any(|symbol| {
                symbol["file_path"] == "src/runtime.rs"
                    && symbol["confidence"] == "bounded"
                    && symbol["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("previous indexed identity"))
            })),
        "rename previous identity should only supply bounded graph evidence: {result}"
    );
}

#[test]
fn affected_tool_does_not_recommend_refresh_for_unrelated_staleness() {
    let fixture = indexed_fixture();
    let excluded = fixture.workspace.path().join("target/excluded.rs");
    fs::create_dir_all(excluded.parent().expect("excluded parent")).expect("create target");
    fs::write(&excluded, "pub fn excluded() {}\n").expect("write excluded source");
    fs::write(
        fixture.workspace.path().join("desk-asset.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
    )
    .expect("write SVG asset");
    fs::write(
        fixture.workspace.path().join("src/alpha.rs"),
        "pub fn unrelated_stale_change() {}\n",
    )
    .expect("modify unrelated indexed source");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-unrelated-stale",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "paths": [
                        "target/excluded.rs",
                        "desk-asset.svg",
                        "missing-asset.svg"
                    ]
                }
            }
        }),
    );
    let result = assert_tool_success(&response, json!("affected-unrelated-stale"));
    let classifications = result["uncovered_inputs"]
        .as_array()
        .expect("uncovered classifications");
    assert!(classifications.iter().any(|input| {
        input["path"] == "target/excluded.rs" && input["classification"] == "valid_uncovered"
    }));
    assert!(classifications.iter().any(|input| {
        input["path"] == "desk-asset.svg" && input["classification"] == "valid_uncovered"
    }));
    assert!(classifications.iter().any(|input| {
        input["path"] == "missing-asset.svg" && input["classification"] == "missing"
    }));
    assert!(
        result["follow_ups"].as_array().is_some_and(|follow_ups| {
            follow_ups
                .iter()
                .all(|follow_up| follow_up["action"] != "refresh_stale_index")
        }),
        "unrelated workspace staleness must not recommend refreshing requested inputs: {result}"
    );
    assert!(
        result["blind_spots"]
            .as_array()
            .is_some_and(|blind_spots| blind_spots.iter().any(|blind_spot| {
                blind_spot
                    .as_str()
                    .is_some_and(|text| text.contains("unrelated stale index state"))
            })),
        "unrelated staleness should remain visible without becoming a repair action: {result}"
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

    let conflict = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-input-conflict",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "paths": ["src/runtime.rs"],
                    "changed_paths": ["src/runtime.rs"]
                }
            }
        }),
    );
    let error = assert_tool_error(&conflict, json!("affected-input-conflict"));
    assert_eq!(error["code"], json!("affected_input_conflict"));

    let empty_property_conflict = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-empty-property-conflict",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {
                    "paths": [],
                    "changed_paths": ["src/runtime.rs"]
                }
            }
        }),
    );
    let error = assert_tool_error(
        &empty_property_conflict,
        json!("affected-empty-property-conflict"),
    );
    assert_eq!(
        error["code"],
        json!("affected_input_conflict"),
        "input exclusivity must use property presence, not non-empty arrays"
    );

    let empty_input = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-empty-input",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": []}
            }
        }),
    );
    let error = assert_tool_error(&empty_input, json!("affected-empty-input"));
    assert_eq!(error["code"], json!("invalid_argument"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("at least one")),
        "empty affected input should fail the adapter minimum: {empty_input}"
    );

    let too_many_paths = vec!["src/runtime.rs"; 201];
    let oversized_input = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-oversized-input",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": too_many_paths}
            }
        }),
    );
    let error = assert_tool_error(&oversized_input, json!("affected-oversized-input"));
    assert_eq!(error["code"], json!("invalid_argument"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("at most 200")),
        "oversized affected input should fail the adapter maximum: {oversized_input}"
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
fn malformed_affected_on_cold_project_does_not_activate_before_legacy_retry() {
    let fixture = unindexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

    let malformed = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-cold-malformed",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": [""]}
            }
        }),
    );
    let error = assert_tool_error(&malformed, json!("affected-cold-malformed"));
    assert_eq!(error["code"], json!("invalid_argument"));

    let cold_status = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-cold-status",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let cold_status = json_resource_content(
        assert_success_envelope(&cold_status, json!("affected-cold-status")),
        "codestory://status",
    );
    assert!(cold_status["index_publication"].is_null());
    assert!(cold_status["current_operation"].is_null());
    let storage_path = cold_status["storage_path"]
        .as_str()
        .expect("cold status storage path");
    assert!(
        !Path::new(storage_path).exists(),
        "malformed affected input must not create storage before activation: {cold_status}"
    );

    let legacy = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "affected-cold-legacy",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"changed_paths": ["src/runtime.rs"]}
            }
        }),
    );
    let result = assert_tool_success(&legacy, json!("affected-cold-legacy"));
    assert_eq!(result["changed_paths"], json!(["src/runtime.rs"]));
    assert_eq!(result["matched_file_count"], json!(1));
    assert!(Path::new(storage_path).exists());
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
            "params": {"uri": "codestory://project", "project": fixture.workspace.path()}
        }),
    );

    let result = assert_success_envelope(&response, json!("project-resource"));
    let content = result["contents"]
        .as_array()
        .expect("resource contents")
        .first()
        .expect("first resource content");
    assert!(
        content["uri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with("codestory://project?project=")),
        "project resource should echo its canonical project-bound URI: {content}"
    );
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
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );

    let result = assert_success_envelope(&response, json!("status-resource"));
    let status = json_resource_content(result, "codestory://status");
    let minified = serde_json::to_vec(&status).expect("serialize minified status");
    assert!(
        minified.len() < 24 * 1024,
        "MCP status must stay below 24 KiB; got {} bytes",
        minified.len()
    );
    let compact_response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "compact-status-tool",
            "method": "tools/call",
            "params": {"name": "status", "arguments": {}}
        }),
    );
    let compact = assert_tool_success(&compact_response, json!("compact-status-tool"));
    assert_eq!(compact["state"], json!("working_locally"), "{compact}");
    assert_eq!(
        compact["capabilities"]["local_navigation"],
        json!("ready"),
        "{compact}"
    );
    assert!(
        compact["diagnostics_uri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with("codestory://status?project=")),
        "compact status should link to its project-bound diagnostic resource: {compact}"
    );
    for diagnostic in ["allowed_surfaces", "retrieval_diagnostics"] {
        assert!(compact.get(diagnostic).is_none(), "{compact}");
    }
    let compact_text = assert_tool_text_content(
        assert_success_envelope(&compact_response, json!("compact-status-tool")),
        &compact_response,
    );
    assert!(compact_text.contains("tool: status"));
    assert!(compact_text.contains("state: working_locally"));
    assert!(compact_text.contains("capability.local_navigation: ready"));
    assert!(compact_text.contains("next_action:"));
    let local_summary = "Local repository navigation is ready.";
    assert_eq!(
        status.to_string().matches(local_summary).count(),
        1,
        "canonical readiness guidance must not be cloned per surface: {status}"
    );
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
        status["retrieval_contract_version"].is_number(),
        "status should expose the retrieval contract version: {status}"
    );
    assert!(
        status.get("retrieval_diagnostics").is_none(),
        "normal status must keep engine diagnostics private: {status}"
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
        status["runtime_truth"]["retrieval_status_ref"],
        json!("readiness_lanes.agent_packet_search"),
        "runtime truth should reference the canonical agent readiness lane: {status}"
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
        "an explicit-CPU fixture without a compatible semantic publication must not report retrieval as full: {status}"
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
    assert!(status.get("retrieval_diagnostics").is_none());
    assert!(status.get("legacy_semantic_diagnostics").is_none());
    assert!(
        !contains_key_recursive(&status, &["sidecar", "full_repair"]),
        "normal status must not expose engine lifecycle or repair fields: {status}"
    );
    for maintainer_only_field in [
        "embedding_backend",
        "embedding_device",
        "adapter_identity",
        "ggml_build_identity",
        "semantic_doc_count",
        "fallback_reason",
    ] {
        assert!(
            !contains_key_recursive(&status, &[maintainer_only_field]),
            "normal status must not expose maintainer-only engine details: {status}"
        );
    }
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
    assert!(
        local_default["retrieval_mode"].is_string(),
        "local/default lane should report retrieval mode: {status}"
    );
    assert_eq!(local_default["status"], json!("unavailable"));
    assert_eq!(local_default.as_object().map(serde_json::Map::len), Some(2));
    let agent_lane = readiness_lanes
        .get("agent_packet_search")
        .unwrap_or_else(|| panic!("status should include agent_packet_search lane: {status}"));
    assert_eq!(
        agent_lane["status"],
        json!("unavailable"),
        "agent lane should report broad retrieval capability: {status}"
    );
    assert!(agent_lane["retrieval_mode"].is_string());
    assert_eq!(agent_lane.as_object().map(serde_json::Map::len), Some(2));
    assert_eq!(
        status["runtime_truth"]["readiness_refs"]["local_graph"],
        json!("readiness[goal=local_navigation]"),
        "runtime truth should reference the local graph verdict: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["readiness_refs"]["local_refresh"],
        json!("local_refresh"),
        "runtime truth should reference local refresh state: {status}"
    );
    assert_eq!(
        status["runtime_truth"]["readiness_refs"]["agent_packet_search"],
        json!("readiness_lanes.agent_packet_search"),
        "runtime truth should reference agent packet/search readiness: {status}"
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
        assert_allowed_surface(
            &status,
            surface,
            false,
            "agent_packet_search",
            "unavailable",
        );
        assert!(
            status
                .pointer(&format!("/allowed_surfaces/{surface}/repair_reason"))
                .is_none(),
            "normal status should hide retrieval lifecycle reasons: {status}"
        );
    }
    assert!(
        readiness.iter().any(|verdict| {
            verdict["goal"] == "agent_packet_search"
                && verdict["status"] == "unavailable"
                && verdict.get("minimum_next").is_none()
                && verdict.get("full_repair").is_none()
        }),
        "status should expose capability state without manual retrieval instructions: {status}"
    );
    assert!(
        !next_call_text.contains("\"tool\":\"packet\"")
            && !next_call_text.contains("\"tool\":\"search\""),
        "status should recommend repair, not packet/search calls, when mode is not full: {status}"
    );
    assert_eq!(
        status["recommended_next_calls"],
        json!([]),
        "diagnostics should not send agents through a repair loop; direct tools own preparation: {status}"
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
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
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
    assert_eq!(status["readiness"][0]["status"], json!("unavailable"));
    assert_activation_surface(&status, "ground");
    assert_allowed_surface(
        &status,
        "packet",
        false,
        "agent_packet_search",
        "unavailable",
    );
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
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
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
    assert_allowed_surface(
        &status,
        "packet",
        false,
        "agent_packet_search",
        "unavailable",
    );
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
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
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
    assert_allowed_surface(
        &status,
        "packet",
        false,
        "agent_packet_search",
        "unavailable",
    );
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
                "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
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
        assert_allowed_surface(
            &status,
            "packet",
            false,
            "agent_packet_search",
            "unavailable",
        );
    }
}

#[test]
fn update_available_is_advisory_and_preserves_compatible_surfaces() {
    let mut fixture = indexed_fixture();
    let plugin_data = fixture.cache_dir.path().join("plugin-data-update");
    let installed = write_managed_cli_fixture(&plugin_data, "999.0.0");
    fixture.latest_release_version = Some("999.0.0".to_string());
    fixture.plugin_data_dir = Some(plugin_data);
    fixture.plugin_cli_source = Some("managed".to_string());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-update-advisory",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-update-advisory"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["runtime_update"]["state"], json!("available"));
    assert_eq!(status["runtime_update"]["blocking"], json!(false));
    assert_eq!(status["runtime_update"]["readiness_impact"], json!("none"));
    assert_eq!(
        status["runtime_update"]["active_version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(status["runtime_update"]["latest_version"], "999.0.0");
    assert_eq!(status["runtime_update"]["restart_recommended"], json!(true));
    assert_eq!(
        status["runtime_update"]["recommended_action"],
        json!("restart_host")
    );
    assert_eq!(
        status["runtime_update"]["newer_installed_version"],
        json!("999.0.0")
    );
    assert!(
        status["runtime_update"]["newer_installed_path"]
            .as_str()
            .is_some_and(
                |path| path.ends_with(installed.file_name().unwrap().to_string_lossy().as_ref())
            ),
        "status should expose the checksum-valid managed candidate: {status}"
    );
    assert_eq!(status["readiness"][0]["status"], json!("ready"));
    assert!(status["readiness"][0].get("setup").is_none());
    assert_allowed_surface(&status, "ground", true, "local_navigation", "ready");
    assert_allowed_surface(&status, "files", true, "local_navigation", "ready");
    assert_allowed_surface(
        &status,
        "packet",
        false,
        "agent_packet_search",
        "unavailable",
    );
    let next_call_text = status["recommended_next_calls"].to_string();
    assert!(
        !next_call_text.contains("install-codestory.ps1") && !next_call_text.contains("999.0.0"),
        "release availability must not replace readiness repair guidance: {status}"
    );
    let ground = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-with-update-available",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {}}
        }),
    );
    assert_tool_success(&ground, json!("ground-with-update-available"));
}

#[test]
fn offline_release_metadata_is_non_blocking_and_unknown() {
    let mut fixture = indexed_fixture();
    fixture.plugin_data_dir = Some(fixture.cache_dir.path().join("plugin-data-offline"));
    fixture.latest_release_version = None;
    fixture.disable_release_probe = true;
    fixture.disable_installed_cli_probe = true;
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-offline-release-metadata",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-offline-release-metadata"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["runtime_update"]["state"], json!("unknown"));
    assert_eq!(
        status["runtime_update"]["metadata_source"],
        json!("disabled")
    );
    assert_eq!(status["runtime_update"]["blocking"], json!(false));
    assert_eq!(
        status["runtime_update"]["metadata_refresh_scheduled"],
        json!(false)
    );
    assert_allowed_surface(&status, "ground", true, "local_navigation", "ready");
}

#[test]
fn local_dev_override_does_not_recommend_restart_for_managed_history() {
    let mut fixture = indexed_fixture();
    let plugin_data = fixture.cache_dir.path().join("plugin-data-local-override");
    write_managed_cli_fixture(&plugin_data, "999.0.0");
    fixture.plugin_data_dir = Some(plugin_data);
    fixture.plugin_cli_source = Some("local_dev_override".to_string());
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-local-dev-override",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let result = assert_success_envelope(&response, json!("status-local-dev-override"));
    let status = json_resource_content(result, "codestory://status");

    assert_eq!(status["runtime_update"]["state"], json!("current"));
    assert_eq!(
        status["runtime_update"]["restart_recommended"],
        json!(false)
    );
    assert!(status["runtime_update"]["newer_installed_path"].is_null());
    assert_allowed_surface(&status, "ground", true, "local_navigation", "ready");
}

#[test]
fn status_observes_staleness_and_ground_activates_bounded_local_refresh() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let warmup = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-freshness-warmup",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
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

    let stale = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "status-observes-stale",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let stale = json_resource_content(
        assert_success_envelope(&stale, json!("status-observes-stale")),
        "codestory://status",
    );
    assert_eq!(
        find_index_freshness(&stale).and_then(|freshness| freshness.get("status")),
        Some(&json!("stale")),
        "status must observe source drift without repairing it: {stale}"
    );
    assert!(
        !fixture.cache_dir.path().join("local-refresh.lock").exists(),
        "status must not acquire refresh ownership"
    );
    for surface in ["ground", "files", "affected"] {
        assert_activation_surface(&stale, surface);
    }
    assert_allowed_surface(&stale, "symbol", false, "local_navigation", "unavailable");
    assert_ground_activation_call(&stale);

    let activation = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "ground-activates-refresh",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {"budget": "strict"}}
        }),
    );
    assert_tool_success(&activation, json!("ground-activates-refresh"));

    let refresh_deadline = Instant::now() + Duration::from_secs(15);
    let mut refresh_attempt = 0;
    let refreshed_status = loop {
        let id = format!("status-freshness-after-mutation-{refresh_attempt}");
        let refreshed = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": id.clone(),
                "method": "resources/read",
                "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
            }),
        );
        let refreshed_result = assert_success_envelope(&refreshed, json!(id));
        let status = json_resource_content(refreshed_result, "codestory://status");
        if find_index_freshness(&status)
            .and_then(|freshness| freshness.get("status"))
            .and_then(Value::as_str)
            == Some("fresh")
        {
            break status;
        }
        assert!(
            Instant::now() < refresh_deadline,
            "background local refresh did not complete within 15 seconds: {status}"
        );
        refresh_attempt += 1;
        thread::sleep(Duration::from_millis(50));
    };
    assert_fresh_freshness_counts(&refreshed_status, "codestory://status after mutation");
    assert_eq!(
        refreshed_status["local_refresh"]["state"],
        json!("refreshed"),
        "ground activation must invalidate the cached warm freshness result: {refreshed_status}"
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
        refreshed_status["readiness_lanes"]["agent_packet_search"]["status"],
        json!("unavailable"),
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
    // Twenty samples are the minimum where this nearest-rank p95 is not just
    // the single maximum scheduler outlier under the full parallel suite.
    for index in 0..20 {
        let started = Instant::now();
        let response = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": format!("status-freshness-{index}"),
                "method": "resources/read",
                "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
            }),
        );
        elapsed.push(started.elapsed());
        let result = assert_success_envelope(&response, json!(format!("status-freshness-{index}")));
        last_status = json_resource_content(result, "codestory://status");
    }

    assert_fresh_freshness_counts(&last_status, "cached codestory://status after refresh");
    assert_eq!(
        last_status["local_refresh"]["state"],
        json!("refreshed"),
        "status should stay fresh without stale cache masking after the bounded refresh: {last_status}"
    );
    assert!(
        last_status["index_publication"]["generation"]
            .as_u64()
            .is_some(),
        "fresh status should identify the complete publication: {last_status}"
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

    let mut index_command = test_support::cli_command();
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
    allow_explicit_cpu_embeddings(&mut index_command);
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
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let result = assert_success_envelope(&refreshed, json!("status-freshness-after-reindex"));
    let refreshed_status = json_resource_content(result, "codestory://status");
    assert_fresh_freshness_counts(&refreshed_status, "codestory://status after reindex");
}

#[test]
fn ground_tool_serves_complete_publication_when_refresh_budget_expires() {
    let mut fixture = indexed_fixture();
    fixture.local_refresh_timeout_ms = Some(0);
    let mut server = spawn_stdio_server(&fixture);
    initialize_stdio_server(&mut server, "init-ground-refresh-budget-expired");

    thread::sleep(Duration::from_millis(25));
    fs::write(
        fixture.workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("budget-expired:{project_name}")
}
"#,
    )
    .expect("modify indexed file after indexing");

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

    let result = assert_success_envelope(&response, json!("ground-refresh-budget-expired"));
    let ground = assert_tool_success(&response, json!("ground-refresh-budget-expired"));
    assert_eq!(
        ground.pointer("/stats/file_count").and_then(Value::as_u64),
        Some(5),
        "ground should serve the last complete publication: {response}"
    );
    let served_from = result
        .pointer("/_meta/codestory_publication/served_from")
        .and_then(Value::as_str);
    assert!(
        matches!(
            served_from,
            Some("last_complete_publication" | "complete_publication")
        ),
        "ground should identify the exact complete publication source: {response}"
    );
    assert!(
        result
            .pointer("/_meta/codestory_publication/publication/generation")
            .and_then(Value::as_u64)
            .is_some(),
        "served response should identify its durable publication: {response}"
    );
    assert!(
        result
            .pointer("/_meta/codestory_publication/core_publication/generation")
            .and_then(Value::as_u64)
            .is_some()
            && result
                .pointer("/_meta/codestory_publication/operation/operation_id")
                .and_then(Value::as_str)
                .is_some()
            && result
                .pointer("/_meta/codestory_publication/operation/attempt")
                .and_then(Value::as_u64)
                .is_some(),
        "served response should preserve legacy publication metadata and add operation identity: {response}"
    );
    if served_from == Some("last_complete_publication") {
        assert_eq!(
            result
                .pointer("/_meta/codestory_publication/refresh/state")
                .and_then(Value::as_str),
            Some("refreshing")
        );
    }
}

#[test]
fn independent_clients_serve_one_complete_generation_while_refresh_is_owned() {
    let fixture = indexed_fixture();
    thread::sleep(Duration::from_millis(25));
    fs::write(
        fixture.workspace.path().join("src").join("runtime.rs"),
        "pub fn normalize_project(project_name: &str) -> String { format!(\"owned:{project_name}\") }\n",
    )
    .expect("make the published index stale");

    let mut status_client = spawn_stdio_server(&fixture);
    initialize_stdio_server(&mut status_client, "init-concurrent-status");
    let mut ground_client = spawn_stdio_server(&fixture);
    initialize_stdio_server(&mut ground_client, "init-concurrent-ground");
    let pid = write_live_local_refresh(&fixture);

    let status_response = send_json(
        &mut status_client,
        json!({
            "jsonrpc": "2.0",
            "id": "concurrent-status",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let status = json_resource_content(
        assert_success_envelope(&status_response, json!("concurrent-status")),
        "codestory://status",
    );
    assert_eq!(status["local_refresh"]["state"], json!("refreshing"));
    assert_eq!(status["local_refresh"]["pid"], json!(pid));
    assert_eq!(status["local_refresh"]["phase"], json!("incremental_index"));
    assert_eq!(
        status["local_refresh"]["blocks_local_surfaces"],
        json!(false)
    );
    assert_eq!(status["allowed_surfaces"]["ground"]["allowed"], json!(true));
    let generation = status["local_refresh"]["serving_publication"]["generation"]
        .as_u64()
        .expect("status serving generation");

    let ground_response = send_json(
        &mut ground_client,
        json!({
            "jsonrpc": "2.0",
            "id": "concurrent-ground",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {"budget": "strict"}}
        }),
    );
    let ground = assert_tool_success(&ground_response, json!("concurrent-ground"));
    assert_eq!(ground["stats"]["file_count"], json!(5));
    let ground_result = assert_success_envelope(&ground_response, json!("concurrent-ground"));
    let served_generation =
        ground_result["_meta"]["codestory_publication"]["publication"]["generation"]
            .as_u64()
            .expect("ground serving generation");
    assert!(served_generation >= generation);

    let symbol_response = send_json(
        &mut ground_client,
        json!({
            "jsonrpc": "2.0",
            "id": "concurrent-symbol",
            "method": "tools/call",
            "params": {"name": "symbol", "arguments": {"query": "AppController"}}
        }),
    );
    let symbol = assert_tool_success(&symbol_response, json!("concurrent-symbol"));
    assert_eq!(symbol["node"]["display_name"], json!("AppController"));
    let symbol_result = assert_success_envelope(&symbol_response, json!("concurrent-symbol"));
    assert_eq!(
        symbol_result["_meta"]["codestory_publication"]["publication"]["generation"],
        json!(served_generation)
    );
    assert!(
        symbol_result["_meta"]["codestory_publication"]
            .as_object()
            .is_some_and(|metadata| metadata.contains_key("retrieval_publication")),
        "query-resolved graph responses must retain an explicit retrieval publication identity slot: {symbol_response}"
    );

    let root_symbols_response = send_json(
        &mut ground_client,
        json!({
            "jsonrpc": "2.0",
            "id": "concurrent-root-symbols",
            "method": "resources/read",
            "params": {
                "uri": "codestory://symbols/root",
                "project": fixture.workspace.path()
            }
        }),
    );
    let root_symbols = json_resource_content(
        assert_success_envelope(&root_symbols_response, json!("concurrent-root-symbols")),
        "codestory://symbols/root",
    );
    assert!(
        root_symbols
            .as_array()
            .is_some_and(|symbols| symbols.iter().any(|symbol| {
                symbol["display_name"] == json!("AppController")
                    || symbol["label"] == json!("AppController")
            })),
        "root-symbol resource should stay readable during another client's refresh: {root_symbols}"
    );
}

#[test]
fn two_stdio_processes_observe_only_complete_generations_during_real_refresh() {
    let mut fixture = indexed_fixture();
    fixture.local_refresh_timeout_ms = Some(0);
    let mut warmup_client = spawn_stdio_server(&fixture);
    let warmup_status = send_json(
        &mut warmup_client,
        json!({
            "jsonrpc": "2.0",
            "id": "warmup-generation",
            "method": "resources/read",
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let old_generation = json_resource_content(
        assert_success_envelope(&warmup_status, json!("warmup-generation")),
        "codestory://status",
    )["index_publication"]["generation"]
        .as_u64()
        .expect("old complete generation");
    drop(warmup_client);
    thread::sleep(Duration::from_millis(25));
    for index in 0..96 {
        fs::write(
            fixture
                .workspace
                .path()
                .join("src")
                .join(format!("concurrent_{index}.rs")),
            format!("pub fn concurrent_{index}() -> usize {{ {index} }}\n"),
        )
        .expect("add source file for real refresh");
    }

    let mut reader_client = spawn_stdio_server(&fixture);
    let mut writer_client = spawn_stdio_server(&fixture);
    let writer = thread::spawn(move || {
        let response = send_json(
            &mut writer_client,
            json!({
                "jsonrpc": "2.0",
                "id": "writer-start-refresh",
                "method": "tools/call",
                "params": {"name": "ground", "arguments": {"budget": "strict"}}
            }),
        );
        (writer_client, response)
    });

    let lock_path = fixture.cache_dir.path().join("local-refresh.lock");
    let lock_deadline = Instant::now() + Duration::from_secs(10);
    while !lock_path.exists() {
        if writer.is_finished() {
            break;
        }
        assert!(
            Instant::now() < lock_deadline,
            "writer did not acquire the local refresh lock"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let concurrent_ground = send_json(
        &mut reader_client,
        json!({
            "jsonrpc": "2.0",
            "id": "reader-ground-during-lock",
            "method": "resources/read",
            "params": {
                "uri": "codestory://grounding",
                "project": fixture.workspace.path()
            }
        }),
    );
    let concurrent_ground = json_resource_content(
        assert_success_envelope(&concurrent_ground, json!("reader-ground-during-lock")),
        "codestory://grounding",
    );
    assert!(
        concurrent_ground["stats"]["file_count"]
            .as_u64()
            .is_some_and(|count| count == 5 || count == 101),
        "concurrent resource read observed neither complete file set: {concurrent_ground}"
    );

    // Workspace-wide default-concurrency runs can heavily contend with the
    // real indexer on smaller macOS runners. Keep the assertion bounded while
    // allowing the background publication worker to finish under that load.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let status_response = send_json(
            &mut reader_client,
            json!({
                "jsonrpc": "2.0",
                "id": "reader-status",
                "method": "resources/read",
                "params": {
                    "uri": "codestory://status",
                    "project": fixture.workspace.path()
                }
            }),
        );
        let status = json_resource_content(
            assert_success_envelope(&status_response, json!("reader-status")),
            "codestory://status",
        );
        let generation = status["index_publication"]["generation"]
            .as_u64()
            .expect("reader complete generation");
        assert!(
            generation >= old_generation,
            "reader observed publication generation rollback: {status}"
        );
        let expected_status_file_count = if generation == old_generation { 5 } else { 101 };
        assert_eq!(
            status["index_freshness"]["indexed_file_count"],
            json!(expected_status_file_count),
            "status mixed publication metadata and summary contents: {status}"
        );
        let ground_response = send_json(
            &mut reader_client,
            json!({
                "jsonrpc": "2.0",
                "id": "reader-ground",
                "method": "tools/call",
                "params": {"name": "ground", "arguments": {"budget": "strict"}}
            }),
        );
        let ground = assert_tool_success(&ground_response, json!("reader-ground"));
        let ground_result = assert_success_envelope(&ground_response, json!("reader-ground"));
        let ground_generation =
            ground_result["_meta"]["codestory_publication"]["publication"]["generation"]
                .as_u64()
                .expect("ground response publication generation");
        assert!(
            ground_generation >= old_generation,
            "ground response identified publication generation rollback: {ground_result}"
        );
        let expected_file_count = if ground_generation == old_generation {
            5
        } else {
            101
        };
        assert!(
            ground["stats"]["file_count"]
                .as_u64()
                .is_some_and(|count| count == expected_file_count),
            "reader ground mixed publication metadata and file contents: {ground_result}"
        );

        if generation > old_generation && status["local_refresh"]["state"] != json!("refreshing") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "real refresh did not publish a new complete generation: {status}"
        );
        thread::sleep(Duration::from_millis(25));
    }

    let (_writer_client, writer_status) = writer.join().expect("join writer status client");
    assert_tool_success(&writer_status, json!("writer-start-refresh"));
}

#[test]
fn tools_call_local_graph_refreshes_long_lived_index_after_source_mutation() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);
    let tools = assert_success_envelope(
        &send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": "tool-refresh-catalog",
                "method": "tools/list"
            }),
        ),
        json!("tool-refresh-catalog"),
    )
    .clone();
    let snippet_output_schema = tool_output_schema(&tools, "snippet").clone();

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
            "params": {"uri": "codestory://status", "project": fixture.workspace.path()}
        }),
    );
    let status_result = assert_success_envelope(&status_response, json!("tool-refresh-status"));
    let status = json_resource_content(status_result, "codestory://status");
    assert_fresh_freshness_counts(&status, "codestory://status after local graph tool refresh");
    assert_eq!(
        status["local_refresh"]["state"],
        json!("refreshed"),
        "tool dispatch should have refreshed the long-lived server before status was reread: {status}"
    );
    assert!(
        status["index_publication"]["generation"].as_u64().is_some(),
        "refreshed status should identify the complete publication: {status}"
    );
    assert_eq!(
        status["readiness_lanes"]["agent_packet_search"]["status"],
        json!("unavailable"),
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
        let arguments = if tool == "snippet" {
            json!({"id": node_id, "function_body": true, "lines": 0})
        } else {
            json!({"id": node_id, "depth": 1, "max_nodes": 20})
        };
        let response = send_json(
            &mut server,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": tool,
                    "arguments": arguments
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
        if tool == "snippet" {
            assert_eq!(result["scope"], json!("function_body"), "{result}");
            assert_eq!(result["requested_context"], json!(0), "{result}");
            assert!(
                result["range_source"].as_str().is_some(),
                "function-body snippets should identify their selected range source: {result}"
            );
            let declared_properties = snippet_output_schema["properties"]
                .as_object()
                .expect("snippet outputSchema properties");
            for field in result
                .as_object()
                .expect("snippet structuredContent object")
                .keys()
            {
                assert!(
                    declared_properties.contains_key(field),
                    "function-body structuredContent field {field} is absent from the strict outputSchema: schema={snippet_output_schema}, result={result}"
                );
            }
        }
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
    assert!(
        matches!(
            search_error.pointer("/code").and_then(Value::as_str),
            Some("codestory_preparing" | "codestory_unavailable")
        ),
        "broad search should use the normal readiness response after local graph refresh: {search_response}"
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
                calls.iter().any(|call| {
                    call["tool"] == json!("ground") && call.pointer("/arguments/project").is_some()
                })
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
        guide_text.contains("preparing")
            && guide_text.contains("retry")
            && guide_text.contains("same tool"),
        "agent guide should tell agents to retry the intended tool while CodeStory prepares: {guide}"
    );
    assert!(
        guide_text.contains("repo-text hits as navigation clues"),
        "agent guide should treat repo-text hits as navigation clues: {guide}"
    );
    assert!(
        guide_text.contains("search hits as discovery clues")
            && guide_text.contains("graph or source evidence"),
        "agent guide should distinguish discovery clues from evidence: {guide}"
    );
    assert!(
        guide_text.contains("unsafe to claim") && guide_text.contains("follow_up_commands"),
        "agent guide should name unsafe-to-claim and follow-up states: {guide}"
    );
    assert!(
        guide_text.contains("direct_source_reads")
            && guide_text.contains("unavailable")
            && guide_text.contains("exact source inspection"),
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
fn cold_ground_uses_local_capability_while_search_prepares_embedding_runtime() {
    let fixture = unindexed_fixture();
    write_live_local_refresh(&fixture);
    let mut server = spawn_stdio_server(&fixture);

    let ground = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "cold-ground-local",
            "method": "tools/call",
            "params": {"name": "ground", "arguments": {"budget": "strict"}}
        }),
    );
    assert_tool_success(&ground, json!("cold-ground-local"));

    let search = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "cold-search-unavailable",
            "method": "tools/call",
            "params": {"name": "search", "arguments": {"query": "AppController"}}
        }),
    );
    let error = assert_tool_error(&search, json!("cold-search-unavailable"));
    assert_eq!(error["tool"], json!("search"));
    assert!(
        error["diagnostics_uri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with("codestory://status?project=")),
        "tool errors should return a project-bound diagnostics resource: {error}"
    );
    assert_eq!(
        error["operation"]["capabilities"]["local_navigation"],
        json!("ready")
    );
    if error["cause_code"] == "native_model_not_embedded" {
        assert_eq!(error["code"], json!("codestory_unavailable"));
        assert_eq!(error["state"], json!("unavailable"));
        assert_eq!(error["retry_tool"], Value::Null);
        assert_eq!(
            error["operation"]["capabilities"]["broad_search"],
            json!("unavailable")
        );
    } else {
        assert_eq!(error["code"], json!("codestory_preparing"));
        assert_eq!(error["state"], json!("preparing"));
        assert_eq!(error["retry_tool"], json!("search"));
        assert_eq!(
            error["operation"]["capabilities"]["broad_search"],
            json!("retryable")
        );
    }
    assert_eq!(
        error["operation"]["embedding_retry"]["retry_class"],
        json!("after_server_change")
    );

    let fixture = indexed_fixture();
    write_live_local_refresh(&fixture);
    let mut server = spawn_stdio_server(&fixture);
    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "migration-search-preparing",
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "AppController"}
            }
        }),
    );
    let error = assert_tool_error(&response, json!("migration-search-preparing"));
    if error["cause_code"] == "native_model_not_embedded" {
        assert_eq!(error["code"], json!("codestory_unavailable"));
        assert_eq!(error["state"], json!("unavailable"));
        assert_eq!(error["retry_tool"], Value::Null);
    } else {
        assert_eq!(error["code"], json!("codestory_preparing"));
        assert_eq!(error["state"], json!("preparing"));
        assert_eq!(error["retry_tool"], json!("search"));
    }
}

#[test]
fn packet_repairs_a_missing_search_generation_before_rendering_same_tool_retry() {
    let fixture = indexed_fixture();
    let search_generations = fixture
        .cache_dir
        .path()
        .join("codestory.search-generations");
    assert!(
        search_generations.is_dir(),
        "indexed fixture needs search state"
    );
    fs::remove_dir_all(&search_generations).expect("remove migrated search generations");
    let mut server = spawn_stdio_server(&fixture);
    let packet_request = |id: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "packet",
                "arguments": {"question": "How does AppController open a project?"}
            }
        })
    };

    let first = send_json(&mut server, packet_request("packet-search-repair-first"));
    if first.pointer("/result/isError") != Some(&json!(true)) {
        assert_tool_success(&first, json!("packet-search-repair-first"));
        return;
    }
    let first_error = assert_tool_error(&first, json!("packet-search-repair-first"));
    if first_error["cause_code"] == "native_model_not_embedded" {
        assert_search_repaired_before_terminal_model_absence(
            &mut server,
            first_error,
            &search_generations,
            "packet-search-repair-first",
        );
        return;
    }
    assert_eq!(first_error["code"], json!("codestory_preparing"));
    assert_eq!(first_error["retry_tool"], json!("packet"));
    assert!(first_error["retry_after_ms"].as_u64().is_some());
    assert!(first_error["cause_code"].as_str().is_some());
    let operation_id = first_error["operation"]["operation_id"]
        .as_str()
        .expect("stable activation operation id")
        .to_string();
    let mut last_error = first_error.clone();

    let mut retry_after_ms = first_error["retry_after_ms"].as_u64().unwrap_or(250);
    for attempt in 1..=8 {
        thread::sleep(Duration::from_millis(retry_after_ms.min(1_000)));
        let id = format!("packet-search-repair-retry-{attempt}");
        let response = send_json(&mut server, packet_request(&id));
        if response.pointer("/result/isError") != Some(&json!(true)) {
            assert_tool_success(&response, json!(id));
            assert!(
                search_generations.is_dir(),
                "activation must rebuild search state before packet succeeds"
            );
            return;
        }
        let error = assert_tool_error(&response, json!(id));
        if error["cause_code"] == "native_model_not_embedded" {
            assert_search_repaired_before_terminal_model_absence(
                &mut server,
                error,
                &search_generations,
                &id,
            );
            return;
        }
        assert_eq!(error["code"], json!("codestory_preparing"));
        assert_eq!(error["retry_tool"], json!("packet"));
        assert_eq!(
            error["operation"]["operation_id"],
            json!(operation_id),
            "same-project retry must retain the repair operation id"
        );
        retry_after_ms = error["retry_after_ms"].as_u64().unwrap_or(250);
        last_error = error.clone();
    }
    panic!("packet did not converge after the bounded same-tool retry sequence: {last_error}");
}

#[test]
fn packet_preserves_typed_source_failure_diagnostics() {
    let fixture = unindexed_fixture();
    fs::write(
        fixture.workspace.path().join("codestory_workspace.json"),
        r#"{"members":["src","missing"]}"#,
    )
    .expect("write incomplete discovery fixture");
    let mut server = spawn_stdio_server(&fixture);

    let response = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "packet-typed-source-failure",
            "method": "tools/call",
            "params": {
                "name": "packet",
                "arguments": {"question": "How does AppController open a project?"}
            }
        }),
    );
    let error = assert_tool_error(&response, json!("packet-typed-source-failure"));

    assert_eq!(error["code"], json!("codestory_unavailable"));
    assert_eq!(error["cause_code"], json!("source_discovery_incomplete"));
    assert_eq!(error["retry_tool"], Value::Null);
    assert_eq!(
        error.pointer("/details/coverage_gaps/0/reason"),
        Some(&json!("discovery_incomplete"))
    );
    assert_eq!(
        error.pointer("/details/coverage_gaps/0/retryable"),
        Some(&json!(true))
    );
}

#[test]
fn failed_replacement_retries_keep_identity_and_offer_retained_local_analysis() {
    let fixture = indexed_fixture();
    fs::write(
        fixture.workspace.path().join("codestory_workspace.json"),
        r#"{"members":["src","missing"]}"#,
    )
    .expect("write incomplete replacement fixture");
    let mut server = spawn_stdio_server(&fixture);
    let packet_request = |id: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "packet",
                "arguments": {"question": "How does AppController open a project?"}
            }
        })
    };

    let first = send_json(&mut server, packet_request("retained-packet-first"));
    let first_error = assert_tool_error(&first, json!("retained-packet-first"));
    assert_eq!(first_error["code"], json!("codestory_unavailable"));
    assert_eq!(
        first_error["cause_code"],
        json!("source_discovery_incomplete")
    );
    assert_eq!(first_error["retry_tool"], Value::Null);
    assert!(first_error["retry_after_ms"].is_null());
    assert_eq!(
        first_error["operation"]["capabilities"]["local_navigation"],
        json!("retained")
    );
    assert_eq!(
        first_error["operation"]["capabilities"]["broad_search"],
        json!("unavailable")
    );
    assert!(
        first_error["operation"]["retained_core_publication"]["generation_id"]
            .as_str()
            .is_some()
    );
    assert!(
        first_error["operation"]["revision"].as_u64().is_some()
            && first_error["operation"]["progress"].as_u64().is_some()
    );
    assert_eq!(
        first_error["next_action"],
        json!("continue_with_retained_local_navigation")
    );
    assert!(
        first_error["recommended_next_calls"]
            .as_array()
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call["method"] == "tools/call"
                        && call["tool"] == "affected"
                        && call["arguments"]["paths"] == json!(["<changed-project-path>"])
                }) && calls.iter().any(|call| {
                    call["method"] == "resources/read"
                        && call["uri"]
                            .as_str()
                            .is_some_and(|uri| uri.starts_with("codestory://status?project="))
                })
            }),
        "terminal MCP response must provide useful native follow-ups: {first_error}"
    );

    let first_operation = first_error["operation"].clone();
    let second = send_json(&mut server, packet_request("retained-packet-second"));
    let second_error = assert_tool_error(&second, json!("retained-packet-second"));
    assert_eq!(
        second_error["operation"]["operation_id"],
        first_operation["operation_id"]
    );
    assert!(
        second_error["operation"]["revision"]
            .as_u64()
            .zip(first_operation["revision"].as_u64())
            .is_some_and(|(second, first)| second > first)
    );
    assert!(
        second_error["operation"]["attempt"]
            .as_u64()
            .zip(first_operation["attempt"].as_u64())
            .is_some_and(|(second, first)| second == first + 1)
    );
    assert!(
        second_error["operation"]["progress"]
            .as_u64()
            .zip(first_operation["progress"].as_u64())
            .is_some_and(|(second, first)| second >= first)
    );
    assert_eq!(second_error["operation"]["stage"], first_operation["stage"]);

    let affected = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "retained-affected",
            "method": "tools/call",
            "params": {
                "name": "affected",
                "arguments": {"paths": ["src/runtime.rs"]}
            }
        }),
    );
    assert_tool_success(&affected, json!("retained-affected"));
    let affected_result = assert_success_envelope(&affected, json!("retained-affected"));
    assert!(
        affected_result["_meta"]["codestory_publication"]["core_publication"]["generation"]
            .as_u64()
            .is_some(),
        "affected must pin the retained core publication: {affected_result}"
    );

    let ground = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "retained-ground",
            "method": "tools/call",
            "params": {
                "name": "ground",
                "arguments": {"budget": "strict"}
            }
        }),
    );
    let ground = assert_tool_success(&ground, json!("retained-ground"));
    assert!(
        ground["stats"]["node_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "ground must remain usable from the retained core: {ground}"
    );

    let status = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "retained-status",
            "method": "tools/call",
            "params": {"name": "status", "arguments": {}}
        }),
    );
    let status = assert_tool_success(&status, json!("retained-status"));
    assert_eq!(status["state"], json!("working_locally"));
    assert_eq!(
        status["capabilities"]["local_navigation"],
        json!("retained")
    );
    assert_eq!(status["capabilities"]["broad_search"], json!("unavailable"));
    assert_eq!(
        status["current_operation"]["operation_id"],
        first_operation["operation_id"]
    );

    let diagnostics_uri = first_error["diagnostics_uri"]
        .as_str()
        .expect("project-bound status URI");
    let resource = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "retained-status-resource",
            "method": "resources/read",
            "params": {"uri": diagnostics_uri}
        }),
    );
    let resource = assert_success_envelope(&resource, json!("retained-status-resource"));
    let full_status = json_resource_content(resource, "codestory://status");
    assert_allowed_surface(&full_status, "affected", true, "local_navigation", "ready");
    assert_allowed_surface(&full_status, "ground", true, "local_navigation", "ready");
    assert_allowed_surface(
        &full_status,
        "packet",
        false,
        "agent_packet_search",
        "unavailable",
    );
}
