//! JSON-lines stdio transport for the local integration server.
//!
//! `serve --stdio` reads one JSON-RPC request per stdin line and writes one
//! JSON response per stdout line. Protocol errors are returned as JSON-RPC
//! errors; tool execution failures are encoded as tool-call results so clients
//! can display structured failure content without losing the response envelope.

use anyhow::{Context, Result, bail};
use codestory_contracts::api::{
    AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto, AgentAskRequest,
    AgentPacketRequestDto, AgentResponseModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, ApiError, GraphResponse, GroundingBudgetDto,
    IndexedFileRoleDto, IndexedFilesRequest, ListChildrenSymbolsRequest, ListRootSymbolsRequest,
    NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind, PacketBudgetModeDto, PacketTaskClassDto,
    ReadinessGoalDto, ReadinessStatusDto, ReadinessVerdictDto, SearchRepoTextMode, SearchRequest,
    TrailCallerScope, TrailDirection, TrailMode,
};
use codestory_retrieval::SidecarLayout;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration as StdDuration;

use crate::args;
use crate::http_transport::{
    BROWSER_SYMBOLS_DEFAULT_LIMIT, BROWSER_SYMBOLS_MAX_LIMIT, BROWSER_TRAIL_DEFAULT_DEPTH,
    BROWSER_TRAIL_MAX_DEPTH, browser_references_config, browser_trail_config,
};
use crate::output::context_packet_json;
use crate::runtime::{AmbiguousTargetError, RuntimeContext, map_api_error, resolve_target};
use crate::stdio_catalog::{
    is_tool_name as is_stdio_tool_name, prompt_get_json as stdio_prompt_get_json,
    prompts_list_json as stdio_prompts_list_json,
    resource_templates_list_json as stdio_resource_templates_list_json,
    resources_list_json as stdio_resources_list_json, tools_list_json as stdio_tools_list_json,
};
use crate::{
    build_ambiguous_target_error_output, build_query_resolution_output, build_search_hit_output,
};
use std::time::{Duration, Instant};

const STDIO_PACKET_CACHE_CAPACITY: usize = 8;
const STDIO_STATUS_CACHE_TTL: Duration = Duration::from_secs(5);
const STDIO_MAX_FRAME_BYTES: usize = 1024 * 1024;
const STDIO_CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(3);
const STDIO_CLI_VERSION_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Run the stdio server until stdin closes.
///
/// The server is local, stateful only for small packet/search caches, and keeps
/// telemetry on stderr so stdout remains a newline-delimited JSON stream.
pub(crate) fn run_stdio_server(runtime: RuntimeContext) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = std::io::stdout();
    let mut state = StdioServerState::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        let bytes_read = (&mut stdin)
            .take((STDIO_MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if bytes_read == 0 {
            break;
        }
        if line.len() > STDIO_MAX_FRAME_BYTES {
            let tail_bytes = if line.ends_with(b"\n") {
                0
            } else {
                discard_stdio_frame_tail(&mut stdin)?
            };
            let response = stdio_frame_too_large_error(line.len() + tail_bytes);
            write_stdio_response(&mut stdout, &response)?;
            continue;
        }
        let line = match std::str::from_utf8(&line) {
            Ok(line) => line.trim_end_matches(&['\r', '\n']),
            Err(error) => {
                let response = stdio_jsonrpc_error(
                    serde_json::Value::Null,
                    -32700,
                    format!("Parse error: {error}"),
                );
                write_stdio_response(&mut stdout, &response)?;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_stdio_message(&runtime, &mut state, line) {
            write_stdio_response(&mut stdout, &response)?;
        }
    }
    Ok(())
}

fn discard_stdio_frame_tail<R: BufRead>(reader: &mut R) -> Result<usize> {
    let mut discarded = 0;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(discarded);
        }
        if let Some(index) = available.iter().position(|byte| *byte == b'\n') {
            reader.consume(index + 1);
            return Ok(discarded + index + 1);
        }
        let len = available.len();
        reader.consume(len);
        discarded += len;
    }
}

fn stdio_frame_too_large_error(line_bytes: usize) -> serde_json::Value {
    let mut response = stdio_jsonrpc_error(
        serde_json::Value::Null,
        -32700,
        format!("Parse error: stdio frame exceeded {STDIO_MAX_FRAME_BYTES} byte limit"),
    );
    if let Some(error) = response
        .get_mut("error")
        .and_then(serde_json::Value::as_object_mut)
    {
        error.insert(
            "data".to_string(),
            serde_json::json!({
                "code": "stdio_frame_too_large",
                "max_frame_bytes": STDIO_MAX_FRAME_BYTES,
                "line_bytes": line_bytes,
            }),
        );
    }
    response
}

fn write_stdio_response<W: Write>(stdout: &mut W, response: &serde_json::Value) -> Result<()> {
    let response_id = stdio_response_id_label(response);
    let serialize_started = Instant::now();
    serde_json::to_writer(&mut *stdout, response)?;
    let serialization_ms = stdio_elapsed_ms(serialize_started);
    let newline_started = Instant::now();
    stdout.write_all(b"\n")?;
    let newline_write_ms = stdio_elapsed_ms(newline_started);
    let flush_started = Instant::now();
    stdout.flush()?;
    let flush_ms = stdio_elapsed_ms(flush_started);
    report_stdio_server_phase(&response_id, "response_serialization", serialization_ms);
    report_stdio_server_phase(&response_id, "newline_write", newline_write_ms);
    report_stdio_server_phase(&response_id, "flush", flush_ms);
    Ok(())
}

#[derive(Default)]
struct StdioServerState {
    packet_cache: StdioPacketCache,
    search_cache: StdioSearchFragmentCache,
    status_cache: Option<StdioStatusCacheEntry>,
}

#[derive(Clone)]
struct StdioStatusCacheEntry {
    key: String,
    value: serde_json::Value,
    cached_at: Instant,
}

fn handle_stdio_message(
    runtime: &RuntimeContext,
    state: &mut StdioServerState,
    line: &str,
) -> Option<serde_json::Value> {
    let request: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(stdio_jsonrpc_error(
                serde_json::Value::Null,
                -32700,
                format!("Parse error: {error}"),
            ));
        }
    };
    if !request.is_object() {
        return Some(stdio_jsonrpc_error(
            serde_json::Value::Null,
            -32600,
            "Invalid request: expected JSON-RPC object",
        ));
    }
    let id = request.get("id").cloned()?;
    let Some(method) = request.get("method").and_then(|value| value.as_str()) else {
        return Some(stdio_jsonrpc_error(
            id,
            -32600,
            "Invalid request: missing method",
        ));
    };
    let legacy_response = match method {
        "initialize" => {
            return Some(stdio_jsonrpc_success(
                id,
                stdio_initialize_result_json(&request),
            ));
        }
        "tools/list" => stdio_tools_list_json(),
        "resources/list" => stdio_resources_list_json(),
        "resources/templates/list" => stdio_resource_templates_list_json(),
        "prompts/list" => stdio_prompts_list_json(),
        "prompts/get" => {
            let Some(name) = request
                .pointer("/params/name")
                .and_then(|value| value.as_str())
            else {
                return Some(stdio_jsonrpc_error(
                    id,
                    -32602,
                    "Invalid params: missing prompt name",
                ));
            };
            match stdio_prompt_get_json(name) {
                Ok(response) => response,
                Err(error) => {
                    return Some(stdio_jsonrpc_error(id, -32602, error.to_string()));
                }
            }
        }
        "resources/read" => {
            let Some(uri) = request
                .pointer("/params/uri")
                .and_then(|value| value.as_str())
            else {
                return Some(stdio_jsonrpc_error(
                    id,
                    -32602,
                    "Invalid params: missing resource uri",
                ));
            };
            read_stdio_resource(runtime, state, uri)
        }
        "tools/call" => {
            let Some(name) = request
                .pointer("/params/name")
                .and_then(|value| value.as_str())
            else {
                return Some(stdio_jsonrpc_error(
                    id,
                    -32602,
                    "Invalid params: missing tool name",
                ));
            };
            if !is_stdio_tool_name(name) {
                return Some(stdio_jsonrpc_error(
                    id,
                    -32602,
                    format!("Unknown tool: {name}"),
                ));
            }
            if request
                .pointer("/params/arguments")
                .is_some_and(|value| !value.is_object() && !value.is_null())
            {
                return Some(stdio_jsonrpc_error(
                    id,
                    -32602,
                    "Invalid params: tool arguments must be an object",
                ));
            }
            match stdio_tool_blocked_error(runtime, state, name) {
                Ok(Some(error)) => {
                    return Some(stdio_jsonrpc_success(id, stdio_tool_call_error(&error)));
                }
                Ok(None) => {}
                Err(error) => {
                    let error = serde_json::json!({
                        "code": "readiness_unavailable",
                        "message": format!("Unable to evaluate CodeStory readiness before running `{name}`: {error}"),
                        "tool": name
                    });
                    return Some(stdio_jsonrpc_success(id, stdio_tool_call_error(&error)));
                }
            }
            return Some(stdio_jsonrpc_tool_call_from_legacy(
                id,
                handle_stdio_tool_call(runtime, state, &request),
            ));
        }
        _ => {
            return Some(stdio_jsonrpc_error(
                id,
                -32601,
                format!("Method not found: {method}"),
            ));
        }
    };
    Some(stdio_jsonrpc_from_legacy(id, legacy_response))
}

fn stdio_jsonrpc_success(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn stdio_jsonrpc_error(
    id: serde_json::Value,
    code: i32,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn stdio_jsonrpc_from_legacy(
    id: serde_json::Value,
    response: serde_json::Value,
) -> serde_json::Value {
    if let Some(result) = response.get("result") {
        return stdio_jsonrpc_success(id, result.clone());
    }
    if let Some(error) = response.get("error") {
        let message = stdio_legacy_error_message(error);
        let code = if message.contains("unknown resource") {
            -32602
        } else {
            -32000
        };
        let mut response = stdio_jsonrpc_error(id, code, message);
        if error.is_object()
            && let Some(error_object) = response.get_mut("error")
            && let Some(error_object) = error_object.as_object_mut()
        {
            error_object.insert("data".to_string(), error.clone());
        }
        return response;
    }
    stdio_jsonrpc_success(id, response)
}

fn stdio_tool_blocked_error(
    runtime: &RuntimeContext,
    state: &mut StdioServerState,
    name: &str,
) -> Result<Option<serde_json::Value>> {
    let status = read_stdio_status_resource_cached(runtime, state)?;
    let Some(surface) = status.pointer(&format!("/allowed_surfaces/{name}")) else {
        return Ok(None);
    };
    if surface
        .get("allowed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let readiness_goal = surface
        .get("readiness_goal")
        .and_then(serde_json::Value::as_str);
    let verdict = readiness_goal.and_then(|goal| {
        status
            .get("readiness")
            .and_then(serde_json::Value::as_array)
            .and_then(|verdicts| {
                verdicts.iter().find(|verdict| {
                    verdict.get("goal").and_then(serde_json::Value::as_str) == Some(goal)
                })
            })
    });
    let message = surface
        .get("blocked_reason")
        .and_then(serde_json::Value::as_str)
        .or_else(|| surface.get("summary").and_then(serde_json::Value::as_str))
        .unwrap_or("CodeStory readiness blocks this tool.");
    Ok(Some(serde_json::json!({
        "code": "codestory_tool_blocked",
        "message": format!("CodeStory tool `{name}` is blocked: {message}"),
        "tool": name,
        "readiness_goal": surface.get("readiness_goal").cloned().unwrap_or(serde_json::Value::Null),
        "status": surface.get("status").cloned().unwrap_or(serde_json::Value::Null),
        "failed_layer": surface.get("failed_layer").cloned().unwrap_or(serde_json::Value::Null),
        "repair_reason": surface.get("repair_reason").cloned().unwrap_or(serde_json::Value::Null),
        "minimum_next": surface.get("minimum_next").cloned().unwrap_or_else(|| serde_json::json!([])),
        "full_repair": surface.get("full_repair").cloned().unwrap_or_else(|| serde_json::json!([])),
        "setup": verdict.and_then(|verdict| verdict.get("setup")).cloned().unwrap_or(serde_json::Value::Null),
        "sidecar": verdict.and_then(|verdict| verdict.get("sidecar")).cloned().unwrap_or(serde_json::Value::Null),
    })))
}

fn stdio_jsonrpc_tool_call_from_legacy(
    id: serde_json::Value,
    response: serde_json::Value,
) -> serde_json::Value {
    if let Some(result) = response.get("result") {
        return stdio_jsonrpc_success(id, stdio_tool_call_success(result.clone()));
    }
    if let Some(error) = response.get("error") {
        return stdio_jsonrpc_success(id, stdio_tool_call_error(error));
    }
    stdio_jsonrpc_success(id, stdio_tool_call_success(response))
}

fn stdio_tool_call_success(structured_content: serde_json::Value) -> serde_json::Value {
    let is_packet = stdio_is_packet(&structured_content);
    let mut stdio_phases = Vec::new();
    let text_started = Instant::now();
    let text = stdio_tool_text(&structured_content);
    if is_packet {
        stdio_phases.push(stdio_packet_phase(
            "text_materialization",
            stdio_elapsed_ms(text_started),
        ));
    }

    let response_started = Instant::now();
    let mut response = serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ],
        "structuredContent": structured_content
    });
    if is_packet {
        stdio_phases.push(stdio_packet_phase(
            "tool_response_materialization",
            stdio_elapsed_ms(response_started),
        ));
        if let Some(response) = response.as_object_mut() {
            response.insert(
                "_meta".to_string(),
                serde_json::json!({ "codestory_stdio_phases": stdio_phases }),
            );
        }
    }
    response
}

fn stdio_tool_text(value: &serde_json::Value) -> String {
    if stdio_is_packet(value) {
        return stdio_packet_text(value);
    }
    stdio_json_text(value)
}

fn stdio_is_packet(value: &serde_json::Value) -> bool {
    value.get("packet_id").is_some() && value.get("answer").is_some()
}

fn stdio_packet_phase(label: &str, duration_ms: u32) -> serde_json::Value {
    serde_json::Value::String(format!(
        "packet_stdio_phase label={label} duration_ms={duration_ms}"
    ))
}

fn stdio_elapsed_ms(started_at: Instant) -> u32 {
    started_at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

fn stdio_response_id_label(response: &serde_json::Value) -> String {
    response
        .get("id")
        .map(stdio_json_text)
        .unwrap_or_else(|| "null".to_string())
}

fn report_stdio_server_phase(request_id: &str, label: &str, duration_ms: u32) {
    eprintln!(
        "packet_stdio_server_phase request_id={request_id} label={label} duration_ms={duration_ms}"
    );
}

fn stdio_packet_text(packet: &serde_json::Value) -> String {
    let mut text = String::new();
    append_packet_text_field(
        &mut text,
        "packet_id",
        packet.get("packet_id").and_then(|value| value.as_str()),
    );
    append_packet_text_field(
        &mut text,
        "question",
        packet.get("question").and_then(|value| value.as_str()),
    );
    append_packet_text_field(
        &mut text,
        "task_class",
        packet.get("task_class").and_then(|value| value.as_str()),
    );
    append_packet_text_field(
        &mut text,
        "sufficiency",
        packet
            .pointer("/sufficiency/status")
            .and_then(|value| value.as_str()),
    );
    append_packet_text_field(
        &mut text,
        "budget",
        packet
            .pointer("/budget/requested")
            .and_then(|value| value.as_str()),
    );
    append_packet_bool_field(
        &mut text,
        "truncated",
        packet
            .pointer("/budget/truncated")
            .and_then(|value| value.as_bool()),
    );
    if let Some(status) = packet
        .pointer("/sufficiency/status")
        .and_then(|value| value.as_str())
    {
        let unsafe_to_claim = if status == "sufficient" {
            "false"
        } else {
            "true - resolve gaps, open_next, or follow_up_commands before proof claims"
        };
        append_packet_text_field(&mut text, "unsafe_to_claim", Some(unsafe_to_claim));
    }
    append_packet_text_field(
        &mut text,
        "pagination",
        Some("structuredContent keeps full arrays; compact text lists first 8"),
    );

    for section in packet
        .pointer("/answer/sections")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
    {
        let id = section.get("id").and_then(|value| value.as_str());
        if !matches!(id, Some("packet-evidence-ledger" | "packet-flow-claims")) {
            continue;
        }
        if let Some(title) = section.get("title").and_then(|value| value.as_str()) {
            text.push('\n');
            text.push_str(title);
            text.push('\n');
        }
        for block in section
            .get("blocks")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(markdown) = block.get("markdown").and_then(|value| value.as_str()) {
                text.push_str(markdown);
                if !markdown.ends_with('\n') {
                    text.push('\n');
                }
            }
        }
    }

    append_packet_string_array(
        &mut text,
        "omitted_sections",
        packet.pointer("/budget/omitted_sections"),
        None,
    );
    append_packet_string_array(
        &mut text,
        "gaps",
        packet.pointer("/sufficiency/gaps"),
        Some("none"),
    );
    append_packet_string_array(
        &mut text,
        "open_next",
        packet.pointer("/sufficiency/open_next"),
        Some("none"),
    );
    append_packet_string_array(
        &mut text,
        "follow_up_commands",
        packet.pointer("/sufficiency/follow_up_commands"),
        Some("none"),
    );

    if text.trim().is_empty() {
        stdio_json_text(packet)
    } else {
        text
    }
}

fn append_packet_text_field(text: &mut String, label: &str, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    text.push_str(label);
    text.push_str(": ");
    text.push_str(value);
    text.push('\n');
}

fn append_packet_bool_field(text: &mut String, label: &str, value: Option<bool>) {
    let Some(value) = value else {
        return;
    };
    append_packet_text_field(text, label, Some(if value { "true" } else { "false" }));
}

fn append_packet_string_array(
    text: &mut String,
    title: &str,
    value: Option<&serde_json::Value>,
    empty_text: Option<&str>,
) {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return;
    };
    if items.is_empty() {
        if let Some(empty_text) = empty_text {
            text.push('\n');
            text.push_str(title);
            text.push_str(": ");
            text.push_str(empty_text);
            text.push('\n');
        }
        return;
    }
    text.push('\n');
    text.push_str(title);
    text.push_str(":\n");
    for item in items.iter().take(8) {
        if let Some(item) = item.as_str() {
            text.push_str("- ");
            text.push_str(item);
            text.push('\n');
        }
    }
}

fn stdio_tool_call_error(error: &serde_json::Value) -> serde_json::Value {
    let message = stdio_legacy_error_message(error);
    let structured_content = if error.is_object() {
        error.clone()
    } else {
        serde_json::json!({ "message": message.clone() })
    };
    serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": message
            }
        ],
        "structuredContent": structured_content,
        "isError": true
    })
}

fn stdio_legacy_error_message(error: &serde_json::Value) -> String {
    error
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            error
                .get("message")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| error.to_string())
}

fn stdio_json_text(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn stdio_initialize_result_json(request: &serde_json::Value) -> serde_json::Value {
    let protocol_version = request
        .pointer("/params/protocolVersion")
        .and_then(|value| value.as_str())
        .unwrap_or("2024-11-05");
    let version = env!("CARGO_PKG_VERSION");
    serde_json::json!({
        "protocolVersion": protocol_version,
        "name": "codestory",
        "version": version,
        "serverInfo": {
            "name": "codestory",
            "version": version
        },
        "capabilities": {
            "tools": {
                "listChanged": false
            },
            "resources": {
                "subscribe": false,
                "listChanged": false
            },
            "prompts": {
                "listChanged": false
            }
        }
    })
}

fn handle_stdio_tool_call(
    runtime: &RuntimeContext,
    state: &mut StdioServerState,
    request: &serde_json::Value,
) -> serde_json::Value {
    let name = request
        .pointer("/params/name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let query = request
        .pointer("/params/arguments/query")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    match name {
        "packet" => handle_stdio_packet(runtime, state, request),
        "search" => handle_stdio_search(runtime, state, request, query),
        "ground" => handle_stdio_ground(runtime, request),
        "files" => handle_stdio_files(runtime, request),
        "affected" => handle_stdio_affected(runtime, request),
        "symbol" => handle_stdio_symbol(runtime, request),
        "trail" => handle_stdio_trail(runtime, request, false),
        "callers" => handle_stdio_neighbors(
            runtime,
            request,
            "callers",
            1,
            50,
            Some(TrailDirection::Incoming),
        ),
        "callees" => handle_stdio_neighbors(
            runtime,
            request,
            "callees",
            1,
            50,
            Some(TrailDirection::Outgoing),
        ),
        "trace" => handle_stdio_trail(runtime, request, true),
        "get_node" => handle_stdio_get_node(runtime, request),
        "neighbors" => handle_stdio_neighbors(runtime, request, "neighbors", 1, 50, None),
        "shortest_path" => handle_stdio_shortest_path(runtime, request),
        "query_subgraph" => handle_stdio_neighbors(runtime, request, "query_subgraph", 2, 80, None),
        "definition" => handle_stdio_definition(runtime, request),
        "references" => handle_stdio_references(runtime, request),
        "symbols" => handle_stdio_symbols(runtime, request),
        "snippet" => handle_stdio_snippet(runtime, request),
        "context" => handle_stdio_context(runtime, request),
        _ => serde_json::json!({"error": "unknown tool"}),
    }
}

fn handle_stdio_ground(runtime: &RuntimeContext, request: &serde_json::Value) -> serde_json::Value {
    let budget = match stdio_grounding_budget(request) {
        Ok(budget) => budget,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    runtime
        .grounding
        .grounding_snapshot(budget)
        .map(|snapshot| serde_json::json!({"result": snapshot}))
        .unwrap_or_else(|error| serde_json::json!({"error": stdio_api_error_value(error)}))
}

fn handle_stdio_files(runtime: &RuntimeContext, request: &serde_json::Value) -> serde_json::Value {
    let role = match stdio_file_role(request) {
        Ok(role) => role,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    let limit = request
        .pointer("/params/arguments/limit")
        .and_then(|value| value.as_u64())
        .map(|value| value.clamp(1, 5000) as u32)
        .unwrap_or(500);
    runtime
        .browser
        .indexed_files(IndexedFilesRequest {
            path_contains: request
                .pointer("/params/arguments/path")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            language: request
                .pointer("/params/arguments/language")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            role,
            limit: Some(limit),
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": stdio_api_error_value(error)}))
}

fn handle_stdio_affected(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    let affected = match stdio_affected_request(request) {
        Ok(request) => request,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    runtime
        .browser
        .affected_analysis(affected)
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(|error| serde_json::json!({"error": stdio_api_error_value(error)}))
}

fn stdio_file_role(request: &serde_json::Value) -> Result<Option<IndexedFileRoleDto>> {
    let Some(role) = request
        .pointer("/params/arguments/role")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    match role {
        "source" => Ok(Some(IndexedFileRoleDto::Source)),
        "test" => Ok(Some(IndexedFileRoleDto::Test)),
        "generated" => Ok(Some(IndexedFileRoleDto::Generated)),
        "vendor" => Ok(Some(IndexedFileRoleDto::Vendor)),
        "unknown" => Ok(Some(IndexedFileRoleDto::Unknown)),
        _ => bail!("files.role must be one of source, test, generated, vendor, unknown"),
    }
}

fn stdio_affected_request(request: &serde_json::Value) -> Result<AffectedAnalysisRequest> {
    let changed_paths = stdio_affected_changed_paths(request)?;
    let change_records = stdio_affected_change_records(request)?;
    if changed_paths.is_empty() && change_records.is_empty() {
        bail!("affected.changed_paths or affected.change_records is required");
    }
    Ok(AffectedAnalysisRequest {
        changed_paths,
        change_records,
        depth: stdio_affected_depth(request)?,
        filter: stdio_affected_filter(request)?,
    })
}

fn stdio_affected_changed_paths(request: &serde_json::Value) -> Result<Vec<String>> {
    let Some(value) = request.pointer("/params/arguments/changed_paths") else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        bail!("affected.changed_paths must be an array of non-empty strings");
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .with_context(|| {
                    "affected.changed_paths must be an array of non-empty strings".to_string()
                })
        })
        .collect()
}

fn stdio_affected_change_records(
    request: &serde_json::Value,
) -> Result<Vec<AffectedChangeRecordDto>> {
    let Some(value) = request.pointer("/params/arguments/change_records") else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        bail!("affected.change_records must be an array of objects");
    };
    values.iter().map(stdio_affected_change_record).collect()
}

fn stdio_affected_change_record(value: &serde_json::Value) -> Result<AffectedChangeRecordDto> {
    let object = value
        .as_object()
        .context("affected.change_records entries must be objects")?;
    let path = object
        .get("path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .context("affected.change_records[].path must be a non-empty string")?;
    let kind_value = object
        .get("kind")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .context("affected.change_records[].kind is required")?;
    let kind = stdio_affected_change_kind(kind_value)?;
    let status = object
        .get("status")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| stdio_affected_default_status(&kind).to_string());
    let previous_path = match object.get("previous_path") {
        Some(value) if !value.is_null() => Some(
            value
                .as_str()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .context("affected.change_records[].previous_path must be a non-empty string")?
                .to_string(),
        ),
        _ => None,
    };
    Ok(AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status,
        previous_path,
    })
}

fn stdio_affected_change_kind(value: &str) -> Result<AffectedChangeKindDto> {
    match value {
        "added" => Ok(AffectedChangeKindDto::Added),
        "modified" => Ok(AffectedChangeKindDto::Modified),
        "deleted" => Ok(AffectedChangeKindDto::Deleted),
        "renamed" => Ok(AffectedChangeKindDto::Renamed),
        "copied" => Ok(AffectedChangeKindDto::Copied),
        "untracked" => Ok(AffectedChangeKindDto::Untracked),
        "unknown" => Ok(AffectedChangeKindDto::Unknown),
        value => bail!(
            "affected.change_records[].kind must be one of added, modified, deleted, renamed, copied, untracked, or unknown; got {value}"
        ),
    }
}

fn stdio_affected_default_status(kind: &AffectedChangeKindDto) -> &'static str {
    match kind {
        AffectedChangeKindDto::Added => "A",
        AffectedChangeKindDto::Modified => "M",
        AffectedChangeKindDto::Deleted => "D",
        AffectedChangeKindDto::Renamed => "R",
        AffectedChangeKindDto::Copied => "C",
        AffectedChangeKindDto::Untracked => "??",
        AffectedChangeKindDto::Unknown => "path",
    }
}

fn stdio_affected_depth(request: &serde_json::Value) -> Result<Option<u32>> {
    let Some(value) = request.pointer("/params/arguments/depth") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(depth) = value.as_u64() else {
        bail!("affected.depth must be an integer between 1 and 8");
    };
    if !(1..=8).contains(&depth) {
        bail!("affected.depth must be between 1 and 8");
    }
    Ok(Some(depth as u32))
}

fn stdio_affected_filter(request: &serde_json::Value) -> Result<Option<String>> {
    let Some(value) = request.pointer("/params/arguments/filter") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .map(Some)
        .context("affected.filter must be a string")
}

fn handle_stdio_packet(
    runtime: &RuntimeContext,
    state: &mut StdioServerState,
    request: &serde_json::Value,
) -> serde_json::Value {
    let Some(question) = request
        .pointer("/params/arguments/question")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return serde_json::json!({"error": "packet.question is required"});
    };
    let budget = match stdio_packet_budget(request) {
        Ok(budget) => budget,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    let task_class = match stdio_packet_task_class(request) {
        Ok(task_class) => task_class,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    let latency_budget_ms = match stdio_packet_latency_budget(request) {
        Ok(latency_budget_ms) => latency_budget_ms,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    let extra_probes = match stdio_packet_extra_probes(request) {
        Ok(extra_probes) => extra_probes,
        Err(error) => return serde_json::json!({"error": error.to_string()}),
    };
    let include_evidence = request
        .pointer("/params/arguments/include_evidence")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let cache_key = stdio_packet_cache_key(StdioPacketCacheKeyInput {
        storage_fingerprint: stdio_storage_fingerprint(&runtime.storage_path),
        sidecar_fingerprint: stdio_mandatory_sidecar_fingerprint(
            &runtime.project_root,
            &runtime.storage_path,
        ),
        question,
        budget,
        task_class,
        extra_probes: &extra_probes,
        include_evidence,
        latency_budget_ms,
    });
    if let Some(cached) = state.packet_cache.get(&cache_key) {
        return cached;
    }
    let response = runtime
        .browser
        .packet(AgentPacketRequestDto {
            question: question.to_string(),
            budget,
            task_class,
            extra_probes,
            include_evidence,
            latency_budget_ms,
        })
        .map(|packet| serde_json::json!({"result": packet}))
        .unwrap_or_else(|error| serde_json::json!({"error": stdio_api_error_value(error)}));
    if response.get("result").is_some() {
        state.packet_cache.insert(cache_key, response.clone());
    }
    response
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StdioPacketCacheKey {
    storage_fingerprint: String,
    sidecar_fingerprint: String,
    question: String,
    budget: &'static str,
    task_class: Option<&'static str>,
    extra_probes: Vec<String>,
    include_evidence: bool,
    latency_budget_ms: Option<u32>,
}

struct StdioLruCache<K> {
    entries: std::collections::VecDeque<(K, serde_json::Value)>,
    capacity: usize,
}

impl<K: Clone + PartialEq> StdioLruCache<K> {
    fn new(capacity: usize) -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            capacity,
        }
    }

    fn get(&mut self, key: &K) -> Option<serde_json::Value> {
        let position = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == key)?;
        let entry = self.entries.remove(position)?;
        let value = entry.1.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    fn insert(&mut self, key: K, value: serde_json::Value) {
        if let Some(position) = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == &key)
        {
            self.entries.remove(position);
        }
        self.entries.push_back((key, value));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }
}

struct StdioPacketCache {
    lru: StdioLruCache<StdioPacketCacheKey>,
}

impl Default for StdioPacketCache {
    fn default() -> Self {
        Self {
            lru: StdioLruCache::new(STDIO_PACKET_CACHE_CAPACITY),
        }
    }
}

impl StdioPacketCache {
    fn get(&mut self, key: &StdioPacketCacheKey) -> Option<serde_json::Value> {
        self.lru.get(key)
    }

    fn insert(&mut self, key: StdioPacketCacheKey, value: serde_json::Value) {
        self.lru.insert(key, value);
    }
}

struct StdioPacketCacheKeyInput<'a> {
    storage_fingerprint: String,
    sidecar_fingerprint: String,
    question: &'a str,
    budget: PacketBudgetModeDto,
    task_class: Option<PacketTaskClassDto>,
    extra_probes: &'a [String],
    include_evidence: bool,
    latency_budget_ms: Option<u32>,
}

fn stdio_packet_cache_key(input: StdioPacketCacheKeyInput<'_>) -> StdioPacketCacheKey {
    StdioPacketCacheKey {
        storage_fingerprint: input.storage_fingerprint,
        sidecar_fingerprint: input.sidecar_fingerprint,
        question: input.question.to_string(),
        budget: stdio_packet_budget_label(input.budget),
        task_class: input.task_class.map(stdio_packet_task_class_label),
        extra_probes: input.extra_probes.to_vec(),
        include_evidence: input.include_evidence,
        latency_budget_ms: input.latency_budget_ms,
    }
}

fn stdio_packet_budget_label(budget: PacketBudgetModeDto) -> &'static str {
    match budget {
        PacketBudgetModeDto::Tiny => "tiny",
        PacketBudgetModeDto::Compact => "compact",
        PacketBudgetModeDto::Standard => "standard",
        PacketBudgetModeDto::Deep => "deep",
    }
}

fn stdio_packet_task_class_label(task_class: PacketTaskClassDto) -> &'static str {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => "architecture_explanation",
        PacketTaskClassDto::BugLocalization => "bug_localization",
        PacketTaskClassDto::ChangeImpact => "change_impact",
        PacketTaskClassDto::RouteTracing => "route_tracing",
        PacketTaskClassDto::SymbolOwnership => "symbol_ownership",
        PacketTaskClassDto::DataFlow => "data_flow",
        PacketTaskClassDto::EditPlanning => "edit_planning",
    }
}

fn stdio_storage_fingerprint(storage_path: &std::path::Path) -> String {
    let mut parts = vec![stdio_path_fingerprint(storage_path)];
    parts.push(stdio_path_fingerprint(
        &storage_path.with_extension("db-wal"),
    ));
    parts.push(stdio_path_fingerprint(
        &storage_path.with_extension("db-shm"),
    ));
    parts.join("|")
}

fn stdio_mandatory_sidecar_fingerprint(
    project_root: &std::path::Path,
    storage_path: &std::path::Path,
) -> String {
    let layout = SidecarLayout::from_env_for_project(project_root);
    let status = codestory_retrieval::strict_sidecar_status(project_root, Some(storage_path)).map(
        |report| StdioSidecarStatusFingerprint {
            retrieval_mode: report.retrieval_mode,
            degraded_reason: report.degraded_reason,
            manifest: report.manifest,
        },
    );
    stdio_mandatory_sidecar_fingerprint_from_status(
        codestory_retrieval::embedding_runtime_id(),
        stdio_path_fingerprint(&layout.state_file),
        status,
    )
}

struct StdioSidecarStatusFingerprint {
    retrieval_mode: String,
    degraded_reason: Option<String>,
    manifest: Option<codestory_retrieval::RetrievalIndexManifest>,
}

fn stdio_mandatory_sidecar_fingerprint_from_status(
    active_embedding_backend: impl AsRef<str>,
    sidecar_state_fingerprint: impl AsRef<str>,
    status: std::result::Result<StdioSidecarStatusFingerprint, anyhow::Error>,
) -> String {
    let mut parts = vec![
        format!(
            "active_embedding_backend:{}",
            active_embedding_backend.as_ref()
        ),
        format!("sidecar_state:{}", sidecar_state_fingerprint.as_ref()),
    ];

    match status {
        Ok(report) => {
            parts.push(format!("retrieval_mode:{}", report.retrieval_mode));
            parts.push(format!(
                "degraded_reason:{}",
                report.degraded_reason.unwrap_or_default()
            ));
            if let Some(manifest) = report.manifest {
                parts.push(format!(
                    "manifest_generation:{}",
                    manifest.sidecar_generation.unwrap_or_default()
                ));
                parts.push(format!(
                    "manifest_input_hash:{}",
                    manifest.sidecar_input_hash.unwrap_or_default()
                ));
                parts.push(format!(
                    "manifest_embedding_backend:{}",
                    manifest.embedding_backend.unwrap_or_default()
                ));
                parts.push(format!(
                    "manifest_embedding_dim:{}",
                    manifest
                        .embedding_dim
                        .map(|value| value.to_string())
                        .unwrap_or_default()
                ));
            } else {
                parts.push("manifest:<missing>".to_string());
            }
        }
        Err(error) => {
            parts.push(format!("status_error:{error}"));
        }
    }

    parts.join("|")
}

fn stdio_path_fingerprint(path: &std::path::Path) -> String {
    let Ok(metadata) = std::fs::metadata(path) else {
        return "missing".to_string();
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("len:{}:mtime_ms:{}", metadata.len(), modified_ms)
}

fn stdio_grounding_budget(request: &serde_json::Value) -> Result<GroundingBudgetDto> {
    match request
        .pointer("/params/arguments/budget")
        .and_then(|value| value.as_str())
        .unwrap_or("balanced")
    {
        "strict" => Ok(GroundingBudgetDto::Strict),
        "balanced" => Ok(GroundingBudgetDto::Balanced),
        "max" => Ok(GroundingBudgetDto::Max),
        value => bail!("ground.budget must be one of strict, balanced, or max; got {value}"),
    }
}

fn stdio_packet_budget(request: &serde_json::Value) -> Result<PacketBudgetModeDto> {
    match request
        .pointer("/params/arguments/budget")
        .and_then(|value| value.as_str())
        .unwrap_or("compact")
    {
        "tiny" => Ok(PacketBudgetModeDto::Tiny),
        "compact" => Ok(PacketBudgetModeDto::Compact),
        "standard" => Ok(PacketBudgetModeDto::Standard),
        "deep" => Ok(PacketBudgetModeDto::Deep),
        value => {
            bail!("packet.budget must be one of tiny, compact, standard, or deep; got {value}")
        }
    }
}

fn stdio_packet_task_class(request: &serde_json::Value) -> Result<Option<PacketTaskClassDto>> {
    let Some(task_class) = request
        .pointer("/params/arguments/task_class")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let task_class = match task_class {
        "architecture_explanation" => PacketTaskClassDto::ArchitectureExplanation,
        "bug_localization" => PacketTaskClassDto::BugLocalization,
        "change_impact" => PacketTaskClassDto::ChangeImpact,
        "route_tracing" => PacketTaskClassDto::RouteTracing,
        "symbol_ownership" => PacketTaskClassDto::SymbolOwnership,
        "data_flow" => PacketTaskClassDto::DataFlow,
        "edit_planning" => PacketTaskClassDto::EditPlanning,
        value => bail!(
            "packet.task_class must be one of architecture_explanation, bug_localization, change_impact, route_tracing, symbol_ownership, data_flow, or edit_planning; got {value}"
        ),
    };
    Ok(Some(task_class))
}

fn stdio_packet_latency_budget(request: &serde_json::Value) -> Result<Option<u32>> {
    let Some(value) = request.pointer("/params/arguments/latency_budget_ms") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = value.as_u64() else {
        bail!("packet.latency_budget_ms must be an integer");
    };
    if !(1_000..=120_000).contains(&value) {
        bail!("packet.latency_budget_ms must be between 1000 and 120000");
    }
    Ok(Some(value as u32))
}

fn stdio_packet_extra_probes(request: &serde_json::Value) -> Result<Vec<String>> {
    let Some(value) = request.pointer("/params/arguments/extra_probes") else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        bail!("packet.extra_probes must be an array of strings");
    };
    if values.len() > 16 {
        bail!("packet.extra_probes accepts at most 16 probes");
    }

    let mut probes = Vec::new();
    for value in values {
        let Some(probe) = value.as_str() else {
            bail!("packet.extra_probes must be an array of strings");
        };
        let probe = probe.trim();
        if probe.is_empty() {
            continue;
        }
        if probe.len() > 240 {
            bail!("packet.extra_probes entries must be at most 240 characters");
        }
        if !probes
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(probe))
        {
            probes.push(probe.to_string());
        }
    }
    Ok(probes)
}

fn handle_stdio_search(
    runtime: &RuntimeContext,
    state: &mut StdioServerState,
    request: &serde_json::Value,
    query: String,
) -> serde_json::Value {
    let repo_text = match request
        .pointer("/params/arguments/repo_text")
        .and_then(|value| value.as_str())
    {
        Some("on") => SearchRepoTextMode::On,
        Some("off") => SearchRepoTextMode::Off,
        _ => SearchRepoTextMode::Auto,
    };
    let limit_per_source = request
        .pointer("/params/arguments/limit")
        .and_then(|value| value.as_u64())
        .map(|value| value.clamp(1, 50) as u32)
        .unwrap_or(10);
    let cache_key = StdioSearchFragmentCacheKey {
        storage_fingerprint: stdio_storage_fingerprint(&runtime.storage_path),
        sidecar_fingerprint: stdio_mandatory_sidecar_fingerprint(
            &runtime.project_root,
            &runtime.storage_path,
        ),
        query: query.trim().to_ascii_lowercase(),
        repo_text: match repo_text {
            SearchRepoTextMode::On => "on",
            SearchRepoTextMode::Off => "off",
            SearchRepoTextMode::Auto => "auto",
        }
        .to_string(),
        limit_per_source,
    };
    if let Some(cached) = state.search_cache.get(&cache_key) {
        return cached;
    }
    let response = runtime
        .browser
        .search_results(SearchRequest {
            query: query.clone(),
            repo_text,
            limit_per_source,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .map(|result| serde_json::json!({"result": enrich_stdio_search_result(result)}))
        .unwrap_or_else(|error| serde_json::json!({"error": stdio_api_error_value(error)}));
    if response.get("result").is_some() {
        state.search_cache.insert(cache_key, response.clone());
    }
    response
}

const STDIO_SEARCH_FRAGMENT_CACHE_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
struct StdioSearchFragmentCacheKey {
    storage_fingerprint: String,
    sidecar_fingerprint: String,
    query: String,
    repo_text: String,
    limit_per_source: u32,
}

struct StdioSearchFragmentCache {
    lru: StdioLruCache<StdioSearchFragmentCacheKey>,
}

impl Default for StdioSearchFragmentCache {
    fn default() -> Self {
        Self {
            lru: StdioLruCache::new(STDIO_SEARCH_FRAGMENT_CACHE_CAPACITY),
        }
    }
}

impl StdioSearchFragmentCache {
    fn get(&mut self, key: &StdioSearchFragmentCacheKey) -> Option<serde_json::Value> {
        self.lru.get(key)
    }

    fn insert(&mut self, key: StdioSearchFragmentCacheKey, value: serde_json::Value) {
        self.lru.insert(key, value);
    }
}

fn handle_stdio_symbol(runtime: &RuntimeContext, request: &serde_json::Value) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .browser
                .symbol_context(target.selected.node_id)
                .map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(
            |error| serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)}),
        )
}

fn handle_stdio_trail(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
    default_story: bool,
) -> serde_json::Value {
    let direction = match request
        .pointer("/params/arguments/direction")
        .and_then(|value| value.as_str())
    {
        Some("incoming") => TrailDirection::Incoming,
        Some("outgoing") => TrailDirection::Outgoing,
        _ => TrailDirection::Both,
    };
    let depth = request
        .pointer("/params/arguments/depth")
        .and_then(|value| value.as_u64())
        .map(|value| value.min(BROWSER_TRAIL_MAX_DEPTH as u64) as u32)
        .unwrap_or(BROWSER_TRAIL_DEFAULT_DEPTH);
    let max_nodes = stdio_graph_u32_arg(request, "max_nodes", 120, 1, 120);
    let story = request
        .pointer("/params/arguments/story")
        .and_then(|value| value.as_bool())
        .unwrap_or(default_story);
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            let mut config = browser_trail_config(target.selected.node_id, depth, direction, story);
            config.max_nodes = max_nodes;
            runtime.browser.trail_context(config).map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(
            |error| serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)}),
        )
}

fn handle_stdio_definition(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .browser
                .definition_context(target.selected.node_id.clone())
                .map_err(map_api_error)
                .map(|symbol| {
                    let node_id = target.selected.node_id.0.clone();
                    let links = stdio_node_links(&node_id);
                    let mut definition = serde_json::to_value(build_search_hit_output(
                        &runtime.project_root,
                        &target.selected,
                        Some(&target.requested),
                        false,
                        &[],
                    ))
                    .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}));
                    add_stdio_links(&mut definition, links.clone());
                    serde_json::json!({
                        "resolution": build_query_resolution_output(&runtime.project_root, &target),
                        "definition": definition,
                        "symbol": symbol,
                        "links": links,
                    })
                })
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(
            |error| serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)}),
        )
}

fn handle_stdio_get_node(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .browser
                .node_details(NodeDetailsRequest {
                    id: target.selected.node_id.clone(),
                })
                .map_err(map_api_error)
                .map(|node| {
                    let file_refs = stdio_node_file_refs(&node);
                    let resolution = serde_json::to_value(build_query_resolution_output(
                        &runtime.project_root,
                        &target,
                    ))
                    .unwrap_or(serde_json::Value::Null);
                    serde_json::json!({
                        "resolution": resolution,
                        "node": node,
                        "certainty": "certain",
                        "file_refs": file_refs,
                        "limits": {
                            "max_nodes": 1,
                            "max_edges": 0
                        },
                        "node_count": 1,
                        "edge_count": 0,
                        "truncated": false
                    })
                })
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(
            |error| serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)}),
        )
}

fn handle_stdio_neighbors(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
    tool_name: &str,
    default_depth: u32,
    default_max_nodes: u32,
    fixed_direction: Option<TrailDirection>,
) -> serde_json::Value {
    let direction = fixed_direction.unwrap_or_else(|| stdio_graph_direction(request));
    let depth = stdio_graph_u32_arg(request, "depth", default_depth, 0, 3);
    let max_nodes = stdio_graph_u32_arg(request, "max_nodes", default_max_nodes, 1, 120);
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            let mut config =
                browser_trail_config(target.selected.node_id.clone(), depth, direction, false);
            config.max_nodes = max_nodes;
            runtime
                .browser
                .trail_context(config)
                .map_err(map_api_error)
                .map(|context| {
                    let resolution = serde_json::to_value(build_query_resolution_output(
                        &runtime.project_root,
                        &target,
                    ))
                    .unwrap_or(serde_json::Value::Null);
                    stdio_graph_tool_output(
                        resolution,
                        context.trail,
                        serde_json::json!({
                            "tool": tool_name,
                            "direction": stdio_graph_direction_label(direction),
                            "depth": depth,
                            "max_nodes": max_nodes,
                            "max_edges": max_nodes.saturating_mul(3).max(128)
                        }),
                    )
                })
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(
            |error| serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)}),
        )
}

fn handle_stdio_shortest_path(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    let Some(from_id) = stdio_graph_string_arg(request, "from_id") else {
        return serde_json::json!({"error": "shortest_path.from_id is required"});
    };
    let Some(to_id) = stdio_graph_string_arg(request, "to_id") else {
        return serde_json::json!({"error": "shortest_path.to_id is required"});
    };
    let max_depth = stdio_graph_u32_arg(request, "max_depth", 6, 1, 10);
    let max_nodes = stdio_graph_u32_arg(request, "max_nodes", 80, 2, 120);
    let from = NodeId(from_id.to_string());
    let to = NodeId(to_id.to_string());
    if let Err(error) = runtime
        .browser
        .node_details(NodeDetailsRequest { id: from.clone() })
    {
        return serde_json::json!({"error": stdio_api_error_value(error)});
    }
    if let Err(error) = runtime
        .browser
        .node_details(NodeDetailsRequest { id: to.clone() })
    {
        return serde_json::json!({"error": stdio_api_error_value(error)});
    }
    runtime
        .browser
        .trail_context(codestory_contracts::api::TrailConfigDto {
            root_id: from.clone(),
            mode: TrailMode::ToTargetSymbol,
            target_id: Some(to.clone()),
            depth: max_depth,
            direction: TrailDirection::Outgoing,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: false,
            story: false,
            node_filter: Vec::new(),
            max_nodes,
            layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
        })
        .map(|context| {
            let mut output = stdio_graph_tool_output(
                serde_json::Value::Null,
                context.trail,
                serde_json::json!({
                    "tool": "shortest_path",
                    "direction": "outgoing",
                    "max_depth": max_depth,
                    "max_nodes": max_nodes,
                    "max_edges": max_nodes.saturating_mul(3).max(128)
                }),
            );
            if let Some(object) = output.as_object_mut() {
                object.insert("from_id".to_string(), serde_json::json!(from.0.as_str()));
                object.insert("to_id".to_string(), serde_json::json!(to.0.as_str()));
            }
            serde_json::json!({"result": output})
        })
        .unwrap_or_else(|error| serde_json::json!({"error": stdio_api_error_value(error)}))
}

fn handle_stdio_references(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .browser
                .references_context(browser_references_config(target.selected.node_id.clone()))
                .map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(
            |error| serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)}),
        )
}

fn handle_stdio_symbols(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    let limit = request
        .pointer("/params/arguments/limit")
        .and_then(|value| value.as_u64())
        .map(|value| value.clamp(1, BROWSER_SYMBOLS_MAX_LIMIT as u64) as u32)
        .or(Some(BROWSER_SYMBOLS_DEFAULT_LIMIT));
    let parent_id = request
        .pointer("/params/arguments/parent_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty());
    let result = if let Some(parent_id) = parent_id {
        runtime
            .browser
            .list_children_symbols(ListChildrenSymbolsRequest {
                parent_id: NodeId(parent_id.to_string()),
            })
            .map(|symbols| {
                serde_json::to_value(symbols)
                    .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
            })
    } else {
        runtime
            .browser
            .list_root_symbols(ListRootSymbolsRequest { limit })
            .map(|symbols| {
                serde_json::to_value(symbols)
                    .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
            })
    };
    result
        .map(|value| serde_json::json!({"result": {"symbols": value}}))
        .unwrap_or_else(|error| serde_json::json!({"error": map_api_error(error).to_string()}))
}

fn handle_stdio_snippet(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .browser
                .snippet_context(target.selected.node_id, 4)
                .map_err(map_api_error)
        })
        .map(|result| serde_json::json!({"result": result}))
        .unwrap_or_else(
            |error| serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)}),
        )
}

fn handle_stdio_context(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> serde_json::Value {
    let (target_label, focus_node_id) = match stdio_context_target(runtime, request) {
        Ok(target) => target,
        Err(error) => {
            return serde_json::json!({"error": stdio_legacy_error_value(runtime, &error)});
        }
    };
    let max_results = request
        .pointer("/params/arguments/max_results")
        .and_then(|value| value.as_u64())
        .map(|value| value.clamp(1, 50) as u32)
        .unwrap_or(8);
    let include_evidence = request
        .pointer("/params/arguments/include_evidence")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    runtime
        .browser
        .ask(AgentAskRequest {
            prompt: target_label.clone(),
            retrieval_profile: AgentRetrievalProfileSelectionDto::Preset {
                preset: AgentRetrievalPresetDto::Investigate,
            },
            focus_node_id: Some(focus_node_id.clone()),
            max_results: Some(max_results),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: None,
            include_evidence,
            hybrid_weights: None,
        })
        .map(|mut result| {
            result.retrieval_trace.annotations.push(format!(
                "context_target node={} label=`{}`",
                focus_node_id.0,
                target_label.replace('`', "'")
            ));
            serde_json::json!({"result": context_packet_json(&result)})
        })
        .unwrap_or_else(|error| serde_json::json!({"error": stdio_api_error_value(error)}))
}

fn stdio_graph_direction(request: &serde_json::Value) -> TrailDirection {
    match request
        .pointer("/params/arguments/direction")
        .and_then(|value| value.as_str())
    {
        Some("incoming") => TrailDirection::Incoming,
        Some("outgoing") => TrailDirection::Outgoing,
        _ => TrailDirection::Both,
    }
}

fn stdio_graph_direction_label(direction: TrailDirection) -> &'static str {
    match direction {
        TrailDirection::Incoming => "incoming",
        TrailDirection::Outgoing => "outgoing",
        TrailDirection::Both => "both",
    }
}

fn stdio_graph_u32_arg(
    request: &serde_json::Value,
    name: &str,
    default: u32,
    min: u32,
    max: u32,
) -> u32 {
    request
        .pointer(&format!("/params/arguments/{name}"))
        .and_then(|value| value.as_u64())
        .map(|value| value.clamp(min as u64, max as u64) as u32)
        .unwrap_or(default)
}

fn stdio_graph_string_arg<'a>(request: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    request
        .pointer(&format!("/params/arguments/{name}"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn stdio_graph_tool_output(
    resolution: serde_json::Value,
    graph: GraphResponse,
    limits: serde_json::Value,
) -> serde_json::Value {
    let file_refs = stdio_graph_file_refs(&graph);
    let node_count = graph.nodes.len();
    let edge_count = graph.edges.len();
    let truncated = graph.truncated;
    serde_json::json!({
        "resolution": resolution,
        "graph": graph,
        "certainty": "mixed; inspect graph.edges[].certainty and graph.edges[].confidence",
        "file_refs": file_refs,
        "limits": limits,
        "node_count": node_count,
        "edge_count": edge_count,
        "truncated": truncated,
    })
}

fn stdio_node_file_refs(node: &NodeDetailsDto) -> serde_json::Value {
    match node.file_path.as_deref() {
        Some(path) => serde_json::json!([
            {
                "node_id": node.id.0.as_str(),
                "file_path": path,
                "line": node.start_line
            }
        ]),
        None => serde_json::json!([]),
    }
}

fn stdio_graph_file_refs(graph: &GraphResponse) -> serde_json::Value {
    let mut seen = std::collections::HashSet::<(&str, Option<u32>)>::new();
    let refs = graph
        .nodes
        .iter()
        .filter_map(|node| {
            let path = node.file_path.as_deref()?;
            if !seen.insert((path, None)) {
                return None;
            }
            Some(serde_json::json!({
                "node_id": node.id.0.as_str(),
                "file_path": path,
                "line": null
            }))
        })
        .collect::<Vec<_>>();
    serde_json::json!(refs)
}

fn stdio_api_error_value(error: ApiError) -> serde_json::Value {
    serde_json::to_value(error.clone())
        .unwrap_or_else(|_| serde_json::json!({"message": map_api_error(error).to_string()}))
}

fn stdio_context_target(
    runtime: &RuntimeContext,
    request: &serde_json::Value,
) -> Result<(String, NodeId)> {
    let has_id = request
        .pointer("/params/arguments/id")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty());
    let has_query = request
        .pointer("/params/arguments/query")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty());
    let bookmark = request
        .pointer("/params/arguments/bookmark")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let selector_count =
        usize::from(has_id) + usize::from(has_query) + usize::from(bookmark.is_some());
    if selector_count != 1 {
        bail!("Pass exactly one of id, query, or bookmark for context.");
    }
    if let Some(bookmark_id) = bookmark {
        let bookmark = runtime
            .bookmarks
            .list_bookmarks(None)
            .map_err(map_api_error)?
            .into_iter()
            .find(|bookmark| bookmark.id == bookmark_id)
            .with_context(|| format!("Bookmark not found: {bookmark_id}"))?;
        if bookmark.node_kind == NodeKind::UNKNOWN {
            bail!(
                "Bookmark {bookmark_id} is stale: node {} is no longer present after reindex.",
                bookmark.node_id.0
            );
        }
        return Ok((bookmark.node_label, bookmark.node_id));
    }
    resolve_target(runtime, stdio_target_selection(request), None).map(|target| {
        (
            target.selected.display_name.clone(),
            target.selected.node_id.clone(),
        )
    })
}

fn stdio_target_selection(request: &serde_json::Value) -> args::TargetSelection {
    if let Some(id) = request
        .pointer("/params/arguments/id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    {
        return args::TargetSelection::Id(NodeId(id.trim().to_string()));
    }
    let query = request
        .pointer("/params/arguments/query")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    args::TargetSelection::Query {
        query,
        choose: request
            .pointer("/params/arguments/choose")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize),
    }
}

fn stdio_legacy_error_value(runtime: &RuntimeContext, error: &anyhow::Error) -> serde_json::Value {
    if let Some(ambiguous) = error.downcast_ref::<AmbiguousTargetError>() {
        return serde_json::to_value(build_ambiguous_target_error_output(
            &runtime.project_root,
            ambiguous,
        ))
        .ok()
        .and_then(|value| value.get("error").cloned())
        .unwrap_or_else(|| serde_json::json!(ambiguous.to_string()));
    }

    serde_json::json!(error.to_string())
}

fn read_stdio_resource(
    runtime: &RuntimeContext,
    state: &mut StdioServerState,
    uri: &str,
) -> serde_json::Value {
    let result = match uri {
        "codestory://status" => read_stdio_status_resource_cached(runtime, state),
        "codestory://agent-guide" => Ok(read_stdio_agent_guide_resource()),
        "codestory://project" => runtime
            .open_project_summary()
            .map(|summary| serde_json::json!(summary)),
        "codestory://grounding" => runtime
            .grounding
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .map(|snapshot| serde_json::json!(snapshot))
            .map_err(map_api_error),
        "codestory://symbols/root" => runtime
            .browser
            .list_root_symbols(ListRootSymbolsRequest {
                limit: Some(BROWSER_SYMBOLS_DEFAULT_LIMIT),
            })
            .map(|symbols| serde_json::json!(symbols))
            .map_err(map_api_error),
        _ => read_stdio_template_resource(runtime, uri),
    };
    result
        .map(|value| serde_json::json!({"result": {"contents": [{"uri": uri, "mimeType": "application/json", "text": value.to_string()}]}}))
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
}

fn read_stdio_status_resource_cached(
    runtime: &RuntimeContext,
    state: &mut StdioServerState,
) -> Result<serde_json::Value> {
    let key = stdio_status_cache_key(runtime);
    if let Some(cached) = state.status_cache.as_ref()
        && cached.key == key
        && cached.cached_at.elapsed() < STDIO_STATUS_CACHE_TTL
    {
        return Ok(cached.value.clone());
    }

    let value = read_stdio_status_resource(runtime)?;
    // ponytail: short stdio snapshot cache; storage/sidecar fingerprints bust it when runtime state changes.
    state.status_cache = Some(StdioStatusCacheEntry {
        key,
        value: value.clone(),
        cached_at: Instant::now(),
    });
    Ok(value)
}

fn stdio_status_cache_key(runtime: &RuntimeContext) -> String {
    let layout = SidecarLayout::from_env_for_project(&runtime.project_root);
    [
        format!("project:{}", runtime.project_root.display()),
        format!("storage:{}", runtime.storage_path.display()),
        format!(
            "storage_state:{}",
            stdio_storage_fingerprint(&runtime.storage_path)
        ),
        format!(
            "sidecar_state:{}",
            stdio_path_fingerprint(&layout.state_file)
        ),
        format!(
            "active_embedding_backend:{}",
            codestory_retrieval::embedding_runtime_id()
        ),
    ]
    .join("|")
}

fn read_stdio_status_resource(runtime: &RuntimeContext) -> Result<serde_json::Value> {
    let summary = runtime.open_project_summary()?;
    let retrieval = summary.retrieval.as_ref();
    let sidecar_runtime = codestory_retrieval::sidecar_runtime_auto(&runtime.project_root);
    let (sidecar_mode, degraded_reason, manifest_generation, manifest_input_hash, ownership) =
        match codestory_retrieval::strict_sidecar_status_for_runtime(
            &runtime.project_root,
            Some(&runtime.storage_path),
            sidecar_runtime.clone(),
        ) {
            Ok(report) => {
                let manifest_generation = report
                    .manifest
                    .as_ref()
                    .and_then(|manifest| manifest.sidecar_generation.clone());
                let manifest_input_hash = report
                    .manifest
                    .as_ref()
                    .and_then(|manifest| manifest.sidecar_input_hash.clone());
                (
                    report.retrieval_mode,
                    report.degraded_reason,
                    manifest_generation,
                    manifest_input_hash,
                    report.ownership,
                )
            }
            Err(error) => (
                "unavailable".to_string(),
                Some(format!("sidecar_status_error: {error}")),
                None,
                None,
                None,
            ),
        };
    let sidecar = serde_json::json!({
        "retrieval_mode": sidecar_mode.clone(),
        "degraded_reason": degraded_reason.clone(),
        "sidecar_contract_version": codestory_retrieval::SIDECAR_SCHEMA_VERSION,
        "manifest_generation": manifest_generation.clone(),
        "manifest_input_hash": manifest_input_hash.clone(),
        "ownership": ownership,
    });
    let (server_executable, server_executable_sha256, server_warnings) =
        stdio_server_executable_status();
    let path_candidates = stdio_path_cli_candidate_statuses(server_executable.as_deref());
    let source_checkout_version = stdio_source_checkout_version(&runtime.project_root);
    let plugin_runtime = stdio_plugin_runtime_status();
    let setup_repair = stdio_setup_repair_input(server_executable.as_deref());
    let readiness = crate::readiness::build_readiness_verdicts(crate::readiness::ReadinessInputs {
        project: &summary.root,
        stats: &summary.stats,
        freshness: summary.freshness.as_ref(),
        setup: setup_repair.as_ref(),
        sidecar: Some(crate::readiness::ReadinessSidecarInput {
            profile: Some(sidecar_runtime.profile.as_str()),
            run_id: sidecar_runtime.run_id.as_deref(),
            retrieval_mode: &sidecar_mode,
            degraded_reason: degraded_reason.as_deref(),
            manifest_generation: manifest_generation.as_deref(),
            manifest_input_hash: manifest_input_hash.as_deref(),
        }),
    });
    let sidecar_setup = stdio_sidecar_setup_status(&runtime.project_root);
    let allowed_surfaces = stdio_allowed_surfaces(&readiness);
    let readiness_lanes = crate::build_readiness_lanes_for_runtime(runtime, &readiness);
    let recommended_next_calls = stdio_status_recommended_next_calls(&readiness, &sidecar_setup);
    let local = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::LocalNavigation)
        .expect("local_navigation readiness verdict");
    Ok(serde_json::json!({
        "server_version": env!("CARGO_PKG_VERSION"),
        "cli_version": env!("CARGO_PKG_VERSION"),
        "server_executable": server_executable,
        "server_executable_sha256": server_executable_sha256,
        "source_checkout_version": source_checkout_version,
        "path_candidates": path_candidates,
        "sidecar_contract_version": codestory_retrieval::SIDECAR_SCHEMA_VERSION,
        "plugin_runtime": plugin_runtime,
        "runtime_boundary": {
            "restart_required_for_runtime_change": true,
            "message": "A running MCP server keeps using the CLI process it was launched with; install, override, or PATH changes require a host reload/restart and a fresh codestory://status readback."
        },
        "warnings": server_warnings,
        "project_root": crate::display::clean_path_string(&runtime.project_root.to_string_lossy()),
        "storage_path": crate::display::clean_path_string(&runtime.storage_path.to_string_lossy()),
        "storage_exists": runtime.storage_path.exists(),
        "retrieval_mode": sidecar_mode,
        "degraded_reason": degraded_reason,
        "sidecar_retrieval": sidecar,
        "sidecar_setup": sidecar_setup,
        "legacy_semantic_diagnostics": {
            "mode": retrieval.map(|state| state.mode),
            "semantic_ready": retrieval.is_some_and(|state| state.semantic_ready),
            "semantic_doc_count": retrieval.map(|state| state.semantic_doc_count).unwrap_or(0),
            "fallback_reason": retrieval.and_then(|state| state.fallback_reason),
            "fallback_message": retrieval.and_then(|state| state.fallback_message.as_deref()),
            "diagnostic_only": true
        },
        "index_freshness": summary.freshness,
        "local_refresh": crate::readiness::local_refresh_output(local),
        "readiness": readiness,
        "readiness_lanes": readiness_lanes,
        "allowed_surfaces": allowed_surfaces,
        "recommended_next_calls": recommended_next_calls
    }))
}

fn stdio_setup_repair_input(
    server_executable: Option<&str>,
) -> Option<crate::readiness::ReadinessSetupInput> {
    let latest = stdio_latest_release_version()?;
    let active = env!("CARGO_PKG_VERSION");
    if compare_semver(active, &latest).is_some_and(|ordering| ordering.is_lt()) {
        let newer = stdio_newer_installed_cli(active, &latest, server_executable);
        return Some(crate::readiness::ReadinessSetupInput {
            active_path: server_executable.unwrap_or("<unknown>").to_string(),
            active_version: active.to_string(),
            latest_version: latest,
            newer_installed_path: newer.as_ref().map(|cli| cli.path.clone()),
            newer_installed_version: newer.map(|cli| cli.version),
        });
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstalledCliCandidate {
    path: String,
    version: String,
}

fn stdio_newer_installed_cli(
    active_version: &str,
    latest_version: &str,
    server_executable: Option<&str>,
) -> Option<InstalledCliCandidate> {
    if std::env::var("CODESTORY_DISABLE_INSTALLED_CLI_PROBE").is_ok() {
        return None;
    }
    stdio_installed_cli_candidates(latest_version)
        .into_iter()
        .filter(|candidate| {
            server_executable.is_none_or(|active| !same_path_text(candidate, active))
        })
        .filter_map(|candidate| stdio_cli_version(&candidate).map(|version| (candidate, version)))
        .filter(|(_, version)| {
            compare_semver(version, active_version).is_some_and(|ordering| ordering.is_gt())
        })
        .max_by(|left, right| {
            semver_triplet(&left.1)
                .unwrap_or_default()
                .cmp(&semver_triplet(&right.1).unwrap_or_default())
        })
        .map(|(path, version)| InstalledCliCandidate { path, version })
}

fn stdio_installed_cli_candidates(latest_version: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(cli) = std::env::var("CODESTORY_CLI")
        && !cli.trim().is_empty()
    {
        candidates.push(cli);
    }
    for home in stdio_codestory_home_candidates() {
        let bin = home.join("bin");
        push_cli_candidate_paths(&mut candidates, &bin);
        push_cli_candidate_paths(&mut candidates, &bin.join("releases").join(latest_version));
    }
    dedupe_path_text(candidates)
}

fn stdio_codestory_home_candidates() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Ok(home) = std::env::var("CODESTORY_HOME")
        && !home.trim().is_empty()
    {
        homes.push(PathBuf::from(home));
    }
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA")
        && !local_app_data.trim().is_empty()
    {
        homes.push(PathBuf::from(local_app_data).join("CodeStory"));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        homes.push(PathBuf::from(home).join(".codestory"));
    }
    dedupe_pathbufs(homes)
}

fn push_cli_candidate_paths(candidates: &mut Vec<String>, directory: &Path) {
    candidates.push(
        directory
            .join(if cfg!(windows) {
                "codestory-cli.exe"
            } else {
                "codestory-cli"
            })
            .to_string_lossy()
            .to_string(),
    );
    candidates.push(
        directory
            .join("codestory-cli")
            .to_string_lossy()
            .to_string(),
    );
}

fn stdio_cli_version(candidate: &str) -> Option<String> {
    stdio_cli_version_with_timeout(candidate, STDIO_CLI_VERSION_TIMEOUT)
}

fn stdio_cli_version_with_timeout(candidate: &str, timeout: Duration) -> Option<String> {
    let started_at = Instant::now();
    let mut child = Command::new(candidate)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    loop {
        if let Some(status) = child.try_wait().ok()? {
            let output = child.wait_with_output().ok()?;
            if !status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&output.stdout);
            return text.split_whitespace().find_map(normalize_release_version);
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        let remaining = timeout.saturating_sub(started_at.elapsed());
        std::thread::sleep(STDIO_CLI_VERSION_POLL_INTERVAL.min(remaining));
    }
}

fn stdio_path_cli_candidate_statuses(active_path: Option<&str>) -> serde_json::Value {
    serde_json::Value::Array(
        stdio_path_cli_candidates()
            .into_iter()
            .map(|path| {
                serde_json::json!({
                    "path": crate::display::clean_path_string(&path),
                    "version": stdio_cli_version(&path),
                    "active": active_path.is_some_and(|active| same_path_text(&path, active)),
                })
            })
            .collect(),
    )
}

fn stdio_path_cli_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            push_existing_path_cli_candidates(&mut candidates, &directory);
        }
    }
    dedupe_path_text(candidates)
}

fn push_existing_path_cli_candidates(candidates: &mut Vec<String>, directory: &Path) {
    for binary in stdio_cli_binary_names() {
        let candidate = directory.join(binary);
        if candidate.is_file() {
            candidates.push(candidate.to_string_lossy().to_string());
        }
    }
}

fn stdio_cli_binary_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "codestory-cli.exe",
            "codestory-cli.cmd",
            "codestory-cli.bat",
            "codestory-cli",
        ]
    } else {
        &["codestory-cli"]
    }
}

fn stdio_source_checkout_version(project_root: &Path) -> Option<String> {
    fs::read_to_string(project_root.join("crates/codestory-cli/Cargo.toml"))
        .ok()
        .and_then(|manifest| cargo_package_version(&manifest))
}

fn cargo_package_version(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some(version) = trimmed.strip_prefix("version") else {
            continue;
        };
        let Some(version) = version.trim_start().strip_prefix('=') else {
            continue;
        };
        if let Some(version) = version.trim().strip_prefix('"').and_then(|value| {
            value
                .split_once('"')
                .map(|(version, _)| version.to_string())
        }) {
            return Some(version);
        }
    }
    None
}

fn same_path_text(left: &str, right: &str) -> bool {
    left.trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(right.trim_end_matches(['\\', '/']))
}

fn dedupe_path_text(paths: Vec<String>) -> Vec<String> {
    let mut deduped: Vec<String> = Vec::new();
    for path in paths {
        if path.trim().is_empty() || deduped.iter().any(|seen| same_path_text(seen, &path)) {
            continue;
        }
        deduped.push(path);
    }
    deduped
}

fn dedupe_pathbufs(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped: Vec<PathBuf> = Vec::new();
    for path in paths {
        if deduped
            .iter()
            .any(|seen| same_path_text(&seen.to_string_lossy(), &path.to_string_lossy()))
        {
            continue;
        }
        deduped.push(path);
    }
    deduped
}

fn stdio_latest_release_version() -> Option<String> {
    if let Ok(version) = std::env::var("CODESTORY_LATEST_RELEASE_VERSION")
        && let Some(version) = normalize_release_version(&version)
    {
        return Some(version);
    }
    let response =
        ureq::get("https://api.github.com/repos/TheGreenCedar/CodeStory/releases/latest")
            .timeout(StdDuration::from_secs(2))
            .call()
            .ok()?;
    let body: serde_json::Value = serde_json::from_str(&response.into_string().ok()?).ok()?;
    body.get("tag_name")
        .and_then(|value| value.as_str())
        .and_then(normalize_release_version)
}

fn normalize_release_version(version: &str) -> Option<String> {
    let trimmed = version.trim().trim_start_matches('v');
    semver_triplet(trimmed).map(|_| trimmed.to_string())
}

fn compare_semver(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(semver_triplet(left)?.cmp(&semver_triplet(right)?))
}

fn semver_triplet(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

fn stdio_status_recommended_next_calls(
    readiness: &[ReadinessVerdictDto],
    sidecar_setup: &serde_json::Value,
) -> serde_json::Value {
    if let Some(non_ready) = crate::readiness::primary_non_ready(readiness) {
        if non_ready.goal == ReadinessGoalDto::AgentPacketSearch {
            match sidecar_setup
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("ask")
            {
                "ask" => {
                    return serde_json::json!([
                        {
                            "method": "host/confirm",
                            "instruction": sidecar_setup["prompt"]
                        },
                        {
                            "method": "cli",
                            "command": sidecar_setup["enable_command"]
                        },
                        {
                            "method": "cli",
                            "command": sidecar_setup["disable_command"]
                        },
                        {
                            "method": "resources/read",
                            "uri": "codestory://status"
                        },
                        {
                            "method": "resources/read",
                            "uri": "codestory://agent-guide"
                        }
                    ]);
                }
                "disabled" => {
                    return serde_json::json!([
                        {
                            "method": "host/instruction",
                            "instruction": "Automatic sidecar setup is disabled for this plugin install."
                        },
                        {
                            "method": "cli",
                            "command": sidecar_setup["enable_command"]
                        },
                        {
                            "method": "resources/read",
                            "uri": "codestory://status"
                        },
                        {
                            "method": "resources/read",
                            "uri": "codestory://agent-guide"
                        }
                    ]);
                }
                _ => {}
            }
        }
        return serde_json::Value::Array(
            non_ready
                .full_repair
                .iter()
                .map(|command| stdio_recommended_next_call(command))
                .chain([
                    serde_json::json!({
                        "method": "resources/read",
                        "uri": "codestory://status"
                    }),
                    serde_json::json!({
                        "method": "resources/read",
                        "uri": "codestory://agent-guide"
                    }),
                ])
                .collect(),
        );
    }

    serde_json::json!([
        {
            "method": "resources/read",
            "uri": "codestory://agent-guide"
        },
        {
            "method": "tools/call",
            "tool": "ground",
            "arguments": {
                "budget": "balanced"
            }
        },
        {
            "method": "tools/call",
            "tool": "packet",
            "arguments": {
                "question": "<broad-task-question>",
                "budget": "compact"
            }
        },
        {
            "method": "tools/call",
            "tool": "search",
            "arguments": {
                "query": "<symbol-or-concept>",
                "limit": 10
            }
        },
        {
            "method": "tools/call",
            "tool": "definition",
            "arguments": {
                "id": "<node_id-from-search>"
            }
        },
        {
            "method": "resources/read",
            "uri": "codestory://trail/<node_id-from-search>"
        }
    ])
}

fn stdio_recommended_next_call(command: &str) -> serde_json::Value {
    if command.starts_with("Restart/reload the Codex host/app") {
        return serde_json::json!({
            "method": "host/restart",
            "instruction": command
        });
    }
    serde_json::json!({
        "method": "cli",
        "command": command
    })
}

fn stdio_server_executable_status() -> (Option<String>, Option<String>, Vec<String>) {
    match std::env::current_exe() {
        Ok(path) => {
            let display = crate::display::clean_path_string(&path.to_string_lossy());
            match sha256_file(&path) {
                Ok(sha256) => (Some(display), Some(sha256), Vec::new()),
                Err(error) => (
                    Some(display),
                    None,
                    vec![format!("server_executable_checksum_unavailable: {error}")],
                ),
            }
        }
        Err(error) => (
            None,
            None,
            vec![format!("server_executable_unavailable: {error}")],
        ),
    }
}

fn stdio_plugin_runtime_status() -> serde_json::Value {
    let cli_source = env_nonempty("CODESTORY_PLUGIN_CLI_SOURCE")
        .unwrap_or_else(|| "direct_cli_launch".to_string());
    let warnings = env_nonempty("CODESTORY_PLUGIN_CLI_WARNINGS")
        .map(|value| {
            value
                .split(';')
                .filter(|item| !item.trim().is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::json!({
        "plugin_version": env_nonempty("CODESTORY_PLUGIN_VERSION"),
        "plugin_root": env_nonempty("CODESTORY_PLUGIN_ROOT"),
        "plugin_cache_version": env_nonempty("CODESTORY_PLUGIN_CACHE_VERSION"),
        "cli_version": env_nonempty("CODESTORY_PLUGIN_CLI_VERSION"),
        "cli_source": cli_source,
        "cli_path": env_nonempty("CODESTORY_PLUGIN_CLI_PATH"),
        "cli_sha256": env_nonempty("CODESTORY_PLUGIN_CLI_SHA256"),
        "build_source": env_nonempty("CODESTORY_PLUGIN_CLI_BUILD_SOURCE"),
        "repo_ref": env_nonempty("CODESTORY_PLUGIN_CLI_REPO_REF"),
        "archive_sha256": env_nonempty("CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256"),
        "archive_url": env_nonempty("CODESTORY_PLUGIN_CLI_ARCHIVE_URL"),
        "provisioned_at": env_nonempty("CODESTORY_PLUGIN_CLI_PROVISIONED_AT"),
        "local_dev_override": cli_source == "local_dev_override",
        "path_fallback": cli_source == "path_fallback",
        "managed_binary_path": if cli_source == "managed" { env_nonempty("CODESTORY_PLUGIN_CLI_PATH") } else { None },
        "managed_binary_sha256": if cli_source == "managed" { env_nonempty("CODESTORY_PLUGIN_CLI_SHA256") } else { None },
        "managed_manifest_path": env_nonempty("CODESTORY_PLUGIN_CLI_MANIFEST_PATH"),
        "warnings": warnings
    })
}

pub(crate) fn stdio_sidecar_setup_status(project_root: &Path) -> serde_json::Value {
    let state = match env_nonempty("CODESTORY_PLUGIN_SIDECAR_POLICY_STATE").as_deref() {
        Some("enabled") => "enabled",
        Some("disabled") => "disabled",
        Some(_) => "ask",
        None => "unmanaged",
    };
    let prompt_required = matches!(state, "ask");
    let auto_repair = matches!(state, "enabled");
    let project = crate::display::clean_path_string(&project_root.to_string_lossy());
    let default_repair =
        format!("codestory-cli ready --goal agent --repair --project \"{project}\" --format json");
    let next_repair_command =
        env_nonempty("CODESTORY_PLUGIN_SIDECAR_NEXT_REPAIR_COMMAND").unwrap_or(default_repair);
    serde_json::json!({
        "state": state,
        "auto_repair": auto_repair,
        "prompt_required": prompt_required,
        "prompt": if prompt_required { Some("CodeStory packet/search needs retrieval sidecars. Enable automatic sidecar setup for this plugin install?") } else { None },
        "enable_command": env_nonempty("CODESTORY_PLUGIN_SIDECAR_ENABLE_COMMAND"),
        "disable_command": env_nonempty("CODESTORY_PLUGIN_SIDECAR_DISABLE_COMMAND"),
        "next_repair_command": next_repair_command,
        "policy_path": env_nonempty("CODESTORY_PLUGIN_SIDECAR_POLICY_PATH"),
        "policy_updated_at": env_nonempty("CODESTORY_PLUGIN_SIDECAR_POLICY_UPDATED_AT"),
        "last_repair": {
            "state": env_nonempty("CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_STATE"),
            "updated_at": env_nonempty("CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_AT"),
            "project_root": env_nonempty("CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_PROJECT"),
            "command": env_nonempty("CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_COMMAND")
        }
    })
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).with_context(|| format!("hash {}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn stdio_allowed_surfaces(readiness: &[ReadinessVerdictDto]) -> serde_json::Value {
    let local = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::LocalNavigation);
    let agent = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::AgentPacketSearch);

    let mut surfaces = serde_json::Map::new();
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
        surfaces.insert(surface.to_string(), stdio_allowed_surface(local));
    }
    for surface in ["packet", "search", "context"] {
        surfaces.insert(surface.to_string(), stdio_allowed_surface(agent));
    }
    serde_json::Value::Object(surfaces)
}

fn stdio_allowed_surface(verdict: Option<&ReadinessVerdictDto>) -> serde_json::Value {
    match verdict {
        Some(verdict) => {
            let allowed = verdict.status == ReadinessStatusDto::Ready;
            serde_json::json!({
                "allowed": allowed,
                "readiness_goal": crate::readiness::goal_label(verdict.goal),
                "status": crate::readiness::status_label(verdict.status),
                "failed_layer": crate::readiness::failed_layer(verdict),
                "summary": verdict.summary,
                "repair_reason": stdio_repair_reason(verdict),
                "blocked_reason": if allowed { None } else { Some(verdict.summary.as_str()) },
                "minimum_next": verdict.minimum_next,
                "full_repair": verdict.full_repair,
            })
        }
        None => serde_json::json!({
            "allowed": false,
            "readiness_goal": null,
            "status": "unknown",
            "failed_layer": null,
            "summary": "Readiness verdict was not available for this surface.",
            "repair_reason": null,
            "blocked_reason": "Readiness verdict was not available for this surface.",
            "minimum_next": [],
            "full_repair": [],
        }),
    }
}

fn stdio_repair_reason(verdict: &ReadinessVerdictDto) -> Option<String> {
    if verdict.status == ReadinessStatusDto::RepairSetup {
        return Some("stale_active_cli".to_string());
    }
    if verdict.status == ReadinessStatusDto::RepairRetrieval {
        return verdict
            .sidecar
            .as_ref()
            .and_then(|sidecar| sidecar.degraded_reason.clone())
            .or_else(|| Some("retrieval_not_full".to_string()));
    }
    None
}

fn read_stdio_agent_guide_resource() -> serde_json::Value {
    serde_json::json!({
        "purpose": "Default read-only CodeStory browser loop for local codebase grounding.",
        "recommended_call_sequence": [
            {
                "method": "resources/read",
                "uri": "codestory://status"
            }
        ],
        "readiness_lanes": [
            {
                "readiness_goal": "local_navigation",
                "condition": "Use only surfaces whose codestory://status allowed_surfaces.<surface>.allowed value is true.",
                "surfaces": ["ground", "files", "symbol", "definition", "get_node", "callers", "callees", "neighbors", "shortest_path", "query_subgraph", "symbols", "snippet", "references", "trace", "trail", "affected"],
                "calls": [
                    {
                        "method": "tools/call",
                        "tool": "ground",
                        "arguments": {
                            "budget": "balanced"
                        }
                    },
                    {
                        "method": "tools/call",
                        "tool": "files",
                        "arguments": {
                            "limit": 50
                        }
                    },
                    {
                        "method": "tools/call",
                        "tool": "definition",
                        "arguments": {
                            "id": "<best-node-id>"
                        }
                    },
                    {
                        "method": "tools/call",
                        "tool": "get_node",
                        "arguments": {
                            "id": "<best-node-id>"
                        }
                    },
                    {
                        "method": "tools/call",
                        "tool": "neighbors",
                        "arguments": {
                            "id": "<best-node-id>",
                            "depth": 1
                        }
                    },
                    {
                        "method": "tools/call",
                        "tool": "symbols",
                        "arguments": {
                            "limit": 50
                        }
                    },
                    {
                        "method": "resources/read",
                        "uri": "codestory://snippet/<best-node-id>"
                    },
                    {
                        "method": "resources/read",
                        "uri": "codestory://references/<best-node-id>"
                    },
                    {
                        "method": "resources/read",
                        "uri": "codestory://trail/<best-node-id>"
                    }
                ]
            },
            {
                "readiness_goal": "agent_packet_search",
                "condition": "Use packet/search/context only when their codestory://status allowed_surfaces entries are true.",
                "surfaces": ["packet", "search", "context"],
                "calls": [
                    {
                        "method": "tools/call",
                        "tool": "packet",
                        "arguments": {
                            "question": "<broad-task-question>",
                            "budget": "compact"
                        }
                    },
                    {
                        "method": "tools/call",
                        "tool": "search",
                        "arguments": {
                            "query": "<symbol-or-task>",
                            "limit": 10
                        }
                    },
                    {
                        "method": "tools/call",
                        "tool": "context",
                        "arguments": {
                            "id": "<best-node-id>"
                        }
                    }
                ]
            }
        ],
        "surface_decisions": [
            {
                "surface": "ground",
                "kind": "tool and codestory://grounding resource",
                "when": "Use after status when allowed_surfaces.ground.allowed is true."
            },
            {
                "surface": "packet",
                "kind": "tool",
                "when": "Use for broad structural questions only when allowed_surfaces.packet.allowed is true and strict retrieval is full."
            },
            {
                "surface": "search",
                "kind": "tool",
                "when": "Use for bounded candidate discovery only when allowed_surfaces.search.allowed is true."
            },
            {
                "surface": "context",
                "kind": "tool",
                "when": "Use after selecting one concrete target only when allowed_surfaces.context.allowed is true."
            },
            {
                "surface": "direct_source_reads",
                "kind": "fallback",
                "when": "Use when status reports missing, stale, or degraded index/sidecar state."
            },
            {
                "surface": "cache identity, retrieval status",
                "kind": "deferred",
                "when": "Use CLI or resources until these receive explicit read-only stdio contracts."
            }
        ],
        "safety_notes": [
            "All stdio tools are read-only, non-destructive, idempotent, local-only, and closed-world.",
            "Read codestory://status first and branch on allowed_surfaces before choosing tools.",
            "Use ground for compact repository orientation after status when local_navigation is ready.",
            "Use packet for broad task questions only when packet/search status is allowed; use context only when allowed_surfaces.context.allowed is true.",
            "Treat packet status other than sufficient as unsafe to claim until gaps, open_next, and follow_up_commands are resolved.",
            "Use continuation links from search or definition results before broadening retrieval.",
            "Keep search limits bounded; stdio search clamps limit to 1..50.",
            "Treat repo-text hits as navigation clues and search hits as discovery clues until backed by proof-bearing sidecar, graph, or source evidence."
        ]
    })
}

fn enrich_stdio_search_result(
    result: codestory_contracts::api::SearchResultsDto,
) -> serde_json::Value {
    let mut value = serde_json::to_value(result)
        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}));
    for field in [
        "suggestions",
        "indexed_symbol_hits",
        "repo_text_hits",
        "hits",
    ] {
        if let Some(hits) = value.get_mut(field).and_then(|field| field.as_array_mut()) {
            for hit in hits {
                enrich_stdio_search_hit(hit);
            }
        }
    }
    value
}

fn enrich_stdio_search_hit(hit: &mut serde_json::Value) {
    if !hit
        .get("resolvable")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return;
    }
    let Some(node_id) = hit
        .get("node_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    add_stdio_links(hit, stdio_node_links(&node_id));
}

fn add_stdio_links(hit: &mut serde_json::Value, links: serde_json::Value) {
    if let Some(object) = hit.as_object_mut() {
        object.insert("links".to_string(), links);
    }
}

fn stdio_node_links(node_id: &str) -> serde_json::Value {
    serde_json::json!([
        {
            "rel": "symbol",
            "uri": format!("codestory://symbol/{node_id}")
        },
        {
            "rel": "snippet",
            "uri": format!("codestory://snippet/{node_id}")
        },
        {
            "rel": "references",
            "uri": format!("codestory://references/{node_id}")
        },
        {
            "rel": "trail",
            "uri": format!("codestory://trail/{node_id}")
        }
    ])
}

fn read_stdio_template_resource(runtime: &RuntimeContext, uri: &str) -> Result<serde_json::Value> {
    let Some((kind, node_id)) = uri
        .strip_prefix("codestory://")
        .and_then(|tail| tail.split_once('/'))
    else {
        bail!("unknown resource");
    };
    let node_id = NodeId(node_id.to_string());
    match kind {
        "symbol" => runtime
            .browser
            .symbol_context(node_id)
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        "references" => runtime
            .browser
            .references_context(browser_references_config(node_id))
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        "snippet" => runtime
            .browser
            .snippet_context(node_id, 4)
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        "trail" => runtime
            .browser
            .trail_context(browser_trail_config(
                node_id,
                BROWSER_TRAIL_DEFAULT_DEPTH,
                TrailDirection::Both,
                false,
            ))
            .map(|value| serde_json::json!(value))
            .map_err(map_api_error),
        _ => bail!("unknown resource"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_packet_cache_key_input(question: &str) -> StdioPacketCacheKeyInput<'_> {
        StdioPacketCacheKeyInput {
            storage_fingerprint: "snapshot-a".to_string(),
            sidecar_fingerprint: "sidecar-full".to_string(),
            question,
            budget: PacketBudgetModeDto::Compact,
            task_class: Some(PacketTaskClassDto::ArchitectureExplanation),
            extra_probes: &[],
            include_evidence: true,
            latency_budget_ms: Some(15_000),
        }
    }

    fn packet_key(question: &str, storage_fingerprint: &str) -> StdioPacketCacheKey {
        stdio_packet_cache_key(StdioPacketCacheKeyInput {
            storage_fingerprint: storage_fingerprint.to_string(),
            ..base_packet_cache_key_input(question)
        })
    }

    #[test]
    fn stdio_recommended_next_calls_labels_restart_boundary_as_host_action() {
        let restart =
            "Restart/reload the Codex host/app so MCP relaunches codestory-cli 0.11.11 from C:/Users/alber/AppData/Local/CodeStory/bin/codestory-cli.exe; then open a fresh agent thread and read codestory://status."
                .to_string();
        let calls = stdio_status_recommended_next_calls(
            &[ReadinessVerdictDto {
                goal: ReadinessGoalDto::LocalNavigation,
                status: ReadinessStatusDto::RepairSetup,
                summary: "A newer installed codestory-cli exists outside the active process."
                    .to_string(),
                minimum_next: vec![restart.clone()],
                full_repair: vec![restart.clone()],
                setup: None,
                index: None,
                sidecar: None,
            }],
            &json!({"state": "enabled"}),
        );

        assert_eq!(calls[0]["method"], json!("host/restart"));
        assert_eq!(calls[0]["instruction"], json!(restart));
        assert!(
            calls[0].get("command").is_none(),
            "restart boundary should not be exposed as a CLI command: {calls}"
        );
    }

    #[test]
    fn cargo_package_version_reads_only_package_section() {
        let manifest = r#"
[workspace]
version = "9.9.9"

[package]
name = "codestory-cli"
edition = "2024"
version = "0.11.20"
"#;

        assert_eq!(cargo_package_version(manifest), Some("0.11.20".to_string()));
    }

    #[test]
    fn stdio_cli_version_returns_none_when_probe_times_out() {
        let temp_dir = std::env::temp_dir().join(format!(
            "codestory-stdio-cli-timeout-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir");
        let candidate = temp_dir.join(if cfg!(windows) {
            "codestory-cli.cmd"
        } else {
            "codestory-cli"
        });
        fs::write(
            &candidate,
            if cfg!(windows) {
                "@echo off\r\nping -n 6 127.0.0.1 > nul\r\necho codestory-cli 9.9.9\r\n"
            } else {
                "#!/bin/sh\nsleep 5\necho codestory-cli 9.9.9\n"
            },
        )
        .expect("write slow cli probe");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&candidate)
                .expect("candidate metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&candidate, permissions).expect("chmod candidate");
        }

        let started_at = Instant::now();
        let version =
            stdio_cli_version_with_timeout(&candidate.to_string_lossy(), Duration::from_millis(50));

        assert_eq!(version, None);
        assert!(
            started_at.elapsed() < Duration::from_secs(2),
            "version probe should return near the configured timeout"
        );
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn stdio_blocked_agent_surfaces_name_retrieval_layer_and_canonical_repair() {
        let repair =
            "codestory-cli ready --goal agent --repair --project \"C:/repo/example\" --format json"
                .to_string();
        let readiness = vec![ReadinessVerdictDto {
            goal: ReadinessGoalDto::AgentPacketSearch,
            status: ReadinessStatusDto::RepairRetrieval,
            summary:
                "Agent packet/search needs full sidecar retrieval; current mode is `unavailable`."
                    .to_string(),
            minimum_next: vec![repair.clone()],
            full_repair: vec![
                repair.clone(),
                "codestory-cli retrieval status --project \"C:/repo/example\" --format json"
                    .to_string(),
                "codestory-cli doctor --project \"C:/repo/example\" --format markdown".to_string(),
            ],
            setup: None,
            index: None,
            sidecar: None,
        }];

        let surfaces = stdio_allowed_surfaces(&readiness);
        let packet = &surfaces["packet"];

        assert_eq!(packet["allowed"], json!(false));
        assert_eq!(packet["failed_layer"], json!("retrieval_sidecar"));
        assert_eq!(packet["minimum_next"], json!([repair]));
        assert!(
            packet["full_repair"]
                .as_array()
                .is_some_and(|commands| commands.len() == 3),
            "full repair should keep proof commands behind the canonical minimum repair: {packet}"
        );
    }

    #[test]
    fn stdio_blocks_agent_surfaces_when_only_local_sidecar_is_full() {
        let stats = codestory_contracts::api::StorageStatsDto {
            file_count: 1,
            node_count: 1,
            edge_count: 0,
            error_count: 0,
            fatal_error_count: 0,
        };
        let readiness =
            crate::readiness::build_readiness_verdicts(crate::readiness::ReadinessInputs {
                project: "C:/repo/example",
                stats: &stats,
                freshness: None,
                setup: None,
                sidecar: Some(crate::readiness::ReadinessSidecarInput {
                    profile: Some("local"),
                    run_id: None,
                    retrieval_mode: "full",
                    degraded_reason: None,
                    manifest_generation: Some("generation"),
                    manifest_input_hash: Some("hash"),
                }),
            });

        let surfaces = stdio_allowed_surfaces(&readiness);

        assert_eq!(surfaces["ground"]["allowed"], json!(true));
        assert_eq!(surfaces["files"]["allowed"], json!(true));
        for surface in ["packet", "search", "context"] {
            assert_eq!(
                surfaces[surface]["allowed"],
                json!(false),
                "local/default full sidecar must not unlock {surface}: {surfaces}"
            );
            assert_eq!(
                surfaces[surface]["status"],
                json!("repair_retrieval"),
                "blocked agent surface should stay on the agent retrieval lane: {surfaces}"
            );
        }
    }

    #[test]
    fn stdio_tool_call_success_keeps_packet_timing_out_of_structured_content() {
        let mut packet = json!({
            "packet_id": "packet-1",
            "answer": {
                "retrieval_trace": {
                    "annotations": []
                }
            },
            "budget": {
                "limits": {
                    "max_output_bytes": 0
                },
                "used": {
                    "output_bytes": 0
                }
            }
        });
        for _ in 0..4 {
            let len = serde_json::to_vec(&packet).expect("serialize packet").len();
            packet["budget"]["limits"]["max_output_bytes"] = json!(len);
            packet["budget"]["used"]["output_bytes"] = json!(len);
        }

        let response = stdio_tool_call_success(packet);
        let annotations = response
            .pointer("/structuredContent/answer/retrieval_trace/annotations")
            .and_then(|value| value.as_array())
            .expect("packet annotations");
        assert!(
            annotations.is_empty(),
            "stdio timings should not mutate budgeted packet content: {annotations:?}"
        );

        let packet = response
            .get("structuredContent")
            .expect("structured packet content");
        let packet_bytes = serde_json::to_vec(packet)
            .expect("serialize structured packet")
            .len();
        let max_output_bytes = packet
            .pointer("/budget/limits/max_output_bytes")
            .and_then(|value| value.as_u64())
            .expect("packet max output bytes") as usize;
        let used_output_bytes = packet
            .pointer("/budget/used/output_bytes")
            .and_then(|value| value.as_u64())
            .expect("packet used output bytes") as usize;
        assert_eq!(
            used_output_bytes, packet_bytes,
            "stdio packet content should not make output byte telemetry stale"
        );
        assert!(
            packet_bytes <= max_output_bytes,
            "stdio packet content should stay inside the enforced budget: {packet_bytes} > {max_output_bytes}"
        );

        let annotations = response
            .pointer("/_meta/codestory_stdio_phases")
            .and_then(|value| value.as_array())
            .expect("stdio phases");

        assert!(annotations.iter().any(|annotation| {
            annotation.as_str().is_some_and(|value| {
                value.starts_with("packet_stdio_phase label=text_materialization duration_ms=")
            })
        }));
        assert!(annotations.iter().any(|annotation| {
            annotation.as_str().is_some_and(|value| {
                value.starts_with(
                    "packet_stdio_phase label=tool_response_materialization duration_ms=",
                )
            })
        }));
    }

    #[test]
    fn stdio_search_fragment_cache_reuses_matching_queries() {
        let mut cache = StdioSearchFragmentCache::default();
        let key = StdioSearchFragmentCacheKey {
            storage_fingerprint: "snapshot-a".to_string(),
            sidecar_fingerprint: "sidecar-full".to_string(),
            query: "run_index".to_string(),
            repo_text: "auto".to_string(),
            limit_per_source: 10,
        };
        let response = json!({"result": {"hits": []}});
        cache.insert(key.clone(), response.clone());
        assert_eq!(cache.get(&key), Some(response));
        assert_eq!(
            cache.get(&StdioSearchFragmentCacheKey {
                query: "other".to_string(),
                ..key.clone()
            }),
            None
        );
    }

    #[test]
    fn stdio_search_fragment_cache_evicts_least_recently_used_entry() {
        let mut cache = StdioSearchFragmentCache::default();
        let first = StdioSearchFragmentCacheKey {
            storage_fingerprint: "snapshot-a".to_string(),
            sidecar_fingerprint: "sidecar-full".to_string(),
            query: "first".to_string(),
            repo_text: "auto".to_string(),
            limit_per_source: 10,
        };
        let second = StdioSearchFragmentCacheKey {
            query: "second".to_string(),
            ..first.clone()
        };

        cache.insert(first.clone(), json!({"result": {"hits": ["first"]}}));
        cache.insert(second.clone(), json!({"result": {"hits": ["second"]}}));
        assert!(cache.get(&first).is_some());

        for index in 0..(STDIO_SEARCH_FRAGMENT_CACHE_CAPACITY - 1) {
            cache.insert(
                StdioSearchFragmentCacheKey {
                    query: format!("extra-{index}"),
                    ..first.clone()
                },
                json!({"result": {"hits": [format!("extra-{index}")]}}),
            );
        }

        assert!(cache.get(&first).is_some());
        assert_eq!(cache.get(&second), None);
    }

    #[test]
    fn stdio_packet_cache_reuses_successful_packets_by_lru_key() {
        let mut cache = StdioPacketCache::default();
        let key = packet_key("Explain packet caching.", "snapshot-a");
        let response = json!({"result": {"packet_id": "packet-1"}});

        cache.insert(key.clone(), response.clone());

        assert_eq!(cache.get(&key), Some(response));
        assert_eq!(
            cache.get(&packet_key("Explain a different packet.", "snapshot-a")),
            None
        );
    }

    #[test]
    fn stdio_packet_cache_evicts_least_recently_used_entry() {
        let mut cache = StdioPacketCache::default();
        let first = packet_key("first", "snapshot-a");
        let second = packet_key("second", "snapshot-a");

        cache.insert(first.clone(), json!({"result": {"packet_id": "first"}}));
        cache.insert(second.clone(), json!({"result": {"packet_id": "second"}}));
        assert!(cache.get(&first).is_some());

        for index in 0..(STDIO_PACKET_CACHE_CAPACITY - 1) {
            cache.insert(
                packet_key(&format!("extra-{index}"), "snapshot-a"),
                json!({"result": {"packet_id": format!("extra-{index}")}}),
            );
        }

        assert!(cache.get(&first).is_some());
        assert_eq!(cache.get(&second), None);
    }

    #[test]
    fn stdio_packet_cache_key_changes_with_request_arguments_and_snapshot() {
        let base_input = base_packet_cache_key_input("Explain packet caching.");
        let base = stdio_packet_cache_key(base_input);
        assert_ne!(
            base,
            stdio_packet_cache_key(StdioPacketCacheKeyInput {
                storage_fingerprint: "snapshot-b".to_string(),
                ..base_packet_cache_key_input("Explain packet caching.")
            })
        );
        assert_ne!(
            base,
            stdio_packet_cache_key(StdioPacketCacheKeyInput {
                budget: PacketBudgetModeDto::Tiny,
                ..base_packet_cache_key_input("Explain packet caching.")
            })
        );
        assert_ne!(
            base,
            stdio_packet_cache_key(StdioPacketCacheKeyInput {
                task_class: Some(PacketTaskClassDto::EditPlanning),
                ..base_packet_cache_key_input("Explain packet caching.")
            })
        );
        assert_ne!(
            base,
            stdio_packet_cache_key(StdioPacketCacheKeyInput {
                include_evidence: false,
                ..base_packet_cache_key_input("Explain packet caching.")
            })
        );
        assert_ne!(
            base,
            stdio_packet_cache_key(StdioPacketCacheKeyInput {
                latency_budget_ms: Some(30_000),
                ..base_packet_cache_key_input("Explain packet caching.")
            })
        );
        let extra_probes = ["src/lib.rs run".to_string()];
        assert_ne!(
            base,
            stdio_packet_cache_key(StdioPacketCacheKeyInput {
                extra_probes: &extra_probes,
                ..base_packet_cache_key_input("Explain packet caching.")
            })
        );
    }

    #[test]
    fn stdio_cache_keys_track_sidecar_fingerprint_without_sqlite_change() {
        let storage_fingerprint = "snapshot-a".to_string();
        let full_sidecar =
            "retrieval_mode:full|manifest_generation:project-a|manifest_input_hash:hash-a";
        let stale_sidecar = "retrieval_mode:unavailable|degraded_reason:sidecar_manifest_stale";

        let packet_full = stdio_packet_cache_key(StdioPacketCacheKeyInput {
            storage_fingerprint: storage_fingerprint.clone(),
            sidecar_fingerprint: full_sidecar.to_string(),
            ..base_packet_cache_key_input("Explain packet caching.")
        });
        let packet_stale = stdio_packet_cache_key(StdioPacketCacheKeyInput {
            storage_fingerprint: storage_fingerprint.clone(),
            sidecar_fingerprint: stale_sidecar.to_string(),
            ..base_packet_cache_key_input("Explain packet caching.")
        });
        assert_ne!(packet_full, packet_stale);

        let search_full = StdioSearchFragmentCacheKey {
            storage_fingerprint: storage_fingerprint.clone(),
            sidecar_fingerprint: full_sidecar.to_string(),
            query: "handler".to_string(),
            repo_text: "auto".to_string(),
            limit_per_source: 10,
        };
        let search_stale = StdioSearchFragmentCacheKey {
            sidecar_fingerprint: stale_sidecar.to_string(),
            ..search_full.clone()
        };
        assert_ne!(search_full, search_stale);
        assert_eq!(
            search_full.storage_fingerprint, search_stale.storage_fingerprint,
            "regression must cover sidecar status drift without SQLite fingerprint changes"
        );
    }

    #[test]
    fn stdio_product_cache_key_uses_strict_sidecar_readiness() {
        let storage_fingerprint = "sqlite-and-wal-stable".to_string();
        let manifest = codestory_retrieval::RetrievalIndexManifest {
            project_id: "project-a".into(),
            zoekt_version: "zoekt-real-v1".into(),
            qdrant_collection: "codestory_project_a_hash_a".into(),
            scip_revision: Some("graph-test".into()),
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some(codestory_retrieval::embedding_runtime_id()),
            embedding_dim: Some(codestory_retrieval::RETRIEVAL_EMBEDDING_DIM as i32),
            sidecar_schema_version: Some(codestory_retrieval::SIDECAR_SCHEMA_VERSION),
            sidecar_input_hash: Some("hash-a".into()),
            sidecar_generation: Some("project-a-hash-a".into()),
            projection_count: Some(12),
            symbol_doc_count: Some(120),
            dense_projection_count: Some(12),
            semantic_policy_version: Some("graph_first_v1".into()),
            graph_artifact_hash: Some("graph-hash-a".into()),
            dense_reason_counts_json: Some(r#"{"public_api":12}"#.into()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        };
        let before_stale = stdio_mandatory_sidecar_fingerprint_from_status(
            codestory_retrieval::embedding_runtime_id(),
            "state-file-stable",
            Ok(StdioSidecarStatusFingerprint {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                manifest: Some(manifest.clone()),
            }),
        );
        let successful_key = stdio_packet_cache_key(StdioPacketCacheKeyInput {
            storage_fingerprint: storage_fingerprint.clone(),
            sidecar_fingerprint: before_stale.clone(),
            question: "Explain strict readiness.",
            task_class: None,
            latency_budget_ms: None,
            ..base_packet_cache_key_input("Explain strict readiness.")
        });
        let mut cache = StdioPacketCache::default();
        cache.insert(
            successful_key.clone(),
            json!({"result": {"packet_id": "cached"}}),
        );

        let after_stale = stdio_mandatory_sidecar_fingerprint_from_status(
            codestory_retrieval::embedding_runtime_id(),
            "state-file-stable",
            Ok(StdioSidecarStatusFingerprint {
                retrieval_mode: "unavailable".into(),
                degraded_reason: Some(
                    "sidecar_manifest_stale: indexable_file_added_or_changed_after_sidecar_manifest: src/new_module.rs"
                        .into(),
                ),
                manifest: Some(manifest),
            }),
        );
        let stale_key = stdio_packet_cache_key(StdioPacketCacheKeyInput {
            storage_fingerprint: storage_fingerprint.clone(),
            sidecar_fingerprint: after_stale.clone(),
            question: "Explain strict readiness.",
            task_class: None,
            latency_budget_ms: None,
            ..base_packet_cache_key_input("Explain strict readiness.")
        });

        assert_ne!(before_stale, after_stale);
        assert!(
            before_stale.contains("retrieval_mode:full"),
            "the successful key must be tied to full strict sidecar readiness: {before_stale}"
        );
        assert!(
            after_stale.contains("sidecar_manifest_stale"),
            "stdio product cache key must encode strict stale status: {after_stale}"
        );
        assert_eq!(
            successful_key.storage_fingerprint, stale_key.storage_fingerprint,
            "the regression must cover sidecar drift without SQLite fingerprint changes"
        );
        assert!(
            cache.get(&successful_key).is_some(),
            "the full strict-readiness key should represent a successful warm cache entry"
        );
        assert_eq!(
            cache.get(&stale_key),
            None,
            "same-server cached product result must not be returned once strict status is stale"
        );
    }

    #[test]
    fn stdio_storage_fingerprint_tracks_db_and_wal_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db_path = temp.path().join("codestory.db");
        std::fs::write(&db_path, b"one").expect("write db");
        let initial = stdio_storage_fingerprint(&db_path);

        std::fs::write(&db_path, b"one-two").expect("rewrite db");
        let rewritten = stdio_storage_fingerprint(&db_path);
        assert_ne!(initial, rewritten);

        std::fs::write(temp.path().join("codestory.db-wal"), b"wal").expect("write wal");
        let with_wal = stdio_storage_fingerprint(&db_path);
        assert_ne!(rewritten, with_wal);
    }
}
