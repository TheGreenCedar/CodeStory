use super::FrameworkRoute;
use anyhow::{Result, anyhow};
use std::sync::OnceLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Query, QueryCursor, Tree};

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
                "decorator" => line = Some(capture.node.start_position().row as u32 + 1),
                _ => {}
            }
        }

        let (Some(receiver), Some(method), Some(path), Some(handler), Some(line)) =
            (receiver, method, path, handler, line)
        else {
            continue;
        };
        if !matches!(receiver.as_str(), "app" | "router")
            || !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete")
        {
            continue;
        }
        routes.push(
            FrameworkRoute::new(
                "fastapi",
                method.to_ascii_uppercase(),
                path,
                Some(handler),
                line,
                "decorator",
            )
            .with_claim_evidence("tree_sitter_query", "parser_backed"),
        );
    }

    Ok(routes)
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
    (content_end >= content_start && text[content_end..].chars().all(|ch| ch == quote))
        .then(|| text[content_start..content_end].to_string())
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
@router.get(
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
        let source = "@app.get(\"/fallback\")\nasync def broken(\n";
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
}
