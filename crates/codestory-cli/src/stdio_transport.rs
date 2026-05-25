use anyhow::{Context, Result, bail};
use codestory_contracts::api::{
    AgentAskRequest, AgentPacketRequestDto, AgentResponseModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, GroundingBudgetDto, ListChildrenSymbolsRequest,
    ListRootSymbolsRequest, NodeId, NodeKind, PacketBudgetModeDto, PacketTaskClassDto,
    SearchRepoTextMode, SearchRequest, TrailDirection,
};
use std::io::{BufRead, Write};

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

pub(crate) fn run_stdio_server(runtime: RuntimeContext) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_stdio_message(&runtime, &line) {
            writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn handle_stdio_message(runtime: &RuntimeContext, line: &str) -> Option<serde_json::Value> {
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
            read_stdio_resource(runtime, uri)
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
            return Some(stdio_jsonrpc_tool_call_from_legacy(
                id,
                handle_stdio_tool_call(runtime, &request),
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
    let text = stdio_tool_text(&structured_content);
    serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ],
        "structuredContent": structured_content
    })
}

fn stdio_tool_text(value: &serde_json::Value) -> String {
    if value.get("packet_id").is_some() && value.get("answer").is_some() {
        return stdio_packet_text(value);
    }
    stdio_json_text(value)
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

    append_packet_string_array(&mut text, "gaps", packet.pointer("/sufficiency/gaps"));
    append_packet_string_array(
        &mut text,
        "open_next",
        packet.pointer("/sufficiency/open_next"),
    );
    append_packet_string_array(
        &mut text,
        "follow_up_commands",
        packet.pointer("/sufficiency/follow_up_commands"),
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

fn append_packet_string_array(text: &mut String, title: &str, value: Option<&serde_json::Value>) {
    let Some(items) = value.and_then(|value| value.as_array()) else {
        return;
    };
    if items.is_empty() {
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
    serde_json::json!({
        "protocolVersion": protocol_version,
        "name": "codestory",
        "version": "0.1.0",
        "serverInfo": {
            "name": "codestory",
            "version": "0.1.0"
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
        "packet" => handle_stdio_packet(runtime, request),
        "search" => handle_stdio_search(runtime, request, query),
        "symbol" => handle_stdio_symbol(runtime, request),
        "trail" => handle_stdio_trail(runtime, request),
        "definition" => handle_stdio_definition(runtime, request),
        "references" => handle_stdio_references(runtime, request),
        "symbols" => handle_stdio_symbols(runtime, request),
        "snippet" => handle_stdio_snippet(runtime, request),
        "context" => handle_stdio_context(runtime, request),
        _ => serde_json::json!({"error": "unknown tool"}),
    }
}

fn handle_stdio_packet(runtime: &RuntimeContext, request: &serde_json::Value) -> serde_json::Value {
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
    let include_evidence = request
        .pointer("/params/arguments/include_evidence")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    runtime
        .browser
        .packet(AgentPacketRequestDto {
            question: question.to_string(),
            budget,
            task_class,
            include_evidence,
            latency_budget_ms,
        })
        .map(|packet| serde_json::json!({"result": packet}))
        .unwrap_or_else(|error| serde_json::json!({"error": map_api_error(error).to_string()}))
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

fn handle_stdio_search(
    runtime: &RuntimeContext,
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
    runtime
        .browser
        .search_results(SearchRequest {
            query,
            repo_text,
            limit_per_source,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .map(|result| serde_json::json!({"result": enrich_stdio_search_result(result)}))
        .unwrap_or_else(|error| serde_json::json!({"error": map_api_error(error).to_string()}))
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

fn handle_stdio_trail(runtime: &RuntimeContext, request: &serde_json::Value) -> serde_json::Value {
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
    let story = request
        .pointer("/params/arguments/story")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    resolve_target(runtime, stdio_target_selection(request), None)
        .and_then(|target| {
            runtime
                .browser
                .trail_context(browser_trail_config(
                    target.selected.node_id,
                    depth,
                    direction,
                    story,
                ))
                .map_err(map_api_error)
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
        .unwrap_or_else(|error| serde_json::json!({"error": map_api_error(error).to_string()}))
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

fn read_stdio_resource(runtime: &RuntimeContext, uri: &str) -> serde_json::Value {
    let result = match uri {
        "codestory://status" => read_stdio_status_resource(runtime),
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

fn read_stdio_status_resource(runtime: &RuntimeContext) -> Result<serde_json::Value> {
    let summary = runtime.open_project_summary()?;
    let retrieval = summary.retrieval.as_ref();
    Ok(serde_json::json!({
        "project_root": crate::display::clean_path_string(&runtime.project_root.to_string_lossy()),
        "storage_path": crate::display::clean_path_string(&runtime.storage_path.to_string_lossy()),
        "storage_exists": runtime.storage_path.exists(),
        "retrieval_mode": retrieval.map(|state| state.mode),
        "semantic_ready": retrieval.is_some_and(|state| state.semantic_ready),
        "semantic_doc_count": retrieval.map(|state| state.semantic_doc_count).unwrap_or(0),
        "fallback_reason": retrieval.and_then(|state| state.fallback_reason),
        "fallback_message": retrieval.and_then(|state| state.fallback_message.as_deref()),
        "index_freshness": summary.freshness,
        "recommended_next_calls": [
            {
                "method": "resources/read",
                "uri": "codestory://agent-guide"
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
        ]
    }))
}

fn read_stdio_agent_guide_resource() -> serde_json::Value {
    serde_json::json!({
        "purpose": "Default read-only CodeStory browser loop for local codebase grounding.",
        "recommended_call_sequence": [
            {
                "method": "resources/read",
                "uri": "codestory://status"
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
                    "query": "<symbol-or-task>",
                    "limit": 10
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
        ],
        "safety_notes": [
            "All stdio tools are read-only, non-destructive, idempotent, local-only, and closed-world.",
            "Use packet for broad task questions; use context only after selecting one concrete target.",
            "Use continuation links from search or definition results before broadening retrieval.",
            "Keep search limits bounded; stdio search clamps limit to 1..50.",
            "Treat repo-text hits as evidence until a resolvable symbol is selected."
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
