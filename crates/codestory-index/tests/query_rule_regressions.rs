use codestory_core::{Edge, EdgeKind, Node, NodeKind};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::collections::HashMap;
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
    nodes
        .iter()
        .any(|node| matches_name(&node.serialized_name, name) && node.kind == kind)
}

fn edge_between(
    nodes: &[Node],
    edges: &[Edge],
    kind: EdgeKind,
    source: &str,
    target: &str,
) -> bool {
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
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .count()
}

fn snippet_for_node<'a>(source: &'a str, node: &Node) -> Option<&'a str> {
    let start_line = node.start_line?;
    let end_line = node.end_line?;
    let start_col = node.start_col?;
    let end_col = node.end_col?;
    if start_line != end_line {
        return None;
    }
    let line = source.lines().nth(start_line as usize - 1)?;
    line.get(start_col as usize - 1..end_col as usize - 1)
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
    assert!(
        nodes
            .iter()
            .any(|node| matches_name(&node.serialized_name, "Foo"))
            && !has_node_kind(&nodes, "Foo", NodeKind::MODULE),
        "imported default binding should exist without being typed as MODULE"
    );
    assert!(
        nodes
            .iter()
            .any(|node| matches_name(&node.serialized_name, "baz"))
            && !has_node_kind(&nodes, "baz", NodeKind::MODULE),
        "import alias should exist without being typed as MODULE"
    );
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "Foo",
        "\"./pkg.js\""
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "baz",
        "\"./pkg.js\""
    ));
    assert!(
        !nodes
            .iter()
            .any(|node| node.serialized_name.contains("import Foo")),
        "whole import statement should not be materialized as a module node"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::INHERITANCE, "Example", "Base"),
        "expected Example -> Base inheritance edge"
    );
    assert!(has_node_kind(&nodes, "[\"computed\"]", NodeKind::METHOD));
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "Example",
            "[\"computed\"]"
        ),
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
    assert!(
        nodes
            .iter()
            .any(|node| matches_name(&node.serialized_name, "Foo"))
            && !has_node_kind(&nodes, "Foo", NodeKind::MODULE),
        "imported default binding should exist without being typed as MODULE"
    );
    assert!(
        nodes
            .iter()
            .any(|node| matches_name(&node.serialized_name, "baz"))
            && !has_node_kind(&nodes, "baz", NodeKind::MODULE),
        "import alias should exist without being typed as MODULE"
    );
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "Foo",
        "\"./pkg\""
    ));
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "baz",
        "\"./pkg\""
    ));
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
        edge_between(&nodes, &edges, EdgeKind::CALL, "App", "Badge"),
        "expected TSX JSX component invocation to become App -> Badge CALL"
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
fn test_tsx_imported_component_bindings_stay_non_module_and_callable() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[
        (
            "App.tsx",
            r#"
import BadgeView from "./Badge";

function App() {
  return <BadgeView label="hello" />;
}
"#,
        ),
        (
            "Badge.tsx",
            r#"
export default function Badge(props: { label: string }) {
  return <span>{props.label}</span>;
}
"#,
        ),
    ])?;

    assert!(has_node_kind(&nodes, "\"./Badge\"", NodeKind::MODULE));
    assert!(
        nodes
            .iter()
            .any(|node| matches_name(&node.serialized_name, "BadgeView"))
            && !has_node_kind(&nodes, "BadgeView", NodeKind::MODULE),
        "expected imported TSX component binding to exist without being typed as MODULE"
    );
    assert!(edge_between(
        &nodes,
        &edges,
        EdgeKind::IMPORT,
        "BadgeView",
        "\"./Badge\""
    ));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::CALL, "App", "BadgeView"),
        "expected imported TSX component binding to remain callable from JSX"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "App", "label"),
        "expected imported TSX component invocation to retain prop usage"
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
    assert!(has_node_kind(
        &nodes,
        "concurrent.futures",
        NodeKind::MODULE
    ));
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
            EdgeKind::CALL,
            |source| matches_name(source, "Example"),
            |target| target.contains("trace"),
        ),
        "expected Example to retain trace decorator call"
    );
    assert!(
        edge_between_matching(
            &nodes,
            &edges,
            EdgeKind::CALL,
            |source| matches_name(source, "Example"),
            |target| target.contains("dataclass"),
        ),
        "expected Example to retain dataclass decorator call"
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
            EdgeKind::CALL,
            |source| matches_name(source, "RepoEntry"),
            |target| target.contains("dataclass"),
        ),
        "expected RepoEntry to retain dataclass decorator call"
    );

    Ok(())
}

#[test]
fn test_tsx_arrow_component_owners_and_nested_callables_keep_calls_scoped() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "App.tsx",
        r#"
type Props = { label: string };

const Badge = (props: Props) => <span>{props.label}</span>;
const Inner = () => <div />;

const App = () => {
    const nested = function NestedView() {
        return <Inner />;
    };

    return <Badge label="hello" />;
};
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Badge", NodeKind::FUNCTION));
    assert!(has_node_kind(&nodes, "Inner", NodeKind::FUNCTION));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::CALL, "App", "Badge"),
        "expected App arrow function to own the Badge JSX call"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::USAGE, "App", "label"),
        "expected App arrow function to retain prop usage"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::CALL, "nested", "Inner"),
        "expected nested callable to own the Inner JSX call"
    );
    assert!(
        !edge_between(&nodes, &edges, EdgeKind::CALL, "App", "Inner"),
        "expected nested callable boundary to prevent App from owning Inner JSX"
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

#[test]
fn test_java_annotations_constructors_inner_members_and_enum_constants_surface()
-> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "Main.java",
        r#"
@interface Marker {}

@Marker
class Example {
    Example() {}

    class InnerClass {}
    interface InnerInterface {}
    enum InnerEnum { On, Off }
    record InnerRecord(int value) {
        InnerRecord {}
    }
    @interface InnerAnnotation {}
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Marker", NodeKind::ANNOTATION));
    assert!(has_node_kind(
        &nodes,
        "InnerAnnotation",
        NodeKind::ANNOTATION
    ));
    assert!(has_node_kind(&nodes, "InnerEnum", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "On", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "Off", NodeKind::ENUM_CONSTANT));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Example", "InnerClass"),
        "expected Example -> InnerClass member edge"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "Example",
            "InnerInterface"
        ),
        "expected Example -> InnerInterface member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Example", "InnerEnum"),
        "expected Example -> InnerEnum member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Example", "InnerRecord"),
        "expected Example -> InnerRecord member edge"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "Example",
            "InnerAnnotation"
        ),
        "expected Example -> InnerAnnotation member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "InnerEnum", "On"),
        "expected InnerEnum -> On member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "InnerEnum", "Off"),
        "expected InnerEnum -> Off member edge"
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::ANNOTATION_USAGE),
        "expected annotation usage edges for @Marker"
    );

    let example_class_id = nodes
        .iter()
        .find(|node| matches_name(&node.serialized_name, "Example") && node.kind == NodeKind::CLASS)
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected Example class node"))?;
    let example_ctor_id = nodes
        .iter()
        .find(|node| {
            matches_name(&node.serialized_name, "Example") && node.kind == NodeKind::METHOD
        })
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected Example constructor node"))?;
    let inner_record_id = nodes
        .iter()
        .find(|node| {
            matches_name(&node.serialized_name, "InnerRecord") && node.kind == NodeKind::CLASS
        })
        .map(|node| node.id)
        .ok_or_else(|| anyhow::anyhow!("expected InnerRecord node"))?;
    let inner_record_ctor_id = nodes
        .iter()
        .filter(|node| {
            matches_name(&node.serialized_name, "InnerRecord") && node.kind == NodeKind::METHOD
        })
        .map(|node| node.id)
        .next()
        .ok_or_else(|| anyhow::anyhow!("expected InnerRecord compact constructor node"))?;

    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == example_class_id
                && edge.target == example_ctor_id
        }),
        "expected Example constructor MEMBER edge"
    );
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == inner_record_id
                && edge.target == inner_record_ctor_id
        }),
        "expected InnerRecord compact constructor MEMBER edge"
    );

    Ok(())
}

#[test]
fn test_java_annotations_constructors_inner_types_and_enum_constants_are_covered()
-> anyhow::Result<()> {
    let source = r#"
@interface Marker {}

enum Mode {
    ON,
    OFF
}

class Outer {
    @Deprecated
    Outer() {}

    class Inner {}
    interface NestedFace {}
    enum NestedMode { HOT }
    record NestedRecord(int value) {
        NestedRecord {}
    }
    @interface NestedMarker {}
}
"#;
    let (nodes, edges) = index_project(&[("Main.java", source)])?;
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();

    assert!(has_node_kind(&nodes, "Marker", NodeKind::ANNOTATION));
    assert!(has_node_kind(&nodes, "Mode", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "ON", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "OFF", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "Outer", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "Inner", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "NestedFace", NodeKind::INTERFACE));
    assert!(has_node_kind(&nodes, "NestedMode", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "NestedRecord", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "NestedMarker", NodeKind::ANNOTATION));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Mode", "ON"),
        "expected enum constant membership for Mode -> ON"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "Inner"),
        "expected Outer -> Inner member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "NestedFace"),
        "expected Outer -> NestedFace member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "NestedMode"),
        "expected Outer -> NestedMode member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "NestedRecord"),
        "expected Outer -> NestedRecord member edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "NestedMarker"),
        "expected Outer -> NestedMarker member edge"
    );

    let constructor = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::MEMBER)
        .find_map(|edge| {
            let source = node_by_id.get(&edge.source)?;
            let target = node_by_id.get(&edge.target)?;
            (matches_name(&source.serialized_name, "Outer")
                && matches_name(&target.serialized_name, "Outer")
                && target.kind == NodeKind::METHOD)
                .then_some(*target)
        })
        .expect("expected Outer constructor member edge");
    assert_eq!(constructor.kind, NodeKind::METHOD);

    let deprecated = nodes
        .iter()
        .find(|node| {
            node.kind == NodeKind::ANNOTATION && matches_name(&node.serialized_name, "Deprecated")
        })
        .expect("expected Deprecated annotation node");
    assert_eq!(
        snippet_for_node(source, deprecated),
        Some("Deprecated"),
        "expected Java annotation usage span to cover only the annotation token"
    );
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::ANNOTATION_USAGE && edge.target == deprecated.id
        }),
        "expected an ANNOTATION_USAGE edge for @Deprecated"
    );

    Ok(())
}

#[test]
fn test_c_enums_unions_and_forward_declarations_surface_precisely() -> anyhow::Result<()> {
    let source = r#"
enum Mode { ON, OFF };
union Value {
    int i;
    float f;
};
int forward(int value);
"#;
    let (nodes, edges) = index_project(&[("main.c", source)])?;

    assert!(has_node_kind(&nodes, "Mode", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "ON", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "OFF", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "Value", NodeKind::UNION));
    assert!(has_node_kind(&nodes, "forward", NodeKind::FUNCTION));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Mode", "ON"),
        "expected enum constant membership edge for ON"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Mode", "OFF"),
        "expected enum constant membership edge for OFF"
    );
    let forward = nodes
        .iter()
        .find(|node| matches_name(&node.serialized_name, "forward"))
        .ok_or_else(|| anyhow::anyhow!("expected forward declaration node"))?;
    assert_eq!(
        snippet_for_node(source, forward),
        Some("int forward(int value);"),
        "expected C forward declaration to retain its full declaration span"
    );

    Ok(())
}

#[test]
fn test_cpp_qualified_calls_using_directives_and_namespace_aliases_stay_precise()
-> anyhow::Result<()> {
    let source = r#"
namespace ns {
int make() {
    return 1;
}
}

namespace alias = ns;
using namespace ns;

int run() {
    return ns::make() + alias::make();
}
"#;
    let (nodes, edges) = index_project(&[("main.cpp", source)])?;

    assert!(
        edge_between(&nodes, &edges, EdgeKind::CALL, "run", "make"),
        "expected qualified C++ calls to surface as CALL edges"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::IMPORT, "ns", "ns"),
        "expected using namespace ns; to surface as an IMPORT edge"
    );
    assert!(
        nodes
            .iter()
            .any(|node| matches_name(&node.serialized_name, "alias"))
            && !has_node_kind(&nodes, "alias", NodeKind::MODULE),
        "expected namespace alias binding to stay non-MODULE"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::IMPORT, "alias", "ns"),
        "expected namespace alias import edge"
    );
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let make_target = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .find(|node| matches_name(&node.serialized_name, "make"))
        .ok_or_else(|| anyhow::anyhow!("expected make call placeholder node"))?;
    assert_eq!(
        snippet_for_node(source, make_target),
        Some("make"),
        "expected C++ qualified call placeholder to cover only the terminal identifier"
    );

    Ok(())
}

#[test]
fn test_java_annotations_constructors_inner_types_and_enum_constants_surface() -> anyhow::Result<()>
{
    let (nodes, edges) = index_project(&[(
        "Main.java",
        r#"
@interface Marker {}

@Marker
class Outer {
    Outer() {}

    class Inner {}
    interface Nested {}
    enum Mode { ON }
    record Pair(int value) {
        Pair {}
    }
    @interface Flag {}
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Marker", NodeKind::ANNOTATION));
    assert!(has_node_kind(&nodes, "Outer", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "Outer", NodeKind::METHOD));
    assert!(has_node_kind(&nodes, "Pair", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "Pair", NodeKind::METHOD));
    assert!(has_node_kind(&nodes, "Flag", NodeKind::ANNOTATION));
    assert!(has_node_kind(&nodes, "ON", NodeKind::ENUM_CONSTANT));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "Inner"),
        "expected inner class membership edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "Nested"),
        "expected inner interface membership edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "Mode"),
        "expected inner enum membership edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "Pair"),
        "expected inner record membership edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Outer", "Flag"),
        "expected inner annotation membership edge"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Mode", "ON"),
        "expected enum constant membership edge"
    );
    assert!(
        edge_between_matching(
            &nodes,
            &edges,
            EdgeKind::ANNOTATION_USAGE,
            |source| matches_name(source, "Marker"),
            |target| matches_name(target, "Marker"),
        ),
        "expected Marker annotation usage edge"
    );

    Ok(())
}

#[test]
fn test_java_enum_and_annotation_parents_surface_inner_type_members() -> anyhow::Result<()> {
    let (nodes, edges) = index_project(&[(
        "Main.java",
        r#"
enum Host {
    ONLY;

    class InnerClass {}
    interface InnerInterface {}
    enum InnerEnum { ON }
    record InnerRecord(int value) {}
    @interface InnerAnnotation {}
}

@interface Container {
    class NestedClass {}
    interface NestedInterface {}
    enum NestedEnum { OFF }
    @interface NestedAnnotation {}
}
"#,
    )])?;

    assert!(has_node_kind(&nodes, "Host", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "ONLY", NodeKind::ENUM_CONSTANT));
    assert!(has_node_kind(&nodes, "Container", NodeKind::ANNOTATION));
    assert!(has_node_kind(&nodes, "InnerClass", NodeKind::CLASS));
    assert!(has_node_kind(&nodes, "InnerInterface", NodeKind::INTERFACE));
    assert!(has_node_kind(&nodes, "InnerEnum", NodeKind::ENUM));
    assert!(has_node_kind(&nodes, "InnerRecord", NodeKind::CLASS));
    assert!(has_node_kind(
        &nodes,
        "InnerAnnotation",
        NodeKind::ANNOTATION
    ));
    assert!(has_node_kind(&nodes, "NestedClass", NodeKind::CLASS));
    assert!(has_node_kind(
        &nodes,
        "NestedInterface",
        NodeKind::INTERFACE
    ));
    assert!(has_node_kind(&nodes, "NestedEnum", NodeKind::ENUM));
    assert!(has_node_kind(
        &nodes,
        "NestedAnnotation",
        NodeKind::ANNOTATION
    ));
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Host", "ONLY"),
        "expected enum constant membership edge for Host -> ONLY"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Host", "InnerClass"),
        "expected enum parent member edge for inner class"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Host", "InnerInterface"),
        "expected enum parent member edge for inner interface"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Host", "InnerEnum"),
        "expected enum parent member edge for inner enum"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Host", "InnerRecord"),
        "expected enum parent member edge for inner record"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Host", "InnerAnnotation"),
        "expected enum parent member edge for inner annotation"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Container", "NestedClass"),
        "expected annotation parent member edge for nested class"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "Container",
            "NestedInterface"
        ),
        "expected annotation parent member edge for nested interface"
    );
    assert!(
        edge_between(&nodes, &edges, EdgeKind::MEMBER, "Container", "NestedEnum"),
        "expected annotation parent member edge for nested enum"
    );
    assert!(
        edge_between(
            &nodes,
            &edges,
            EdgeKind::MEMBER,
            "Container",
            "NestedAnnotation"
        ),
        "expected annotation parent member edge for nested annotation"
    );

    Ok(())
}
