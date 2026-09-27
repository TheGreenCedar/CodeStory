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
// File-owned endpoints in the retained 3.2 schema. The line is the method
// declaration, not the path declaration; this pins the emitted source span.
const EXPECTED_ENDPOINTS: [(&str, u32); 50] = [
    ("GET /query", 13),
    ("POST /query", 146),
    ("GET /query_range", 214),
    ("POST /query_range", 373),
    ("GET /query_exemplars", 429),
    ("POST /query_exemplars", 518),
    ("GET /format_query", 577),
    ("POST /format_query", 619),
    ("GET /parse_query", 667),
    ("POST /parse_query", 710),
    ("GET /labels", 759),
    ("POST /labels", 875),
    ("GET /label/{name}/values", 951),
    ("GET /search/metric_names", 1060),
    ("POST /search/metric_names", 1257),
    ("GET /search/label_names", 1306),
    ("POST /search/label_names", 1493),
    ("GET /search/label_values", 1543),
    ("POST /search/label_values", 1740),
    ("GET /series", 1791),
    ("POST /series", 1903),
    ("GET /metadata", 1974),
    ("GET /scrape_pools", 2051),
    ("GET /scrape_pools/config", 2092),
    ("GET /targets", 2141),
    ("GET /targets/metadata", 2225),
    ("GET /targets/relabel_steps", 2296),
    ("GET /rules", 2362),
    ("GET /alerts", 2510),
    ("GET /alertmanagers", 2552),
    ("GET /status/config", 2587),
    ("GET /status/runtimeinfo", 2634),
    ("GET /status/buildinfo", 2679),
    ("GET /status/flags", 2717),
    ("GET /status/tsdb", 2763),
    ("GET /status/tsdb/blocks", 2833),
    ("GET /status/walreplay", 2880),
    ("GET /status/self_metrics", 2915),
    ("PUT /admin/tsdb/delete_series", 2986),
    ("POST /admin/tsdb/delete_series", 3069),
    ("PUT /admin/tsdb/clean_tombstones", 3153),
    ("POST /admin/tsdb/clean_tombstones", 3184),
    ("PUT /admin/tsdb/snapshot", 3216),
    ("POST /admin/tsdb/snapshot", 3260),
    ("POST /read", 3305),
    ("POST /write", 3321),
    ("POST /otlp/v1/metrics", 3337),
    ("GET /notifications", 3353),
    ("GET /notifications/live", 3388),
    ("GET /features", 3426),
];

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

    let mut endpoints = storage
        .get_nodes()?
        .into_iter()
        .filter_map(|node| {
            node.canonical_id
                .as_deref()
                .and_then(|value| value.strip_prefix("openapi:endpoint:"))
                .map(|label| {
                    (
                        label.to_owned(),
                        node.file_node_id.map(|owner| owner.0),
                        node.start_line,
                    )
                })
        })
        .collect::<Vec<_>>();
    endpoints.sort();
    let mut expected = EXPECTED_ENDPOINTS
        .into_iter()
        .map(|(label, line)| (label.to_owned(), Some(file_id), Some(line)))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(
        endpoints, expected,
        "OpenAPI 3.2 golden must project every method/path with file ownership and source line"
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
