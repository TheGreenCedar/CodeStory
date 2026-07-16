use super::FrameworkRoute;
use anyhow::{Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Point, Query, QueryCursor, Tree};

const PYTHON_FASTAPI_QUERY: &str = r#"
(decorated_definition
  (decorator
    (call
      function: (attribute
        object: (identifier) @receiver
        attribute: (identifier) @method)
      arguments: (argument_list
        . (string) @path))) @decorator
  definition: (function_definition
    name: (identifier) @handler))
"#;

static PYTHON_FASTAPI_COMPILED_QUERY: OnceLock<Result<Query, String>> = OnceLock::new();

const JAVASCRIPT_EXPRESS_QUERY: &str = r#"
(call_expression
  function: (member_expression
    object: (identifier) @receiver
    property: (property_identifier) @method)
  arguments: (arguments) @arguments) @route
"#;

static JAVASCRIPT_EXPRESS_COMPILED_QUERY: OnceLock<Result<Query, String>> = OnceLock::new();
static TYPESCRIPT_EXPRESS_COMPILED_QUERY: OnceLock<Result<Query, String>> = OnceLock::new();
static TSX_EXPRESS_COMPILED_QUERY: OnceLock<Result<Query, String>> = OnceLock::new();

#[derive(Clone, Copy)]
pub(super) enum JavaScriptDialect {
    JavaScript,
    TypeScript,
    Tsx,
}

#[derive(Clone, Copy)]
enum JavaScriptServerFramework {
    Express,
    Fastify,
}

impl JavaScriptServerFramework {
    fn module_name(self) -> &'static str {
        match self {
            Self::Express => "express",
            Self::Fastify => "fastify",
        }
    }
}

#[derive(Default)]
struct JavaScriptFrameworkBindings {
    constructors: HashSet<String>,
    router_constructors: HashSet<String>,
    modules: HashSet<String>,
    receivers: HashSet<String>,
    require_shadowed: bool,
}

#[derive(Default)]
struct FastApiBindings {
    constructors: HashSet<String>,
    modules: HashSet<String>,
    receivers: HashSet<String>,
}

#[derive(Clone, Copy)]
struct ReceiverOwnershipWrite {
    statement_start_byte: usize,
    owned: bool,
}

#[derive(Default)]
struct ReceiverOwnershipTimeline {
    writes: HashMap<String, Vec<ReceiverOwnershipWrite>>,
}

impl ReceiverOwnershipTimeline {
    fn record_statement(
        &mut self,
        statement_start_byte: usize,
        before: &HashSet<String>,
        after: &HashSet<String>,
    ) {
        for receiver in before.symmetric_difference(after) {
            self.writes
                .entry(receiver.clone())
                .or_default()
                .push(ReceiverOwnershipWrite {
                    statement_start_byte,
                    owned: after.contains(receiver),
                });
        }
    }

    fn owns_at(&self, receiver: &str, before_byte: usize) -> bool {
        let Some(writes) = self.writes.get(receiver) else {
            return false;
        };
        let index = writes.partition_point(|write| write.statement_start_byte < before_byte);
        index > 0 && writes[index - 1].owned
    }
}

#[derive(Default)]
pub(super) struct JavaScriptFrameworkTimeline {
    express: ReceiverOwnershipTimeline,
    fastify: ReceiverOwnershipTimeline,
    #[cfg(test)]
    statement_visits: usize,
}

#[derive(Default)]
pub(super) struct FastApiBindingTimeline {
    receivers: ReceiverOwnershipTimeline,
    #[cfg(test)]
    statement_visits: usize,
}

pub(super) fn collect_python_fastapi_routes_with_timeline(
    language: &Language,
    tree: &Tree,
    source: &str,
    timeline: &FastApiBindingTimeline,
) -> Result<Vec<FrameworkRoute>> {
    let query = PYTHON_FASTAPI_COMPILED_QUERY
        .get_or_init(|| {
            Query::new(language, PYTHON_FASTAPI_QUERY).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| anyhow!(message.clone()))?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut routes = Vec::new();

    while {
        matches.advance();
        matches.get().is_some()
    } {
        let Some(query_match) = matches.get() else {
            continue;
        };
        let mut receiver = None;
        let mut method = None;
        let mut path = None;
        let mut handler = None;
        let mut line = None;
        let mut route_start_byte = None;
        let mut module_scope = false;

        for capture in query_match.captures {
            let name = capture_names
                .get(capture.index as usize)
                .copied()
                .unwrap_or_default();
            match name {
                "receiver" => receiver = node_text(capture.node, source),
                "method" => method = node_text(capture.node, source),
                "path" => path = python_static_string(capture.node, source),
                "handler" => handler = node_text(capture.node, source),
                "decorator" => {
                    line = Some(capture.node.start_position().row as u32 + 1);
                    route_start_byte = Some(capture.node.start_byte());
                    module_scope = capture
                        .node
                        .parent()
                        .and_then(|node| node.parent())
                        .is_some_and(|node| node.kind() == "module");
                }
                _ => {}
            }
        }

        let (
            Some(receiver),
            Some(method),
            Some(path),
            Some(handler),
            Some(line),
            Some(route_start_byte),
        ) = (receiver, method, path, handler, line, route_start_byte)
        else {
            continue;
        };
        if !module_scope || !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete")
        {
            continue;
        }
        let route = FrameworkRoute::new(
            "fastapi",
            method.to_ascii_uppercase(),
            path,
            Some(handler),
            line,
            "decorator",
        );
        if timeline.receivers.owns_at(&receiver, route_start_byte) {
            routes.push(route.with_claim_evidence("tree_sitter_query", "parser_backed"));
        }
    }

    Ok(routes)
}

pub(super) fn collect_javascript_express_routes_with_timeline(
    language: &Language,
    dialect: JavaScriptDialect,
    tree: &Tree,
    source: &str,
    timeline: &JavaScriptFrameworkTimeline,
) -> Result<Vec<FrameworkRoute>> {
    let cache = match dialect {
        JavaScriptDialect::JavaScript => &JAVASCRIPT_EXPRESS_COMPILED_QUERY,
        JavaScriptDialect::TypeScript => &TYPESCRIPT_EXPRESS_COMPILED_QUERY,
        JavaScriptDialect::Tsx => &TSX_EXPRESS_COMPILED_QUERY,
    };
    let query = cache
        .get_or_init(|| {
            Query::new(language, JAVASCRIPT_EXPRESS_QUERY).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| anyhow!(message.clone()))?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut routes = Vec::new();

    while {
        matches.advance();
        matches.get().is_some()
    } {
        let Some(query_match) = matches.get() else {
            continue;
        };
        let mut receiver = None;
        let mut method = None;
        let mut arguments = None;
        let mut route_node = None;
        for capture in query_match.captures {
            let name = capture_names
                .get(capture.index as usize)
                .copied()
                .unwrap_or_default();
            match name {
                "receiver" => receiver = node_text(capture.node, source),
                "method" => method = node_text(capture.node, source),
                "arguments" => arguments = Some(capture.node),
                "route" => route_node = Some(capture.node),
                _ => {}
            }
        }
        let (Some(receiver), Some(method), Some(arguments), Some(route_node)) =
            (receiver, method, arguments, route_node)
        else {
            continue;
        };
        if !matches!(
            method.as_str(),
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
        ) || !javascript_route_is_module_statement(route_node)
            || !timeline.express.owns_at(&receiver, route_node.start_byte())
        {
            continue;
        }
        let mut argument_cursor = arguments.walk();
        let values = arguments
            .named_children(&mut argument_cursor)
            .filter(|node| node.kind() != "comment")
            .collect::<Vec<_>>();
        let Some(path) = values
            .first()
            .copied()
            .and_then(|node| javascript_static_string(node, source))
        else {
            continue;
        };
        let handler = values
            .get(1)
            .copied()
            .filter(|node| matches!(node.kind(), "identifier" | "member_expression"))
            .and_then(|node| node_text(node, source));
        routes.push(
            FrameworkRoute::new(
                "express",
                method.to_ascii_uppercase(),
                path,
                handler,
                route_node.start_position().row as u32 + 1,
                "heuristic",
            )
            .with_claim_evidence("tree_sitter_query", "parser_backed"),
        );
    }

    Ok(routes)
}

pub(super) fn collect_javascript_fastify_routes_with_timeline(
    language: &Language,
    dialect: JavaScriptDialect,
    tree: &Tree,
    source: &str,
    timeline: &JavaScriptFrameworkTimeline,
) -> Result<Vec<FrameworkRoute>> {
    let cache = match dialect {
        JavaScriptDialect::JavaScript => &JAVASCRIPT_EXPRESS_COMPILED_QUERY,
        JavaScriptDialect::TypeScript => &TYPESCRIPT_EXPRESS_COMPILED_QUERY,
        JavaScriptDialect::Tsx => &TSX_EXPRESS_COMPILED_QUERY,
    };
    let query = cache
        .get_or_init(|| {
            Query::new(language, JAVASCRIPT_EXPRESS_QUERY).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| anyhow!(message.clone()))?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut routes = Vec::new();

    while {
        matches.advance();
        matches.get().is_some()
    } {
        let Some(query_match) = matches.get() else {
            continue;
        };
        let mut receiver = None;
        let mut method = None;
        let mut arguments = None;
        let mut route_node = None;
        for capture in query_match.captures {
            let name = capture_names
                .get(capture.index as usize)
                .copied()
                .unwrap_or_default();
            match name {
                "receiver" => receiver = node_text(capture.node, source),
                "method" => method = node_text(capture.node, source),
                "arguments" => arguments = Some(capture.node),
                "route" => route_node = Some(capture.node),
                _ => {}
            }
        }
        let (Some(receiver), Some(method), Some(arguments), Some(route_node)) =
            (receiver, method, arguments, route_node)
        else {
            continue;
        };
        if route_node.has_error()
            || !javascript_route_is_module_statement(route_node)
            || !timeline.fastify.owns_at(&receiver, route_node.start_byte())
        {
            continue;
        }
        let mut argument_cursor = arguments.walk();
        let values = arguments
            .named_children(&mut argument_cursor)
            .filter(|node| node.kind() != "comment")
            .collect::<Vec<_>>();
        let parsed = if is_fastify_route_method(&method) {
            let Some(path) = values
                .first()
                .copied()
                .and_then(|node| javascript_static_string(node, source))
            else {
                continue;
            };
            if !matches!(values.len(), 2 | 3) || (values.len() == 3 && values[1].kind() != "object")
            {
                continue;
            }
            let handler = values
                .last()
                .copied()
                .filter(|node| matches!(node.kind(), "identifier" | "member_expression"))
                .and_then(|node| node_text(node, source));
            Some((method.to_ascii_uppercase(), path, handler))
        } else if method == "route" && values.len() == 1 {
            parse_fastify_route_object(values[0], source)
        } else {
            None
        };
        let Some((method, path, handler)) = parsed else {
            continue;
        };
        routes.push(
            FrameworkRoute::new(
                "fastify",
                method,
                path,
                handler,
                route_node.start_position().row as u32 + 1,
                "heuristic",
            )
            .with_claim_evidence("tree_sitter_query", "parser_backed"),
        );
    }

    Ok(routes)
}

fn is_fastify_route_method(method: &str) -> bool {
    matches!(
        method,
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
    )
}

fn parse_fastify_route_object(
    object: Node<'_>,
    source: &str,
) -> Option<(String, String, Option<String>)> {
    if object.kind() != "object" {
        return None;
    }
    let mut method = None;
    let mut path = None;
    let mut path_seen = false;
    let mut handler = None;
    let mut handler_seen = false;
    let mut cursor = object.walk();
    for property in object.named_children(&mut cursor) {
        if property.kind() == "comment" {
            continue;
        }
        if property.kind() == "shorthand_property_identifier" {
            let name = node_text(property, source)?;
            if name == "handler" {
                if handler_seen {
                    return None;
                }
                handler_seen = true;
                handler = Some(name);
            } else if matches!(name.as_str(), "method" | "url") {
                return None;
            }
            continue;
        }
        if property.kind() != "pair" {
            return None;
        }
        let key_node = property.child_by_field_name("key")?;
        let key = match key_node.kind() {
            "property_identifier" | "identifier" => node_text(key_node, source),
            "string" => javascript_static_string(key_node, source),
            _ => return None,
        };
        let value = property.child_by_field_name("value")?;
        match key.as_deref() {
            Some("method") if method.is_none() => {
                let value = javascript_static_string(value, source)?;
                if !is_fastify_route_method(&value.to_ascii_lowercase()) {
                    return None;
                }
                method = Some(value.to_ascii_uppercase());
            }
            Some("url") if !path_seen => {
                path_seen = true;
                path = Some(javascript_static_string(value, source)?);
            }
            Some("handler") if !handler_seen => {
                handler_seen = true;
                handler = matches!(value.kind(), "identifier" | "member_expression")
                    .then_some(value)
                    .and_then(|node| node_text(node, source));
            }
            Some("method" | "url" | "handler") => return None,
            _ => {}
        }
    }
    if !handler_seen {
        return None;
    }
    Some((method?, path?, handler))
}

pub(super) fn allow_javascript_express_lexical_fallback(
    tree: &Tree,
    source: &str,
    route: &FrameworkRoute,
    timeline: &JavaScriptFrameworkTimeline,
) -> bool {
    let Some(line) = source.lines().nth(route.line.saturating_sub(1) as usize) else {
        return false;
    };
    let Some((receiver, argument)) = javascript_route_receiver_and_argument(line, &route.method)
    else {
        return false;
    };
    if !matches!(argument.chars().next(), Some('\'' | '"')) || route.raw_path.contains('\\') {
        return false;
    }
    syntax_error_near_line(tree.root_node(), route.line)
        && javascript_line_is_module_scope(tree, line, route.line)
        && javascript_byte_is_code(source, source_byte_at_line(source, route.line))
        && timeline
            .express
            .owns_at(receiver, source_byte_at_line(source, route.line))
}

pub(super) fn allow_javascript_fastify_lexical_fallback(
    tree: &Tree,
    source: &str,
    route: &FrameworkRoute,
    timeline: &JavaScriptFrameworkTimeline,
) -> bool {
    if !is_fastify_route_method(&route.method.to_ascii_lowercase()) {
        return false;
    }
    let Some(line) = source.lines().nth(route.line.saturating_sub(1) as usize) else {
        return false;
    };
    let code_line = super::strip_c_style_comments(line)
        .into_iter()
        .next()
        .unwrap_or_default();
    let receiver = if let Some((receiver, argument)) =
        javascript_route_receiver_and_argument(&code_line, &route.method)
    {
        if !javascript_fallback_has_static_argument(argument, route) {
            return false;
        }
        receiver
    } else {
        let Some((receiver, _)) = code_line.trim_start().split_once(".route(") else {
            return false;
        };
        if javascript_fallback_has_top_level_spread(&code_line)
            || !javascript_fallback_has_direct_handler_property(&code_line)
            || !javascript_fallback_has_static_method_property(&code_line, &route.method)
            || !javascript_fallback_has_static_property(&code_line, "url", &route.raw_path)
        {
            return false;
        }
        if receiver.is_empty()
            || !receiver
                .chars()
                .all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
        {
            return false;
        }
        receiver
    };
    !route.raw_path.contains('\\')
        && syntax_error_near_line(tree.root_node(), route.line)
        && javascript_line_is_module_scope(tree, line, route.line)
        && javascript_byte_is_code(source, source_byte_at_line(source, route.line))
        && timeline
            .fastify
            .owns_at(receiver, source_byte_at_line(source, route.line))
}

fn javascript_fallback_static_string_remainder<'a>(
    value: &'a str,
    expected: &str,
) -> Option<&'a str> {
    let value = value.trim_start();
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))?;
    let end = value[quote.len_utf8()..].find(quote)? + quote.len_utf8();
    let literal = &value[quote.len_utf8()..end];
    (literal == expected && !literal.contains('\\'))
        .then(|| value[end + quote.len_utf8()..].trim_start())
}

fn javascript_fallback_has_static_argument(argument: &str, route: &FrameworkRoute) -> bool {
    let Some(remainder) = javascript_fallback_static_string_remainder(argument, &route.raw_path)
    else {
        return false;
    };
    if remainder.starts_with(',') {
        return true;
    }
    let Some(handler) = route.handler.as_deref() else {
        return false;
    };
    let candidate = remainder
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.')))
        .next()
        .unwrap_or_default();
    !candidate.is_empty() && candidate.rsplit('.').next() == Some(handler)
}

fn javascript_fallback_has_static_property(line: &str, key: &str, expected: &str) -> bool {
    let Some(after) = javascript_fallback_single_top_level_property_tail(line, key) else {
        return false;
    };
    let Some(value) = after.trim_start().strip_prefix(':') else {
        return false;
    };
    javascript_fallback_static_string_remainder(value, expected).is_some_and(|remainder| {
        remainder.is_empty() || remainder.starts_with(',') || remainder.starts_with('}')
    })
}

fn javascript_fallback_has_static_method_property(line: &str, expected: &str) -> bool {
    let Some(after) = javascript_fallback_single_top_level_property_tail(line, "method") else {
        return false;
    };
    let Some(value) = after.trim_start().strip_prefix(':') else {
        return false;
    };
    let value = value.trim_start();
    let Some(quote) = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
    else {
        return false;
    };
    let Some(end) = value[quote.len_utf8()..]
        .find(quote)
        .map(|end| end + quote.len_utf8())
    else {
        return false;
    };
    let literal = &value[quote.len_utf8()..end];
    let remainder = value[end + quote.len_utf8()..].trim_start();
    literal.eq_ignore_ascii_case(expected)
        && !literal.contains('\\')
        && (remainder.is_empty() || remainder.starts_with(',') || remainder.starts_with('}'))
}

fn javascript_fallback_has_direct_handler_property(line: &str) -> bool {
    let Some(after) = javascript_fallback_single_top_level_property_tail(line, "handler") else {
        return false;
    };
    let after = after.trim_start();
    if after.is_empty() || after.starts_with(',') || after.starts_with('}') {
        return true;
    }
    let Some(value) = after.strip_prefix(':').map(str::trim_start) else {
        return false;
    };
    let end = value
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.')))
        .unwrap_or(value.len());
    let candidate = &value[..end];
    let remainder = value[end..].trim_start();
    !candidate.is_empty()
        && candidate
            .split('.')
            .all(|part| !part.is_empty() && is_javascript_identifier(part))
        && (remainder.is_empty() || remainder.starts_with(',') || remainder.starts_with('}'))
}

fn javascript_fallback_single_top_level_property_tail<'a>(
    line: &'a str,
    key: &str,
) -> Option<&'a str> {
    let tails = javascript_fallback_top_level_property_tails(line, key)?;
    match tails.as_slice() {
        [tail] => Some(*tail),
        _ => None,
    }
}

fn javascript_fallback_top_level_property_tails<'a>(
    line: &'a str,
    key: &str,
) -> Option<Vec<&'a str>> {
    let (_, arguments) = line.split_once(".route(")?;
    let object = arguments.trim_start().strip_prefix('{')?;
    let mut depth = 0usize;
    let mut quote = None;
    let mut quote_start = None;
    let mut escaped = false;
    let mut tails = Vec::new();
    for (index, ch) in object.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                if active_quote != '`'
                    && depth == 0
                    && quote_start.is_some_and(|start| &object[start..index] == key)
                    && object[index + ch.len_utf8()..]
                        .trim_start()
                        .starts_with(':')
                {
                    tails.push(&object[index + ch.len_utf8()..]);
                }
                quote = None;
                quote_start = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            quote_start = Some(index + ch.len_utf8());
            continue;
        }
        match ch {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' if depth == 0 => break,
            '}' | ']' | ')' => depth -= 1,
            _ => {}
        }
        if depth != 0 || !object[index..].starts_with(key) {
            continue;
        }
        let before = &object[..index];
        let after = &object[index + key.len()..];
        if before
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
            || after
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
        {
            continue;
        }
        tails.push(after);
    }
    Some(tails)
}

fn javascript_fallback_has_top_level_spread(line: &str) -> bool {
    let Some((_, arguments)) = line.split_once(".route(") else {
        return true;
    };
    let Some(object) = arguments.trim_start().strip_prefix('{') else {
        return true;
    };
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in object.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            continue;
        }
        match ch {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' if depth == 0 => break,
            '}' | ']' | ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && object[index..].starts_with("...") {
            return true;
        }
    }
    false
}

fn is_javascript_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn javascript_byte_is_code(source: &str, target: usize) -> bool {
    let mut chars = source.char_indices().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut block_comment = false;
    let mut line_comment = false;
    while let Some((index, ch)) = chars.next() {
        if index >= target {
            return quote.is_none() && !block_comment && !line_comment;
        }
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if ch == '/' {
            match chars.peek().map(|(_, next)| *next) {
                Some('/') => {
                    chars.next();
                    line_comment = true;
                    continue;
                }
                Some('*') => {
                    chars.next();
                    block_comment = true;
                    continue;
                }
                _ => {}
            }
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
    }
    quote.is_none() && !block_comment && !line_comment
}

fn javascript_route_is_module_statement(node: Node<'_>) -> bool {
    node.parent().is_some_and(|node| {
        node.kind() == "expression_statement"
            && node.parent().is_some_and(|node| node.kind() == "program")
    })
}

fn javascript_line_is_module_scope(tree: &Tree, line: &str, line_number: u32) -> bool {
    let mut node = node_at_line_start(tree, line, line_number);
    while let Some(current) = node {
        if matches!(current.kind(), "string" | "template_string" | "comment") {
            return false;
        }
        if matches!(
            current.kind(),
            "function_declaration"
                | "function_expression"
                | "arrow_function"
                | "class_declaration"
                | "method_definition"
        ) {
            return false;
        }
        let parent = current.parent();
        if parent.is_some_and(|parent| parent.kind() == "program") {
            return matches!(current.kind(), "expression_statement" | "ERROR");
        }
        node = parent;
    }
    false
}

fn javascript_route_receiver_and_argument<'a>(
    line: &'a str,
    method: &str,
) -> Option<(&'a str, &'a str)> {
    let needle = format!(".{}(", method.to_ascii_lowercase());
    let (receiver, argument) = line.trim_start().split_once(&needle)?;
    (!receiver.is_empty()
        && receiver
            .chars()
            .all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()))
    .then_some((receiver, argument.trim_start()))
}

fn javascript_static_string(node: Node<'_>, source: &str) -> Option<String> {
    let text = node.utf8_text(source.as_bytes()).ok()?.trim();
    match node.kind() {
        "string" => {
            let quote = text.chars().next()?;
            (matches!(quote, '\'' | '"')
                && text.ends_with(quote)
                && text.len() >= 2
                && !text[1..text.len() - 1].contains('\\'))
            .then(|| text[1..text.len() - 1].to_string())
        }
        "template_string" => {
            let mut cursor = node.walk();
            (!node
                .named_children(&mut cursor)
                .any(|child| child.kind() == "template_substitution")
                && text.starts_with('`')
                && text.ends_with('`')
                && text.len() >= 2
                && !text[1..text.len() - 1].contains('\\'))
            .then(|| text[1..text.len() - 1].to_string())
        }
        _ => None,
    }
}

pub(super) fn build_javascript_framework_timeline(
    tree: &Tree,
    source: &str,
) -> JavaScriptFrameworkTimeline {
    let mut timeline = JavaScriptFrameworkTimeline::default();
    let mut express = JavaScriptFrameworkBindings::default();
    let mut fastify = JavaScriptFrameworkBindings::default();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        #[cfg(test)]
        {
            timeline.statement_visits += 1;
        }
        let express_before = express.receivers.clone();
        apply_javascript_module_statement(
            statement,
            source,
            JavaScriptServerFramework::Express,
            &mut express,
        );
        timeline.express.record_statement(
            statement.start_byte(),
            &express_before,
            &express.receivers,
        );

        let fastify_before = fastify.receivers.clone();
        apply_javascript_module_statement(
            statement,
            source,
            JavaScriptServerFramework::Fastify,
            &mut fastify,
        );
        timeline.fastify.record_statement(
            statement.start_byte(),
            &fastify_before,
            &fastify.receivers,
        );
    }
    timeline
}

fn apply_javascript_module_statement(
    statement: Node<'_>,
    source: &str,
    framework: JavaScriptServerFramework,
    bindings: &mut JavaScriptFrameworkBindings,
) {
    if statement.kind() == "import_statement" {
        apply_javascript_framework_import(statement, source, framework, bindings);
        return;
    }

    let mut declarators = Vec::new();
    collect_nodes_of_kind(statement, "variable_declarator", &mut declarators);
    if !declarators.is_empty() {
        for declarator in declarators {
            apply_javascript_framework_declarator(declarator, source, framework, bindings);
            if let Some(value) = declarator.child_by_field_name("value") {
                invalidate_javascript_writes(value, source, bindings);
            }
        }
        return;
    }

    if let Some(name) = javascript_runtime_declaration_name(statement, source) {
        invalidate_javascript_framework_binding(&name, bindings);
        return;
    }

    invalidate_javascript_writes(statement, source, bindings);
}

fn invalidate_javascript_writes(
    node: Node<'_>,
    source: &str,
    bindings: &mut JavaScriptFrameworkBindings,
) {
    let mut names = HashSet::new();
    collect_javascript_written_names(node, source, &mut names);
    for name in names {
        invalidate_javascript_framework_binding(&name, bindings);
    }
}

fn apply_javascript_framework_import(
    node: Node<'_>,
    source: &str,
    framework: JavaScriptServerFramework,
    bindings: &mut JavaScriptFrameworkBindings,
) {
    let source_module = node
        .child_by_field_name("source")
        .and_then(|node| javascript_static_string(node, source));
    let Some(clause) = node
        .named_children(&mut node.walk())
        .find(|child| child.kind() == "import_clause")
    else {
        return;
    };
    if node_has_direct_anonymous_token(node, &["type", "typeof"])
        || node_has_direct_anonymous_token(clause, &["type", "typeof"])
    {
        return;
    }
    let imported_bindings = javascript_import_local_bindings(clause, source);
    for name in &imported_bindings {
        invalidate_javascript_framework_binding(name, bindings);
    }
    if source_module.as_deref() != Some(framework.module_name()) {
        return;
    }

    let mut cursor = clause.walk();
    for child in clause.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                if let Some(name) = node_text(child, source) {
                    bindings.constructors.insert(name);
                }
            }
            "namespace_import" => {
                if let Some(name) = last_identifier(child, source) {
                    bindings.modules.insert(name);
                }
            }
            "named_imports" => {
                let mut import_cursor = child.walk();
                for specifier in child.named_children(&mut import_cursor) {
                    if specifier.kind() != "import_specifier"
                        || node_has_direct_anonymous_token(specifier, &["type", "typeof"])
                    {
                        continue;
                    }
                    let imported = specifier
                        .child_by_field_name("name")
                        .and_then(|node| node_text(node, source));
                    let local = specifier
                        .child_by_field_name("alias")
                        .and_then(|node| node_text(node, source))
                        .or_else(|| imported.clone());
                    let Some(local) = local else {
                        continue;
                    };
                    match framework {
                        JavaScriptServerFramework::Express
                            if imported.as_deref() == Some("Router") =>
                        {
                            bindings.router_constructors.insert(local);
                        }
                        JavaScriptServerFramework::Fastify
                            if matches!(imported.as_deref(), Some("fastify" | "default")) =>
                        {
                            bindings.constructors.insert(local);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn apply_javascript_framework_declarator(
    node: Node<'_>,
    source: &str,
    framework: JavaScriptServerFramework,
    bindings: &mut JavaScriptFrameworkBindings,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let mut names = HashSet::new();
    collect_javascript_binding_names(name_node, source, &mut names);
    for name in &names {
        invalidate_javascript_framework_binding(name, bindings);
    }
    let Some(value) = node.child_by_field_name("value") else {
        return;
    };

    if expression_is_framework_require(value, source, framework, bindings) {
        if name_node.kind() == "identifier"
            && let Some(name) = node_text(name_node, source)
        {
            bindings.constructors.insert(name);
        } else if name_node.kind() == "object_pattern" {
            match framework {
                JavaScriptServerFramework::Express => bindings
                    .router_constructors
                    .extend(commonjs_router_bindings(name_node, source)),
                JavaScriptServerFramework::Fastify => bindings
                    .constructors
                    .extend(commonjs_fastify_bindings(name_node, source)),
            }
        }
        return;
    }

    if names.len() != 1 || name_node.kind() != "identifier" {
        return;
    }
    let Some(name) = names.into_iter().next() else {
        return;
    };
    if expression_constructs_framework_receiver(value, source, framework, bindings) {
        bindings.receivers.insert(name);
    }
}

fn expression_is_framework_require(
    node: Node<'_>,
    source: &str,
    framework: JavaScriptServerFramework,
    bindings: &JavaScriptFrameworkBindings,
) -> bool {
    let node = javascript_transparent_expression(node);
    if bindings.require_shadowed || node.kind() != "call_expression" {
        return false;
    }
    node.child_by_field_name("function")
        .and_then(|node| node_text(node, source))
        .as_deref()
        == Some("require")
        && node
            .child_by_field_name("arguments")
            .and_then(|arguments| arguments.named_child(0))
            .and_then(|argument| javascript_static_string(argument, source))
            .as_deref()
            == Some(framework.module_name())
}

fn expression_constructs_framework_receiver(
    node: Node<'_>,
    source: &str,
    framework: JavaScriptServerFramework,
    bindings: &JavaScriptFrameworkBindings,
) -> bool {
    let node = javascript_transparent_expression(node);
    if node.kind() != "call_expression" {
        return false;
    }
    let Some(function) = node.child_by_field_name("function") else {
        return false;
    };
    match function.kind() {
        "identifier" => node_text(function, source).is_some_and(|name| match framework {
            JavaScriptServerFramework::Express => {
                bindings.constructors.contains(&name)
                    || bindings.router_constructors.contains(&name)
            }
            JavaScriptServerFramework::Fastify => bindings.constructors.contains(&name),
        }),
        "member_expression" => {
            let object = function
                .child_by_field_name("object")
                .and_then(|node| node_text(node, source));
            let property = function
                .child_by_field_name("property")
                .and_then(|node| node_text(node, source));
            match framework {
                JavaScriptServerFramework::Express => {
                    object.is_some_and(|name| {
                        bindings.constructors.contains(&name) || bindings.modules.contains(&name)
                    }) && property.as_deref() == Some("Router")
                }
                JavaScriptServerFramework::Fastify => {
                    object.is_some_and(|name| bindings.modules.contains(&name))
                        && matches!(property.as_deref(), Some("fastify" | "default"))
                }
            }
        }
        _ => false,
    }
}

fn invalidate_javascript_framework_binding(name: &str, bindings: &mut JavaScriptFrameworkBindings) {
    bindings.constructors.remove(name);
    bindings.router_constructors.remove(name);
    bindings.modules.remove(name);
    bindings.receivers.remove(name);
    if name == "require" {
        bindings.require_shadowed = true;
    }
}

fn collect_nodes_of_kind<'tree>(node: Node<'tree>, kind: &str, out: &mut Vec<Node<'tree>>) {
    if node.kind() == kind {
        out.push(node);
        return;
    }
    if !matches!(
        node.kind(),
        "export_statement" | "lexical_declaration" | "variable_declaration"
    ) {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_nodes_of_kind(child, kind, out);
    }
}

fn collect_javascript_binding_names(node: Node<'_>, source: &str, names: &mut HashSet<String>) {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            if let Some(name) = node_text(node, source) {
                names.insert(name);
            }
        }
        "pair_pattern" => {
            if let Some(value) = node.child_by_field_name("value") {
                collect_javascript_binding_names(value, source, names);
            }
        }
        "assignment_pattern" => {
            if let Some(left) = node.child_by_field_name("left") {
                collect_javascript_binding_names(left, source, names);
            }
        }
        "rest_pattern" => {
            if let Some(argument) = node.child_by_field_name("argument") {
                collect_javascript_binding_names(argument, source, names);
            }
        }
        "object_pattern" | "array_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_javascript_binding_names(child, source, names);
            }
        }
        _ => {}
    }
}

fn javascript_import_local_bindings(node: Node<'_>, source: &str) -> HashSet<String> {
    let mut bindings = HashSet::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                if let Some(name) = node_text(child, source) {
                    bindings.insert(name);
                }
            }
            "namespace_import" => {
                if let Some(name) = last_identifier(child, source) {
                    bindings.insert(name);
                }
            }
            "named_imports" => {
                let mut import_cursor = child.walk();
                for specifier in child.named_children(&mut import_cursor) {
                    if specifier.kind() != "import_specifier" {
                        continue;
                    }
                    if node_has_direct_anonymous_token(specifier, &["type", "typeof"]) {
                        continue;
                    }
                    let local = specifier
                        .child_by_field_name("alias")
                        .or_else(|| specifier.child_by_field_name("name"))
                        .and_then(|node| node_text(node, source));
                    if let Some(local) = local {
                        bindings.insert(local);
                    }
                }
            }
            _ => {}
        }
    }
    bindings
}

fn javascript_runtime_declaration_name(node: Node<'_>, source: &str) -> Option<String> {
    let declaration = if matches!(
        node.kind(),
        "function_declaration" | "class_declaration" | "enum_declaration"
    ) {
        node
    } else if node.kind() == "export_statement" {
        node.named_children(&mut node.walk()).find(|child| {
            matches!(
                child.kind(),
                "function_declaration" | "class_declaration" | "enum_declaration"
            )
        })?
    } else {
        return None;
    };
    declaration
        .child_by_field_name("name")
        .and_then(|name| node_text(name, source))
}

fn javascript_transparent_expression(mut node: Node<'_>) -> Node<'_> {
    loop {
        let expression = match node.kind() {
            "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression" => node.named_child(0),
            "type_assertion" => node.named_child(1),
            _ => return node,
        };
        let Some(expression) = expression else {
            return node;
        };
        node = expression;
    }
}

fn collect_javascript_written_names(node: Node<'_>, source: &str, names: &mut HashSet<String>) {
    if matches!(
        node.kind(),
        "function_declaration"
            | "function_expression"
            | "arrow_function"
            | "class_declaration"
            | "class"
    ) {
        return;
    }
    if node.kind() == "assignment_expression" {
        if let Some(left) = node.child_by_field_name("left") {
            collect_javascript_binding_names(left, source, names);
        }
        return;
    }
    if node.kind() == "update_expression" {
        if let Some(argument) = node.named_child(0) {
            collect_javascript_binding_names(argument, source, names);
        }
        return;
    }
    if node.kind() == "for_in_statement"
        && let Some(left) = node.child_by_field_name("left")
        && !matches!(left.kind(), "lexical_declaration" | "variable_declaration")
    {
        collect_javascript_binding_names(left, source, names);
    }
    if node.kind() == "variable_declarator" {
        if node.parent().is_some_and(|declaration| {
            declaration.kind() == "variable_declaration"
                && node_has_direct_anonymous_token(declaration, &["var"])
        }) && let Some(name) = node.child_by_field_name("name")
        {
            collect_javascript_binding_names(name, source, names);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_javascript_written_names(child, source, names);
    }
}

fn last_identifier(node: Node<'_>, source: &str) -> Option<String> {
    let mut result = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "identifier" {
            result = node_text(child, source);
        } else if let Some(identifier) = last_identifier(child, source) {
            result = Some(identifier);
        }
    }
    result
}

fn node_has_direct_anonymous_token(node: Node<'_>, expected: &[&str]) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| !child.is_named() && expected.contains(&child.kind()))
}

fn commonjs_router_bindings(node: Node<'_>, source: &str) -> HashSet<String> {
    let mut bindings = HashSet::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "shorthand_property_identifier_pattern" => {
                if node_text(child, source).as_deref() == Some("Router") {
                    bindings.insert("Router".to_string());
                }
            }
            "pair_pattern" => {
                let key = child
                    .child_by_field_name("key")
                    .and_then(|node| node_text(node, source));
                if key.as_deref() != Some("Router") {
                    continue;
                }
                if let Some(value) = child.child_by_field_name("value") {
                    collect_javascript_binding_names(value, source, &mut bindings);
                }
            }
            _ => {}
        }
    }
    bindings
}

fn commonjs_fastify_bindings(node: Node<'_>, source: &str) -> HashSet<String> {
    let mut bindings = HashSet::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "shorthand_property_identifier_pattern" => {
                if node_text(child, source).as_deref() == Some("fastify") {
                    bindings.insert("fastify".to_string());
                }
            }
            "pair_pattern" => {
                let key = child
                    .child_by_field_name("key")
                    .and_then(|node| node_text(node, source));
                if key.as_deref() != Some("fastify") {
                    continue;
                }
                if let Some(value) = child.child_by_field_name("value") {
                    collect_javascript_binding_names(value, source, &mut bindings);
                }
            }
            _ => {}
        }
    }
    bindings
}

pub(super) fn allow_python_fastapi_lexical_fallback(
    tree: &Tree,
    source: &str,
    route: &FrameworkRoute,
    timeline: &FastApiBindingTimeline,
) -> bool {
    let Some(line) = source.lines().nth(route.line.saturating_sub(1) as usize) else {
        return false;
    };
    let Some((receiver, argument)) = fastapi_decorator_receiver_and_argument(line, &route.method)
    else {
        return false;
    };
    let raw = argument.starts_with("r\"")
        || argument.starts_with("r'")
        || argument.starts_with("R\"")
        || argument.starts_with("R'");
    let direct_static = raw || argument.starts_with('"') || argument.starts_with('\'');
    direct_static
        && (raw || !route.raw_path.contains('\\'))
        && timeline
            .receivers
            .owns_at(receiver, source_byte_at_line(source, route.line))
        && syntax_error_near_line(tree.root_node(), route.line)
        && line_is_module_scope(tree, line, route.line)
        && !line_is_inside_python_string_or_comment(tree, line, route.line)
}

fn apply_fastapi_import(node: Node<'_>, source: &str, bindings: &mut FastApiBindings) {
    match node.kind() {
        "import_from_statement" => {
            let module = node
                .child_by_field_name("module_name")
                .and_then(|node| node_text(node, source));
            if module
                .as_deref()
                .is_some_and(|module| module == "fastapi" || module.starts_with("fastapi."))
            {
                for (imported, local) in python_import_bindings(node, source) {
                    if matches!(imported.as_str(), "FastAPI" | "APIRouter") {
                        bindings.constructors.insert(local);
                    }
                }
            }
        }
        "import_statement" => {
            for (imported, local) in python_import_bindings(node, source) {
                if imported == "fastapi" {
                    bindings.modules.insert(local);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn build_fastapi_binding_timeline(tree: &Tree, source: &str) -> FastApiBindingTimeline {
    let mut timeline = FastApiBindingTimeline::default();
    let mut bindings = FastApiBindings::default();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        #[cfg(test)]
        {
            timeline.statement_visits += 1;
        }
        let before = bindings.receivers.clone();
        if let Some(assignment) = module_assignment(statement) {
            apply_fastapi_assignment(assignment, source, &mut bindings);
        } else {
            invalidate_module_statement_bindings(statement, source, &mut bindings);
            apply_fastapi_import(statement, source, &mut bindings);
        }
        timeline
            .receivers
            .record_statement(statement.start_byte(), &before, &bindings.receivers);
    }
    timeline
}

fn invalidate_module_statement_bindings(
    statement: Node<'_>,
    source: &str,
    bindings: &mut FastApiBindings,
) {
    if node_is_star_import(statement) {
        bindings.receivers.clear();
        bindings.constructors.clear();
        bindings.modules.clear();
        return;
    }
    let mut names = HashSet::new();
    collect_module_bound_names(statement, source, &mut names);
    for name in names {
        bindings.receivers.remove(&name);
        bindings.constructors.remove(&name);
        bindings.modules.remove(&name);
    }
}

fn node_is_star_import(node: Node<'_>) -> bool {
    node.kind() == "import_from_statement"
        && node
            .named_children(&mut node.walk())
            .any(|child| child.kind() == "wildcard_import")
}

fn python_import_bindings(node: Node<'_>, source: &str) -> Vec<(String, String)> {
    let direct_import = node.kind() == "import_statement";
    let module_node = node.child_by_field_name("module_name");
    let mut bindings = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if module_node.is_some_and(|module| module.id() == child.id()) {
            continue;
        }
        let binding = match child.kind() {
            "aliased_import" => {
                let imported = child
                    .child_by_field_name("name")
                    .and_then(|name| node_text(name, source));
                let local = child
                    .child_by_field_name("alias")
                    .and_then(|alias| node_text(alias, source));
                imported.zip(local)
            }
            "dotted_name" => node_text(child, source).map(|imported| {
                let local = if direct_import {
                    imported
                        .split_once('.')
                        .map_or_else(|| imported.clone(), |(root, _)| root.to_string())
                } else {
                    imported.clone()
                };
                (imported, local)
            }),
            _ => None,
        };
        if let Some(binding) = binding {
            bindings.push(binding);
        }
    }
    bindings
}

fn collect_module_bound_names(node: Node<'_>, source: &str, names: &mut HashSet<String>) {
    match node.kind() {
        "function_definition" | "class_definition" => {
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|node| node_text(node, source))
            {
                names.insert(name);
            }
            return;
        }
        "decorated_definition" => {
            if let Some(definition) = node.child_by_field_name("definition") {
                collect_module_bound_names(definition, source, names);
            }
            return;
        }
        "lambda" => return,
        "assignment" | "augmented_assignment" | "named_expression" => {
            if let Some(target) = node.child_by_field_name("left") {
                collect_python_binding_target(target, source, names);
            }
        }
        "delete_statement" => {
            let mut cursor = node.walk();
            for target in node.named_children(&mut cursor) {
                collect_python_binding_target(target, source, names);
            }
            return;
        }
        "type_alias_statement" => {
            if let Some(target) = node.child_by_field_name("name") {
                collect_python_binding_target(target, source, names);
            }
        }
        "for_statement" => {
            if let Some(target) = node.child_by_field_name("left") {
                collect_python_binding_target(target, source, names);
            }
        }
        "with_item" => {
            if let Some(target) = node.child_by_field_name("alias") {
                collect_python_binding_target(target, source, names);
            }
        }
        "except_clause" => {
            if let Some(target) = node.child_by_field_name("name") {
                collect_python_binding_target(target, source, names);
            }
        }
        "import_from_statement" => collect_import_from_bound_names(node, source, names),
        "import_statement" => collect_import_bound_names(node, source, names),
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_module_bound_names(child, source, names);
    }
}

fn collect_python_binding_target(node: Node<'_>, source: &str, names: &mut HashSet<String>) {
    if node.kind() == "identifier" {
        if let Some(name) = node_text(node, source) {
            names.insert(name);
        }
        return;
    }
    if matches!(node.kind(), "attribute" | "subscript") {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_python_binding_target(child, source, names);
    }
}

fn collect_import_from_bound_names(node: Node<'_>, source: &str, names: &mut HashSet<String>) {
    names.extend(
        python_import_bindings(node, source)
            .into_iter()
            .map(|(_, local)| local),
    );
}

fn collect_import_bound_names(node: Node<'_>, source: &str, names: &mut HashSet<String>) {
    names.extend(
        python_import_bindings(node, source)
            .into_iter()
            .map(|(_, local)| local),
    );
}

fn module_assignment(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "assignment" {
        return Some(node);
    }
    (node.kind() == "expression_statement")
        .then(|| node.named_child(0))
        .flatten()
        .filter(|node| node.kind() == "assignment")
}

fn apply_fastapi_assignment(node: Node<'_>, source: &str, bindings: &mut FastApiBindings) {
    let mut targets = HashSet::new();
    collect_assignment_binding_targets(node, source, &mut targets);
    let constructs_fastapi = assignment_constructs_fastapi(node, source, bindings);
    for target in &targets {
        bindings.receivers.remove(target);
        bindings.constructors.remove(target);
        bindings.modules.remove(target);
    }
    if constructs_fastapi
        && targets.len() == 1
        && let Some(receiver) = targets.into_iter().next()
    {
        bindings.receivers.insert(receiver);
    }
}

fn collect_assignment_binding_targets(node: Node<'_>, source: &str, targets: &mut HashSet<String>) {
    if let Some(left) = node.child_by_field_name("left") {
        collect_python_binding_target(left, source, targets);
    }
    if let Some(right) = node.child_by_field_name("right")
        && right.kind() == "assignment"
    {
        collect_assignment_binding_targets(right, source, targets);
    }
}

fn assignment_constructs_fastapi(node: Node<'_>, source: &str, bindings: &FastApiBindings) -> bool {
    let mut value = node.child_by_field_name("right");
    while let Some(assignment) = value.filter(|node| node.kind() == "assignment") {
        value = assignment.child_by_field_name("right");
    }
    if let Some(call) = value.filter(|node| node.kind() == "call")
        && let Some(function) = call.child_by_field_name("function")
    {
        return match function.kind() {
            "identifier" => node_text(function, source)
                .is_some_and(|name| bindings.constructors.contains(&name)),
            "attribute" => {
                let module = function
                    .child_by_field_name("object")
                    .and_then(|node| node_text(node, source));
                let constructor = function
                    .child_by_field_name("attribute")
                    .and_then(|node| node_text(node, source));
                module.is_some_and(|module| bindings.modules.contains(&module))
                    && constructor
                        .as_deref()
                        .is_some_and(|name| matches!(name, "FastAPI" | "APIRouter"))
            }
            _ => false,
        };
    }
    false
}

fn line_is_module_scope(tree: &Tree, line: &str, line_number: u32) -> bool {
    let mut node = node_at_line_start(tree, line, line_number);
    while let Some(current) = node {
        if matches!(
            current.kind(),
            "function_definition" | "class_definition" | "lambda"
        ) {
            return false;
        }
        node = current.parent();
    }
    true
}

fn source_byte_at_line(source: &str, line: u32) -> usize {
    source
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum()
}

fn fastapi_decorator_receiver_and_argument<'a>(
    line: &'a str,
    method: &str,
) -> Option<(&'a str, &'a str)> {
    let trimmed = line.trim_start().strip_prefix('@')?;
    let (receiver, tail) = trimmed.split_once('.')?;
    let argument = tail.strip_prefix(&format!("{}(", method.to_ascii_lowercase()))?;
    Some((receiver, argument.trim_start()))
}

fn syntax_error_near_line(node: Node<'_>, line: u32) -> bool {
    let start = node.start_position().row as u32 + 1;
    let end = node.end_position().row as u32 + 1;
    if (node.is_error() || node.is_missing())
        && start <= line.saturating_add(2)
        && end.saturating_add(2) >= line
    {
        return true;
    }
    if !node.has_error() {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| syntax_error_near_line(child, line))
}

fn line_is_inside_python_string_or_comment(tree: &Tree, line: &str, line_number: u32) -> bool {
    let mut node = node_at_line_start(tree, line, line_number);
    while let Some(current) = node {
        if matches!(current.kind(), "string" | "comment") {
            return true;
        }
        node = current.parent();
    }
    false
}

fn node_at_line_start<'tree>(
    tree: &'tree Tree,
    line: &str,
    line_number: u32,
) -> Option<Node<'tree>> {
    let row = line_number.saturating_sub(1) as usize;
    let col = line.len().saturating_sub(line.trim_start().len());
    let point = Point::new(row, col);
    tree.root_node().descendant_for_point_range(point, point)
}

fn node_text(node: Node<'_>, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes())
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn python_static_string(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    if node
        .named_children(&mut cursor)
        .any(|child| child.kind() == "interpolation")
    {
        return None;
    }
    let text = node.utf8_text(source.as_bytes()).ok()?.trim();
    let quote_start = text.find(['\'', '"'])?;
    let prefix = &text[..quote_start];
    if prefix.chars().any(|ch| matches!(ch, 'f' | 'F' | 'b' | 'B'))
        || !prefix.chars().all(|ch| matches!(ch, 'r' | 'R' | 'u' | 'U'))
    {
        return None;
    }
    let quote = text.as_bytes()[quote_start] as char;
    let delimiter_len = if text[quote_start..].starts_with(&format!("{quote}{quote}{quote}")) {
        3
    } else {
        1
    };
    let content_start = quote_start + delimiter_len;
    let content_end = text.len().checked_sub(delimiter_len)?;
    let content = (content_end >= content_start
        && text[content_end..].chars().all(|ch| ch == quote))
    .then(|| &text[content_start..content_end])?;
    let raw = prefix.chars().any(|ch| matches!(ch, 'r' | 'R'));
    (raw || !content.contains('\\')).then(|| content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;
    use tree_sitter::Parser;

    fn parse_python(source: &str) -> Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("python grammar");
        parser.parse(source, None).expect("python tree")
    }

    fn parse_javascript(language: &Language, source: &str) -> Tree {
        let mut parser = Parser::new();
        parser.set_language(language).expect("javascript grammar");
        parser.parse(source, None).expect("javascript tree")
    }

    fn collect_python_fastapi_routes(
        language: &Language,
        tree: &Tree,
        source: &str,
    ) -> Result<Vec<FrameworkRoute>> {
        let timeline = build_fastapi_binding_timeline(tree, source);
        collect_python_fastapi_routes_with_timeline(language, tree, source, &timeline)
    }

    fn collect_javascript_express_routes(
        language: &Language,
        dialect: JavaScriptDialect,
        tree: &Tree,
        source: &str,
    ) -> Result<Vec<FrameworkRoute>> {
        let timeline = build_javascript_framework_timeline(tree, source);
        collect_javascript_express_routes_with_timeline(language, dialect, tree, source, &timeline)
    }

    fn collect_javascript_fastify_routes(
        language: &Language,
        dialect: JavaScriptDialect,
        tree: &Tree,
        source: &str,
    ) -> Result<Vec<FrameworkRoute>> {
        let timeline = build_javascript_framework_timeline(tree, source);
        collect_javascript_fastify_routes_with_timeline(language, dialect, tree, source, &timeline)
    }

    #[test]
    fn test_shared_javascript_timeline_visits_module_statements_once() -> Result<()> {
        let source = r#"
import express from "express";
import Fastify from "fastify";
const app = express();
const api = Fastify();
app.get("/express", expressHandler);
api.get("/fastify", fastifyHandler);
app = otherFramework();
api = otherFramework();
app.get("/shadowed-express", expressHandler);
api.get("/shadowed-fastify", fastifyHandler);
"#;
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let timeline = build_javascript_framework_timeline(&tree, source);
        assert_eq!(
            timeline.statement_visits,
            tree.root_node().named_child_count(),
            "the shared timeline must visit each module statement exactly once"
        );

        let express = collect_javascript_express_routes_with_timeline(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
            &timeline,
        )?;
        let fastify = collect_javascript_fastify_routes_with_timeline(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
            &timeline,
        )?;
        assert_eq!(
            express
                .iter()
                .map(|route| route.path.as_str())
                .collect::<Vec<_>>(),
            ["/express"]
        );
        assert_eq!(
            fastify
                .iter()
                .map(|route| route.path.as_str())
                .collect::<Vec<_>>(),
            ["/fastify"]
        );
        assert_eq!(
            timeline.statement_visits,
            tree.root_node().named_child_count()
        );
        Ok(())
    }

    #[test]
    fn test_fastapi_timeline_parses_multiline_aliases_and_shadowing_from_nodes() -> Result<()> {
        let source = r#"
from fastapi import (
    FastAPI as BuildApi,  # application factory
    APIRouter as BuildRouter,
)
import fastapi as api_module

app = BuildApi()
router = BuildRouter()
module_app = api_module.FastAPI()

@app.get("/app")
async def app_route(): pass

@router.post("/router")
async def router_route(): pass

@module_app.put("/module")
async def module_route(): pass

BuildRouter = OtherRouter
shadowed = BuildRouter()
@shadowed.get("/shadowed")
async def shadowed_route(): pass
"#;
        let tree = parse_python(source);
        let timeline = build_fastapi_binding_timeline(&tree, source);
        assert_eq!(
            timeline.statement_visits,
            tree.root_node().named_child_count()
        );
        let routes = collect_python_fastapi_routes_with_timeline(
            &tree_sitter_python::LANGUAGE.into(),
            &tree,
            source,
            &timeline,
        )?;
        assert_eq!(
            routes
                .iter()
                .map(|route| (route.method.as_str(), route.path.as_str()))
                .collect::<HashSet<_>>(),
            HashSet::from([("GET", "/app"), ("POST", "/router"), ("PUT", "/module")])
        );
        Ok(())
    }

    #[test]
    fn test_express_query_fixture_matrix_and_line_scan_comparison() -> Result<()> {
        let source = r#"
import express from "express";
const app = express();

app.get("/simple", simple);

app.post(
  "/multiline",
  multiline,
);

app.put(`/static-template`, update);
app.patch(`/dynamic/${itemId}`, dynamic);
app.get("/prefix" + suffix, concatenated);
app.delete(buildPath("/nested-path"), nested);
app.options("/wrapped", wrap(handler));

router.get("/unowned", unowned);

const example = `
app.head("/string-only", documented);
`;

// app.get("/comment-only", commented);
/* app.post("/block-comment", commented); */
"#;
        let expected = HashSet::from([
            ("GET".to_string(), "/simple".to_string()),
            ("POST".to_string(), "/multiline".to_string()),
            ("PUT".to_string(), "/static-template".to_string()),
            ("OPTIONS".to_string(), "/wrapped".to_string()),
        ]);
        let lexical =
            super::super::collect_framework_routes(Path::new("routes.ts"), "typescript", source)
                .into_iter()
                .filter(|route| route.framework == "express")
                .map(|route| (route.method, route.path))
                .collect::<HashSet<_>>();
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let parser_routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        let parser = parser_routes
            .iter()
            .map(|route| (route.method.clone(), route.path.clone()))
            .collect::<HashSet<_>>();

        let lexical_tp = lexical.intersection(&expected).count();
        let lexical_fp = lexical.difference(&expected).count();
        let lexical_fn = expected.difference(&lexical).count();
        let parser_tp = parser.intersection(&expected).count();
        let parser_fp = parser.difference(&expected).count();
        let parser_fn = expected.difference(&parser).count();
        eprintln!(
            "express route matrix: line_scan tp={lexical_tp} fp={lexical_fp} fn={lexical_fn}; tree_sitter_query tp={parser_tp} fp={parser_fp} fn={parser_fn}"
        );

        assert_eq!((parser_tp, parser_fp, parser_fn), (4, 0, 0));
        assert!(lexical_fp > 0 || lexical_fn > 0);
        assert!(parser_routes.iter().all(|route| {
            route.extraction_provenance == "tree_sitter_query"
                && route.claim_tier == "parser_backed"
                && route.confidence == "heuristic"
        }));
        assert_eq!(
            parser_routes
                .iter()
                .find(|route| route.path == "/wrapped")
                .and_then(|route| route.handler.as_deref()),
            None,
            "nested handlers must not become name-based handler claims"
        );
        Ok(())
    }

    #[test]
    fn test_express_indexes_javascript_jsx_typescript_and_tsx_surfaces() -> Result<()> {
        let cases = [
            ("routes.js", "js", "const marker = 'js';"),
            (
                "routes.jsx",
                "jsx",
                "export const View = () => <main>jsx</main>;",
            ),
            ("routes.ts", "ts", "const marker: string = 'ts';"),
            (
                "routes.tsx",
                "tsx",
                "export const View = () => <main>tsx</main>;",
            ),
        ];
        for (path, extension, extra) in cases {
            let source = format!(
                r#"
import express from "express";
const app = express();
app.get("/health", health);
function health() {{ return true; }}
{extra}
"#
            );
            let result = super::super::index_file(
                Path::new(path),
                &source,
                &super::super::get_language_for_ext(extension).expect("language config"),
                None,
                None,
            )?;
            let route = result
                .nodes
                .iter()
                .find(|node| {
                    node.canonical_id
                        .as_deref()
                        .is_some_and(|value| value.contains(r#""framework":"express""#))
                })
                .unwrap_or_else(|| panic!("missing Express route for {path}"));
            let metadata = route.canonical_id.as_deref().expect("route metadata");
            assert!(metadata.contains(r#""extraction_provenance":"tree_sitter_query""#));
            assert!(metadata.contains(r#""claim_tier":"parser_backed""#));
        }
        Ok(())
    }

    #[test]
    fn test_express_receiver_ownership_rejects_unowned_shadowed_and_nested_calls() -> Result<()> {
        let source = r#"
import express from "express";
const app = express();
const server = express();
app.get("/owned", owned);
server.get("/inline", () => {
  const server = otherFramework();
});
app = otherFramework();
app.get("/shadowed", shadowed);

const conditional = express();
if (flag) {
  conditional = otherFramework();
}
conditional.get("/conditionally-shadowed", conditionalHandler);

const unrelated = otherFramework();
unrelated.get("/unrelated", unrelatedHandler);

function register(app) {
  const nested = express();
  const server = otherFramework();
  app.get("/injected", injected);
}

server.get("/still-owned", stillOwned);
nested.get("/not-module-owned", notModuleOwned);
"#;
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        assert_eq!(
            routes
                .iter()
                .map(|route| route.path.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["/owned", "/inline", "/still-owned"])
        );
        Ok(())
    }

    #[test]
    fn test_express_query_ignores_hand_scanner_state_with_unrelated_tree_error() -> Result<()> {
        let source = r#"
import express from "express";
const pattern = /"/;
const app = express();
app.get("/after-regex", handler);
broken = (
"#;
        let language: Language = tree_sitter_javascript::LANGUAGE.into();
        let tree = parse_javascript(&language, source);
        assert!(tree.root_node().has_error());
        let routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::JavaScript,
            &tree,
            source,
        )?;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/after-regex");
        Ok(())
    }

    #[test]
    fn test_express_property_assignment_does_not_invalidate_receiver() -> Result<()> {
        let source = r#"
import express from "express";
const app = express();
app.locals.title = "CodeStory";
app.get("/after-property-write", handler);
"#;
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/after-property-write");
        Ok(())
    }

    #[test]
    fn test_express_shadowed_commonjs_require_is_not_provenance() -> Result<()> {
        let source = r#"
const require = fakeRequire;
const express = require("express");
const app = express();
app.get("/fake-require", handler);
"#;
        let language: Language = tree_sitter_javascript::LANGUAGE.into();
        let tree = parse_javascript(&language, source);
        assert!(!tree.root_node().has_error());
        let routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::JavaScript,
            &tree,
            source,
        )?;
        assert!(routes.is_empty());
        Ok(())
    }

    #[test]
    fn test_express_block_local_declaration_preserves_outer_receiver_but_assignment_does_not()
    -> Result<()> {
        let source = r#"
import express from "express";
const app = express();
{
  const app = otherFramework();
  app.get("/block-local", localHandler);
}
app.get("/outer", outerHandler);

const server = express();
if (flag) {
  server = otherFramework();
}
server.get("/assigned", assignedHandler);
"#;
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/outer");
        Ok(())
    }

    #[test]
    fn test_express_function_scoped_var_and_loop_target_invalidate_module_receivers() -> Result<()>
    {
        let source = r#"
import express from "express";
var app = express();
if (flag) {
  var/*comment*/ app = otherFramework();
}
app.get("/var-redeclared", handler);

var newlineApp = express();
if (flag) {
  var
  newlineApp = otherFramework();
}
newlineApp.get("/newline-var-redeclared", handler);

const server = express();
for (server of candidates) {
  consume(server);
}
server.get("/loop-assigned", handler);
"#;
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        assert!(routes.is_empty());
        Ok(())
    }

    #[test]
    fn test_express_type_only_imports_are_not_runtime_provenance() -> Result<()> {
        let cases = [
            r#"
import type express from "express";
const app = express();
app.get("/type-default", handler);
"#,
            r#"
import { type Router } from "express";
const router = Router();
router.get("/type-router", handler);
"#,
            r#"
import/*comment*/ type express from "express";
const app = express();
app.get("/commented-type-default", handler);
"#,
            r#"
import
type express from "express";
const app = express();
app.get("/newline-type-default", handler);
"#,
            r#"
import { type/*comment*/ Router } from "express";
const router = Router();
router.get("/commented-type-router", handler);
"#,
            r#"
import {
  type Router
} from "express";
const router = Router();
router.get("/newline-type-router", handler);
"#,
            r#"
import typeof express from "express";
const app = express();
app.get("/typeof-default", handler);
"#,
            r#"
import/*comment*/ typeof express from "express";
const app = express();
app.get("/commented-typeof-default", handler);
"#,
        ];
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        for source in cases {
            let tree = parse_javascript(&language, source);
            assert!(!tree.root_node().has_error(), "invalid fixture: {source}");
            let routes = collect_javascript_express_routes(
                &language,
                JavaScriptDialect::TypeScript,
                &tree,
                source,
            )?;
            assert!(routes.is_empty());
        }
        Ok(())
    }

    #[test]
    fn test_express_commonjs_and_router_constructor_provenance() -> Result<()> {
        let source = r#"
const express = require("express");
const app = express();
const router = express.Router();
const { Router: ExpressRouter } = require("express");
const aliasedRouter = ExpressRouter();
app.get("/app", appHandler);
router.post("/router", routerHandler);
aliasedRouter.patch("/aliased-router", aliasedHandler);
"#;
        let language: Language = tree_sitter_javascript::LANGUAGE.into();
        let tree = parse_javascript(&language, source);
        let routes = collect_javascript_express_routes(
            &language,
            JavaScriptDialect::JavaScript,
            &tree,
            source,
        )?;
        assert_eq!(
            routes
                .iter()
                .map(|route| route.path.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["/app", "/router", "/aliased-router"])
        );

        let named_router_source = r#"
import { Router as ExpressRouter } from "express";
const router = ExpressRouter();
router.get("/named-router", handler);
"#;
        let typescript: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&typescript, named_router_source);
        let routes = collect_javascript_express_routes(
            &typescript,
            JavaScriptDialect::TypeScript,
            &tree,
            named_router_source,
        )?;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/named-router");
        Ok(())
    }

    #[test]
    fn test_express_malformed_template_does_not_restore_string_content_as_a_route() -> Result<()> {
        let source = r#"
import express from "express";
const app = express();
const example = `
app.get("/string-only", documented);
"#;
        let result = super::super::index_file(
            Path::new("broken.ts"),
            source,
            &super::super::get_language_for_ext("ts").expect("typescript config"),
            None,
            None,
        )?;
        assert!(result.nodes.iter().all(|node| {
            !node
                .canonical_id
                .as_deref()
                .is_some_and(|value| value.contains(r#""framework":"express""#))
        }));
        Ok(())
    }

    #[test]
    fn test_fastify_query_fixture_matrix_and_line_scan_comparison() -> Result<()> {
        let source = r#"
import buildFastify from "fastify";
import { fastify as createFastify } from "fastify";
const api = buildFastify();
const secondary = createFastify();

api.get("/simple", simple);
api.post(
  "/multiline",
  multiline,
);
api.put(`/static-template`, update);
api.route({
  handler: handlers.remove,
  "url": "/object",
  method: "DELETE",
});
secondary.patch("/wrapped", wrap(handler));

api.get(`/dynamic/${itemId}`, dynamic);
api.get("/prefix" + suffix, concatenated);
api.get(buildPath("/nested-path"), nested);
api.get("/escaped\\tpath", escaped);
api.route({ method: ["GET"], url: "/array-method", handler });
api.route({ method: methodName, url: "/dynamic-method", handler });
api.route({ method: "GET", url: makeUrl(), handler });
api.route({ method: "GET", url: "/missing-handler" });
api.route({ method: "GET", method: "POST", url: "/duplicate-method", handler });
api.route({ method: "GET", url: "/spread", handler, ...dynamicOptions });

server.get("/unrelated", unrelated);
const example = `api.head("/string-only", documented);`;
// api.options("/comment-only", commented);
/* api.get("/block-comment", commented); */
"#;
        let expected = HashSet::from([
            ("GET".to_string(), "/simple".to_string()),
            ("POST".to_string(), "/multiline".to_string()),
            ("PUT".to_string(), "/static-template".to_string()),
            ("DELETE".to_string(), "/object".to_string()),
            ("PATCH".to_string(), "/wrapped".to_string()),
        ]);
        let lexical =
            super::super::collect_framework_routes(Path::new("routes.ts"), "typescript", source)
                .into_iter()
                .filter(|route| route.framework == "fastify")
                .map(|route| (route.method, route.path))
                .collect::<HashSet<_>>();
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let parser_routes = collect_javascript_fastify_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        let parser = parser_routes
            .iter()
            .map(|route| (route.method.clone(), route.path.clone()))
            .collect::<HashSet<_>>();

        let lexical_tp = lexical.intersection(&expected).count();
        let lexical_fp = lexical.difference(&expected).count();
        let lexical_fn = expected.difference(&lexical).count();
        let parser_tp = parser.intersection(&expected).count();
        let parser_fp = parser.difference(&expected).count();
        let parser_fn = expected.difference(&parser).count();
        eprintln!(
            "fastify route matrix: line_scan tp={lexical_tp} fp={lexical_fp} fn={lexical_fn}; tree_sitter_query tp={parser_tp} fp={parser_fp} fn={parser_fn}"
        );

        assert_eq!((parser_tp, parser_fp, parser_fn), (5, 0, 0));
        assert!(lexical_fp > 0 || lexical_fn > 0);
        assert!(parser_routes.iter().all(|route| {
            route.extraction_provenance == "tree_sitter_query"
                && route.claim_tier == "parser_backed"
                && route.confidence == "heuristic"
        }));
        assert_eq!(
            parser_routes
                .iter()
                .find(|route| route.path == "/object")
                .and_then(|route| route.handler.as_deref()),
            Some("handlers.remove")
        );
        assert_eq!(
            parser_routes
                .iter()
                .find(|route| route.path == "/wrapped")
                .and_then(|route| route.handler.as_deref()),
            None,
            "wrapped handlers must not become name-based handler claims"
        );
        Ok(())
    }

    #[test]
    fn test_fastify_supports_trace_and_rejects_duplicate_dynamic_urls() -> Result<()> {
        let source = r#"
import Fastify from "fastify";
const api = Fastify();
api.trace("/trace-direct", directHandler);
api.route({ method: "TRACE", url: "/trace-object", handler: objectHandler });
api.route({ method: "GET", url: dynamicUrl, url: "/duplicate", handler });
"#;
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let routes = collect_javascript_fastify_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        assert_eq!(
            routes
                .iter()
                .map(|route| (route.method.as_str(), route.path.as_str()))
                .collect::<HashSet<_>>(),
            HashSet::from([("TRACE", "/trace-direct"), ("TRACE", "/trace-object"),])
        );
        Ok(())
    }

    #[test]
    fn test_fastify_uses_all_javascript_grammars_and_import_forms() -> Result<()> {
        let cases = [
            (
                tree_sitter_javascript::LANGUAGE.into(),
                JavaScriptDialect::JavaScript,
                r#"const Fastify = require("fastify");"#,
                "Fastify()",
            ),
            (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                JavaScriptDialect::TypeScript,
                r#"import { fastify as makeServer } from "fastify";"#,
                "makeServer() as FastifyInstance",
            ),
            (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                JavaScriptDialect::Tsx,
                r#"import * as FastifyModule from "fastify";"#,
                "FastifyModule.fastify() satisfies FastifyInstance",
            ),
            (
                tree_sitter_javascript::LANGUAGE.into(),
                JavaScriptDialect::JavaScript,
                r#"const { fastify: makeServer } = require("fastify");"#,
                "makeServer()",
            ),
            (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                JavaScriptDialect::TypeScript,
                r#"const TypedFastify = require("fastify") as typeof import("fastify");"#,
                "TypedFastify()",
            ),
        ];
        for (language, dialect, import, initializer) in cases {
            let source = format!(
                "{import}\nconst arbitraryName = {initializer};\narbitraryName.get(\"/health\", {{ schema }}, health);"
            );
            let tree = parse_javascript(&language, &source);
            let routes = collect_javascript_fastify_routes(&language, dialect, &tree, &source)?;
            assert_eq!(routes.len(), 1);
            assert_eq!(routes[0].path, "/health");
        }
        Ok(())
    }

    #[test]
    fn test_fastify_receiver_provenance_invalidates_unsafe_ownership() -> Result<()> {
        let source = r#"
import Fastify from "fastify";
const api = Fastify();
api.get("/owned", owned);
api.decorate("feature", true);
api.get("/after-property-use", afterPropertyUse);

const reassigned = Fastify();
if (flag) reassigned = otherFramework();
reassigned.get("/reassigned", handler);

const unsupported = wrap(Fastify());
unsupported.get("/nested-builder", handler);
const factoryReturned = makeServer();
factoryReturned.get("/factory-returned", handler);
const unrelated = otherFramework();
unrelated.get("/unrelated", handler);

const memberAlias = require("fastify");
memberAlias.fastify = otherFramework;
const memberBuilt = memberAlias.fastify();
memberBuilt.get("/overwritten-member", handler);

const nestedAssigned = Fastify();
const assignmentResult = (nestedAssigned = otherFramework());
nestedAssigned.get("/nested-assignment", handler);

let updated = Fastify();
updated++;
updated.get("/updated", handler);

const enumReplaced = Fastify();
enum enumReplaced { Value }
enumReplaced.get("/enum-replaced", handler);

const exportedReplaced = Fastify();
export function exportedReplaced() {}
exportedReplaced.get("/exported-replaced", handler);

function register(api) {
  api.get("/injected", handler);
  const nested = Fastify();
  nested.get("/nested", handler);
}

const shadowed = Fastify();
function shadowed() {}
shadowed.get("/declaration-replaced", handler);
"#;
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tree = parse_javascript(&language, source);
        let routes = collect_javascript_fastify_routes(
            &language,
            JavaScriptDialect::TypeScript,
            &tree,
            source,
        )?;
        assert_eq!(
            routes
                .iter()
                .map(|route| route.path.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["/owned", "/after-property-use"])
        );
        Ok(())
    }

    #[test]
    fn test_fastify_clean_and_malformed_files_keep_provenance_separate() -> Result<()> {
        let clean = r#"
import Fastify from "fastify";
const api = Fastify();
api.get("/clean", handler);
"#;
        let clean_result = super::super::index_file(
            Path::new("clean.ts"),
            clean,
            &super::super::get_language_for_ext("ts").expect("typescript config"),
            None,
            None,
        )?;
        let clean_metadata = clean_result
            .nodes
            .iter()
            .find_map(|node| {
                node.canonical_id.as_deref().filter(|value| {
                    value.contains(r#""framework":"fastify""#)
                        && value.contains(r#""path":"/clean""#)
                })
            })
            .expect("clean Fastify route");
        assert!(clean_metadata.contains(r#""extraction_provenance":"tree_sitter_query""#));
        assert!(!clean_metadata.contains(r#""extraction_provenance":"lexical_fallback""#));

        let unrelated_error = r#"
import Fastify from "fastify";
const server = Fastify();
server.get("/clean", handler);
server.get("/dynamic" + suffix, handler);



broken = (
"#;
        let unrelated_result = super::super::index_file(
            Path::new("unrelated-error.ts"),
            unrelated_error,
            &super::super::get_language_for_ext("ts").expect("typescript config"),
            None,
            None,
        )?;
        let fastify_metadata = unrelated_result
            .nodes
            .iter()
            .filter_map(|node| {
                node.canonical_id
                    .as_deref()
                    .filter(|value| value.contains(r#""framework":"fastify""#))
            })
            .collect::<Vec<_>>();
        assert_eq!(fastify_metadata.len(), 1);
        assert!(fastify_metadata[0].contains(r#""path":"/clean""#));

        let malformed = r#"
const Fastify = require("fastify");
const server = Fastify();
server.get("/recover" handler);
"#;
        let malformed_result = super::super::index_file(
            Path::new("malformed.js"),
            malformed,
            &super::super::get_language_for_ext("js").expect("javascript config"),
            None,
            None,
        )?;
        let fallback_metadata = malformed_result
            .nodes
            .iter()
            .find_map(|node| {
                node.canonical_id.as_deref().filter(|value| {
                    value.contains(r#""framework":"fastify""#)
                        && value.contains(r#""path":"/recover""#)
                })
            })
            .expect("error-local Fastify fallback route");
        assert!(fallback_metadata.contains(r#""extraction_provenance":"lexical_fallback""#));
        assert!(fallback_metadata.contains(r#""claim_tier":"structural""#));
        Ok(())
    }

    #[test]
    fn test_fastify_malformed_fallback_requires_a_whole_static_path() -> Result<()> {
        let malformed = r#"
const Fastify = require("fastify");
const server = Fastify();
server.get("/concatenated" + suffix handler);
server.route({ method: "GET", schema: { url: "/nested" }, url: makeUrl("/dynamic"), handler
server.route({ schema: { method: "GET" }, url: "/nested-method", handler
server.route({ method: "GET", method: dynamicMethod, url: "/later-dynamic-method", handler
server.route({ method: "GET", "method": "POST", url: "/duplicate-method", handler
server.route({ method: "GET", url: "/later-dynamic-url", url: dynamicUrl, handler
server.route({ method: "GET", url: "/duplicate-url", "url": "/other-url", handler
server.route({ method: "GET", url: "/spread", handler, ...dynamicOptions
server.route({ method: "GET", url: "/comment-handler", /* handler: commented */
if (flag) {
  server.get("/nested-recovery" handler);
}
"#;
        let result = super::super::index_file(
            Path::new("dynamic-malformed.js"),
            malformed,
            &super::super::get_language_for_ext("js").expect("javascript config"),
            None,
            None,
        )?;
        assert!(result.nodes.iter().all(|node| {
            !node
                .canonical_id
                .as_deref()
                .is_some_and(|value| value.contains(r#""framework":"fastify""#))
        }));
        Ok(())
    }

    #[test]
    fn test_fastify_handler_edges_require_direct_resolvable_names() -> Result<()> {
        let source = r#"
import Fastify from "fastify";
const api = Fastify();
api.get("/direct", directHandler);
api.post("/inline", async () => true);
api.put("/wrapped", wrap(directHandler));
function directHandler() { return true; }
"#;
        let result = super::super::index_file(
            Path::new("handlers.ts"),
            source,
            &super::super::get_language_for_ext("ts").expect("typescript config"),
            None,
            None,
        )?;
        let route = |path: &str| {
            result
                .nodes
                .iter()
                .find(|node| {
                    node.canonical_id.as_deref().is_some_and(|value| {
                        value.contains(r#""framework":"fastify""#)
                            && value.contains(&format!(r#""path":"{path}""#))
                    })
                })
                .unwrap_or_else(|| panic!("missing Fastify route {path}"))
        };
        let handler = result
            .nodes
            .iter()
            .find(|node| node.serialized_name == "directHandler")
            .expect("direct handler");
        assert!(result.edges.iter().any(|edge| {
            edge.kind == codestory_contracts::graph::EdgeKind::CALL
                && edge.source == route("/direct").id
                && edge.target == handler.id
        }));
        for path in ["/inline", "/wrapped"] {
            assert!(result.edges.iter().all(|edge| {
                edge.kind != codestory_contracts::graph::EdgeKind::CALL
                    || edge.source != route(path).id
            }));
        }
        Ok(())
    }

    #[test]
    fn test_fastapi_query_fixture_matrix_and_line_scan_comparison() -> Result<()> {
        let source = r#"
from fastapi import Depends, FastAPI

app = FastAPI()

@app.get("/simple")
async def simple(): pass

@app.post(r"/raw/{item_id}")
async def raw_path(): pass

@app.put(
    "/multiline",
)
async def multiline(): pass

@app.patch("/nested-dependency", dependencies=[Depends(require_user())])
async def nested_dependency(): pass

@app.get(f"/users/{user_id}")
async def dynamic_template(): pass

@app.get(build_path("/nested-path"))
async def nested_path(): pass

# @app.delete("/comment-only")

EXAMPLE = """
@app.get("/docstring-only")
async def example(): pass
"""
"#;
        let expected = HashSet::from([
            ("GET".to_string(), "/simple".to_string()),
            ("POST".to_string(), "/raw/:item_id".to_string()),
            ("PUT".to_string(), "/multiline".to_string()),
            ("PATCH".to_string(), "/nested-dependency".to_string()),
        ]);
        let lexical =
            super::super::collect_framework_routes(Path::new("routes.py"), "python", source)
                .into_iter()
                .filter(|route| route.framework == "fastapi")
                .map(|route| (route.method, route.path))
                .collect::<HashSet<_>>();
        let tree = parse_python(source);
        let parser_routes =
            collect_python_fastapi_routes(&tree_sitter_python::LANGUAGE.into(), &tree, source)?;
        let parser = parser_routes
            .iter()
            .map(|route| (route.method.clone(), route.path.clone()))
            .collect::<HashSet<_>>();

        let lexical_tp = lexical.intersection(&expected).count();
        let lexical_fp = lexical.difference(&expected).count();
        let lexical_fn = expected.difference(&lexical).count();
        let parser_tp = parser.intersection(&expected).count();
        let parser_fp = parser.difference(&expected).count();
        let parser_fn = expected.difference(&parser).count();
        eprintln!(
            "fastapi route matrix: line_scan tp={lexical_tp} fp={lexical_fp} fn={lexical_fn}; tree_sitter_query tp={parser_tp} fp={parser_fp} fn={parser_fn}"
        );

        assert_eq!((lexical_tp, lexical_fp, lexical_fn), (3, 3, 1));
        assert_eq!((parser_tp, parser_fp, parser_fn), (4, 0, 0));
        assert!(parser_routes.iter().all(|route| {
            route.extraction_provenance == "tree_sitter_query"
                && route.claim_tier == "parser_backed"
                && route.confidence == "decorator"
                && route.handler.is_some()
        }));
        Ok(())
    }

    #[test]
    fn test_fastapi_indexed_route_records_parser_claim_and_handler_edge() -> Result<()> {
        let source = r#"
from fastapi import APIRouter

api = APIRouter()

@api.get(
    r"/users/{user_id}",
)
async def show_user():
    return None
"#;
        let result = super::super::index_file(
            Path::new("routes.py"),
            source,
            &super::super::get_language_for_ext("py").expect("python config"),
            None,
            None,
        )?;
        let route = result
            .nodes
            .iter()
            .find(|node| {
                node.serialized_name == "GET /users/:user_id (fastapi route; confidence=decorator)"
            })
            .expect("parser-backed FastAPI route");
        let handler = result
            .nodes
            .iter()
            .find(|node| node.serialized_name == "show_user")
            .expect("decorated handler");
        let canonical_id = route.canonical_id.as_deref().expect("route metadata");
        assert!(canonical_id.contains(r#""extraction_provenance":"tree_sitter_query""#));
        assert!(canonical_id.contains(r#""claim_tier":"parser_backed""#));
        assert!(result.edges.iter().any(|edge| {
            edge.kind == codestory_contracts::graph::EdgeKind::CALL
                && edge.source == route.id
                && edge.target == handler.id
        }));
        Ok(())
    }

    #[test]
    fn test_fastapi_syntax_error_fallback_stays_structural() -> Result<()> {
        let source = r#"from fastapi import FastAPI
app = FastAPI()
@app.get("/fallback")
async def broken(
"#;
        let result = super::super::index_file(
            Path::new("routes.py"),
            source,
            &super::super::get_language_for_ext("py").expect("python config"),
            None,
            None,
        )?;
        let route = result
            .nodes
            .iter()
            .find(|node| {
                node.serialized_name == "GET /fallback (fastapi route; confidence=heuristic)"
            })
            .expect("structural FastAPI fallback");
        let canonical_id = route.canonical_id.as_deref().expect("route metadata");
        assert!(canonical_id.contains(r#""extraction_provenance":"lexical_fallback""#));
        assert!(canonical_id.contains(r#""claim_tier":"structural""#));
        Ok(())
    }

    #[test]
    fn test_fastapi_receiver_ownership_is_module_scoped_and_invalidated_by_reassignment()
    -> Result<()> {
        let nested_scopes = r#"
from fastapi import FastAPI

def build_fastapi():
    app = FastAPI()
    @app.get("/inside-one")
    async def inside_one(): pass

def build_other():
    app = OtherFramework()
    @app.get("/inside-two")
    async def inside_two(): pass
"#;
        let reassigned = r#"
from fastapi import FastAPI

app = FastAPI()
app = OtherFramework()

@app.get("/after-reassignment")
async def after_reassignment(): pass
"#;
        let constructor_reassigned = r#"
from fastapi import FastAPI

FastAPI = OtherFactory
app = FastAPI()

@app.get("/after-constructor-reassignment")
async def after_constructor_reassignment(): pass
"#;
        let imported_constructor_shadowed = r#"
from fastapi import FastAPI
from other_framework import FastAPI

app = FastAPI()
@app.get("/shadowed-import")
async def shadowed_import(): pass
"#;
        let imported_module_shadowed = r#"
import fastapi as framework
import other_framework as framework

app = framework.FastAPI()
@app.get("/shadowed-module")
async def shadowed_module(): pass
"#;
        let class_shadowed = r#"
from fastapi import FastAPI

class FastAPI: pass
app = FastAPI()
@app.get("/shadowed-class")
async def shadowed_class(): pass
"#;
        let function_shadowed = r#"
from fastapi import FastAPI

def FastAPI(): return OtherFramework()
app = FastAPI()
@app.get("/shadowed-function")
async def shadowed_function(): pass
"#;
        let tuple_reassigned = r#"
from fastapi import FastAPI

app = FastAPI()
app, other = OtherFramework(), None
@app.get("/after-tuple-reassignment")
async def after_tuple_reassignment(): pass
"#;
        let nested_destructuring_reassigned = r#"
from fastapi import FastAPI

app = FastAPI()
(app, (other,)) = (OtherFramework(), (None,))
@app.get("/after-nested-reassignment")
async def after_nested_reassignment(): pass
"#;
        let chained_reassigned = r#"
from fastapi import APIRouter

router = APIRouter()
app = router = OtherFramework()
@router.get("/after-chained-reassignment")
async def after_chained_reassignment(): pass
"#;
        let chained_fastapi_construction = r#"
from fastapi import FastAPI

app = router = FastAPI()
@app.get("/ambiguous-chained-construction")
async def ambiguous_chained_construction(): pass
"#;
        for source in [
            nested_scopes,
            reassigned,
            constructor_reassigned,
            imported_constructor_shadowed,
            imported_module_shadowed,
            class_shadowed,
            function_shadowed,
            tuple_reassigned,
            nested_destructuring_reassigned,
            chained_reassigned,
            chained_fastapi_construction,
        ] {
            let result = super::super::index_file(
                Path::new("scopes.py"),
                source,
                &super::super::get_language_for_ext("py").expect("python config"),
                None,
                None,
            )?;
            assert!(result.nodes.iter().all(|node| {
                !node
                    .canonical_id
                    .as_deref()
                    .is_some_and(|value| value.contains(r#""framework":"fastapi""#))
            }));
        }
        Ok(())
    }

    #[test]
    fn test_fastapi_unsupported_decorators_and_nonliteral_paths_do_not_emit_exact_routes()
    -> Result<()> {
        let source = r#"
from fastapi import FastAPI

app = FastAPI()

@app.get(path="/keyword")
async def keyword_path(): pass

@app.get("/escaped\x2fpath")
async def escaped_path(): pass

@app.head("/head")
async def head_path(): pass

@app.options("/options")
async def options_path(): pass

@app.api_route("/api-route", methods=["GET"])
async def api_route_path(): pass

@app.websocket("/socket")
async def websocket_path(): pass
"#;
        let routes = collect_python_fastapi_routes(
            &tree_sitter_python::LANGUAGE.into(),
            &parse_python(source),
            source,
        )?;
        assert!(
            routes.is_empty(),
            "unsupported shapes must not emit exact routes"
        );
        Ok(())
    }

    #[test]
    fn test_fastapi_unrelated_parse_error_does_not_restore_dynamic_or_docstring_routes()
    -> Result<()> {
        let cases = [
            r#"
from fastapi import FastAPI
app = FastAPI()
EXAMPLE = """
@app.get("/docstring-only")
async def documented(): pass
"""
broken = (
"#,
            r#"
from fastapi import FastAPI
app = FastAPI()
@app.get(f"/dynamic/{item_id}")
async def broken(
"#,
            r#"
from fastapi import FastAPI
app = FastAPI()
@app.get(build_path("/nested"))
async def broken(
"#,
        ];
        for source in cases {
            let result = super::super::index_file(
                Path::new("broken.py"),
                source,
                &super::super::get_language_for_ext("py").expect("python config"),
                None,
                None,
            )?;
            assert!(result.nodes.iter().all(|node| {
                !node
                    .canonical_id
                    .as_deref()
                    .is_some_and(|value| value.contains(r#""framework":"fastapi""#))
            }));
        }
        Ok(())
    }
}
