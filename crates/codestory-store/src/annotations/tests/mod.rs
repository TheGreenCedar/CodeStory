use super::resolution::{CoreAnchorCandidate, resolve_bookmark};
use super::*;
use std::collections::HashMap;
use tempfile::TempDir;

fn binding(root: &Path, token: &str) -> NativeRootBinding {
    NativeRootBinding::new(Some(token.to_string()), root)
}

fn open_store(dir: &TempDir, token: &str) -> AnnotationStore {
    let path = dir.path().join("annotations.sqlite3");
    AnnotationStore::open_for_write(&path, &binding(dir.path(), token)).expect("open sidecar")
}

fn legacy_snapshot() -> LegacyAnnotationSnapshot {
    LegacyAnnotationSnapshot {
        categories: vec![AnnotationCategory {
            id: 1,
            name: "Favorites".to_string(),
        }],
        bookmarks: vec![LegacyBookmarkRow {
            id: 1,
            category_id: 1,
            comment: Some("keep".to_string()),
            canonical_id: None,
            file_identity: Some("/repo/src/lib.rs".to_string()),
            qualified_name: Some("alpha".to_string()),
            kind: Some(3),
            normalized_signature: Some("111".to_string()),
            start_line: Some(10),
        }],
    }
}

#[test]
fn sidecar_owns_a_versioned_schema_with_wal_and_foreign_keys() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");

    assert_eq!(
        store.schema_version().expect("schema version"),
        ANNOTATION_SCHEMA_VERSION
    );
    let journal_mode: String = store
        .conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    let foreign_keys: i64 = store
        .conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("foreign keys");
    assert_eq!(foreign_keys, 1);
    let busy_timeout: i64 = store
        .conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("busy timeout");
    assert_eq!(busy_timeout, SIDECAR_BUSY_TIMEOUT.as_millis() as i64);
}

#[test]
fn observational_open_never_materializes_the_sidecar() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("annotations.sqlite3");

    assert!(
        AnnotationStore::open_observational(&path)
            .expect("observational open")
            .is_none()
    );
    assert!(
        !path.exists(),
        "an observational open must not create the sidecar"
    );
}

#[test]
fn observational_open_rejects_a_newer_sidecar_schema() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("annotations.sqlite3");
    {
        let store = open_store(&dir, "unix:1:1");
        store
            .conn
            .execute(
                "UPDATE annotation_schema_version SET version = ?1 WHERE id = 1",
                params![ANNOTATION_SCHEMA_VERSION + 1],
            )
            .expect("stamp newer schema");
    }

    let error = AnnotationStore::open_observational(&path).expect_err("newer schema fails closed");
    assert!(
        matches!(error, AnnotationError::UnsupportedSchema { found, .. } if found == ANNOTATION_SCHEMA_VERSION + 1),
        "unexpected error: {error}"
    );
}

#[test]
fn core_import_is_journaled_idempotent_and_retains_a_backup() {
    let dir = TempDir::new().expect("temp dir");
    let backup = dir.path().join("annotations.pre-migration.json");
    let mut store = open_store(&dir, "unix:1:1");
    let snapshot = legacy_snapshot();

    assert!(
        store
            .import_core_annotations(&snapshot, &backup)
            .expect("first import")
    );
    assert!(
        backup.is_file(),
        "the pre-migration backup must be retained"
    );
    assert!(store.core_import_completed().expect("journal"));
    assert_eq!(store.bookmarks(None).expect("bookmarks").len(), 1);

    assert!(
        !store
            .import_core_annotations(&snapshot, &backup)
            .expect("second import"),
        "a journaled import must not run twice"
    );
    assert_eq!(
        store.bookmarks(None).expect("bookmarks").len(),
        1,
        "re-running the cutover must not duplicate annotations"
    );
}

#[test]
fn a_crash_before_the_import_commit_reimports_on_restart() {
    let dir = TempDir::new().expect("temp dir");
    let backup = dir.path().join("annotations.pre-migration.json");
    let path = dir.path().join("annotations.sqlite3");
    // A crash before the single import transaction commits leaves the schema
    // in place with neither journal row nor imported rows.
    {
        let store = open_store(&dir, "unix:1:1");
        assert!(!store.core_import_completed().expect("journal"));
        assert!(store.bookmarks(None).expect("bookmarks").is_empty());
    }

    let mut restarted = AnnotationStore::open_for_write(&path, &binding(dir.path(), "unix:1:1"))
        .expect("restart sidecar");
    assert!(
        restarted
            .import_core_annotations(&legacy_snapshot(), &backup)
            .expect("import after restart")
    );
    assert_eq!(restarted.bookmarks(None).expect("bookmarks").len(), 1);
}

#[test]
fn a_same_filesystem_move_keeps_the_binding_and_a_copy_fails_closed() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("annotations.sqlite3");
    let moved = dir.path().join("moved-root");
    {
        let _ = open_store(&dir, "unix:1:42");
    }

    let after_move = AnnotationStore::open_for_write(
        &path,
        &NativeRootBinding::new(Some("unix:1:42".into()), &moved),
    )
    .expect("same native root after a move");
    let (identity, root_path) = after_move
        .native_root_binding()
        .expect("binding")
        .expect("bound");
    assert_eq!(identity, "unix:1:42");
    assert_eq!(root_path, moved);
    drop(after_move);

    let error = AnnotationStore::open_for_write(&path, &binding(dir.path(), "unix:1:43"))
        .expect_err("a clone or cross-volume copy fails closed");
    assert!(
        matches!(error, AnnotationError::ForeignNativeRoot { .. }),
        "unexpected error: {error}"
    );

    let unidentified =
        AnnotationStore::open_for_write(&path, &NativeRootBinding::new(None, dir.path()))
            .expect_err("an unobservable root fails closed");
    assert!(
        matches!(unidentified, AnnotationError::ForeignNativeRoot { .. }),
        "unexpected error: {unidentified}"
    );
}

#[test]
fn two_writers_share_the_sidecar_under_wal_without_losing_annotations() {
    // Core publication runs its own writer; annotation CRUD must not have to
    // wait behind it or fail because of it, so the sidecar carries its own WAL
    // connection and busy timeout per process.
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("annotations.sqlite3");
    let first = open_store(&dir, "unix:1:1");
    let second = AnnotationStore::open_for_write(&path, &binding(dir.path(), "unix:1:1"))
        .expect("second writer");

    let alpha = first.create_category("Alpha").expect("first category");
    let beta = second.create_category("Beta").expect("second category");
    first
        .create_bookmark(alpha.id, &BookmarkAnchorInput::default())
        .expect("first bookmark");
    second
        .create_bookmark(beta.id, &BookmarkAnchorInput::default())
        .expect("second bookmark");

    assert_eq!(first.bookmarks(None).expect("first read").len(), 2);
    assert_eq!(second.bookmarks(None).expect("second read").len(), 2);
    assert_eq!(first.categories().expect("first categories").len(), 2);
}

#[test]
fn deleting_a_category_cascades_to_its_bookmarks_only() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let kept = store.create_category("Kept").expect("create kept");
    let dropped = store.create_category("Dropped").expect("create dropped");
    store
        .create_bookmark(kept.id, &BookmarkAnchorInput::default())
        .expect("kept bookmark");
    store
        .create_bookmark(dropped.id, &BookmarkAnchorInput::default())
        .expect("dropped bookmark");

    store.delete_category(dropped.id).expect("delete category");

    let remaining = store.bookmarks(None).expect("bookmarks");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].category_id, kept.id);
}

#[test]
fn category_names_stay_unique() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    store.create_category("Favorites").expect("create");

    let error = store
        .create_category("Favorites")
        .expect_err("duplicate name is refused");
    assert!(
        matches!(error, AnnotationError::DuplicateCategoryName(name) if name == "Favorites"),
        "duplicate category names must fail closed"
    );
}

#[test]
fn export_and_import_round_trip_preserves_annotations() {
    let source_dir = TempDir::new().expect("temp dir");
    let store = open_store(&source_dir, "unix:1:1");
    let category = store.create_category("Favorites").expect("create category");
    store
        .create_bookmark(
            category.id,
            &BookmarkAnchorInput {
                canonical_id: Some("codestory:symbol:alpha".to_string()),
                file_identity: Some("/repo/src/lib.rs".to_string()),
                qualified_name: Some("alpha".to_string()),
                kind: Some(3),
                normalized_signature: Some("111".to_string()),
                start_line: Some(10),
                comment: Some("note".to_string()),
                evidence: None,
            },
        )
        .expect("create bookmark");
    let export = store.export().expect("export");

    let target_dir = TempDir::new().expect("temp dir");
    let mut target = open_store(&target_dir, "unix:2:2");
    assert_eq!(target.import(&export).expect("import"), 1);

    let imported = target.bookmarks(None).expect("bookmarks");
    assert_eq!(imported.len(), 1);
    assert_eq!(
        imported[0].canonical_id.as_deref(),
        Some("codestory:symbol:alpha")
    );
    assert_eq!(imported[0].comment.as_deref(), Some("note"));
    assert_eq!(target.categories().expect("categories").len(), 1);
}

#[derive(Default)]
struct FakeCore {
    generation: Option<i64>,
    by_canonical_id: HashMap<String, Vec<CoreAnchorCandidate>>,
    by_tuple: HashMap<(String, String, i64), Vec<CoreAnchorCandidate>>,
    by_signature: HashMap<(String, Option<String>), Vec<CoreAnchorCandidate>>,
}

impl CoreAnchorIndex for FakeCore {
    fn current_generation(&self) -> Option<i64> {
        self.generation
    }

    fn candidates_by_canonical_id(&self, canonical_id: &str) -> Vec<CoreAnchorCandidate> {
        self.by_canonical_id
            .get(canonical_id)
            .cloned()
            .unwrap_or_default()
    }

    fn candidates_by_anchor_tuple(
        &self,
        file_identity: &str,
        qualified_name: &str,
        kind: i64,
    ) -> Vec<CoreAnchorCandidate> {
        self.by_tuple
            .get(&(file_identity.to_string(), qualified_name.to_string(), kind))
            .cloned()
            .unwrap_or_default()
    }

    fn candidates_by_normalized_signature(
        &self,
        normalized_signature: &str,
        file_identity: Option<&str>,
    ) -> Vec<CoreAnchorCandidate> {
        self.by_signature
            .get(&(
                normalized_signature.to_string(),
                file_identity.map(str::to_string),
            ))
            .cloned()
            .unwrap_or_default()
    }
}

fn candidate(node_id: i64, file: &str, qualified_name: &str) -> CoreAnchorCandidate {
    CoreAnchorCandidate {
        node_id,
        canonical_id: None,
        file_identity: Some(file.to_string()),
        qualified_name: Some(qualified_name.to_string()),
        kind: Some(3),
        normalized_signature: Some("111".to_string()),
        start_line: Some(40),
    }
}

fn anchored_bookmark(store: &AnnotationStore, generation: Option<i64>) -> AnnotationBookmark {
    let name = format!(
        "Favorites-{}",
        store.categories().expect("categories").len()
    );
    let category = store.create_category(&name).expect("create category");
    store
        .create_bookmark(
            category.id,
            &BookmarkAnchorInput {
                canonical_id: None,
                file_identity: Some("/repo/src/lib.rs".to_string()),
                qualified_name: Some("alpha".to_string()),
                kind: Some(3),
                normalized_signature: Some("111".to_string()),
                start_line: Some(10),
                comment: None,
                evidence: Some(BookmarkAnchorEvidence {
                    generation,
                    node_id: Some(7),
                    canonical_id: None,
                    file_identity: Some("/repo/src/lib.rs".to_string()),
                    qualified_name: Some("alpha".to_string()),
                    kind: Some(3),
                    normalized_signature: Some("111".to_string()),
                    start_line: Some(10),
                }),
            },
        )
        .expect("create bookmark")
}

#[test]
fn a_position_shifting_edit_re_resolves_the_unchanged_anchor() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let mut core = FakeCore {
        generation: Some(5),
        ..FakeCore::default()
    };
    core.by_tuple.insert(
        ("/repo/src/lib.rs".to_string(), "alpha".to_string(), 3),
        vec![candidate(99, "/repo/src/lib.rs", "alpha")],
    );

    let resolution = resolve_bookmark(&bookmark, &core);

    assert_eq!(resolution.node_id(), Some(99));
    assert_eq!(resolution.status(), ResolutionStatus::Bound);
}

#[test]
fn an_ambiguous_match_never_guesses() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let mut core = FakeCore {
        generation: Some(5),
        ..FakeCore::default()
    };
    core.by_tuple.insert(
        ("/repo/src/lib.rs".to_string(), "alpha".to_string(), 3),
        vec![
            candidate(99, "/repo/src/lib.rs", "alpha"),
            candidate(100, "/repo/src/lib.rs", "alpha"),
        ],
    );

    let resolution = resolve_bookmark(&bookmark, &core);

    assert_eq!(resolution.status(), ResolutionStatus::Orphaned);
    assert_eq!(
        resolution.orphan_reason(),
        Some(OrphanReason::AmbiguousMatch)
    );
}

#[test]
fn a_unique_rename_rebinds_only_with_adjacent_generation_evidence() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let mut core = FakeCore {
        generation: Some(5),
        ..FakeCore::default()
    };
    core.by_signature.insert(
        ("111".to_string(), Some("/repo/src/lib.rs".to_string())),
        vec![candidate(99, "/repo/src/lib.rs", "renamed")],
    );

    let adjacent = anchored_bookmark(&store, Some(4));
    let resolution = resolve_bookmark(&adjacent, &core);
    assert_eq!(resolution.node_id(), Some(99), "adjacent rename rebinds");

    let stale = anchored_bookmark(&store, Some(1));
    let resolution = resolve_bookmark(&stale, &core);
    assert_eq!(
        resolution.orphan_reason(),
        Some(OrphanReason::GenerationGap),
        "unobserved intervening history must not support an inference"
    );
}

#[test]
fn a_unique_move_rebinds_and_an_ambiguous_move_orphans() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let mut core = FakeCore {
        generation: Some(5),
        ..FakeCore::default()
    };
    core.by_signature.insert(
        ("111".to_string(), None),
        vec![candidate(99, "/repo/src/moved.rs", "alpha")],
    );
    assert_eq!(resolve_bookmark(&bookmark, &core).node_id(), Some(99));

    core.by_signature.insert(
        ("111".to_string(), None),
        vec![
            candidate(99, "/repo/src/moved.rs", "alpha"),
            candidate(100, "/repo/src/other.rs", "alpha"),
        ],
    );
    assert_eq!(
        resolve_bookmark(&bookmark, &core).orphan_reason(),
        Some(OrphanReason::AmbiguousMatch)
    );
}

#[test]
fn a_deleted_target_orphans_and_reappearance_rebinds() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let mut core = FakeCore {
        generation: Some(5),
        ..FakeCore::default()
    };

    let orphaned = resolve_bookmark(&bookmark, &core);
    assert_eq!(orphaned.orphan_reason(), Some(OrphanReason::TargetDeleted));
    store
        .apply_resolution(&bookmark.uuid, &orphaned)
        .expect("persist orphan");
    let stored = store
        .bookmark(&bookmark.uuid)
        .expect("reload")
        .expect("present");
    assert_eq!(stored.resolution_status, ResolutionStatus::Orphaned);
    assert_eq!(stored.orphan_reason, Some(OrphanReason::TargetDeleted));
    assert_eq!(
        stored.qualified_name.as_deref(),
        Some("alpha"),
        "an orphan stays a visible, user-owned annotation with its anchor intact"
    );

    core.by_tuple.insert(
        ("/repo/src/lib.rs".to_string(), "alpha".to_string(), 3),
        vec![candidate(99, "/repo/src/lib.rs", "alpha")],
    );
    let rebound = resolve_bookmark(&stored, &core);
    assert_eq!(rebound.node_id(), Some(99));
    store
        .apply_resolution(&stored.uuid, &rebound)
        .expect("persist rebind");
    let stored = store
        .bookmark(&bookmark.uuid)
        .expect("reload")
        .expect("present");
    assert_eq!(stored.resolution_status, ResolutionStatus::Bound);
    assert_eq!(stored.orphan_reason, None);
    assert_eq!(stored.start_line, Some(40));
}

#[test]
fn a_canonical_id_resolves_before_the_anchor_tuple() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let category = store.create_category("Favorites").expect("create category");
    let bookmark = store
        .create_bookmark(
            category.id,
            &BookmarkAnchorInput {
                canonical_id: Some("route_endpoint:GET:/users".to_string()),
                file_identity: Some("/repo/src/lib.rs".to_string()),
                qualified_name: Some("alpha".to_string()),
                kind: Some(3),
                ..BookmarkAnchorInput::default()
            },
        )
        .expect("create bookmark");
    let mut core = FakeCore {
        generation: Some(5),
        ..FakeCore::default()
    };
    core.by_canonical_id.insert(
        "route_endpoint:GET:/users".to_string(),
        vec![candidate(11, "/repo/src/routes.rs", "users_handler")],
    );
    core.by_tuple.insert(
        ("/repo/src/lib.rs".to_string(), "alpha".to_string(), 3),
        vec![candidate(99, "/repo/src/lib.rs", "alpha")],
    );

    assert_eq!(resolve_bookmark(&bookmark, &core).node_id(), Some(11));
}

#[test]
fn an_anchorless_bookmark_is_an_unresolvable_orphan() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let category = store.create_category("Favorites").expect("create category");
    let bookmark = store
        .create_bookmark(category.id, &BookmarkAnchorInput::default())
        .expect("create bookmark");

    assert_eq!(
        resolve_bookmark(&bookmark, &FakeCore::default()).orphan_reason(),
        Some(OrphanReason::UnresolvableAnchor)
    );
}

#[test]
fn update_bookmark_patches_only_the_requested_fields() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let first = store.create_category("First").expect("create first");
    let second = store.create_category("Second").expect("create second");
    let bookmark = store
        .create_bookmark(
            first.id,
            &BookmarkAnchorInput {
                comment: Some("note".to_string()),
                ..BookmarkAnchorInput::default()
            },
        )
        .expect("create bookmark");

    let untouched = store
        .update_bookmark(&bookmark.uuid, Some(second.id), None)
        .expect("move category");
    assert_eq!(untouched.category_id, second.id);
    assert_eq!(untouched.comment.as_deref(), Some("note"));

    let cleared = store
        .update_bookmark(&bookmark.uuid, None, Some(None))
        .expect("clear comment");
    assert_eq!(cleared.comment, None);
    assert_eq!(cleared.category_id, second.id);
}
