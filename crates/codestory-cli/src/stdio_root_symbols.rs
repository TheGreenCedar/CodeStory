use super::*;

const MAX_PAGE_BYTES: usize = 8 * 1024;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageRequest {
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}

impl PageRequest {
    pub fn parse(query: Option<&str>) -> Result<(Option<String>, Self)> {
        let mut project = None;
        let mut limit = None;
        let mut cursor = None;
        if let Some(query) = query {
            for field in query.split('&') {
                let (key, value) = field
                    .split_once('=')
                    .context("root_symbols_page_malformed: expected named selector")?;
                match key {
                    "project" if project.is_none() => {
                        if value.is_empty() {
                            bail!("resource_project_malformed: empty project");
                        }
                        project = Some(strict_uri_component_decode(value, "resource project")?);
                    }
                    "limit" if limit.is_none() => {
                        let parsed: usize = value
                            .parse()
                            .context("root_symbols_page_malformed: invalid limit")?;
                        if !(1..=MAX_LIMIT).contains(&parsed) || parsed.to_string() != value {
                            bail!(
                                "root_symbols_page_malformed: limit must be canonical and between 1 and 200"
                            );
                        }
                        limit = Some(parsed);
                    }
                    "cursor" if cursor.is_none() => {
                        let decoded = strict_uri_component_decode(value, "root symbol cursor")?;
                        parse_cursor(&decoded)?;
                        cursor = Some(decoded);
                    }
                    _ => bail!("root_symbols_page_malformed: unknown or duplicate selector"),
                }
            }
        }
        Ok((
            project,
            Self {
                limit: limit.unwrap_or(DEFAULT_LIMIT),
                cursor,
            },
        ))
    }

    pub fn uri(&self, base: &str) -> String {
        let separator = if base.contains('?') { '&' } else { '?' };
        let mut uri = format!("{base}{separator}limit={}", self.limit);
        if let Some(cursor) = &self.cursor {
            uri.push_str("&cursor=");
            uri.push_str(&strict_uri_component_encode(cursor));
        }
        uri
    }
}

fn parse_cursor(cursor: &str) -> Result<(&str, usize)> {
    let mut fields = cursor.split('.');
    if fields.next() != Some("rs1") {
        bail!("root_symbols_cursor_malformed: unsupported cursor");
    }
    let digest = fields
        .next()
        .context("root_symbols_cursor_malformed: missing binding")?;
    let raw_offset = fields
        .next()
        .context("root_symbols_cursor_malformed: missing offset")?;
    let offset: usize = raw_offset
        .parse()
        .context("root_symbols_cursor_malformed: invalid offset")?;
    if fields.next().is_some()
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || offset == 0
        || offset.to_string() != raw_offset
    {
        bail!("root_symbols_cursor_malformed: noncanonical cursor");
    }
    Ok((digest, offset))
}

pub(super) fn offset(request: &PageRequest) -> Result<usize> {
    request
        .cursor
        .as_deref()
        .map(parse_cursor)
        .transpose()
        .map(|v| v.map_or(0, |(_, offset)| offset))
}

// Keep request, publication binding, and inventory page explicit during rendering.
#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    request: &PageRequest,
    id: &serde_json::Value,
    project_uri: &str,
    project_id: &str,
    publication: serde_json::Value,
    inventory_fingerprint: &str,
    total: usize,
    mut symbols: Vec<serde_json::Value>,
) -> Result<serde_json::Value> {
    let binding = serde_json::json!([project_id, &publication, inventory_fingerprint]);
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&binding)?));
    let start = offset(request)?;
    if let Some(cursor) = &request.cursor
        && parse_cursor(cursor)?.0 != digest
    {
        return Err(map_api_error(codestory_contracts::api::ApiError::new(
            "root_symbols_cursor_stale",
            "project, publication or inventory changed; restart root symbols",
        )));
    }
    if start > total
        || symbols.len() > request.limit
        || symbols.len() > total.saturating_sub(start)
        || (start < total && symbols.is_empty())
    {
        return Err(map_api_error(codestory_contracts::api::ApiError::new(
            "root_symbols_page_incomplete",
            "inventory page cannot establish progress",
        )));
    }
    let uri = request.uri(project_uri);
    loop {
        let end = start + symbols.len();
        let remaining = total - end;
        let next = (remaining > 0).then(|| {
            PageRequest {
                limit: request.limit,
                cursor: Some(format!("rs1.{digest}.{end}")),
            }
            .uri(project_uri)
        });
        let value = serde_json::json!({
            "schema_version": 1,
            "symbols": symbols,
            "start_offset": start,
            "returned_count": symbols.len(),
            "total_count": total,
            "omitted_before_count": start,
            "remaining_count": remaining,
            "truncated": remaining > 0,
            "continuation_uri": next,
            "project_id": project_id,
            "publication": publication,
            "inventory_sha256": inventory_fingerprint,
            "max_response_bytes": MAX_PAGE_BYTES,
        });
        let response = serde_json::json!({"result": {"contents": [{
            "uri": uri, "mimeType": "application/json", "text": value.to_string()
        }]}});
        let wire = stdio_jsonrpc_from_legacy(id.clone(), response.clone());
        if serde_json::to_vec(&wire)?.len() <= MAX_PAGE_BYTES {
            return Ok(response);
        }
        if symbols.len() <= 1 {
            return Err(map_api_error(codestory_contracts::api::ApiError::new(
                "root_symbols_page_budget_exceeded",
                "complete row or metadata cannot fit response; no cursor advanced",
            )));
        }
        symbols.pop();
    }
}

#[cfg(test)]
#[path = "stdio_root_symbols_independent_tests.rs"]
mod independent_tests;
