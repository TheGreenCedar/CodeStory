use super::*;

#[test]
fn full_refresh_publishes_verified_generic_tagged_template_parser_partial() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    let source_path = workspace.path().join("job-store.ts");
    let storage_path = cache.path().join("codestory.db");
    fs::write(
        &source_path,
        "declare function sql<T>(parts: TemplateStringsArray): T;\nexport const row = sql<unknown>`SELECT 1`;\n",
    )
    .expect("write generic tagged-template fixture");

    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("stable parser-partial source must remain publishable");

    let storage = Storage::open(&storage_path).expect("open published storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("read publication")
        .expect("parser partial must not suppress core publication");
    storage
        .validate_dense_anchor_publication(&publication)
        .expect("parser partial must retain a coherent dense-anchor publication");
    let file = storage
        .get_file_by_path(&source_path)
        .expect("read source row")
        .expect("indexed TypeScript source");
    assert!(
        !file.complete,
        "fixture must exercise parser-partial coverage"
    );
    assert!(file.indexed);
    let inventory = storage.files().inventory().expect("file inventory");
    let stored = inventory
        .iter()
        .find(|entry| entry.id == file.id)
        .expect("stored source inventory");
    assert!(!stored.retry_required);
    assert!(stored.content_hash.is_some());
    assert!(
        storage
            .get_errors(None)
            .expect("file errors")
            .iter()
            .all(|error| error.file_id.map(|id| id.0) != Some(file.id)),
        "parser partial must not be persisted as a file-level source failure"
    );
    drop(storage);

    controller
        .open_project_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("strict reader must admit the completed search generation");
    let diagnostics = controller
        .indexed_files(IndexedFilesRequest {
            path_contains: Some("job-store.ts".to_string()),
            language: None,
            role: None,
            limit: Some(10),
        })
        .expect("file diagnostics");
    assert!(diagnostics.coverage_gaps.iter().any(|entry| {
        entry.path == "job-store.ts"
            && entry.reason == FileCoverageReason::ParserPartial
            && !entry.retryable
            && entry.verified_source
            && entry.projection_available
    }));
}

#[test]
fn full_refresh_publishes_verified_c_multi_alias_typedef() {
    let _env = hybrid_test_env();
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    let source_path = workspace.path().join("aliases.h");
    let storage_path = cache.path().join("codestory.db");
    fs::write(
        &source_path,
        "typedef original_type first_alias_t, second_alias_t;\n",
    )
    .expect("write generic C multi-alias fixture");

    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("deterministic C graph collection must publish on the first full refresh");

    let storage = Storage::open(&storage_path).expect("open published storage");
    let publication = storage
        .get_complete_index_publication()
        .expect("read publication")
        .expect("complete C graph collection must publish a core");
    storage
        .validate_dense_anchor_publication(&publication)
        .expect("C graph collection must retain a coherent dense-anchor publication");
    let file = storage
        .get_file_by_path(&source_path)
        .expect("read source row")
        .expect("indexed C source");
    assert!(file.indexed);
    assert!(file.complete);
    assert_eq!(file.language, "c");
    let inventory = storage.files().inventory().expect("file inventory");
    let stored = inventory
        .iter()
        .find(|entry| entry.id == file.id)
        .expect("stored source inventory");
    assert!(stored.content_hash.is_some());
    assert!(!stored.retry_required);
    assert!(
        storage.get_errors(None).expect("file errors").is_empty(),
        "persisted coverage must be clean before publication"
    );
}

#[test]
fn activation_search_repair_rejects_publication_drift_and_discards_the_candidate() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let expected = test_index_publication(1, "13131313-1313-4313-8313-131313131313");
    storage
        .put_index_publication(&expected)
        .expect("publish expected core generation");
    drop(storage);

    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
        .expect("bind activation writer");
    let replacement = test_index_publication(2, "14141414-1414-4414-8414-141414141414");
    let replacement_for_hook = replacement.clone();
    arm_activation_search_before_revalidate_hook(move |path| {
        Store::open(path)
            .expect("open drifting publication")
            .put_index_publication(&replacement_for_hook)
            .expect("publish drifted core identity");
    });

    let error = controller
        .prepare_search_state_for_activation(&CancellationToken::new())
        .expect_err("drifted core publication must reject prepared search state");

    assert_eq!(error.code, "publication_changed");
    assert_eq!(
        Store::database_index_publication(&storage_path).expect("live publication"),
        Some(replacement)
    );
    let rejected_path = search_index_path_for_publication(&storage_path, Some(&expected))
        .expect("rejected generation path");
    assert!(
        !rejected_path.exists(),
        "drifted search generation must not remain published"
    );
}

#[test]
fn activation_search_repair_reports_generation_lock_contention_as_cache_busy() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let publication = test_index_publication(1, "16161616-1616-4616-8616-161616161616");
    storage
        .put_index_publication(&publication)
        .expect("publish core generation");
    let held_reader = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("build completed search generation");
    let search_path = search_index_path_for_publication(&storage_path, Some(&publication))
        .expect("search generation path");
    fs::remove_file(search_generation_completion_path(&search_path))
        .expect("remove completion marker while reader is pinned");
    drop(storage);

    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("bind activation writer");
    let error = controller
        .prepare_search_state_for_activation(&CancellationToken::new())
        .expect_err("pinned generation must make repair retryable");

    assert_eq!(error.code, "cache_busy");
    drop(held_reader);
}

#[test]
fn cancelled_activation_search_repair_exposes_no_completed_generation() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let publication = test_index_publication(1, "15151515-1515-4515-8515-151515151515");
    storage
        .put_index_publication(&publication)
        .expect("publish core generation");
    drop(storage);

    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    controller
        .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
        .expect("bind activation writer");
    let token = CancellationToken::new();
    arm_publication_test_fault(
        PublicationTestBoundary::SearchValidation,
        PublicationTestAction::Cancel,
    );
    let error = controller
        .prepare_search_state_for_activation(&token)
        .expect_err("cancelled search repair must stop before completion");

    assert_eq!(error.code, "cancelled");
    let search_path = search_index_path_for_publication(&storage_path, Some(&publication))
        .expect("search generation path");
    assert!(
        read_search_generation_completion(&search_path, &publication.generation_id).is_none(),
        "cancelled repair must not expose a completed generation"
    );
    let mut reader = Storage::open(&storage_path).expect("open strict reader storage");
    let reader_error = match load_persisted_search_state(&mut reader, &storage_path) {
        Err(error) => error,
        Ok(_) => panic!("reader must remain fail-closed after cancelled repair"),
    };
    assert_eq!(reader_error.code, "cache_busy");
}
