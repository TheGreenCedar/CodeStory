//! B59-shaped OpenAPI 3.2 golden YAML collector coverage.
//!
//! Fixture is Prometheus `web/api/v1/testdata/openapi_3.2_golden.yaml`
//! (sha256 93590a228d76de40245623308699e98b8f2b84ddd119cfddc583560f55903c93).
//! Do not reopen the Prometheus breadth bank; this is an indexer-local RED.

use anyhow::Result;
use codestory_contracts::events::EventBus;
use codestory_contracts::graph::FileCoverageReason;
use codestory_indexer::{WorkspaceIndexer, looks_like_openapi_schema};
use codestory_store::Store as Storage;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const GOLDEN_SHA256: &str = "93590a228d76de40245623308699e98b8f2b84ddd119cfddc583560f55903c93";

fn golden_bytes() -> &'static [u8] {
    include_bytes!("fixtures/openapi_3.2_golden.yaml")
}

#[test]
fn openapi_3_2_golden_fixture_identity_and_markers() {
    let bytes = golden_bytes();
    assert_eq!(bytes.len(), 245_217);
    let digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    };
    assert_eq!(digest, GOLDEN_SHA256);
    let source = std::str::from_utf8(bytes).expect("utf-8 OpenAPI golden");
    assert_eq!(source.lines().next(), Some("openapi: 3.2.0"));
    assert!(looks_like_openapi_schema(source));
    assert!(!source.as_bytes().contains(&0));
}

#[test]
fn openapi_3_2_golden_yaml_produces_endpoint_projection_without_collector_failure() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let relative = PathBuf::from("web/api/v1/testdata/openapi_3.2_golden.yaml");
    let full = dir.path().join(&relative);
    fs::create_dir_all(full.parent().expect("parent"))?;
    fs::write(&full, golden_bytes())?;

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_index_policy(codestory_contracts::workspace::SourceIndexPolicy::default());
    let event_bus = EventBus::new();
    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![relative.clone()],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;

    let files = storage.get_files()?;
    let file = files
        .iter()
        .find(|file| file.path.ends_with(&relative))
        .unwrap_or_else(|| panic!("expected indexed file for {}", relative.display()));

    assert_eq!(
        file.language, "openapi",
        "dedicated OpenAPI routing must win for the golden YAML"
    );
    assert!(file.complete, "schema file must be complete");
    assert!(file.indexed);

    let file_id = file.id;
    assert!(
        storage.has_file_owned_openapi_endpoint_projection(file_id)?,
        "OpenAPI 3.2 golden YAML must leave a file-owned endpoint projection (B59 CollectorFailure shape)"
    );

    let errors = storage.get_errors(None)?;
    let collector_failures = errors
        .iter()
        .filter(|error| {
            error.file_id.map(|id| id.0) == Some(file_id)
                && error.coverage_reason == Some(FileCoverageReason::CollectorFailure)
        })
        .collect::<Vec<_>>();
    assert!(
        collector_failures.is_empty(),
        "OpenAPI 3.2 golden YAML must not land CollectorFailure; errors={collector_failures:?}"
    );

    let endpoints = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| {
            node.canonical_id
                .as_deref()
                .is_some_and(|value| value.starts_with("openapi:endpoint:"))
        })
        .count();
    assert!(
        endpoints >= 1,
        "expected at least one openapi:endpoint projection; got {endpoints}"
    );
    Ok(())
}

#[test]
fn openapi_3_2_golden_exceeds_default_structural_unit_cap_if_routed_as_yaml() {
    use codestory_indexer::structural::{StructuralCollectionError, index_structural_source};
    use std::path::Path;
    let source = std::str::from_utf8(golden_bytes()).expect("utf-8");
    let result = index_structural_source(
        Path::new("web/api/v1/testdata/openapi_3.2_golden.yaml"),
        source,
    );
    match result {
        Err(StructuralCollectionError::UnitLimit {
            observed_unit_count,
            structural_unit_cap,
        }) => {
            assert!(
                observed_unit_count > structural_unit_cap,
                "observed={observed_unit_count} cap={structural_unit_cap}"
            );
        }
        Ok(_) => panic!("expected structural unit-limit for golden when not OpenAPI-routed"),
        Err(other) => panic!("unexpected structural error: {other}"),
    }
}
