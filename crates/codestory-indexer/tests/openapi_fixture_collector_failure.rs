//! U03/U06-shaped OpenAPI/Swagger JSON fixtures that failed unseen-216 as
//! `source_collector_failure` at `source_verification`.
//!
//! Root cause: dedicated OpenAPI endpoint node ids were hashed from method+path
//! only. Sibling fixtures that share routes (for example several localstack
//! `GET /pets` schemas) collided under `INSERT OR REPLACE`, rebinding
//! `file_node_id` so full-refresh coverage saw verified openapi files without
//! file-owned endpoint projection evidence.

use anyhow::Result;
use codestory_contracts::events::EventBus;
use codestory_contracts::graph::FileCoverageReason;
use codestory_indexer::{WorkspaceIndexer, looks_like_openapi_schema};
use codestory_store::Store as Storage;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> &'static [u8] {
    match name {
        "pets.json" => include_bytes!("fixtures/openapi_u03_u06/pets.json"),
        "petstore-swagger.json" => include_bytes!("fixtures/openapi_u03_u06/petstore-swagger.json"),
        "openapi.spec.tf.json" => include_bytes!("fixtures/openapi_u03_u06/openapi.spec.tf.json"),
        "openapi.spec.global-auth.json" => {
            include_bytes!("fixtures/openapi_u03_u06/openapi.spec.global-auth.json")
        }
        "openapi.spec.circular-ref.json" => {
            include_bytes!("fixtures/openapi_u03_u06/openapi.spec.circular-ref.json")
        }
        _ => panic!("unknown fixture {name}"),
    }
}

fn colliding_u03_cases() -> [(&'static str, &'static str); 5] {
    [
        ("tests/aws/files/pets.json", "pets.json"),
        (
            "tests/aws/files/petstore-swagger.json",
            "petstore-swagger.json",
        ),
        (
            "tests/aws/files/openapi.spec.tf.json",
            "openapi.spec.tf.json",
        ),
        (
            "tests/aws/files/openapi.spec.global-auth.json",
            "openapi.spec.global-auth.json",
        ),
        (
            "tests/aws/files/openapi.spec.circular-ref.json",
            "openapi.spec.circular-ref.json",
        ),
    ]
}

fn index_files(files: &[(&str, &[u8])]) -> Result<(tempfile::TempDir, Storage)> {
    let dir = tempfile::tempdir()?;
    let mut files_to_index = Vec::new();
    for &(relative, bytes) in files {
        let relative = PathBuf::from(relative);
        let full = dir.path().join(&relative);
        fs::create_dir_all(full.parent().expect("parent"))?;
        fs::write(&full, bytes)?;
        files_to_index.push(relative);
    }

    let mut storage = Storage::new_in_memory()?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_index_policy(codestory_contracts::workspace::SourceIndexPolicy::default());
    indexer.run_incremental(
        &mut storage,
        &codestory_workspace::RefreshInfo {
            mode: codestory_workspace::BuildMode::FullRefresh,
            files_to_index,
            files_to_remove: vec![],
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    Ok((dir, storage))
}

#[test]
fn u03_localstack_openapi_fixtures_look_like_schemas() {
    for (_, name) in colliding_u03_cases() {
        let source = std::str::from_utf8(fixture(name)).unwrap();
        assert!(
            looks_like_openapi_schema(source),
            "{name} must match OpenAPI/Swagger markers"
        );
    }
}

#[test]
fn u03_localstack_openapi_fixtures_project_endpoints_without_collector_failure() -> Result<()> {
    for (relative, name) in colliding_u03_cases() {
        let (_dir, storage) = index_files(&[(relative, fixture(name))])?;
        let file = storage
            .get_files()?
            .into_iter()
            .find(|file| file.path.ends_with(relative))
            .unwrap_or_else(|| panic!("expected indexed file for {relative}"));
        assert_eq!(file.language, "openapi", "{relative} language");
        assert!(file.complete, "{relative} complete");
        assert!(
            storage.has_file_owned_openapi_endpoint_projection(file.id)?,
            "{relative} must have file-owned openapi endpoint projection"
        );
        let collector_failures = storage
            .get_errors(None)?
            .into_iter()
            .filter(|error| {
                error.file_id.map(|id| id.0) == Some(file.id)
                    && error.coverage_reason == Some(FileCoverageReason::CollectorFailure)
            })
            .collect::<Vec<_>>();
        assert!(
            collector_failures.is_empty(),
            "{relative} collector_failures={collector_failures:?}"
        );
    }
    Ok(())
}

#[test]
fn u03_colliding_route_siblings_keep_file_owned_openapi_projections() -> Result<()> {
    let files = colliding_u03_cases()
        .into_iter()
        .map(|(relative, name)| (relative, fixture(name)))
        .collect::<Vec<_>>();
    let (_dir, storage) = index_files(&files)?;
    let indexed = storage.get_files()?;
    assert_eq!(indexed.len(), files.len());

    let mut failures = Vec::new();
    for file in &indexed {
        let relative = file.path.to_string_lossy();
        if file.language != "openapi" {
            failures.push(format!(
                "{relative}: expected language openapi, got {}",
                file.language
            ));
            continue;
        }
        if !storage.has_file_owned_openapi_endpoint_projection(file.id)? {
            failures.push(format!(
                "{relative}: openapi without file-owned endpoint projection"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "shared-route OpenAPI siblings must not collide; failures={failures:#?}"
    );
    Ok(())
}

#[test]
fn u06_kratos_openapi_and_swagger_project_when_present() -> Result<()> {
    let pin = std::env::var_os("CODESTORY_U06_KRATOS_PIN").map(PathBuf::from);
    let Some(pin) = pin.filter(|p| p.is_dir()) else {
        eprintln!(
            "skipping U06 pin fixtures; set CODESTORY_U06_KRATOS_PIN to exercise both schemas together"
        );
        return Ok(());
    };
    let openapi = fs::read(pin.join(".schema/openapi.json"))?;
    let swagger = fs::read(pin.join("spec/swagger.json"))?;
    let (_dir, storage) = index_files(&[
        (".schema/openapi.json", openapi.as_slice()),
        ("spec/swagger.json", swagger.as_slice()),
    ])?;
    for relative in [".schema/openapi.json", "spec/swagger.json"] {
        let file = storage
            .get_files()?
            .into_iter()
            .find(|file| file.path.ends_with(relative))
            .unwrap_or_else(|| panic!("expected indexed file for {relative}"));
        assert_eq!(file.language, "openapi", "{relative}");
        assert!(
            storage.has_file_owned_openapi_endpoint_projection(file.id)?,
            "{relative} missing projection"
        );
    }
    Ok(())
}
