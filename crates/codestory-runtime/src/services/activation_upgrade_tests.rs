use super::*;
use crate::Runtime;
use flate2::{Decompress, FlushDecompress, Status};
use std::fs;

const INDEX_ARTIFACT_ENCODING_MAGIC: &[u8; 8] = b"\x89CSIDX1\n";
const MAX_COMPRESSED_INDEX_ARTIFACT_DECODE_BYTES: usize = 64 * 1024 * 1024;

fn decode_seed_parser_artifact(blob: &[u8]) -> serde_json::Value {
    let Some(encoded) = blob.strip_prefix(INDEX_ARTIFACT_ENCODING_MAGIC) else {
        return serde_json::from_slice(blob).expect("legacy raw parser artifact");
    };
    let (raw_len, compressed) = encoded
        .split_at_checked(std::mem::size_of::<u64>())
        .expect("encoded parser artifact header");
    let expected_len = usize::try_from(u64::from_le_bytes(raw_len.try_into().unwrap()))
        .expect("parser artifact length fits this platform");
    assert!(
        expected_len <= MAX_COMPRESSED_INDEX_ARTIFACT_DECODE_BYTES,
        "seed parser artifact exceeds the test decoder bound"
    );
    let mut decoder = Decompress::new(true);
    let mut raw = Vec::with_capacity(expected_len.saturating_add(1));
    loop {
        let input_before = decoder.total_in();
        let output_before = decoder.total_out();
        let input_offset = usize::try_from(input_before).expect("compressed input offset");
        let status = decoder
            .decompress_vec(
                &compressed[input_offset..],
                &mut raw,
                FlushDecompress::Finish,
            )
            .expect("valid seed parser artifact");
        assert!(raw.len() <= expected_len, "seed parser artifact length");
        if status == Status::StreamEnd {
            break;
        }
        assert!(
            decoder.total_in() != input_before || decoder.total_out() != output_before,
            "seed parser artifact is truncated"
        );
    }
    assert_eq!(raw.len(), expected_len, "seed parser artifact length");
    assert_eq!(
        decoder.total_in(),
        compressed.len() as u64,
        "seed parser artifact trailing bytes"
    );
    serde_json::from_slice(&raw).expect("seed parser artifact JSON")
}

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
    connection
        .execute_batch(&format!(
            "PRAGMA user_version = {};
         UPDATE index_artifact_cache SET cache_key = 'legacy-' || cache_key;",
            codestory_store::CURRENT_SCHEMA_VERSION - 1,
        ))
        .expect("simulate prior schema and cache keys");
    let cached_rows = {
        let mut statement = connection
            .prepare("SELECT rowid, artifact_blob FROM index_artifact_cache")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert!(
        !cached_rows.is_empty(),
        "seed must contain parser artifacts"
    );
    for (rowid, blob) in cached_rows {
        let mut artifact = decode_seed_parser_artifact(&blob);
        let fields = artifact
            .as_object_mut()
            .expect("seed parser artifact is an object");
        for field in [
            "resolution_file",
            "resolution_input_schema_version",
            "call_resolution_inputs",
        ] {
            assert!(fields.remove(field).is_some(), "seed artifact has {field}");
        }
        connection
            .execute(
                "UPDATE index_artifact_cache SET artifact_blob = ?1 WHERE rowid = ?2",
                (serde_json::to_vec(&artifact).unwrap(), rowid),
            )
            .unwrap();
    }
    connection
        .execute_batch("DELETE FROM proof_resolution_publication; PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("finish prior schema fixture");
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
