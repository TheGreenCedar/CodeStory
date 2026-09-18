//! U09/U12-shaped Rust `async ||` let-bindings that failed unseen-216 as
//! `source_collector_failure` at `source_verification`.
//!
//! Root cause: `rust.graph.scm` owned closures as FUNCTION via
//! `(closure_expression)`, but the local VARIABLE stanza's text predicate only
//! excluded `|…|` and `move |…|`. `async ||` / `async move ||` matched both
//! stanzas and aborted graph execution with `DuplicateVariable`.

use anyhow::Result;
use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{FileCoverageReason, NodeKind};
use codestory_indexer::{WorkspaceIndexer, get_language_for_ext, index_file};
use codestory_store::Store as Storage;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

const ASYNC_CLOSURE_FIXTURE: &str = r#"
pub fn sample() {
    let plain = 1;
    let sync_closure = || plain;
    let move_closure = move || plain;
    let async_closure = async || {
        let inner = 2;
        inner
    };
    let async_move_closure = async move || {
        let inner = 3;
        inner
    };
    let _ = (sync_closure, move_closure, async_closure, async_move_closure);
}
"#;

#[test]
fn rust_async_closure_let_bindings_index_without_duplicate_variable() -> Result<()> {
    let config = get_language_for_ext("rs").expect("rust language");
    let result = index_file(
        Path::new("async_closures.rs"),
        ASYNC_CLOSURE_FIXTURE,
        &config,
        None,
        None,
    )?;
    let file = result.files.first().expect("file row");
    assert!(file.indexed);
    assert!(file.complete, "parser-complete async-closure fixture");

    let function_names: Vec<_> = result
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::FUNCTION)
        .map(|node| node.serialized_name.as_str())
        .collect();
    for name in [
        "sync_closure",
        "move_closure",
        "async_closure",
        "async_move_closure",
    ] {
        assert!(
            function_names.iter().any(|candidate| candidate.contains(name)),
            "expected FUNCTION node for {name}, got {function_names:?}"
        );
    }

    let variable_names: Vec<_> = result
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::VARIABLE)
        .map(|node| node.serialized_name.as_str())
        .collect();
    assert!(
        variable_names.iter().any(|candidate| candidate.contains("plain")),
        "expected VARIABLE node for plain let, got {variable_names:?}"
    );
    for name in [
        "sync_closure",
        "move_closure",
        "async_closure",
        "async_move_closure",
    ] {
        assert!(
            variable_names
                .iter()
                .all(|candidate| !candidate.ends_with(name) && !candidate.contains(&format!("::{name}"))),
            "closure binding {name} must not also be VARIABLE, got {variable_names:?}"
        );
    }
    Ok(())
}

#[test]
fn rust_async_closure_workspace_refresh_has_no_collector_failure() -> Result<()> {
    let dir = tempdir()?;
    let root = dir.path();
    let path = root.join("src/lib.rs");
    fs::create_dir_all(path.parent().expect("parent"))?;
    fs::write(&path, ASYNC_CLOSURE_FIXTURE)?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path],
        files_to_remove: vec![],
        existing_file_ids: std::collections::HashMap::new(),
    };
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let errors = storage.get_errors(None)?;
    let collector_failures: Vec<_> = errors
        .iter()
        .filter(|error| error.coverage_reason == Some(FileCoverageReason::CollectorFailure))
        .collect();
    assert!(
        collector_failures.is_empty(),
        "async || let-bindings must not record collector_failure: {collector_failures:?}"
    );
    let files = storage.get_files()?;
    assert_eq!(files.len(), 1);
    assert!(files[0].indexed);
    assert!(files[0].complete);
    Ok(())
}
