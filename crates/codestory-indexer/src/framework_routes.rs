use super::FrameworkRoute;
use anyhow::{Result, anyhow};
use std::collections::HashSet;
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

#[derive(Default)]
struct FastApiBindings {
    constructors: HashSet<String>,
    modules: HashSet<String>,
    receivers: HashSet<String>,
}

pub(super) fn collect_python_fastapi_routes(
    language: &Language,
    tree: &Tree,
    source: &str,
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
        if module_fastapi_receiver_owned_at(tree, source, &receiver, route_start_byte) {
            routes.push(route.with_claim_evidence("tree_sitter_query", "parser_backed"));
        }
    }

    Ok(routes)
}

pub(super) fn allow_python_fastapi_lexical_fallback(
    tree: &Tree,
    source: &str,
    route: &FrameworkRoute,
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
        && module_fastapi_receiver_owned_at(
            tree,
            source,
            receiver,
            source_byte_at_line(source, route.line),
        )
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
                && let Some(statement) = node_text(node, source)
                && let Some((_, imported)) = statement.rsplit_once(" import ")
            {
                for spec in imported
                    .trim_matches(|ch: char| ch == '(' || ch == ')' || ch.is_whitespace())
                    .split(',')
                {
                    let mut parts = spec.split_whitespace();
                    let Some(name) = parts.next() else {
                        continue;
                    };
                    let alias = match (parts.next(), parts.next()) {
                        (Some("as"), Some(alias)) => alias,
                        _ => name,
                    };
                    if matches!(name, "FastAPI" | "APIRouter") {
                        bindings.constructors.insert(alias.to_string());
                    }
                }
            }
        }
        "import_statement" => {
            if let Some(statement) = node_text(node, source)
                && let Some(imported) = statement.strip_prefix("import ")
            {
                for spec in imported.split(',') {
                    let mut parts = spec.split_whitespace();
                    if parts.next() != Some("fastapi") {
                        continue;
                    }
                    let alias = match (parts.next(), parts.next()) {
                        (Some("as"), Some(alias)) => alias,
                        _ => "fastapi",
                    };
                    bindings.modules.insert(alias.to_string());
                }
            }
        }
        _ => {}
    }
}

fn module_fastapi_receiver_owned_at(
    tree: &Tree,
    source: &str,
    receiver: &str,
    before_byte: usize,
) -> bool {
    let mut bindings = FastApiBindings::default();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.start_byte() >= before_byte {
            break;
        }
        if let Some(assignment) = module_assignment(statement) {
            apply_fastapi_assignment(assignment, source, &mut bindings);
        } else {
            invalidate_module_statement_bindings(statement, source, &mut bindings);
            apply_fastapi_import(statement, source, &mut bindings);
        }
    }
    bindings.receivers.contains(receiver)
}

fn invalidate_module_statement_bindings(
    statement: Node<'_>,
    source: &str,
    bindings: &mut FastApiBindings,
) {
    if node_is_star_import(statement, source) {
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

fn node_is_star_import(node: Node<'_>, source: &str) -> bool {
    node.kind() == "import_from_statement"
        && node_text(node, source).is_some_and(|statement| {
            statement
                .rsplit_once(" import ")
                .is_some_and(|(_, imported)| imported.trim() == "*")
        })
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
    let Some(statement) = node_text(node, source) else {
        return;
    };
    let Some((_, imported)) = statement.rsplit_once(" import ") else {
        return;
    };
    for spec in imported
        .trim_matches(|ch: char| ch == '(' || ch == ')' || ch.is_whitespace())
        .split(',')
    {
        let mut parts = spec.split_whitespace();
        let Some(imported_name) = parts.next() else {
            continue;
        };
        if imported_name == "*" {
            continue;
        }
        let bound = match (parts.next(), parts.next()) {
            (Some("as"), Some(alias)) => alias,
            _ => imported_name,
        };
        names.insert(bound.to_string());
    }
}

fn collect_import_bound_names(node: Node<'_>, source: &str, names: &mut HashSet<String>) {
    let Some(statement) = node_text(node, source) else {
        return;
    };
    let Some(imported) = statement.strip_prefix("import ") else {
        return;
    };
    for spec in imported.split(',') {
        let mut parts = spec.split_whitespace();
        let Some(module) = parts.next() else {
            continue;
        };
        let bound = match (parts.next(), parts.next()) {
            (Some("as"), Some(alias)) => alias,
            _ => module.split('.').next().unwrap_or(module),
        };
        names.insert(bound.to_string());
    }
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
    fn test_fastapi_unowned_receiver_and_unrelated_app_are_ignored() -> Result<()> {
        let imported = r#"
from fastapi import APIRouter

@router.get("/factory-owned-elsewhere")
async def external_router(): pass
"#;
        let imported_result = super::super::index_file(
            Path::new("imported.py"),
            imported,
            &super::super::get_language_for_ext("py").expect("python config"),
            None,
            None,
        )?;
        assert!(imported_result.nodes.iter().all(|node| {
            !node
                .canonical_id
                .as_deref()
                .is_some_and(|value| value.contains(r#""framework":"fastapi""#))
        }));

        let unrelated = r#"
app = OtherFramework()

@app.get("/not-fastapi")
async def unrelated_handler(): pass
"#;
        let unrelated_result = super::super::index_file(
            Path::new("unrelated.py"),
            unrelated,
            &super::super::get_language_for_ext("py").expect("python config"),
            None,
            None,
        )?;
        assert!(unrelated_result.nodes.iter().all(|node| {
            !node
                .canonical_id
                .as_deref()
                .is_some_and(|value| value.contains(r#""framework":"fastapi""#))
        }));
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
