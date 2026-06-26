use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct HttpFixture {
    workspace: TempDir,
    cache_dir: TempDir,
}

struct HttpServer {
    child: Child,
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

fn read_repo_file(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).expect("repo file should be readable")
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source.find(start).expect("start marker exists");
    let tail = &source[start_index..];
    let end_index = tail.find(end).expect("end marker exists");
    &tail[..end_index]
}

fn route_arm<'a>(handler: &'a str, route: &str, end: &str) -> &'a str {
    source_between(handler, &format!("\"{route}\" => {{"), end)
}

fn write_deep_rust_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "tiny-http-contract-fixture"
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

pub fn step0() -> usize { step1() }
pub fn step1() -> usize { step2() }
pub fn step2() -> usize { step3() }
pub fn step3() -> usize { step4() }
pub fn step4() -> usize { step5() }
pub fn step5() -> usize { step6() }
pub fn step6() -> usize { step7() }
pub fn step7() -> usize { step8() }
pub fn step8() -> usize { step9() }
pub fn step9() -> usize { step10() }
pub fn step10() -> usize { step11() }
pub fn step11() -> usize { step12() }
pub fn step12() -> usize { step13() }
pub fn step13() -> usize { step14() }
pub fn step14() -> usize { step15() }
pub fn step15() -> usize { 15 }
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

fn indexed_fixture() -> HttpFixture {
    let workspace = tempfile::tempdir().expect("workspace dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_deep_rust_workspace(workspace.path());

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

    HttpFixture {
        workspace,
        cache_dir,
    }
}

fn free_local_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind free local port");
    let addr = listener.local_addr().expect("read local addr");
    drop(listener);
    addr.to_string()
}

#[test]
fn http_serve_rejects_non_loopback_addr_before_opening_runtime_state() {
    let workspace = tempfile::tempdir().expect("workspace dir");
    let cache_dir = tempfile::tempdir().expect("cache dir");
    write_deep_rust_workspace(workspace.path());

    let output = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("serve")
        .arg("--refresh")
        .arg("none")
        .arg("--project")
        .arg(workspace.path())
        .arg("--cache-dir")
        .arg(cache_dir.path())
        .arg("--addr")
        .arg("0.0.0.0:0")
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .output()
        .expect("run serve");
    assert!(
        !output.status.success(),
        "non-loopback serve should be rejected without an acknowledgement flag"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--allow-non-loopback")
            && stderr.contains("without request authentication")
            && !stderr.contains("index not ready"),
        "serve should fail at the bind guard before opening cache/index state:\n{stderr}"
    );
}

fn spawn_http_server(fixture: &HttpFixture) -> (HttpServer, String) {
    let addr = free_local_addr();
    let child = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("serve")
        .arg("--refresh")
        .arg("none")
        .arg("--project")
        .arg(fixture.workspace.path())
        .arg("--cache-dir")
        .arg(fixture.cache_dir.path())
        .arg("--addr")
        .arg(&addr)
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn http server");

    let mut server = HttpServer { child };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = server.child.try_wait().expect("poll http server") {
            panic!("http server exited before readiness: {status}");
        }
        if http_get(&addr, "/health")
            .ok()
            .is_some_and(|response| response.status == 200 && response.body["ok"] == true)
        {
            return (server, addr);
        }
        assert!(
            Instant::now() < deadline,
            "http server did not become ready on {addr}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

struct HttpResponse {
    status: u16,
    body: Value,
}

fn http_get(addr: &str, target: &str) -> std::io::Result<HttpResponse> {
    http_get_with_headers(addr, target, &[("Host", addr)])
}

fn http_get_with_headers(
    addr: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> std::io::Result<HttpResponse> {
    let mut stream = TcpStream::connect(addr)?;
    write!(stream, "GET {target} HTTP/1.1\r\n")?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "Connection: close\r\n\r\n")?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)?;

    let mut response_bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => response_bytes.extend_from_slice(&chunk[..read]),
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionReset
                    && !response_bytes.is_empty() =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    let response = String::from_utf8(response_bytes)
        .unwrap_or_else(|error| panic!("HTTP response should be UTF-8: {error}"));
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("HTTP response should include headers and body: {response:?}"));
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("HTTP response should include numeric status: {headers:?}"));
    let body = serde_json::from_str(body.trim())
        .unwrap_or_else(|error| panic!("HTTP body should be JSON: {error}\n{body}"));
    Ok(HttpResponse { status, body })
}

#[test]
fn http_serve_rejects_non_loopback_host_and_origin_headers() {
    let fixture = indexed_fixture();
    let (_server, addr) = spawn_http_server(&fixture);

    let bad_host = http_get_with_headers(&addr, "/health", &[("Host", "evil.test:3917")])
        .expect("bad host response");
    assert_eq!(bad_host.status, 403);
    assert_eq!(
        bad_host.body.pointer("/error/code").and_then(Value::as_str),
        Some("forbidden_http_boundary"),
        "non-loopback Host should fail closed: {}",
        bad_host.body
    );

    let bad_origin = http_get_with_headers(
        &addr,
        "/health",
        &[("Host", &addr), ("Origin", "http://evil.test:3917")],
    )
    .expect("bad origin response");
    assert_eq!(bad_origin.status, 403);
    assert_eq!(
        bad_origin
            .body
            .pointer("/error/code")
            .and_then(Value::as_str),
        Some("forbidden_http_boundary"),
        "non-loopback Origin should fail closed: {}",
        bad_origin.body
    );

    let malformed_ipv6_host =
        http_get_with_headers(&addr, "/health", &[("Host", "[::1]evil.test:3917")])
            .expect("malformed ipv6 host response");
    assert_eq!(
        malformed_ipv6_host.status, 403,
        "malformed bracketed Host should fail closed: {}",
        malformed_ipv6_host.body
    );

    let malformed_ipv6_origin = http_get_with_headers(
        &addr,
        "/health",
        &[("Host", &addr), ("Origin", "http://[::1]evil.test:3917")],
    )
    .expect("malformed ipv6 origin response");
    assert_eq!(
        malformed_ipv6_origin.status, 403,
        "malformed bracketed Origin should fail closed: {}",
        malformed_ipv6_origin.body
    );

    for host in ["localhost:3917", "[::1]:3917"] {
        let response = http_get_with_headers(&addr, "/health", &[("Host", host)])
            .unwrap_or_else(|error| panic!("loopback Host {host}: {error}"));
        assert_eq!(
            response.status, 200,
            "loopback Host {host} should stay allowed: {}",
            response.body
        );
    }

    let loopback_origin = http_get_with_headers(
        &addr,
        "/health",
        &[("Host", &addr), ("Origin", "http://localhost:3917")],
    )
    .expect("loopback origin response");
    assert_eq!(
        loopback_origin.status, 200,
        "loopback Origin should stay allowed: {}",
        loopback_origin.body
    );
}

fn get_json(addr: &str, target: &str) -> Value {
    let response = http_get(addr, target).unwrap_or_else(|error| panic!("GET {target}: {error}"));
    assert_eq!(
        response.status, 200,
        "GET {target} should succeed with JSON body: {}",
        response.body
    );
    response.body
}

fn assert_nonempty_array(value: &Value, pointer: &str) -> usize {
    let items = value
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected nonempty array at {pointer}: {value}"));
    assert!(
        !items.is_empty(),
        "expected nonempty array at {pointer}: {value}"
    );
    items.len()
}

fn max_node_depth(value: &Value, pointer: &str) -> u64 {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected graph nodes at {pointer}: {value}"))
        .iter()
        .map(|node| {
            node.get("depth")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| panic!("graph node should include numeric depth: {node}"))
        })
        .max()
        .expect("graph should include at least one node")
}

fn graph_node_labels<'a>(value: &'a Value, pointer: &str) -> Vec<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected graph nodes at {pointer}: {value}"))
        .iter()
        .map(|node| {
            node.get("label")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("graph node should include string label: {node}"))
        })
        .collect()
}

fn symbol_id_by_label(symbols: &Value, label: &str) -> String {
    symbols
        .as_array()
        .unwrap_or_else(|| panic!("symbols should be an array: {symbols}"))
        .iter()
        .find(|symbol| symbol["label"] == label)
        .and_then(|symbol| symbol["id"].as_str())
        .unwrap_or_else(|| panic!("symbols should include label {label}: {symbols}"))
        .to_string()
}

#[test]
fn http_routes_and_stdio_tools_keep_aligned_default_contracts() {
    let http = read_repo_file("crates/codestory-cli/src/http_transport.rs");
    let stdio = read_repo_file("crates/codestory-cli/src/stdio_transport.rs");
    let catalog = read_repo_file("crates/codestory-cli/src/stdio_catalog.rs");
    let http_handler = source_between(
        &http,
        "pub(crate) fn handle_http_request",
        "fn resolve_http_target_from_params",
    );
    let shared_browser_defaults = source_between(
        &http,
        "const BROWSER_TRAIL_DEFAULT_DEPTH",
        "fn handle_http_request",
    );
    let http_trail = route_arm(http_handler, "/trail", "_ =>");
    let http_references = route_arm(http_handler, "/references", "\"/symbols\"");
    let http_symbols = route_arm(http_handler, "/symbols", "\"/trail\"");
    let stdio_trail = source_between(
        &stdio,
        "fn handle_stdio_trail",
        "fn handle_stdio_definition",
    );
    let stdio_references = source_between(
        &stdio,
        "fn handle_stdio_references",
        "fn handle_stdio_symbols",
    );
    let stdio_symbols =
        source_between(&stdio, "fn handle_stdio_symbols", "fn handle_stdio_snippet");

    for route in [
        "/search",
        "/definition",
        "/references",
        "/symbols",
        "/trail",
    ] {
        assert!(
            http_handler.contains(&format!("\"{route}\"")),
            "HTTP handler should keep existing route {route}"
        );
    }
    for tool in ["definition", "references", "symbols", "trail"] {
        assert!(
            catalog.contains(&format!("name: \"{tool}\"")),
            "stdio catalog should keep tool {tool}"
        );
    }

    assert!(
        shared_browser_defaults.contains("BROWSER_TRAIL_DEFAULT_DEPTH: u32 = 2")
            && http_trail.contains("browser_trail_depth(")
            && stdio_trail.contains("BROWSER_TRAIL_DEFAULT_DEPTH"),
        "HTTP and stdio trail should share the named default depth=2 contract"
    );
    assert!(
        shared_browser_defaults.contains("BROWSER_TRAIL_MAX_DEPTH: u32 = 10")
            && http_trail.contains("browser_trail_depth(")
            && stdio_trail.contains("BROWSER_TRAIL_MAX_DEPTH"),
        "HTTP /trail and stdio trail should share the named maximum depth=10 contract"
    );
    assert!(
        shared_browser_defaults.contains("BROWSER_TRAIL_MAX_NODES: u32 = 80")
            && http_trail.contains("browser_trail_config(")
            && stdio_trail.contains("browser_trail_config("),
        "HTTP and stdio trail should share max_nodes=80 through the common helper"
    );

    assert!(
        shared_browser_defaults.contains("BROWSER_REFERENCES_DEPTH: u32 = 0")
            && http_references.contains("browser_references_config(")
            && stdio_references.contains("browser_references_config("),
        "HTTP and stdio references should share incoming depth=0 semantics through the common helper"
    );
    assert!(
        source_between(
            &http,
            "fn browser_references_config",
            "fn browser_trail_config"
        )
        .contains("direction: TrailDirection::Incoming"),
        "shared references helper should preserve incoming direction"
    );
    assert!(
        shared_browser_defaults.contains("BROWSER_REFERENCES_MAX_NODES: u32 = 120")
            && http_references.contains("browser_references_config(")
            && stdio_references.contains("browser_references_config("),
        "HTTP and stdio references should share max_nodes=120 through the common helper"
    );

    assert!(
        shared_browser_defaults.contains("BROWSER_SYMBOLS_MAX_LIMIT: u32 = 2_000")
            && http_symbols.contains("browser_symbols_limit(")
            && stdio_symbols.contains("BROWSER_SYMBOLS_MAX_LIMIT"),
        "HTTP and stdio symbols should share the explicit limit clamp"
    );
    assert!(
        shared_browser_defaults.contains("BROWSER_SYMBOLS_DEFAULT_LIMIT: u32 = 300")
            && http_symbols.contains("browser_symbols_limit(")
            && stdio_symbols.contains("BROWSER_SYMBOLS_DEFAULT_LIMIT"),
        "HTTP /symbols should default omitted root limit to the stdio default of 300"
    );
    assert!(
        catalog.contains(".with_default(ValueLiteral::Integer(300))")
            && catalog.contains(".with_bounds(1, 2000)"),
        "stdio symbols catalog should document the shared 300 default and 1..2000 bounds"
    );
}

#[test]
fn http_target_ambiguity_returns_json_error_and_choose_resolves() {
    let fixture = indexed_fixture();
    let (_server, addr) = spawn_http_server(&fixture);

    let ambiguous = http_get(&addr, "/definition?q=configure").expect("ambiguous definition");
    assert_eq!(ambiguous.status, 400);
    assert_eq!(
        ambiguous
            .body
            .pointer("/error/code")
            .and_then(Value::as_str),
        Some("ambiguous_target"),
        "ambiguous HTTP target should return structured alternatives: {}",
        ambiguous.body
    );
    let alternatives = ambiguous
        .body
        .pointer("/error/alternatives")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ambiguous alternatives: {}", ambiguous.body));
    assert!(alternatives.len() >= 2);
    let second_id = alternatives[1]
        .get("node_id")
        .and_then(Value::as_str)
        .expect("second alternative node id")
        .to_string();
    assert_eq!(alternatives[0].get("number"), Some(&Value::from(1)));
    assert!(
        alternatives[0]
            .get("node_ref")
            .and_then(Value::as_str)
            .is_some(),
        "alternatives should include node refs: {}",
        ambiguous.body
    );

    let chosen = get_json(&addr, "/definition?q=configure&choose=2");
    assert_eq!(
        chosen
            .pointer("/resolution/resolved/node_id")
            .and_then(Value::as_str),
        Some(second_id.as_str()),
        "HTTP choose should resolve the displayed alternative #2: {chosen}"
    );

    let invalid =
        http_get(&addr, "/definition?q=AppController&choose=abc").expect("invalid choose response");
    assert_eq!(invalid.status, 400);
    assert_eq!(
        invalid.body.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_target"),
        "invalid choose should return structured bad-request JSON: {}",
        invalid.body
    );
}

#[test]
fn http_smoke_keeps_existing_routes_and_default_semantics_against_indexed_repo() {
    let fixture = indexed_fixture();
    let (_server, addr) = spawn_http_server(&fixture);

    let search = http_get(&addr, "/search?q=AppController&repo_text=off")
        .expect("search fail-closed response");
    assert_eq!(
        search.status, 400,
        "HTTP /search is product search and should fail closed without full sidecars: {}",
        search.body
    );
    assert_eq!(
        search.body.pointer("/error/code").and_then(Value::as_str),
        Some("search_unavailable"),
        "HTTP /search should return a structured mandatory-sidecar error: {}",
        search.body
    );
    assert!(
        search
            .body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("sidecar retrieval primary")),
        "HTTP /search should explain the sidecar-primary boundary: {}",
        search.body
    );

    let definition = get_json(&addr, "/definition?q=AppController");
    assert_eq!(
        definition["symbol"]["node"]["display_name"], "AppController",
        "/definition should resolve by query and include symbol details: {definition}"
    );
    assert!(
        definition["resolution"].is_object() && definition["definition"].is_object(),
        "/definition should preserve resolution and definition objects: {definition}"
    );

    let symbols = get_json(&addr, "/symbols");
    let symbol_count = symbols
        .as_array()
        .unwrap_or_else(|| panic!("/symbols should return a JSON array: {symbols}"))
        .len();
    assert!(
        (1..=300).contains(&symbol_count),
        "omitted /symbols limit should return a bounded nonempty root list: {symbols}"
    );
    assert!(
        symbols
            .as_array()
            .expect("symbols array")
            .iter()
            .any(|symbol| symbol["label"] == "AppController"),
        "/symbols should include indexed root symbols: {symbols}"
    );
    let step0_id = symbol_id_by_label(&symbols, "step0");
    let step10_id = symbol_id_by_label(&symbols, "step10");

    let references = get_json(&addr, &format!("/references?id={step10_id}"));
    assert!(
        references["resolution"].is_object() && references["references"]["focus"].is_object(),
        "/references should preserve resolution and references context: {references}"
    );
    assert_nonempty_array(&references, "/references/trail/nodes");
    let reference_labels = graph_node_labels(&references, "/references/trail/nodes");
    let references_with_depth = get_json(&addr, &format!("/references?id={step10_id}&depth=99"));
    assert_eq!(
        reference_labels,
        graph_node_labels(&references_with_depth, "/references/trail/nodes"),
        "/references should ignore depth and keep stdio-compatible default semantics"
    );
    assert!(
        !reference_labels.contains(&"step11"),
        "omitted /references should not walk outgoing callees: {references}"
    );

    let default_trail = get_json(&addr, "/trail?q=step0");
    assert_eq!(default_trail["focus"]["display_name"], "step0");
    assert_nonempty_array(&default_trail, "/trail/nodes");
    assert!(
        max_node_depth(&default_trail, "/trail/nodes") <= 2,
        "/trail should support q and default omitted depth to the stdio depth: {default_trail}"
    );
    assert!(
        default_trail.get("story").is_none(),
        "/trail should not include story unless requested: {default_trail}"
    );

    let story_trail = get_json(&addr, &format!("/trail?id={step0_id}&story=true"));
    assert_eq!(story_trail["focus"]["display_name"], "step0");
    assert!(
        story_trail
            .pointer("/story/summary")
            .and_then(Value::as_str)
            .is_some_and(|summary| summary.contains("Story trail")),
        "/trail?story=true should include a readable story summary: {story_trail}"
    );
    assert_nonempty_array(&story_trail, "/story/uncertainty");
    assert!(
        story_trail
            .pointer("/story/test_scope")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item
                .as_str()
                .is_some_and(|text| text.contains("tests and benches excluded")))),
        "/trail?story=true should make production test scope explicit: {story_trail}"
    );

    let incoming_trail = get_json(
        &addr,
        &format!("/trail?id={step0_id}&direction=incoming&depth=1"),
    );
    assert_eq!(incoming_trail["focus"]["display_name"], "step0");
    assert_nonempty_array(&incoming_trail, "/trail/nodes");
    assert!(
        max_node_depth(&incoming_trail, "/trail/nodes") <= 1,
        "/trail should accept direction and explicit depth params: {incoming_trail}"
    );

    let trail = get_json(&addr, &format!("/trail?id={step0_id}&depth=99"));
    assert_eq!(trail["focus"]["display_name"], "step0");
    assert_nonempty_array(&trail, "/trail/nodes");
    assert!(
        max_node_depth(&trail, "/trail/nodes") <= 10,
        "/trail depth=99 should be clamped to the stdio maximum depth: {trail}"
    );

    let one_symbol = get_json(&addr, "/symbols?limit=1");
    assert_eq!(
        one_symbol.as_array().map(Vec::len),
        Some(1),
        "/symbols should honor an explicit bounded root limit: {one_symbol}"
    );
}
