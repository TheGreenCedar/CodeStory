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

fn assert_imports_resolved(edges: &[codestory_core::Edge]) {
    let imports: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::IMPORT)
        .collect();
    assert!(!imports.is_empty(), "IMPORT edge not found");
    for edge in imports {
        assert!(
            edge.resolved_target.is_some(),
            "IMPORT edge missing resolved target"
        );
        let confidence = edge.confidence.unwrap_or(0.0);
        assert!(
            confidence >= 0.9,
            "IMPORT edge confidence too low: {}",
            confidence
        );
    }
}

#[test]
fn test_import_resolution_javascript() -> anyhow::Result<()> {
    let code = r#"
import foo from "foo";
function main() {}
"#;
    let edges = index_single_file("main.js", code)?;
    assert_imports_resolved(&edges);
    Ok(())
}

#[test]
fn test_import_resolution_typescript() -> anyhow::Result<()> {
    let code = r#"
import { foo } from "foo";
function main() {}
"#;
    let edges = index_single_file("main.ts", code)?;
    assert_imports_resolved(&edges);
    Ok(())
}

#[test]
fn test_import_resolution_java() -> anyhow::Result<()> {
    let code = r#"
import java.util.List;
class Test {}
"#;
    let edges = index_single_file("Test.java", code)?;
    assert_imports_resolved(&edges);
    Ok(())
}

#[test]
fn test_import_resolution_rust() -> anyhow::Result<()> {
    let code = r#"
use std::collections::HashMap;
fn main() {}
"#;
    let edges = index_single_file("main.rs", code)?;
    assert_imports_resolved(&edges);
    Ok(())
}
