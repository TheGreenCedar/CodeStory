use super::*;
use crate::Runtime;
use std::fs;

fn legacy_activation_fixture() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let project = tempfile::tempdir().expect("project");
    let seed_cache = tempfile::tempdir().expect("seed cache");
    let cache = tempfile::tempdir().expect("legacy cache");
    fs::write(
        project.path().join("lib.rs"),
        "pub fn legacy_value() -> i32 { 28 }\n",
    )
    .expect("write source");
    let seed_path = seed_cache.path().join("codestory.db");
    let runtime = Runtime::new();
    runtime
        .project_service()
        .open_project_summary_with_storage_path(project.path().to_path_buf(), seed_path.clone())
        .expect("open seed");
    runtime
        .index_service()
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish seed");
    let storage_path = cache.path().join("codestory.db");
    fs::copy(
        codestory_store::resolve_core_database_path(&seed_path).unwrap(),
        &storage_path,
    )
    .expect("copy legacy flat database");
    let mut permissions = fs::metadata(&storage_path).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(&storage_path, permissions).unwrap();
    let connection = rusqlite::Connection::open(&storage_path).unwrap();
    connection.execute_batch(&format!(
        "PRAGMA user_version = {}; UPDATE index_artifact_cache SET cache_key = 'legacy-' || cache_key,
         artifact_blob = CAST(json_remove(CAST(artifact_blob AS TEXT), '$.resolution_file',
         '$.resolution_input_schema_version', '$.call_resolution_inputs') AS BLOB);
         DELETE FROM proof_resolution_publication; PRAGMA wal_checkpoint(TRUNCATE);",
        codestory_store::CURRENT_SCHEMA_VERSION - 1,
    )).expect("simulate prior schema and parser artifacts");
    drop(connection);
    (project, cache, storage_path)
}

#[test]
fn activation_rebuilds_legacy_cache_before_opening_it() {
    let (project, _cache, storage_path) = legacy_activation_fixture();
    let before = fs::read(&storage_path).unwrap();
    let runtime = Runtime::new();
    runtime
        .activation_service()
        .activate_core_only(
            project.path(),
            &storage_path,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("managed activation rebuilds legacy parser inputs");
    assert_eq!(
        fs::read(&storage_path).unwrap(),
        before,
        "legacy rollback bytes remain unchanged"
    );
    let store = Store::open_read_only(&storage_path).unwrap();
    let publication = store.get_complete_index_publication().unwrap().unwrap();
    assert_eq!(
        publication.mode,
        codestory_store::IndexPublicationMode::Full
    );
    store
        .validate_proof_resolution_publication(&publication)
        .unwrap();
    store
        .validate_structural_text_unit_publication(&publication)
        .unwrap();
    drop(store);
    Runtime::new()
        .activation_service()
        .activate_core_only(
            project.path(),
            &storage_path,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("healthy new generation remains reusable");
    assert_eq!(
        Store::database_index_publication(&storage_path).unwrap(),
        Some(publication)
    );
}

#[test]
fn activation_failed_legacy_rebuild_preserves_original_database() {
    for action in [
        crate::PublicationTestAction::Fail,
        crate::PublicationTestAction::Cancel,
    ] {
        let (project, _cache, storage_path) = legacy_activation_fixture();
        let before = fs::read(&storage_path).unwrap();
        let service = Runtime::new().activation_service();
        let operation = ActivationOperation {
            service: service.clone(),
            operation_id: "legacy-rebuild-fault".to_string(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        crate::arm_publication_test_fault(crate::PublicationTestBoundary::SearchBuild, action);
        service
            .activate_once(
                &operation,
                project.path().to_path_buf(),
                storage_path.clone(),
                ActivationGoal::CoreOnly,
            )
            .expect_err("prepublication failure must reject replacement");
        assert!(
            fs::read(&storage_path).unwrap() == before,
            "failed upgrade preserves legacy bytes at {action:?}"
        );
        assert_eq!(
            Store::database_schema_version_observational(&storage_path).unwrap(),
            codestory_store::CURRENT_SCHEMA_VERSION - 1
        );
        assert!(
            !storage_path
                .parent()
                .unwrap()
                .join("core/publication.json")
                .exists()
        );
        assert!(
            crate::PUBLICATION_TEST_FAULT.with(|fault| fault.borrow().is_none()),
            "activation reached the injected publication boundary"
        );
    }
}
