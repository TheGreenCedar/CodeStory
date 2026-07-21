use super::*;
use rusqlite::OptionalExtension;

#[test]
fn file_role_classification_ignores_materialized_repo_cache_prefix() {
    assert_eq!(
        FileRole::classify_path(Path::new(
            "C:/repo/target/repo-cache/repos/nvm-sh-nvm/install.sh"
        )),
        FileRole::Source
    );
    assert_eq!(
        FileRole::classify_path(Path::new(
            "C:/repo/target/repo-cache/repos/psf-requests/tests/test_sessions.py"
        )),
        FileRole::Test
    );
    assert_eq!(
        FileRole::classify_path(Path::new("target/generated/client.ts")),
        FileRole::Generated
    );
}
use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, EdgeId, EdgeKind, ErrorInfo, FileCoverageReason,
    IndexStep, Node, NodeId, NodeKind, Occurrence, OccurrenceKind, ResolutionCertainty,
    SourceLocation, TrailConfig, TrailDirection,
};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::{SystemTime, UNIX_EPOCH};

fn test_publication() -> IndexPublicationRecord {
    IndexPublicationRecord {
        generation: 1,
        generation_id: "test-generation".to_string(),
        run_id: "test-run".to_string(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    }
}

#[test]
fn semantic_projection_compatibility_accepts_only_manifestless_empty_structural_state() {
    let storage = Storage::new_in_memory().expect("create storage");

    assert_eq!(
        storage
            .validate_structural_text_unit_publication_or_legacy_empty(&test_publication())
            .expect("empty legacy state is compatible"),
        StructuralTextPublicationCompatibility::LegacyEmpty
    );
}

#[test]
fn semantic_projection_compatibility_rejects_each_manifestless_nonempty_structural_store() {
    for table in ["unit", "projection", "artifact_cache"] {
        let storage = Storage::new_in_memory().expect("create storage");
        storage
            .conn
            .execute_batch("PRAGMA foreign_keys = OFF")
            .expect("disable fixture foreign keys");
        match table {
            "unit" => storage.conn.execute(
                "INSERT INTO structural_text_unit (
                    node_id, file_id, placement_id, content_hash, source_content_hash,
                    descriptor_version, producer, evidence_tier, resolution, language,
                    kind, start_line, start_col, end_line, end_col, file_role
                 ) VALUES (1, 1, ?1, ?1, ?1, 1, 'test', 'structural_text',
                    'source_range_only', 'text', 1, 1, 1, 1, 1, 'source')",
                ["1".repeat(64)],
            ),
            "projection" => storage.conn.execute(
                "INSERT INTO structural_text_projection (
                    file_id, source_content_hash, descriptor_version, producer,
                    language, file_role, unit_count, unit_digest
                 ) VALUES (1, ?1, 1, 'test', 'text', 'source', 0, ?1)",
                ["2".repeat(64)],
            ),
            "artifact_cache" => storage.conn.execute(
                "INSERT INTO structural_text_artifact_cache (
                    file_path, file_id, cache_key, source_content_hash,
                    descriptor_version, producer, artifact_digest, artifact_blob,
                    updated_at_epoch_ms
                 ) VALUES ('legacy.txt', 1, 'v1:test', ?1, 1, 'test', ?1, X'01', 1)",
                ["3".repeat(64)],
            ),
            _ => unreachable!(),
        }
        .expect("insert manifestless structural fixture");

        let error = storage
            .validate_structural_text_unit_publication_or_legacy_empty(&test_publication())
            .expect_err("nonempty manifestless structural state must fail closed");
        assert!(
            error.to_string().contains("missing for nonempty state"),
            "unexpected {table} error: {error}"
        );
    }
}

fn unique_temp_db_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "codestory-store-{label}-{}-{stamp}.sqlite",
        std::process::id()
    ))
}

fn source_policy_identity(
    policy_version: &str,
    byte_cap: u64,
    structural_unit_cap: u64,
) -> SourcePolicyExclusionPolicyIdentity<'_> {
    SourcePolicyExclusionPolicyIdentity::new(policy_version, byte_cap, structural_unit_cap)
}

fn sqlite_index_exists(storage: &Storage, index_name: &str) -> Result<bool, StorageError> {
    storage
        .conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1
             )",
            [index_name],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

fn create_versioned_observation_fixture(path: &Path, version: u32) {
    let connection = Connection::open(path).expect("create observation fixture");
    connection
        .pragma_update(None, "user_version", version)
        .expect("set observation fixture schema");
    drop(connection);
}

fn assert_no_sqlite_sidecars(path: &Path) {
    assert!(!PathBuf::from(format!("{}-wal", path.display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", path.display())).exists());
    assert!(!PathBuf::from(format!("{}-journal", path.display())).exists());
}

fn assert_core_promotion_stats_reconcile(stats: &CorePromotionStats) {
    let named_ms = stats
        .lock_recovery_ms
        .saturating_add(stats.candidate_validation_ms)
        .saturating_add(stats.previous_validation_ms)
        .saturating_add(stats.rollback_backup_copy_ms.unwrap_or_default())
        .saturating_add(stats.backup_validation_ms.unwrap_or_default())
        .saturating_add(stats.prepared_journal_write_ms)
        .saturating_add(stats.prepared_journal_file_sync_ms)
        .saturating_add(stats.prepared_journal_directory_sync_ms)
        .saturating_add(stats.staged_to_live_restore_ms)
        .saturating_add(stats.promoted_validation_ms)
        .saturating_add(stats.committed_journal_ms)
        .saturating_add(stats.cleanup_ms);
    assert_eq!(
        named_ms.saturating_add(stats.unattributed_ms),
        stats.total_ms
    );
}

fn durable_sqlite_state(path: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    [path.to_path_buf(), sqlite_sidecar_path(path, "-wal")]
        .into_iter()
        .map(|path| {
            let bytes = if path.is_file() {
                Some(fs::read(&path).expect("read durable SQLite state"))
            } else {
                None
            };
            (path, bytes)
        })
        .collect()
}

#[test]
fn file_identity_lookup_batches_above_default_bind_limit_with_set_semantics()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[Node {
        id: NodeId(40_000),
        kind: NodeKind::FILE,
        serialized_name: "large.rs".to_string(),
        ..Default::default()
    }])?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "early_match".to_string(),
            file_node_id: Some(NodeId(40_000)),
            ..Default::default()
        },
        Node {
            id: NodeId(500),
            kind: NodeKind::CLASS,
            serialized_name: "direct_match".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(40_001),
            kind: NodeKind::METHOD,
            serialized_name: "late_match".to_string(),
            file_node_id: Some(NodeId(40_000)),
            ..Default::default()
        },
    ])?;

    let previous_limit = storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 64)?;
    assert!(previous_limit >= 64);

    // Two bindings per candidate made the former single query exceed SQLite's
    // 32,766 default once this set grew past 16,383 IDs.
    let mut candidates = (0_i64..=32_766).collect::<Vec<_>>();
    candidates.extend([40_000, 40_000, 50_000]);
    let node_kinds = storage.get_node_kinds_for_files(&candidates)?;

    assert_eq!(
        node_kinds,
        vec![
            (NodeId(1), NodeKind::FUNCTION),
            (NodeId(500), NodeKind::CLASS),
            (NodeId(40_000), NodeKind::FILE),
            (NodeId(40_001), NodeKind::METHOD),
        ]
    );
    storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, previous_limit)?;
    Ok(())
}

#[test]
fn file_identity_lookup_rejects_runtime_limit_below_two_bindings() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 1)?;

    let error = storage
        .get_node_kinds_for_files(&[1])
        .expect_err("two-predicate lookup must reject a one-variable runtime limit");
    assert!(
        error
            .to_string()
            .contains("cannot support the two file identity predicates"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[test]
fn observational_open_preserves_current_database_bytes_without_sidecars() {
    let path = unique_temp_db_path("observational-current");
    create_versioned_observation_fixture(&path, SCHEMA_VERSION);
    let before = fs::read(&path).expect("read current fixture before observation");
    assert_no_sqlite_sidecars(&path);

    let observed = Storage::open_observational(&path).expect("observe current database");
    assert_eq!(
        observed.schema_version().expect("read observed schema"),
        SCHEMA_VERSION
    );
    drop(observed);

    assert_eq!(
        fs::read(&path).expect("read current fixture after observation"),
        before
    );
    assert_no_sqlite_sidecars(&path);
    fs::remove_file(path).expect("remove current fixture");
}

#[test]
fn freshness_observational_open_accepts_current_schema_without_mutation() {
    let path = unique_temp_db_path("freshness-observational-current");
    {
        let storage = Storage::open(&path).expect("create migrated current fixture");
        let (busy, log_frames, checkpointed_frames): (i64, i64, i64) = storage
            .get_connection()
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("checkpoint current fixture");
        assert_eq!(busy, 0, "current fixture checkpoint remained busy");
        assert_eq!(log_frames, checkpointed_frames);
    }
    let wal_path = sqlite_sidecar_path(&path, "-wal");
    if wal_path.exists() {
        assert_eq!(
            fs::metadata(&wal_path)
                .expect("inspect checkpointed WAL")
                .len(),
            0,
            "current fixture retained uncheckpointed WAL bytes"
        );
        fs::remove_file(&wal_path).expect("remove empty checkpointed WAL");
    }
    let shm_path = sqlite_sidecar_path(&path, "-shm");
    if shm_path.exists() {
        fs::remove_file(&shm_path).expect("remove closed checkpoint SHM");
    }
    let before = durable_sqlite_state(&path);
    assert_no_sqlite_sidecars(&path);

    let observed = Storage::open_freshness_observational(&path)
        .expect("freshness observer should accept the current schema");
    assert_eq!(
        observed.schema_version().expect("read observed schema"),
        SCHEMA_VERSION
    );
    assert!(
        !observed
            .has_incomplete_incremental_run()
            .expect("read current marker")
    );
    drop(observed);

    assert_eq!(durable_sqlite_state(&path), before);
    assert_no_sqlite_sidecars(&path);
    fs::remove_file(path).expect("remove current freshness fixture");
}

#[test]
fn freshness_observational_open_accepts_only_a_durably_marked_incomplete_sentinel() {
    let path = unique_temp_db_path("freshness-observational-fenced");
    {
        let storage = Storage::open(&path).expect("open fenced fixture");
        storage
            .begin_incremental_run()
            .expect("install incomplete-run fence");
    }
    let read_only_error = Storage::open_read_only(&path)
        .err()
        .expect("ordinary read-only open must reject the sentinel");
    assert!(
        read_only_error
            .to_string()
            .contains("requires schema version"),
        "{read_only_error}"
    );
    let observational_error = Storage::open_observational(&path)
        .err()
        .expect("ordinary observation must reject the sentinel");
    assert!(
        observational_error
            .to_string()
            .contains("requires schema version"),
        "{observational_error}"
    );

    let before = durable_sqlite_state(&path);
    let observed = Storage::open_freshness_observational(&path)
        .expect("freshness observer should accept the fenced sentinel");
    assert_eq!(
        observed.schema_version().expect("read fenced schema"),
        INCOMPLETE_INCREMENTAL_SCHEMA_VERSION
    );
    assert!(
        observed
            .has_incomplete_incremental_run()
            .expect("read durable incomplete marker")
    );
    drop(observed);

    assert_eq!(durable_sqlite_state(&path), before);
    let verification = Storage::open(&path).expect("reopen fenced fixture");
    assert_eq!(
        verification.schema_version().expect("verify fenced schema"),
        INCOMPLETE_INCREMENTAL_SCHEMA_VERSION
    );
    assert!(
        verification
            .has_incomplete_incremental_run()
            .expect("verify durable marker")
    );
    drop(verification);
    let _ = cleanup_sqlite_sidecars(&path);
}

#[test]
fn freshness_observational_open_rejects_unmarked_sentinel_and_arbitrary_schemas_without_mutation() {
    for (label, version, expected_error) in [
        (
            "unmarked-sentinel",
            INCOMPLETE_INCREMENTAL_SCHEMA_VERSION,
            "durable incomplete-run marker",
        ),
        ("old-schema", SCHEMA_VERSION - 1, "requires schema version"),
        (
            "future-schema",
            SCHEMA_VERSION + 1,
            "requires schema version",
        ),
    ] {
        let path = unique_temp_db_path(label);
        create_versioned_observation_fixture(&path, version);
        let before = durable_sqlite_state(&path);
        assert_no_sqlite_sidecars(&path);

        let error = Storage::open_freshness_observational(&path)
            .err()
            .expect("unsupported freshness schema must fail closed");
        assert!(error.to_string().contains(expected_error), "{error}");
        assert_eq!(durable_sqlite_state(&path), before);
        assert_no_sqlite_sidecars(&path);
        fs::remove_file(path).expect("remove rejected freshness fixture");
    }
}

#[test]
fn observational_open_reads_committed_wal_without_mutating_durable_sqlite_state() {
    let path = unique_temp_db_path("observational-wal");
    let storage = Storage::open(&path).expect("open WAL fixture storage");
    let publication = IndexPublicationRecord {
        generation: 2,
        generation_id: "22222222-2222-4222-8222-222222222222".into(),
        run_id: "observational-wal-run".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 2,
    };
    storage
        .put_index_publication(&publication)
        .expect("publish committed WAL fixture");
    let wal_path = sqlite_sidecar_path(&path, "-wal");
    let shm_path = sqlite_sidecar_path(&path, "-shm");
    assert!(
        wal_path.is_file(),
        "fixture must retain committed WAL state"
    );
    assert!(shm_path.is_file(), "fixture must retain its WAL index");
    let durable_paths = [&path, &wal_path];
    let before = durable_paths
        .iter()
        .map(|path| fs::read(path).expect("read SQLite fixture before observation"))
        .collect::<Vec<_>>();
    let shm_len_before = fs::metadata(&shm_path)
        .expect("inspect SHM before observation")
        .len();

    let observed = Storage::open_observational(&path).expect("observe WAL-backed database");
    assert_eq!(
        observed
            .get_complete_index_publication()
            .expect("read observed WAL publication"),
        Some(publication)
    );
    drop(observed);

    let after = durable_paths
        .iter()
        .map(|path| fs::read(path).expect("read SQLite fixture after observation"))
        .collect::<Vec<_>>();
    assert_eq!(after, before, "observation changed durable SQLite state");
    assert_eq!(
        fs::metadata(&shm_path)
            .expect("SHM must remain after observation")
            .len(),
        shm_len_before,
        "observation materialized or resized the existing SHM wal-index"
    );
    drop(storage);
    if wal_path.exists() {
        fs::remove_file(&wal_path).expect("remove WAL fixture");
    }
    if shm_path.exists() {
        fs::remove_file(&shm_path).expect("remove SHM fixture");
    }
    fs::remove_file(path).expect("remove WAL database fixture");
}

#[test]
fn freshness_observational_open_preserves_fenced_wal_state_and_marker() {
    let path = unique_temp_db_path("freshness-observational-fenced-wal");
    let storage = Storage::open(&path).expect("open fenced WAL fixture");
    storage
        .begin_incremental_run()
        .expect("install fenced WAL marker");
    let wal_path = sqlite_sidecar_path(&path, "-wal");
    let shm_path = sqlite_sidecar_path(&path, "-shm");
    assert!(wal_path.is_file(), "fixture must retain fenced WAL state");
    assert!(shm_path.is_file(), "fixture must retain its WAL index");
    let before = durable_sqlite_state(&path);
    let shm_len_before = fs::metadata(&shm_path)
        .expect("inspect fenced SHM before observation")
        .len();

    let observed = Storage::open_freshness_observational(&path)
        .expect("freshness observer should read the fenced WAL snapshot");
    assert_eq!(
        observed.schema_version().expect("read fenced WAL schema"),
        INCOMPLETE_INCREMENTAL_SCHEMA_VERSION
    );
    assert!(
        observed
            .has_incomplete_incremental_run()
            .expect("read fenced WAL marker")
    );
    drop(observed);

    assert_eq!(durable_sqlite_state(&path), before);
    assert_eq!(
        fs::metadata(&shm_path)
            .expect("SHM must remain after freshness observation")
            .len(),
        shm_len_before,
        "freshness observation materialized or resized the existing SHM wal-index"
    );
    assert_eq!(
        storage.schema_version().expect("verify live fenced schema"),
        INCOMPLETE_INCREMENTAL_SCHEMA_VERSION
    );
    assert!(
        storage
            .has_incomplete_incremental_run()
            .expect("verify live fenced marker")
    );

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&path);
}

#[test]
fn observational_wal_snapshot_pins_frames_during_concurrent_checkpoint() {
    let path = unique_temp_db_path("observational-wal-checkpoint");
    let storage = Storage::open(&path).expect("open concurrent WAL fixture");
    let first = IndexPublicationRecord {
        generation: 1,
        generation_id: "11111111-1111-4111-8111-111111111111".into(),
        run_id: "observational-wal-run-one".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    };
    let second = IndexPublicationRecord {
        generation: 2,
        generation_id: "22222222-2222-4222-8222-222222222222".into(),
        run_id: "observational-wal-run-two".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 2,
    };
    storage
        .put_index_publication(&first)
        .expect("publish first WAL identity");
    let observed = Storage::open_observational(&path).expect("open WAL observer");
    let snapshot = observed.read_snapshot().expect("pin WAL snapshot");
    assert_eq!(
        snapshot
            .storage()
            .get_complete_index_publication()
            .expect("read first pinned identity"),
        Some(first.clone())
    );

    storage
        .put_index_publication(&second)
        .expect("publish concurrent WAL identity");
    let (busy, _, _): (i64, i64, i64) = storage
        .get_connection()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("attempt checkpoint while observer is pinned");
    assert_ne!(busy, 0, "checkpoint truncated frames held by observer");
    assert_eq!(
        snapshot
            .storage()
            .get_complete_index_publication()
            .expect("reread pinned identity"),
        Some(first)
    );
    snapshot.finish().expect("release WAL snapshot");
    drop(observed);

    let (busy, _, _): (i64, i64, i64) = storage
        .get_connection()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("checkpoint after observer release");
    assert_eq!(busy, 0);
    let current = Storage::open_observational(&path).expect("observe current checkpointed state");
    assert_eq!(
        current
            .get_complete_index_publication()
            .expect("read current identity"),
        Some(second)
    );
    drop(current);
    drop(storage);
    let wal_path = sqlite_sidecar_path(&path, "-wal");
    let shm_path = sqlite_sidecar_path(&path, "-shm");
    if wal_path.exists() {
        fs::remove_file(wal_path).expect("remove checkpoint WAL fixture");
    }
    if shm_path.exists() {
        fs::remove_file(shm_path).expect("remove checkpoint SHM fixture");
    }
    fs::remove_file(path).expect("remove checkpoint database fixture");
}

#[test]
fn observational_open_reports_incomplete_wal_pair_without_materializing_shm() {
    let path = unique_temp_db_path("observational-incomplete-wal");
    create_versioned_observation_fixture(&path, SCHEMA_VERSION);
    let wal_path = sqlite_sidecar_path(&path, "-wal");
    let shm_path = sqlite_sidecar_path(&path, "-shm");
    fs::write(&wal_path, b"incomplete WAL fixture").expect("write WAL without SHM");
    let database_before = fs::read(&path).expect("read incomplete-WAL database");
    let wal_before = fs::read(&wal_path).expect("read incomplete WAL");

    let error = Storage::open_observational(&path)
        .err()
        .expect("incomplete WAL pair must fail closed");
    assert!(error.to_string().contains("incomplete WAL sidecar pair"));
    let freshness_error = Storage::open_freshness_observational(&path)
        .err()
        .expect("freshness observation must reject an incomplete WAL pair");
    assert!(
        freshness_error
            .to_string()
            .contains("incomplete WAL sidecar pair")
    );

    assert_eq!(fs::read(&path).expect("reread database"), database_before);
    assert_eq!(fs::read(&wal_path).expect("reread WAL"), wal_before);
    assert!(!shm_path.exists(), "observation materialized missing SHM");
    fs::remove_file(wal_path).expect("remove incomplete WAL");
    fs::remove_file(path).expect("remove incomplete-WAL database");
}

#[test]
fn observational_open_reports_rollback_journal_without_recovery() {
    let path = unique_temp_db_path("observational-rollback-journal");
    create_versioned_observation_fixture(&path, SCHEMA_VERSION);
    let journal_path = sqlite_sidecar_path(&path, "-journal");
    fs::write(&journal_path, b"pending rollback evidence").expect("write rollback journal");
    let database_before = fs::read(&path).expect("read rollback database");
    let journal_before = fs::read(&journal_path).expect("read rollback journal");

    let error = Storage::open_observational(&path)
        .err()
        .expect("rollback recovery must fail closed");
    assert!(error.to_string().contains("rollback recovery is pending"));
    let freshness_error = Storage::open_freshness_observational(&path)
        .err()
        .expect("freshness observation must reject rollback recovery");
    assert!(
        freshness_error
            .to_string()
            .contains("rollback recovery is pending")
    );

    assert_eq!(fs::read(&path).expect("reread database"), database_before);
    assert_eq!(
        fs::read(&journal_path).expect("reread rollback journal"),
        journal_before
    );
    fs::remove_file(journal_path).expect("remove rollback journal");
    fs::remove_file(path).expect("remove rollback database");
}

#[test]
fn observational_open_reports_old_schema_without_migration_or_sidecars() {
    let path = unique_temp_db_path("observational-old-schema");
    create_versioned_observation_fixture(&path, SCHEMA_VERSION - 1);
    let before = fs::read(&path).expect("read old-schema fixture before observation");
    assert_no_sqlite_sidecars(&path);

    assert_eq!(
        Storage::database_schema_version_observational(&path)
            .expect("inspect old schema without migration"),
        SCHEMA_VERSION - 1
    );
    assert_eq!(
        fs::read(&path).expect("read old-schema fixture after version observation"),
        before
    );
    assert_no_sqlite_sidecars(&path);

    let error = Storage::open_observational(&path)
        .err()
        .expect("old schema must fail closed");
    assert!(
        error.to_string().contains("requires schema version"),
        "{error}"
    );

    assert_eq!(
        fs::read(&path).expect("read old-schema fixture after observation"),
        before
    );
    assert_no_sqlite_sidecars(&path);
    fs::remove_file(path).expect("remove old-schema fixture");
}

#[test]
fn observational_open_reports_pending_promotion_without_recovery() {
    let path = unique_temp_db_path("observational-promotion");
    create_versioned_observation_fixture(&path, SCHEMA_VERSION);
    let prepared = promotion_prepared_journal_path(&path);
    fs::write(&prepared, b"pending promotion evidence").expect("write pending promotion fixture");
    let database_before = fs::read(&path).expect("read promotion database before observation");
    let journal_before = fs::read(&prepared).expect("read promotion journal before observation");
    assert_no_sqlite_sidecars(&path);

    let error = Storage::open_observational(&path)
        .err()
        .expect("pending recovery must fail closed");
    assert!(error.to_string().contains("recovery is pending"), "{error}");
    let freshness_error = Storage::open_freshness_observational(&path)
        .err()
        .expect("freshness observation must reject pending promotion");
    assert!(
        freshness_error.to_string().contains("recovery is pending"),
        "{freshness_error}"
    );

    assert_eq!(
        fs::read(&path).expect("read promotion database after observation"),
        database_before
    );
    assert_eq!(
        fs::read(&prepared).expect("read promotion journal after observation"),
        journal_before
    );
    assert_no_sqlite_sidecars(&path);
    fs::remove_file(prepared).expect("remove promotion journal fixture");
    fs::remove_file(path).expect("remove promotion database fixture");
}

#[test]
fn write_transaction_commits_or_rolls_back_as_one_unit() {
    let path = unique_temp_db_path("write-transaction");
    let mut storage = Storage::open(&path).expect("open storage");

    {
        let mut transaction = storage.write_transaction().expect("begin transaction");
        transaction
            .storage_mut()
            .conn
            .execute("CREATE TABLE publication_probe (value INTEGER)", [])
            .expect("create rollback probe");
    }
    assert!(
        storage
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'publication_probe'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .expect("query rollback probe")
            .is_none()
    );

    let mut transaction = storage.write_transaction().expect("begin transaction");
    transaction
        .storage_mut()
        .conn
        .execute("CREATE TABLE publication_probe (value INTEGER)", [])
        .expect("create commit probe");
    transaction.finish().expect("commit transaction");
    assert!(
        storage
            .conn
            .execute("DROP TABLE publication_probe", [])
            .is_ok()
    );
    drop(storage);
    let _ = std::fs::remove_file(path);
}

#[test]
fn incomplete_incremental_run_marker_survives_reopen_until_success() -> Result<(), StorageError> {
    let path = unique_temp_db_path("incomplete-incremental-run");
    {
        let storage = Storage::open(&path)?;
        assert_eq!(Storage::database_schema_version(&path)?, SCHEMA_VERSION);
        assert!(!Storage::database_has_incomplete_incremental_run(&path)?);
        assert!(!storage.has_incomplete_incremental_run()?);
        storage.begin_incremental_run()?;
        assert!(storage.has_incomplete_incremental_run()?);
        assert!(Storage::database_has_incomplete_incremental_run(&path)?);
        assert_eq!(
            Storage::database_schema_version(&path)?,
            INCOMPLETE_INCREMENTAL_SCHEMA_VERSION
        );
    }
    {
        let storage = Storage::open(&path)?;
        assert!(storage.has_incomplete_incremental_run()?);
        storage.finish_incremental_run()?;
        assert!(!storage.has_incomplete_incremental_run()?);
        assert!(!Storage::database_has_incomplete_incremental_run(&path)?);
        assert_eq!(Storage::database_schema_version(&path)?, SCHEMA_VERSION);
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    Ok(())
}

#[test]
fn index_publication_identity_round_trips_through_typed_and_read_only_reads()
-> Result<(), StorageError> {
    let path = unique_temp_db_path("index-publication-round-trip");
    let publication = IndexPublicationRecord {
        generation: 7,
        generation_id: "generation-7".to_string(),
        run_id: "run-7".to_string(),
        mode: IndexPublicationMode::Incremental,
        published_at_epoch_ms: 1234,
    };
    {
        let storage = Storage::open(&path)?;
        assert!(storage.get_index_publication()?.is_none());
        storage.put_index_publication(&publication)?;
        assert_eq!(storage.get_index_publication()?, Some(publication.clone()));
    }
    assert_eq!(
        Storage::database_index_publication(&path)?,
        Some(publication)
    );

    let _ = cleanup_sqlite_sidecars(&path);
    Ok(())
}

#[test]
fn source_policy_exclusion_publication_binds_complete_rows_to_core_identity()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let publication = IndexPublicationRecord {
        generation: 4,
        generation_id: "generation-4".into(),
        run_id: "run-4".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 44,
    };
    let candidates = vec![
        OversizedSourceExclusionCandidate {
            normalized_path: "src/generated/registers.h".into(),
            content_hash: "a".repeat(64),
            observed_size: 4_000_000,
            observed_unit_count: 0,
            policy_version: "oversized-source-v1".into(),
            byte_cap: 1_000_000,
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        },
        OversizedSourceExclusionCandidate {
            normalized_path: "vendor/ordinary.rs".into(),
            content_hash: "b".repeat(64),
            observed_size: 1_000_001,
            observed_unit_count: 0,
            policy_version: "oversized-source-v1".into(),
            byte_cap: 1_000_000,
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        },
        OversizedSourceExclusionCandidate {
            normalized_path: "work/evidence.json".into(),
            content_hash: "c".repeat(64),
            observed_size: 250_000,
            observed_unit_count: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP + 1,
            policy_version: "oversized-source-v1".into(),
            byte_cap: 1_000_000,
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        },
    ];

    let manifest = storage.publish_source_policy_exclusion_generation(
        &publication,
        "project-4",
        "workspace-4",
        source_policy_identity(
            "oversized-source-v1",
            1_000_000,
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        ),
        &candidates,
    )?;
    assert_eq!(manifest.exclusion_count, 3);
    assert_eq!(manifest.exclusion_digest.len(), 64);
    assert_eq!(storage.get_source_policy_exclusions()?.len(), 3);
    assert_eq!(
        storage.validate_source_policy_exclusion_publication(
            &publication,
            "project-4",
            "workspace-4",
            source_policy_identity(
                "oversized-source-v1",
                1_000_000,
                codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            ),
        )?,
        manifest
    );

    storage.conn.execute(
        "UPDATE source_policy_exclusion SET content_hash = ?1 WHERE normalized_path = ?2",
        params!["c".repeat(64), "vendor/ordinary.rs"],
    )?;
    assert!(
        storage
            .validate_source_policy_exclusion_publication(
                &publication,
                "project-4",
                "workspace-4",
                source_policy_identity(
                    "oversized-source-v1",
                    1_000_000,
                    codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
                ),
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn source_policy_exclusion_transaction_failure_preserves_previous_manifest()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let first_publication = IndexPublicationRecord {
        generation: 1,
        generation_id: "generation-1".into(),
        run_id: "run-1".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 11,
    };
    let first = vec![OversizedSourceExclusionCandidate {
        normalized_path: "vendor/first.h".into(),
        content_hash: "a".repeat(64),
        observed_size: 2_000_000,
        observed_unit_count: 0,
        policy_version: "oversized-source-v1".into(),
        byte_cap: 1_000_000,
        structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
    }];
    let expected = storage.publish_source_policy_exclusion_generation(
        &first_publication,
        "project",
        "workspace",
        source_policy_identity(
            "oversized-source-v1",
            1_000_000,
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        ),
        &first,
    )?;
    storage.conn.execute_batch(
        "CREATE TRIGGER reject_second_policy_exclusion
         BEFORE INSERT ON source_policy_exclusion
         WHEN NEW.normalized_path = 'vendor/reject.h'
         BEGIN
           SELECT RAISE(ABORT, 'injected exclusion write failure');
         END;",
    )?;
    let second_publication = IndexPublicationRecord {
        generation: 2,
        generation_id: "generation-2".into(),
        run_id: "run-2".into(),
        mode: IndexPublicationMode::Incremental,
        published_at_epoch_ms: 22,
    };
    let second = vec![OversizedSourceExclusionCandidate {
        normalized_path: "vendor/reject.h".into(),
        content_hash: "b".repeat(64),
        observed_size: 3_000_000,
        observed_unit_count: 0,
        policy_version: "oversized-source-v1".into(),
        byte_cap: 1_000_000,
        structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
    }];
    assert!(
        storage
            .publish_source_policy_exclusion_generation(
                &second_publication,
                "project",
                "workspace",
                source_policy_identity(
                    "oversized-source-v1",
                    1_000_000,
                    codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
                ),
                &second,
            )
            .is_err()
    );
    assert_eq!(
        storage.get_source_policy_exclusion_manifest()?,
        Some(expected)
    );
    assert_eq!(storage.get_source_policy_exclusions()?.len(), 1);
    storage.validate_source_policy_exclusion_publication(
        &first_publication,
        "project",
        "workspace",
        source_policy_identity(
            "oversized-source-v1",
            1_000_000,
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        ),
    )?;
    Ok(())
}

#[test]
fn index_publication_identity_rejects_negative_published_timestamp() {
    assert!(
        index_publication_record_from_values(
            1,
            "generation-1".to_string(),
            "run-1".to_string(),
            "full".to_string(),
            -1,
        )
        .is_err()
    );
}

#[test]
fn schema_18_migrates_to_empty_publication_identity_without_synthesis() -> Result<(), StorageError>
{
    let path = unique_temp_db_path("index-publication-v18-migration");
    {
        let storage = Storage::open(&path)?;
        storage
            .get_connection()
            .execute_batch("DROP TABLE index_publication;")?;
        storage.set_schema_version(18)?;
    }

    assert!(Storage::database_index_publication(&path)?.is_none());
    let storage = Storage::open(&path)?;
    assert_eq!(Storage::database_schema_version(&path)?, SCHEMA_VERSION);
    assert!(storage.get_index_publication()?.is_none());

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&path);
    Ok(())
}

#[test]
fn schema_19_adds_nullable_file_content_hash_without_losing_rows() -> Result<(), StorageError> {
    let path = unique_temp_db_path("file-content-hash-v19-migration");
    {
        let conn = rusqlite::Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE file (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                language TEXT,
                modification_time INTEGER,
                indexed INTEGER DEFAULT 0,
                complete INTEGER DEFAULT 0,
                line_count INTEGER DEFAULT 0,
                file_role TEXT NOT NULL DEFAULT 'source'
            );
            INSERT INTO file (
                id, path, language, modification_time, indexed, complete, line_count, file_role
            ) VALUES (7, 'src/lib.rs', 'rust', 42, 1, 1, 3, 'source');
            PRAGMA user_version = 19;",
        )?;
    }

    let storage = Storage::open(&path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    assert_eq!(storage.get_files()?.len(), 1);
    assert_eq!(storage.get_file_content_hash(7)?, None);

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&path);
    Ok(())
}

#[test]
fn incomplete_incremental_begin_failure_keeps_clean_schema_and_no_marker()
-> Result<(), StorageError> {
    let path = unique_temp_db_path("incomplete-begin-rollback");
    let storage = Storage::open(&path)?;
    storage.get_connection().execute_batch(
        "CREATE TRIGGER fail_incomplete_begin
         BEFORE INSERT ON incomplete_index_run
         BEGIN SELECT RAISE(ABORT, 'forced marker insert failure'); END;",
    )?;

    assert!(storage.begin_incremental_run().is_err());
    assert!(!storage.has_incomplete_incremental_run()?);
    assert_eq!(Storage::database_schema_version(&path)?, SCHEMA_VERSION);

    drop(storage);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    Ok(())
}

#[test]
fn transient_incomplete_schema_fence_requires_marker() -> Result<(), StorageError> {
    let path = unique_temp_db_path("incomplete-schema-fence");
    {
        let storage = Storage::open(&path)?;
        storage.set_schema_version(INCOMPLETE_INCREMENTAL_SCHEMA_VERSION)?;
    }

    assert!(Storage::database_has_incomplete_incremental_run(&path).is_err());
    let error = match Storage::open(&path) {
        Ok(_) => panic!("schema fence without marker must fail closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("marked incomplete"));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    Ok(())
}

#[test]
fn interrupted_v19_run_migrates_manifest_column_without_clearing_fence() -> Result<(), StorageError>
{
    let path = unique_temp_db_path("interrupted-v19-manifest-migration");
    {
        let storage = Storage::open(&path)?;
        storage.get_connection().execute(
            "ALTER TABLE retrieval_index_manifest RENAME COLUMN lexical_version TO zoekt_version",
            [],
        )?;
        storage.begin_incremental_run()?;
    }

    let storage = Storage::open(&path)?;
    assert_eq!(
        Storage::database_schema_version(&path)?,
        INCOMPLETE_INCREMENTAL_SCHEMA_VERSION
    );
    assert!(storage.has_incomplete_incremental_run()?);
    let columns = storage
        .conn
        .prepare("PRAGMA table_info(retrieval_index_manifest)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(columns.iter().any(|column| column == "lexical_version"));
    assert!(
        columns
            .iter()
            .any(|column| column == "rollback_record_json")
    );
    assert!(!columns.iter().any(|column| column == "zoekt_version"));
    storage.finish_incremental_run()?;
    assert_eq!(Storage::database_schema_version(&path)?, SCHEMA_VERSION);

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&path);
    Ok(())
}

#[test]
fn sequential_future_schema_is_not_mistaken_for_incomplete_fence() -> Result<(), StorageError> {
    let path = unique_temp_db_path("future-schema-fence");
    {
        let storage = Storage::open(&path)?;
        storage.begin_incremental_run()?;
        storage.set_schema_version(SCHEMA_VERSION + 1)?;
    }

    assert!(Storage::database_has_incomplete_incremental_run(&path).is_err());
    let error = match Storage::open(&path) {
        Ok(_) => panic!("future schema must fail even when the incomplete marker exists"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Unsupported database schema"));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    Ok(())
}

#[test]
fn test_batch_inserts() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let nodes = vec![
        Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "func1".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::CLASS,
            serialized_name: "Class1".to_string(),
            ..Default::default()
        },
    ];

    storage.insert_nodes_batch(&nodes)?;

    let mut stmt = storage.conn.prepare("SELECT count(*) FROM node")?;
    let count: i64 = stmt.query_row([], |row| row.get(0))?;
    assert_eq!(count, 2);

    Ok(())
}

fn file_node(id: i64, path: &str) -> Node {
    Node {
        id: NodeId(id),
        kind: NodeKind::FILE,
        serialized_name: path.to_string(),
        start_line: Some(1),
        start_col: Some(1),
        end_line: Some(1),
        end_col: Some(1),
        ..Default::default()
    }
}

fn insert_file_row(storage: &Storage, id: i64, path: &str) -> Result<(), StorageError> {
    storage.insert_file(&FileInfo {
        id,
        path: PathBuf::from(path),
        language: "typescript".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 1,
        file_role: FileRole::Source,
    })
}

#[test]
fn openapi_endpoint_projection_requires_file_owned_graph_evidence() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_rows = [
        (1, "openapi.json"),
        (2, "metadata-only.json"),
        (3, "wrong-file.yaml"),
        (4, "ordinary-function.json"),
        (5, "empty-endpoint.json"),
    ];
    for &(id, path) in &file_rows {
        insert_file_row(&storage, id, path)?;
    }
    let file_nodes = file_rows
        .iter()
        .map(|(id, path)| file_node(*id, path))
        .collect::<Vec<_>>();
    let endpoints = [
        Node {
            id: NodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "GET /ready".to_string(),
            canonical_id: Some("openapi:endpoint:GET /ready".to_string()),
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
        Node {
            id: NodeId(201),
            kind: NodeKind::FUNCTION,
            serialized_name: "GET /forged".to_string(),
            canonical_id: Some("openapi:endpoint:GET /forged".to_string()),
            file_node_id: Some(NodeId(2)),
            ..Default::default()
        },
        Node {
            id: NodeId(301),
            kind: NodeKind::FUNCTION,
            serialized_name: "GET /wrong-file".to_string(),
            canonical_id: Some("openapi:endpoint:GET /wrong-file".to_string()),
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
        Node {
            id: NodeId(401),
            kind: NodeKind::FUNCTION,
            serialized_name: "ordinary".to_string(),
            canonical_id: Some("route_endpoint:GET /ordinary".to_string()),
            file_node_id: Some(NodeId(4)),
            ..Default::default()
        },
        Node {
            id: NodeId(501),
            kind: NodeKind::FUNCTION,
            serialized_name: "empty".to_string(),
            canonical_id: Some("openapi:endpoint:".to_string()),
            file_node_id: Some(NodeId(5)),
            ..Default::default()
        },
    ];
    let mut nodes = file_nodes;
    nodes.extend(endpoints.iter().cloned());
    storage.insert_nodes_batch(&nodes)?;

    let graph_files = [(1, 101), (3, 301), (4, 401), (5, 501)];
    storage.insert_edges_batch(
        &graph_files
            .iter()
            .map(|(file_id, endpoint_id)| Edge {
                id: EdgeId(10_000 + endpoint_id),
                source: NodeId(*file_id),
                target: NodeId(*endpoint_id),
                kind: EdgeKind::MEMBER,
                file_node_id: Some(NodeId(*file_id)),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )?;
    storage.insert_occurrences_batch(
        &graph_files
            .iter()
            .map(|(file_id, endpoint_id)| Occurrence {
                element_id: *endpoint_id,
                kind: OccurrenceKind::DEFINITION,
                location: SourceLocation {
                    file_node_id: NodeId(*file_id),
                    start_line: 1,
                    start_col: 1,
                    end_line: 1,
                    end_col: 2,
                },
            })
            .collect::<Vec<_>>(),
    )?;

    assert!(storage.has_file_owned_openapi_endpoint_projection(1)?);
    for file_id in [2, 3, 4, 5] {
        assert!(
            !storage.has_file_owned_openapi_endpoint_projection(file_id)?,
            "file {file_id} must not authenticate forged OpenAPI projection evidence"
        );
    }
    Ok(())
}

#[test]
fn projection_file_upsert_updates_language_across_structural_transitions()
-> Result<(), StorageError> {
    fn flush_file(storage: &mut Storage, file: &FileInfo) -> Result<(), StorageError> {
        storage
            .flush_projection_batch(ProjectionBatch {
                files: std::slice::from_ref(file),
                file_content_hashes: &[],
                nodes: &[],
                structural_text_units: &[],
                structural_text_projections: &[],
                structural_text_cache_writes: &[],
                edges: &[],
                occurrences: &[],
                component_access: &[],
                callable_projection_states: &[],
                file_errors: &[],
            })
            .map(|_| ())
    }

    let mut storage = Storage::new_in_memory()?;
    let path = PathBuf::from("config.json");
    let mut file = FileInfo {
        id: 77,
        path: path.clone(),
        language: "json".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 1,
        file_role: FileRole::Source,
    };
    storage.insert_file(&file)?;

    for language in ["openapi", "json"] {
        file.language = language.to_string();
        file.modification_time += 1;
        flush_file(&mut storage, &file)?;
        let stored = storage.get_files()?;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].path, path);
        assert_eq!(stored[0].language, language);
    }

    file.language = "openapi".to_string();
    file.complete = false;
    file.modification_time += 1;
    flush_file(&mut storage, &file)?;
    let stored = storage.get_files()?;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].path, path);
    assert_eq!(
        stored[0].language, "json",
        "incomplete retry evidence must retain the previous verified classification"
    );
    assert!(!stored[0].complete);
    Ok(())
}

#[test]
fn framework_synthetic_node_source_metadata_prefers_definitions() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    insert_file_row(&storage, 1, "src/routes/+page.svelte")?;
    insert_file_row(&storage, 2, "src-tauri/src/lib.rs")?;

    let usage_file = file_node(1, "src/routes/+page.svelte");
    let definition_file = file_node(2, "src-tauri/src/lib.rs");
    let usage = Node {
        id: NodeId(42),
        kind: NodeKind::FUNCTION,
        serialized_name: "tauri command get_snapshot (tauri command; confidence=heuristic)"
            .to_string(),
        qualified_name: Some("framework::tauri::command::get_snapshot".to_string()),
        canonical_id: Some("tauri:command:get_snapshot".to_string()),
        file_node_id: Some(NodeId(1)),
        start_line: Some(7),
        start_col: Some(1),
        ..Default::default()
    };
    let definition = Node {
        file_node_id: Some(NodeId(2)),
        start_line: Some(21),
        ..usage.clone()
    };

    storage.insert_nodes_batch(&[usage_file.clone(), definition_file.clone(), usage.clone()])?;
    storage.insert_nodes_batch(&[definition_file.clone(), definition.clone()])?;
    assert_eq!(
        storage
            .get_node(NodeId(42))?
            .and_then(|node| node.file_node_id),
        Some(NodeId(2))
    );

    let mut reverse = Storage::new_in_memory()?;
    insert_file_row(&reverse, 1, "src/routes/+page.svelte")?;
    insert_file_row(&reverse, 2, "src-tauri/src/lib.rs")?;
    reverse.insert_nodes_batch(&[usage_file, definition_file.clone(), definition])?;
    reverse.insert_nodes_batch(&[definition_file, usage])?;
    assert_eq!(
        reverse
            .get_node(NodeId(42))?
            .and_then(|node| node.file_node_id),
        Some(NodeId(2))
    );

    insert_file_row(&reverse, 3, "app/posts/[slug]/page.tsx")?;
    insert_file_row(&reverse, 4, "src/collections/Posts.ts")?;
    let payload_usage_file = file_node(3, "app/posts/[slug]/page.tsx");
    let payload_definition_file = file_node(4, "src/collections/Posts.ts");
    let payload_usage = Node {
        id: NodeId(77),
        kind: NodeKind::CONSTANT,
        serialized_name: "payload collection posts (collection; confidence=heuristic)".to_string(),
        qualified_name: Some("framework::payload::collection::posts".to_string()),
        canonical_id: Some("payload:collection:posts".to_string()),
        file_node_id: Some(NodeId(3)),
        start_line: Some(12),
        start_col: Some(37),
        ..Default::default()
    };
    let payload_definition = Node {
        file_node_id: Some(NodeId(4)),
        start_line: Some(3),
        start_col: Some(1),
        ..payload_usage.clone()
    };

    reverse.insert_nodes_batch(&[
        payload_definition_file.clone(),
        payload_usage_file.clone(),
        payload_definition,
    ])?;
    reverse.insert_nodes_batch(&[payload_usage_file, payload_usage])?;
    assert_eq!(
        reverse
            .get_node(NodeId(77))?
            .and_then(|node| node.file_node_id),
        Some(NodeId(4))
    );

    Ok(())
}

#[test]
fn endpoint_synthetic_node_source_metadata_is_stable_for_duplicate_routes()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    insert_file_row(&storage, 10, "src/routes/admin.ts")?;
    insert_file_row(&storage, 11, "src/routes/api.ts")?;

    let admin_file = file_node(10, "src/routes/admin.ts");
    let api_file = file_node(11, "src/routes/api.ts");
    let canonical_id = r#"route_endpoint:{"framework":"express","method":"GET","path":"/users","raw_path":"/users","params":[],"confidence":"heuristic","source_convention":"call","provenance":["framework:express"]}"#;
    let admin_route = Node {
        id: NodeId(901),
        kind: NodeKind::FUNCTION,
        serialized_name: "GET /users (express route; confidence=heuristic)".to_string(),
        qualified_name: Some("framework::express::GET /users".to_string()),
        canonical_id: Some(canonical_id.to_string()),
        file_node_id: Some(NodeId(10)),
        start_line: Some(8),
        start_col: Some(1),
        ..Default::default()
    };
    let api_route = Node {
        file_node_id: Some(NodeId(11)),
        start_line: Some(42),
        ..admin_route.clone()
    };

    storage.insert_nodes_batch(&[api_file.clone(), admin_file.clone(), api_route.clone()])?;
    storage.insert_nodes_batch(&[admin_file.clone(), admin_route.clone()])?;
    assert_eq!(
        storage
            .get_node(NodeId(901))?
            .and_then(|node| node.file_node_id),
        Some(NodeId(10))
    );

    let mut reverse = Storage::new_in_memory()?;
    insert_file_row(&reverse, 10, "src/routes/admin.ts")?;
    insert_file_row(&reverse, 11, "src/routes/api.ts")?;
    reverse.insert_nodes_batch(&[admin_file, api_file.clone(), admin_route])?;
    reverse.insert_nodes_batch(&[api_file, api_route])?;
    assert_eq!(
        reverse
            .get_node(NodeId(901))?
            .and_then(|node| node.file_node_id),
        Some(NodeId(10))
    );

    Ok(())
}

#[test]
fn projection_flush_prefers_framework_definition_over_usage() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    insert_file_row(&storage, 1, "src/routes/+page.svelte")?;
    insert_file_row(&storage, 2, "src-tauri/src/lib.rs")?;

    let usage_file = file_node(1, "src/routes/+page.svelte");
    let definition_file = file_node(2, "src-tauri/src/lib.rs");
    let usage = Node {
        id: NodeId(42),
        kind: NodeKind::FUNCTION,
        serialized_name: "tauri command get_snapshot (tauri command; confidence=heuristic)"
            .to_string(),
        qualified_name: Some("framework::tauri::command::get_snapshot".to_string()),
        canonical_id: Some("tauri:command:get_snapshot".to_string()),
        file_node_id: Some(NodeId(1)),
        start_line: Some(7),
        start_col: Some(1),
        ..Default::default()
    };
    let definition = Node {
        file_node_id: Some(NodeId(2)),
        start_line: Some(21),
        ..usage.clone()
    };

    storage.insert_nodes_batch(&[usage_file, definition_file, usage])?;
    assert_eq!(
        storage
            .get_node(NodeId(42))?
            .and_then(|node| node.file_node_id),
        Some(NodeId(1))
    );

    storage.flush_projection_batch(ProjectionBatch {
        files: &[],
        file_content_hashes: &[],
        nodes: &[definition],
        structural_text_units: &[],
        structural_text_projections: &[],
        structural_text_cache_writes: &[],
        edges: &[],
        occurrences: &[],
        component_access: &[],
        callable_projection_states: &[],
        file_errors: &[],
    })?;

    assert_eq!(
        storage
            .get_node(NodeId(42))?
            .and_then(|node| node.file_node_id),
        Some(NodeId(2))
    );

    Ok(())
}

#[test]
fn test_resolution_indexes_are_created() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    let mut node_stmt = storage.conn.prepare("PRAGMA index_list('node')")?;
    let node_indexes = node_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        node_indexes
            .iter()
            .any(|name| name == "idx_node_kind_serialized_name")
    );

    let mut edge_stmt = storage.conn.prepare("PRAGMA index_list('edge')")?;
    let edge_indexes = edge_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        edge_indexes
            .iter()
            .any(|name| name == "idx_edge_kind_resolved_target")
    );

    let mut callable_state_stmt = storage
        .conn
        .prepare("PRAGMA index_list('callable_projection_state')")?;
    let callable_state_indexes = callable_state_stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        callable_state_indexes
            .iter()
            .any(|name| name == "idx_callable_projection_state_file_node")
    );

    Ok(())
}

#[test]
fn test_index_artifact_cache_round_trip() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let payload = br#"{"cached":true}"#;

    storage.upsert_index_artifact_cache(Path::new("src/lib.rs"), "cache-key", payload)?;

    assert_eq!(
        storage.get_index_artifact_cache(Path::new("src/lib.rs"), "cache-key")?,
        Some(payload.to_vec())
    );
    assert_eq!(
        storage.get_index_artifact_cache(Path::new("src/lib.rs"), "other-key")?,
        None
    );

    Ok(())
}

#[test]
fn test_index_artifact_cache_batch_is_ordered_and_empty_is_noop() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let path = Path::new("src/lib.rs");
    let empty: [IndexArtifactCacheWrite<'_>; 0] = [];

    assert_eq!(storage.upsert_index_artifact_cache_batch(&empty)?, 0);
    assert_eq!(
        storage.upsert_index_artifact_cache_batch(&[
            IndexArtifactCacheWrite {
                path,
                cache_key: "first-key",
                artifact_blob: b"first",
            },
            IndexArtifactCacheWrite {
                path,
                cache_key: "last-key",
                artifact_blob: b"last",
            },
        ])?,
        2
    );

    assert_eq!(storage.get_index_artifact_cache(path, "first-key")?, None);
    assert_eq!(
        storage.get_index_artifact_cache(path, "last-key")?,
        Some(b"last".to_vec())
    );
    Ok(())
}

#[test]
fn test_index_artifact_cache_batch_rolls_back_every_row_on_failure() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let stable_path = Path::new("stable.rs");
    storage.upsert_index_artifact_cache(stable_path, "stable-key", b"stable")?;
    storage.get_connection().execute_batch(
        "CREATE TRIGGER reject_failed_artifact_cache_write
         BEFORE INSERT ON index_artifact_cache
         WHEN NEW.file_path = 'fail.rs'
         BEGIN
           SELECT RAISE(ABORT, 'forced artifact-cache batch failure');
         END;",
    )?;

    let error = storage
        .upsert_index_artifact_cache_batch(&[
            IndexArtifactCacheWrite {
                path: stable_path,
                cache_key: "replacement-key",
                artifact_blob: b"replacement",
            },
            IndexArtifactCacheWrite {
                path: Path::new("new.rs"),
                cache_key: "new-key",
                artifact_blob: b"new",
            },
            IndexArtifactCacheWrite {
                path: Path::new("fail.rs"),
                cache_key: "fail-key",
                artifact_blob: b"fail",
            },
        ])
        .expect_err("trigger must abort the artifact-cache batch");
    assert!(
        error
            .to_string()
            .contains("forced artifact-cache batch failure")
    );

    assert_eq!(
        storage.get_index_artifact_cache(stable_path, "stable-key")?,
        Some(b"stable".to_vec())
    );
    assert_eq!(
        storage.get_index_artifact_cache(stable_path, "replacement-key")?,
        None
    );
    assert_eq!(
        storage.get_index_artifact_cache(Path::new("new.rs"), "new-key")?,
        None
    );
    Ok(())
}

#[test]
fn test_index_artifact_cache_reader_observes_committed_batches_without_write_access()
-> Result<(), StorageError> {
    let dir = tempfile::tempdir().map_err(|error| StorageError::Other(error.to_string()))?;
    let database_path = dir.path().join("staged.sqlite");
    let storage = Storage::open_build(&database_path)?;
    let reader = storage
        .index_artifact_cache_reader()?
        .expect("file-backed storage must provide a cache reader");
    let path = Path::new("src/lib.rs");

    assert_eq!(reader.get(path, "first-key")?, None);
    storage.upsert_index_artifact_cache_batch(&[IndexArtifactCacheWrite {
        path,
        cache_key: "first-key",
        artifact_blob: b"first",
    }])?;
    assert_eq!(reader.get(path, "first-key")?, Some(b"first".to_vec()));

    storage.upsert_index_artifact_cache_batch(&[IndexArtifactCacheWrite {
        path,
        cache_key: "second-key",
        artifact_blob: b"second",
    }])?;
    assert_eq!(reader.get(path, "first-key")?, None);
    assert_eq!(reader.get(path, "second-key")?, Some(b"second".to_vec()));

    let query_only: i64 = reader
        .conn
        .query_row("PRAGMA query_only", [], |row| row.get(0))?;
    assert_eq!(query_only, 1);
    assert!(
        reader
            .conn
            .execute("DELETE FROM index_artifact_cache", [])
            .is_err(),
        "query-only reader must reject writes"
    );
    Ok(())
}

#[test]
fn structural_projection_cache_write_is_atomic_with_file_and_unit_rows() -> Result<(), StorageError>
{
    let mut storage = Storage::new_in_memory()?;
    storage.get_connection().execute_batch(
        "CREATE TRIGGER reject_structural_cache_write
         BEFORE INSERT ON structural_text_artifact_cache
         BEGIN
           SELECT RAISE(ABORT, 'forced structural cache failure');
         END;",
    )?;
    let source_hash = "a".repeat(64);
    let file = FileInfo {
        id: 70,
        path: PathBuf::from(".github/workflows/ci.yml"),
        language: "github_actions_workflow".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 2,
        file_role: FileRole::Source,
    };
    let nodes = [
        file_node(file.id, ".github/workflows/ci.yml"),
        Node {
            id: NodeId(71),
            kind: NodeKind::FUNCTION,
            serialized_name: "build".to_string(),
            canonical_id: Some("github-actions:job:build".to_string()),
            file_node_id: Some(NodeId(file.id)),
            start_line: Some(2),
            start_col: Some(3),
            end_line: Some(2),
            end_col: Some(7),
            ..Default::default()
        },
    ];
    let unit = StructuralTextUnit {
        node_id: NodeId(71),
        file_id: file.id,
        placement_id: "b".repeat(64),
        content_hash: "c".repeat(64),
        source_content_hash: source_hash.clone(),
        descriptor_version: STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer: "github_actions_workflow".to_string(),
        evidence_tier: "structural_text".to_string(),
        resolution: "source_range_only".to_string(),
        language: file.language.clone(),
        kind: NodeKind::FUNCTION,
        start_line: 2,
        start_col: 3,
        end_line: 2,
        end_col: 7,
        file_role: FileRole::Source,
    };
    let projection = StructuralTextProjection {
        file_id: file.id,
        source_content_hash: source_hash.clone(),
        descriptor_version: STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer: "github_actions_workflow".to_string(),
        language: file.language.clone(),
        file_role: FileRole::Source,
        unit_count: 1,
        unit_digest: structural_text_unit_digest(std::slice::from_ref(&unit)),
    };
    let error = storage
        .flush_projection_batch(ProjectionBatch {
            files: std::slice::from_ref(&file),
            file_content_hashes: &[FileContentHash {
                file_id: file.id,
                content_hash: source_hash,
            }],
            nodes: &nodes,
            structural_text_units: std::slice::from_ref(&unit),
            structural_text_projections: std::slice::from_ref(&projection),
            structural_text_cache_writes: &[StructuralTextArtifactCacheWrite {
                path: Path::new(".github/workflows/ci.yml"),
                file_id: file.id,
                cache_key: "v1:cache",
                artifact_blob: b"artifact",
            }],
            edges: &[],
            occurrences: &[],
            component_access: &[],
            callable_projection_states: &[],
            file_errors: &[],
        })
        .expect_err("trigger must abort the complete structural projection batch");
    assert!(
        error
            .to_string()
            .contains("forced structural cache failure")
    );
    assert!(storage.get_files()?.is_empty());
    assert!(storage.get_nodes()?.is_empty());
    assert_eq!(storage.get_structural_text_unit(NodeId(71))?, None);
    assert_eq!(
        storage.get_structural_text_artifact_cache(
            Path::new(".github/workflows/ci.yml"),
            "v1:cache"
        )?,
        None
    );
    Ok(())
}

#[test]
fn structural_publication_prunes_deleted_excluded_and_changed_cache_membership()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let current_hash = "a".repeat(64);
    let changed_hash = "b".repeat(64);
    let files = [
        FileInfo {
            id: 80,
            path: PathBuf::from("styles.css"),
            language: "css".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        },
        FileInfo {
            id: 81,
            path: PathBuf::from("changed.sql"),
            language: "sql".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        },
    ];
    let empty_digest = structural_text_unit_digest(&[]);
    let projections = [
        StructuralTextProjection {
            file_id: files[0].id,
            source_content_hash: current_hash.clone(),
            descriptor_version: STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
            producer: "css".to_string(),
            language: files[0].language.clone(),
            file_role: FileRole::Source,
            unit_count: 0,
            unit_digest: empty_digest.clone(),
        },
        StructuralTextProjection {
            file_id: files[1].id,
            source_content_hash: changed_hash.clone(),
            descriptor_version: STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
            producer: "sql".to_string(),
            language: files[1].language.clone(),
            file_role: FileRole::Source,
            unit_count: 0,
            unit_digest: empty_digest,
        },
    ];
    storage.flush_projection_batch(ProjectionBatch {
        files: &files,
        file_content_hashes: &[
            FileContentHash {
                file_id: files[0].id,
                content_hash: current_hash.clone(),
            },
            FileContentHash {
                file_id: files[1].id,
                content_hash: changed_hash,
            },
        ],
        nodes: &[
            file_node(files[0].id, "styles.css"),
            file_node(files[1].id, "changed.sql"),
        ],
        structural_text_units: &[],
        structural_text_projections: &projections,
        structural_text_cache_writes: &[StructuralTextArtifactCacheWrite {
            path: Path::new("styles.css"),
            file_id: files[0].id,
            cache_key: "v1:current",
            artifact_blob: b"current",
        }],
        edges: &[],
        occurrences: &[],
        component_access: &[],
        callable_projection_states: &[],
        file_errors: &[],
    })?;

    for (path, file_id, source_hash, producer, blob) in [
        (
            "changed.sql",
            81_i64,
            "c".repeat(64),
            "sql",
            b"changed".as_slice(),
        ),
        (
            "deleted.sql",
            82_i64,
            "d".repeat(64),
            "sql",
            b"deleted".as_slice(),
        ),
        (
            "newly-excluded.sql",
            83_i64,
            "e".repeat(64),
            "sql",
            b"excluded".as_slice(),
        ),
    ] {
        storage.get_connection().execute(
            "INSERT INTO structural_text_artifact_cache (
                file_path, file_id, cache_key, source_content_hash,
                descriptor_version, producer, artifact_digest, artifact_blob,
                updated_at_epoch_ms
             ) VALUES (?1, ?2, 'v1:stale', ?3, ?4, ?5, ?6, ?7, 1)",
            params![
                path,
                file_id,
                source_hash,
                STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION as i64,
                producer,
                format!("{:x}", Sha256::digest(blob)),
                blob,
            ],
        )?;
    }

    storage.publish_structural_text_unit_generation(&IndexPublicationRecord {
        generation: 1,
        generation_id: "generation-cache-membership".to_string(),
        run_id: "run-cache-membership".to_string(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    })?;

    let remaining = storage.get_connection().query_row(
        "SELECT COUNT(*) FROM structural_text_artifact_cache",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    assert_eq!(remaining, 1);
    assert_eq!(
        storage.get_structural_text_artifact_cache(Path::new("styles.css"), "v1:current")?,
        Some(b"current".to_vec())
    );
    for stale_path in ["changed.sql", "deleted.sql", "newly-excluded.sql"] {
        assert_eq!(
            storage.get_structural_text_artifact_cache(Path::new(stale_path), "v1:stale")?,
            None,
            "{stale_path} retained stale structural cache membership"
        );
    }
    Ok(())
}

#[test]
fn disposable_full_build_is_the_only_relaxed_sqlite_profile() -> Result<(), StorageError> {
    fn profile(storage: &Storage) -> Result<(String, i64, i64, i64), StorageError> {
        let connection = storage.get_connection();
        Ok((
            connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?,
            connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?,
            connection.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))?,
            connection.query_row("PRAGMA page_size", [], |row| row.get(0))?,
        ))
    }

    let dir = tempfile::tempdir().map_err(|error| StorageError::Other(error.to_string()))?;
    let live_path = dir.path().join("live.sqlite");
    let generic_build_path = dir.path().join("generic-build.sqlite");
    let disposable_path = dir.path().join("disposable.sqlite");

    let live = Storage::open(&live_path)?;
    let generic_build = Storage::open_build(&generic_build_path)?;
    let disposable = Storage::open_disposable_full_build(&disposable_path)?;
    let mut incremental_clone = crate::SnapshotStore::clone_live_to_staged(&live_path)?;

    for (name, storage) in [("live", &live), ("generic build", &generic_build)] {
        let (journal_mode, synchronous, _, _) = profile(storage)?;
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal", "{name}");
        assert_eq!(synchronous, 1, "{name} must retain synchronous=NORMAL");
    }
    let (journal_mode, synchronous, _, _) = profile(incremental_clone.store_mut())?;
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(
        synchronous, 1,
        "incremental clone must retain synchronous=NORMAL"
    );

    let (journal_mode, synchronous, checkpoint_pages, page_size) = profile(&disposable)?;
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(synchronous, 0);
    assert_eq!(
        checkpoint_pages,
        (DISPOSABLE_FULL_BUILD_WAL_AUTOCHECKPOINT_BYTES as i64 + page_size - 1) / page_size
    );
    assert!(checkpoint_pages > 0);
    Ok(())
}

#[test]
fn test_resolution_support_snapshot_round_trip_and_invalidation() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let payload = br#"{"support":1}"#;

    assert!(!storage.has_ready_resolution_support_snapshot(1)?);

    storage.put_resolution_support_snapshot(1, payload)?;

    assert!(storage.has_ready_resolution_support_snapshot(1)?);
    assert_eq!(
        storage.get_resolution_support_snapshot(1)?,
        Some(payload.to_vec())
    );

    storage.invalidate_resolution_support_snapshot()?;

    assert!(!storage.has_ready_resolution_support_snapshot(1)?);
    assert_eq!(storage.get_resolution_support_snapshot(1)?, None);

    Ok(())
}

#[test]
fn test_resolution_support_snapshot_read_classifies_runtime_capacity() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    storage.put_resolution_support_snapshot(1, &vec![b'x'; 2_048])?;

    let previous_limit = storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_LENGTH, 1_024)?;
    assert!(matches!(
        storage.get_resolution_support_snapshot(1),
        Err(StorageError::ResolutionSupportSnapshotTooBig)
    ));
    storage.invalidate_resolution_support_snapshot()?;
    storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_LENGTH, previous_limit)?;
    assert!(!storage.has_ready_resolution_support_snapshot(1)?);

    Ok(())
}

#[test]
fn test_resolution_support_snapshot_write_classifies_runtime_row_capacity()
-> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let snapshot_blob = vec![b'x'; 1_024];
    let previous_limit = storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_LENGTH, snapshot_blob.len() as i32)?;

    assert!(matches!(
        storage.put_resolution_support_snapshot(1, &snapshot_blob),
        Err(StorageError::ResolutionSupportSnapshotTooBig)
    ));
    storage.invalidate_resolution_support_snapshot()?;

    storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_LENGTH, previous_limit)?;
    assert!(!storage.has_ready_resolution_support_snapshot(1)?);

    Ok(())
}

#[test]
fn test_update_file_metadata_preserves_resolution_support_snapshot() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    storage.insert_file(&FileInfo {
        id: 11,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;
    storage.put_resolution_support_snapshot(1, br#"{"hot":true}"#)?;

    storage.update_file_metadata(
        &FileInfo {
            id: 11,
            path: PathBuf::from("src/lib.rs"),
            language: "rust".to_string(),
            modification_time: 2,
            indexed: true,
            complete: true,
            line_count: 10,
            file_role: FileRole::Source,
        },
        None,
    )?;

    assert!(storage.has_ready_resolution_support_snapshot(1)?);
    Ok(())
}

#[test]
fn projection_batch_round_trips_and_clears_file_content_hash() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let files = [FileInfo {
        id: 17,
        path: PathBuf::from("src/snapshot.rs"),
        language: "rust".to_string(),
        modification_time: 9,
        indexed: true,
        complete: true,
        line_count: 4,
        file_role: FileRole::Source,
    }];
    let hashes = [FileContentHash {
        file_id: 17,
        content_hash: "sha256:first".to_string(),
    }];

    storage.flush_projection_batch(ProjectionBatch {
        files: &files,
        file_content_hashes: &hashes,
        nodes: &[],
        structural_text_units: &[],
        structural_text_projections: &[],
        structural_text_cache_writes: &[],
        edges: &[],
        occurrences: &[],
        component_access: &[],
        callable_projection_states: &[],
        file_errors: &[],
    })?;
    assert_eq!(
        storage.get_file_content_hash(17)?.as_deref(),
        Some("sha256:first")
    );

    storage.flush_projection_batch(ProjectionBatch {
        files: &files,
        file_content_hashes: &[],
        nodes: &[],
        structural_text_units: &[],
        structural_text_projections: &[],
        structural_text_cache_writes: &[],
        edges: &[],
        occurrences: &[],
        component_access: &[],
        callable_projection_states: &[],
        file_errors: &[],
    })?;
    assert_eq!(storage.get_file_content_hash(17)?, None);
    Ok(())
}

fn flush_projection_persistence_fixture(
    storage: &mut Storage,
) -> Result<ProjectionFlushBreakdown, StorageError> {
    let source_hash = "a".repeat(64);
    let files = [FileInfo {
        id: 1,
        path: PathBuf::from("src/a.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 3,
        file_role: FileRole::Source,
    }];
    let file_content_hashes = [FileContentHash {
        file_id: 1,
        content_hash: source_hash.clone(),
    }];
    let nodes = [
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "src/a.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "a::run".to_string(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(2),
            start_col: Some(1),
            end_line: Some(3),
            end_col: Some(2),
            ..Default::default()
        },
    ];
    let structural_units = [StructuralTextUnit {
        node_id: NodeId(2),
        file_id: 1,
        placement_id: "p".repeat(64),
        content_hash: "b".repeat(64),
        source_content_hash: source_hash.clone(),
        descriptor_version: STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer: "collector".to_string(),
        evidence_tier: "structural_text".to_string(),
        resolution: "source_range_only".to_string(),
        language: "rust".to_string(),
        kind: NodeKind::FUNCTION,
        start_line: 2,
        start_col: 1,
        end_line: 3,
        end_col: 2,
        file_role: FileRole::Source,
    }];
    let structural_projections = [StructuralTextProjection {
        file_id: 1,
        source_content_hash: source_hash,
        descriptor_version: STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer: "collector".to_string(),
        language: "rust".to_string(),
        file_role: FileRole::Source,
        unit_count: 1,
        unit_digest: structural_text_unit_digest(&structural_units),
    }];
    let structural_cache_writes = [StructuralTextArtifactCacheWrite {
        path: &files[0].path,
        file_id: 1,
        cache_key: "v1:test",
        artifact_blob: b"{}",
    }];
    let edges = [Edge {
        id: EdgeId(10),
        source: NodeId(1),
        target: NodeId(2),
        kind: EdgeKind::MEMBER,
        file_node_id: Some(NodeId(1)),
        line: Some(2),
        resolved_source: None,
        resolved_target: None,
        confidence: None,
        certainty: None,
        callsite_identity: None,
        candidate_targets: Vec::new(),
    }];
    let occurrences = [Occurrence {
        element_id: 2,
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: NodeId(1),
            start_line: 2,
            start_col: 1,
            end_line: 3,
            end_col: 2,
        },
    }];
    let component_access = [(NodeId(2), AccessKind::Public)];
    let callable_projection_states = [CallableProjectionState {
        file_id: 1,
        symbol_key: "a::run".to_string(),
        node_id: NodeId(2),
        signature_hash: 11,
        body_hash: 12,
        start_line: 2,
        end_line: 3,
    }];
    let file_errors = [ErrorInfo {
        message: "warn".to_string(),
        file_id: Some(NodeId(1)),
        line: Some(2),
        column: Some(1),
        is_fatal: false,
        index_step: IndexStep::Indexing,
        coverage_reason: None,
    }];

    storage.flush_projection_batch(ProjectionBatch {
        files: &files,
        file_content_hashes: &file_content_hashes,
        nodes: &nodes,
        structural_text_units: &structural_units,
        structural_text_projections: &structural_projections,
        structural_text_cache_writes: &structural_cache_writes,
        edges: &edges,
        occurrences: &occurrences,
        component_access: &component_access,
        callable_projection_states: &callable_projection_states,
        file_errors: &file_errors,
    })
}

#[test]
fn projection_batch_reports_exact_shape_and_commits_once() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let commits = Arc::new(AtomicUsize::new(0));
    let observed_commits = Arc::clone(&commits);
    storage.conn.commit_hook(Some(move || {
        observed_commits.fetch_add(1, AtomicOrdering::SeqCst);
        false
    }))?;

    let breakdown = flush_projection_persistence_fixture(&mut storage)?;
    storage.conn.commit_hook(None::<fn() -> bool>)?;

    assert_eq!(commits.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(breakdown.persistence.transactions, 1);
    assert_eq!(breakdown.persistence.files.row_attempts, 1);
    assert_eq!(breakdown.persistence.nodes.row_attempts, 2);
    assert_eq!(breakdown.persistence.structural_text.row_attempts, 5);
    assert_eq!(breakdown.persistence.edges.row_attempts, 1);
    assert_eq!(breakdown.persistence.occurrences.row_attempts, 1);
    assert_eq!(breakdown.persistence.component_access.row_attempts, 1);
    assert_eq!(breakdown.persistence.callable_projection.row_attempts, 1);
    assert_eq!(breakdown.persistence.file_errors.row_attempts, 2);
    assert_eq!(breakdown.persistence.dirty_state.row_attempts, 4);
    assert_eq!(breakdown.persistence.row_attempts(), 18);
    assert_eq!(breakdown.persistence.statement_executions(), 18);
    assert_eq!(breakdown.persistence.files.bound_bytes, 122);
    assert_eq!(breakdown.persistence.file_errors.bound_bytes, 52);
    assert_eq!(breakdown.persistence.dirty_state.bound_bytes, 48);
    assert!(breakdown.persistence.bound_bytes() > 1_000);
    assert!(breakdown.persistence.transaction_wall_ms >= breakdown.persistence.commit_ms);

    let stored_errors = storage.get_errors(None)?;
    assert_eq!(stored_errors.len(), 1);
    assert_eq!(stored_errors[0].message, "warn");
    assert_eq!(
        storage
            .get_grounding_snapshot_metadata()?
            .expect("snapshot metadata")
            .summary_state,
        GroundingSnapshotState::Dirty
    );
    assert!(!storage.has_ready_resolution_support_snapshot(1)?);
    Ok(())
}

fn seed_ready_projection_snapshots(storage: &Storage) -> Result<(), StorageError> {
    storage.write_grounding_snapshot_states(
        GroundingSnapshotState::Ready,
        GroundingSnapshotState::Ready,
        Some(1),
        Some(1),
    )?;
    storage.put_resolution_support_snapshot(1, br#"{"ready":true}"#)
}

#[test]
fn projection_batch_family_and_commit_failures_roll_back_everything() -> Result<(), StorageError> {
    let denied_operations = [
        ("file", false),
        ("node", false),
        ("structural_text_unit", false),
        ("edge", false),
        ("occurrence", false),
        ("component_access", false),
        ("callable_projection_state", false),
        ("error", false),
        ("grounding_snapshot_meta", true),
        ("resolution_support_snapshot", false),
    ];

    for (table, deny_update) in denied_operations {
        let mut storage = Storage::new_in_memory()?;
        seed_ready_projection_snapshots(&storage)?;
        let denied_table = table.to_string();
        storage
            .conn
            .authorizer(Some(move |context: AuthContext<'_>| {
                let denied = match context.action {
                    AuthAction::Insert { table_name } => !deny_update && table_name == denied_table,
                    AuthAction::Update { table_name, .. } => {
                        deny_update && table_name == denied_table
                    }
                    _ => false,
                };
                if denied {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }))?;

        assert!(
            flush_projection_persistence_fixture(&mut storage).is_err(),
            "{table} denial should reject the complete projection batch"
        );
        storage
            .conn
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)?;
        assert!(storage.get_files()?.is_empty(), "{table} left file rows");
        assert!(storage.get_nodes()?.is_empty(), "{table} left node rows");
        assert!(storage.get_edges()?.is_empty(), "{table} left edge rows");
        assert!(storage.get_errors(None)?.is_empty(), "{table} left errors");
        assert!(
            storage.has_ready_grounding_snapshots()?,
            "{table} dirtied grounding state outside the transaction"
        );
        assert!(
            storage.has_ready_resolution_support_snapshot(1)?,
            "{table} dirtied resolution state outside the transaction"
        );
    }

    let mut storage = Storage::new_in_memory()?;
    seed_ready_projection_snapshots(&storage)?;
    storage.conn.commit_hook(Some(|| true))?;
    assert!(flush_projection_persistence_fixture(&mut storage).is_err());
    storage.conn.commit_hook(None::<fn() -> bool>)?;
    assert!(storage.get_files()?.is_empty());
    assert!(storage.get_nodes()?.is_empty());
    assert!(storage.get_edges()?.is_empty());
    assert!(storage.get_errors(None)?.is_empty());
    assert!(storage.has_ready_grounding_snapshots()?);
    assert!(storage.has_ready_resolution_support_snapshot(1)?);
    Ok(())
}

#[test]
fn test_present_kind_queries() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(1),
            kind: NodeKind::CLASS,
            serialized_name: "A".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::METHOD,
            serialized_name: "A::run".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(2),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
    ])?;

    let node_kinds = storage.get_present_node_kinds()?;
    let edge_kinds = storage.get_present_edge_kinds()?;
    assert!(node_kinds.contains(&NodeKind::CLASS));
    assert!(node_kinds.contains(&NodeKind::METHOD));
    assert!(edge_kinds.contains(&EdgeKind::MEMBER));
    assert!(edge_kinds.contains(&EdgeKind::CALL));
    Ok(())
}

#[test]
fn test_component_access_round_trip() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(41),
            kind: NodeKind::METHOD,
            serialized_name: "run".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(42),
            kind: NodeKind::FIELD,
            serialized_name: "state".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_component_access_batch(&[
        (NodeId(41), AccessKind::Protected),
        (NodeId(42), AccessKind::Private),
    ])?;

    assert_eq!(
        storage.get_component_access(NodeId(41))?,
        Some(AccessKind::Protected)
    );
    let map = storage.get_component_access_map_for_nodes(&[NodeId(41), NodeId(42)])?;
    assert_eq!(map.get(&NodeId(42)).copied(), Some(AccessKind::Private));
    Ok(())
}

#[test]
fn component_access_lookup_batches_at_runtime_bind_limit() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(41),
            kind: NodeKind::METHOD,
            serialized_name: "run".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(42),
            kind: NodeKind::FIELD,
            serialized_name: "state".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(43),
            kind: NodeKind::METHOD,
            serialized_name: "reset".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_component_access_batch(&[
        (NodeId(41), AccessKind::Protected),
        (NodeId(42), AccessKind::Private),
        (NodeId(43), AccessKind::Public),
    ])?;

    let previous_limit = storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 2)?;
    assert!(previous_limit >= 2);

    let map = storage.get_component_access_map_for_nodes(&[
        NodeId(99),
        NodeId(43),
        NodeId(41),
        NodeId(42),
        NodeId(41),
        NodeId(100),
    ])?;
    assert_eq!(map.len(), 3);
    assert_eq!(map.get(&NodeId(41)), Some(&AccessKind::Protected));
    assert_eq!(map.get(&NodeId(42)), Some(&AccessKind::Private));
    assert_eq!(map.get(&NodeId(43)), Some(&AccessKind::Public));
    storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, previous_limit)?;
    Ok(())
}

#[test]
fn component_access_lookup_rejects_zero_bind_limit() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    storage
        .get_connection()
        .set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 0)?;

    let error = storage
        .get_component_access_map_for_nodes(&[NodeId(41)])
        .expect_err("component-access lookup must reject a zero-variable runtime limit");
    assert!(
        error
            .to_string()
            .contains("cannot support component-access lookup"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[test]
fn test_symbol_search_doc_contract_mismatch_detection() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[Node {
        id: NodeId(500),
        kind: NodeKind::FUNCTION,
        serialized_name: "do_work".to_string(),
        ..Default::default()
    }])?;
    storage.upsert_symbol_search_docs_batch(&[SymbolSearchDoc {
        node_id: NodeId(500),
        file_node_id: None,
        kind: NodeKind::FUNCTION,
        display_name: "do_work".to_string(),
        qualified_name: Some("pkg::do_work".to_string()),
        file_path: Some("src/lib.rs".to_string()),
        start_line: Some(12),
        doc_text: "semantic_doc_version: 6\nsymbol: do_work".to_string(),
        doc_version: 6,
        doc_hash: "symbol-search-hash-500".to_string(),
        policy_version: "graph_first_v1".to_string(),
        source_provenance: "extracted".to_string(),
        updated_at_epoch_ms: 123,
    }])?;

    assert!(!storage.has_symbol_search_doc_contract_mismatch(6, "graph_first_v1")?);
    assert!(storage.has_symbol_search_doc_contract_mismatch(5, "graph_first_v1")?);
    assert!(storage.has_symbol_search_doc_contract_mismatch(6, "graph_first_v2")?);
    Ok(())
}

fn dense_anchor(node_id: i64, file_node_id: Option<i64>, source: &str) -> DenseAnchorInput {
    DenseAnchorInput {
        node_id: NodeId(node_id),
        file_node_id: file_node_id.map(NodeId),
        kind: NodeKind::FUNCTION,
        display_name: format!("function_{node_id}"),
        qualified_name: Some(format!("pkg::function_{node_id}")),
        file_path: Some("src/lib.rs".to_string()),
        start_line: Some(node_id as u32),
        end_line: Some(node_id as u32 + 2),
        file_role: FileRole::Source,
        source_provenance: "parser".to_string(),
        text: format!("function function_{node_id}"),
        document_hash: format!("hash-{node_id}"),
        selection_reason: "public_symbol".to_string(),
        policy_version: "dense-anchor-v1".to_string(),
        source_identity: source.to_string(),
        updated_at_epoch_ms: 123,
    }
}

#[test]
fn dense_anchor_inputs_round_trip_prune_and_copy_with_node_ownership() -> Result<(), StorageError> {
    let source_path = unique_temp_db_path("dense-anchor-source");
    let destination_path = unique_temp_db_path("dense-anchor-destination");
    {
        let mut source = Storage::open(&source_path)?;
        source.insert_nodes_batch(&[
            file_node(700, "src/lib.rs"),
            Node {
                id: NodeId(701),
                kind: NodeKind::FUNCTION,
                serialized_name: "function_701".to_string(),
                file_node_id: Some(NodeId(700)),
                ..Default::default()
            },
            Node {
                id: NodeId(702),
                kind: NodeKind::FUNCTION,
                serialized_name: "function_702".to_string(),
                file_node_id: Some(NodeId(700)),
                ..Default::default()
            },
        ])?;
        source.upsert_dense_anchor_inputs_batch(&[
            dense_anchor(701, Some(700), "core:g1:r1"),
            dense_anchor(702, Some(700), "core:g1:r1"),
        ])?;

        let rows = source.get_dense_anchor_inputs_batch_after(None, 10)?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], dense_anchor(701, Some(700), "core:g1:r1"));
        assert_eq!(
            source.prune_dense_anchor_inputs_to_node_ids(&[NodeId(702)])?,
            1
        );
        assert_eq!(source.get_dense_anchor_input_reuse_metadata()?.len(), 1);
    }

    {
        let mut destination = Storage::open(&destination_path)?;
        destination.insert_nodes_batch(&[
            file_node(700, "src/lib.rs"),
            Node {
                id: NodeId(702),
                kind: NodeKind::FUNCTION,
                serialized_name: "function_702".to_string(),
                file_node_id: Some(NodeId(700)),
                ..Default::default()
            },
        ])?;
        assert_eq!(destination.copy_dense_anchor_inputs_from(&source_path)?, 1);
        assert_eq!(
            destination.get_dense_anchor_inputs_batch_after(None, 10)?,
            vec![dense_anchor(702, Some(700), "core:g1:r1")]
        );
    }

    let _ = cleanup_sqlite_sidecars(&source_path);
    let _ = cleanup_sqlite_sidecars(&destination_path);
    Ok(())
}

#[test]
fn dense_anchor_manifest_rebinds_carry_forward_and_detects_mutation() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        file_node(700, "src/lib.rs"),
        Node {
            id: NodeId(701),
            kind: NodeKind::FUNCTION,
            serialized_name: "function_701".to_string(),
            file_node_id: Some(NodeId(700)),
            ..Default::default()
        },
    ])?;
    storage.upsert_dense_anchor_inputs_batch(&[dense_anchor(
        701,
        Some(700),
        "core:previous:run",
    )])?;
    let first_publication = IndexPublicationRecord {
        generation: 1,
        generation_id: "generation-1".into(),
        run_id: "run-1".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    };
    let first = storage.publish_dense_anchor_generation(&first_publication, "dense-anchor-v1")?;
    storage.put_index_publication(&first_publication)?;
    assert_eq!(
        storage.validate_dense_anchor_publication(&first_publication)?,
        first
    );
    assert_eq!(first.anchor_count, 1);
    assert_eq!(first.anchor_digest.len(), 64);
    assert_eq!(
        storage.get_dense_anchor_inputs_batch_after(None, 10)?[0].source_identity,
        "core:generation-1:run-1"
    );

    let second_publication = IndexPublicationRecord {
        generation: 2,
        generation_id: "generation-2".into(),
        run_id: "run-2".into(),
        mode: IndexPublicationMode::Incremental,
        published_at_epoch_ms: 2,
    };
    let second = storage.publish_dense_anchor_generation(&second_publication, "dense-anchor-v1")?;
    assert_eq!(second.anchor_digest, first.anchor_digest);
    assert_eq!(
        storage.get_dense_anchor_inputs_batch_after(None, 10)?[0].source_identity,
        "core:generation-2:run-2"
    );

    let mut changed = storage.get_dense_anchor_inputs_batch_after(None, 10)?;
    changed[0].text.push_str(" changed");
    storage.upsert_dense_anchor_inputs_batch(&changed)?;
    assert!(storage.get_dense_anchor_publication_manifest()?.is_none());
    Ok(())
}

#[test]
fn schema_22_migrates_to_dense_anchor_inputs_without_synthesizing_rows() -> Result<(), StorageError>
{
    let path = unique_temp_db_path("dense-anchor-v23-migration");
    {
        let storage = Storage::open(&path)?;
        storage.get_connection().execute_batch(
            "DROP TABLE dense_anchor_publication;
                 DROP TABLE dense_anchor_input;",
        )?;
        storage.set_schema_version(22)?;
    }

    let storage = Storage::open(&path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    assert!(
        storage
            .get_dense_anchor_inputs_batch_after(None, 10)?
            .is_empty()
    );
    assert!(storage.get_dense_anchor_publication_manifest()?.is_none());
    let indexes = storage
        .get_connection()
        .prepare("PRAGMA index_list(dense_anchor_input)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        indexes
            .iter()
            .any(|name| name == "idx_dense_anchor_input_reuse")
    );

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&path);
    Ok(())
}

#[test]
fn test_llm_symbol_doc_round_trip() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "do_work".to_string(),
        ..Default::default()
    }])?;

    storage.upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
        node_id: NodeId(501),
        file_node_id: None,
        kind: NodeKind::FUNCTION,
        display_name: "pkg::do_work".to_string(),
        qualified_name: Some("pkg::do_work".to_string()),
        file_path: Some("src/lib.rs".to_string()),
        start_line: Some(12),
        doc_text: "function pkg::do_work in src/lib.rs line 12".to_string(),
        doc_version: 2,
        doc_hash: "semantic-hash-501".to_string(),
        embedding_profile: None,
        embedding_model: "local-hash-384".to_string(),
        embedding_backend: None,
        embedding_dim: 384,
        doc_shape: None,
        semantic_policy_version: Some("graph_first_v1".to_string()),
        dense_reason: Some("public_api".to_string()),
        embedding: vec![0.25_f32; 384],
        updated_at_epoch_ms: 123,
    }])?;

    let docs = storage.get_llm_symbol_docs_by_node_ids(&[NodeId(501)])?;
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].node_id, NodeId(501));
    assert_eq!(docs[0].doc_version, 2);
    assert_eq!(docs[0].doc_hash, "semantic-hash-501");
    assert_eq!(docs[0].embedding_dim, 384);
    assert_eq!(docs[0].embedding.len(), 384);
    Ok(())
}

#[test]
fn test_llm_symbol_doc_stats_report_contract_metadata() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "do_work".to_string(),
        ..Default::default()
    }])?;

    storage.upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
        node_id: NodeId(501),
        file_node_id: None,
        kind: NodeKind::FUNCTION,
        display_name: "pkg::do_work".to_string(),
        qualified_name: Some("pkg::do_work".to_string()),
        file_path: Some("src/lib.rs".to_string()),
        start_line: Some(12),
        doc_text: "semantic_doc_version: 2\nsymbol_kind: FUNCTION\nname: pkg::do_work".to_string(),
        doc_version: 2,
        doc_hash: "semantic-hash-501".to_string(),
        embedding_profile: Some("coderank-embed".to_string()),
        embedding_model: "per-user-server:coderank-embed:q8_0".to_string(),
        embedding_backend: Some("per_user_server".to_string()),
        embedding_dim: 768,
        doc_shape: Some("semantic_doc_version=2;alias_mode=alias_variant".to_string()),
        semantic_policy_version: Some("graph_first_v1".to_string()),
        dense_reason: Some("public_api".to_string()),
        embedding: vec![0.25_f32; 4],
        updated_at_epoch_ms: 123,
    }])?;

    let stats = storage.get_llm_symbol_doc_stats()?;
    let stored_contract =
        serde_json::to_value(&stats).expect("serialize stored semantic doc stats");
    for field in ["doc_count", "cache_key", "dimension", "doc_shape"] {
        assert!(
            stored_contract.get(field).is_some(),
            "stored semantic-doc stats should report `{field}` for reuse/debug diagnostics"
        );
    }

    Ok(())
}

#[test]
fn test_llm_symbol_doc_stats_treats_legacy_null_contract_metadata_as_mixed()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(501),
            kind: NodeKind::FUNCTION,
            serialized_name: "legacy_work".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(502),
            kind: NodeKind::FUNCTION,
            serialized_name: "fresh_work".to_string(),
            ..Default::default()
        },
    ])?;

    storage.upsert_llm_symbol_docs_batch(&[
        LlmSymbolDoc {
            node_id: NodeId(501),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "legacy_work".to_string(),
            qualified_name: None,
            file_path: Some("src/lib.rs".to_string()),
            start_line: Some(12),
            doc_text: "legacy semantic doc".to_string(),
            doc_version: 4,
            doc_hash: "legacy-hash".to_string(),
            embedding_profile: None,
            embedding_model: "same-cache-key".to_string(),
            embedding_backend: None,
            embedding_dim: 384,
            doc_shape: None,
            semantic_policy_version: None,
            dense_reason: None,
            embedding: vec![0.25_f32; 4],
            updated_at_epoch_ms: 123,
        },
        LlmSymbolDoc {
            node_id: NodeId(502),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "fresh_work".to_string(),
            qualified_name: None,
            file_path: Some("src/lib.rs".to_string()),
            start_line: Some(24),
            doc_text: "fresh semantic doc".to_string(),
            doc_version: 4,
            doc_hash: "fresh-hash".to_string(),
            embedding_profile: Some("bge-small-en-v1.5".to_string()),
            embedding_model: "same-cache-key".to_string(),
            embedding_backend: Some("hash".to_string()),
            embedding_dim: 384,
            doc_shape: Some("semantic_doc_version=4;scope=durable_symbols".to_string()),
            semantic_policy_version: Some("graph_first_v1".to_string()),
            dense_reason: Some("public_api".to_string()),
            embedding: vec![0.5_f32; 4],
            updated_at_epoch_ms: 456,
        },
    ])?;

    let stats = storage.get_llm_symbol_doc_stats()?;

    assert_eq!(stats.embedding_model.as_deref(), Some("same-cache-key"));
    assert!(stats.mixed_embedding_profiles);
    assert!(stats.mixed_embedding_backends);
    assert!(stats.mixed_doc_shapes);
    assert!(!stats.mixed_embedding_models);
    assert!(!stats.mixed_dimensions);
    assert!(!stats.mixed_doc_versions);
    Ok(())
}

#[test]
fn test_symbol_summary_uses_current_content_hash() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "do_work".to_string(),
        ..Default::default()
    }])?;
    let doc = LlmSymbolDoc {
        node_id: NodeId(501),
        file_node_id: None,
        kind: NodeKind::FUNCTION,
        display_name: "pkg::do_work".to_string(),
        qualified_name: Some("pkg::do_work".to_string()),
        file_path: Some("src/lib.rs".to_string()),
        start_line: Some(12),
        doc_text: "function pkg::do_work in src/lib.rs line 12".to_string(),
        doc_version: 2,
        doc_hash: "semantic-hash-501".to_string(),
        embedding_profile: None,
        embedding_model: "local-hash-384".to_string(),
        embedding_backend: None,
        embedding_dim: 384,
        doc_shape: None,
        semantic_policy_version: Some("graph_first_v1".to_string()),
        dense_reason: Some("public_api".to_string()),
        embedding: vec![0.25_f32; 384],
        updated_at_epoch_ms: 123,
    };

    storage.upsert_llm_symbol_docs_batch(std::slice::from_ref(&doc))?;
    storage.upsert_symbol_summaries_batch(&[SymbolSummaryRecord {
        node_id: NodeId(501),
        content_hash: "semantic-hash-501".to_string(),
        summary: "do_work coordinates the package work.".to_string(),
        model: "test-model".to_string(),
        updated_at_epoch_ms: 456,
    }])?;

    let summaries = storage.get_current_symbol_summaries_by_node_ids(&[NodeId(501)])?;
    assert_eq!(
        summaries
            .get(&NodeId(501))
            .map(|record| record.summary.as_str()),
        Some("do_work coordinates the package work.")
    );

    let changed_doc = LlmSymbolDoc {
        doc_hash: "semantic-hash-501-changed".to_string(),
        ..doc
    };
    storage.upsert_llm_symbol_docs_batch(&[changed_doc])?;
    assert!(
        storage
            .get_current_symbol_summaries_by_node_ids(&[NodeId(501)])?
            .is_empty(),
        "summary should not be returned once the symbol doc hash changes"
    );
    Ok(())
}

#[test]
fn test_llm_symbol_doc_copy_forward_preserves_reuse_metadata() -> Result<(), StorageError> {
    let live_path = unique_temp_db_path("llm-copy-source");
    let _ = cleanup_sqlite_sidecars(&live_path);

    {
        let mut live = Storage::open(&live_path)?;
        live.insert_nodes_batch(&[Node {
            id: NodeId(501),
            kind: NodeKind::FUNCTION,
            serialized_name: "do_work".to_string(),
            ..Default::default()
        }])?;
        live.upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
            node_id: NodeId(501),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: "pkg::do_work".to_string(),
            qualified_name: Some("pkg::do_work".to_string()),
            file_path: Some("src/lib.rs".to_string()),
            start_line: Some(12),
            doc_text: "function pkg::do_work in src/lib.rs line 12".to_string(),
            doc_version: 2,
            doc_hash: "semantic-hash-501".to_string(),
            embedding_profile: Some("bge-small-en-v1.5".to_string()),
            embedding_model: "local-hash-384".to_string(),
            embedding_backend: Some("hash".to_string()),
            embedding_dim: 384,
            doc_shape: Some("semantic_doc_version=2".to_string()),
            semantic_policy_version: Some("graph_first_v1".to_string()),
            dense_reason: Some("public_api".to_string()),
            embedding: vec![0.25_f32; 384],
            updated_at_epoch_ms: 123,
        }])?;
    }

    let mut staged = Storage::new_in_memory()?;
    staged.insert_nodes_batch(&[Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "do_work".to_string(),
        ..Default::default()
    }])?;

    assert_eq!(staged.copy_llm_symbol_docs_from(&live_path)?, 1);
    let metadata = staged.get_llm_symbol_doc_reuse_metadata()?;
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].node_id, NodeId(501));
    assert_eq!(metadata[0].doc_version, 2);
    assert_eq!(metadata[0].doc_hash, "semantic-hash-501");
    assert_eq!(
        metadata[0].embedding_profile.as_deref(),
        Some("bge-small-en-v1.5")
    );
    assert_eq!(metadata[0].embedding_model, "local-hash-384");
    assert_eq!(metadata[0].embedding_backend.as_deref(), Some("hash"));
    assert_eq!(metadata[0].embedding_dim, 384);
    assert_eq!(
        metadata[0].doc_shape.as_deref(),
        Some("semantic_doc_version=2")
    );

    assert_eq!(staged.prune_llm_symbol_docs_to_node_ids(&[NodeId(501)])?, 0);
    assert_eq!(staged.prune_llm_symbol_docs_to_node_ids(&[])?, 1);
    assert!(staged.get_all_llm_symbol_docs()?.is_empty());

    cleanup_sqlite_sidecars(&live_path)?;
    Ok(())
}

#[test]
fn test_search_symbol_projection_round_trip_and_backfill() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(699),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(700),
            kind: NodeKind::FUNCTION,
            serialized_name: "short_name".to_string(),
            qualified_name: Some("pkg::short_name".to_string()),
            file_node_id: Some(NodeId(699)),
            start_line: Some(10),
            end_line: Some(12),
            ..Default::default()
        },
        Node {
            id: NodeId(701),
            kind: NodeKind::METHOD,
            serialized_name: "secondary".to_string(),
            file_node_id: Some(NodeId(699)),
            ..Default::default()
        },
    ])?;

    storage.upsert_search_symbol_projection_batch(&[
        SearchSymbolProjection {
            node_id: NodeId(700),
            display_name: "pkg::short_name".to_string(),
        },
        SearchSymbolProjection {
            node_id: NodeId(701),
            display_name: "secondary".to_string(),
        },
    ])?;
    assert_eq!(storage.get_search_symbol_projection_count()?, 2);
    let projection = storage.get_search_symbol_projection_batch_after(None, 10)?;
    assert_eq!(projection.len(), 2);
    assert_eq!(projection[0].display_name, "pkg::short_name");
    let details = storage.get_search_symbol_projection_detail_batch_after(None, 10)?;
    assert_eq!(details.len(), 2);
    assert_eq!(details[0].file_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(details[0].start_line, Some(10));
    assert_eq!(details[0].end_line, Some(12));

    storage.clear_search_symbol_projection()?;
    assert_eq!(storage.get_search_symbol_projection_count()?, 0);

    let rebuilt = storage.rebuild_search_symbol_projection_from_node_table()?;
    assert_eq!(rebuilt, 3);
    let projection = storage.get_search_symbol_projection_batch_after(None, 10)?;
    assert_eq!(projection.len(), 3);
    assert_eq!(projection[0].display_name, "src/lib.rs");
    assert_eq!(projection[1].display_name, "pkg::short_name");
    assert_eq!(projection[2].display_name, "secondary");
    Ok(())
}

#[test]
fn canonical_search_symbols_page_node_table_independently_of_projection() -> Result<(), StorageError>
{
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(100),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(30),
            kind: NodeKind::METHOD,
            serialized_name: "whitespace_fallback".to_string(),
            qualified_name: Some("   ".to_string()),
            file_node_id: Some(NodeId(100)),
            ..Default::default()
        },
        Node {
            id: NodeId(10),
            kind: NodeKind::FUNCTION,
            serialized_name: "qualified_fallback".to_string(),
            qualified_name: Some("pkg::qualified".to_string()),
            file_node_id: Some(NodeId(100)),
            start_line: Some(7),
            end_line: Some(11),
            ..Default::default()
        },
        Node {
            id: NodeId(20),
            kind: NodeKind::CLASS,
            serialized_name: "empty_fallback".to_string(),
            qualified_name: Some(String::new()),
            file_node_id: Some(NodeId(100)),
            ..Default::default()
        },
    ])?;
    storage.upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
        node_id: NodeId(10),
        display_name: "stale_projection_name".to_string(),
    }])?;

    assert_eq!(storage.get_canonical_search_symbol_count()?, 4);
    let first_page = storage.get_canonical_search_symbol_batch_after(None, 2)?;
    assert_eq!(
        first_page,
        vec![
            SearchSymbolProjection {
                node_id: NodeId(10),
                display_name: "pkg::qualified".to_string(),
            },
            SearchSymbolProjection {
                node_id: NodeId(20),
                display_name: "empty_fallback".to_string(),
            },
        ]
    );
    let second_page = storage.get_canonical_search_symbol_batch_after(Some(NodeId(20)), 2)?;
    assert_eq!(
        second_page,
        vec![
            SearchSymbolProjection {
                node_id: NodeId(30),
                display_name: "whitespace_fallback".to_string(),
            },
            SearchSymbolProjection {
                node_id: NodeId(100),
                display_name: "src/lib.rs".to_string(),
            },
        ]
    );
    assert!(
        storage
            .get_canonical_search_symbol_batch_after(Some(NodeId(100)), 2)?
            .is_empty()
    );

    let details = storage.get_canonical_search_symbol_detail_batch_after(None, usize::MAX)?;
    assert_eq!(details.len(), 4);
    assert_eq!(details[0].node_id, NodeId(10));
    assert_eq!(details[0].display_name, "pkg::qualified");
    assert_eq!(details[0].node_kind, Some(NodeKind::FUNCTION as i64));
    assert_eq!(details[0].file_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(details[0].start_line, Some(7));
    assert_eq!(details[0].end_line, Some(11));

    storage.clear_search_symbol_projection()?;
    assert_eq!(storage.get_search_symbol_projection_count()?, 0);
    assert_eq!(
        storage.get_canonical_search_symbol_batch_after(None, usize::MAX)?,
        [first_page, second_page].concat()
    );
    Ok(())
}

#[test]
fn canonical_search_symbol_batches_reject_zero_limit() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    assert!(matches!(
        storage.get_canonical_search_symbol_batch_after(None, 0),
        Err(StorageError::InvalidBatchLimit(
            "get_canonical_search_symbol_batch_after"
        ))
    ));
    assert!(matches!(
        storage.get_canonical_search_symbol_detail_batch_after(None, 0),
        Err(StorageError::InvalidBatchLimit(
            "get_canonical_search_symbol_detail_batch_after"
        ))
    ));
    Ok(())
}

#[test]
fn staged_build_node_pages_filter_and_order_across_keyset_boundaries() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("semantic-node-pages");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let mut storage = Storage::open_build(&db_path)?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(700),
            kind: NodeKind::FILE,
            serialized_name: "src/seven.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(500),
            kind: NodeKind::FILE,
            serialized_name: "src/five.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(30),
            kind: NodeKind::METHOD,
            serialized_name: "third".to_string(),
            file_node_id: Some(NodeId(700)),
            ..Default::default()
        },
        Node {
            id: NodeId(10),
            kind: NodeKind::FUNCTION,
            serialized_name: "first".to_string(),
            file_node_id: Some(NodeId(700)),
            ..Default::default()
        },
        Node {
            id: NodeId(40),
            kind: NodeKind::UNKNOWN,
            serialized_name: "excluded".to_string(),
            file_node_id: Some(NodeId(500)),
            ..Default::default()
        },
        Node {
            id: NodeId(20),
            kind: NodeKind::CLASS,
            serialized_name: "second".to_string(),
            file_node_id: Some(NodeId(500)),
            ..Default::default()
        },
        Node {
            id: NodeId(60),
            kind: NodeKind::FUNCTION,
            serialized_name: "fourth".to_string(),
            ..Default::default()
        },
    ])?;
    storage.cache.nodes.write().clear();

    let accepted_kinds = [
        NodeKind::METHOD,
        NodeKind::FUNCTION,
        NodeKind::CLASS,
        NodeKind::FUNCTION,
    ];
    let first_page = storage.get_nodes_by_kinds_batch_after_for_build(&accepted_kinds, None, 2)?;
    assert_eq!(
        first_page.iter().map(|node| node.id).collect::<Vec<_>>(),
        vec![NodeId(10), NodeId(20)]
    );
    let second_page =
        storage.get_nodes_by_kinds_batch_after_for_build(&accepted_kinds, Some(NodeId(20)), 2)?;
    assert_eq!(
        second_page.iter().map(|node| node.id).collect::<Vec<_>>(),
        vec![NodeId(30), NodeId(60)]
    );
    assert!(
        storage
            .get_nodes_by_kinds_batch_after_for_build(&accepted_kinds, Some(NodeId(60)), 2)?
            .is_empty()
    );
    assert_eq!(
        storage.get_node_file_ids_by_kinds_for_build(&accepted_kinds)?,
        vec![NodeId(500), NodeId(700)]
    );
    assert!(
        storage
            .get_nodes_by_kinds_batch_after_for_build(&[], None, 2)?
            .is_empty()
    );
    assert!(
        storage
            .get_node_file_ids_by_kinds_for_build(&[])?
            .is_empty()
    );
    assert!(
        storage.cache.nodes.read().is_empty(),
        "build node scans must not populate StorageCache"
    );

    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;
    Ok(())
}

#[test]
fn staged_build_node_page_rejects_zero_limit_and_live_stores() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("semantic-node-page-limit");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let storage = Storage::open_build(&db_path)?;
    assert!(matches!(
        storage.get_nodes_by_kinds_batch_after_for_build(&[NodeKind::FUNCTION], None, 0),
        Err(StorageError::InvalidBatchLimit(
            "get_nodes_by_kinds_batch_after_for_build"
        ))
    ));
    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;

    let live = Storage::new_in_memory()?;
    assert!(matches!(
        live.get_nodes_by_kinds_batch_after_for_build(&[NodeKind::FUNCTION], None, 1),
        Err(StorageError::BuildModeRequired(
            "get_nodes_by_kinds_batch_after_for_build"
        ))
    ));
    assert!(matches!(
        live.get_node_file_ids_by_kinds_for_build(&[NodeKind::FUNCTION]),
        Err(StorageError::BuildModeRequired(
            "get_node_file_ids_by_kinds_for_build"
        ))
    ));
    assert!(matches!(
        live.get_nodes_by_ids_no_cache_for_build(&[NodeId(1)]),
        Err(StorageError::BuildModeRequired(
            "get_nodes_by_ids_no_cache_for_build"
        ))
    ));
    Ok(())
}

#[test]
fn staged_build_node_page_plan_uses_integer_primary_key_without_temp_sort()
-> Result<(), StorageError> {
    let db_path = unique_temp_db_path("semantic-node-page-plan");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let storage = Storage::open_build(&db_path)?;
    let page_sql = nodes_by_kinds_batch_sql(3, true);
    assert!(page_sql.contains("FROM node NOT INDEXED"));
    let mut statement = storage
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {page_sql}"))?;
    let plan = statement
        .query_map(
            rusqlite::params![
                NodeKind::FUNCTION as i32,
                NodeKind::METHOD as i32,
                NodeKind::CLASS as i32,
                0_i64,
                4_096_i64
            ],
            |row| row.get::<_, String>(3),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        plan.iter()
            .any(|line| line.contains("INTEGER PRIMARY KEY (rowid>?)")),
        "semantic node page did not use integer-primary-key traversal: {plan:?}"
    );
    assert!(
        plan.iter().all(|line| !line.contains("USE TEMP B-TREE")),
        "semantic node page introduced a temporary sort: {plan:?}"
    );

    drop(statement);
    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;
    Ok(())
}

#[test]
fn staged_build_node_lookup_batches_duplicates_without_touching_cache() -> Result<(), StorageError>
{
    let db_path = unique_temp_db_path("semantic-node-lookup");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let mut storage = Storage::open_build(&db_path)?;
    let nodes = (1_i64..=205)
        .map(|id| Node {
            id: NodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("cached-{id}"),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    storage.insert_nodes_batch(&nodes)?;
    storage.conn.execute(
        "UPDATE node SET serialized_name = 'database-1' WHERE id = 1",
        [],
    )?;
    let cache_len_before = storage.cache.nodes.read().len();
    assert_eq!(
        storage
            .cache
            .nodes
            .read()
            .get(&NodeId(1))
            .map(|node| node.serialized_name.as_str()),
        Some("cached-1")
    );

    let mut requested_ids = (1_i64..=205).map(NodeId).collect::<Vec<_>>();
    requested_ids.extend([NodeId(1), NodeId(200), NodeId(205), NodeId(999)]);
    let lookup = storage.get_nodes_by_ids_no_cache_for_build(&requested_ids)?;
    assert_eq!(lookup.query_batches, 2);
    assert_eq!(lookup.nodes.len(), 205);
    assert_eq!(
        lookup
            .nodes
            .get(&NodeId(1))
            .map(|node| node.serialized_name.as_str()),
        Some("database-1"),
        "uncached lookup read a stale cached node"
    );
    assert!(!lookup.nodes.contains_key(&NodeId(999)));
    assert_eq!(storage.cache.nodes.read().len(), cache_len_before);
    assert_eq!(
        storage
            .cache
            .nodes
            .read()
            .get(&NodeId(1))
            .map(|node| node.serialized_name.as_str()),
        Some("cached-1"),
        "uncached lookup mutated StorageCache"
    );
    let empty = storage.get_nodes_by_ids_no_cache_for_build(&[])?;
    assert!(empty.nodes.is_empty());
    assert_eq!(empty.query_batches, 0);

    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;
    Ok(())
}

#[test]
fn staged_build_edge_batches_match_incident_edge_order_and_remain_bounded()
-> Result<(), StorageError> {
    let db_path = unique_temp_db_path("semantic-edge-batches");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let mut storage = Storage::open_build(&db_path)?;
    let nodes = (1_i64..=10)
        .map(|id| Node {
            id: NodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("node-{id}"),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    storage.insert_nodes_batch(&nodes)?;
    let mut edges = vec![
        Edge {
            id: EdgeId(-9),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            resolved_target: Some(NodeId(3)),
            certainty: Some(ResolutionCertainty::Uncertain),
            confidence: Some(0.2),
            ..Default::default()
        },
        Edge {
            id: EdgeId(-3),
            source: NodeId(4),
            target: NodeId(1),
            kind: EdgeKind::USAGE,
            ..Default::default()
        },
    ];
    edges.extend((0_i64..9).map(|offset| Edge {
        id: EdgeId(10 + offset),
        source: NodeId(1),
        target: NodeId(2 + offset),
        kind: EdgeKind::MEMBER,
        ..Default::default()
    }));
    storage.insert_edges_batch(&edges)?;
    storage.cache.nodes.write().clear();

    let expected = storage
        .get_edges_for_node_ids(&[NodeId(1)])?
        .remove(&NodeId(1))
        .expect("seed edge list");
    let mut streamed = Vec::new();
    let mut after_edge_id = None;
    loop {
        let batch =
            storage.get_edges_for_node_ids_batch_after_for_build(&[NodeId(1)], after_edge_id, 3)?;
        assert!(batch.len() <= 3);
        if batch.is_empty() {
            break;
        }
        assert!(batch.windows(2).all(|pair| pair[0].id < pair[1].id));
        after_edge_id = batch.last().map(|edge| edge.id);
        streamed.extend(batch);
    }
    assert_eq!(streamed, expected);
    assert_eq!(
        streamed[0].resolved_target, None,
        "streamed lookup must retain ignored CALL-resolution behavior"
    );
    assert!(
        storage.cache.nodes.read().is_empty(),
        "edge streaming must not populate StorageCache"
    );
    assert!(matches!(
        storage.get_edges_for_node_ids_batch_after_for_build(&[NodeId(1)], None, 0),
        Err(StorageError::InvalidBatchLimit(
            "get_edges_for_node_ids_batch_after_for_build"
        ))
    ));
    let too_many = (0..=BUILD_EDGE_SEED_BATCH_SIZE)
        .map(|offset| NodeId(offset as i64 + 100))
        .collect::<Vec<_>>();
    assert!(matches!(
        storage.get_edges_for_node_ids_batch_after_for_build(&too_many, None, 1),
        Err(StorageError::BuildEdgeSeedBatchTooLarge {
            actual,
            maximum: BUILD_EDGE_SEED_BATCH_SIZE,
            ..
        }) if actual == BUILD_EDGE_SEED_BATCH_SIZE + 1
    ));

    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;
    let live = Storage::new_in_memory()?;
    assert!(matches!(
        live.get_edges_for_node_ids_batch_after_for_build(&[NodeId(1)], None, 1),
        Err(StorageError::BuildModeRequired(
            "get_edges_for_node_ids_batch_after_for_build"
        ))
    ));
    Ok(())
}

#[test]
fn test_scoped_search_symbol_projection_rebuild() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(800),
            kind: NodeKind::FILE,
            serialized_name: "src/changed.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(801),
            kind: NodeKind::FUNCTION,
            serialized_name: "old_name".to_string(),
            qualified_name: Some("pkg::old_name".to_string()),
            file_node_id: Some(NodeId(800)),
            ..Default::default()
        },
        Node {
            id: NodeId(810),
            kind: NodeKind::FILE,
            serialized_name: "src/untouched.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(811),
            kind: NodeKind::FUNCTION,
            serialized_name: "untouched".to_string(),
            qualified_name: Some("pkg::untouched".to_string()),
            file_node_id: Some(NodeId(810)),
            ..Default::default()
        },
    ])?;
    assert_eq!(
        storage.rebuild_search_symbol_projection_from_node_table()?,
        4
    );

    storage.insert_nodes_batch(&[Node {
        id: NodeId(801),
        kind: NodeKind::FUNCTION,
        serialized_name: "renamed".to_string(),
        qualified_name: Some("pkg::renamed".to_string()),
        file_node_id: Some(NodeId(800)),
        ..Default::default()
    }])?;
    storage.upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
        node_id: NodeId(811),
        display_name: "stale_other_file".to_string(),
    }])?;

    let touched = HashSet::from([NodeId(800)]);
    assert_eq!(
        storage.rebuild_search_symbol_projection_for_file_scope(&touched)?,
        2
    );

    let projection = storage.get_search_symbol_projection_batch_after(None, 10)?;
    let names_by_id: HashMap<_, _> = projection
        .into_iter()
        .map(|entry| (entry.node_id, entry.display_name))
        .collect();
    assert_eq!(
        names_by_id.get(&NodeId(800)).map(String::as_str),
        Some("src/changed.rs")
    );
    assert_eq!(
        names_by_id.get(&NodeId(801)).map(String::as_str),
        Some("pkg::renamed")
    );
    assert_eq!(
        names_by_id.get(&NodeId(811)).map(String::as_str),
        Some("stale_other_file")
    );
    Ok(())
}

#[test]
fn test_clear_removes_fk_dependents_and_cache() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_node = Node {
        id: NodeId(500),
        kind: NodeKind::FILE,
        serialized_name: "src/main.rs".to_string(),
        ..Default::default()
    };
    let function_node = Node {
        id: NodeId(501),
        kind: NodeKind::FUNCTION,
        serialized_name: "main".to_string(),
        file_node_id: Some(file_node.id),
        ..Default::default()
    };

    storage.insert_file(&FileInfo {
        id: file_node.id.0,
        path: PathBuf::from("src/main.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;
    storage.insert_nodes_batch(&[file_node.clone(), function_node.clone()])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(700),
        source: function_node.id,
        target: function_node.id,
        kind: EdgeKind::CALL,
        file_node_id: Some(file_node.id),
        ..Default::default()
    }])?;
    storage.insert_occurrences_batch(&[Occurrence {
        element_id: function_node.id.0,
        kind: codestory_contracts::graph::OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_node.id,
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 4,
        },
    }])?;
    storage.insert_component_access_batch(&[(function_node.id, AccessKind::Public)])?;
    storage.upsert_callable_projection_states(&[CallableProjectionState {
        file_id: file_node.id.0,
        symbol_key: "src/main.rs::main:FUNCTION".to_string(),
        node_id: function_node.id,
        signature_hash: 101,
        body_hash: 202,
        start_line: 1,
        end_line: 1,
    }])?;
    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "test".to_string(),
        file_id: Some(file_node.id),
        line: Some(1),
        column: Some(1),
        is_fatal: false,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
        coverage_reason: None,
    })?;
    storage.conn.execute(
        "INSERT INTO local_symbol (id, name, file_id) VALUES (?1, ?2, ?3)",
        params![1_i64, "main", file_node.id.0],
    )?;

    let category_id = storage.create_bookmark_category("Favorites")?;
    let _ = storage.add_bookmark(category_id, function_node.id, Some("keep"))?;

    // Ensure cache is warm before clear.
    assert!(storage.get_node(function_node.id)?.is_some());

    storage.clear()?;

    for table in [
        "occurrence",
        "edge",
        "llm_symbol_doc",
        "symbol_summary",
        "callable_projection_state",
        "component_access",
        "bookmark_node",
        "local_symbol",
        "error",
        "node",
        "file",
    ] {
        let count: i64 =
            storage
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
        assert_eq!(count, 0, "expected {table} to be empty after clear");
    }

    // Categories are user-managed metadata; clear only removes node-linked data.
    assert_eq!(storage.get_bookmark_categories()?.len(), 1);
    assert!(storage.get_node(function_node.id)?.is_none());
    Ok(())
}

#[test]
fn test_callable_projection_state_round_trip() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_file(&FileInfo {
        id: 11,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 40,
        file_role: FileRole::Source,
    })?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(11),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
        Node {
            id: NodeId(102),
            kind: NodeKind::FUNCTION,
            serialized_name: "helper".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
    ])?;
    storage.upsert_callable_projection_states(&[
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
            node_id: NodeId(101),
            signature_hash: 111,
            body_hash: 211,
            start_line: 10,
            end_line: 20,
        },
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::helper:FUNCTION".to_string(),
            node_id: NodeId(102),
            signature_hash: 112,
            body_hash: 212,
            start_line: 30,
            end_line: 35,
        },
    ])?;

    let stored = storage.get_callable_projection_states_for_file(11)?;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].symbol_key, "src/lib.rs::run:FUNCTION");

    storage.upsert_callable_projection_states(&[CallableProjectionState {
        file_id: 11,
        symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
        node_id: NodeId(101),
        signature_hash: 111,
        body_hash: 299,
        start_line: 12,
        end_line: 22,
    }])?;
    let updated = storage.get_callable_projection_states_for_file(11)?;
    assert_eq!(updated.len(), 2);
    let run_state = updated
        .iter()
        .find(|state| state.symbol_key == "src/lib.rs::run:FUNCTION")
        .expect("updated run state");
    assert_eq!(run_state.body_hash, 299);
    assert_eq!(run_state.start_line, 12);
    Ok(())
}

#[test]
fn test_delete_callable_projection_states_for_file() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_file(&FileInfo {
        id: 11,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 40,
        file_role: FileRole::Source,
    })?;
    storage.insert_file(&FileInfo {
        id: 12,
        path: PathBuf::from("src/other.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(11),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(12),
            kind: NodeKind::FILE,
            serialized_name: "src/other.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
        Node {
            id: NodeId(102),
            kind: NodeKind::FUNCTION,
            serialized_name: "helper".to_string(),
            file_node_id: Some(NodeId(11)),
            ..Default::default()
        },
        Node {
            id: NodeId(201),
            kind: NodeKind::FUNCTION,
            serialized_name: "keep".to_string(),
            file_node_id: Some(NodeId(12)),
            ..Default::default()
        },
    ])?;
    storage.upsert_callable_projection_states(&[
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
            node_id: NodeId(101),
            signature_hash: 111,
            body_hash: 211,
            start_line: 10,
            end_line: 20,
        },
        CallableProjectionState {
            file_id: 11,
            symbol_key: "src/lib.rs::helper:FUNCTION".to_string(),
            node_id: NodeId(102),
            signature_hash: 112,
            body_hash: 212,
            start_line: 30,
            end_line: 35,
        },
        CallableProjectionState {
            file_id: 12,
            symbol_key: "src/other.rs::keep:FUNCTION".to_string(),
            node_id: NodeId(201),
            signature_hash: 311,
            body_hash: 411,
            start_line: 1,
            end_line: 5,
        },
    ])?;

    let removed = storage.delete_callable_projection_states_for_file(11)?;
    assert_eq!(removed, 2);
    assert!(
        storage
            .get_callable_projection_states_for_file(11)?
            .is_empty()
    );
    assert_eq!(
        storage.get_callable_projection_states_for_file(12)?.len(),
        1
    );
    Ok(())
}

#[test]
fn test_delete_projection_for_callers_removes_callable_scoped_data() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_id = 9_i64;
    let file_node = Node {
        id: NodeId(file_id),
        kind: NodeKind::FILE,
        serialized_name: "src/lib.rs".to_string(),
        ..Default::default()
    };
    let caller_a = Node {
        id: NodeId(901),
        kind: NodeKind::FUNCTION,
        serialized_name: "run".to_string(),
        file_node_id: Some(file_node.id),
        ..Default::default()
    };
    let caller_b = Node {
        id: NodeId(902),
        kind: NodeKind::FUNCTION,
        serialized_name: "keep".to_string(),
        file_node_id: Some(file_node.id),
        ..Default::default()
    };

    storage.insert_file(&FileInfo {
        id: file_id,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 50,
        file_role: FileRole::Source,
    })?;
    storage.insert_nodes_batch(&[
        file_node.clone(),
        caller_a.clone(),
        caller_b.clone(),
        Node {
            id: NodeId(903),
            kind: NodeKind::FUNCTION,
            serialized_name: "callee".to_string(),
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: caller_a.id,
            target: NodeId(903),
            kind: EdgeKind::CALL,
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: caller_b.id,
            target: NodeId(903),
            kind: EdgeKind::CALL,
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
        Edge {
            id: EdgeId(3),
            source: caller_a.id,
            target: NodeId(903),
            kind: EdgeKind::USAGE,
            file_node_id: Some(file_node.id),
            ..Default::default()
        },
    ])?;
    storage.insert_occurrences_batch(&[
        Occurrence {
            element_id: caller_a.id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 1,
                start_col: 0,
                end_line: 3,
                end_col: 1,
            },
        },
        Occurrence {
            element_id: caller_b.id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 10,
                start_col: 0,
                end_line: 12,
                end_col: 1,
            },
        },
        Occurrence {
            element_id: NodeId(903).0,
            kind: OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 2,
                start_col: 4,
                end_line: 2,
                end_col: 10,
            },
        },
        Occurrence {
            element_id: NodeId(903).0,
            kind: OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: file_node.id,
                start_line: 11,
                start_col: 4,
                end_line: 11,
                end_col: 10,
            },
        },
    ])?;
    storage.upsert_callable_projection_states(&[
        CallableProjectionState {
            file_id,
            symbol_key: "src/lib.rs::run:FUNCTION".to_string(),
            node_id: caller_a.id,
            signature_hash: 111,
            body_hash: 211,
            start_line: 1,
            end_line: 3,
        },
        CallableProjectionState {
            file_id,
            symbol_key: "src/lib.rs::keep:FUNCTION".to_string(),
            node_id: caller_b.id,
            signature_hash: 112,
            body_hash: 212,
            start_line: 10,
            end_line: 12,
        },
    ])?;

    let summary = storage.delete_projection_for_callers(file_id, &[caller_a.id])?;
    assert_eq!(summary.removed_edge_count, 2);
    assert_eq!(summary.removed_occurrence_count, 2);
    assert_eq!(summary.removed_callable_projection_state_count, 1);

    let remaining_edges = storage.get_edges()?;
    assert_eq!(remaining_edges.len(), 1);
    assert_eq!(remaining_edges[0].source, caller_b.id);

    let remaining_occurrences = storage.get_occurrences()?;
    assert_eq!(remaining_occurrences.len(), 2);
    assert!(
        remaining_occurrences
            .iter()
            .any(|occurrence| occurrence.element_id == caller_b.id.0)
    );
    assert!(
        remaining_occurrences
            .iter()
            .any(|occurrence| occurrence.element_id == NodeId(903).0)
    );

    let remaining_states = storage.get_callable_projection_states_for_file(file_id)?;
    assert_eq!(remaining_states.len(), 1);
    assert_eq!(remaining_states[0].node_id, caller_b.id);
    Ok(())
}

#[test]
fn test_opening_v3_db_resets_projection_state() -> Result<(), StorageError> {
    let db_path = std::env::temp_dir().join(format!(
        "codestory-store-v3-migration-{}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&db_path);
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        schema::create_tables(&conn)?;
        schema::create_indexes(&conn, StorageOpenMode::Live)?;
        conn.pragma_update(None, "user_version", 3)?;
        conn.execute(
            "INSERT INTO file (id, path, language, modification_time, indexed, complete, line_count)
             VALUES (1, 'src/lib.rs', 'rust', 1, 1, 1, 10)",
            [],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name) VALUES (?1, ?2, ?3)",
            params![1_i64, NodeKind::FILE as i32, "src/lib.rs"],
        )?;
        conn.execute(
            "INSERT INTO callable_projection_state (file_id, symbol_key, node_id, signature_hash, body_hash, start_line, end_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![1_i64, "sym", 1_i64, 11_i64, 22_i64, 1_i64, 2_i64],
        )?;
        conn.execute(
            "INSERT INTO bookmark_category (id, name) VALUES (1, 'Favorites')",
            [],
        )?;
        conn.execute(
            "INSERT INTO bookmark_node (id, category_id, node_id, comment) VALUES (1, 1, 1, 'saved')",
            [],
        )?;
    }

    let storage = Storage::open(&db_path)?;
    assert!(storage.get_files()?.is_empty());
    assert!(storage.get_nodes()?.is_empty());
    assert!(
        storage
            .get_callable_projection_states_for_file(1)?
            .is_empty()
    );
    assert!(storage.get_bookmarks(None)?.is_empty());
    assert!(storage.get_bookmark_categories()?.is_empty());
    drop(storage);
    let _ = std::fs::remove_file(&db_path);
    Ok(())
}

#[test]
fn live_open_migrates_v17_llm_doc_columns_before_secondary_indexes() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("v17-ast-first-live-migration");
    let _ = std::fs::remove_file(&db_path);
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "CREATE TABLE llm_symbol_doc (
                node_id INTEGER PRIMARY KEY,
                file_node_id INTEGER,
                kind INTEGER NOT NULL,
                display_name TEXT NOT NULL,
                qualified_name TEXT,
                file_path TEXT,
                start_line INTEGER,
                doc_text TEXT NOT NULL,
                doc_version INTEGER NOT NULL DEFAULT 0,
                doc_hash TEXT NOT NULL DEFAULT '',
                embedding_model TEXT NOT NULL,
                embedding_profile TEXT,
                embedding_backend TEXT,
                embedding_dim INTEGER NOT NULL,
                doc_shape TEXT,
                embedding_blob BLOB NOT NULL,
                updated_at_epoch_ms INTEGER NOT NULL
            )",
            [],
        )?;
        conn.pragma_update(None, "user_version", 17)?;
    }

    let storage = Storage::open(&db_path)?;
    let columns = storage
        .conn
        .prepare("PRAGMA table_info(llm_symbol_doc)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        columns
            .iter()
            .any(|column| column == "semantic_policy_version")
    );
    assert!(columns.iter().any(|column| column == "dense_reason"));
    let policy_index_count: i64 = storage.conn.query_row(
        "SELECT COUNT(*)
         FROM sqlite_master
         WHERE type = 'index'
           AND name = 'idx_llm_symbol_doc_policy_reason'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(policy_index_count, 1);

    drop(storage);
    let _ = std::fs::remove_file(&db_path);
    Ok(())
}

#[test]
fn live_open_migrates_v18_manifest_to_lexical_schema_without_losing_rows()
-> Result<(), StorageError> {
    let db_path = unique_temp_db_path("v18-precise-semantic-manifest-repair");
    let _ = std::fs::remove_file(&db_path);
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "CREATE TABLE retrieval_index_manifest (
                project_id TEXT PRIMARY KEY,
                zoekt_version TEXT NOT NULL,
                semantic_generation TEXT NOT NULL,
                scip_revision TEXT,
                built_at_epoch_ms INTEGER NOT NULL,
                disk_bytes INTEGER,
                degraded_modes_json TEXT NOT NULL DEFAULT '[]',
                embedding_backend TEXT,
                embedding_dim INTEGER,
                sidecar_schema_version INTEGER,
                sidecar_input_hash TEXT,
                sidecar_generation TEXT,
                projection_count INTEGER,
                symbol_doc_count INTEGER,
                dense_projection_count INTEGER,
                semantic_policy_version TEXT,
                graph_artifact_hash TEXT,
                dense_reason_counts_json TEXT
            )",
            [],
        )?;
        conn.execute(
            "INSERT INTO retrieval_index_manifest (
                project_id,
                zoekt_version,
                semantic_generation,
                built_at_epoch_ms,
                degraded_modes_json
            ) VALUES ('proj', 'legacy-v1', 'collection', 1, '[]')",
            [],
        )?;
        conn.pragma_update(None, "user_version", 18)?;
    }

    let storage = Storage::open(&db_path)?;
    let columns = storage
        .conn
        .prepare("PRAGMA table_info(retrieval_index_manifest)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    for column in [
        "lexical_version",
        "precise_semantic_import_status",
        "precise_semantic_import_reason",
        "precise_semantic_import_revision",
        "precise_semantic_import_producer",
    ] {
        assert!(columns.iter().any(|existing| existing == column));
    }
    assert!(!columns.iter().any(|existing| existing == "zoekt_version"));
    let manifest = storage
        .get_retrieval_index_manifest("proj")?
        .expect("manifest survives repair");
    assert_eq!(manifest.project_id, "proj");
    assert_eq!(manifest.lexical_version, "legacy-v1");
    assert_eq!(manifest.precise_semantic_import_status, None);

    drop(storage);
    let _ = std::fs::remove_file(&db_path);
    Ok(())
}

#[test]
fn current_schema_uses_only_lexical_manifest_column() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("current-lexical-manifest-contract");
    let _ = std::fs::remove_file(&db_path);
    let storage = Storage::open(&db_path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    let columns = storage
        .conn
        .prepare("PRAGMA table_info(retrieval_index_manifest)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(columns.iter().any(|column| column == "lexical_version"));
    assert!(!columns.iter().any(|column| column == "zoekt_version"));

    drop(storage);
    let _ = std::fs::remove_file(&db_path);
    Ok(())
}

#[test]
fn schema_24_adds_atomic_retrieval_rollback_without_losing_current() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("v24-retrieval-rollback-migration");
    let _ = std::fs::remove_file(&db_path);
    let current = RetrievalIndexManifest {
        project_id: "proj".into(),
        lexical_version: "v1".into(),
        semantic_generation: "codestory_proj_aaaaaaaaaaaaaaaa".into(),
        scip_revision: Some("graph".into()),
        built_at_epoch_ms: 1,
        disk_bytes: None,
        degraded_modes_json: "[]".into(),
        embedding_backend: Some("backend".into()),
        embedding_dim: Some(768),
        sidecar_schema_version: Some(5),
        sidecar_input_hash: Some("a".repeat(64)),
        sidecar_generation: Some("proj-aaaaaaaaaaaaaaaa".into()),
        projection_count: Some(0),
        symbol_doc_count: Some(0),
        dense_projection_count: Some(0),
        semantic_policy_version: Some("graph_first_v1".into()),
        graph_artifact_hash: Some("graph".into()),
        dense_reason_counts_json: Some("{}".into()),
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    };
    {
        let mut storage = Storage::open(&db_path)?;
        storage.upsert_retrieval_index_manifest(&current)?;
        storage.conn.execute(
            "ALTER TABLE retrieval_index_manifest DROP COLUMN rollback_record_json",
            [],
        )?;
        storage.set_schema_version(24)?;
    }

    let storage = Storage::open(&db_path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    assert_eq!(
        storage.get_retrieval_index_publication("proj")?,
        Some((current, None))
    );
    drop(storage);
    let _ = cleanup_sqlite_sidecars(&db_path);
    Ok(())
}

#[test]
fn schema_26_adds_nullable_error_coverage_reason_idempotently() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("v26-error-coverage-reason-migration");
    let _ = std::fs::remove_file(&db_path);
    {
        let storage = Storage::open(&db_path)?;
        storage.conn.execute(
            "INSERT INTO error (message, fatal, indexed) VALUES ('legacy error', 0, 1)",
            [],
        )?;
        storage
            .conn
            .execute("ALTER TABLE error DROP COLUMN coverage_reason", [])?;
        storage.set_schema_version(25)?;
    }

    let storage = Storage::open(&db_path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    let columns = storage
        .conn
        .prepare("PRAGMA table_info(error)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(columns.iter().any(|column| column == "coverage_reason"));
    let errors = storage.get_errors(None)?;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].coverage_reason, None);

    schema::migrate_v26_error_coverage_reason(&storage.conn)?;
    schema::migrate_v26_error_coverage_reason(&storage.conn)?;

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&db_path);
    Ok(())
}

#[test]
fn schema_27_adds_source_policy_tables_without_synthesizing_publication() -> Result<(), StorageError>
{
    let db_path = unique_temp_db_path("v27-source-policy-exclusion-migration");
    let _ = std::fs::remove_file(&db_path);
    {
        let storage = Storage::open(&db_path)?;
        storage
            .conn
            .execute("DROP TABLE source_policy_exclusion_publication", [])?;
        storage
            .conn
            .execute("DROP TABLE source_policy_exclusion", [])?;
        storage.set_schema_version(26)?;
    }

    let storage = Storage::open(&db_path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    assert!(storage.get_source_policy_exclusions()?.is_empty());
    assert!(
        storage.get_source_policy_exclusion_manifest()?.is_none(),
        "migration cannot manufacture verified exclusion evidence"
    );
    schema::migrate_v27_source_policy_exclusions(&storage.conn)?;
    schema::migrate_v27_source_policy_exclusions(&storage.conn)?;

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&db_path);
    Ok(())
}

#[test]
fn schema_28_adds_structural_unit_tables_without_synthesizing_publication()
-> Result<(), StorageError> {
    let db_path = unique_temp_db_path("v28-structural-unit-migration");
    let _ = std::fs::remove_file(&db_path);
    {
        let storage = Storage::open(&db_path)?;
        for table in [
            "structural_text_unit_publication",
            "structural_text_projection",
            "structural_text_unit",
            "structural_text_artifact_cache",
        ] {
            storage.conn.execute(&format!("DROP TABLE {table}"), [])?;
        }
        storage.set_schema_version(27)?;
    }

    let storage = Storage::open(&db_path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    assert!(
        storage
            .get_structural_text_unit_publication_manifest()?
            .is_none(),
        "migration cannot manufacture verified structural evidence"
    );
    assert!(
        storage
            .get_structural_text_projection_file_ids()?
            .is_empty()
    );
    schema::migrate_v28_structural_text_units(&storage.conn)?;
    schema::migrate_v28_structural_text_units(&storage.conn)?;

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&db_path);
    Ok(())
}

#[test]
fn schema_30_preserves_publication_and_adds_semantic_projection_mode() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("v30-semantic-projection-publication-mode");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let previous = IndexPublicationRecord {
        generation: 9,
        generation_id: "generation-nine".into(),
        run_id: "run-nine".into(),
        mode: IndexPublicationMode::Incremental,
        published_at_epoch_ms: 9,
    };
    {
        let storage = Storage::open(&db_path)?;
        storage.put_index_publication(&previous)?;
        storage.conn.execute_batch(
            "ALTER TABLE index_publication RENAME TO index_publication_v30;
             CREATE TABLE index_publication (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                generation INTEGER NOT NULL CHECK (generation > 0),
                generation_id TEXT NOT NULL UNIQUE CHECK (length(generation_id) > 0),
                run_id TEXT NOT NULL CHECK (length(run_id) > 0),
                mode TEXT NOT NULL CHECK (mode IN ('full', 'incremental')),
                published_at_epoch_ms INTEGER NOT NULL CHECK (published_at_epoch_ms >= 0)
             );
             INSERT INTO index_publication
             SELECT * FROM index_publication_v30;
             DROP TABLE index_publication_v30;",
        )?;
        storage.set_schema_version(29)?;
    }

    let storage = Storage::open(&db_path)?;
    assert_eq!(storage.schema_version()?, SCHEMA_VERSION);
    assert_eq!(storage.get_index_publication()?, Some(previous));
    let republished = IndexPublicationRecord {
        generation: 10,
        generation_id: "generation-ten".into(),
        run_id: "run-ten".into(),
        mode: IndexPublicationMode::SemanticProjection,
        published_at_epoch_ms: 10,
    };
    storage.put_index_publication(&republished)?;
    assert_eq!(storage.get_index_publication()?, Some(republished));

    drop(storage);
    let _ = cleanup_sqlite_sidecars(&db_path);
    Ok(())
}

#[test]
fn v19_and_v20_manifests_migrate_once_and_new_writes_do_not_recreate_legacy_column()
-> Result<(), StorageError> {
    for source_version in [19, 20] {
        let db_path = unique_temp_db_path(&format!("v{source_version}-lexical-manifest-migration"));
        let _ = std::fs::remove_file(&db_path);
        {
            let conn = rusqlite::Connection::open(&db_path)?;
            conn.execute_batch(
                "CREATE TABLE retrieval_index_manifest (
                project_id TEXT PRIMARY KEY,
                zoekt_version TEXT NOT NULL,
                semantic_generation TEXT NOT NULL,
                scip_revision TEXT,
                built_at_epoch_ms INTEGER NOT NULL,
                disk_bytes INTEGER,
                degraded_modes_json TEXT NOT NULL DEFAULT '[]',
                embedding_backend TEXT,
                embedding_dim INTEGER,
                sidecar_schema_version INTEGER,
                sidecar_input_hash TEXT,
                sidecar_generation TEXT,
                projection_count INTEGER,
                symbol_doc_count INTEGER,
                dense_projection_count INTEGER,
                semantic_policy_version TEXT,
                graph_artifact_hash TEXT,
                dense_reason_counts_json TEXT,
                precise_semantic_import_status TEXT,
                precise_semantic_import_reason TEXT,
                precise_semantic_import_revision TEXT,
                precise_semantic_import_producer TEXT
            );
            INSERT INTO retrieval_index_manifest (
                project_id, zoekt_version, semantic_generation,
                built_at_epoch_ms, degraded_modes_json
            ) VALUES ('proj', 'legacy-v1', 'collection', 1, '[]');",
            )?;
            conn.pragma_update(None, "user_version", source_version)?;
        }

        let mut storage = Storage::open(&db_path)?;
        let mut manifest = storage
            .get_retrieval_index_manifest("proj")?
            .expect("legacy manifest row survives migration");
        assert_eq!(manifest.lexical_version, "legacy-v1");
        manifest.lexical_version = "sqlite-fts5-v1".into();
        storage.upsert_retrieval_index_manifest(&manifest)?;
        drop(storage);

        let storage = Storage::open(&db_path)?;
        let columns = storage
            .conn
            .prepare("PRAGMA table_info(retrieval_index_manifest)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(columns.iter().any(|column| column == "lexical_version"));
        assert!(!columns.iter().any(|column| column == "zoekt_version"));
        assert_eq!(
            storage
                .get_retrieval_index_manifest("proj")?
                .expect("updated manifest")
                .lexical_version,
            "sqlite-fts5-v1"
        );

        drop(storage);
        let _ = std::fs::remove_file(&db_path);
    }
    Ok(())
}

#[test]
fn live_open_preserves_correct_v18_manifest_precise_semantic_values() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("v18-precise-semantic-manifest-preserve");
    let _ = std::fs::remove_file(&db_path);
    {
        let mut storage = Storage::open(&db_path)?;
        storage.upsert_retrieval_index_manifest(&RetrievalIndexManifest {
            project_id: "proj".into(),
            lexical_version: "legacy-v1".into(),
            semantic_generation: "collection".into(),
            scip_revision: None,
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: None,
            embedding_dim: None,
            sidecar_schema_version: Some(1),
            sidecar_input_hash: Some("input".into()),
            sidecar_generation: Some("generation".into()),
            projection_count: Some(2),
            symbol_doc_count: Some(3),
            dense_projection_count: Some(4),
            semantic_policy_version: Some("graph_first_v1".into()),
            graph_artifact_hash: Some("graph".into()),
            dense_reason_counts_json: Some("{\"public_api\":4}".into()),
            precise_semantic_import_status: Some("fresh".into()),
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: Some("rev".into()),
            precise_semantic_import_producer: Some("producer".into()),
        })?;
    }

    let storage = Storage::open(&db_path)?;
    let manifest = storage
        .get_retrieval_index_manifest("proj")?
        .expect("manifest remains present");
    assert_eq!(
        manifest.precise_semantic_import_status,
        Some("fresh".into())
    );
    assert_eq!(
        manifest.precise_semantic_import_revision,
        Some("rev".into())
    );
    assert_eq!(
        manifest.precise_semantic_import_producer,
        Some("producer".into())
    );

    drop(storage);
    let _ = std::fs::remove_file(&db_path);
    Ok(())
}

#[test]
fn test_promote_staged_snapshot_replaces_live_db_while_live_reader_is_open()
-> Result<(), StorageError> {
    let live_path = unique_temp_db_path("live");
    let staged_path = unique_temp_db_path("staged");
    let backup_path = live_path.with_extension("sqlite.backup");
    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&staged_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);

    {
        let mut seed = Storage::open(&live_path)?;
        seed.insert_files_batch(&[FileInfo {
            id: 1,
            path: PathBuf::from("live.rs"),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 10,
            file_role: FileRole::Source,
        }])?;
        let live_publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "live-generation".to_string(),
            run_id: "live-run".to_string(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        seed.publish_structural_text_unit_generation(&live_publication)?;
        seed.put_index_publication(&live_publication)?;
        seed.publish_source_policy_exclusion_generation(
            &live_publication,
            "test-project",
            "test-workspace",
            source_policy_identity(
                OVERSIZED_SOURCE_POLICY_VERSION,
                DEFAULT_SOURCE_FILE_BYTE_CAP,
                codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            ),
            &[],
        )?;
        drop(seed);
        let live = Storage::open_read_only(&live_path)?;

        {
            let mut staged = Storage::open_build(&staged_path)?;
            staged.insert_files_batch(&[FileInfo {
                id: 2,
                path: PathBuf::from("staged.rs"),
                language: "rust".to_string(),
                modification_time: 2,
                indexed: true,
                complete: true,
                line_count: 20,
                file_role: FileRole::Source,
            }])?;
            let staged_publication = IndexPublicationRecord {
                generation: 2,
                generation_id: "staged-generation".to_string(),
                run_id: "staged-run".to_string(),
                mode: IndexPublicationMode::Full,
                published_at_epoch_ms: 2,
            };
            staged.publish_structural_text_unit_generation(&staged_publication)?;
            staged.put_index_publication(&staged_publication)?;
            staged.publish_source_policy_exclusion_generation(
                &staged_publication,
                "test-project",
                "test-workspace",
                source_policy_identity(
                    OVERSIZED_SOURCE_POLICY_VERSION,
                    DEFAULT_SOURCE_FILE_BYTE_CAP,
                    codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
                ),
                &[],
            )?;
            staged.finalize_staged_snapshot()?;
        }

        Storage::promote_staged_snapshot(&staged_path, &live_path)?;

        let live_reader_files = live.get_files()?;
        assert_eq!(live_reader_files.len(), 1);
    }

    let promoted = Storage::open(&live_path)?;
    let promoted_files = promoted.get_files()?;
    assert_eq!(promoted_files.len(), 1);
    assert_eq!(promoted_files[0].id, 2);
    assert_eq!(promoted_files[0].path, PathBuf::from("staged.rs"));
    drop(promoted);

    assert!(!staged_path.exists());
    assert!(!PathBuf::from(format!("{}-wal", staged_path.display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", staged_path.display())).exists());

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&staged_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);
    Ok(())
}

#[test]
fn reader_open_during_healthy_promotion_does_not_recover_active_backup() -> Result<(), StorageError>
{
    let live_path = unique_temp_db_path("active-promotion-live");
    let backup_path = live_path.with_extension("sqlite.backup");
    let lock_path = promotion_lock_path(&live_path);
    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);

    seed_promotion_file(&live_path, 2, "new.rs")?;
    seed_promotion_file(&backup_path, 1, "old.rs")?;
    let prepared_path = promotion_prepared_journal_path(&live_path);
    write_promotion_journal(
        &prepared_path,
        &promotion_journal(&backup_path, &live_path)?,
    )?;
    let promotion_lock = PromotionLock::acquire(&live_path)?;

    let during_promotion = Storage::open(&live_path)?;
    assert_eq!(
        during_promotion.get_files()?[0].path,
        PathBuf::from("new.rs")
    );
    drop(during_promotion);
    assert!(
        backup_path.exists(),
        "active promoter still owns its backup"
    );

    drop(promotion_lock);
    let recovered = Storage::open(&live_path)?;
    assert_eq!(recovered.get_files()?[0].path, PathBuf::from("old.rs"));
    drop(recovered);
    assert!(
        !backup_path.exists(),
        "recovery consumes the abandoned backup"
    );
    assert!(!prepared_path.exists(), "recovery consumes its journal");

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);
    let _ = std::fs::remove_file(lock_path);
    Ok(())
}

const DISPOSABLE_BUILD_ABORT_PATH_ENV: &str = "CODESTORY_TEST_DISPOSABLE_BUILD_ABORT_PATH";
const DISPOSABLE_BUILD_ABORT_SENTINEL_ENV: &str = "CODESTORY_TEST_DISPOSABLE_BUILD_ABORT_SENTINEL";
const PROMOTION_ABORT_LIVE_ENV: &str = "CODESTORY_TEST_PROMOTION_ABORT_LIVE";
const PROMOTION_ABORT_STAGED_ENV: &str = "CODESTORY_TEST_PROMOTION_ABORT_STAGED";

#[test]
fn disposable_full_build_abort_child() {
    let Some(staged_path) = std::env::var_os(DISPOSABLE_BUILD_ABORT_PATH_ENV).map(PathBuf::from)
    else {
        return;
    };
    let sentinel_path = PathBuf::from(
        std::env::var_os(DISPOSABLE_BUILD_ABORT_SENTINEL_ENV)
            .expect("disposable abort sentinel path"),
    );
    let mut staged =
        Storage::open_disposable_full_build(&staged_path).expect("open disposable child stage");
    staged
        .insert_files_batch(&[FileInfo {
            id: 2,
            path: PathBuf::from("unpublished.rs"),
            language: "rust".to_string(),
            modification_time: 2,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        }])
        .expect("write disposable child stage");
    fs::write(&sentinel_path, b"disposable-stage-written\n").expect("write abort sentinel");
    File::open(&sentinel_path)
        .and_then(|file| file.sync_all())
        .expect("sync abort sentinel");
    std::process::abort();
}

#[test]
fn process_abort_during_disposable_build_never_mutates_live() {
    let live_path = unique_temp_db_path("disposable-build-abort-live");
    let staged_path = unique_temp_db_path("disposable-build-abort-staged");
    let sentinel_path = unique_temp_db_path("disposable-build-abort-sentinel");
    seed_promotion_file(&live_path, 1, "old.rs").expect("seed live generation");

    let status =
        std::process::Command::new(std::env::current_exe().expect("resolve store test executable"))
            .arg("--exact")
            .arg("storage_impl::tests::disposable_full_build_abort_child")
            .arg("--nocapture")
            .env(DISPOSABLE_BUILD_ABORT_PATH_ENV, &staged_path)
            .env(DISPOSABLE_BUILD_ABORT_SENTINEL_ENV, &sentinel_path)
            .status()
            .expect("run disposable build abort child");
    assert!(!status.success(), "abort child exited successfully");
    assert_eq!(
        fs::read(&sentinel_path).expect("read disposable abort sentinel"),
        b"disposable-stage-written\n"
    );

    let live = Storage::open(&live_path).expect("reopen live after staged abort");
    assert_eq!(
        live.get_files().expect("read preserved live")[0].path,
        PathBuf::from("old.rs")
    );
    assert_eq!(
        live.get_complete_index_publication()
            .expect("read preserved publication")
            .expect("complete preserved publication")
            .generation,
        1
    );
    drop(live);
    assert!(!live_path.with_extension("sqlite.backup").exists());
    assert!(!promotion_prepared_journal_path(&live_path).exists());
    assert!(!promotion_committed_journal_path(&live_path).exists());

    cleanup_sqlite_sidecars(&live_path).expect("clean live fixture");
    cleanup_sqlite_sidecars(&staged_path).expect("clean aborted stage");
    let _ = fs::remove_file(sentinel_path);
}

fn seed_promotion_file_with_identity(
    path: &Path,
    id: i64,
    name: &str,
    publish: bool,
) -> Result<(), StorageError> {
    let mut storage = Storage::open(path)?;
    storage.insert_files_batch(&[FileInfo {
        id,
        path: PathBuf::from(name),
        language: "rust".to_string(),
        modification_time: id,
        indexed: true,
        complete: true,
        line_count: 1,
        file_role: FileRole::Source,
    }])?;
    if publish {
        let publication = IndexPublicationRecord {
            generation: id.max(0) as u64,
            generation_id: format!("generation-{id}"),
            run_id: format!("run-{id}"),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: id.max(0),
        };
        storage.publish_structural_text_unit_generation(&publication)?;
        storage.put_index_publication(&publication)?;
        storage.publish_source_policy_exclusion_generation(
            &publication,
            "test-project",
            "test-workspace",
            source_policy_identity(
                OVERSIZED_SOURCE_POLICY_VERSION,
                DEFAULT_SOURCE_FILE_BYTE_CAP,
                codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            ),
            &[],
        )?;
    }
    storage.finalize_staged_snapshot()
}

fn seed_promotion_file(path: &Path, id: i64, name: &str) -> Result<(), StorageError> {
    seed_promotion_file_with_identity(path, id, name, true)
}

fn publish_bound_test_structural_cache(path: &Path) -> Result<(), StorageError> {
    let mut storage = Storage::open(path)?;
    let file = storage
        .get_files()?
        .into_iter()
        .next()
        .expect("promotion fixture file");
    let source_hash = format!("{:064x}", file.id);
    let producer = "test_structural_collector".to_string();
    let projection = StructuralTextProjection {
        file_id: file.id,
        source_content_hash: source_hash.clone(),
        descriptor_version: STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer,
        language: file.language.clone(),
        file_role: file.file_role,
        unit_count: 0,
        unit_digest: structural_text_unit_digest(&[]),
    };
    storage.flush_projection_batch(ProjectionBatch {
        files: std::slice::from_ref(&file),
        file_content_hashes: &[FileContentHash {
            file_id: file.id,
            content_hash: source_hash,
        }],
        nodes: &[],
        structural_text_units: &[],
        structural_text_projections: std::slice::from_ref(&projection),
        structural_text_cache_writes: &[StructuralTextArtifactCacheWrite {
            path: &file.path,
            file_id: file.id,
            cache_key: "v1:test",
            artifact_blob: b"verified structural cache",
        }],
        edges: &[],
        occurrences: &[],
        component_access: &[],
        callable_projection_states: &[],
        file_errors: &[],
    })?;
    let publication = storage
        .get_complete_index_publication()?
        .expect("promotion fixture publication");
    storage.publish_structural_text_unit_generation(&publication)?;
    storage.finalize_staged_snapshot()
}

fn corrupt_test_structural_cache(path: &Path, corruption: &str) -> Result<(), StorageError> {
    let connection = Connection::open(path)?;
    match corruption {
        "blob" => connection.execute(
            "UPDATE structural_text_artifact_cache SET artifact_blob = ?1",
            [b"corrupt blob".as_slice()],
        )?,
        "digest" => connection.execute(
            "UPDATE structural_text_artifact_cache SET artifact_digest = ?1",
            ["0".repeat(64)],
        )?,
        "key" => connection.execute(
            "UPDATE structural_text_artifact_cache SET cache_key = 'unversioned'",
            [],
        )?,
        "source" => connection.execute(
            "UPDATE structural_text_artifact_cache SET source_content_hash = ?1",
            ["f".repeat(64)],
        )?,
        "producer" => connection.execute(
            "UPDATE structural_text_artifact_cache SET producer = 'wrong-producer'",
            [],
        )?,
        "file" => connection.execute(
            "UPDATE structural_text_artifact_cache SET file_id = file_id + 1000",
            [],
        )?,
        _ => panic!("unknown structural cache corruption {corruption}"),
    };
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(())
}

fn copy_promotion_database_fixture(source: &Path, destination: &Path) -> Result<(), StorageError> {
    cleanup_sqlite_sidecars(destination)?;
    let source = Connection::open(source)?;
    source.backup(MAIN_DB, destination, None::<fn(rusqlite::backup::Progress)>)?;
    Ok(())
}

fn seed_disposable_promotion_file(path: &Path, id: i64, name: &str) -> Result<(), StorageError> {
    let mut storage = Storage::open_disposable_full_build(path)?;
    storage.insert_files_batch(&[FileInfo {
        id,
        path: PathBuf::from(name),
        language: "rust".to_string(),
        modification_time: id,
        indexed: true,
        complete: true,
        line_count: 1,
        file_role: FileRole::Source,
    }])?;
    let publication = IndexPublicationRecord {
        generation: id.max(0) as u64,
        generation_id: format!("generation-{id}"),
        run_id: format!("run-{id}"),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: id.max(0),
    };
    storage.publish_structural_text_unit_generation(&publication)?;
    storage.put_index_publication(&publication)?;
    storage.publish_source_policy_exclusion_generation(
        &publication,
        "test-project",
        "test-workspace",
        source_policy_identity(
            OVERSIZED_SOURCE_POLICY_VERSION,
            DEFAULT_SOURCE_FILE_BYTE_CAP,
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        ),
        &[OversizedSourceExclusionCandidate {
            normalized_path: format!("vendor/registers-{id}.h"),
            content_hash: format!("{:064x}", id.max(0)),
            observed_size: DEFAULT_SOURCE_FILE_BYTE_CAP + id.max(0) as u64,
            observed_unit_count: 0,
            policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
            byte_cap: DEFAULT_SOURCE_FILE_BYTE_CAP,
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        }],
    )?;
    storage.seal_disposable_full_build().map(|_| ())
}

fn seed_unpublished_file(path: &Path, id: i64, name: &str) -> Result<(), StorageError> {
    seed_promotion_file_with_identity(path, id, name, false)
}

fn publish_nonempty_test_source_policy(path: &Path, generation: u64) -> Result<(), StorageError> {
    let mut storage = Storage::open(path)?;
    let publication = storage
        .get_complete_index_publication()?
        .expect("seeded publication");
    storage.publish_source_policy_exclusion_generation(
        &publication,
        "test-project",
        "test-workspace",
        source_policy_identity(
            OVERSIZED_SOURCE_POLICY_VERSION,
            DEFAULT_SOURCE_FILE_BYTE_CAP,
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        ),
        &[OversizedSourceExclusionCandidate {
            normalized_path: format!("vendor/registers-{generation}.h"),
            content_hash: format!("{generation:064x}"),
            observed_size: DEFAULT_SOURCE_FILE_BYTE_CAP + generation,
            observed_unit_count: 0,
            policy_version: OVERSIZED_SOURCE_POLICY_VERSION.to_string(),
            byte_cap: DEFAULT_SOURCE_FILE_BYTE_CAP,
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
        }],
    )?;
    Ok(())
}

fn promotion_journal(
    previous_path: &Path,
    candidate_path: &Path,
) -> Result<PromotionJournal, StorageError> {
    let previous =
        read_recovery_database_identity(previous_path, RecoveryDatabaseContract::CurrentPromotion)?;
    let candidate = require_complete_promotion_database_identity(candidate_path, "Test candidate")?;
    Ok(PromotionJournal {
        version: PROMOTION_JOURNAL_VERSION,
        previous_source_policy: previous
            .as_ref()
            .map(|publication| {
                read_source_policy_exclusion_rollback_identity(previous_path, publication)
            })
            .transpose()?
            .flatten(),
        candidate_source_policy: read_source_policy_exclusion_rollback_identity(
            candidate_path,
            &candidate,
        )?,
        previous_structural_text: previous
            .as_ref()
            .map(|publication| {
                read_structural_text_unit_rollback_identity(previous_path, publication)
            })
            .transpose()?
            .flatten(),
        candidate_structural_text: read_structural_text_unit_rollback_identity(
            candidate_path,
            &candidate,
        )?,
        previous,
        candidate,
    })
}

fn promotion_journal_for_version(
    previous_path: &Path,
    candidate_path: &Path,
    version: u32,
) -> Result<PromotionJournal, StorageError> {
    let mut journal = promotion_journal(previous_path, candidate_path)?;
    journal.version = version;
    if version < SOURCE_POLICY_PROMOTION_JOURNAL_VERSION {
        journal.previous_source_policy = None;
        journal.candidate_source_policy = None;
    }
    if version < STRUCTURAL_TEXT_PROMOTION_JOURNAL_VERSION {
        journal.previous_structural_text = None;
        journal.candidate_structural_text = None;
    }
    Ok(journal)
}

fn restamp_complete_promotion_fixture(
    path: &Path,
    schema_version: u32,
) -> Result<(), StorageError> {
    let conn = Connection::open(path)?;
    if schema_version < STRUCTURAL_TEXT_PROMOTION_MIN_SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS structural_text_unit;
             DROP TABLE IF EXISTS structural_text_projection;
             DROP TABLE IF EXISTS structural_text_artifact_cache;
             DROP TABLE IF EXISTS structural_text_unit_publication;",
        )?;
    }
    if schema_version < SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS source_policy_exclusion;
             DROP TABLE IF EXISTS source_policy_exclusion_publication;",
        )?;
    }
    conn.execute("DELETE FROM incomplete_index_run", [])?;
    conn.pragma_update(None, "user_version", schema_version.to_string())?;
    Ok(())
}

fn downgrade_source_policy_fixture_to_v1(path: &Path) -> Result<(), StorageError> {
    let conn = Connection::open(path)?;
    let mut records = read_source_policy_exclusions(&conn)?;
    for record in &mut records {
        record.observed_unit_count = 0;
        record.policy_version = "oversized-source-v1".to_string();
        record.structural_unit_cap = codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP;
    }
    let legacy_digest = legacy_source_policy_exclusion_digest(&records);
    conn.execute_batch(
        "ALTER TABLE source_policy_exclusion RENAME TO source_policy_exclusion_v2;
         ALTER TABLE source_policy_exclusion_publication
            RENAME TO source_policy_exclusion_publication_v2;
         CREATE TABLE source_policy_exclusion (
            normalized_path TEXT PRIMARY KEY CHECK(length(normalized_path) > 0),
            project_id TEXT NOT NULL CHECK(length(project_id) > 0),
            workspace_id TEXT NOT NULL CHECK(length(workspace_id) > 0),
            content_hash TEXT NOT NULL CHECK(length(content_hash) = 64),
            observed_size INTEGER NOT NULL CHECK(observed_size > 0),
            policy_version TEXT NOT NULL CHECK(length(policy_version) > 0),
            byte_cap INTEGER NOT NULL CHECK(byte_cap > 0),
            core_generation_id TEXT NOT NULL CHECK(length(core_generation_id) > 0),
            core_run_id TEXT NOT NULL CHECK(length(core_run_id) > 0)
         );
         CREATE TABLE source_policy_exclusion_publication (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            schema_version INTEGER NOT NULL,
            complete INTEGER NOT NULL CHECK(complete = 1),
            project_id TEXT NOT NULL CHECK(length(project_id) > 0),
            workspace_id TEXT NOT NULL CHECK(length(workspace_id) > 0),
            core_generation_id TEXT NOT NULL CHECK(length(core_generation_id) > 0),
            core_run_id TEXT NOT NULL CHECK(length(core_run_id) > 0),
            exclusion_count INTEGER NOT NULL CHECK(exclusion_count >= 0),
            exclusion_digest TEXT NOT NULL CHECK(length(exclusion_digest) = 64),
            policy_version TEXT NOT NULL CHECK(length(policy_version) > 0),
            byte_cap INTEGER NOT NULL CHECK(byte_cap > 0),
            published_at_epoch_ms INTEGER NOT NULL CHECK(published_at_epoch_ms >= 0)
         );
         INSERT INTO source_policy_exclusion (
            normalized_path, project_id, workspace_id, content_hash, observed_size,
            policy_version, byte_cap, core_generation_id, core_run_id
         )
         SELECT normalized_path, project_id, workspace_id, content_hash, observed_size,
                'oversized-source-v1', byte_cap, core_generation_id, core_run_id
         FROM source_policy_exclusion_v2;
         INSERT INTO source_policy_exclusion_publication (
            id, schema_version, complete, project_id, workspace_id, core_generation_id,
            core_run_id, exclusion_count, exclusion_digest, policy_version, byte_cap,
            published_at_epoch_ms
         )
         SELECT id, 1, complete, project_id, workspace_id, core_generation_id,
                core_run_id, exclusion_count, exclusion_digest, 'oversized-source-v1',
                byte_cap, published_at_epoch_ms
         FROM source_policy_exclusion_publication_v2;
         DROP TABLE source_policy_exclusion_v2;
         DROP TABLE source_policy_exclusion_publication_v2;",
    )?;
    conn.execute(
        "UPDATE source_policy_exclusion_publication SET exclusion_digest = ?1",
        params![legacy_digest],
    )?;
    Ok(())
}

#[test]
fn recovery_schema_contracts_match_their_durable_journal_generations() {
    let current = RecoveryDatabaseContract::CurrentPromotion;
    let legacy_journal = RecoveryDatabaseContract::Journal(LEGACY_PROMOTION_JOURNAL_VERSION);
    let source_policy_journal =
        RecoveryDatabaseContract::Journal(SOURCE_POLICY_PROMOTION_JOURNAL_VERSION);
    let structural_journal =
        RecoveryDatabaseContract::Journal(STRUCTURAL_TEXT_PROMOTION_JOURNAL_VERSION);
    let structural_policy_journal =
        RecoveryDatabaseContract::Journal(STRUCTURAL_POLICY_PROMOTION_JOURNAL_VERSION);
    let semantic_projection_journal = RecoveryDatabaseContract::Journal(PROMOTION_JOURNAL_VERSION);
    let legacy_backup = RecoveryDatabaseContract::LegacyBackup;

    for schema_version in
        LEGACY_PROMOTION_MIN_SCHEMA_VERSION..=SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION
    {
        assert!(legacy_journal.supports_complete_schema(schema_version));
    }
    for schema_version in LEGACY_PROMOTION_MIN_SCHEMA_VERSION..=SCHEMA_VERSION {
        assert!(legacy_backup.supports_complete_schema(schema_version));
    }
    assert!(
        source_policy_journal.supports_complete_schema(SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION)
    );
    assert!(
        structural_journal.supports_complete_schema(STRUCTURAL_TEXT_PROMOTION_MIN_SCHEMA_VERSION)
    );
    assert!(
        structural_policy_journal
            .supports_complete_schema(STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION)
    );
    assert!(
        semantic_projection_journal
            .supports_complete_schema(STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION)
    );
    assert!(semantic_projection_journal.supports_complete_schema(SCHEMA_VERSION));
    assert!(current.supports_complete_schema(STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION));
    assert!(current.supports_complete_schema(SCHEMA_VERSION));
    assert!(legacy_journal.supports_complete_schema(SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION));
    assert!(!legacy_journal.supports_complete_schema(SCHEMA_VERSION));
    assert!(!source_policy_journal.supports_complete_schema(SCHEMA_VERSION));

    for (contract, rejected) in [
        (legacy_journal, LEGACY_PROMOTION_MIN_SCHEMA_VERSION - 1),
        (
            source_policy_journal,
            SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION - 1,
        ),
        (
            structural_journal,
            STRUCTURAL_TEXT_PROMOTION_MIN_SCHEMA_VERSION - 1,
        ),
        (
            structural_policy_journal,
            STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION - 1,
        ),
        (
            semantic_projection_journal,
            STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION - 1,
        ),
        (legacy_backup, LEGACY_PROMOTION_MIN_SCHEMA_VERSION - 1),
        (current, STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION - 1),
    ] {
        assert!(!contract.supports_complete_schema(rejected));
        assert!(!contract.supports_complete_schema(SCHEMA_VERSION + 1));
    }
    assert!(
        !RecoveryDatabaseContract::Journal(PROMOTION_JOURNAL_VERSION + 1)
            .supports_complete_schema(SCHEMA_VERSION)
    );
}

#[test]
fn legacy_journal_recovery_runs_before_supported_schema_migration() {
    for (label, journal_version, schema_version, committed, expected_generation) in [
        (
            "v1-schema21-prepared",
            LEGACY_PROMOTION_JOURNAL_VERSION,
            LEGACY_PROMOTION_MIN_SCHEMA_VERSION,
            false,
            1,
        ),
        (
            "v1-schema21-committed",
            LEGACY_PROMOTION_JOURNAL_VERSION,
            LEGACY_PROMOTION_MIN_SCHEMA_VERSION,
            true,
            2,
        ),
        (
            "v1-schema27-prepared",
            LEGACY_PROMOTION_JOURNAL_VERSION,
            SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION,
            false,
            1,
        ),
        (
            "v2-schema27-prepared",
            SOURCE_POLICY_PROMOTION_JOURNAL_VERSION,
            SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION,
            false,
            1,
        ),
        (
            "v2-schema27-committed",
            SOURCE_POLICY_PROMOTION_JOURNAL_VERSION,
            SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION,
            true,
            2,
        ),
        (
            "v3-schema28-prepared",
            STRUCTURAL_TEXT_PROMOTION_JOURNAL_VERSION,
            STRUCTURAL_TEXT_PROMOTION_MIN_SCHEMA_VERSION,
            false,
            1,
        ),
        (
            "v3-schema28-committed",
            STRUCTURAL_TEXT_PROMOTION_JOURNAL_VERSION,
            STRUCTURAL_TEXT_PROMOTION_MIN_SCHEMA_VERSION,
            true,
            2,
        ),
        (
            "v4-schema29-prepared",
            STRUCTURAL_POLICY_PROMOTION_JOURNAL_VERSION,
            STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION,
            false,
            1,
        ),
        (
            "v4-schema29-committed",
            STRUCTURAL_POLICY_PROMOTION_JOURNAL_VERSION,
            STRUCTURAL_POLICY_PROMOTION_MIN_SCHEMA_VERSION,
            true,
            2,
        ),
    ] {
        let live_path = unique_temp_db_path(label);
        let backup_path = live_path.with_extension("sqlite.backup");
        seed_promotion_file(&live_path, 2, "new.rs").expect("seed legacy live");
        seed_promotion_file(&backup_path, 1, "old.rs").expect("seed legacy backup");
        if journal_version == STRUCTURAL_TEXT_PROMOTION_JOURNAL_VERSION {
            downgrade_source_policy_fixture_to_v1(&live_path)
                .expect("downgrade live source policy fixture");
            downgrade_source_policy_fixture_to_v1(&backup_path)
                .expect("downgrade backup source policy fixture");
        }
        let journal = promotion_journal_for_version(&backup_path, &live_path, journal_version)
            .expect("build legacy journal");
        restamp_complete_promotion_fixture(&backup_path, schema_version)
            .expect("restamp legacy backup");
        restamp_complete_promotion_fixture(&live_path, schema_version)
            .expect("restamp legacy live");
        let journal_path = if committed {
            promotion_committed_journal_path(&live_path)
        } else {
            promotion_prepared_journal_path(&live_path)
        };
        write_promotion_journal(&journal_path, &journal).expect("write legacy journal");

        let raw = Connection::open_with_flags(&live_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open legacy database before recovery");
        let raw_schema: i64 = raw
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read legacy schema");
        assert_eq!(raw_schema as u32, schema_version);
        drop(raw);

        let recovered = Storage::open(&live_path).expect("recover then migrate legacy database");
        assert_eq!(
            recovered
                .get_complete_index_publication()
                .expect("read recovered publication")
                .expect("complete recovered publication")
                .generation,
            expected_generation
        );
        assert_eq!(
            recovered.get_files().expect("read recovered files")[0].path,
            PathBuf::from(if committed { "new.rs" } else { "old.rs" })
        );
        assert_eq!(
            recovered.schema_version().expect("read migrated schema"),
            SCHEMA_VERSION
        );
        drop(recovered);

        assert!(!journal_path.exists(), "recovery must consume its journal");
        assert!(!backup_path.exists(), "recovery must consume its backup");
        cleanup_sqlite_sidecars(&live_path).expect("clean recovered live fixture");
    }
}

#[test]
fn retained_v3_journal_deserializes_before_structural_policy_migration() {
    let live_path = unique_temp_db_path("v3-journal-deserialization-live");
    let backup_path = live_path.with_extension("sqlite.backup");
    seed_promotion_file(&live_path, 2, "new.rs").expect("seed candidate");
    seed_promotion_file(&backup_path, 1, "old.rs").expect("seed previous");
    let journal = promotion_journal_for_version(
        &backup_path,
        &live_path,
        STRUCTURAL_TEXT_PROMOTION_JOURNAL_VERSION,
    )
    .expect("build v3 journal");
    let mut value = serde_json::to_value(journal).expect("serialize v3 journal");
    for identity in ["previous_source_policy", "candidate_source_policy"] {
        value[identity]
            .as_object_mut()
            .expect("source policy identity")
            .remove("structural_unit_cap");
    }
    let decoded: PromotionJournal =
        serde_json::from_value(value).expect("deserialize retained v3 journal");
    assert_eq!(
        decoded
            .candidate_source_policy
            .expect("candidate source policy")
            .structural_unit_cap,
        codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP
    );
    cleanup_sqlite_sidecars(&backup_path).expect("clean previous fixture");
    cleanup_sqlite_sidecars(&live_path).expect("clean candidate fixture");
}

#[test]
fn schema_21_legacy_backup_recovers_before_migration() {
    let live_path = unique_temp_db_path("schema21-legacy-backup-live");
    let backup_path = live_path.with_extension("sqlite.backup");
    seed_promotion_file(&backup_path, 1, "old.rs").expect("seed schema 21 backup");
    restamp_complete_promotion_fixture(&backup_path, LEGACY_PROMOTION_MIN_SCHEMA_VERSION)
        .expect("restamp schema 21 backup");

    let recovered = Storage::open(&live_path).expect("recover schema 21 legacy backup");
    assert_eq!(
        recovered
            .get_complete_index_publication()
            .expect("read recovered publication")
            .expect("complete recovered publication")
            .generation,
        1
    );
    assert_eq!(
        recovered.schema_version().expect("read migrated schema"),
        SCHEMA_VERSION
    );
    drop(recovered);

    assert!(
        !backup_path.exists(),
        "legacy recovery must consume its backup"
    );
    cleanup_sqlite_sidecars(&live_path).expect("clean recovered legacy fixture");
}

#[test]
fn schema_28_journal_less_backup_requires_complete_auxiliary_publications() {
    let live_path = unique_temp_db_path("schema28-journal-less-valid-live");
    let backup_path = live_path.with_extension("sqlite.backup");
    seed_promotion_file(&backup_path, 1, "old.rs").expect("seed schema 28 backup");
    publish_bound_test_structural_cache(&backup_path).expect("bind schema 28 structural cache");

    let recovered = Storage::open(&live_path).expect("recover valid schema 28 backup");
    let publication = recovered
        .get_complete_index_publication()
        .expect("read recovered publication")
        .expect("complete recovered publication");
    recovered
        .validate_source_policy_exclusion_publication(
            &publication,
            "test-project",
            "test-workspace",
            source_policy_identity(
                OVERSIZED_SOURCE_POLICY_VERSION,
                DEFAULT_SOURCE_FILE_BYTE_CAP,
                codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            ),
        )
        .expect("validate recovered source policy publication");
    recovered
        .validate_structural_text_unit_publication(&publication)
        .expect("validate recovered structural publication");
    assert_eq!(
        recovered
            .get_structural_text_artifact_cache(Path::new("old.rs"), "v1:test")
            .expect("read recovered structural cache"),
        Some(b"verified structural cache".to_vec())
    );
    drop(recovered);
    assert!(
        !backup_path.exists(),
        "valid recovery must consume its backup"
    );
    cleanup_sqlite_sidecars(&live_path).expect("clean recovered schema 28 fixture");
}

#[test]
fn schema_28_journal_less_backup_protects_against_invalid_live_auxiliary_state() {
    for live_state in ["same-corrupt", "newer-corrupt"] {
        let live_path = unique_temp_db_path(&format!("schema28-journal-less-{live_state}-live"));
        let backup_path = live_path.with_extension("sqlite.backup");
        if live_state == "newer-corrupt" {
            seed_promotion_file(&live_path, 2, "new.rs").expect("seed newer live publication");
            publish_bound_test_structural_cache(&live_path)
                .expect("bind newer live structural cache");
        }
        seed_promotion_file(&backup_path, 1, "old.rs").expect("seed valid schema 28 backup");
        publish_bound_test_structural_cache(&backup_path)
            .expect("bind valid backup structural cache");
        if live_state == "same-corrupt" {
            copy_promotion_database_fixture(&backup_path, &live_path)
                .expect("copy same-identity live fixture");
        }
        corrupt_test_structural_cache(&live_path, "blob").expect("corrupt live structural cache");

        if live_state == "same-corrupt" {
            let recovered = Storage::open(&live_path).expect("restore valid same-identity backup");
            let publication = recovered
                .get_complete_index_publication()
                .expect("read restored publication")
                .expect("complete restored publication");
            recovered
                .validate_structural_text_unit_publication(&publication)
                .expect("restored structural publication");
            assert_eq!(
                recovered
                    .get_structural_text_artifact_cache(Path::new("old.rs"), "v1:test")
                    .expect("read restored cache"),
                Some(b"verified structural cache".to_vec())
            );
            drop(recovered);
            assert!(
                !backup_path.exists(),
                "successful same-identity restore retained its backup"
            );
        } else {
            let error = match Storage::open(&live_path) {
                Ok(_) => panic!("newer corrupt live publication passed journal-less recovery"),
                Err(error) => error,
            };
            assert!(
                error
                    .to_string()
                    .to_ascii_lowercase()
                    .contains("structural artifact cache"),
                "unexpected newer-live recovery error: {error}"
            );
            assert!(
                backup_path.exists(),
                "invalid newer live publication destroyed the valid backup"
            );
            let live = Connection::open_with_flags(&live_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open retained newer live fixture");
            let generation: String = live
                .query_row(
                    "SELECT generation_id FROM index_publication WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .expect("read retained newer live generation");
            assert_eq!(generation, "generation-2");
        }

        cleanup_sqlite_sidecars(&backup_path).expect("clean protected backup");
        cleanup_sqlite_sidecars(&live_path).expect("clean protected live");
    }
}

#[test]
fn schema_28_journal_less_backup_rejects_missing_or_corrupt_auxiliary_state() {
    for corruption in [
        "source-policy-missing",
        "source-policy-corrupt",
        "structural-missing",
        "structural-corrupt",
        "cache-missing",
        "cache-corrupt",
    ] {
        let live_path = unique_temp_db_path(&format!("schema28-journal-less-{corruption}-live"));
        let backup_path = live_path.with_extension("sqlite.backup");
        seed_promotion_file(&backup_path, 1, "old.rs").expect("seed schema 28 backup");
        publish_bound_test_structural_cache(&backup_path).expect("bind schema 28 structural cache");
        let connection = Connection::open(&backup_path).expect("open schema 28 backup");
        match corruption {
            "source-policy-missing" => connection
                .execute("DELETE FROM source_policy_exclusion_publication", [])
                .expect("remove source policy manifest"),
            "source-policy-corrupt" => connection
                .execute(
                    "UPDATE source_policy_exclusion_publication
                     SET exclusion_digest = ?1",
                    ["0".repeat(64)],
                )
                .expect("corrupt source policy manifest"),
            "structural-missing" => connection
                .execute("DELETE FROM structural_text_unit_publication", [])
                .expect("remove structural manifest"),
            "structural-corrupt" => connection
                .execute(
                    "UPDATE structural_text_unit_publication SET unit_digest = ?1",
                    ["0".repeat(64)],
                )
                .expect("corrupt structural manifest"),
            "cache-missing" => connection
                .execute("DROP TABLE structural_text_artifact_cache", [])
                .expect("remove structural cache table"),
            "cache-corrupt" => connection
                .execute(
                    "UPDATE structural_text_artifact_cache SET artifact_blob = ?1",
                    [b"corrupt cache".as_slice()],
                )
                .expect("corrupt structural cache"),
            _ => unreachable!(),
        };
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint corrupt schema 28 backup");
        drop(connection);

        let error = match Storage::open(&live_path) {
            Ok(_) => panic!("schema 28 recovery accepted {corruption}"),
            Err(error) => error,
        };
        assert!(
            error.to_string().to_ascii_lowercase().contains(
                if corruption.starts_with("source-policy") {
                    "source policy"
                } else if corruption.starts_with("cache") {
                    "structural artifact cache"
                } else {
                    "structural text unit"
                }
            ),
            "unexpected {corruption} recovery error: {error}"
        );
        assert!(
            backup_path.exists(),
            "{corruption} recovery consumed its backup"
        );
        assert!(
            !live_path.exists(),
            "{corruption} recovery installed an invalid backup"
        );

        cleanup_sqlite_sidecars(&backup_path).expect("clean rejected schema 28 backup");
        cleanup_sqlite_sidecars(&live_path).expect("clean rejected schema 28 live");
    }
}

#[test]
fn schema_27_journal_less_backup_allows_absent_policy_but_rejects_corrupt_present_policy() {
    for policy_state in ["absent", "corrupt"] {
        let live_path = unique_temp_db_path(&format!("schema27-journal-less-{policy_state}-live"));
        let backup_path = live_path.with_extension("sqlite.backup");
        seed_promotion_file(&backup_path, 1, "old.rs").expect("seed schema 27 backup");
        let connection = Connection::open(&backup_path).expect("open schema 27 backup");
        if policy_state == "absent" {
            connection
                .execute("DELETE FROM source_policy_exclusion_publication", [])
                .expect("remove optional schema 27 policy manifest");
        } else {
            connection
                .execute(
                    "UPDATE source_policy_exclusion_publication
                     SET exclusion_digest = ?1",
                    ["0".repeat(64)],
                )
                .expect("corrupt schema 27 policy manifest");
        }
        drop(connection);
        restamp_complete_promotion_fixture(
            &backup_path,
            SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION,
        )
        .expect("restamp schema 27 backup");

        if policy_state == "absent" {
            let recovered =
                Storage::open(&live_path).expect("recover schema 27 backup without policy");
            assert_eq!(
                recovered
                    .get_complete_index_publication()
                    .expect("read schema 27 publication")
                    .expect("complete schema 27 publication")
                    .generation,
                1
            );
            assert!(
                recovered
                    .get_source_policy_exclusion_manifest()
                    .expect("read optional schema 27 policy")
                    .is_none()
            );
            drop(recovered);
            assert!(!backup_path.exists());
        } else {
            let error = match Storage::open(&live_path) {
                Ok(_) => panic!("schema 27 recovery accepted corrupt present policy"),
                Err(error) => error,
            };
            assert!(
                error
                    .to_string()
                    .to_ascii_lowercase()
                    .contains("source policy"),
                "unexpected schema 27 policy error: {error}"
            );
            assert!(backup_path.exists());
            assert!(!live_path.exists());
        }

        cleanup_sqlite_sidecars(&backup_path).expect("clean schema 27 backup");
        cleanup_sqlite_sidecars(&live_path).expect("clean schema 27 live");
    }
}

#[test]
fn promotion_recovery_rejects_unsupported_and_unmarked_schema_identities() {
    for (label, journal_version, schema_version, expected_error) in [
        (
            "unsupported-old-v1",
            LEGACY_PROMOTION_JOURNAL_VERSION,
            LEGACY_PROMOTION_MIN_SCHEMA_VERSION - 1,
            "unsupported schema version",
        ),
        (
            "invalid-v1-schema28",
            LEGACY_PROMOTION_JOURNAL_VERSION,
            SCHEMA_VERSION,
            "unsupported schema version",
        ),
        (
            "invalid-v2-schema28",
            SOURCE_POLICY_PROMOTION_JOURNAL_VERSION,
            SCHEMA_VERSION,
            "unsupported schema version",
        ),
        (
            "unsupported-future-v3",
            PROMOTION_JOURNAL_VERSION,
            SCHEMA_VERSION + 1,
            "unsupported schema version",
        ),
        (
            "unmarked-incomplete-v2",
            SOURCE_POLICY_PROMOTION_JOURNAL_VERSION,
            INCOMPLETE_INCREMENTAL_SCHEMA_VERSION,
            "without its marker",
        ),
    ] {
        let live_path = unique_temp_db_path(label);
        let backup_path = live_path.with_extension("sqlite.backup");
        let prepared_path = promotion_prepared_journal_path(&live_path);
        seed_promotion_file(&live_path, 2, "new.rs").expect("seed recovery live");
        seed_promotion_file(&backup_path, 1, "old.rs").expect("seed recovery backup");
        let journal = promotion_journal_for_version(&backup_path, &live_path, journal_version)
            .expect("build recovery journal");
        restamp_complete_promotion_fixture(&live_path, schema_version)
            .expect("stamp rejected recovery schema");
        write_promotion_journal(&prepared_path, &journal).expect("write recovery journal");

        let error = match Storage::open(&live_path) {
            Ok(_) => panic!("unsupported recovery schema must fail closed"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains(expected_error),
            "unexpected recovery error for {label}: {error}"
        );
        assert!(
            prepared_path.exists(),
            "failed recovery must retain journal"
        );
        assert!(backup_path.exists(), "failed recovery must retain backup");

        std::fs::remove_file(&prepared_path).expect("remove rejected journal");
        cleanup_sqlite_sidecars(&backup_path).expect("clean rejected backup");
        cleanup_sqlite_sidecars(&live_path).expect("clean rejected live");
    }
}

#[test]
fn promotion_journal_binds_source_policy_exclusion_count_and_digest() -> Result<(), StorageError> {
    let previous_path = unique_temp_db_path("promotion-policy-previous");
    let candidate_path = unique_temp_db_path("promotion-policy-candidate");
    seed_promotion_file(&previous_path, 1, "old")?;
    seed_promotion_file(&candidate_path, 2, "new")?;

    for (path, generation) in [(&previous_path, 1_u64), (&candidate_path, 2_u64)] {
        let mut storage = Storage::open(path)?;
        let publication = storage
            .get_complete_index_publication()?
            .expect("seeded publication");
        storage.publish_source_policy_exclusion_generation(
            &publication,
            "project",
            "workspace",
            source_policy_identity(
                "oversized-source-v1",
                1_000_000,
                codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            ),
            &[OversizedSourceExclusionCandidate {
                normalized_path: format!("vendor/registers-{generation}.h"),
                content_hash: format!("{generation:x}").repeat(64),
                observed_size: 1_000_000 + generation,
                observed_unit_count: 0,
                policy_version: "oversized-source-v1".into(),
                byte_cap: 1_000_000,
                structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            }],
        )?;
    }

    let journal = promotion_journal(&previous_path, &candidate_path)?;
    let previous = journal
        .previous_source_policy
        .expect("previous exclusion rollback identity");
    let candidate = journal
        .candidate_source_policy
        .expect("candidate exclusion rollback identity");
    assert_eq!(previous.exclusion_count, 1);
    assert_eq!(candidate.exclusion_count, 1);
    assert_eq!(previous.core_published_at_epoch_ms, 1);
    assert_eq!(candidate.core_published_at_epoch_ms, 2);
    assert_ne!(previous.exclusion_digest, candidate.exclusion_digest);

    cleanup_sqlite_sidecars(&previous_path)?;
    cleanup_sqlite_sidecars(&candidate_path)?;
    Ok(())
}

#[test]
fn staged_promotion_rejects_missing_corrupt_or_timestamp_drifted_candidate_manifest() {
    for corruption in ["missing", "digest", "timestamp"] {
        let live_path = unique_temp_db_path(&format!("promotion-policy-live-{corruption}"));
        let staged_path = unique_temp_db_path(&format!("promotion-policy-staged-{corruption}"));
        seed_promotion_file(&live_path, 1, "old.rs").expect("seed live publication");
        seed_promotion_file(&staged_path, 2, "new.rs").expect("seed staged publication");
        let staged = Storage::open(&staged_path).expect("open staged publication");
        match corruption {
            "missing" => {
                staged
                    .get_connection()
                    .execute("DELETE FROM source_policy_exclusion_publication", [])
                    .expect("remove candidate manifest");
            }
            "digest" => {
                staged
                    .get_connection()
                    .execute(
                        "UPDATE source_policy_exclusion_publication SET exclusion_digest = ?1",
                        params!["0".repeat(64)],
                    )
                    .expect("corrupt candidate digest");
            }
            "timestamp" => {
                staged
                    .get_connection()
                    .execute(
                        "UPDATE source_policy_exclusion_publication SET published_at_epoch_ms = published_at_epoch_ms + 1",
                        [],
                    )
                    .expect("drift candidate timestamp");
            }
            _ => unreachable!(),
        }
        drop(staged);

        let error = Storage::promote_staged_snapshot(&staged_path, &live_path)
            .expect_err("invalid candidate manifest must block promotion");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("source policy exclusion"),
            "unexpected promotion error: {error}"
        );
        let live = Storage::open(&live_path).expect("reopen preserved live publication");
        assert_eq!(
            live.get_complete_index_publication()
                .expect("live publication")
                .expect("complete live publication")
                .generation_id,
            "generation-1"
        );
        assert_eq!(
            live.get_files().expect("live files")[0].path,
            PathBuf::from("old.rs")
        );

        cleanup_sqlite_sidecars(&live_path).expect("clean live fixture");
        cleanup_sqlite_sidecars(&staged_path).expect("clean staged fixture");
    }
}

#[test]
fn staged_promotion_rejects_missing_corrupt_or_drifted_structural_manifest() {
    for corruption in ["missing", "digest", "timestamp"] {
        let live_path = unique_temp_db_path(&format!("promotion-structural-live-{corruption}"));
        let staged_path = unique_temp_db_path(&format!("promotion-structural-staged-{corruption}"));
        seed_promotion_file(&live_path, 1, "old.rs").expect("seed live publication");
        seed_promotion_file(&staged_path, 2, "new.rs").expect("seed staged publication");
        let staged = Storage::open(&staged_path).expect("open staged publication");
        match corruption {
            "missing" => {
                staged
                    .get_connection()
                    .execute("DELETE FROM structural_text_unit_publication", [])
                    .expect("remove structural manifest");
            }
            "digest" => {
                staged
                    .get_connection()
                    .execute(
                        "UPDATE structural_text_unit_publication SET unit_digest = ?1",
                        params!["0".repeat(64)],
                    )
                    .expect("corrupt structural digest");
            }
            "timestamp" => {
                staged
                    .get_connection()
                    .execute(
                        "UPDATE structural_text_unit_publication
                         SET published_at_epoch_ms = published_at_epoch_ms + 1",
                        [],
                    )
                    .expect("drift structural timestamp");
            }
            _ => unreachable!(),
        }
        drop(staged);

        let error = Storage::promote_staged_snapshot(&staged_path, &live_path)
            .expect_err("invalid structural manifest must block promotion");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("structural text unit"),
            "unexpected promotion error: {error}"
        );
        let live = Storage::open(&live_path).expect("reopen preserved live publication");
        assert_eq!(
            live.get_complete_index_publication()
                .expect("live publication")
                .expect("complete live publication")
                .generation_id,
            "generation-1"
        );
        assert_eq!(
            live.get_files().expect("live files")[0].path,
            PathBuf::from("old.rs")
        );

        cleanup_sqlite_sidecars(&live_path).expect("clean live fixture");
        cleanup_sqlite_sidecars(&staged_path).expect("clean staged fixture");
    }
}

#[test]
fn staged_promotion_rejects_every_corrupt_structural_cache_binding() {
    for corruption in ["blob", "digest", "key", "source", "producer", "file"] {
        let live_path =
            unique_temp_db_path(&format!("promotion-structural-cache-live-{corruption}"));
        let staged_path =
            unique_temp_db_path(&format!("promotion-structural-cache-staged-{corruption}"));
        let backup_path = live_path.with_extension("sqlite.backup");
        let prepared_path = promotion_prepared_journal_path(&live_path);
        let committed_path = promotion_committed_journal_path(&live_path);
        seed_promotion_file(&live_path, 1, "old.rs").expect("seed live publication");
        publish_bound_test_structural_cache(&live_path).expect("bind live structural cache");
        seed_promotion_file(&staged_path, 2, "new.rs").expect("seed staged publication");
        publish_bound_test_structural_cache(&staged_path).expect("bind staged structural cache");
        corrupt_test_structural_cache(&staged_path, corruption)
            .expect("corrupt staged structural cache");

        let error = Storage::promote_staged_snapshot(&staged_path, &live_path)
            .expect_err("corrupt structural cache must block promotion");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("structural artifact cache"),
            "unexpected {corruption} promotion error: {error}"
        );
        let live = Storage::open(&live_path).expect("reopen preserved live publication");
        let publication = live
            .get_complete_index_publication()
            .expect("read live publication")
            .expect("complete live publication");
        assert_eq!(publication.generation_id, "generation-1");
        live.validate_structural_text_unit_publication(&publication)
            .expect("preserved live structural publication");
        assert_eq!(
            live.get_files().expect("live files")[0].path,
            PathBuf::from("old.rs")
        );
        assert!(staged_path.exists(), "rejected candidate remains retryable");
        assert!(
            !backup_path.exists(),
            "candidate rejection created a backup"
        );
        assert!(
            !prepared_path.exists(),
            "candidate rejection created a prepared journal"
        );
        assert!(
            !committed_path.exists(),
            "candidate rejection created a committed journal"
        );
        drop(live);

        cleanup_sqlite_sidecars(&live_path).expect("clean live fixture");
        cleanup_sqlite_sidecars(&staged_path).expect("clean staged fixture");
    }
}

#[test]
fn prepared_recovery_rejects_corrupt_previous_and_backup_structural_caches() {
    for corruption_role in ["previous-live", "backup"] {
        let live_path = unique_temp_db_path(&format!("prepared-cache-{corruption_role}-live"));
        let staged_path = unique_temp_db_path(&format!("prepared-cache-{corruption_role}-staged"));
        let backup_path = live_path.with_extension("sqlite.backup");
        let prepared_path = promotion_prepared_journal_path(&live_path);
        seed_promotion_file(&live_path, 1, "old.rs").expect("seed previous publication");
        publish_bound_test_structural_cache(&live_path).expect("bind previous structural cache");
        seed_promotion_file(&staged_path, 2, "new.rs").expect("seed candidate publication");
        publish_bound_test_structural_cache(&staged_path).expect("bind candidate structural cache");
        copy_promotion_database_fixture(&live_path, &backup_path).expect("copy previous backup");
        let journal =
            promotion_journal(&backup_path, &staged_path).expect("build prepared journal");

        if corruption_role == "previous-live" {
            corrupt_test_structural_cache(&live_path, "blob").expect("corrupt previous live cache");
        } else {
            corrupt_test_structural_cache(&backup_path, "blob")
                .expect("corrupt previous backup cache");
            copy_promotion_database_fixture(&staged_path, &live_path)
                .expect("install candidate before prepared recovery");
        }
        write_promotion_journal(&prepared_path, &journal).expect("write prepared journal");

        let error = match Storage::open(&live_path) {
            Ok(_) => panic!("prepared recovery accepted corrupt structural cache"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("structural artifact cache"),
            "unexpected {corruption_role} recovery error: {error}"
        );
        assert!(
            prepared_path.exists(),
            "failed prepared recovery consumed its journal"
        );
        assert!(
            backup_path.exists(),
            "failed prepared recovery consumed its backup"
        );
        let live = Connection::open_with_flags(&live_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open failed-recovery live database");
        let live_file: String = live
            .query_row("SELECT path FROM file ORDER BY id LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("read failed-recovery live file");
        assert_eq!(
            live_file,
            if corruption_role == "previous-live" {
                "old.rs"
            } else {
                "new.rs"
            }
        );
        drop(live);

        std::fs::remove_file(&prepared_path).expect("remove prepared journal");
        cleanup_sqlite_sidecars(&backup_path).expect("clean prepared backup");
        cleanup_sqlite_sidecars(&staged_path).expect("clean prepared candidate");
        cleanup_sqlite_sidecars(&live_path).expect("clean prepared live");
    }
}

#[test]
fn committed_recovery_rejects_corrupt_candidate_structural_cache() {
    let live_path = unique_temp_db_path("committed-cache-live");
    let staged_path = unique_temp_db_path("committed-cache-staged");
    let backup_path = live_path.with_extension("sqlite.backup");
    let committed_path = promotion_committed_journal_path(&live_path);
    seed_promotion_file(&live_path, 1, "old.rs").expect("seed previous publication");
    publish_bound_test_structural_cache(&live_path).expect("bind previous structural cache");
    seed_promotion_file(&staged_path, 2, "new.rs").expect("seed candidate publication");
    publish_bound_test_structural_cache(&staged_path).expect("bind candidate structural cache");
    copy_promotion_database_fixture(&live_path, &backup_path).expect("copy previous backup");
    let journal = promotion_journal(&backup_path, &staged_path).expect("build committed journal");
    copy_promotion_database_fixture(&staged_path, &live_path)
        .expect("install committed candidate fixture");
    corrupt_test_structural_cache(&live_path, "blob").expect("corrupt committed candidate cache");
    write_promotion_journal(&committed_path, &journal).expect("write committed journal");

    let error = match Storage::open(&live_path) {
        Ok(_) => panic!("committed recovery accepted corrupt structural cache"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("structural artifact cache"),
        "unexpected committed recovery error: {error}"
    );
    assert!(
        committed_path.exists(),
        "failed committed recovery consumed its journal"
    );
    assert!(
        backup_path.exists(),
        "failed committed recovery consumed its backup"
    );
    let live = Connection::open_with_flags(&live_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open corrupt committed live database");
    let live_file: String = live
        .query_row("SELECT path FROM file ORDER BY id LIMIT 1", [], |row| {
            row.get(0)
        })
        .expect("read corrupt committed live file");
    assert_eq!(live_file, "new.rs");
    drop(live);

    std::fs::remove_file(&committed_path).expect("remove committed journal");
    cleanup_sqlite_sidecars(&backup_path).expect("clean committed backup");
    cleanup_sqlite_sidecars(&staged_path).expect("clean committed candidate");
    cleanup_sqlite_sidecars(&live_path).expect("clean committed live");
}

#[test]
fn legacy_committed_journal_without_source_policy_identity_recovers_for_runtime_repair() {
    let live_path = unique_temp_db_path("legacy-committed-policy-live");
    seed_promotion_file(&live_path, 1, "legacy.rs").expect("seed legacy live publication");
    let live = Storage::open(&live_path).expect("open legacy live publication");
    let candidate = live
        .get_complete_index_publication()
        .expect("read legacy publication")
        .expect("complete legacy publication");
    live.get_connection()
        .execute("DELETE FROM source_policy_exclusion_publication", [])
        .expect("remove post-v27 policy identity from legacy fixture");
    drop(live);
    restamp_complete_promotion_fixture(&live_path, SOURCE_POLICY_PROMOTION_MIN_SCHEMA_VERSION)
        .expect("restore the schema 27 journal-v1 producer shape");

    let committed_path = promotion_committed_journal_path(&live_path);
    write_promotion_journal(
        &committed_path,
        &PromotionJournal {
            version: LEGACY_PROMOTION_JOURNAL_VERSION,
            previous: None,
            candidate: candidate.clone(),
            previous_source_policy: None,
            candidate_source_policy: None,
            previous_structural_text: None,
            candidate_structural_text: None,
        },
    )
    .expect("write legacy committed journal");

    let recovered = Storage::open(&live_path).expect("recover legacy committed promotion");
    assert_eq!(
        recovered
            .get_complete_index_publication()
            .expect("recovered publication"),
        Some(candidate)
    );
    assert!(
        recovered
            .get_source_policy_exclusion_manifest()
            .expect("legacy policy manifest")
            .is_none(),
        "store recovery must not synthesize policy evidence"
    );
    assert!(!committed_path.exists());

    cleanup_sqlite_sidecars(&live_path).expect("clean legacy fixture");
}

#[test]
fn staged_promotion_abort_child() {
    let Some(live_path) = std::env::var_os(PROMOTION_ABORT_LIVE_ENV).map(PathBuf::from) else {
        return;
    };
    let staged_path =
        PathBuf::from(std::env::var_os(PROMOTION_ABORT_STAGED_ENV).expect("child staged path"));
    let result = Storage::promote_staged_snapshot(&staged_path, &live_path);
    panic!("promotion abort hook returned: {result:?}");
}

#[test]
fn staged_promotion_abort_recovers_old_or_complete_new_and_cleans_artifacts() {
    let live_path = unique_temp_db_path("promotion-abort-live");
    let staged_path = unique_temp_db_path("promotion-abort-staged");
    let sentinel_path = unique_temp_db_path("promotion-abort-sentinel");
    let backup_path = live_path.with_extension("sqlite.backup");
    let prepared_path = promotion_prepared_journal_path(&live_path);
    let committed_path = promotion_committed_journal_path(&live_path);
    seed_promotion_file(&live_path, 1, "old.rs").expect("seed live generation");
    seed_disposable_promotion_file(&staged_path, 2, "new.rs")
        .expect("seed sealed disposable staged generation");
    publish_nonempty_test_source_policy(&live_path, 1).expect("publish live exclusion identity");

    let status =
        std::process::Command::new(std::env::current_exe().expect("resolve store test executable"))
            .arg("--exact")
            .arg("storage_impl::tests::staged_promotion_abort_child")
            .arg("--nocapture")
            .env(PROMOTION_ABORT_LIVE_ENV, &live_path)
            .env(PROMOTION_ABORT_STAGED_ENV, &staged_path)
            .env(PROMOTION_ABORT_SENTINEL_ENV, &sentinel_path)
            .status()
            .expect("run promotion abort child");
    assert!(
        !status.success(),
        "promotion abort child exited successfully"
    );
    assert_eq!(
        std::fs::read(&sentinel_path).expect("read promotion abort sentinel"),
        PROMOTION_ABORT_SENTINEL,
        "ordinary child failure must not satisfy the crash proof"
    );

    let interrupted = Connection::open_with_flags(&live_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open interrupted live generation without recovery");
    let interrupted_path: String = interrupted
        .query_row("SELECT path FROM file ORDER BY id LIMIT 1", [], |row| {
            row.get(0)
        })
        .expect("read interrupted live generation");
    assert_eq!(
        interrupted_path, "new.rs",
        "abort hook must run after the live database mutation"
    );
    drop(interrupted);

    let live = Storage::open(&live_path).expect("open live generation after abort");
    assert_eq!(
        live.get_files().expect("read live generation")[0].path,
        PathBuf::from("old.rs")
    );
    assert_eq!(
        live.get_source_policy_exclusions()
            .expect("read rolled-back exclusions")[0]
            .normalized_path,
        "vendor/registers-1.h"
    );
    drop(live);
    assert!(
        staged_path.exists(),
        "staged generation must remain retryable"
    );
    assert!(
        !backup_path.exists(),
        "opening live storage must consume the recovery backup"
    );
    assert!(!prepared_path.exists(), "rollback must consume its journal");
    assert!(!committed_path.exists(), "aborted promotion cannot commit");

    let retry_stats = Storage::promote_staged_snapshot(&staged_path, &live_path)
        .expect("retry promotion after abort");
    assert_core_promotion_stats_reconcile(&retry_stats);
    assert!(retry_stats.previous_live_bytes.is_some());
    assert!(retry_stats.rollback_backup_copy_ms.is_some());
    assert!(retry_stats.backup_validation_ms.is_some());
    assert_eq!(
        retry_stats.rollback_backup_bytes,
        retry_stats.previous_live_bytes
    );
    let live = Storage::open(&live_path).expect("open recovered live generation");
    assert_eq!(
        live.get_files().expect("read recovered generation")[0].path,
        PathBuf::from("new.rs")
    );
    assert_eq!(
        live.get_source_policy_exclusions()
            .expect("read promoted exclusions")[0]
            .normalized_path,
        "vendor/registers-2.h"
    );
    drop(live);
    for artifact in sqlite_sidecar_paths(&staged_path)
        .into_iter()
        .chain(sqlite_sidecar_paths(&backup_path))
    {
        assert!(
            !artifact.exists(),
            "successful retry left promotion artifact {}",
            artifact.display()
        );
    }

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&staged_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);
    let _ = std::fs::remove_file(prepared_path);
    let _ = std::fs::remove_file(committed_path);
    let _ = std::fs::remove_file(&sentinel_path);
}

#[test]
fn retained_committed_promotion_stays_live_and_blocks_the_next_writer() {
    let live_path = unique_temp_db_path("promotion-cleanup-failure-live");
    let staged_path = unique_temp_db_path("promotion-cleanup-failure-staged");
    let second_staged_path = unique_temp_db_path("promotion-cleanup-failure-second-staged");
    let backup_path = live_path.with_extension("sqlite.backup");
    let committed_path = promotion_committed_journal_path(&live_path);
    let cleanup_failure_path = promotion_cleanup_failure_path(&live_path);
    seed_promotion_file(&live_path, 1, "old.rs").expect("seed live generation");
    seed_promotion_file(&staged_path, 2, "new.rs").expect("seed staged generation");
    seed_promotion_file(&second_staged_path, 3, "newer.rs").expect("seed second staged generation");
    publish_nonempty_test_source_policy(&live_path, 1).expect("publish live exclusion identity");
    publish_nonempty_test_source_policy(&staged_path, 2)
        .expect("publish staged exclusion identity");
    publish_nonempty_test_source_policy(&second_staged_path, 3)
        .expect("publish second staged exclusion identity");
    std::fs::write(&cleanup_failure_path, b"blocked").expect("inject cleanup failure");

    let committed_stats = Storage::promote_staged_snapshot(&staged_path, &live_path)
        .expect("committed promotion tolerates deferred cleanup");
    assert_core_promotion_stats_reconcile(&committed_stats);
    assert!(committed_stats.previous_live_bytes.is_some());
    assert!(committed_stats.rollback_backup_copy_ms.is_some());
    assert!(committed_stats.backup_validation_ms.is_some());
    assert_eq!(
        committed_stats.rollback_backup_bytes,
        committed_stats.previous_live_bytes
    );
    let error = Storage::promote_staged_snapshot(&second_staged_path, &live_path)
        .expect_err("retained committed artifacts must block the next promotion");
    assert!(error.to_string().contains("prior artifacts remain"));
    assert!(backup_path.exists() && committed_path.exists());
    assert!(second_staged_path.exists());

    std::fs::remove_file(&cleanup_failure_path).expect("restore cleanup");
    let reopened = Storage::open(&live_path).expect("reopen committed live generation");
    assert_eq!(
        reopened.get_files().expect("read committed generation")[0].path,
        PathBuf::from("new.rs")
    );
    assert_eq!(
        reopened
            .get_source_policy_exclusions()
            .expect("read committed exclusions")[0]
            .normalized_path,
        "vendor/registers-2.h"
    );
    drop(reopened);
    assert!(!backup_path.exists() && !committed_path.exists());

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&staged_path);
    let _ = cleanup_sqlite_sidecars(&second_staged_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);
}

#[test]
fn prepared_promotion_refuses_to_overwrite_an_unrelated_newer_live_publication() {
    let live_path = unique_temp_db_path("prepared-newer-live");
    let candidate_path = unique_temp_db_path("prepared-newer-candidate");
    let backup_path = live_path.with_extension("sqlite.backup");
    let prepared_path = promotion_prepared_journal_path(&live_path);
    seed_promotion_file(&live_path, 3, "newer.rs").expect("seed unrelated newer live");
    seed_promotion_file(&backup_path, 1, "old.rs").expect("seed previous backup");
    seed_promotion_file(&candidate_path, 2, "candidate.rs").expect("seed candidate");
    let journal = promotion_journal(&backup_path, &candidate_path).expect("build journal");
    write_promotion_journal(&prepared_path, &journal).expect("write prepared journal");

    let error = match Storage::open(&live_path) {
        Ok(_) => panic!("prepared recovery must reject an unrelated live publication"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("unrelated live publication"),
        "unexpected prepared recovery error: {error}"
    );
    assert!(
        prepared_path.exists(),
        "failed-closed recovery keeps its journal"
    );
    assert!(
        backup_path.exists(),
        "failed-closed recovery keeps its backup"
    );

    std::fs::remove_file(&prepared_path).expect("remove prepared journal");
    cleanup_sqlite_sidecars(&backup_path).expect("remove previous backup");
    let live = Storage::open(&live_path).expect("reopen untouched newer live");
    assert_eq!(
        live.get_files().expect("read newer live")[0].path,
        PathBuf::from("newer.rs")
    );
    drop(live);

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&candidate_path);
}

#[test]
fn publicationless_promotion_state_is_ambiguous_and_fails_closed() {
    let live_path = unique_temp_db_path("publicationless-live");
    let backup_path = live_path.with_extension("sqlite.backup");
    let staged_path = unique_temp_db_path("publicationless-staged");
    seed_unpublished_file(&live_path, 1, "live.rs").expect("seed unpublished live");
    seed_unpublished_file(&backup_path, 2, "backup.rs").expect("seed unpublished backup");

    let error = match Storage::open(&live_path) {
        Ok(_) => panic!("publicationless legacy backup cannot prove recovery identity"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("no complete publication identity"),
        "unexpected publicationless recovery error: {error}"
    );
    assert!(backup_path.exists(), "ambiguous backup must be retained");

    cleanup_sqlite_sidecars(&backup_path).expect("remove ambiguous backup");
    seed_unpublished_file(&staged_path, 3, "staged.rs").expect("seed unpublished candidate");
    let error = Storage::promote_staged_snapshot(&staged_path, &live_path)
        .expect_err("promotion requires a complete candidate publication");
    assert!(
        error
            .to_string()
            .contains("no complete publication identity"),
        "unexpected unpublished candidate error: {error}"
    );
    let live = Storage::open(&live_path).expect("reopen untouched unpublished live");
    assert_eq!(
        live.get_files().expect("read untouched live")[0].path,
        PathBuf::from("live.rs")
    );
    drop(live);

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&staged_path);
}

#[test]
fn legacy_backup_never_overwrites_a_newer_complete_publication() {
    let live_path = unique_temp_db_path("newer-legacy-live");
    let backup_path = live_path.with_extension("sqlite.backup");
    seed_promotion_file(&live_path, 2, "new.rs").expect("seed newer live generation");
    seed_promotion_file(&backup_path, 1, "old.rs").expect("seed older backup generation");

    let live = Storage::open(&live_path).expect("open newer live generation");
    assert_eq!(
        live.get_files().expect("read newer live generation")[0].path,
        PathBuf::from("new.rs")
    );
    drop(live);
    assert!(!backup_path.exists(), "older backup should be cleaned");

    let _ = cleanup_sqlite_sidecars(&live_path);
    let _ = cleanup_sqlite_sidecars(&backup_path);
}

#[test]
fn invalid_legacy_backup_fails_closed_without_overwriting_live() {
    let live_path = unique_temp_db_path("invalid-legacy-backup-live");
    let backup_path = live_path.with_extension("sqlite.backup");
    seed_promotion_file(&live_path, 2, "new.rs").expect("seed live generation");
    std::fs::write(&backup_path, b"not a sqlite database").expect("write invalid backup");

    let error = match Storage::open(&live_path) {
        Ok(_) => panic!("invalid backup must fail closed"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("database") || error.to_string().contains("SQLite"),
        "unexpected recovery error: {error}"
    );
    std::fs::remove_file(&backup_path).expect("remove invalid backup");
    let live = Storage::open(&live_path).expect("reopen untouched live generation");
    assert_eq!(
        live.get_files().expect("read untouched live generation")[0].path,
        PathBuf::from("new.rs")
    );

    drop(live);
    let _ = cleanup_sqlite_sidecars(&live_path);
}

#[test]
fn test_resolution_query_plan_prefers_new_indexes() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    let mut node_plan_stmt = storage.conn.prepare(
            "EXPLAIN QUERY PLAN SELECT id FROM node WHERE kind IN (3, 11, 12) AND serialized_name = 'foo' LIMIT 1",
        )?;
    let node_plan = node_plan_stmt
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        node_plan
            .iter()
            .any(|line| line.contains("idx_node_kind_serialized_name"))
    );

    let mut edge_plan_stmt = storage.conn.prepare(
            "EXPLAIN QUERY PLAN SELECT COUNT(*) FROM edge WHERE kind = 3 AND resolved_target_node_id IS NULL",
        )?;
    let edge_plan = edge_plan_stmt
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        edge_plan
            .iter()
            .any(|line| line.contains("idx_edge_kind_resolved_target"))
    );

    Ok(())
}

#[test]
fn semantic_context_endpoint_indexes_replace_edge_scan_without_other_deferred_indexes()
-> Result<(), StorageError> {
    const ENDPOINT_INDEXES: &[&str] = &[
        "idx_edge_source",
        "idx_edge_target",
        "idx_edge_resolved_source",
        "idx_edge_resolved_target",
    ];
    const UNRELATED_DEFERRED_INDEXES: &[&str] = &[
        "idx_edge_file",
        "idx_edge_kind_source",
        "idx_node_file",
        "idx_occurrence_element",
    ];
    const REPRESENTATIVE_NODE_COUNT: i64 = 12_000;
    const REPRESENTATIVE_EDGE_COUNT: i64 = 48_000;

    let db_path = unique_temp_db_path("semantic-endpoint-indexes");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let storage = Storage::open_build(&db_path)?;
    storage.conn.execute_batch(&format!(
        "WITH RECURSIVE sequence(value) AS (
             SELECT 1
             UNION ALL
             SELECT value + 1 FROM sequence WHERE value < {REPRESENTATIVE_NODE_COUNT}
         )
         INSERT INTO node(id, kind, serialized_name)
         SELECT value, 3, printf('node-%d', value) FROM sequence;
         WITH RECURSIVE sequence(value) AS (
             SELECT 1
             UNION ALL
             SELECT value + 1 FROM sequence WHERE value < {REPRESENTATIVE_EDGE_COUNT}
         )
         INSERT INTO edge(
             id,
             source_node_id,
             target_node_id,
             kind,
             resolved_source_node_id,
             resolved_target_node_id
         )
         SELECT
             value,
             (value % {REPRESENTATIVE_NODE_COUNT}) + 1,
             ((value * 17) % {REPRESENTATIVE_NODE_COUNT}) + 1,
             2,
             ((value * 19) % {REPRESENTATIVE_NODE_COUNT}) + 1,
             ((value * 23) % {REPRESENTATIVE_NODE_COUNT}) + 1
         FROM sequence;"
    ))?;

    for index_name in ENDPOINT_INDEXES
        .iter()
        .chain(UNRELATED_DEFERRED_INDEXES.iter())
    {
        assert!(!sqlite_index_exists(&storage, index_name)?);
    }

    let plan_sql = format!(
        "EXPLAIN QUERY PLAN
         {EDGE_SELECT_BASE}
         WHERE e.source_node_id IN (?1)
            OR e.target_node_id IN (?2)
            OR e.resolved_source_node_id IN (?3)
            OR e.resolved_target_node_id IN (?4)
         ORDER BY e.id"
    );
    let plan = |storage: &Storage| -> Result<Vec<String>, StorageError> {
        let mut statement = storage.conn.prepare(&plan_sql)?;
        statement
            .query_map(rusqlite::params![17_i64, 17_i64, 17_i64, 17_i64], |row| {
                row.get(3)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)
    };
    let scan_plan = plan(&storage)?;
    assert!(
        scan_plan.iter().any(|line| line.contains("SCAN e")),
        "semantic endpoint lookup did not scan before early indexes: {scan_plan:?}"
    );

    let node_ids = [NodeId(17), NodeId(311), NodeId(1_021), NodeId(5_099)];
    let scan_callbacks = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let scan_counter = std::sync::Arc::clone(&scan_callbacks);
    storage.conn.progress_handler(
        100,
        Some(move || {
            scan_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        }),
    )?;
    let scan_started = std::time::Instant::now();
    let scan_edges = storage.get_edges_for_node_ids(&node_ids)?;
    let scan_elapsed = scan_started.elapsed();
    storage.conn.progress_handler(0, None::<fn() -> bool>)?;

    storage.create_semantic_context_endpoint_indexes_for_build()?;
    for index_name in ENDPOINT_INDEXES {
        assert!(sqlite_index_exists(&storage, index_name)?);
    }
    for index_name in UNRELATED_DEFERRED_INDEXES {
        assert!(!sqlite_index_exists(&storage, index_name)?);
    }

    let indexed_plan = plan(&storage)?;
    for index_name in ENDPOINT_INDEXES {
        assert!(
            indexed_plan.iter().any(|line| line.contains(index_name)),
            "semantic endpoint lookup did not use {index_name}: {indexed_plan:?}"
        );
    }
    assert!(
        indexed_plan.iter().all(|line| !line.contains("SCAN e")),
        "semantic endpoint lookup still scanned after early indexes: {indexed_plan:?}"
    );

    let indexed_callbacks = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let indexed_counter = std::sync::Arc::clone(&indexed_callbacks);
    storage.conn.progress_handler(
        100,
        Some(move || {
            indexed_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        }),
    )?;
    let indexed_started = std::time::Instant::now();
    let indexed_edges = storage.get_edges_for_node_ids(&node_ids)?;
    let indexed_elapsed = indexed_started.elapsed();
    storage.conn.progress_handler(0, None::<fn() -> bool>)?;

    assert_eq!(indexed_edges, scan_edges);
    for edges in indexed_edges.values() {
        assert!(
            edges.windows(2).all(|pair| pair[0].id < pair[1].id),
            "semantic endpoint results lost deterministic edge-id order"
        );
    }
    let scan_callback_count = scan_callbacks.load(std::sync::atomic::Ordering::Relaxed);
    let indexed_callback_count = indexed_callbacks.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        scan_callback_count > indexed_callback_count.saturating_mul(5),
        "representative endpoint lookup VM work did not improve enough: scan={scan_callback_count}, indexed={indexed_callback_count}"
    );
    eprintln!(
        "semantic endpoint representative proof: nodes={REPRESENTATIVE_NODE_COUNT} edges={REPRESENTATIVE_EDGE_COUNT} scan_callbacks={scan_callback_count} indexed_callbacks={indexed_callback_count} scan_ms={} indexed_ms={}",
        scan_elapsed.as_millis(),
        indexed_elapsed.as_millis(),
    );

    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;
    Ok(())
}

#[test]
fn staged_summary_build_uses_bulk_node_file_rank_index_for_file_aggregation()
-> Result<(), StorageError> {
    const DESTINATION_INDEXES: &[&str] = &[
        "idx_grounding_file_snapshot_path",
        "idx_grounding_file_snapshot_rank",
        "idx_grounding_node_snapshot_file_rank",
        "idx_grounding_node_snapshot_root_rank",
    ];

    let db_path = unique_temp_db_path("summary-index-phases");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let mut storage = Storage::open_build(&db_path)?;
    storage.insert_files_batch(&[FileInfo {
        id: 1,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 5,
        file_role: FileRole::Source,
    }])?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(1),
            ..Default::default()
        },
    ])?;

    storage.prepare_deferred_secondary_indexes_for_summary()?;
    assert!(sqlite_index_exists(&storage, "idx_node_file")?);
    for index_name in DESTINATION_INDEXES {
        assert!(!sqlite_index_exists(&storage, index_name)?);
    }

    let node_snapshot_plan_sql = format!(
        "EXPLAIN QUERY PLAN {}",
        grounding_node_snapshot_insert_sql()
    );
    let mut node_plan_stmt = storage.conn.prepare(&node_snapshot_plan_sql)?;
    let node_plan = node_plan_stmt
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        node_plan
            .iter()
            .any(|line| line.contains("SCAN ranked_nodes")),
        "node snapshot did not use the narrow ranking coroutine: {node_plan:?}"
    );
    assert!(
        node_plan.iter().any(|line| {
            line.contains("SEARCH n USING INTEGER PRIMARY KEY")
                || line.contains("SEARCH n USING COVERING INDEX")
        }),
        "ranked node rows did not rejoin through a source-node index: {node_plan:?}"
    );
    assert!(
        node_plan.iter().any(|line| line.contains("LIST SUBQUERY")),
        "MEMBER targets were not materialized through one indexed list query: {node_plan:?}"
    );
    assert!(
        node_plan
            .iter()
            .any(|line| line.contains("idx_edge_kind_target")),
        "MEMBER-target root classification lost its covering lookup: {node_plan:?}"
    );
    assert!(
        node_plan.iter().all(|line| !line.contains("CORRELATED")),
        "MEMBER-target root classification regressed to per-node probes: {node_plan:?}"
    );
    assert!(
        node_plan.iter().all(|line| !line.contains("AUTOMATIC")),
        "node snapshot built an automatic duplicate index: {node_plan:?}"
    );
    assert!(
        node_plan
            .iter()
            .all(|line| !line.contains("grounding_node_snapshot")),
        "node snapshot maintained a destination index during insertion: {node_plan:?}"
    );
    drop(node_plan_stmt);

    storage.refresh_grounding_summary_snapshots_for_staged_finalize()?;
    assert!(sqlite_index_exists(
        &storage,
        "idx_grounding_node_snapshot_file_rank"
    )?);
    for index_name in [
        "idx_grounding_file_snapshot_path",
        "idx_grounding_file_snapshot_rank",
        "idx_grounding_node_snapshot_root_rank",
    ] {
        assert!(!sqlite_index_exists(&storage, index_name)?);
    }
    assert!(storage.has_ready_grounding_summary_snapshots()?);
    assert_eq!(storage.get_grounding_file_summary_count()?, 1);

    let file_snapshot_plan_sql = format!(
        "EXPLAIN QUERY PLAN\n{}\n{}",
        grounding_file_snapshot_cte_sql(),
        GROUNDING_FILE_SNAPSHOT_SELECT_SQL,
    );
    let mut plan_stmt = storage.conn.prepare(&file_snapshot_plan_sql)?;
    let plan = plan_stmt
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert!(
        plan.iter()
            .any(|line| line.contains("idx_grounding_node_snapshot_file_rank")),
        "file-summary join did not use the persistent file-rank index: {plan:?}"
    );
    assert!(
        plan.iter().all(|line| !line.contains("AUTOMATIC")),
        "file-summary join built an automatic index: {plan:?}"
    );
    drop(plan_stmt);

    storage.complete_deferred_secondary_indexes_after_summary()?;
    for index_name in DESTINATION_INDEXES {
        assert!(sqlite_index_exists(&storage, index_name)?);
    }

    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;
    Ok(())
}

#[test]
fn legacy_staged_finalize_builds_complete_secondary_index_set() -> Result<(), StorageError> {
    let db_path = unique_temp_db_path("legacy-summary-finalize");
    let _ = cleanup_sqlite_sidecars(&db_path);
    let mut storage = Storage::open_build(&db_path)?;
    storage.insert_files_batch(&[FileInfo {
        id: 1,
        path: PathBuf::from("legacy.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 1,
        file_role: FileRole::Source,
    }])?;

    storage.finalize_staged_snapshot()?;

    assert!(storage.has_ready_grounding_summary_snapshots()?);
    for index_name in [
        "idx_node_file",
        "idx_edge_source",
        "idx_edge_target",
        "idx_edge_resolved_source",
        "idx_edge_resolved_target",
        "idx_grounding_file_snapshot_path",
        "idx_grounding_file_snapshot_rank",
        "idx_grounding_node_snapshot_file_rank",
        "idx_grounding_node_snapshot_root_rank",
    ] {
        assert!(sqlite_index_exists(&storage, index_name)?);
    }

    drop(storage);
    cleanup_sqlite_sidecars(&db_path)?;
    Ok(())
}

#[test]
fn test_occurrence_insert() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let nodes = vec![
        Node {
            id: NodeId(10),
            kind: NodeKind::FILE,
            serialized_name: "file.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(11),
            kind: NodeKind::FUNCTION,
            serialized_name: "foo".to_string(),
            ..Default::default()
        },
    ];
    storage.insert_nodes_batch(&nodes)?;
    let occurrences = vec![Occurrence {
        element_id: 11,
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: NodeId(10),
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 10,
        },
    }];
    storage.insert_occurrences_batch(&occurrences)?;
    let mut stmt = storage.conn.prepare("SELECT count(*) FROM occurrence")?;
    let count: i64 = stmt.query_row([], |row| row.get(0))?;
    assert_eq!(count, 1);
    Ok(())
}

#[test]
fn test_file_storage() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let info = FileInfo {
        id: 1,
        path: PathBuf::from("src/main.rs"),
        language: "rust".to_string(),
        modification_time: 12345678,
        indexed: true,
        complete: true,
        line_count: 100,
        file_role: FileRole::Source,
    };
    storage.insert_file(&info)?;
    let files = storage.get_files()?;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, PathBuf::from("src/main.rs"));
    assert_eq!(files[0].line_count, 100);
    Ok(())
}

#[test]
fn batched_nodes_and_occurrences_match_single_node_lookup() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "src/main.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(10),
            ..Default::default()
        },
    ])?;
    storage.insert_occurrences_batch(&[Occurrence {
        element_id: NodeId(2).0,
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: NodeId(1),
            start_line: 10,
            start_col: 0,
            end_line: 10,
            end_col: 4,
        },
    }])?;

    let batched_nodes = storage.get_nodes_by_ids(&[NodeId(1), NodeId(2)])?;
    assert_eq!(batched_nodes.len(), 2);
    assert_eq!(
        batched_nodes
            .get(&NodeId(2))
            .map(|node| node.serialized_name.as_str()),
        Some("run")
    );

    let batched_occurrences = storage.get_occurrences_for_node_ids(&[NodeId(2)])?;
    assert_eq!(
        batched_occurrences.get(&NodeId(2)).map(|occs| occs.len()),
        Some(1)
    );
    assert_eq!(
        storage
            .get_occurrences_for_node(NodeId(2))?
            .first()
            .map(|occ| occ.location.start_line),
        Some(10)
    );
    Ok(())
}

#[test]
fn batched_edges_for_node_ids_matches_single_node_lookup() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "callee".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::METHOD,
            serialized_name: "resolved".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(4),
            kind: NodeKind::CLASS,
            serialized_name: "Owner".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(4),
            target: NodeId(3),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
        Edge {
            id: EdgeId(3),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            resolved_target: Some(NodeId(3)),
            certainty: Some(ResolutionCertainty::Certain),
            confidence: Some(1.0),
            ..Default::default()
        },
    ])?;

    let node_ids = [NodeId(1), NodeId(2), NodeId(3), NodeId(4)];
    let batched = storage.get_edges_for_node_ids(&node_ids)?;
    for node_id in node_ids {
        let single_edge_ids = storage
            .get_edges_for_node_id(node_id)?
            .into_iter()
            .map(|edge| edge.id)
            .collect::<Vec<_>>();
        let batched_edge_ids = batched
            .get(&node_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|edge| edge.id)
            .collect::<Vec<_>>();
        assert_eq!(
            batched_edge_ids, single_edge_ids,
            "batched lookup should match single-node lookup for {node_id:?}"
        );
    }

    Ok(())
}

#[test]
fn test_error_storage_round_trips_coverage_reason() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let info = FileInfo {
        id: 1,
        path: PathBuf::from("src/main.rs"),
        language: "rust".to_string(),
        modification_time: 12345678,
        indexed: true,
        complete: true,
        line_count: 100,
        file_role: FileRole::Source,
    };
    storage.insert_file(&info)?;
    let error = codestory_contracts::graph::ErrorInfo {
        message: "Syntax error".to_string(),
        file_id: Some(NodeId(1)),
        line: Some(10),
        column: Some(5),
        is_fatal: true,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
        coverage_reason: Some(FileCoverageReason::CollectorFailure),
    };
    storage.insert_error(&error)?;
    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "Recoverable parse warning".to_string(),
        file_id: Some(NodeId(1)),
        line: Some(20),
        column: Some(1),
        is_fatal: false,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
        coverage_reason: None,
    })?;
    for (message, reason) in [
        ("Malformed structural source", FileCoverageReason::Malformed),
        ("Binary structural source", FileCoverageReason::Binary),
    ] {
        storage.insert_error(&codestory_contracts::graph::ErrorInfo {
            message: message.to_string(),
            file_id: Some(NodeId(1)),
            line: None,
            column: None,
            is_fatal: false,
            index_step: codestory_contracts::graph::IndexStep::Indexing,
            coverage_reason: Some(reason),
        })?;
    }
    let stats = storage.get_stats()?;
    assert_eq!(stats.error_count, 4);
    assert_eq!(stats.fatal_error_count, 1);
    let errors = storage.get_errors(None)?;
    let syntax_error = errors
        .iter()
        .find(|error| error.message == "Syntax error")
        .expect("syntax error");
    let warning = errors
        .iter()
        .find(|error| error.message == "Recoverable parse warning")
        .expect("recoverable warning");
    assert_eq!(
        syntax_error.coverage_reason,
        Some(FileCoverageReason::CollectorFailure)
    );
    assert!(errors.iter().any(|error| {
        error.message == "Malformed structural source"
            && error.coverage_reason == Some(FileCoverageReason::Malformed)
    }));
    assert!(errors.iter().any(|error| {
        error.message == "Binary structural source"
            && error.coverage_reason == Some(FileCoverageReason::Binary)
    }));
    assert_eq!(warning.coverage_reason, None);
    storage.refresh_grounding_summary_snapshots()?;
    assert!(storage.has_ready_grounding_summary_snapshots()?);
    let snapshot_stats = storage.get_stats()?;
    assert_eq!(snapshot_stats.error_count, 4);
    assert_eq!(snapshot_stats.fatal_error_count, 1);
    Ok(())
}

#[test]
fn test_node_cache() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;
    let node = Node {
        id: NodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "test_node".to_string(),
        ..Default::default()
    };
    storage.insert_node(&node)?;
    {
        let cache = storage.cache.nodes.read();
        assert!(cache.contains_key(&NodeId(1)));
    }
    let fetched = storage.get_node(NodeId(1))?.unwrap();
    assert_eq!(fetched.serialized_name, "test_node");
    Ok(())
}

#[test]
fn test_delete_file_projection() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_node_id = 1_234_i64;
    let file_node = Node {
        id: NodeId(file_node_id),
        kind: NodeKind::FILE,
        serialized_name: "src/main.rs".to_string(),
        start_line: Some(1),
        start_col: Some(1),
        end_line: Some(3),
        end_col: Some(1),
        ..Default::default()
    };
    let func_node = Node {
        id: NodeId(2_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "foo".to_string(),
        file_node_id: Some(NodeId(file_node_id)),
        start_line: Some(1),
        start_col: Some(1),
        end_line: Some(1),
        end_col: Some(20),
        ..Default::default()
    };
    storage.insert_file(&FileInfo {
        id: file_node_id,
        path: PathBuf::from("src/main.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;
    storage.insert_nodes_batch(&[file_node.clone(), func_node.clone()])?;

    storage.insert_edges_batch(&[Edge {
        id: EdgeId(9_001),
        source: file_node.id,
        target: func_node.id,
        kind: EdgeKind::MEMBER,
        file_node_id: Some(file_node.id),
        ..Default::default()
    }])?;

    storage.insert_occurrences_batch(&[Occurrence {
        element_id: func_node.id.0,
        kind: codestory_contracts::graph::OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_node.id,
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 3,
        },
    }])?;

    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "test".to_string(),
        file_id: Some(file_node.id),
        line: Some(1),
        column: None,
        is_fatal: false,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
        coverage_reason: None,
    })?;
    storage.upsert_llm_symbol_docs_batch(&[LlmSymbolDoc {
        node_id: func_node.id,
        file_node_id: Some(file_node.id),
        kind: NodeKind::FUNCTION,
        display_name: "foo".to_string(),
        qualified_name: None,
        file_path: Some("src/main.rs".to_string()),
        start_line: Some(1),
        doc_text: "foo symbol".to_string(),
        doc_version: 2,
        doc_hash: "semantic-hash-foo".to_string(),
        embedding_profile: None,
        embedding_model: "local-hash-384".to_string(),
        embedding_backend: None,
        embedding_dim: 384,
        doc_shape: None,
        semantic_policy_version: Some("graph_first_v1".to_string()),
        dense_reason: Some("public_api".to_string()),
        embedding: vec![0.1_f32; 384],
        updated_at_epoch_ms: 1,
    }])?;
    storage.upsert_symbol_summaries_batch(&[SymbolSummaryRecord {
        node_id: func_node.id,
        content_hash: "semantic-hash-foo".to_string(),
        summary: "foo symbol summary".to_string(),
        model: "test-model".to_string(),
        updated_at_epoch_ms: 2,
    }])?;
    storage.upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
        node_id: func_node.id,
        display_name: "foo".to_string(),
    }])?;
    storage.upsert_callable_projection_states(&[CallableProjectionState {
        file_id: file_node_id,
        symbol_key: "src/main.rs::foo:FUNCTION".to_string(),
        node_id: func_node.id,
        signature_hash: 111,
        body_hash: 211,
        start_line: 1,
        end_line: 1,
    }])?;

    let category_id = storage.create_bookmark_category("Cat")?;
    let _ = storage.add_bookmark(category_id, func_node.id, Some("test"))?;

    let summary = storage.delete_file_projection(file_node_id)?;
    assert_eq!(summary.canonical_file_node_id, file_node_id);
    assert_eq!(summary.removed_node_count, 2);
    assert_eq!(summary.removed_edge_count, 1);
    assert_eq!(summary.removed_occurrence_count, 1);
    assert_eq!(summary.removed_error_count, 1);
    assert_eq!(summary.removed_file_row_count, 1);
    assert_eq!(summary.removed_callable_projection_state_count, 1);

    assert!(storage.get_nodes()?.is_empty());
    assert!(storage.get_edges()?.is_empty());
    assert!(storage.get_occurrences()?.is_empty());
    assert!(storage.get_all_llm_symbol_docs()?.is_empty());
    assert_eq!(storage.get_search_symbol_projection_count()?, 0);
    let symbol_summary_count: i64 =
        storage
            .conn
            .query_row("SELECT count(*) FROM symbol_summary", [], |row| row.get(0))?;
    assert_eq!(symbol_summary_count, 0);
    assert!(
        storage
            .get_callable_projection_states_for_file(file_node_id)?
            .is_empty()
    );
    assert!(storage.get_errors(None)?.is_empty());
    assert!(storage.get_bookmarks(Some(category_id))?.is_empty());

    let cache = storage.cache.nodes.read();
    assert!(!cache.contains_key(&NodeId(file_node_id)));
    assert!(!cache.contains_key(&NodeId(2_001)));

    Ok(())
}

#[test]
fn test_delete_file_projection_preserves_cross_file_edges_and_clears_resolution()
-> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;
    let file_a_id = 1_001_i64;
    let file_b_id = 2_001_i64;

    storage.insert_file(&FileInfo {
        id: file_a_id,
        path: PathBuf::from("src/a.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;
    storage.insert_file(&FileInfo {
        id: file_b_id,
        path: PathBuf::from("src/b.rs"),
        language: "rust".to_string(),
        modification_time: 1,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;

    let file_a = Node {
        id: NodeId(file_a_id),
        kind: NodeKind::FILE,
        serialized_name: "src/a.rs".to_string(),
        ..Default::default()
    };
    let file_b = Node {
        id: NodeId(file_b_id),
        kind: NodeKind::FILE,
        serialized_name: "src/b.rs".to_string(),
        ..Default::default()
    };
    let caller_in_a = Node {
        id: NodeId(10_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "caller".to_string(),
        file_node_id: Some(file_a.id),
        ..Default::default()
    };
    let unresolved_in_a = Node {
        id: NodeId(10_002),
        kind: NodeKind::FUNCTION,
        serialized_name: "callee".to_string(),
        file_node_id: Some(file_a.id),
        ..Default::default()
    };
    let callee_in_b = Node {
        id: NodeId(20_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "callee".to_string(),
        file_node_id: Some(file_b.id),
        ..Default::default()
    };
    storage.insert_nodes_batch(&[
        file_a.clone(),
        file_b.clone(),
        caller_in_a.clone(),
        unresolved_in_a.clone(),
        callee_in_b.clone(),
    ])?;

    storage.insert_edges_batch(&[Edge {
        id: EdgeId(30_001),
        source: caller_in_a.id,
        target: unresolved_in_a.id,
        kind: EdgeKind::CALL,
        file_node_id: Some(file_a.id),
        resolved_target: Some(callee_in_b.id),
        confidence: Some(0.91),
        certainty: Some(codestory_contracts::graph::ResolutionCertainty::Certain),
        candidate_targets: vec![callee_in_b.id],
        ..Default::default()
    }])?;

    storage.upsert_callable_projection_states(&[
        CallableProjectionState {
            file_id: file_a_id,
            symbol_key: "src/a.rs::caller:FUNCTION".to_string(),
            node_id: caller_in_a.id,
            signature_hash: 111,
            body_hash: 211,
            start_line: 1,
            end_line: 2,
        },
        CallableProjectionState {
            file_id: file_a_id,
            symbol_key: "src/a.rs::stale-callee:FUNCTION".to_string(),
            node_id: callee_in_b.id,
            signature_hash: 112,
            body_hash: 212,
            start_line: 3,
            end_line: 4,
        },
    ])?;

    let summary = storage.delete_file_projection(file_b_id)?;
    assert_eq!(summary.canonical_file_node_id, file_b_id);
    assert_eq!(summary.removed_node_count, 2);
    assert_eq!(summary.removed_edge_count, 0);
    assert_eq!(summary.removed_callable_projection_state_count, 1);

    let edges = storage.get_edges()?;
    assert_eq!(edges.len(), 1);
    let edge = &edges[0];
    assert_eq!(edge.source, caller_in_a.id);
    assert_eq!(edge.target, unresolved_in_a.id);
    assert_eq!(edge.file_node_id, Some(file_a.id));
    assert_eq!(edge.resolved_target, None);
    assert_eq!(edge.confidence, None);
    assert_eq!(edge.certainty, None);
    assert!(edge.candidate_targets.is_empty());

    assert!(storage.get_node(file_b.id)?.is_none());
    assert!(storage.get_node(callee_in_b.id)?.is_none());
    assert!(storage.get_node(caller_in_a.id)?.is_some());
    let remaining_states = storage.get_callable_projection_states_for_file(file_a_id)?;
    assert_eq!(remaining_states.len(), 1);
    assert_eq!(remaining_states[0].node_id, caller_in_a.id);

    Ok(())
}

#[test]
fn test_bookmark_crud() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    // Create category
    let cat_id = storage.create_bookmark_category("Favorites")?;
    assert!(cat_id > 0);

    // Get categories
    let categories = storage.get_bookmark_categories()?;
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].name, "Favorites");

    // Create node for bookmark
    let node = Node {
        id: NodeId(100),
        kind: NodeKind::FUNCTION,
        serialized_name: "my_function".to_string(),
        ..Default::default()
    };
    storage.insert_node(&node)?;

    // Add bookmark
    let bm_id = storage.add_bookmark(cat_id, NodeId(100), Some("Important function"))?;
    assert!(bm_id > 0);

    // Get bookmarks
    let bookmarks = storage.get_bookmarks(Some(cat_id))?;
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].node_id, NodeId(100));
    assert_eq!(bookmarks[0].comment, Some("Important function".to_string()));

    // Update comment
    storage.update_bookmark_comment(bm_id, "Updated comment")?;
    let bookmarks = storage.get_bookmarks(Some(cat_id))?;
    assert_eq!(bookmarks[0].comment, Some("Updated comment".to_string()));

    // Delete bookmark
    storage.delete_bookmark(bm_id)?;
    let bookmarks = storage.get_bookmarks(Some(cat_id))?;
    assert_eq!(bookmarks.len(), 0);

    // Delete category
    storage.delete_bookmark_category(cat_id)?;
    let categories = storage.get_bookmark_categories()?;
    assert_eq!(categories.len(), 0);

    Ok(())
}

#[test]
fn test_update_bookmark_tri_state_comment_patch() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    let category_id = storage.create_bookmark_category("General")?;
    storage.insert_node(&Node {
        id: NodeId(300),
        kind: NodeKind::FUNCTION,
        serialized_name: "tri_state_target".to_string(),
        ..Default::default()
    })?;
    let bookmark_id = storage.add_bookmark(category_id, NodeId(300), Some("initial"))?;

    // Omitted comment keeps existing value.
    storage.update_bookmark(bookmark_id, None, None)?;
    let mut bookmarks = storage.get_bookmarks(Some(category_id))?;
    assert_eq!(bookmarks.remove(0).comment.as_deref(), Some("initial"));

    // Explicit null clears the comment.
    storage.update_bookmark(bookmark_id, None, Some(None))?;
    let mut bookmarks = storage.get_bookmarks(Some(category_id))?;
    assert_eq!(bookmarks.remove(0).comment, None);

    // Explicit value sets the comment.
    storage.update_bookmark(bookmark_id, None, Some(Some("updated")))?;
    let mut bookmarks = storage.get_bookmarks(Some(category_id))?;
    assert_eq!(bookmarks.remove(0).comment.as_deref(), Some("updated"));

    Ok(())
}

#[test]
fn test_get_errors() -> Result<(), StorageError> {
    let storage = Storage::new_in_memory()?;

    // Insert errors
    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "Fatal error".to_string(),
        file_id: None,
        line: Some(10),
        column: None,
        is_fatal: true,
        index_step: codestory_contracts::graph::IndexStep::Indexing,
        coverage_reason: Some(FileCoverageReason::SourceChanged),
    })?;
    storage.insert_error(&codestory_contracts::graph::ErrorInfo {
        message: "Warning".to_string(),
        file_id: None,
        line: Some(20),
        column: None,
        is_fatal: false,
        index_step: codestory_contracts::graph::IndexStep::Collection,
        coverage_reason: None,
    })?;

    // Get all errors
    let errors = storage.get_errors(None)?;
    assert_eq!(errors.len(), 2);
    let fatal = errors
        .iter()
        .find(|error| error.message == "Fatal error")
        .expect("fatal error");
    let warning = errors
        .iter()
        .find(|error| error.message == "Warning")
        .expect("warning");
    assert_eq!(
        fatal.coverage_reason,
        Some(FileCoverageReason::SourceChanged)
    );
    assert_eq!(warning.coverage_reason, None);

    // Get fatal errors only
    let filter = codestory_contracts::graph::ErrorFilter {
        fatal_only: true,
        indexed_only: false,
    };
    let errors = storage.get_errors(Some(&filter))?;
    assert_eq!(errors.len(), 1);
    assert!(errors[0].is_fatal);

    Ok(())
}

#[test]
fn test_trail_query() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    // Create a simple graph: A -> B -> C
    let nodes = vec![
        Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "A".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "B".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "C".to_string(),
            ..Default::default()
        },
    ];
    storage.insert_nodes_batch(&nodes)?;

    let edges = vec![
        Edge {
            id: codestory_contracts::graph::EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
        Edge {
            id: codestory_contracts::graph::EdgeId(2),
            source: NodeId(2),
            target: NodeId(3),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
    ];
    storage.insert_edges_batch(&edges)?;

    // Trail from A, depth 1, should get A and B
    let config = TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 100,
    };
    let result = storage.get_trail(&config)?;
    assert_eq!(result.nodes.len(), 2);
    assert!(!result.truncated);

    // Trail from A, depth 2, should get A, B, and C
    let config = TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 2,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 100,
    };
    let result = storage.get_trail(&config)?;
    assert_eq!(result.nodes.len(), 3);

    // Trail from A, depth 0 (infinite), should also get A, B, and C (bounded by max_nodes)
    let config = TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 0,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 100,
    };
    let result = storage.get_trail(&config)?;
    assert_eq!(result.nodes.len(), 3);

    Ok(())
}

#[test]
fn test_trail_to_target_symbol_simple_path() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let nodes = vec![
        Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "A".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "B".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "C".to_string(),
            ..Default::default()
        },
    ];
    storage.insert_nodes_batch(&nodes)?;

    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(2),
            target: NodeId(3),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
    ])?;

    let result = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::ToTargetSymbol,
        target_id: Some(NodeId(3)),
        depth: 2,
        direction: TrailDirection::Outgoing, // ignored/forced by mode, but set for clarity
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 100,
    })?;

    assert_eq!(result.nodes.len(), 3);
    assert_eq!(result.edges.len(), 2);
    assert!(!result.truncated);

    Ok(())
}

#[test]
fn test_trail_to_target_symbol_prunes_unreachable_incoming_fanout() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let mut nodes = vec![
        Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "Root".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "Middle".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "Bridge".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(4),
            kind: NodeKind::FUNCTION,
            serialized_name: "Target".to_string(),
            ..Default::default()
        },
    ];
    for id in 100..130 {
        nodes.push(Node {
            id: NodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("Noise{id}"),
            ..Default::default()
        });
    }
    storage.insert_nodes_batch(&nodes)?;

    let mut edges = vec![
        Edge {
            id: EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(2),
            target: NodeId(3),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
        Edge {
            id: EdgeId(3),
            source: NodeId(3),
            target: NodeId(4),
            kind: EdgeKind::CALL,
            ..Default::default()
        },
    ];
    for id in 100..130 {
        edges.push(Edge {
            id: EdgeId(id),
            source: NodeId(id),
            target: NodeId(4),
            kind: EdgeKind::CALL,
            ..Default::default()
        });
    }
    storage.insert_edges_batch(&edges)?;

    let result = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::ToTargetSymbol,
        target_id: Some(NodeId(4)),
        depth: 3,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 4,
    })?;

    assert_eq!(
        result.nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
        vec![NodeId(1), NodeId(2), NodeId(3), NodeId(4)]
    );
    assert_eq!(
        result.edges.iter().map(|edge| edge.id).collect::<Vec<_>>(),
        vec![EdgeId(1), EdgeId(2), EdgeId(3)]
    );
    assert!(!result.truncated);

    Ok(())
}

#[test]
fn test_trail_to_target_symbol_no_path_returns_endpoints() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "A".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "B".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "C".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(1),
        source: NodeId(1),
        target: NodeId(2),
        kind: EdgeKind::CALL,
        ..Default::default()
    }])?;

    let result = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::ToTargetSymbol,
        target_id: Some(NodeId(3)),
        depth: 0,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 100,
    })?;

    assert_eq!(
        result.nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
        vec![NodeId(1), NodeId(3)]
    );
    assert!(result.edges.is_empty());
    assert!(!result.truncated);

    Ok(())
}

#[test]
fn test_trail_ignores_ambiguous_call_resolutions() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let caller = Node {
        id: NodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "caller".to_string(),
        qualified_name: Some("caller".to_string()),
        ..Default::default()
    };
    let call_symbol = Node {
        id: NodeId(10),
        kind: NodeKind::UNKNOWN,
        serialized_name: "add".to_string(),
        ..Default::default()
    };
    let resolved = Node {
        id: NodeId(3),
        kind: NodeKind::METHOD,
        serialized_name: "SomeType::add".to_string(),
        qualified_name: Some("SomeType::add".to_string()),
        ..Default::default()
    };

    storage.insert_nodes_batch(&[caller.clone(), call_symbol.clone(), resolved.clone()])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(100),
        source: caller.id,
        target: call_symbol.id,
        kind: EdgeKind::CALL,
        resolved_target: Some(resolved.id),
        confidence: Some(0.6),
        ..Default::default()
    }])?;

    // Exploring from the resolved target should not traverse this edge.
    let result = storage.get_trail(&TrailConfig {
        root_id: resolved.id,
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Incoming,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;

    assert!(result.edges.is_empty());
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].id, resolved.id);

    Ok(())
}

#[test]
fn test_trail_production_scope_excludes_test_callers() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let file_prod = Node {
        id: NodeId(100),
        kind: NodeKind::FILE,
        serialized_name: "src/lib.rs".to_string(),
        ..Default::default()
    };
    let file_test = Node {
        id: NodeId(101),
        kind: NodeKind::FILE,
        serialized_name: "tests/integration.rs".to_string(),
        ..Default::default()
    };
    let prod_target = Node {
        id: NodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "target".to_string(),
        file_node_id: Some(file_prod.id),
        ..Default::default()
    };
    let test_caller = Node {
        id: NodeId(2),
        kind: NodeKind::FUNCTION,
        serialized_name: "test_caller".to_string(),
        file_node_id: Some(file_test.id),
        ..Default::default()
    };
    let unresolved_target = Node {
        id: NodeId(3),
        kind: NodeKind::UNKNOWN,
        serialized_name: "target".to_string(),
        file_node_id: Some(file_test.id),
        ..Default::default()
    };

    storage.insert_nodes_batch(&[
        file_prod,
        file_test,
        prod_target,
        test_caller,
        unresolved_target,
    ])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(1),
        source: NodeId(2),
        target: NodeId(3),
        kind: EdgeKind::CALL,
        resolved_target: Some(NodeId(1)),
        file_node_id: Some(NodeId(101)),
        ..Default::default()
    }])?;

    let production_only = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Incoming,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert!(production_only.edges.is_empty());

    let include_tests = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Incoming,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert_eq!(include_tests.edges.len(), 1);

    Ok(())
}

#[test]
fn test_trail_can_hide_utility_calls() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    let caller = Node {
        id: NodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "caller".to_string(),
        ..Default::default()
    };
    let utility_symbol = Node {
        id: NodeId(2),
        kind: NodeKind::UNKNOWN,
        serialized_name: "len".to_string(),
        ..Default::default()
    };

    storage.insert_nodes_batch(&[caller, utility_symbol])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(10),
        source: NodeId(1),
        target: NodeId(2),
        kind: EdgeKind::CALL,
        ..Default::default()
    }])?;

    let hidden = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: false,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert!(hidden.edges.is_empty());

    let shown = storage.get_trail(&TrailConfig {
        root_id: NodeId(1),
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 1,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::CALL],
        show_utility_calls: true,
        node_filter: Vec::new(),
        max_nodes: 50,
    })?;
    assert_eq!(shown.edges.len(), 1);

    Ok(())
}

#[test]
fn test_helper_calls_are_not_suppressed_as_ambiguous() {
    assert!(!should_ignore_call_resolution(
        "Self::flush_projection_batch",
        Some(ResolutionCertainty::Uncertain),
        Some(0.40)
    ));
    assert!(!should_ignore_call_resolution(
        "WorkspaceIndexer::seed_symbol_table",
        Some(ResolutionCertainty::Probable),
        Some(0.70)
    ));
}

#[test]
fn test_safe_enum_conversion() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    // Test that we can round-trip all NodeKind variants
    let node = Node {
        id: NodeId(1),
        kind: NodeKind::ENUM_CONSTANT,
        serialized_name: "test".to_string(),
        ..Default::default()
    };
    storage.insert_nodes_batch(&[node])?;

    let nodes = storage.get_nodes()?;
    assert_eq!(nodes[0].kind, NodeKind::ENUM_CONSTANT);

    // Test that we can round-trip all EdgeKind variants
    let edges = vec![Edge {
        id: codestory_contracts::graph::EdgeId(1),
        source: NodeId(1),
        target: NodeId(1),
        kind: EdgeKind::ANNOTATION_USAGE,
        ..Default::default()
    }];
    storage.insert_edges_batch(&edges)?;

    let edges = storage.get_edges()?;
    assert_eq!(edges[0].kind, EdgeKind::ANNOTATION_USAGE);

    Ok(())
}

#[test]
fn grounding_node_snapshot_preserves_columns_rank_and_member_root_direction()
-> Result<(), StorageError> {
    #[derive(Debug, PartialEq)]
    struct SnapshotRow {
        node_id: i64,
        kind: i32,
        serialized_name: String,
        qualified_name: Option<String>,
        canonical_id: Option<String>,
        file_node_id: Option<i64>,
        start_line: Option<i64>,
        start_col: Option<i64>,
        end_line: Option<i64>,
        end_col: Option<i64>,
        display_name: String,
        file_path: Option<String>,
        node_rank: i64,
        sort_start_line: i64,
        is_root: i64,
        file_symbol_rank: Option<i64>,
    }

    let mut storage = Storage::new_in_memory()?;
    storage.insert_file(&FileInfo {
        id: 100,
        path: PathBuf::from("src/lib.rs"),
        language: "rust".to_string(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 20,
        file_role: FileRole::Source,
    })?;
    storage.insert_nodes_batch(&[
        Node {
            id: NodeId(100),
            kind: NodeKind::FILE,
            serialized_name: "source-node-path.rs".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(200),
            kind: NodeKind::FILE,
            serialized_name: "generated/fallback.ts".to_string(),
            ..Default::default()
        },
        Node {
            id: NodeId(101),
            kind: NodeKind::STRUCT,
            serialized_name: "Widget".to_string(),
            qualified_name: Some("crate::Widget".to_string()),
            canonical_id: Some("rust:struct:Widget".to_string()),
            file_node_id: Some(NodeId(100)),
            start_line: Some(2),
            start_col: Some(3),
            end_line: Some(8),
            end_col: Some(1),
        },
        Node {
            id: NodeId(102),
            kind: NodeKind::FUNCTION,
            serialized_name: "run".to_string(),
            file_node_id: Some(NodeId(100)),
            start_line: Some(10),
            start_col: Some(1),
            end_line: Some(12),
            end_col: Some(2),
            ..Default::default()
        },
        Node {
            id: NodeId(103),
            kind: NodeKind::MODULE,
            serialized_name: "\"./types\"".to_string(),
            file_node_id: Some(NodeId(100)),
            start_line: Some(1),
            ..Default::default()
        },
        Node {
            id: NodeId(201),
            kind: NodeKind::CLASS,
            serialized_name: "Fallback".to_string(),
            file_node_id: Some(NodeId(200)),
            ..Default::default()
        },
        Node {
            id: NodeId(202),
            kind: NodeKind::UNKNOWN,
            serialized_name: "excluded".to_string(),
            file_node_id: Some(NodeId(200)),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(1),
        source: NodeId(101),
        target: NodeId(102),
        kind: EdgeKind::MEMBER,
        file_node_id: Some(NodeId(100)),
        ..Default::default()
    }])?;

    storage.refresh_grounding_summary_snapshots()?;
    let mut stmt = storage.conn.prepare(
        "SELECT
            node_id,
            kind,
            serialized_name,
            qualified_name,
            canonical_id,
            file_node_id,
            start_line,
            start_col,
            end_line,
            end_col,
            display_name,
            file_path,
            node_rank,
            sort_start_line,
            is_root,
            file_symbol_rank
         FROM grounding_node_snapshot
         ORDER BY node_id",
    )?;
    let actual = stmt
        .query_map([], |row| {
            Ok(SnapshotRow {
                node_id: row.get(0)?,
                kind: row.get(1)?,
                serialized_name: row.get(2)?,
                qualified_name: row.get(3)?,
                canonical_id: row.get(4)?,
                file_node_id: row.get(5)?,
                start_line: row.get(6)?,
                start_col: row.get(7)?,
                end_line: row.get(8)?,
                end_col: row.get(9)?,
                display_name: row.get(10)?,
                file_path: row.get(11)?,
                node_rank: row.get(12)?,
                sort_start_line: row.get(13)?,
                is_root: row.get(14)?,
                file_symbol_rank: row.get(15)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    assert_eq!(
        actual,
        vec![
            SnapshotRow {
                node_id: 101,
                kind: NodeKind::STRUCT as i32,
                serialized_name: "Widget".to_string(),
                qualified_name: Some("crate::Widget".to_string()),
                canonical_id: Some("rust:struct:Widget".to_string()),
                file_node_id: Some(100),
                start_line: Some(2),
                start_col: Some(3),
                end_line: Some(8),
                end_col: Some(1),
                display_name: "crate::Widget".to_string(),
                file_path: Some("src/lib.rs".to_string()),
                node_rank: 0,
                sort_start_line: 2,
                is_root: 1,
                file_symbol_rank: Some(1),
            },
            SnapshotRow {
                node_id: 102,
                kind: NodeKind::FUNCTION as i32,
                serialized_name: "run".to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: Some(100),
                start_line: Some(10),
                start_col: Some(1),
                end_line: Some(12),
                end_col: Some(2),
                display_name: "run".to_string(),
                file_path: Some("src/lib.rs".to_string()),
                node_rank: 1,
                sort_start_line: 10,
                is_root: 0,
                file_symbol_rank: Some(2),
            },
            SnapshotRow {
                node_id: 103,
                kind: NodeKind::MODULE as i32,
                serialized_name: "\"./types\"".to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: Some(100),
                start_line: Some(1),
                start_col: None,
                end_line: None,
                end_col: None,
                display_name: "\"./types\"".to_string(),
                file_path: Some("src/lib.rs".to_string()),
                node_rank: 5,
                sort_start_line: 1,
                is_root: 1,
                file_symbol_rank: Some(3),
            },
            SnapshotRow {
                node_id: 201,
                kind: NodeKind::CLASS as i32,
                serialized_name: "Fallback".to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: Some(200),
                start_line: None,
                start_col: None,
                end_line: None,
                end_col: None,
                display_name: "Fallback".to_string(),
                file_path: Some("generated/fallback.ts".to_string()),
                node_rank: 0,
                sort_start_line: 2_147_483_647,
                is_root: 1,
                file_symbol_rank: Some(1),
            },
        ]
    );
    Ok(())
}

#[test]
fn test_grounding_queries_rank_symbols_and_roots() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    storage.insert_file(&FileInfo {
        id: 100,
        path: PathBuf::from("src/a.rs"),
        language: "rust".to_string(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;
    storage.insert_file(&FileInfo {
        id: 200,
        path: PathBuf::from("src/b.rs"),
        language: "rust".to_string(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 10,
        file_role: FileRole::Source,
    })?;
    storage.insert_nodes_batch(&[
        codestory_contracts::graph::Node {
            id: NodeId(100),
            kind: NodeKind::FILE,
            serialized_name: "src/a.rs".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(200),
            kind: NodeKind::FILE,
            serialized_name: "src/b.rs".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "zeta".to_string(),
            file_node_id: Some(NodeId(100)),
            start_line: Some(8),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(102),
            kind: NodeKind::STRUCT,
            serialized_name: "Alpha".to_string(),
            file_node_id: Some(NodeId(100)),
            start_line: Some(2),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(201),
            kind: NodeKind::MODULE,
            serialized_name: "\"./types\"".to_string(),
            file_node_id: Some(NodeId(200)),
            start_line: Some(1),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(202),
            kind: NodeKind::CLASS,
            serialized_name: "Widget".to_string(),
            file_node_id: Some(NodeId(200)),
            start_line: Some(2),
            ..Default::default()
        },
    ])?;

    let summaries = storage.get_grounding_file_summaries()?;
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].file.id, 100);
    assert_eq!(summaries[0].symbol_count, 2);
    assert_eq!(summaries[0].best_node_rank, 0);

    let top = storage.get_grounding_top_symbols_for_files(&[100, 200], 1)?;
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].node.id, NodeId(102));
    assert_eq!(top[1].node.id, NodeId(202));

    let roots = storage.get_grounding_root_symbol_candidates(2, 0)?;
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].node.id, NodeId(102));
    assert_eq!(roots[1].node.id, NodeId(202));

    Ok(())
}

#[test]
fn test_grounding_member_counts_and_occurrence_lines() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    storage.insert_nodes_batch(&[
        codestory_contracts::graph::Node {
            id: NodeId(1),
            kind: NodeKind::STRUCT,
            serialized_name: "Widget".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(2),
            kind: NodeKind::FIELD,
            serialized_name: "title".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(3),
            kind: NodeKind::FIELD,
            serialized_name: "count".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(10),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(11),
            kind: NodeKind::FUNCTION,
            serialized_name: "render".to_string(),
            file_node_id: Some(NodeId(10)),
            start_line: None,
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[
        Edge {
            id: EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
        Edge {
            id: EdgeId(2),
            source: NodeId(1),
            target: NodeId(3),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        },
    ])?;
    storage.insert_occurrences_batch(&[
        codestory_contracts::graph::Occurrence {
            element_id: 11,
            kind: codestory_contracts::graph::OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: NodeId(10),
                start_line: 20,
                start_col: 1,
                end_line: 20,
                end_col: 5,
            },
        },
        codestory_contracts::graph::Occurrence {
            element_id: 11,
            kind: codestory_contracts::graph::OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: NodeId(10),
                start_line: 5,
                start_col: 1,
                end_line: 5,
                end_col: 5,
            },
        },
    ])?;

    let member_counts = storage.get_grounding_member_counts(&[NodeId(1)])?;
    assert_eq!(member_counts.get(&NodeId(1)), Some(&2));

    let fallback_lines = storage.get_grounding_min_occurrence_lines(&[NodeId(11)])?;
    assert_eq!(fallback_lines.get(&NodeId(11)), Some(&20));

    Ok(())
}

#[test]
fn test_grounding_edge_digests_ignore_ambiguous_resolved_targets() -> Result<(), StorageError> {
    let mut storage = Storage::new_in_memory()?;

    storage.insert_nodes_batch(&[
        codestory_contracts::graph::Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(2),
            kind: NodeKind::UNKNOWN,
            serialized_name: "len".to_string(),
            ..Default::default()
        },
        codestory_contracts::graph::Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "Vec::len".to_string(),
            ..Default::default()
        },
    ])?;
    storage.insert_edges_batch(&[Edge {
        id: EdgeId(10),
        source: NodeId(1),
        target: NodeId(2),
        kind: EdgeKind::CALL,
        resolved_target: Some(NodeId(3)),
        certainty: Some(ResolutionCertainty::Uncertain),
        ..Default::default()
    }])?;

    let counts = storage.get_grounding_edge_digest_counts(&[NodeId(1), NodeId(2), NodeId(3)])?;
    assert!(counts.iter().any(|entry| {
        entry.node_id == NodeId(1) && entry.kind == EdgeKind::CALL && entry.count == 1
    }));
    assert!(counts.iter().any(|entry| {
        entry.node_id == NodeId(2) && entry.kind == EdgeKind::CALL && entry.count == 1
    }));
    assert!(!counts.iter().any(|entry| entry.node_id == NodeId(3)));

    Ok(())
}
