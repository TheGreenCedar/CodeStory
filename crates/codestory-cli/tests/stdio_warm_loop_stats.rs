use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;
use tempfile::{TempDir, tempdir};

const WARM_REPETITIONS: usize = 20;

#[derive(Debug)]
struct WarmLoopFixture {
    workspace: TempDir,
    cache_dir: TempDir,
}

struct StdioServer {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Drop for StdioServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
struct TimedJson {
    elapsed_ms: f64,
    response_bytes: u64,
    value: Value,
}

#[derive(Debug, Clone, Serialize)]
struct OperationSample {
    sequence: usize,
    operation: String,
    elapsed_ms: f64,
    response_bytes: u64,
}

#[derive(Debug, Serialize)]
struct ToolLatencyStats {
    operation: String,
    samples: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    response_bytes_p50: u64,
    response_bytes_max: u64,
}

#[derive(Debug, Serialize)]
struct ColdOneShotStats {
    total_ms: f64,
    search_ms: f64,
    symbol_ms: f64,
    trail_ms: f64,
    snippet_ms: f64,
    search_response_bytes: u64,
    symbol_response_bytes: u64,
    trail_response_bytes: u64,
    snippet_response_bytes: u64,
}

#[derive(Debug, Serialize)]
struct PreconditionStats {
    index_ms: f64,
    index_semantic_reload_ms: Option<u64>,
    semantic_doc_count: u64,
}

#[derive(Debug, Serialize)]
struct ProtocolStats {
    startup_ms: f64,
    tools_list_ms: f64,
    first_tool: String,
    first_tool_ms: f64,
    stdout_protocol_only: bool,
}

#[derive(Debug, Serialize)]
struct StateStats {
    warm_search_dir_unchanged: bool,
    fallback_reason: Option<String>,
    warm_stdio_semantic_reload_ms: Option<u64>,
    semantic_reload_note: String,
    error_count: u64,
}

#[derive(Debug, Serialize)]
struct StdioWarmLoopStats {
    schema_version: u32,
    scenario: String,
    project_root: String,
    cache_dir: String,
    storage_path: String,
    search_dir: String,
    warm_repetitions: usize,
    precondition: PreconditionStats,
    protocol: ProtocolStats,
    cold_one_shot: ColdOneShotStats,
    cold_equivalent_total_ms: f64,
    warm_stdio_total_ms: f64,
    warm_stdio_per_loop_ms: f64,
    warm_vs_cold_per_loop_ratio: f64,
    sidecar_status: ToolLatencyStats,
    warm_stdio: Vec<ToolLatencyStats>,
    state: StateStats,
    transcript: Vec<OperationSample>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

fn release_cli_binary() -> PathBuf {
    repo_root()
        .join("target")
        .join("release")
        .join(format!("codestory-cli{}", std::env::consts::EXE_SUFFIX))
}

fn write_tiny_rust_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "tiny-stdio-warm-loop-fixture"
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

fn indexed_fixture(binary: &Path) -> (WarmLoopFixture, TimedJson) {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let index = run_cli_json(
        binary,
        workspace.path(),
        cache_dir.path(),
        &[
            "index".to_string(),
            "--refresh".to_string(),
            "full".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    (
        WarmLoopFixture {
            workspace,
            cache_dir,
        },
        index,
    )
}

fn run_cli_json(binary: &Path, workspace: &Path, cache_dir: &Path, args: &[String]) -> TimedJson {
    let started = Instant::now();
    let output = Command::new(binary)
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_EMBED_RUNTIME_MODE", "hash")
        .output()
        .expect("run codestory-cli");
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    TimedJson {
        elapsed_ms,
        response_bytes: output.stdout.len() as u64,
        value: serde_json::from_slice(&output.stdout).expect("parse CLI JSON output"),
    }
}

fn spawn_stdio_server(binary: &Path, fixture: &WarmLoopFixture) -> StdioServer {
    let mut child = Command::new(binary)
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

    StdioServer {
        stdin: child.stdin.take(),
        stdout: BufReader::new(child.stdout.take().expect("stdio stdout")),
        child,
    }
}

fn send_json(server: &mut StdioServer, request: Value) -> TimedJson {
    let started = Instant::now();
    let stdin = server.stdin.as_mut().expect("stdio stdin");
    writeln!(stdin, "{request}").expect("write request line");
    stdin.flush().expect("flush request line");

    let mut response = String::new();
    let response_bytes = server
        .stdout
        .read_line(&mut response)
        .expect("read response line");
    assert!(response_bytes > 0, "stdio server closed before responding");
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    TimedJson {
        elapsed_ms,
        response_bytes: response_bytes as u64,
        value: serde_json::from_str(response.trim()).expect("stdio response must be JSON-RPC"),
    }
}

fn close_stdio_server(mut server: StdioServer) -> String {
    drop(server.stdin.take());
    let mut trailing_stdout = String::new();
    server
        .stdout
        .read_to_string(&mut trailing_stdout)
        .expect("read trailing stdout");
    let status = server.child.wait().expect("wait for stdio server");
    assert!(
        status.success(),
        "stdio server should exit cleanly after stdin closes"
    );
    trailing_stdout
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

fn record_sample(samples: &mut Vec<OperationSample>, operation: &str, timed: &TimedJson) {
    samples.push(OperationSample {
        sequence: samples.len() + 1,
        operation: operation.to_string(),
        elapsed_ms: round2(timed.elapsed_ms),
        response_bytes: timed.response_bytes,
    });
}

fn tool_call(id: impl Into<Value>, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.into(),
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": arguments,
        }
    })
}

fn resource_read(id: impl Into<Value>, uri: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.into(),
        "method": "resources/read",
        "params": {"uri": uri}
    })
}

fn search_dir_for_storage(storage_path: &Path) -> PathBuf {
    let parent = storage_path.parent().expect("storage parent");
    let stem = storage_path
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("storage file stem");
    parent.join(format!("{stem}.search"))
}

fn json_path<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut current = value;
    for key in path {
        current = match current {
            Value::Object(fields) => fields
                .get(*key)
                .unwrap_or_else(|| panic!("missing object key {key:?} at path {path:?}")),
            Value::Array(items) => {
                let index = key.parse::<usize>().unwrap_or_else(|_| {
                    panic!("expected array index at path {path:?}, got {key:?}")
                });
                items
                    .get(index)
                    .unwrap_or_else(|| panic!("missing array index {index} at path {path:?}"))
            }
            _ => panic!("cannot descend into non-container at path {path:?}, key {key:?}"),
        };
    }
    current
}

fn string_field<'a>(value: &'a Value, path: &[&str]) -> &'a str {
    json_path(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("expected string at path {:?}", path))
}

fn optional_string_field(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = match current {
            Value::Object(fields) => fields.get(*key)?,
            Value::Array(items) => items.get(key.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    current.as_str().map(ToOwned::to_owned)
}

fn maybe_u64_field(value: &Value, path: &[&str]) -> Option<u64> {
    let mut current = value;
    for key in path {
        current = match current {
            Value::Object(fields) => fields.get(*key)?,
            Value::Array(items) => items.get(key.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    current.as_u64()
}

fn u64_field(value: &Value, path: &[&str]) -> u64 {
    json_path(value, path)
        .as_u64()
        .unwrap_or_else(|| panic!("expected u64 at path {:?}", path))
}

fn top_symbol_id_from_search(value: &Value) -> String {
    string_field(value, &["indexed_symbol_hits", "0", "node_id"]).to_string()
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

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    assert!(!values.is_empty(), "percentile requires samples");
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| {
        left.partial_cmp(right)
            .expect("latency samples should not be NaN")
    });
    let rank = ((sorted.len().saturating_sub(1) as f64) * percentile).ceil() as usize;
    round2(sorted[rank.min(sorted.len() - 1)])
}

fn percentile_u64(values: &[u64], percentile: f64) -> u64 {
    assert!(!values.is_empty(), "percentile requires samples");
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = ((sorted.len().saturating_sub(1) as f64) * percentile).ceil() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn warm_tool_stats(samples: &[OperationSample]) -> Vec<ToolLatencyStats> {
    let mut grouped: BTreeMap<&str, Vec<&OperationSample>> = BTreeMap::new();
    for sample in samples {
        if !matches!(
            sample.operation.as_str(),
            "search" | "symbol" | "trail" | "snippet" | "resources/read:status"
        ) {
            continue;
        }
        grouped.entry(&sample.operation).or_default().push(sample);
    }
    grouped
        .into_iter()
        .map(|(operation, samples)| tool_latency_stats(operation, samples.into_iter()))
        .collect()
}

fn operation_stats(samples: &[OperationSample], operation: &str) -> ToolLatencyStats {
    let filtered = samples
        .iter()
        .filter(|sample| sample.operation == operation)
        .collect::<Vec<_>>();
    tool_latency_stats(operation, filtered.into_iter())
}

fn tool_latency_stats<'a>(
    operation: &str,
    samples: impl Iterator<Item = &'a OperationSample>,
) -> ToolLatencyStats {
    let samples = samples.collect::<Vec<_>>();
    assert!(
        !samples.is_empty(),
        "missing operation samples for {operation}"
    );
    let latencies = samples
        .iter()
        .map(|sample| sample.elapsed_ms)
        .collect::<Vec<_>>();
    let bytes = samples
        .iter()
        .map(|sample| sample.response_bytes)
        .collect::<Vec<_>>();
    ToolLatencyStats {
        operation: operation.to_string(),
        samples: samples.len(),
        p50_ms: percentile(&latencies, 0.50),
        p95_ms: percentile(&latencies, 0.95),
        p99_ms: percentile(&latencies, 0.99),
        max_ms: percentile(&latencies, 1.0),
        response_bytes_p50: percentile_u64(&bytes, 0.50),
        response_bytes_max: percentile_u64(&bytes, 1.0),
    }
}

#[test]
#[ignore = "warm-loop stats harness; run with cargo test -p codestory-cli --test stdio_warm_loop_stats -- --ignored --nocapture after cargo build --release -p codestory-cli"]
fn warm_stdio_agent_loop_emits_stats_without_protocol_pollution() {
    let binary = release_cli_binary();
    assert!(
        binary.is_file(),
        "missing release binary at {}. Run `cargo build --release -p codestory-cli` first.",
        binary.display()
    );

    let (fixture, index) = indexed_fixture(&binary);
    let index_json = &index.value;
    let storage_path = PathBuf::from(string_field(index_json, &["storage_path"]));
    let search_dir = search_dir_for_storage(&storage_path);
    let cold_search = run_cli_json(
        &binary,
        fixture.workspace.path(),
        fixture.cache_dir.path(),
        &[
            "search".to_string(),
            "--query".to_string(),
            "AppController".to_string(),
            "--repo-text".to_string(),
            "off".to_string(),
            "--limit".to_string(),
            "10".to_string(),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );
    let top_symbol_id = top_symbol_id_from_search(&cold_search.value);
    let cold_symbol = run_cli_json(
        &binary,
        fixture.workspace.path(),
        fixture.cache_dir.path(),
        &[
            "symbol".to_string(),
            format!("--id={top_symbol_id}"),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );
    let cold_trail = run_cli_json(
        &binary,
        fixture.workspace.path(),
        fixture.cache_dir.path(),
        &[
            "trail".to_string(),
            format!("--id={top_symbol_id}"),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );
    let cold_snippet = run_cli_json(
        &binary,
        fixture.workspace.path(),
        fixture.cache_dir.path(),
        &[
            "snippet".to_string(),
            format!("--id={top_symbol_id}"),
            "--refresh".to_string(),
            "none".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    );

    let search_dir_before_warm = fs::metadata(&search_dir)
        .expect("search dir metadata before warm stdio reads")
        .modified()
        .expect("search dir modified time before warm stdio reads");

    let spawn_started = Instant::now();
    let mut server = spawn_stdio_server(&binary, &fixture);
    let mut transcript = Vec::new();
    let initialize = send_json(
        &mut server,
        json!({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "warm-loop-stats", "version": "0"}
            }
        }),
    );
    assert_success_envelope(&initialize.value, json!("initialize"));
    let startup_ms = spawn_started.elapsed().as_secs_f64() * 1000.0;
    record_sample(&mut transcript, "initialize", &initialize);

    let tools = send_json(
        &mut server,
        json!({"jsonrpc": "2.0", "id": "tools-list", "method": "tools/list"}),
    );
    assert_success_envelope(&tools.value, json!("tools-list"));
    record_sample(&mut transcript, "tools/list", &tools);

    let mut seen_operations = BTreeSet::new();
    let mut first_tool_ms = None;
    let mut warm_status_payload = None;
    for iteration in 0..WARM_REPETITIONS {
        let search = send_json(
            &mut server,
            tool_call(
                format!("search-{iteration}"),
                "search",
                json!({"query": "AppController", "repo_text": "off", "limit": 10}),
            ),
        );
        let search_result =
            assert_success_envelope(&search.value, json!(format!("search-{iteration}")));
        if first_tool_ms.is_none() {
            first_tool_ms = Some(search.elapsed_ms);
        }
        assert!(
            !search_result["indexed_symbol_hits"]
                .as_array()
                .expect("search hits")
                .is_empty(),
            "warm stdio search should return indexed hits: {search_result}"
        );
        seen_operations.insert("search");
        record_sample(&mut transcript, "search", &search);

        let symbol = send_json(
            &mut server,
            tool_call(
                format!("symbol-{iteration}"),
                "symbol",
                json!({"id": top_symbol_id}),
            ),
        );
        assert_success_envelope(&symbol.value, json!(format!("symbol-{iteration}")));
        seen_operations.insert("symbol");
        record_sample(&mut transcript, "symbol", &symbol);

        let trail = send_json(
            &mut server,
            tool_call(
                format!("trail-{iteration}"),
                "trail",
                json!({"id": top_symbol_id, "direction": "both", "depth": 2}),
            ),
        );
        assert_success_envelope(&trail.value, json!(format!("trail-{iteration}")));
        seen_operations.insert("trail");
        record_sample(&mut transcript, "trail", &trail);

        let snippet = send_json(
            &mut server,
            tool_call(
                format!("snippet-{iteration}"),
                "snippet",
                json!({"id": top_symbol_id}),
            ),
        );
        assert_success_envelope(&snippet.value, json!(format!("snippet-{iteration}")));
        seen_operations.insert("snippet");
        record_sample(&mut transcript, "snippet", &snippet);

        let status = send_json(
            &mut server,
            resource_read(format!("status-{iteration}"), "codestory://status"),
        );
        let status_result =
            assert_success_envelope(&status.value, json!(format!("status-{iteration}")));
        warm_status_payload = Some(json_resource_content(status_result, "codestory://status"));
        seen_operations.insert("resources/read:status");
        record_sample(&mut transcript, "resources/read:status", &status);
    }
    let warm_status_payload = warm_status_payload.expect("warm status payload");

    let trailing_stdout = close_stdio_server(server);
    let stdout_protocol_clean = trailing_stdout.lines().all(|line| line.trim().is_empty());
    assert!(
        stdout_protocol_clean,
        "stdio stdout should contain only JSON-RPC response lines; trailing stdout was {trailing_stdout:?}"
    );
    for expected in [
        "search",
        "symbol",
        "trail",
        "snippet",
        "resources/read:status",
    ] {
        assert!(
            seen_operations.contains(expected),
            "warm transcript should include {expected}"
        );
    }

    let search_dir_after = fs::metadata(&search_dir)
        .expect("search dir metadata after warm stdio reads")
        .modified()
        .expect("search dir modified time after warm stdio reads");
    let warm_search_dir_unchanged = search_dir_before_warm == search_dir_after;
    assert!(
        warm_search_dir_unchanged,
        "warm stdio read commands should not recreate the persisted search dir"
    );

    let cold_one_shot = ColdOneShotStats {
        total_ms: round2(
            cold_search.elapsed_ms
                + cold_symbol.elapsed_ms
                + cold_trail.elapsed_ms
                + cold_snippet.elapsed_ms,
        ),
        search_ms: round2(cold_search.elapsed_ms),
        symbol_ms: round2(cold_symbol.elapsed_ms),
        trail_ms: round2(cold_trail.elapsed_ms),
        snippet_ms: round2(cold_snippet.elapsed_ms),
        search_response_bytes: cold_search.response_bytes,
        symbol_response_bytes: cold_symbol.response_bytes,
        trail_response_bytes: cold_trail.response_bytes,
        snippet_response_bytes: cold_snippet.response_bytes,
    };
    let warm_stdio_total_ms = round2(
        transcript
            .iter()
            .filter(|sample| {
                matches!(
                    sample.operation.as_str(),
                    "search" | "symbol" | "trail" | "snippet"
                )
            })
            .map(|sample| sample.elapsed_ms)
            .sum(),
    );
    let warm_stdio_per_loop_ms = round2(warm_stdio_total_ms / WARM_REPETITIONS as f64);
    let cold_equivalent_total_ms = round2(cold_one_shot.total_ms * WARM_REPETITIONS as f64);
    let warm_vs_cold_per_loop_ratio = round2(warm_stdio_per_loop_ms / cold_one_shot.total_ms);
    let stats = StdioWarmLoopStats {
        project_root: fixture.workspace.path().display().to_string(),
        cache_dir: fixture.cache_dir.path().display().to_string(),
        storage_path: storage_path.display().to_string(),
        search_dir: search_dir.display().to_string(),
        warm_repetitions: WARM_REPETITIONS,
        schema_version: 1,
        scenario: "small_fixture_release_warm_stdio_agent_loop".to_string(),
        precondition: PreconditionStats {
            index_ms: round2(index.elapsed_ms),
            index_semantic_reload_ms: maybe_u64_field(
                index_json,
                &["phase_timings", "semantic_reload_ms"],
            ),
            semantic_doc_count: u64_field(index_json, &["retrieval", "semantic_doc_count"]),
        },
        protocol: ProtocolStats {
            startup_ms: round2(startup_ms),
            tools_list_ms: round2(tools.elapsed_ms),
            first_tool: "search".to_string(),
            first_tool_ms: round2(first_tool_ms.expect("first search tool timing")),
            stdout_protocol_only: stdout_protocol_clean,
        },
        cold_one_shot,
        cold_equivalent_total_ms,
        warm_stdio_total_ms,
        warm_stdio_per_loop_ms,
        warm_vs_cold_per_loop_ratio,
        sidecar_status: operation_stats(&transcript, "resources/read:status"),
        warm_stdio: warm_tool_stats(&transcript),
        state: StateStats {
            warm_search_dir_unchanged,
            fallback_reason: optional_string_field(&warm_status_payload, &["fallback_reason"]),
            warm_stdio_semantic_reload_ms: None,
            semantic_reload_note:
                "serve --stdio does not expose a dedicated semantic reload phase; startup_ms includes any warm-server load cost"
                    .to_string(),
            error_count: u64_field(index_json, &["summary", "stats", "error_count"]),
        },
        transcript,
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&stats).expect("serialize warm-loop stats")
    );

    assert!(
        stats.protocol.startup_ms < 10_000.0,
        "warm stdio startup should stay bounded on a small fixture, got {:.2}ms",
        stats.protocol.startup_ms
    );
    assert!(
        stats.protocol.first_tool_ms < 5_000.0,
        "first stdio tool should stay bounded on a small fixture, got {:.2}ms",
        stats.protocol.first_tool_ms
    );
}
