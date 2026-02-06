use codestory_core::EdgeKind;
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_common_method_calls_do_not_resolve_globally() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let root = dir.path();

    // Create a file that defines a project method named `push`, plus calls to common stdlib-ish
    // methods (`Vec::push`, `Vec::sort`, `Vec::dedup`). We should not resolve these calls to
    // unrelated project methods by name alone.
    let f1 = root.join("main.rs");
    fs::write(
        &f1,
        r#"
struct NavigationHistory;
impl NavigationHistory {
    fn push(&mut self, _x: i32) {}
}

mod tests {
    pub fn test_deduplication() {}
}

fn main() {
    let mut v = vec![1, 1, 2];
    v.push(3);
    v.sort();
    v.dedup();
}
"#,
    )?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();

    let refresh_info = codestory_project::RefreshInfo {
        files_to_index: vec![f1.clone()],
        files_to_remove: vec![],
    };
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;

    let node_name = |id| {
        nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.serialized_name.as_str())
    };

    let mut found = 0;
    for edge in edges.iter().filter(|e| e.kind == EdgeKind::CALL) {
        let Some(name) = node_name(edge.target) else {
            continue;
        };
        if matches!(name, "push" | "sort" | "dedup") {
            found += 1;
            assert!(
                edge.resolved_target.is_none(),
                "CALL edge to `{name}` should remain unresolved (got {:?})",
                edge.resolved_target
            );
        }
    }

    assert!(found >= 3, "expected CALL edges for push/sort/dedup");
    Ok(())
}
