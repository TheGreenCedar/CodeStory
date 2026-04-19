use codestory_contracts::api::{NodeKind, TrailDirection};
use codestory_contracts::query::{
    FilterQuery, GraphQueryAst, GraphQueryOperation, LimitQuery, SearchQuery, SymbolQuery,
    TrailQuery,
};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQueryParseError {
    pub message: String,
    pub offset: usize,
    pub source: String,
}

impl fmt::Display for GraphQueryParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.message)?;
        writeln!(f, "{}", self.source)?;
        let caret_offset = self.offset.min(self.source.len());
        writeln!(f, "{}^", " ".repeat(caret_offset))
    }
}

impl std::error::Error for GraphQueryParseError {}

pub fn parse_graph_query(source: &str) -> Result<GraphQueryAst, GraphQueryParseError> {
    let mut operations = Vec::new();
    for (segment, offset) in split_pipeline(source)? {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return Err(parse_error(
                source,
                offset,
                "Expected query operation after `|`",
            ));
        }
        operations.push(parse_operation(
            source,
            trimmed,
            offset + leading_ws(segment),
        )?);
    }
    if operations.is_empty() {
        return Err(parse_error(
            source,
            0,
            "Expected at least one query operation",
        ));
    }
    Ok(GraphQueryAst { operations })
}

fn split_pipeline(source: &str) -> Result<Vec<(&str, usize)>, GraphQueryParseError> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for (idx, ch) in source.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(parse_error(source, idx, "Unexpected `)`"));
                }
            }
            '|' if depth == 0 => {
                segments.push((&source[start..idx], start));
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }

    if quote.is_some() {
        return Err(parse_error(
            source,
            source.len(),
            "Unterminated string literal",
        ));
    }
    if depth > 0 {
        return Err(parse_error(source, source.len(), "Unclosed `(`"));
    }
    segments.push((&source[start..], start));
    Ok(segments)
}

fn parse_operation(
    source: &str,
    segment: &str,
    segment_offset: usize,
) -> Result<GraphQueryOperation, GraphQueryParseError> {
    let open = segment.find('(').ok_or_else(|| {
        parse_error(
            source,
            segment_offset + segment.len(),
            "Expected `(` after operation name",
        )
    })?;
    let close = segment.rfind(')').ok_or_else(|| {
        parse_error(
            source,
            segment_offset + segment.len(),
            "Expected closing `)`",
        )
    })?;
    if close < open || !segment[close + 1..].trim().is_empty() {
        return Err(parse_error(
            source,
            segment_offset + close + 1,
            "Unexpected text after operation",
        ));
    }

    let name = segment[..open].trim().to_ascii_lowercase();
    let args_source = &segment[open + 1..close];
    let args_offset = segment_offset + open + 1;
    let args = parse_args(source, args_source, args_offset)?;

    match name.as_str() {
        "trail" => {
            let symbol = required_string_arg(source, &args, "symbol", 0, args_offset)?;
            Ok(GraphQueryOperation::Trail(TrailQuery {
                symbol,
                depth: optional_u32_arg(source, &args, "depth")?,
                direction: optional_direction_arg(source, &args, "direction")?,
            }))
        }
        "symbol" => Ok(GraphQueryOperation::Symbol(SymbolQuery {
            query: required_string_arg(source, &args, "query", 0, args_offset)?,
        })),
        "search" => Ok(GraphQueryOperation::Search(SearchQuery {
            query: required_string_arg(source, &args, "query", 0, args_offset)?,
        })),
        "filter" => Ok(GraphQueryOperation::Filter(FilterQuery {
            kind: optional_kind_arg(source, &args, "kind")?,
            file: optional_string_arg(&args, "file"),
            depth: optional_u32_arg(source, &args, "depth")?,
        })),
        "limit" => Ok(GraphQueryOperation::Limit(LimitQuery {
            count: required_u32_arg(source, &args, "n", 0, args_offset)?,
        })),
        _ => Err(parse_error(
            source,
            segment_offset,
            format!("Unknown query operation `{name}`"),
        )),
    }
}

#[derive(Debug, Clone)]
struct ParsedArg {
    key: Option<String>,
    value: String,
    offset: usize,
}

fn parse_args(
    source: &str,
    args_source: &str,
    args_offset: usize,
) -> Result<Vec<ParsedArg>, GraphQueryParseError> {
    let mut args = Vec::new();
    for (part, offset) in split_args(source, args_source, args_offset)? {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let item_offset = offset + leading_ws(part);
        if let Some(colon) = find_top_level_colon(trimmed) {
            let key = trimmed[..colon].trim();
            if key.is_empty() {
                return Err(parse_error(source, item_offset, "Expected argument name"));
            }
            let value = trimmed[colon + 1..].trim();
            if value.is_empty() {
                return Err(parse_error(
                    source,
                    item_offset + colon + 1,
                    "Expected argument value",
                ));
            }
            args.push(ParsedArg {
                key: Some(key.to_ascii_lowercase()),
                value: parse_value(
                    source,
                    value,
                    item_offset + colon + 1 + leading_ws(&trimmed[colon + 1..]),
                )?,
                offset: item_offset,
            });
        } else {
            args.push(ParsedArg {
                key: None,
                value: parse_value(source, trimmed, item_offset)?,
                offset: item_offset,
            });
        }
    }
    Ok(args)
}

fn split_args<'a>(
    source: &str,
    args_source: &'a str,
    args_offset: usize,
) -> Result<Vec<(&'a str, usize)>, GraphQueryParseError> {
    let mut args = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in args_source.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ',' => {
                args.push((&args_source[start..idx], args_offset + start));
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return Err(parse_error(
            source,
            args_offset + args_source.len(),
            "Unterminated string literal",
        ));
    }
    args.push((&args_source[start..], args_offset + start));
    Ok(args)
}

fn find_top_level_colon(value: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in value.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ':' => return Some(idx),
            _ => {}
        }
    }
    None
}

fn parse_value(source: &str, raw: &str, offset: usize) -> Result<String, GraphQueryParseError> {
    let raw = raw.trim();
    if raw.len() >= 2 {
        let first = raw.as_bytes()[0] as char;
        let last = raw.as_bytes()[raw.len() - 1] as char;
        if (first == '\'' && last == '\'') || (first == '"' && last == '"') {
            return Ok(raw[1..raw.len() - 1]
                .replace("\\'", "'")
                .replace("\\\"", "\"")
                .replace("\\\\", "\\"));
        }
        if first == '\'' || first == '"' {
            return Err(parse_error(source, offset, "Unterminated string literal"));
        }
    }
    Ok(raw.to_string())
}

fn required_string_arg(
    source: &str,
    args: &[ParsedArg],
    key: &str,
    positional_index: usize,
    fallback_offset: usize,
) -> Result<String, GraphQueryParseError> {
    optional_string_arg(args, key)
        .or_else(|| positional_arg(args, positional_index))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| parse_error(source, fallback_offset, format!("Missing `{key}` argument")))
}

fn optional_string_arg(args: &[ParsedArg], key: &str) -> Option<String> {
    args.iter()
        .find(|arg| arg.key.as_deref() == Some(key))
        .map(|arg| arg.value.clone())
}

fn positional_arg(args: &[ParsedArg], index: usize) -> Option<String> {
    args.iter()
        .filter(|arg| arg.key.is_none())
        .nth(index)
        .map(|arg| arg.value.clone())
}

fn required_u32_arg(
    source: &str,
    args: &[ParsedArg],
    key: &str,
    positional_index: usize,
    fallback_offset: usize,
) -> Result<u32, GraphQueryParseError> {
    let raw = optional_string_arg(args, key)
        .or_else(|| positional_arg(args, positional_index))
        .ok_or_else(|| parse_error(source, fallback_offset, "Missing numeric argument"))?;
    raw.parse::<u32>().map_err(|_| {
        parse_error(
            source,
            arg_offset(args, key).unwrap_or(fallback_offset),
            format!("Expected `{raw}` to be an integer"),
        )
    })
}

fn optional_u32_arg(
    source: &str,
    args: &[ParsedArg],
    key: &str,
) -> Result<Option<u32>, GraphQueryParseError> {
    let Some(raw) = optional_string_arg(args, key) else {
        return Ok(None);
    };
    raw.parse::<u32>().map(Some).map_err(|_| {
        parse_error(
            source,
            arg_offset(args, key).unwrap_or(0),
            format!("Expected `{raw}` to be an integer"),
        )
    })
}

fn optional_direction_arg(
    source: &str,
    args: &[ParsedArg],
    key: &str,
) -> Result<Option<TrailDirection>, GraphQueryParseError> {
    let Some(raw) = optional_string_arg(args, key) else {
        return Ok(None);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "incoming" | "in" => Ok(Some(TrailDirection::Incoming)),
        "outgoing" | "out" => Ok(Some(TrailDirection::Outgoing)),
        "both" => Ok(Some(TrailDirection::Both)),
        _ => Err(parse_error(
            source,
            arg_offset(args, key).unwrap_or(0),
            format!("Unknown direction `{raw}`"),
        )),
    }
}

fn optional_kind_arg(
    source: &str,
    args: &[ParsedArg],
    key: &str,
) -> Result<Option<NodeKind>, GraphQueryParseError> {
    let Some(raw) = optional_string_arg(args, key) else {
        return Ok(None);
    };
    parse_node_kind(&raw).map(Some).ok_or_else(|| {
        parse_error(
            source,
            arg_offset(args, key).unwrap_or(0),
            format!("Unknown node kind `{raw}`"),
        )
    })
}

fn parse_node_kind(raw: &str) -> Option<NodeKind> {
    let normalized = raw.trim().replace('-', "_").to_ascii_uppercase();
    match normalized.as_str() {
        "MODULE" => Some(NodeKind::MODULE),
        "NAMESPACE" => Some(NodeKind::NAMESPACE),
        "PACKAGE" => Some(NodeKind::PACKAGE),
        "FILE" => Some(NodeKind::FILE),
        "STRUCT" => Some(NodeKind::STRUCT),
        "CLASS" => Some(NodeKind::CLASS),
        "INTERFACE" => Some(NodeKind::INTERFACE),
        "ANNOTATION" => Some(NodeKind::ANNOTATION),
        "UNION" => Some(NodeKind::UNION),
        "ENUM" => Some(NodeKind::ENUM),
        "TYPEDEF" => Some(NodeKind::TYPEDEF),
        "TYPE_PARAMETER" => Some(NodeKind::TYPE_PARAMETER),
        "BUILTIN_TYPE" => Some(NodeKind::BUILTIN_TYPE),
        "FUNCTION" => Some(NodeKind::FUNCTION),
        "METHOD" => Some(NodeKind::METHOD),
        "MACRO" => Some(NodeKind::MACRO),
        "GLOBAL_VARIABLE" => Some(NodeKind::GLOBAL_VARIABLE),
        "FIELD" => Some(NodeKind::FIELD),
        "VARIABLE" => Some(NodeKind::VARIABLE),
        "CONSTANT" => Some(NodeKind::CONSTANT),
        "ENUM_CONSTANT" => Some(NodeKind::ENUM_CONSTANT),
        "UNKNOWN" => Some(NodeKind::UNKNOWN),
        _ => None,
    }
}

fn arg_offset(args: &[ParsedArg], key: &str) -> Option<usize> {
    args.iter()
        .find(|arg| arg.key.as_deref() == Some(key))
        .map(|arg| arg.offset)
}

fn leading_ws(value: &str) -> usize {
    value.len() - value.trim_start().len()
}

fn parse_error(source: &str, offset: usize, message: impl Into<String>) -> GraphQueryParseError {
    GraphQueryParseError {
        message: message.into(),
        offset,
        source: source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::query::GraphQueryOperation;

    #[test]
    fn parses_pipeline_with_trail_filter_limit() {
        let ast = parse_graph_query(
            "trail(symbol: 'ResolutionPass', depth: 3, direction: outgoing) | filter(kind: function) | limit(5)",
        )
        .expect("parse");

        assert_eq!(ast.operations.len(), 3);
        assert!(matches!(ast.operations[0], GraphQueryOperation::Trail(_)));
        assert!(matches!(ast.operations[1], GraphQueryOperation::Filter(_)));
        assert!(matches!(ast.operations[2], GraphQueryOperation::Limit(_)));
    }

    #[test]
    fn reports_bad_token_with_offset() {
        let err = parse_graph_query("trail(symbol: 'Foo'").expect_err("unclosed");
        assert!(err.message.contains("Unclosed"));
        assert_eq!(err.offset, "trail(symbol: 'Foo'".len());
    }
}
