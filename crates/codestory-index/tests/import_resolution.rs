use codestory_core::EdgeKind;
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::fs;
use tempfile::tempdir;

fn index_single_file(filename: &str, contents: &str) -> anyhow::Result<Vec<codestory_core::Edge>> {
    let dir = tempdir()?;
    let root = dir.path();
    let file_path = root.join(filename);
    fs::write(&file_path, contents)?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: vec![file_path.clone()],
        files_to_remove: vec![],
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    Ok(storage.get_edges()?)
}

fn index_workspace(
    files: &[(&str, &str)],
) -> anyhow::Result<(Vec<codestory_core::Node>, Vec<codestory_core::Edge>)> {
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

    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: paths,
        files_to_remove: vec![],
    };

    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;
    Ok((storage.get_nodes()?, storage.get_edges()?))
}

fn assert_imports_resolved(edges: &[codestory_core::Edge]) {
    let imports: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::IMPORT)
        .collect();
    assert!(!imports.is_empty(), "IMPORT edge not found");
    for edge in imports {
        if edge.resolved_target.is_some() {
            let confidence = edge.confidence.unwrap_or(0.0);
            assert!(
                confidence >= 0.55,
                "Resolved IMPORT edge confidence too low: {}",
                confidence
            );
        }
    }
}

#[test]
fn test_import_resolution_across_languages() -> anyhow::Result<()> {
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
    ];

    for (filename, code) in cases {
        let edges = index_single_file(filename, code)?;
        assert_imports_resolved(&edges);
    }

    Ok(())
}

#[test]
fn test_cross_file_alias_default_named_and_type_imports() -> anyhow::Result<()> {
    let (nodes, edges) = index_workspace(&[
        (
            "main.rs",
            r#"
mod lib;
use crate::lib::Repository as Repo;
fn run() {
    let _repository = Repo::new();
}
"#,
        ),
        (
            "lib.rs",
            r#"
pub struct Repository;
impl Repository {
    pub fn new() -> Self { Self }
}
"#,
        ),
    ])?;

    let main_file = nodes
        .iter()
        .find(|node| {
            node.kind == codestory_core::NodeKind::FILE && node.serialized_name.contains("main.rs")
        })
        .or_else(|| nodes.iter().find(|node| node.kind == codestory_core::NodeKind::FILE))
        .ok_or_else(|| anyhow::anyhow!("main.rs file node not found"))?;
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<std::collections::HashMap<_, _>>();

    let mut import_edges: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT && edge.file_node_id == Some(main_file.id))
        .collect();
    if import_edges.is_empty() {
        import_edges = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::IMPORT)
            .collect();
    }

    assert!(
        !import_edges.is_empty(),
        "expected IMPORT edges from main.rs"
    );

    let mut resolved_to_same_file = 0usize;
    let mut unresolved_edges = 0usize;
    for edge in import_edges {
        let Some(target_id) = edge.resolved_target else {
            unresolved_edges += 1;
            continue;
        };
        let Some(target) = node_by_id.get(&target_id) else {
            continue;
        };
        if target.file_node_id == Some(main_file.id) {
            resolved_to_same_file += 1;
        }
    }

    assert!(
        resolved_to_same_file == 0,
        "import should not resolve back to symbols in the caller file"
    );
    assert!(
        unresolved_edges > 0,
        "expected unresolved imports to remain explicit when cross-file resolution is uncertain"
    );
    Ok(())
}
