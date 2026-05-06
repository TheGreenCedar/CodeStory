use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::TempDir;

struct StdioFixture {
    workspace: TempDir,
    cache_dir: TempDir,
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
}

pub fn open_project(project_name: &str) -> String {
    runtime::normalize_project(project_name)
}
"#,
    )
    .expect("write lib.rs");
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

    let output = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("index")
        .arg("--refresh")
        .arg("full")
        .arg("--format")
        .arg("json")
        .arg("--project")
        .arg(workspace.path())
        .arg("--cache-dir")
        .arg(cache_dir.path())
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .output()
        .expect("run index");
    assert!(
        output.status.success(),
        "index failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    StdioFixture {
        workspace,
        cache_dir,
    }
}

fn spawn_stdio_server(fixture: &StdioFixture) -> StdioServer {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("serve")
        .arg("--stdio")
        .arg("--refresh")
        .arg("none")
        .arg("--project")
        .arg(fixture.workspace.path())
        .arg("--cache-dir")
        .arg(fixture.cache_dir.path())
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdio server");

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
    assert!(
        result.get("capabilities").is_some(),
        "initialize should report server capabilities: {response}"
    );
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
        "search",
        "symbol",
        "trail",
        "definition",
        "references",
        "symbols",
        "snippet",
        "ask",
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
fn transcript_calls_search_tool() {
    let fixture = indexed_fixture();
    let mut server = spawn_stdio_server(&fixture);

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

    let result = assert_success_envelope(&response, json!(10));
    assert!(
        result["indexed_symbol_hits"]
            .as_array()
            .is_some_and(|hits| hits
                .iter()
                .any(|hit| hit["display_name"] == "AppController")),
        "search tool should return AppController hit: {result}"
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
