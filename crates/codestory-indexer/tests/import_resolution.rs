use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{EdgeKind, NodeKind};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use std::fs;
use tempfile::tempdir;

fn index_single_file(
    filename: &str,
    contents: &str,
) -> anyhow::Result<Vec<codestory_contracts::graph::Edge>> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join(filename);
    fs::write(&file_path, contents)?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![file_path.clone()],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    Ok(storage.get_edges()?)
}

fn index_workspace(
    files: &[(&str, &str)],
) -> anyhow::Result<(
    Vec<codestory_contracts::graph::Node>,
    Vec<codestory_contracts::graph::Edge>,
)> {
    let dir = tempdir()?;
    let root = dir.path();
    let mut paths = Vec::with_capacity(files.len());

    for (filename, contents) in files {
        let file_path = root.join(filename);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file_path, contents)?;
        paths.push(file_path);
    }

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: paths,
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;
    Ok((storage.get_nodes()?, storage.get_edges()?))
}

fn assert_import_edges_extracted(edges: &[codestory_contracts::graph::Edge]) {
    assert!(
        edges.iter().any(|edge| edge.kind == EdgeKind::IMPORT),
        "IMPORT edge not found"
    );
}

fn matches_name(actual: &str, wanted: &str) -> bool {
    actual == wanted
        || actual.ends_with(&format!(".{wanted}"))
        || actual.ends_with(&format!("::{wanted}"))
}

fn has_node_kind(nodes: &[codestory_contracts::graph::Node], name: &str, kind: NodeKind) -> bool {
    nodes
        .iter()
        .any(|node| matches_name(&node.serialized_name, name) && node.kind == kind)
}

fn file_path_for_node<'a>(
    nodes_by_id: &std::collections::HashMap<
        codestory_contracts::graph::NodeId,
        &'a codestory_contracts::graph::Node,
    >,
    node: &codestory_contracts::graph::Node,
) -> Option<&'a str> {
    node.file_node_id
        .and_then(|file_id| nodes_by_id.get(&file_id).copied())
        .map(|file| file.serialized_name.as_str())
}

fn node_in_file<'a>(
    nodes: &'a [codestory_contracts::graph::Node],
    nodes_by_id: &std::collections::HashMap<
        codestory_contracts::graph::NodeId,
        &'a codestory_contracts::graph::Node,
    >,
    name: &str,
    kind: NodeKind,
    file_suffix: &str,
) -> Option<&'a codestory_contracts::graph::Node> {
    nodes.iter().find(|node| {
        matches_name(&node.serialized_name, name)
            && node.kind == kind
            && file_path_for_node(nodes_by_id, node)
                .map(|path| path.replace('\\', "/").ends_with(file_suffix))
                .unwrap_or(false)
    })
}

fn edge_importer_path<'a>(
    nodes_by_id: &std::collections::HashMap<
        codestory_contracts::graph::NodeId,
        &'a codestory_contracts::graph::Node,
    >,
    edge: &codestory_contracts::graph::Edge,
) -> Option<&'a str> {
    if let Some(file_id) = edge.file_node_id {
        return nodes_by_id
            .get(&file_id)
            .map(|file| file.serialized_name.as_str());
    }

    nodes_by_id.get(&edge.source).and_then(|source| {
        if source.kind == NodeKind::FILE {
            Some(source.serialized_name.as_str())
        } else {
            file_path_for_node(nodes_by_id, source)
        }
    })
}

fn path_ends_with(path: &str, suffix: &str) -> bool {
    path.replace('\\', "/").ends_with(suffix)
}

fn assert_import_resolved_to(
    nodes: &[codestory_contracts::graph::Node],
    edges: &[codestory_contracts::graph::Edge],
    importer_suffix: &str,
    target_suffix: &str,
    target_name: &str,
) {
    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();

    let resolved = edges.iter().any(|edge| {
        if edge.kind != EdgeKind::IMPORT {
            return false;
        }
        if !edge_importer_path(&nodes_by_id, edge)
            .map(|path| path_ends_with(path, importer_suffix))
            .unwrap_or(false)
        {
            return false;
        }
        if edge.confidence.unwrap_or(0.0) < 0.55 {
            return false;
        }

        let Some(target_id) = edge.resolved_target else {
            return false;
        };
        let Some(target) = nodes_by_id.get(&target_id) else {
            return false;
        };

        matches_name(&target.serialized_name, target_name)
            && file_path_for_node(&nodes_by_id, target)
                .map(|path| path_ends_with(path, target_suffix))
                .unwrap_or(false)
    });

    if !resolved {
        let import_edges = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::IMPORT)
            .map(|edge| {
                let importer = edge_importer_path(&nodes_by_id, edge).unwrap_or("<unknown>");
                let source = nodes_by_id
                    .get(&edge.source)
                    .map(|node| node.serialized_name.as_str())
                    .unwrap_or("<missing>");
                let target = nodes_by_id
                    .get(&edge.target)
                    .map(|node| node.serialized_name.as_str())
                    .unwrap_or("<missing>");
                let resolved = edge
                    .resolved_target
                    .and_then(|target_id| nodes_by_id.get(&target_id).copied())
                    .map(|node| node.serialized_name.as_str())
                    .unwrap_or("<unresolved>");
                format!(
                    "{importer}: {source} -> {target} resolved={resolved} confidence={:?}",
                    edge.confidence
                )
            })
            .collect::<Vec<_>>();
        let target_candidates = nodes
            .iter()
            .filter(|node| matches_name(&node.serialized_name, target_name))
            .map(|node| {
                let file = file_path_for_node(&nodes_by_id, node).unwrap_or("<unknown>");
                format!("{:?} {} in {file}", node.kind, node.serialized_name)
            })
            .collect::<Vec<_>>();

        panic!(
            "expected import from {importer_suffix} to resolve to {target_name} in {target_suffix}. IMPORT edges: {import_edges:?}. Target candidates: {target_candidates:?}"
        );
    }
}

#[test]
fn test_import_edges_are_extracted_across_languages() -> anyhow::Result<()> {
    let cases = [
        (
            "main.ts",
            r#"
import type { Foo } from "./foo";
const value: Foo = { id: 1 };
function main() {}
"#,
        ),
        (
            "Test.java",
            r#"
import java.util.List;
class Test {}
"#,
        ),
        (
            "main.rs",
            r#"
use std::collections::HashMap;
fn main() {}
"#,
        ),
        (
            "main.rb",
            r#"
require_relative "./helper"
"#,
        ),
    ];

    for (filename, code) in cases {
        let edges = index_single_file(filename, code)?;
        assert_import_edges_extracted(&edges);
    }

    Ok(())
}

#[test]
fn test_cross_file_imports_resolve_to_indexed_targets() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[
        (
            "src/foo.ts",
            r#"
export interface Foo {
    id: number;
}
"#,
        ),
        (
            "src/main.ts",
            r#"
import type { Foo } from "./foo";
const value: Foo = { id: 1 };
"#,
        ),
        (
            "src/widget.rs",
            r#"
pub struct Widget;
"#,
        ),
        (
            "src/lib.rs",
            r#"
mod widget;
use crate::widget::Widget;

pub fn make_widget() -> Widget {
    Widget
}
"#,
        ),
    ])?;

    assert_import_resolved_to(&nodes, &edges, "src/main.ts", "src/foo.ts", "Foo");
    assert_import_resolved_to(&nodes, &edges, "src/lib.rs", "src/widget.rs", "Widget");
    Ok(())
}

#[test]
fn test_generic_parser_backed_imports_resolve_to_indexed_targets() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[
        (
            "src/app/shared/SharedHelper.kt",
            r#"
package app.shared

class SharedHelper
"#,
        ),
        (
            "src/app/Main.kt",
            r#"
package app

import app.shared.SharedHelper

fun main() {
    SharedHelper()
}
"#,
        ),
        (
            "Sources/SharedKit.swift",
            r#"
class SharedKit {
    func value() -> Int { return 1 }
}
"#,
        ),
        (
            "Sources/App.swift",
            r#"
import SharedKit

func main() -> Int {
    return 1
}
"#,
        ),
        (
            "lib/helper.dart",
            r#"
String helper() {
  return 'ready';
}
"#,
        ),
        (
            "lib/main.dart",
            r#"
import './helper.dart';

void main() {
  helper();
}
"#,
        ),
        (
            "scripts/logger.sh",
            r#"
logger() {
  printf "%s\n" "$1"
}
"#,
        ),
        (
            "scripts/main.sh",
            r#"
source ./logger.sh

main() {
  logger "ready"
}
"#,
        ),
    ])?;

    assert_import_resolved_to(
        &nodes,
        &edges,
        "src/app/Main.kt",
        "src/app/shared/SharedHelper.kt",
        "SharedHelper",
    );
    assert_import_resolved_to(
        &nodes,
        &edges,
        "Sources/App.swift",
        "Sources/SharedKit.swift",
        "SharedKit",
    );
    assert_import_resolved_to(&nodes, &edges, "lib/main.dart", "lib/helper.dart", "helper");
    assert_import_resolved_to(
        &nodes,
        &edges,
        "scripts/main.sh",
        "scripts/logger.sh",
        "logger",
    );
    Ok(())
}

#[test]
fn test_javascript_require_and_dynamic_import_surface_as_import_edges() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[
        (
            "main.js",
            r#"
const pkg = require("./pkg.js");

async function load() {
    const feature = await import("./feature.js");
    return [pkg, feature];
}
"#,
        ),
        ("pkg.js", "export const value = 1;\n"),
        ("feature.js", "export default 1;\n"),
    ])?;

    assert!(has_node_kind(&nodes, "\"./pkg.js\"", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "\"./feature.js\"", NodeKind::MODULE));

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();

    let import_targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT)
        .filter_map(|edge| node_by_id.get(&edge.effective_target()).copied())
        .map(|node| node.serialized_name.as_str())
        .collect::<Vec<_>>();

    assert!(
        import_targets
            .iter()
            .any(|name| matches_name(name, "\"./pkg.js\"")),
        "expected require(\"./pkg.js\") to surface as IMPORT"
    );
    assert!(
        import_targets
            .iter()
            .any(|name| matches_name(name, "\"./feature.js\"")),
        "expected dynamic import(\"./feature.js\") to surface as IMPORT"
    );

    let generic_runtime_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .map(|node| node.serialized_name.as_str())
        .filter(|name| matches_name(name, "require") || matches_name(name, "import"))
        .collect::<Vec<_>>();

    assert!(
        generic_runtime_calls.is_empty(),
        "expected runtime module loading to avoid generic CALL placeholders, found {generic_runtime_calls:?}"
    );

    Ok(())
}

#[test]
fn test_ruby_require_runtime_imports_avoid_string_call_false_positives() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[
        (
            "main.rb",
            r#"
require_relative "./workflow"
require_relative(
  "./multiline"
)
require "json"

puts "not an import"
log("also not import")
"#,
        ),
        (
            "workflow.rb",
            r#"
class Workflow
end
"#,
        ),
        ("multiline.rb", "class Multiline\nend\n"),
    ])?;

    assert!(has_node_kind(&nodes, "\"./workflow\"", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "\"./multiline\"", NodeKind::MODULE));
    assert!(has_node_kind(&nodes, "\"json\"", NodeKind::MODULE));

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();

    let import_targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .map(|node| node.serialized_name.as_str())
        .collect::<Vec<_>>();

    assert!(
        import_targets
            .iter()
            .any(|name| matches_name(name, "\"./workflow\"")),
        "expected require_relative \"./workflow\" to surface as IMPORT"
    );
    assert!(
        import_targets
            .iter()
            .any(|name| matches_name(name, "\"./multiline\"")),
        "expected multiline require_relative \"./multiline\" to surface as IMPORT"
    );
    assert!(
        import_targets
            .iter()
            .any(|name| matches_name(name, "\"json\"")),
        "expected require \"json\" to surface as IMPORT"
    );
    assert!(
        !import_targets
            .iter()
            .any(|name| matches_name(name, "\"not an import\"")),
        "puts string arguments must not surface as IMPORT"
    );
    assert!(
        !import_targets
            .iter()
            .any(|name| matches_name(name, "\"also not import\"")),
        "ordinary string call arguments must not surface as IMPORT"
    );

    let generic_runtime_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .map(|node| node.serialized_name.as_str())
        .filter(|name| matches_name(name, "require") || matches_name(name, "require_relative"))
        .collect::<Vec<_>>();

    assert!(
        generic_runtime_calls.is_empty(),
        "expected Ruby runtime module loading to avoid generic CALL placeholders, found {generic_runtime_calls:?}"
    );

    Ok(())
}

#[test]
fn test_ruby_receiver_qualified_require_calls_do_not_surface_as_imports() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[(
        "main.rb",
        r#"
loader.require "not_receiver_import"
loader.require_relative("not_receiver_relative_import")
"#,
    )])?;

    assert!(!has_node_kind(
        &nodes,
        "\"not_receiver_import\"",
        NodeKind::MODULE
    ));
    assert!(!has_node_kind(
        &nodes,
        "\"not_receiver_relative_import\"",
        NodeKind::MODULE
    ));

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();

    let import_targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .map(|node| node.serialized_name.as_str())
        .collect::<Vec<_>>();

    assert!(
        !import_targets
            .iter()
            .any(|name| matches_name(name, "\"not_receiver_import\"")),
        "receiver-qualified require calls must not surface as IMPORT"
    );
    assert!(
        !import_targets
            .iter()
            .any(|name| matches_name(name, "\"not_receiver_relative_import\"")),
        "receiver-qualified require_relative calls must not surface as IMPORT"
    );

    Ok(())
}

#[test]
fn test_ruby_same_line_dynamic_require_does_not_surface_as_import() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[
        (
            "main.rb",
            r#"
path = "./dynamic"
require "./workflow"; require(path)
"#,
        ),
        (
            "workflow.rb",
            r#"
class Workflow
end
"#,
        ),
    ])?;

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();

    let import_targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT)
        .filter_map(|edge| node_by_id.get(&edge.target).copied())
        .map(|node| node.serialized_name.as_str())
        .collect::<Vec<_>>();

    assert!(
        import_targets
            .iter()
            .any(|name| matches_name(name, "\"./workflow\"")),
        "expected static require \"./workflow\" to surface as IMPORT"
    );
    assert!(!has_node_kind(&nodes, "\"./dynamic\"", NodeKind::MODULE));
    assert!(
        !import_targets
            .iter()
            .any(|name| matches_name(name, "\"./dynamic\"")),
        "same-line dynamic require(path) must not surface the variable value as IMPORT"
    );

    Ok(())
}

#[test]
fn test_javascript_static_import_aliases_resolve_and_feed_constructor_calls() -> anyhow::Result<()>
{
    let (nodes, edges) = index_workspace(&[
        (
            "app.js",
            r#"
import Client from "./client.js";

function makeClient() {
    const client = new Client();
    return client;
}
"#,
        ),
        (
            "client.js",
            r#"
class Client {
    request() {}
}

export default Client;
"#,
        ),
    ])?;

    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();
    let make_client = node_in_file(
        &nodes,
        &nodes_by_id,
        "makeClient",
        NodeKind::FUNCTION,
        "app.js",
    )
    .ok_or_else(|| anyhow::anyhow!("makeClient node not found"))?;
    let import_alias = node_in_file(&nodes, &nodes_by_id, "Client", NodeKind::UNKNOWN, "app.js")
        .ok_or_else(|| anyhow::anyhow!("Client import alias node not found"))?;
    let imported_class = node_in_file(&nodes, &nodes_by_id, "Client", NodeKind::CLASS, "client.js")
        .ok_or_else(|| anyhow::anyhow!("Client class node not found"))?;

    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.source == make_client.id
                && edge.effective_target() == import_alias.id
        }),
        "new Client() should create a CALL edge from makeClient to the imported alias"
    );
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::IMPORT
                && edge.source == import_alias.id
                && edge.effective_target() == imported_class.id
                && edge.certainty.is_some_and(|certainty| {
                    certainty == codestory_contracts::graph::ResolutionCertainty::Certain
                })
        }),
        "Client default import should resolve by relative path to the class in client.js"
    );

    Ok(())
}

#[test]
fn test_javascript_bound_function_receiver_calls_imported_default() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[
        (
            "client.js",
            r#"
import dispatchRequest from "./dispatchRequest.js";

class Client {
    _request(config) {
        return dispatchRequest.call(this, config);
    }
}

export default Client;
"#,
        ),
        (
            "dispatchRequest.js",
            r#"
export default function dispatchRequest(config) {
    return config;
}
"#,
        ),
    ])?;

    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();
    let request_method = node_in_file(
        &nodes,
        &nodes_by_id,
        "_request",
        NodeKind::METHOD,
        "client.js",
    )
    .ok_or_else(|| anyhow::anyhow!("Client._request method not found"))?;
    let import_alias = node_in_file(
        &nodes,
        &nodes_by_id,
        "dispatchRequest",
        NodeKind::UNKNOWN,
        "client.js",
    )
    .ok_or_else(|| anyhow::anyhow!("dispatchRequest import alias not found"))?;
    let imported_function = node_in_file(
        &nodes,
        &nodes_by_id,
        "dispatchRequest",
        NodeKind::FUNCTION,
        "dispatchRequest.js",
    )
    .ok_or_else(|| anyhow::anyhow!("dispatchRequest function not found"))?;

    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.source == request_method.id
                && edge.target == import_alias.id
                && edge.effective_target() == import_alias.id
                && edge.certainty.is_some_and(|certainty| {
                    certainty == codestory_contracts::graph::ResolutionCertainty::Certain
                })
        }),
        "dispatchRequest.call(...) should create a certain CALL edge to the imported dispatchRequest alias"
    );
    assert!(
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::IMPORT
                && edge.source == import_alias.id
                && edge.effective_target() == imported_function.id
        }),
        "dispatchRequest default import should resolve by relative path to dispatchRequest.js"
    );

    Ok(())
}
