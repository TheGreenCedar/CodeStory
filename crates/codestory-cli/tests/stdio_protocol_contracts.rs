use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
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
fn tool_catalog_keeps_stable_read_only_browser_tool_names() {
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

    assert_eq!(
        sorted_field_values(&tools, "tools", "name"),
        vec![
            "context",
            "definition",
            "references",
            "search",
            "snippet",
            "symbol",
            "symbols",
            "trail",
        ],
        "stdio browser tool names should stay stable and read-only: {tools}"
    );

    for tool in tools["tools"].as_array().expect("tools array") {
        assert_read_only_tool_metadata(tool);

        let name = tool["name"].as_str().expect("tool name");
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
        schema_property(trail, "story")["type"],
        "boolean",
        "trail.story should be a boolean opt-in: {trail}"
    );
    assert_eq!(
        schema_property(trail, "story").get("default"),
        Some(&json!(false)),
        "trail.story should document the stdio default: {trail}"
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
        "context",
        "definition",
        "references",
        "search",
        "snippet",
        "symbol",
        "symbols",
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
    }

    let search_hit_schema = tool_output_schema(&tools, "search")
        .pointer("/properties/hits/items")
        .unwrap_or_else(|| panic!("search outputSchema should describe hit items: {tools}"));
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
fn resources_read_status_reports_stale_index_freshness_with_bounded_latency() {
    let fixture = indexed_fixture();
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

    let mut server = spawn_stdio_server(&fixture);
    let mut elapsed = Vec::new();
    let mut last_status = Value::Null;
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

    assert_stale_freshness_counts(&last_status, "codestory://status");
    elapsed.sort_unstable();
    let p95 = elapsed[(elapsed.len() * 95).div_ceil(100) - 1];
    assert!(
        p95 < Duration::from_millis(250),
        "warm status freshness check p95 should stay under 250ms for a small repo, got {p95:?}"
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
            .is_some_and(|calls| calls.len() >= 3),
        "agent guide should include a concise default browser loop or call sequence: {guide}"
    );
    let mut strings = Vec::new();
    string_values_recursive(&guide, &mut strings);
    for expected in ["search", "definition", "snippet"] {
        assert!(
            strings.iter().any(|value| value.contains(expected)),
            "agent guide should recommend {expected} in its call sequence: {guide}"
        );
    }
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
fn context_tool_maps_target_id_to_deep_browser_request() {
    let fixture = indexed_fixture();
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
fn search_tool_exposes_continuation_links_and_clamps_tiny_payloads() {
    let fixture = indexed_fixture();
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
fn search_tool_does_not_offer_symbol_links_for_non_resolvable_repo_text_hits() {
    let fixture = indexed_fixture();
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
}

#[test]
fn definition_tool_exposes_symbol_snippet_references_and_trail_links() {
    let fixture = indexed_fixture();
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
fn symbol_tool_reports_ambiguous_targets_and_choose_resolves_displayed_number() {
    let fixture = indexed_fixture();
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
