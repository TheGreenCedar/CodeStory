//! The core store must open under cache roots longer than Windows' `MAX_PATH`.
//!
//! The packaged `codestory-cli.exe` carries no `longPathAware` application
//! manifest, so `CreateFileW` stays capped at 260 characters for it no matter
//! what `LongPathsEnabled` says on the host. Rust's standard library hides
//! that cap by converting to extended-length form internally; SQLite's Win32
//! VFS does not. The core store runs in WAL mode and SQLite derives `-wal`
//! (+4) and `-shm` (+4) by appending to the main database path, so a store
//! that opens at 258 characters still fails when SQLite reaches for its
//! siblings — with a bare "unable to open database file".
//!
//! These tests exercise the real store surfaces at a padded path on every
//! platform. On Windows they additionally assert the sibling paths cross the
//! cap, which is the condition that actually reproduced the failure. On other
//! platforms there is no cap, so they are regression cover proving the
//! conversion stayed a no-op and did not disturb ordinary opens.

use codestory_store::{FileInfo, FileRole, IndexArtifactCacheWrite, Store};
use std::path::{Path, PathBuf};

/// Comfortably past `MAX_PATH` once a temporary root is prefixed, while every
/// individual component stays under the 255-character component limit.
const PADDING_COMPONENT: &str = "codestory-long-path-padding-segment-0123456789abcdef";
const PADDING_DEPTH: usize = 5;

/// Create a database path deep enough to cross `MAX_PATH`, with its parent
/// directories already present.
fn deep_database_path(root: &Path, name: &str) -> PathBuf {
    let mut directory = root.to_path_buf();
    for index in 0..PADDING_DEPTH {
        directory = directory.join(format!("{PADDING_COMPONENT}-{index}"));
    }
    std::fs::create_dir_all(&directory).expect("create the padded store directory");
    let path = directory.join(name);
    assert!(
        path.as_os_str().len() > 260,
        "the fixture must cross MAX_PATH, got {} characters",
        path.as_os_str().len()
    );
    path
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn seeded_file(id: i64, path: &str) -> FileInfo {
    FileInfo {
        id,
        path: PathBuf::from(path),
        language: "rust".to_string(),
        modification_time: 1_700_000_000_000,
        indexed: true,
        complete: true,
        line_count: 7,
        file_role: FileRole::classify_path(Path::new(path)),
    }
}

#[test]
fn a_live_store_round_trips_under_a_path_longer_than_max_path() {
    let root = tempfile::tempdir().expect("temporary store root");
    let path = deep_database_path(root.path(), "codestory.db");

    let mut storage = Store::open(&path).expect("open a live store beyond MAX_PATH");
    storage
        .insert_files_batch(&[seeded_file(1, "src/lib.rs")])
        .expect("write through the padded store");

    // WAL mode materializes both siblings while the connection is open. They
    // are the files that actually crossed the cap in the reported failure.
    let wal = sibling(&path, "-wal");
    let shm = sibling(&path, "-shm");
    assert!(wal.exists(), "WAL sidecar must exist at {}", wal.display());
    assert!(shm.exists(), "SHM sidecar must exist at {}", shm.display());
    assert!(
        wal.as_os_str().len() > 260 && shm.as_os_str().len() > 260,
        "both sidecar paths must cross MAX_PATH for this test to mean anything"
    );

    drop(storage);

    let reopened = Store::open_read_only(&path).expect("reopen the padded store read-only");
    let files = reopened.files().get_files().expect("read the padded store");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, PathBuf::from("src/lib.rs"));
}

#[test]
fn an_observational_store_opens_under_a_path_longer_than_max_path() {
    let root = tempfile::tempdir().expect("temporary store root");
    let path = deep_database_path(root.path(), "codestory.db");

    let mut storage = Store::open(&path).expect("open a live store beyond MAX_PATH");
    storage
        .insert_files_batch(&[seeded_file(1, "src/main.rs")])
        .expect("write through the padded store");
    drop(storage);

    // The observational reader is the one open site that must go through a
    // SQLite `file:` URI, because `immutable=1` has no `OpenFlags` spelling.
    // The extended-length prefix has to survive URI parsing intact.
    let observer = Store::open_observational(&path).expect("observe the padded store");
    let files = observer.files().get_files().expect("observe stored files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, PathBuf::from("src/main.rs"));

    assert_eq!(
        Store::database_schema_version(&path).expect("read the padded schema version"),
        codestory_store::CURRENT_SCHEMA_VERSION
    );
}

#[test]
fn a_database_snapshot_copies_between_paths_longer_than_max_path() {
    let root = tempfile::tempdir().expect("temporary store root");
    let source_path = deep_database_path(root.path(), "source.db");
    let target_path = deep_database_path(root.path(), "target.db");

    let mut source = Store::open(&source_path).expect("open the padded source store");
    source
        .insert_files_batch(&[seeded_file(1, "src/copied.rs")])
        .expect("seed the padded source store");
    drop(source);

    // `copy_database_snapshot` opens the source read-only and lets SQLite's
    // backup API open the target, so both sides need the conversion.
    Store::copy_database_snapshot(&source_path, &target_path).expect("copy beyond MAX_PATH");

    let copied = Store::open_read_only(&target_path).expect("open the padded copy");
    let files = copied.files().get_files().expect("read the padded copy");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, PathBuf::from("src/copied.rs"));
}

#[test]
fn an_attached_source_database_copies_beyond_max_path() {
    let root = tempfile::tempdir().expect("temporary store root");
    let source_path = deep_database_path(root.path(), "cache-source.db");
    let target_path = deep_database_path(root.path(), "cache-target.db");

    let cached_file = Path::new("src/cached.rs");
    let source = Store::open(&source_path).expect("open the padded cache source");
    source
        .upsert_index_artifact_cache_batch(&[IndexArtifactCacheWrite {
            path: cached_file,
            cache_key: "cache-key-v1",
            artifact_blob: b"padded-artifact",
        }])
        .expect("seed the padded artifact cache");
    drop(source);

    // `ATTACH DATABASE` opens the attached file with its own WAL siblings.
    let mut target = Store::open(&target_path).expect("open the padded cache target");
    let copied = target
        .copy_index_artifact_cache_from(&source_path)
        .expect("attach and copy beyond MAX_PATH");
    assert_eq!(copied, 1);
    assert_eq!(
        target
            .get_index_artifact_cache(cached_file, "cache-key-v1")
            .expect("read the copied artifact"),
        Some(b"padded-artifact".to_vec())
    );
}

/// Wiring guard: every SQLite open in the core store's implementation must
/// name the conversion helper.
///
/// On non-Windows hosts the conversion is a no-op, so no behavioral test can
/// notice it being removed. This reads the production source instead, which
/// makes a reverted call site fail here rather than only on a Windows host
/// with a deep cache root.
#[test]
fn every_core_store_sqlite_open_routes_through_the_conversion_helper() {
    const SOURCE: &str = include_str!("../src/storage_impl/mod.rs");
    /// Opening a database by one of these spellings hands SQLite a filename.
    const OPEN_SPELLINGS: [&str; 5] = [
        "Connection::open(",
        "Connection::open_with_flags(",
        "ATTACH DATABASE",
        ".backup(",
        ".restore(",
    ];
    /// Any of these in the call's immediate neighborhood proves the filename
    /// was converted, or that no filename is involved at all.
    const ACCEPTED: [&str; 5] = [
        "sqlite_path::",
        // PinnedCopySource holds the immutable image lease through ATTACH.
        // The separate guard below proves both branches convert the path.
        "source_path.attach_argument()",
        "open_in_memory",
        "DETACH DATABASE",
        // Opt out explicitly, next to the call, when a site genuinely does
        // not open a filesystem path.
        "sqlite-open-path: not required",
    ];
    /// How far before the opening line the conversion may appear (an
    /// `ATTACH` binds its argument a couple of lines earlier).
    const LINES_BEFORE: usize = 3;
    /// How far after the opening line the conversion may appear (a multi-line
    /// call passes it as an argument).
    const LINES_AFTER: usize = 4;

    let lines: Vec<&str> = SOURCE.lines().collect();
    let mut unguarded = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !OPEN_SPELLINGS
            .iter()
            .any(|spelling| line.contains(spelling))
        {
            continue;
        }
        let window_start = index.saturating_sub(LINES_BEFORE);
        let window_end = (index + LINES_AFTER + 1).min(lines.len());
        let window = lines[window_start..window_end].join("\n");
        if ACCEPTED.iter().any(|accepted| window.contains(accepted)) {
            continue;
        }
        unguarded.push(format!("line {}: {}", index + 1, line.trim()));
    }

    assert!(
        unguarded.is_empty(),
        "these SQLite opens in crates/codestory-store/src/storage_impl/mod.rs do not convert \
         their path with codestory_store::sqlite_path, so their -wal/-shm siblings will fail \
         past MAX_PATH on Windows:\n{}",
        unguarded.join("\n")
    );
}

/// Both `PinnedCopySource` ATTACH branches must convert their path:
/// a published generation attaches through the immutable observational URI
/// (no `-wal`/`-shm` may appear beside the sealed image), while a legacy
/// fixed-path source attaches through the plain converted path argument.
/// Behavior-level proof exercises each branch instead of asserting on the
/// private implementation body.
#[test]
fn pinned_copy_source_attach_converts_both_image_and_legacy_paths() {
    let cached_file = Path::new("src/cached.rs");

    // Legacy branch: a fixed-path source with no publication pointer.
    let root = tempfile::tempdir().expect("temporary store root");
    let legacy_source = root.path().join("legacy.db");
    let source = Store::open(&legacy_source).expect("open legacy source");
    source
        .upsert_index_artifact_cache_batch(&[IndexArtifactCacheWrite {
            path: cached_file,
            cache_key: "legacy-key",
            artifact_blob: b"legacy-artifact",
        }])
        .expect("seed legacy artifact");
    drop(source);

    let legacy_target = root.path().join("legacy-target.db");
    let mut target = Store::open(&legacy_target).expect("open legacy target");
    assert_eq!(
        target
            .copy_index_artifact_cache_from(&legacy_source)
            .expect("attach a legacy fixed-path source"),
        1
    );
    assert_eq!(
        target
            .get_index_artifact_cache(cached_file, "legacy-key")
            .expect("read the legacy copy"),
        Some(b"legacy-artifact".to_vec())
    );
    drop(target);

    // Published branch: a sealed generation pins the copy source, so the
    // attach must use the immutable observational URI — proven by the source
    // image gaining no WAL/SHM siblings during the read.
    let published_root = tempfile::tempdir().expect("published store root");
    let live_path = published_root.path().join("codestory.db");
    let mut stage =
        codestory_store::SnapshotStore::open_staged(&live_path).expect("open staged source");
    stage
        .store_mut()
        .upsert_index_artifact_cache_batch(&[IndexArtifactCacheWrite {
            path: cached_file,
            cache_key: "published-key",
            artifact_blob: b"published-artifact",
        }])
        .expect("seed published artifact");
    let publication = codestory_store::IndexPublicationRecord {
        generation: 1,
        generation_id: "generation-1".to_owned(),
        run_id: "run-1".to_owned(),
        mode: codestory_store::IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    };
    stage
        .store_mut()
        .put_index_publication(&publication)
        .expect("source publication");
    stage
        .store_mut()
        .publish_structural_text_unit_generation(&publication)
        .expect("source structural identity");
    stage
        .store_mut()
        .publish_source_policy_exclusion_generation(
            &publication,
            "test-project",
            "test-workspace",
            codestory_store::SourcePolicyExclusionPolicyIdentity::new(
                codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION,
                codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP,
                codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            ),
            &[],
        )
        .expect("source policy identity");
    stage
        .publish(&live_path)
        .expect("publish source generation");

    let layout = codestory_store::CorePublicationLayout::from_storage_path(&live_path)
        .expect("publication layout");
    let sealed_source = layout
        .resolve_active_database()
        .expect("resolve published source")
        .expect("published generation exists");
    assert_ne!(sealed_source, live_path);

    // The target must sit in a different directory: `CorePublicationLayout`
    // roots at `<parent>/core`, so a sibling path would resolve the source's
    // pointer and open read-only.
    let target_dir = published_root.path().join("target");
    std::fs::create_dir(&target_dir).expect("target directory");
    let published_target = target_dir.join("published-target.db");
    let mut target = Store::open(&published_target).expect("open published target");
    assert_eq!(
        target
            .copy_index_artifact_cache_from(&live_path)
            .expect("attach the pinned published image"),
        1
    );
    assert_eq!(
        target
            .get_index_artifact_cache(cached_file, "published-key")
            .expect("read the published copy"),
        Some(b"published-artifact".to_vec())
    );
    drop(target);
    assert_eq!(
        sibling(&sealed_source, "-wal").exists() || sibling(&sealed_source, "-shm").exists(),
        false,
        "attaching a sealed generation must not materialize lock siblings"
    );
}
