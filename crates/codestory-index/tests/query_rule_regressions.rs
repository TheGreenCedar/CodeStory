use codestory_core::{Edge, EdgeKind, Node, NodeKind};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::fs;
use tempfile::tempdir;

fn index_project(files: &[(&str, &str)]) -> anyhow::Result<(Vec<Node>, Vec<Edge>)> {
    let dir = tempdir()?;
    let root = dir.path();
    let mut files_to_index = Vec::with_capacity(files.len());
    for (relative_path, contents) in files {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
        files_to_index.push(path);
    }

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_project::RefreshInfo {
        files_to_index,
        files_to_remove: vec![],
    };
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let errors = storage.get_errors(None)?;
    anyhow::ensure!(errors.is_empty(), "indexing errors: {errors:?}");
    Ok((storage.get_nodes()?, storage.get_edges()?))
}

fn matches_name(actual: &str, wanted: &str) -> bool {
    actual == wanted
        || actual.ends_with(&format!(".{wanted}"))
        || actual.ends_with(&format!("::{wanted}"))
        || actual.ends_with(&format!(" {wanted}"))
}

fn has_node_kind(nodes: &[Node], name: &str, kind: NodeKind) -> bool {
    nodes.iter()
        .any(|node| matches_name(&node.serialized_name, name) && node.kind == kind)
}

fn edge_between(nodes: &[Node], edges: &[Edge], kind: EdgeKind, source: &str, target: &str) -> bool {
    let source_ids = nodes
        .iter()
        .filter(|node| matches_name(&node.serialized_name, source))
        .map(|node| node.id)
        .collect::<Vec<_>>();
    let target_ids = nodes
        .iter()
        .filter(|node| matches_name(&node.serialized_name, target))
        .map(|node| node.id)
        .collect::<Vec<_>>();
    !source_ids.is_empty()
        && !target_ids.is_empty()
        && edges.iter().any(|edge| {
            let edge_source = edge.resolved_source.unwrap_or(edge.source);
            let edge_target = edge.resolved_target.unwrap_or(edge.target);
            edge.kind == kind
                && source_ids.contains(&edge_source)
                && target_ids.contains(&edge_target)
        })
}

fn edge_between_matching(
    nodes: &[Node],
    edges: &[Edge],
    kind: EdgeKind,
    source_predicate: impl Fn(&str) -> bool,
    target_predicate: impl Fn(&str) -> bool,
) -> bool {
    edges.iter().any(|edge| {
        if edge.kind != kind {
            return false;
        }
        let source_name = nodes
            .iter()
            .find(|node| node.id == edge.resolved_source.unwrap_or(edge.source))
            .map(|node| node.serialized_name.as_str())
            .unwrap_or("");
        let target_name = nodes
            .iter()
            .find(|node| node.id == edge.resolved_target.unwrap_or(edge.target))
            .map(|node| node.serialized_name.as_str())
            .unwrap_or("");
        source_predicate(source_name) && target_predicate(target_name)
    })
}

fn call_edge_count(edges: &[Edge]) -> usize {
    edges.iter().filter(|edge| edge.kind == EdgeKind::CALL).count()
}

#[test]
fn test_javascript_import_aliases_and_computed_methods_are_precise() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.js",
        r#"
import Foo, { bar as baz } from "./pkg.js";

class Base {}

class Example extends Base {
  ["computed"]() {
    return baz();
  }
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "\"./pkg.js\"", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "Foo", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "baz", NodeKind::MODULE));
    assert!(
        !nodes.iter().any(|node| node.serialized_name.contains("import Foo")),
        "whole import statement should not be materialized as a module node"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Example", "Base"),
        "expected Example -> Base inheritance edge"
    );
    assert!(has_node_kind(&nodes, "[\"computed\"]", NodeKind::METHOD));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Example", "[\"computed\"]"),
        "expected computed method membership edge"
    );

    Ok(())
}

#[test]
fn test_typescript_aliases_and_interface_inheritance_stay_well_typed() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.ts",
        r#"
import Foo, { bar as baz } from "./pkg";

interface IFoo {}
interface IBase {}
interface Child extends IBase {}

class Base {}
class Example extends Base implements IFoo {
  value = baz;
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "\"./pkg\"", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "Foo", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "baz", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "IFoo", NodeKind::INTERFACE));
    assert!(has_node_kind(&nodes, "IBase", NodeKind::INTERFACE));
    assert!(
        !nodes.iter().any(|node| {
            node.kind == NodeKind::CLASS
                && (matches_name(&node.serialized_name, "IFoo")
                    || matches_name(&node.serialized_name, "IBase"))
        }),
        "interface inheritance targets should not be re-materialized as CLASS nodes"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Example", "Base"),
        "expected Example -> Base inheritance edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Example", "IFoo"),
        "expected Example -> IFoo inheritance edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Child", "IBase"),
        "expected Child -> IBase inheritance edge"
    );

    Ok(())
}

#[test]
fn test_tsx_parenthesized_jsx_component_and_props_are_tracked() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "App.tsx",
        r#"
type Props = { label: string };

function Badge(props: Props) {
  return <span>{props.label}</span>;
}

function App() {
  return (
    <Badge label="hello" variant="primary"></Badge>
  );
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Props", NodeKind::TYPEDEF));
    assert!(has_node_kind(&nodes, "Badge", NodeKind::FUNCTION));
    assert!(has_node_kind(&nodes, "App", NodeKind::FUNCTION));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "App", "Badge"),
        "expected TSX JSX usage to retain App -> Badge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "App", "label"),
        "expected TSX JSX usage to retain App -> label"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "App", "variant"),
        "expected TSX JSX usage to retain App -> variant"
    );

    Ok(())
}

#[test]
fn test_python_relative_imports_type_parameters_and_decorated_methods() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.py",
        r#"
from .pkg import helper as alias

def deco(fn):
    return fn

class Box[T]:
    @deco
    def run(self):
        return alias()
"#,
    )])?;

    assert!(has_node_kind(&nodes, ".pkg", NodeKind::MODULE));
    assert!(
        !has_node_kind(&nodes, "helper", NodeKind::MODULE),
        "imported symbol names should not be forced to MODULE"
    );
    assert!(
        !has_node_kind(&nodes, "alias", NodeKind::MODULE),
        "import aliases should not be forced to MODULE"
    );
    assert!(has_node_kind(&nodes, "T", NodeKind::TYPE_PARAMETER));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Box", "T"),
        "expected Box -> T type-parameter membership edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Box", "run"),
        "expected decorated method to remain a class member"
    );

    Ok(())
}

#[test]
fn test_python_multi_imports_and_call_decorators_do_not_duplicate_nodes() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.py",
        r#"
from helpers.future import annotations
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass

def trace(fn):
    return fn

@trace
@dataclass(frozen=True)
class Example:
    value: int
"#,
    )])?;

    assert!(has_node_kind(&nodes, "helpers.future", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "concurrent.futures", NodeKind::MODULE));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "annotations",
        "helpers.future"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "ThreadPoolExecutor",
        "concurrent.futures"
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "as_completed",
        "concurrent.futures"
    ));
    assert!(
        edge_between_matching(
            &nodes,
            &edges,
            EdgeKind::USAGE,
            |source| matches_name(source, "Example"),
            |target| target.contains("trace"),
        ),
        "expected Example to retain trace decorator usage"
    );
    assert!(
        edge_between_matching(
            &nodes,
            &edges,
            EdgeKind::USAGE,
            |source| matches_name(source, "Example"),
            |target| target.contains("dataclass"),
        ),
        "expected Example to retain dataclass decorator usage"
    );

    Ok(())
}

#[test]
fn test_python_single_call_decorator_does_not_hit_identifier_rule() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "main.py",
        r#"
from dataclasses import dataclass

@dataclass(frozen=True)
class RepoEntry:
    name: str
"#,
    )])?;

    assert!(
        edge_between_matching(
            &nodes,
            &edges,
            EdgeKind::USAGE,
            |source| matches_name(source, "RepoEntry"),
            |target| target.contains("dataclass"),
        ),
        "expected RepoEntry to retain dataclass decorator usage"
    );

    Ok(())
}

#[test]
fn test_java_fallback_calls_and_modern_type_declarations() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "Main.java",
        r#"
record Pair(int x, int y) {}

enum Mode {
    ON,
    OFF
}

interface Runner {}

class Example implements Runner {
    int foo() {
        return 1;
    }

    int bar() {
        return foo();
    }
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Pair", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "Mode", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "Runner", NodeKind::INTERFACE));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Example", "Runner"),
        "expected Example -> Runner inheritance edge"
    );
    assert_eq!(
        call_edge_count(&edges),
        1,
        "fallback call extraction should avoid duplicate Java CALL edges"
    );
    assert!(
        edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .all(|edge| edge.source != edge.target),
        "Java CALL edges should not remain reflexive self-loops after attribution"
    );

    Ok(())
}
