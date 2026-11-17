//! Loopback HTTP context-budget and deduplication proxy for agent harnesses.
//!
//! `codestory proxy` and `codestory run` interpose strictly on loopback
//! (`127.0.0.1` or `::1`), transform newly appended oversized or duplicate tool
//! outputs deterministically while preserving byte-identical prompt prefixes so
//! upstream KV prefix caches stay intact, and stream responses untouched.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::Path,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::args::{ProxyCommand, RunCommand};

const DEFAULT_UPSTREAM_BASE: &str = "https://api.openai.com/v1";
const MIN_DEDUP_NORMALIZED_BYTES: usize = 256;
const MIN_COMPACT_BUDGET_BYTES: usize = 256;
const MAX_REQUEST_HEADER_BYTES: usize = 256 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 128 * 1024 * 1024;
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(600);
const PROXY_HEALTH_PATH: &str = "/__codestory_proxy/health";
const PROXY_MARKER_PREFIX: &str = "[codestory-proxy:";

#[derive(Clone, Debug)]
struct ProxyConfig {
    listen: SocketAddr,
    upstream_base: String,
    max_tool_bytes: usize,
    dedup: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ProxyStats {
    requests_total: AtomicU64,
    requests_rewritten: AtomicU64,
    tool_outputs_seen: AtomicU64,
    tool_outputs_capped: AtomicU64,
    tool_outputs_deduped: AtomicU64,
    input_bytes_before: AtomicU64,
    input_bytes_after: AtomicU64,
}

#[derive(Debug, Serialize)]
struct ProxyStatsSnapshot {
    status: &'static str,
    requests_total: u64,
    requests_rewritten: u64,
    tool_outputs_seen: u64,
    tool_outputs_capped: u64,
    tool_outputs_deduped: u64,
    input_bytes_before: u64,
    input_bytes_after: u64,
    bytes_saved: u64,
}

impl ProxyStats {
    fn snapshot(&self) -> ProxyStatsSnapshot {
        let before = self.input_bytes_before.load(Ordering::Relaxed);
        let after = self.input_bytes_after.load(Ordering::Relaxed);
        ProxyStatsSnapshot {
            status: "ok",
            requests_total: self.requests_total.load(Ordering::Relaxed),
            requests_rewritten: self.requests_rewritten.load(Ordering::Relaxed),
            tool_outputs_seen: self.tool_outputs_seen.load(Ordering::Relaxed),
            tool_outputs_capped: self.tool_outputs_capped.load(Ordering::Relaxed),
            tool_outputs_deduped: self.tool_outputs_deduped.load(Ordering::Relaxed),
            input_bytes_before: before,
            input_bytes_after: after,
            bytes_saved: before.saturating_sub(after),
        }
    }

    fn format_summary(&self) -> String {
        let s = self.snapshot();
        let pct = if s.input_bytes_before > 0 {
            (s.bytes_saved as f64 * 100.0) / (s.input_bytes_before as f64)
        } else {
            0.0
        };
        format!(
            "[codestory-proxy] requests={} rewritten={} tool_outputs={} capped={} deduped={} tool_bytes={}->{} saved={} ({:.1}%)",
            s.requests_total,
            s.requests_rewritten,
            s.tool_outputs_seen,
            s.tool_outputs_capped,
            s.tool_outputs_deduped,
            s.input_bytes_before,
            s.input_bytes_after,
            s.bytes_saved,
            pct
        )
    }
}

struct ProxyHandle {
    local_addr: SocketAddr,
    stats: Arc<ProxyStats>,
    shutdown: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl ProxyHandle {
    fn base_url(&self) -> String {
        match self.local_addr {
            SocketAddr::V4(v4) => format!("http://127.0.0.1:{}", v4.port()),
            SocketAddr::V6(v6) => format!("http://[::1]:{}", v6.port()),
        }
    }

    fn stop(mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Default)]
struct SessionState {
    /// Maps `(stable_item_key, raw_content_sha256)` to the exact string previously emitted.
    /// Re-emitting identical bytes for unchanged prefix items keeps upstream KV caches hot.
    memo_by_item: HashMap<(String, [u8; 32]), String>,
    /// Maps normalized tool-output content hash to the first item key that emitted it.
    seen_hashes: HashMap<[u8; 32], String>,
}

#[derive(Debug)]
struct ProxyTransformState {
    max_tool_bytes: usize,
    dedup: bool,
    sessions: HashMap<String, SessionState>,
}

impl ProxyTransformState {
    fn new(max_tool_bytes: usize, dedup: bool) -> Self {
        Self {
            max_tool_bytes,
            dedup,
            sessions: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
struct TransformOutcome {
    tracked_payload: bool,
    rewritten: bool,
    tool_outputs_seen: u64,
    tool_outputs_capped: u64,
    tool_outputs_deduped: u64,
    bytes_before: u64,
    bytes_after: u64,
}

#[derive(Debug)]
struct ParsedHttpRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

pub(super) async fn run_proxy(cmd: ProxyCommand) -> Result<()> {
    let config = ProxyConfig {
        listen: parse_loopback_addr(&cmd.listen)?,
        upstream_base: normalize_upstream_base(&cmd.upstream)?,
        max_tool_bytes: cmd.max_tool_bytes,
        dedup: cmd.dedup,
    };
    let handle = spawn_proxy_server(config.clone())?;
    eprintln!(
        "[codestory-proxy] listening={} upstream={} max_tool_bytes={} dedup={}",
        handle.base_url(),
        config.upstream_base,
        config.max_tool_bytes,
        config.dedup
    );
    let _ = tokio::signal::ctrl_c().await;
    let stats = Arc::clone(&handle.stats);
    handle.stop();
    eprintln!("{}", stats.format_summary());
    Ok(())
}

pub(super) async fn run_run(cmd: RunCommand) -> Result<()> {
    if cmd.command.is_empty() {
        bail!("missing child command (example: `codestory run -- codex exec ...`)");
    }
    let upstream = resolve_run_upstream(cmd.upstream.as_deref())?;
    let config = ProxyConfig {
        listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        upstream_base: upstream.clone(),
        max_tool_bytes: cmd.max_tool_bytes,
        dedup: cmd.dedup,
    };
    let handle = spawn_proxy_server(config)?;
    let proxy_base = handle.base_url();
    let openai_base = openai_env_base_url(&proxy_base, &upstream);

    let program = &cmd.command[0];
    let mut args = cmd.command[1..].to_vec();
    maybe_inject_codex_uncompressed_flag(program, &mut args);

    let mut child = std::process::Command::new(program);
    child
        .args(&args)
        .env("OPENAI_BASE_URL", &openai_base)
        .env("ANTHROPIC_BASE_URL", &proxy_base)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let no_proxy = extend_no_proxy(std::env::var("NO_PROXY").ok().as_deref());
    child.env("NO_PROXY", &no_proxy);

    let status = child
        .spawn()
        .with_context(|| format!("launch wrapped command `{program}`"))?
        .wait()
        .with_context(|| format!("wait for wrapped command `{program}`"))?;

    let stats = Arc::clone(&handle.stats);
    handle.stop();
    eprintln!("{}", stats.format_summary());

    if !status.success() {
        let code = status.code().unwrap_or(1);
        std::process::exit(code);
    }
    Ok(())
}

fn maybe_inject_codex_uncompressed_flag(program: &str, args: &mut Vec<String>) {
    let is_codex = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("codex"));
    if !is_codex {
        return;
    }
    let already_configured = args
        .iter()
        .any(|arg| arg.contains("enable_request_compression"));
    if !already_configured {
        args.splice(
            0..0,
            [
                "-c".to_string(),
                "features.enable_request_compression=false".to_string(),
            ],
        );
    }
}

fn resolve_run_upstream(explicit: Option<&str>) -> Result<String> {
    if let Some(url) = explicit {
        return normalize_upstream_base(url);
    }
    for key in ["OPENAI_BASE_URL", "ANTHROPIC_BASE_URL"] {
        if let Some(value) = std::env::var_os(key) {
            let trimmed = value.to_string_lossy().trim().to_string();
            if !trimmed.is_empty() {
                return normalize_upstream_base(&trimmed);
            }
        }
    }
    normalize_upstream_base(DEFAULT_UPSTREAM_BASE)
}

fn openai_env_base_url(proxy_base: &str, upstream_base: &str) -> String {
    if extract_url_path(upstream_base).is_empty() {
        proxy_base.to_string()
    } else {
        format!("{proxy_base}{}", extract_url_path(upstream_base))
    }
}

fn extend_no_proxy(existing: Option<&str>) -> String {
    let mut parts: Vec<String> = existing
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect();
    for host in ["127.0.0.1", "localhost", "::1"] {
        if !parts.iter().any(|part| part.eq_ignore_ascii_case(host)) {
            parts.push(host.to_string());
        }
    }
    parts.join(",")
}

fn parse_loopback_addr(listen: &str) -> Result<SocketAddr> {
    let trimmed = listen.trim();
    if let Ok(addr) = trimmed.parse::<SocketAddr>() {
        if !addr.ip().is_loopback() {
            bail!(
                "refusing non-loopback proxy listen address `{trimmed}` (use 127.0.0.1 or ::1)"
            );
        }
        return Ok(addr);
    }
    let resolved: Vec<SocketAddr> = trimmed
        .to_socket_addrs()
        .with_context(|| format!("resolve proxy listen address `{trimmed}`"))?
        .collect();
    let Some(addr) = resolved.into_iter().find(|addr| addr.ip().is_loopback()) else {
        bail!("proxy listen address `{trimmed}` did not resolve to a loopback address");
    };
    Ok(addr)
}

fn normalize_upstream_base(upstream: &str) -> Result<String> {
    let trimmed = upstream.trim().trim_end_matches('/');
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"));
    if rest.is_none_or(|tail| tail.is_empty()) {
        bail!("upstream URL `{upstream}` must start with http:// or https://");
    }
    Ok(trimmed.to_string())
}

fn extract_url_path(base_url: &str) -> &str {
    let after_scheme = base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url);
    match after_scheme.find('/') {
        Some(idx) => after_scheme[idx..].trim_end_matches('/'),
        None => "",
    }
}

fn join_upstream_url(upstream_base: &str, request_target: &str) -> String {
    let base_path = extract_url_path(upstream_base);
    let target = if request_target.starts_with('/') {
        request_target.to_string()
    } else {
        format!("/{request_target}")
    };
    if !base_path.is_empty() && target.starts_with(base_path) {
        let suffix = &target[base_path.len()..];
        if suffix.is_empty() || suffix.starts_with('/') || suffix.starts_with('?') {
            return format!("{upstream_base}{suffix}");
        }
    }
    format!("{upstream_base}{target}")
}

fn spawn_proxy_server(config: ProxyConfig) -> Result<ProxyHandle> {
    if !config.listen.ip().is_loopback() {
        bail!("proxy bind address must be loopback");
    }
    let listener = TcpListener::bind(config.listen)
        .with_context(|| format!("bind loopback proxy at {}", config.listen))?;
    listener
        .set_nonblocking(true)
        .context("set proxy listener nonblocking")?;
    let local_addr = listener.local_addr()?;
    let stats = Arc::new(ProxyStats::default());
    let shutdown = Arc::new(AtomicBool::new(false));
    let transform = Arc::new(Mutex::new(ProxyTransformState::new(
        config.max_tool_bytes,
        config.dedup,
    )));
    let agent = Arc::new(
        ureq::AgentBuilder::new()
            .timeout_connect(UPSTREAM_CONNECT_TIMEOUT)
            .timeout_read(UPSTREAM_READ_TIMEOUT)
            .build(),
    );

    let worker_stats = Arc::clone(&stats);
    let worker_shutdown = Arc::clone(&shutdown);
    let upstream_base = config.upstream_base.clone();
    let join_handle = thread::spawn(move || {
        while !worker_shutdown.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _peer)) => {
                    let _ = stream.set_nonblocking(false);
                    let upstream = upstream_base.clone();
                    let stats = Arc::clone(&worker_stats);
                    let transform = Arc::clone(&transform);
                    let agent = Arc::clone(&agent);
                    thread::spawn(move || {
                        let _ = handle_connection(stream, &upstream, &stats, &transform, &agent);
                    });
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    });

    Ok(ProxyHandle {
        local_addr,
        stats,
        shutdown,
        join_handle: Some(join_handle),
    })
}

fn handle_connection(
    mut stream: TcpStream,
    upstream_base: &str,
    stats: &ProxyStats,
    transform: &Mutex<ProxyTransformState>,
    agent: &ureq::Agent,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone().context("clone client TCP stream")?);
    let req = match read_http_request(&mut reader, &mut stream)? {
        Some(req) => req,
        None => return Ok(()),
    };

    if req.method.eq_ignore_ascii_case("GET") && req.target == PROXY_HEALTH_PATH {
        let body = serde_json::to_vec(&stats.snapshot())?;
        return write_simple_response(&mut stream, 200, "OK", "application/json", &body);
    }

    stats.requests_total.fetch_add(1, Ordering::Relaxed);
    let out_body = maybe_rewrite_request_body(&req, stats, transform);
    let upstream_url = join_upstream_url(upstream_base, &req.target);

    let mut request = agent.request(&req.method, &upstream_url);
    for (name, value) in &req.headers {
        if is_forwarded_request_header(name) {
            request = request.set(name, value);
        }
    }
    request = request.set("Accept-Encoding", "identity");

    let has_body = !out_body.is_empty()
        || matches!(
            req.method.to_ascii_uppercase().as_str(),
            "POST" | "PUT" | "PATCH"
        );
    let result = if has_body {
        request.send_bytes(&out_body)
    } else {
        request.call()
    };

    match result {
        Ok(resp) => stream_upstream_response(&mut stream, &req.method, resp),
        Err(ureq::Error::Status(_, resp)) => stream_upstream_response(&mut stream, &req.method, resp),
        Err(ureq::Error::Transport(err)) => {
            let payload = serde_json::json!({
                "error": {
                    "type": "codestory_proxy_upstream_error",
                    "message": err.to_string(),
                }
            });
            let body = serde_json::to_vec(&payload)?;
            write_simple_response(&mut stream, 502, "Bad Gateway", "application/json", &body)
        }
    }
}

fn read_http_request(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
) -> Result<Option<ParsedHttpRequest>> {
    let mut header_bytes = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            if header_bytes.is_empty() {
                return Ok(None);
            }
            bail!("unexpected EOF while reading HTTP headers");
        }
        header_bytes.extend_from_slice(line.as_bytes());
        if header_bytes.len() > MAX_REQUEST_HEADER_BYTES {
            bail!("HTTP headers exceed {MAX_REQUEST_HEADER_BYTES} bytes");
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
    }

    let header_text = String::from_utf8(header_bytes).context("HTTP headers are not UTF-8")?;
    let mut lines = header_text.lines();
    let Some(request_line) = lines.next() else {
        return Ok(None);
    };
    let mut parts = request_line.split_whitespace();
    let Some(method) = parts.next() else {
        bail!("missing HTTP method");
    };
    let Some(target) = parts.next() else {
        bail!("missing HTTP target");
    };

    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut is_chunked = false;
    let mut expect_continue = false;
    for raw in lines {
        if raw.is_empty() {
            continue;
        }
        let Some((name, value)) = raw.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            if let Ok(len) = value.parse::<usize>() {
                content_length = Some(len);
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            is_chunked = true;
        } else if name.eq_ignore_ascii_case("expect")
            && value.to_ascii_lowercase().contains("100-continue")
        {
            expect_continue = true;
        }
        headers.push((name, value));
    }

    if expect_continue {
        writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        writer.flush()?;
    }

    let body = if is_chunked {
        read_chunked_body(reader)?
    } else if let Some(len) = content_length {
        if len > MAX_REQUEST_BODY_BYTES {
            bail!("HTTP request body length {len} exceeds {MAX_REQUEST_BODY_BYTES} bytes");
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        buf
    } else {
        Vec::new()
    };

    Ok(Some(ParsedHttpRequest {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body,
    }))
}

fn read_chunked_body(reader: &mut BufReader<TcpStream>) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            bail!("unexpected EOF in chunked request body");
        }
        let size_hex = line
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_hex, 16)
            .with_context(|| format!("invalid HTTP chunk size `{size_hex}`"))?;
        if size == 0 {
            loop {
                line.clear();
                let n = reader.read_line(&mut line)?;
                if n == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
            }
            break;
        }
        if body.len().saturating_add(size) > MAX_REQUEST_BODY_BYTES {
            bail!("chunked HTTP body exceeds {MAX_REQUEST_BODY_BYTES} bytes");
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
    }
    Ok(body)
}

fn is_forwarded_request_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "connection"
            | "keep-alive"
            | "proxy-connection"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
            | "expect"
            | "accept-encoding"
    )
}

fn is_forwarded_response_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "content-length"
            | "content-encoding"
            | "te"
            | "trailers"
            | "upgrade"
    )
}

fn stream_upstream_response(
    stream: &mut TcpStream,
    request_method: &str,
    resp: ureq::Response,
) -> Result<()> {
    let status = resp.status();
    let status_text = resp.status_text().to_string();
    let mut head = format!("HTTP/1.1 {status} {status_text}\r\n");
    for name in resp.headers_names() {
        if !is_forwarded_response_header(&name) {
            continue;
        }
        for value in resp.all(&name) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str("Connection: close\r\n");

    let no_body = status == 204
        || status == 304
        || (100..200).contains(&status)
        || request_method.eq_ignore_ascii_case("HEAD");
    if no_body {
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.flush()?;
        return Ok(());
    }

    head.push_str("Transfer-Encoding: chunked\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()?;

    let mut reader = resp.into_reader();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            stream.write_all(b"0\r\n\r\n")?;
            stream.flush()?;
            break;
        }
        let header = format!("{n:x}\r\n");
        stream.write_all(header.as_bytes())?;
        stream.write_all(&buf[..n])?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
    }
    Ok(())
}

fn write_simple_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn header_value<'a>(headers: &'a [(String, String)], target: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(target))
        .map(|(_, value)| value.as_str())
}

fn maybe_rewrite_request_body(
    req: &ParsedHttpRequest,
    stats: &ProxyStats,
    transform: &Mutex<ProxyTransformState>,
) -> Vec<u8> {
    if !req.method.eq_ignore_ascii_case("POST") || req.body.is_empty() {
        return req.body.clone();
    }
    if let Some(encoding) = header_value(&req.headers, "content-encoding") {
        let lower = encoding.trim().to_ascii_lowercase();
        if !lower.is_empty() && lower != "identity" {
            return req.body.clone();
        }
    }
    if let Some(ct) = header_value(&req.headers, "content-type")
        && !ct.to_ascii_lowercase().contains("application/json")
    {
        return req.body.clone();
    }
    let Ok(mut payload) = serde_json::from_slice::<Value>(&req.body) else {
        return req.body.clone();
    };
    let outcome = {
        let mut guard = transform.lock().expect("proxy transform lock");
        guard.transform_request_json(&mut payload, &req.headers)
    };
    if !outcome.tracked_payload {
        return req.body.clone();
    }
    stats
        .tool_outputs_seen
        .fetch_add(outcome.tool_outputs_seen, Ordering::Relaxed);
    stats
        .tool_outputs_capped
        .fetch_add(outcome.tool_outputs_capped, Ordering::Relaxed);
    stats
        .tool_outputs_deduped
        .fetch_add(outcome.tool_outputs_deduped, Ordering::Relaxed);
    stats
        .input_bytes_before
        .fetch_add(outcome.bytes_before, Ordering::Relaxed);
    stats
        .input_bytes_after
        .fetch_add(outcome.bytes_after, Ordering::Relaxed);
    if outcome.rewritten {
        stats.requests_rewritten.fetch_add(1, Ordering::Relaxed);
    }
    // Always serialize tracked LLM JSON payloads consistently once touched by the proxy
    // so turn 1 and turn 2+ keep identical key ordering for upstream KV prefix cache hits.
    serde_json::to_vec(&payload).unwrap_or_else(|_| req.body.clone())
}

impl ProxyTransformState {
    fn transform_request_json(
        &mut self,
        payload: &mut Value,
        headers: &[(String, String)],
    ) -> TransformOutcome {
        let Some(obj) = payload.as_object() else {
            return TransformOutcome::default();
        };
        let has_responses_input = obj.get("input").is_some_and(Value::is_array);
        let has_messages = obj.get("messages").is_some_and(Value::is_array);
        if !has_responses_input && !has_messages {
            return TransformOutcome::default();
        }

        let session_key = derive_session_key(obj, headers);
        let max_tool_bytes = self.max_tool_bytes;
        let dedup = self.dedup;
        let session = self.sessions.entry(session_key).or_default();

        let mut outcome = TransformOutcome {
            tracked_payload: true,
            ..TransformOutcome::default()
        };

        let mut targeted_calls = HashSet::new();
        if let Some(Value::Array(items)) = payload.get("input") {
            for item in items {
                collect_responses_targeted_call_id(item, &mut targeted_calls);
            }
        }
        if let Some(Value::Array(messages)) = payload.get("messages") {
            for msg in messages {
                collect_message_targeted_call_ids(msg, &mut targeted_calls);
            }
        }

        if let Some(Value::Array(items)) = payload.get_mut("input") {
            for (idx, item) in items.iter_mut().enumerate() {
                transform_responses_item(
                    item,
                    idx,
                    session,
                    max_tool_bytes,
                    &targeted_calls,
                    dedup,
                    &mut outcome,
                );
            }
        }
        if let Some(Value::Array(messages)) = payload.get_mut("messages") {
            for (idx, msg) in messages.iter_mut().enumerate() {
                transform_chat_or_anthropic_message(
                    msg,
                    idx,
                    session,
                    max_tool_bytes,
                    &targeted_calls,
                    dedup,
                    &mut outcome,
                );
            }
        }
        outcome
    }
}

fn effective_budget_for_call(base_budget: usize, call_id: Option<&str>, targeted_calls: &HashSet<String>) -> usize {
    if base_budget == 0 {
        return 0;
    }
    if call_id.is_some_and(|id| targeted_calls.contains(id)) {
        base_budget.saturating_mul(2)
    } else {
        base_budget
    }
}

fn is_targeted_lookup_tool(name: &str, payload_text: &str) -> bool {
    if matches!(name, "slice" | "read" | "refs" | "find") || name.starts_with("mcp__codestory") {
        return true;
    }
    let command_text = if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(payload_text) {
        map.get("cmd")
            .or_else(|| map.get("command"))
            .and_then(Value::as_str)
            .unwrap_or(payload_text)
            .to_string()
    } else {
        payload_text.to_string()
    };
    let lower = command_text.to_ascii_lowercase();
    let has_cs_lookup = ["cs slice", "cs read", "cs refs", "cs find", "codestory-cli slice", "codestory-cli read", "codestory-cli refs", "codestory-cli find"]
        .iter()
        .any(|pat| lower.contains(pat));
    if !has_cs_lookup {
        return false;
    }
    !["write-test", "pytest", "go test", "cargo test", "cargo check", "npm test", "git diff"]
        .iter()
        .any(|pat| lower.contains(pat))
}

fn collect_responses_targeted_call_id(item: &Value, targeted_calls: &mut HashSet<String>) {
    let Some(obj) = item.as_object() else {
        return;
    };
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
    if item_type != "function_call" && item_type != "custom_tool_call" {
        return;
    }
    let Some(call_id) = obj.get("call_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return;
    };
    let name = obj.get("name").and_then(Value::as_str).unwrap_or("");
    let raw_args = obj
        .get("arguments")
        .or_else(|| obj.get("input"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if is_targeted_lookup_tool(name, raw_args) {
        targeted_calls.insert(call_id.to_string());
    }
}

fn collect_message_targeted_call_ids(msg: &Value, targeted_calls: &mut HashSet<String>) {
    let Some(obj) = msg.as_object() else {
        return;
    };
    if obj.get("role").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    if let Some(Value::Array(tool_calls)) = obj.get("tool_calls") {
        for tc in tool_calls {
            let Some(tc_obj) = tc.as_object() else { continue };
            let Some(id) = tc_obj.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                continue;
            };
            let func = tc_obj.get("function").and_then(Value::as_object);
            let name = func.and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or("");
            let args = func.and_then(|f| f.get("arguments")).and_then(Value::as_str).unwrap_or("");
            if is_targeted_lookup_tool(name, args) {
                targeted_calls.insert(id.to_string());
            }
        }
    }
    if let Some(Value::Array(blocks)) = obj.get("content") {
        for block in blocks {
            let Some(b) = block.as_object() else { continue };
            if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(id) = b.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                continue;
            };
            let name = b.get("name").and_then(Value::as_str).unwrap_or("");
            let input_json = b.get("input").map(|v| v.to_string()).unwrap_or_default();
            if is_targeted_lookup_tool(name, &input_json) {
                targeted_calls.insert(id.to_string());
            }
        }
    }
}

fn transform_responses_item(
    item: &mut Value,
    idx: usize,
    session: &mut SessionState,
    max_tool_bytes: usize,
    targeted_calls: &HashSet<String>,
    dedup: bool,
    outcome: &mut TransformOutcome,
) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
    if item_type != "function_call_output" && item_type != "custom_tool_call_output" {
        return;
    }
    let call_id = obj.get("call_id").and_then(Value::as_str).filter(|id| !id.is_empty());
    let budget = effective_budget_for_call(max_tool_bytes, call_id, targeted_calls);
    let item_key = call_id
        .map(|id| format!("resp:{id}"))
        .unwrap_or_else(|| format!("resp-idx:{idx}"));

    if let Some(output) = obj.get_mut("output") {
        transform_output_value(output, &item_key, session, budget, dedup, outcome);
    }
}

fn transform_chat_or_anthropic_message(
    msg: &mut Value,
    idx: usize,
    session: &mut SessionState,
    max_tool_bytes: usize,
    targeted_calls: &HashSet<String>,
    dedup: bool,
    outcome: &mut TransformOutcome,
) {
    let Some(obj) = msg.as_object_mut() else {
        return;
    };
    let role = obj.get("role").and_then(Value::as_str).unwrap_or("");
    if role == "tool" {
        let call_id = obj
            .get("tool_call_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let budget = effective_budget_for_call(max_tool_bytes, call_id, targeted_calls);
        let item_key = call_id
            .map(|id| format!("chat:{id}"))
            .unwrap_or_else(|| format!("chat-idx:{idx}"));
        if let Some(content) = obj.get_mut("content") {
            transform_output_value(content, &item_key, session, budget, dedup, outcome);
        }
        return;
    }
    if role == "user"
        && let Some(Value::Array(blocks)) = obj.get_mut("content")
    {
        for (block_idx, block) in blocks.iter_mut().enumerate() {
            let Some(block_obj) = block.as_object_mut() else {
                continue;
            };
            if block_obj.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let call_id = block_obj
                .get("tool_use_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty());
            let budget = effective_budget_for_call(max_tool_bytes, call_id, targeted_calls);
            let item_key = call_id
                .map(|id| format!("anth:{id}"))
                .unwrap_or_else(|| format!("anth-idx:{idx}:{block_idx}"));
            if let Some(content) = block_obj.get_mut("content") {
                transform_output_value(content, &item_key, session, budget, dedup, outcome);
            }
        }
    }
}

fn transform_output_value(
    value: &mut Value,
    item_key: &str,
    session: &mut SessionState,
    max_tool_bytes: usize,
    dedup: bool,
    outcome: &mut TransformOutcome,
) {
    match value {
        Value::String(text) => {
            if let Some(replacement) =
                rewrite_tool_text(text, item_key, session, max_tool_bytes, dedup, outcome)
            {
                *text = replacement;
            }
        }
        Value::Array(parts) => {
            for (part_idx, part) in parts.iter_mut().enumerate() {
                let Some(part_obj) = part.as_object_mut() else {
                    continue;
                };
                if let Some(Value::String(text)) = part_obj.get_mut("text") {
                    let sub_key = format!("{item_key}:part{part_idx}");
                    if let Some(replacement) =
                        rewrite_tool_text(text, &sub_key, session, max_tool_bytes, dedup, outcome)
                    {
                        *text = replacement;
                    }
                }
            }
        }
        _ => {}
    }
}

fn rewrite_tool_text(
    raw: &str,
    item_key: &str,
    session: &mut SessionState,
    max_tool_bytes: usize,
    dedup: bool,
    outcome: &mut TransformOutcome,
) -> Option<String> {
    let raw_len = raw.len() as u64;
    outcome.tool_outputs_seen += 1;
    outcome.bytes_before += raw_len;

    if raw.starts_with(PROXY_MARKER_PREFIX) {
        outcome.bytes_after += raw_len;
        return None;
    }

    let raw_hash = sha256_bytes(raw.as_bytes());
    if let Some(cached) = session
        .memo_by_item
        .get(&(item_key.to_string(), raw_hash))
        .cloned()
    {
        outcome.bytes_after += cached.len() as u64;
        if cached != raw {
            outcome.rewritten = true;
            return Some(cached);
        }
        return None;
    }

    let mut chosen = raw.to_string();
    let normalized = normalize_for_dedup(raw);
    let norm_hash = sha256_bytes(normalized.as_bytes());
    if dedup && normalized.len() >= MIN_DEDUP_NORMALIZED_BYTES {
        if let Some(first_key) = session.seen_hashes.get(&norm_hash) {
            if first_key != item_key {
                let marker = format!(
                    "{PROXY_MARKER_PREFIX} duplicate tool output identical to earlier call {first_key}; {} bytes omitted]",
                    raw.len()
                );
                if marker.len() < raw.len() {
                    chosen = marker;
                    outcome.tool_outputs_deduped += 1;
                    outcome.rewritten = true;
                }
            }
        } else {
            session.seen_hashes.insert(norm_hash, item_key.to_string());
        }
    }

    if chosen == raw && max_tool_bytes > 0 && raw.len() > max_tool_bytes {
        let budget = max_tool_bytes.max(MIN_COMPACT_BUDGET_BYTES);
        let compacted = compact_tool_output(raw, budget);
        if compacted.len() < raw.len() {
            chosen = compacted;
            outcome.tool_outputs_capped += 1;
            outcome.rewritten = true;
        }
    }

    session
        .memo_by_item
        .insert((item_key.to_string(), raw_hash), chosen.clone());
    outcome.bytes_after += chosen.len() as u64;
    if chosen != raw { Some(chosen) } else { None }
}

fn strip_volatile_json_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("chunk_id");
            map.remove("wall_time_seconds");
            map.remove("wall_time");
            map.remove("original_token_count");
            if map.get("exit_code").and_then(Value::as_i64) == Some(0) {
                map.remove("exit_code");
            }
            for v in map.values_mut() {
                strip_volatile_json_fields(v);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_volatile_json_fields(item);
            }
        }
        _ => {}
    }
}

fn normalize_for_dedup(raw: &str) -> String {
    let trimmed_raw = raw.trim();
    if (trimmed_raw.starts_with('{') || trimmed_raw.starts_with('['))
        && let Ok(mut parsed) = serde_json::from_str::<Value>(trimmed_raw)
    {
        strip_volatile_json_fields(&mut parsed);
        if let Ok(canonical) = serde_json::to_string(&parsed) {
            return canonical;
        }
    }
    // Codex exec wraps command output with per-call headers like `Chunk ID:` and
    // `Wall time:`; strip volatile lines while preserving non-zero exit codes.
    let mut lines = raw.lines().peekable();
    let mut kept = Vec::new();
    let mut in_wrapper = true;
    for line in lines.by_ref() {
        let trimmed = line.trim();
        if in_wrapper {
            if trimmed.starts_with("Chunk ID:")
                || trimmed.starts_with("Wall time:")
                || trimmed.starts_with("Wall time ")
                || trimmed.starts_with("Script completed")
                || trimmed.starts_with("Original token count:")
                || trimmed == "Output:"
            {
                continue;
            }
            if let Some(code_str) = trimmed.strip_prefix("Process exited with code ") {
                if code_str.trim() == "0" {
                    continue;
                }
                kept.push(trimmed);
                in_wrapper = false;
                continue;
            }
            if trimmed.is_empty() && kept.is_empty() {
                continue;
            }
            in_wrapper = false;
        }
        kept.push(line.trim_end());
    }
    kept.join("\n").trim().to_string()
}

fn compact_tool_output(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_string();
    }
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    if lines.len() <= 4 {
        return hard_head_tail_cap(raw, max_bytes);
    }

    let head_budget = (max_bytes * 50) / 100;
    let diag_budget = (max_bytes * 15) / 100;
    let tail_budget = (max_bytes * 25) / 100;

    let mut head_end = 0usize;
    let mut head_used = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 && head_used + line.len() > head_budget {
            break;
        }
        head_used += line.len();
        head_end = idx + 1;
        if head_used >= head_budget {
            break;
        }
    }

    let mut tail_start = lines.len();
    let mut tail_used = 0usize;
    for idx in (head_end..lines.len()).rev() {
        let len = lines[idx].len();
        if tail_start < lines.len() && tail_used + len > tail_budget {
            break;
        }
        tail_used += len;
        tail_start = idx;
        if tail_used >= tail_budget {
            break;
        }
    }

    if head_end >= tail_start {
        return hard_head_tail_cap(raw, max_bytes);
    }

    let mut diag_indices = Vec::new();
    let mut diag_used = 0usize;
    for (idx, line) in lines
        .iter()
        .enumerate()
        .take(tail_start)
        .skip(head_end)
    {
        if is_diagnostic_line(line) && diag_used + line.len() + 12 <= diag_budget {
            diag_used += line.len() + 12;
            diag_indices.push(idx);
        }
    }

    let mut included: HashSet<usize> = (0..head_end).chain(tail_start..lines.len()).collect();
    for &idx in &diag_indices {
        included.insert(idx);
    }
    let omitted_lines = lines.len().saturating_sub(included.len());
    let omitted_bytes: usize = lines
        .iter()
        .enumerate()
        .filter(|(idx, _)| !included.contains(idx))
        .map(|(_, line)| line.len())
        .sum();

    let marker = format!(
        "\n{PROXY_MARKER_PREFIX} omitted {omitted_lines} middle lines / {omitted_bytes} bytes of {}]\n",
        raw.len()
    );

    let mut out = String::with_capacity(max_bytes);
    for line in &lines[..head_end] {
        out.push_str(line);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !diag_indices.is_empty() {
        out.push_str("[codestory-proxy: middle diagnostic lines]\n");
        for idx in diag_indices {
            out.push_str(&format!("L{}: {}", idx + 1, lines[idx]));
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out.push_str(&marker);
    for line in &lines[tail_start..] {
        if out.len() + line.len() > max_bytes {
            break;
        }
        out.push_str(line);
    }

    if out.len() > max_bytes {
        return hard_head_tail_cap(raw, max_bytes);
    }
    out
}

fn hard_head_tail_cap(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_string();
    }
    let reserve = 96usize;
    if max_bytes <= reserve + 32 {
        let end = floor_char_boundary(raw, max_bytes);
        return raw[..end].to_string();
    }
    let usable = max_bytes - reserve;
    let head_bytes = (usable * 60) / 100;
    let tail_bytes = usable - head_bytes;
    let head_end = floor_char_boundary(raw, head_bytes);
    let tail_start = ceil_char_boundary(raw, raw.len().saturating_sub(tail_bytes));
    if head_end >= tail_start {
        let end = floor_char_boundary(raw, max_bytes);
        return raw[..end].to_string();
    }
    let omitted = tail_start - head_end;
    let candidate = format!(
        "{}\n{PROXY_MARKER_PREFIX} truncated {omitted} bytes of {}]\n{}",
        &raw[..head_end],
        raw.len(),
        &raw[tail_start..]
    );
    if candidate.len() <= max_bytes {
        candidate
    } else {
        let end = floor_char_boundary(&candidate, max_bytes);
        candidate[..end].to_string()
    }
}

fn is_diagnostic_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.len() > 240 {
        return false;
    }
    trimmed.contains("error[")
        || trimmed.contains("error:")
        || trimmed.contains("Error:")
        || trimmed.contains("ERROR")
        || trimmed.contains("FAILED")
        || trimmed.contains("panic")
        || trimmed.contains("Traceback")
        || trimmed.contains("AssertionError")
}

fn floor_char_boundary(text: &str, idx: usize) -> usize {
    let mut pos = idx.min(text.len());
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn ceil_char_boundary(text: &str, idx: usize) -> usize {
    let mut pos = idx.min(text.len());
    while pos < text.len() && !text.is_char_boundary(pos) {
        pos += 1;
    }
    pos
}

fn derive_session_key(obj: &serde_json::Map<String, Value>, headers: &[(String, String)]) -> String {
    for key in ["prompt_cache_key", "previous_response_id"] {
        if let Some(val) = obj.get(key).and_then(Value::as_str)
            && !val.is_empty()
        {
            return format!("body:{key}:{val}");
        }
    }
    for hdr in ["x-codex-window-id", "x-codex-session-id", "x-session-id"] {
        if let Some(val) = header_value(headers, hdr)
            && !val.is_empty()
        {
            return format!("hdr:{hdr}:{val}");
        }
    }
    let mut hasher = Sha256::new();
    for key in ["model", "instructions", "system"] {
        if let Some(val) = obj.get(key)
            && let Ok(bytes) = serde_json::to_vec(val)
        {
            hasher.update(&bytes);
        }
    }
    if let Some(first_input) = obj
        .get("input")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        && let Ok(bytes) = serde_json::to_vec(first_input)
    {
        hasher.update(&bytes);
    } else if let Some(first_msg) = obj
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        && let Ok(bytes) = serde_json::to_vec(first_msg)
    {
        hasher.update(&bytes);
    }
    format!("hash:{:x}", hasher.finalize())
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_non_loopback_bind_addresses() {
        assert!(parse_loopback_addr("127.0.0.1:0").is_ok());
        assert!(parse_loopback_addr("[::1]:8080").is_ok());
        assert!(parse_loopback_addr("0.0.0.0:8080").is_err());
        assert!(parse_loopback_addr("192.168.1.20:8080").is_err());
    }

    #[test]
    fn normalization_ignores_codex_exec_wrapper_metadata_but_keeps_nonzero_exit() {
        let body = "a\n".repeat(200);
        let run1 = format!(
            "Chunk ID: abc123\nWall time: 0.0412 seconds\nProcess exited with code 0\nOriginal token count: 90\nOutput:\n{body}"
        );
        let run2 = format!(
            "Chunk ID: def999\nWall time: 1.2500 seconds\nProcess exited with code 0\nOriginal token count: 90\nOutput:\n{body}"
        );
        let run_fail = format!(
            "Chunk ID: xyz000\nWall time: 0.1000 seconds\nProcess exited with code 2\nOriginal token count: 90\nOutput:\n{body}"
        );
        assert_eq!(normalize_for_dedup(&run1), normalize_for_dedup(&run2));
        assert_ne!(normalize_for_dedup(&run1), normalize_for_dedup(&run_fail));
    }

    #[test]
    fn prefix_stays_byte_identical_across_responses_turns_with_cap_and_dedup() {
        let mut state = ProxyTransformState::new(1024, true);
        let headers = vec![("x-codex-session-id".to_string(), "sess-1".to_string())];
        let huge_output = (0..120)
            .map(|i| {
                if i == 60 {
                    "error[E0425]: cannot find value `missing_symbol` in this scope\n".to_string()
                } else {
                    format!("line {i:03}: checking workspace state and dependencies in detail\n")
                }
            })
            .collect::<String>();

        let mut turn1 = json!({
            "model": "gpt-6.1-sol",
            "instructions": "You are a coding assistant.",
            "input": [
                {"role": "user", "content": "Inspect build failure"},
                {"type": "function_call_output", "call_id": "call_1", "output": huge_output}
            ]
        });
        let out1 = state.transform_request_json(&mut turn1, &headers);
        assert!(out1.rewritten);
        assert_eq!(out1.tool_outputs_capped, 1);
        let prefix_turn1 = turn1["input"][1]["output"].as_str().unwrap().to_string();
        assert!(prefix_turn1.len() <= 1024);
        assert!(prefix_turn1.contains("error[E0425]"));
        assert!(prefix_turn1.contains("line 119:"));

        let repeated_small = "Chunk ID: c1\nWall time: 0.01s\nOutput:\n".to_string() + &"same\n".repeat(90);
        let mut turn2 = json!({
            "model": "gpt-6.1-sol",
            "instructions": "You are a coding assistant.",
            "input": [
                {"role": "user", "content": "Inspect build failure"},
                {"type": "function_call_output", "call_id": "call_1", "output": huge_output},
                {"type": "function_call_output", "call_id": "call_2", "output": repeated_small}
            ]
        });
        state.transform_request_json(&mut turn2, &headers);
        assert_eq!(
            turn2["input"][1]["output"].as_str().unwrap(),
            prefix_turn1,
            "turn 1 prefix item must stay byte-identical on turn 2"
        );

        let repeated_again = "Chunk ID: c2\nWall time: 0.44s\nOutput:\n".to_string() + &"same\n".repeat(90);
        let mut turn3 = json!({
            "model": "gpt-6.1-sol",
            "instructions": "You are a coding assistant.",
            "input": [
                {"role": "user", "content": "Inspect build failure"},
                {"type": "function_call_output", "call_id": "call_1", "output": huge_output},
                {"type": "function_call_output", "call_id": "call_2", "output": repeated_small},
                {"type": "function_call_output", "call_id": "call_3", "output": repeated_again}
            ]
        });
        let out3 = state.transform_request_json(&mut turn3, &headers);
        assert_eq!(out3.tool_outputs_deduped, 1);
        assert_eq!(turn3["input"][1]["output"].as_str().unwrap(), prefix_turn1);
        assert_eq!(turn3["input"][2]["output"].as_str().unwrap(), repeated_small);
        assert!(turn3["input"][3]["output"]
            .as_str()
            .unwrap()
            .contains("duplicate tool output identical to earlier call resp:call_2"));
    }

    #[test]
    fn supports_anthropic_tool_result_and_openai_chat_tool_messages() {
        let mut state = ProxyTransformState::new(512, true);
        let repeated = "alpha beta gamma delta\n".repeat(30);

        let mut anth = json!({
            "model": "claude-sonnet",
            "system": "sys",
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "tu_1", "content": repeated.clone()},
                    {"type": "tool_result", "tool_use_id": "tu_2", "content": repeated.clone()}
                ]}
            ]
        });
        let out_anth = state.transform_request_json(&mut anth, &[]);
        assert_eq!(out_anth.tool_outputs_deduped, 1);
        assert!(anth["messages"][0]["content"][1]["content"]
            .as_str()
            .unwrap()
            .contains("anth:tu_1"));

        let mut chat = json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user", "content": "check"},
                {"role": "tool", "tool_call_id": "tc_1", "content": repeated.clone()},
                {"role": "tool", "tool_call_id": "tc_2", "content": repeated}
            ]
        });
        let out_chat = state.transform_request_json(&mut chat, &[]);
        assert_eq!(out_chat.tool_outputs_deduped, 1);
        assert!(chat["messages"][2]["content"]
            .as_str()
            .unwrap()
            .contains("chat:tc_1"));
    }

    #[test]
    fn loopback_proxy_streams_sse_preserves_prefix_and_forwards_upstream_status() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("bind mock upstream");
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let captured_bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured_bodies);

        let upstream_thread = thread::spawn(move || {
            for idx in 0..3 {
                let (mut conn, _) = upstream_listener.accept().unwrap();
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let req = read_http_request(&mut reader, &mut conn).unwrap().unwrap();
                if idx == 2 {
                    let err_body = br#"{"error":"rate_limited"}"#;
                    let head = format!(
                        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        err_body.len()
                    );
                    conn.write_all(head.as_bytes()).unwrap();
                    conn.write_all(err_body).unwrap();
                    continue;
                }
                let parsed: Value = serde_json::from_slice(&req.body).unwrap();
                captured_clone.lock().unwrap().push(parsed);
                conn.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
                for event in ["data: {\"type\":\"delta\"}\n\n", "data: [DONE]\n\n"] {
                    let chunk = format!("{:x}\r\n{event}\r\n", event.len());
                    conn.write_all(chunk.as_bytes()).unwrap();
                    conn.flush().unwrap();
                }
                conn.write_all(b"0\r\n\r\n").unwrap();
                conn.flush().unwrap();
            }
        });

        let handle = spawn_proxy_server(ProxyConfig {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            upstream_base: format!("http://127.0.0.1:{}/v1", upstream_addr.port()),
            max_tool_bytes: 700,
            dedup: true,
        })
        .expect("spawn proxy");
        let proxy_url = format!("{}/v1/responses", handle.base_url());
        let big = (0..60).map(|i| format!("row {i:02} diagnostic payload\n")).collect::<String>();

        let resp1 = ureq::post(&proxy_url)
            .set("Content-Type", "application/json")
            .send_bytes(
                &serde_json::to_vec(&json!({
                    "model": "m",
                    "input": [
                        {"role": "user", "content": "hi"},
                        {"type": "function_call_output", "call_id": "c1", "output": big}
                    ]
                }))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(resp1.status(), 200);
        assert!(resp1.into_string().unwrap().contains("[DONE]"));

        let resp2 = ureq::post(&proxy_url)
            .set("Content-Type", "application/json")
            .send_bytes(
                &serde_json::to_vec(&json!({
                    "model": "m",
                    "input": [
                        {"role": "user", "content": "hi"},
                        {"type": "function_call_output", "call_id": "c1", "output": big},
                        {"type": "function_call_output", "call_id": "c2", "output": big}
                    ]
                }))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(resp2.status(), 200);
        let _ = resp2.into_string().unwrap();

        let err_resp = ureq::post(&proxy_url)
            .set("Content-Type", "application/json")
            .send_bytes(br#"{"model":"m","input":[]}"#);
        match err_resp {
            Err(ureq::Error::Status(429, r)) => {
                assert!(r.into_string().unwrap().contains("rate_limited"));
            }
            other => panic!("expected forwarded 429 status, got {other:?}"),
        }

        let health_resp = ureq::get(&format!("{}{PROXY_HEALTH_PATH}", handle.base_url()))
            .call()
            .unwrap();
        let health: Value = serde_json::from_reader(health_resp.into_reader()).unwrap();
        assert_eq!(health["tool_outputs_capped"].as_u64(), Some(1));
        assert_eq!(health["tool_outputs_deduped"].as_u64(), Some(1));
        assert!(health["bytes_saved"].as_u64().unwrap() > 1000);

        let recorded = captured_bodies.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0]["input"][1], recorded[1]["input"][1]);
        assert!(recorded[1]["input"][2]["output"]
            .as_str()
            .unwrap()
            .contains("duplicate tool output"));

        handle.stop();
        upstream_thread.join().unwrap();
    }

    #[test]
    fn targeted_lookup_calls_receive_double_byte_budget_while_noisy_shell_caps_strictly() {
        let mut state = ProxyTransformState::new(400, false);
        let lookup_out = "x\n".repeat(300); // 600B: > 400B, <= 800B (2x cap)
        let shell_out = "y\n".repeat(300);  // 600B: > 400B -> gets capped
        let mut req = json!({
            "model": "gpt-6.1-sol",
            "input": [
                {"type": "function_call", "call_id": "call_read", "name": "exec_command", "arguments": "{\"cmd\":\"cs read django/db/models/query.py:1-120\"}"},
                {"type": "function_call_output", "call_id": "call_read", "output": lookup_out.clone()},
                {"type": "function_call", "call_id": "call_shell", "name": "exec_command", "arguments": "{\"cmd\":\"pytest -v\"}"},
                {"type": "function_call_output", "call_id": "call_shell", "output": shell_out.clone()}
            ]
        });
        let outcome = state.transform_request_json(&mut req, &[]);
        assert_eq!(outcome.tool_outputs_capped, 1);
        assert_eq!(req["input"][1]["output"].as_str().unwrap(), lookup_out);
        assert!(req["input"][3]["output"].as_str().unwrap().len() <= 400);
    }

    #[test]
    fn normalization_ignores_codex_json_chunk_id_and_wall_time_but_keeps_nonzero_exit() {
        let out_body = "line payload\n".repeat(40);
        let j1 = json!({"status": "fulfilled", "value": {"chunk_id": "a1", "wall_time_seconds": 0.02, "exit_code": 0, "output": out_body}}).to_string();
        let j2 = json!({"status": "fulfilled", "value": {"chunk_id": "b2", "wall_time_seconds": 1.75, "exit_code": 0, "output": out_body}}).to_string();
        let j_err = json!({"status": "fulfilled", "value": {"chunk_id": "c3", "wall_time_seconds": 0.1, "exit_code": 1, "output": out_body}}).to_string();
        assert_eq!(normalize_for_dedup(&j1), normalize_for_dedup(&j2));
        assert_ne!(normalize_for_dedup(&j1), normalize_for_dedup(&j_err));
    }
}
