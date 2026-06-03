use anyhow::{Context, Result, bail};
use codestory_contracts::api::{
    LayoutDirection, ListChildrenSymbolsRequest, ListRootSymbolsRequest, NodeId,
    SearchRepoTextMode, SearchRequest, TrailCallerScope, TrailConfigDto, TrailDirection, TrailMode,
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

use crate::args;
use crate::runtime::{self, AmbiguousTargetError, RuntimeContext, map_api_error, resolve_target};
use crate::{
    build_ambiguous_target_error_output, build_query_resolution_output, build_search_hit_output,
};

pub(crate) const BROWSER_TRAIL_DEFAULT_DEPTH: u32 = 2;
pub(crate) const BROWSER_TRAIL_MAX_DEPTH: u32 = 10;
const BROWSER_TRAIL_MAX_NODES: u32 = 80;
const BROWSER_REFERENCES_DEPTH: u32 = 0;
const BROWSER_REFERENCES_MAX_NODES: u32 = 120;
pub(crate) const BROWSER_SYMBOLS_DEFAULT_LIMIT: u32 = 300;
pub(crate) const BROWSER_SYMBOLS_MAX_LIMIT: u32 = 2_000;

pub(crate) fn handle_http_request(runtime: &RuntimeContext, mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request_bytes = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    let mut headers_complete = false;
    loop {
        let read = match stream.read(&mut buffer) {
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                break;
            }
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            break;
        }
        request_bytes.extend_from_slice(&buffer[..read]);
        if request_bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            headers_complete = true;
            break;
        }
        if request_bytes.len() >= 8192 {
            break;
        }
    }
    if !headers_complete {
        return write_http_json(
            &mut stream,
            400,
            &serde_json::json!({"error": "bad request"}),
        );
    }
    let request = String::from_utf8_lossy(&request_bytes);
    let line = request.lines().next().unwrap_or_default();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    if method != "GET" {
        return write_http_json(
            &mut stream,
            405,
            &serde_json::json!({"error": "method not allowed"}),
        );
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = parse_query_string(query);
    match path {
        "/health" => write_http_json(&mut stream, 200, &serde_json::json!({"ok": true})),
        "/search" => {
            let query = params.get("q").cloned().unwrap_or_default();
            let repo_text = params
                .get("repo_text")
                .and_then(|value| search_repo_text_mode_param(value))
                .unwrap_or(SearchRepoTextMode::Auto);
            let limit_per_source = params
                .get("limit")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(10)
                .clamp(1, 100);
            let results = match runtime.browser.search_results(SearchRequest {
                query,
                repo_text,
                limit_per_source,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            }) {
                Ok(results) => results,
                Err(error) => {
                    return write_http_error_json(
                        &mut stream,
                        400,
                        "search_unavailable",
                        map_api_error(error).to_string(),
                    );
                }
            };
            write_http_json(&mut stream, 200, &results)
        }
        "/symbol" => {
            let Some(target) =
                resolve_http_target_from_params(&mut stream, runtime, &params, None)?
            else {
                return Ok(());
            };
            let context = runtime
                .browser
                .symbol_context(target.selected.node_id)
                .map_err(map_api_error)?;
            write_http_json(&mut stream, 200, &context)
        }
        "/definition" => {
            let Some(target) =
                resolve_http_target_from_params(&mut stream, runtime, &params, None)?
            else {
                return Ok(());
            };
            let context = runtime
                .browser
                .definition_context(target.selected.node_id.clone())
                .map_err(map_api_error)?;
            write_http_json(
                &mut stream,
                200,
                &serde_json::json!({
                    "resolution": build_query_resolution_output(&runtime.project_root, &target),
                    "definition": build_search_hit_output(&runtime.project_root, &target.selected, Some(&target.requested), false, &[]),
                    "symbol": context,
                }),
            )
        }
        "/references" => {
            let Some(target) =
                resolve_http_target_from_params(&mut stream, runtime, &params, None)?
            else {
                return Ok(());
            };
            let context = runtime
                .browser
                .references_context(browser_references_config(target.selected.node_id.clone()))
                .map_err(map_api_error)?;
            write_http_json(
                &mut stream,
                200,
                &serde_json::json!({
                    "resolution": build_query_resolution_output(&runtime.project_root, &target),
                    "references": context,
                }),
            )
        }
        "/symbols" => {
            let limit = browser_symbols_limit(params.get("limit").map(String::as_str));
            if let Some(parent_id) = params.get("parent_id").filter(|value| !value.is_empty()) {
                let symbols = runtime
                    .browser
                    .list_children_symbols(ListChildrenSymbolsRequest {
                        parent_id: NodeId(parent_id.clone()),
                    })
                    .map_err(map_api_error)?;
                write_http_json(&mut stream, 200, &symbols)
            } else {
                let symbols = runtime
                    .browser
                    .list_root_symbols(ListRootSymbolsRequest { limit })
                    .map_err(map_api_error)?;
                write_http_json(&mut stream, 200, &symbols)
            }
        }
        "/trail" => {
            let Some(target) =
                resolve_http_target_from_params(&mut stream, runtime, &params, None)?
            else {
                return Ok(());
            };
            let depth = browser_trail_depth(params.get("depth").map(String::as_str));
            let direction = browser_trail_direction(params.get("direction").map(String::as_str));
            let story = browser_bool_param(params.get("story").map(String::as_str));
            let context = runtime
                .browser
                .trail_context(browser_trail_config(
                    target.selected.node_id,
                    depth,
                    direction,
                    story,
                ))
                .map_err(map_api_error)?;
            write_http_json(&mut stream, 200, &context)
        }
        _ => write_http_json(&mut stream, 404, &serde_json::json!({"error": "not found"})),
    }
}

fn resolve_http_target_from_params(
    stream: &mut TcpStream,
    runtime: &RuntimeContext,
    params: &HashMap<String, String>,
    file_filter: Option<&str>,
) -> Result<Option<runtime::ResolvedTarget>> {
    let Some(selection) = http_target_selection_or_error(stream, params)? else {
        return Ok(None);
    };
    resolve_http_target(stream, runtime, selection, file_filter)
}

pub(crate) fn browser_references_config(root_id: NodeId) -> TrailConfigDto {
    TrailConfigDto {
        root_id,
        mode: TrailMode::AllReferencing,
        target_id: None,
        depth: BROWSER_REFERENCES_DEPTH,
        direction: TrailDirection::Incoming,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: Vec::new(),
        show_utility_calls: false,
        hide_speculative: false,
        story: false,
        node_filter: Vec::new(),
        max_nodes: BROWSER_REFERENCES_MAX_NODES,
        layout_direction: LayoutDirection::Horizontal,
    }
}

pub(crate) fn browser_trail_config(
    root_id: NodeId,
    depth: u32,
    direction: TrailDirection,
    story: bool,
) -> TrailConfigDto {
    TrailConfigDto {
        root_id,
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth,
        direction,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: Vec::new(),
        show_utility_calls: false,
        hide_speculative: false,
        story,
        node_filter: Vec::new(),
        max_nodes: BROWSER_TRAIL_MAX_NODES,
        layout_direction: LayoutDirection::Horizontal,
    }
}

fn browser_trail_depth(value: Option<&str>) -> u32 {
    value
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| value.min(BROWSER_TRAIL_MAX_DEPTH))
        .unwrap_or(BROWSER_TRAIL_DEFAULT_DEPTH)
}

fn browser_trail_direction(value: Option<&str>) -> TrailDirection {
    match value {
        Some("incoming") => TrailDirection::Incoming,
        Some("outgoing") => TrailDirection::Outgoing,
        _ => TrailDirection::Both,
    }
}

fn browser_bool_param(value: Option<&str>) -> bool {
    matches!(
        value.map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn browser_symbols_limit(value: Option<&str>) -> Option<u32> {
    Some(
        value
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| value.clamp(1, BROWSER_SYMBOLS_MAX_LIMIT))
            .unwrap_or(BROWSER_SYMBOLS_DEFAULT_LIMIT),
    )
}

fn http_target_selection_or_error(
    stream: &mut TcpStream,
    params: &HashMap<String, String>,
) -> Result<Option<args::TargetSelection>> {
    match target_selection_from_params(params) {
        Ok(selection) => Ok(Some(selection)),
        Err(error) => {
            write_http_error_json(stream, 400, "invalid_target", error.to_string())?;
            Ok(None)
        }
    }
}

fn resolve_http_target(
    stream: &mut TcpStream,
    runtime: &RuntimeContext,
    target: args::TargetSelection,
    file_filter: Option<&str>,
) -> Result<Option<runtime::ResolvedTarget>> {
    match resolve_target(runtime, target, file_filter) {
        Ok(target) => Ok(Some(target)),
        Err(error) => {
            if let Some(ambiguous) = error.downcast_ref::<AmbiguousTargetError>() {
                let output = build_ambiguous_target_error_output(&runtime.project_root, ambiguous);
                write_http_json(stream, 400, &output)?;
            } else {
                write_http_error_json(stream, 400, "target_resolution_failed", error.to_string())?;
            }
            Ok(None)
        }
    }
}

fn target_selection_from_params(params: &HashMap<String, String>) -> Result<args::TargetSelection> {
    if let Some(id) = params.get("id").filter(|value| !value.trim().is_empty()) {
        return Ok(args::TargetSelection::Id(NodeId(id.trim().to_string())));
    }
    let query = params.get("q").cloned().unwrap_or_default();
    if query.trim().is_empty() {
        bail!("Pass `q` or `id`.");
    }
    Ok(args::TargetSelection::Query {
        query,
        choose: query_choose_param(params)?,
    })
}

fn query_choose_param(params: &HashMap<String, String>) -> Result<Option<usize>> {
    params
        .get("choose")
        .map(|value| {
            value.parse::<usize>().with_context(|| {
                format!("Invalid `choose` value `{value}`; expected a positive integer.")
            })
        })
        .transpose()
}

fn write_http_json<T: serde::Serialize>(
    stream: &mut TcpStream,
    status: u16,
    value: &T,
) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn parse_query_string(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((url_decode(key)?, url_decode(value)?))
        })
        .collect()
}

pub(crate) fn search_repo_text_mode_param(value: &str) -> Option<SearchRepoTextMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(SearchRepoTextMode::Auto),
        "on" | "true" | "1" => Some(SearchRepoTextMode::On),
        "off" | "false" | "0" => Some(SearchRepoTextMode::Off),
        _ => None,
    }
}

fn url_decode(value: &str) -> Option<String> {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' => out.push(b' '),
            b'%' if idx + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[idx + 1..idx + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                idx += 2;
            }
            byte => out.push(byte),
        }
        idx += 1;
    }
    String::from_utf8(out).ok()
}

fn write_http_error_json(
    stream: &mut TcpStream,
    status: u16,
    code: &'static str,
    message: impl Into<String>,
) -> Result<()> {
    write_http_json(
        stream,
        status,
        &serde_json::json!({
            "error": {
                "code": code,
                "message": message.into()
            }
        }),
    )
}
