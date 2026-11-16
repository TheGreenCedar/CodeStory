use super::resolution::{CoreAnchorCandidate, resolve_bookmark};
use super::*;
use crate::StorageError;
use tempfile::TempDir;

fn binding(root: &Path, token: &str) -> NativeRootBinding {
    NativeRootBinding::new(Some(token.to_string()), root)
}

fn open_store(dir: &TempDir, token: &str) -> AnnotationStore {
    let path = dir.path().join("annotations.sqlite3");
    AnnotationStore::open_for_write(&path, &binding(dir.path(), token)).expect("open sidecar")
}

#[test]
fn rebind_batch_rolls_back_earlier_bookmarks_when_a_later_write_fails() {
    let dir = tempfile::tempdir().expect("annotation directory");
    let store = open_store(&dir, "root");
    let category = store.create_category("Saved").expect("category");
    let first = store
        .create_bookmark(category.id, &BookmarkAnchorInput::default())
        .expect("first bookmark");
    let second = store
        .create_bookmark(category.id, &BookmarkAnchorInput::default())
        .expect("second bookmark");
    let before = store.bookmarks(None).expect("original bookmarks");
    store
        .conn
        .execute_batch(&format!(
            "CREATE TRIGGER refuse_second_rebind BEFORE UPDATE ON bookmark
             WHEN NEW.uuid = '{}' BEGIN SELECT RAISE(ABORT, 'rebind fault'); END;",
            second.uuid
        ))
        .expect("install scoped write fault");
    let orphan = AnnotationResolution::Orphaned {
        reason: OrphanReason::TargetDeleted,
    };
    store
        .apply_resolutions(&[(first.uuid, orphan.clone()), (second.uuid, orphan)])
        .expect_err("later write error aborts the batch");
    assert_eq!(
        store.bookmarks(None).expect("bookmarks after abort"),
        before
    );
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
            normalized_signature: Some("shape:111".to_string()),
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

fn durable_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(dir)
        .expect("read annotation directory")
        .map(|entry| {
            let entry = entry.expect("directory entry");
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("read durable file"),
            )
        })
        .collect()
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

    // Closed database: the rejection must not alter durable bytes.
    let before = durable_tree(dir.path());
    let error = AnnotationStore::open_observational(&path).expect_err("newer schema fails closed");
    assert!(
        matches!(error, AnnotationError::UnsupportedSchema { found, .. } if found == ANNOTATION_SCHEMA_VERSION + 1),
        "unexpected error: {error}"
    );
    let after = durable_tree(dir.path());
    let wal = after.get("annotations.sqlite3-wal");
    assert_eq!(
        after.get("annotations.sqlite3"),
        before.get("annotations.sqlite3"),
        "the rejected observational open changed the durable database image"
    );
    assert!(
        wal.is_none_or(|bytes| bytes.is_empty()),
        "the rejected observational open journaled frames into the WAL"
    );
}

#[test]
fn observational_open_does_not_mutate_pending_recovery_state() {
    // Pending-recovery fixture: keep the writer connection alive so the
    // committed schema rows stay in the -wal sidecar while the observational
    // path runs against the un-checkpointed database.
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("annotations.sqlite3");
    let pending = open_store(&dir, "unix:2:1");
    let wal_path = dir.path().join("annotations.sqlite3-wal");
    assert!(wal_path.is_file(), "fixture must retain a populated WAL");
    let before = durable_tree(dir.path());

    let observed = AnnotationStore::open_observational(&path)
        .expect("observational open")
        .expect("current-schema sidecar is observable");
    assert_eq!(
        observed.schema_version().expect("observed schema version"),
        ANNOTATION_SCHEMA_VERSION
    );
    drop(observed);

    let after = durable_tree(dir.path());
    assert_eq!(
        after.get("annotations.sqlite3"),
        before.get("annotations.sqlite3"),
        "observation changed the durable database image"
    );
    assert_eq!(
        after.get("annotations.sqlite3-wal"),
        before.get("annotations.sqlite3-wal"),
        "observation appended to or truncated the pending WAL"
    );
    drop(pending);
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
fn an_import_that_dies_part_way_through_leaves_no_journal_row_and_reimports() {
    // The journal row and the imported rows have to reach disk together. If
    // the journal commits first, a death before the rows land leaves a sidecar
    // that says "already imported" and holds nothing, and every annotation the
    // user ever made is gone while the legacy tables still hold them.
    //
    // Merely reopening a fresh sidecar cannot see that: it is indistinguishable
    // from a brand-new one. So this kills the import *between* the two writes,
    // with a trigger that aborts the first row insert, and then asserts the
    // journal did not survive on its own.
    let dir = TempDir::new().expect("temp dir");
    let backup = dir.path().join("annotations.pre-migration.json");
    let path = dir.path().join("annotations.sqlite3");
    {
        let mut store = open_store(&dir, "unix:1:1");
        store
            .conn
            .execute(
                "CREATE TRIGGER die_part_way_through BEFORE INSERT ON bookmark
                 BEGIN SELECT RAISE(ABORT, 'simulated death during the import'); END",
                [],
            )
            .expect("install the failure");

        let error = store
            .import_core_annotations(&legacy_snapshot(), &backup)
            .expect_err("the import must fail");
        assert!(
            error.to_string().contains("simulated death"),
            "unexpected error: {error}"
        );
        assert!(
            !store.core_import_completed().expect("journal"),
            "a journal row must not outlive the rows it claims were imported"
        );
        assert!(
            store.bookmarks(None).expect("bookmarks").is_empty(),
            "the failed import must leave nothing behind"
        );
    }

    // Restart against untouched legacy tables: the import has to run again and
    // restore every annotation.
    let mut restarted = AnnotationStore::open_for_write(&path, &binding(dir.path(), "unix:1:1"))
        .expect("restart sidecar");
    restarted
        .conn
        .execute("DROP TRIGGER die_part_way_through", [])
        .expect("clear the failure");
    assert!(
        restarted
            .import_core_annotations(&legacy_snapshot(), &backup)
            .expect("import after restart"),
        "a restart must re-run an import that never committed"
    );
    let restored = restarted.bookmarks(None).expect("bookmarks");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].comment.as_deref(), Some("keep"));
    assert_eq!(restarted.categories().expect("categories").len(), 1);
}

#[test]
fn a_restart_before_any_import_still_imports() {
    let dir = TempDir::new().expect("temp dir");
    let backup = dir.path().join("annotations.pre-migration.json");
    let path = dir.path().join("annotations.sqlite3");
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
fn an_imported_annotation_keeps_addressing_its_legacy_row_id() {
    // A pre-cutover read hands out legacy row ids, and the write that follows
    // is often the one that performs the cutover. The imported uuid is derived
    // from the legacy id so that id still addresses the same annotation.
    let dir = TempDir::new().expect("temp dir");
    let backup = dir.path().join("annotations.pre-migration.json");
    let mut store = open_store(&dir, "unix:1:1");
    let snapshot = legacy_snapshot();
    let legacy_id = snapshot.bookmarks[0].id;

    assert!(
        store
            .import_core_annotations(&snapshot, &backup)
            .expect("import")
    );

    let addressed = store
        .bookmark(&legacy_bookmark_uuid(legacy_id))
        .expect("lookup")
        .expect("the legacy id still addresses the imported annotation");
    assert_eq!(addressed.comment.as_deref(), Some("keep"));
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
    // Every durable anchor field supplied by the fixture must survive the
    // round trip; each is fallback evidence for later rebinding.
    assert_eq!(
        imported[0].canonical_id.as_deref(),
        Some("codestory:symbol:alpha")
    );
    assert_eq!(
        imported[0].file_identity.as_deref(),
        Some("/repo/src/lib.rs")
    );
    assert_eq!(imported[0].qualified_name.as_deref(), Some("alpha"));
    assert_eq!(imported[0].kind, Some(3));
    assert_eq!(imported[0].normalized_signature.as_deref(), Some("111"));
    assert_eq!(imported[0].start_line, Some(10));
    assert_eq!(imported[0].comment.as_deref(), Some("note"));
    let categories = target.categories().expect("categories");
    assert_eq!(categories.len(), 1);
    assert_eq!(imported[0].category_id, categories[0].id);
    assert_eq!(categories[0].name, "Favorites");
}

/// One workspace's worth of anchor candidates.
///
/// The fake holds *symbols*, not per-lookup answer lists, and derives every
/// lookup from them the way core does. Hand-keying each lookup separately made
/// it possible to describe a workspace no indexer could ever produce — for
/// instance one where a symbol answers a signature probe under a name it does
/// not have — which is exactly how a dead rebind ladder passed its unit tests.
#[derive(Default)]
struct FakeCore {
    generation: Option<i64>,
    symbols: Vec<CoreAnchorCandidate>,
}

impl FakeCore {
    fn at_generation(generation: i64) -> Self {
        Self {
            generation: Some(generation),
            symbols: Vec::new(),
        }
    }

    fn with(mut self, candidate: CoreAnchorCandidate) -> Self {
        self.symbols.push(candidate);
        self
    }
}

impl CoreAnchorIndex for FakeCore {
    fn current_generation(&self) -> Result<Option<i64>, StorageError> {
        Ok(self.generation)
    }

    fn candidates_by_canonical_id(
        &self,
        canonical_id: &str,
    ) -> Result<Vec<CoreAnchorCandidate>, StorageError> {
        Ok(self
            .symbols
            .iter()
            .filter(|candidate| candidate.canonical_id.as_deref() == Some(canonical_id))
            .cloned()
            .collect())
    }

    fn candidates_by_anchor_tuple(
        &self,
        file_identity: &str,
        qualified_name: &str,
        kind: i64,
    ) -> Result<Vec<CoreAnchorCandidate>, StorageError> {
        Ok(self
            .symbols
            .iter()
            .filter(|candidate| {
                candidate.file_identity.as_deref() == Some(file_identity)
                    && candidate.qualified_name.as_deref() == Some(qualified_name)
                    && candidate.kind == Some(kind)
            })
            .cloned()
            .collect())
    }

    fn candidates_by_qualified_name(
        &self,
        qualified_name: &str,
        kind: i64,
    ) -> Result<Vec<CoreAnchorCandidate>, StorageError> {
        Ok(self
            .symbols
            .iter()
            .filter(|candidate| {
                candidate.qualified_name.as_deref() == Some(qualified_name)
                    && candidate.kind == Some(kind)
            })
            .cloned()
            .collect())
    }

    fn candidates_by_normalized_signature(
        &self,
        normalized_signature: &str,
        file_identity: &str,
        kind: i64,
    ) -> Result<Vec<CoreAnchorCandidate>, StorageError> {
        Ok(self
            .symbols
            .iter()
            .filter(|candidate| {
                candidate.normalized_signature.as_deref() == Some(normalized_signature)
                    && candidate.file_identity.as_deref() == Some(file_identity)
                    && candidate.kind == Some(kind)
            })
            .cloned()
            .collect())
    }
}

fn candidate(node_id: i64, file: &str, qualified_name: &str) -> CoreAnchorCandidate {
    CoreAnchorCandidate {
        node_id,
        canonical_id: None,
        file_identity: Some(file.to_string()),
        qualified_name: Some(qualified_name.to_string()),
        kind: Some(3),
        normalized_signature: Some(ALPHA_SIGNATURE.to_string()),
        start_line: Some(40),
    }
}

fn candidate_with_signature(
    node_id: i64,
    file: &str,
    qualified_name: &str,
    signature: &str,
) -> CoreAnchorCandidate {
    CoreAnchorCandidate {
        normalized_signature: Some(signature.to_string()),
        ..candidate(node_id, file, qualified_name)
    }
}

const ALPHA_SIGNATURE: &str = "shape:111";
const OTHER_SIGNATURE: &str = "shape:222";
/// A signature with no body evidence behind it: only a kind and a line count.
const OUTLINE_SIGNATURE: &str = "outline:111";

fn discriminating() -> AnchorDiscrimination {
    AnchorDiscrimination {
        anchor_tuple_unique: true,
        signature_unique_in_file: true,
        qualified_name_unique: true,
    }
}

fn anchored_bookmark(store: &AnnotationStore, generation: Option<i64>) -> AnnotationBookmark {
    anchored_bookmark_with(store, generation, Some(discriminating()), ALPHA_SIGNATURE)
}

fn anchored_bookmark_with_discrimination(
    store: &AnnotationStore,
    generation: Option<i64>,
    discrimination: Option<AnchorDiscrimination>,
) -> AnnotationBookmark {
    anchored_bookmark_with(store, generation, discrimination, ALPHA_SIGNATURE)
}

fn anchored_bookmark_with(
    store: &AnnotationStore,
    generation: Option<i64>,
    discrimination: Option<AnchorDiscrimination>,
    signature: &str,
) -> AnnotationBookmark {
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
                normalized_signature: Some(signature.to_string()),
                start_line: Some(10),
                comment: None,
                evidence: Some(BookmarkAnchorEvidence {
                    generation,
                    node_id: Some(7),
                    canonical_id: None,
                    file_identity: Some("/repo/src/lib.rs".to_string()),
                    qualified_name: Some("alpha".to_string()),
                    kind: Some(3),
                    normalized_signature: Some(signature.to_string()),
                    start_line: Some(10),
                    discrimination,
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
    let core = FakeCore::at_generation(5).with(candidate(99, "/repo/src/lib.rs", "alpha"));

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve bookmark");

    assert_eq!(resolution.node_id(), Some(99));
    assert_eq!(resolution.status(), ResolutionStatus::Bound);
}

#[test]
fn a_bind_records_how_well_its_evidence_separated_the_symbol() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let crowded = FakeCore::at_generation(5)
        .with(candidate(99, "/repo/src/lib.rs", "alpha"))
        // A same-shaped sibling in the same file: the signature does not
        // separate the annotated symbol from it.
        .with(candidate(100, "/repo/src/lib.rs", "sibling"))
        // The same name in another file: the name does not separate it either.
        .with(candidate(101, "/repo/src/other.rs", "alpha"));

    let AnnotationResolution::Bound { evidence, .. } =
        resolve_bookmark(&bookmark, &crowded).expect("resolve bookmark")
    else {
        panic!("the exact anchor tuple still binds");
    };
    assert_eq!(
        evidence.discrimination,
        Some(AnchorDiscrimination {
            anchor_tuple_unique: true,
            signature_unique_in_file: false,
            qualified_name_unique: false,
        })
    );

    let uncrowded = FakeCore::at_generation(5)
        .with(candidate(99, "/repo/src/lib.rs", "alpha"))
        .with(candidate_with_signature(
            100,
            "/repo/src/lib.rs",
            "sibling",
            OTHER_SIGNATURE,
        ));
    let AnnotationResolution::Bound { evidence, .. } =
        resolve_bookmark(&bookmark, &uncrowded).expect("resolve bookmark")
    else {
        panic!("the exact anchor tuple still binds");
    };
    assert_eq!(evidence.discrimination, Some(discriminating()));

    let repeated_name = FakeCore::at_generation(4)
        .with(candidate(99, "/repo/src/lib.rs", "alpha"))
        .with(candidate_with_signature(
            100,
            "/repo/src/lib.rs",
            "alpha",
            OTHER_SIGNATURE,
        ));
    let evidence = anchor_evidence(
        &candidate(99, "/repo/src/lib.rs", "alpha"),
        Some(4),
        &repeated_name,
    )
    .expect("record old Core discrimination before replacement");
    assert_eq!(
        evidence.discrimination,
        Some(AnchorDiscrimination {
            anchor_tuple_unique: false,
            signature_unique_in_file: true,
            qualified_name_unique: false,
        })
    );
}

#[test]
fn an_ambiguous_match_never_guesses() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let core = FakeCore::at_generation(5)
        .with(candidate(99, "/repo/src/lib.rs", "alpha"))
        .with(candidate(100, "/repo/src/lib.rs", "alpha"));

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve bookmark");

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
    // `alpha` is gone and a same-shaped symbol under a new name stands in the
    // same file: that is a rename, and nothing else in the file shares the
    // shape.
    let core = FakeCore::at_generation(5)
        .with(candidate(99, "/repo/src/lib.rs", "renamed"))
        .with(candidate_with_signature(
            100,
            "/repo/src/lib.rs",
            "bystander",
            OTHER_SIGNATURE,
        ));

    let adjacent = anchored_bookmark(&store, Some(4));
    let resolution = resolve_bookmark(&adjacent, &core).expect("resolve bookmark");
    assert_eq!(resolution.node_id(), Some(99), "adjacent rename rebinds");

    let stale = anchored_bookmark(&store, Some(1));
    let resolution = resolve_bookmark(&stale, &core).expect("resolve bookmark");
    assert_eq!(
        resolution.orphan_reason(),
        Some(OrphanReason::GenerationGap),
        "unobserved intervening history must not support an inference"
    );
}

#[test]
fn a_rename_is_never_inferred_from_evidence_that_already_matched_a_sibling() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    // The anchor's signature already matched a sibling when it was proven, so
    // the sibling surviving the anchor's deletion is not a rename. Deleting
    // `alpha` leaves exactly one same-shaped candidate, which is precisely the
    // shape of a wrong rebind.
    let bookmark = anchored_bookmark_with_discrimination(
        &store,
        Some(4),
        Some(AnchorDiscrimination {
            anchor_tuple_unique: true,
            signature_unique_in_file: false,
            qualified_name_unique: true,
        }),
    );
    let core = FakeCore::at_generation(5).with(candidate(100, "/repo/src/lib.rs", "sibling"));

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve bookmark");

    assert_eq!(resolution.status(), ResolutionStatus::Orphaned);
    assert_eq!(
        resolution.orphan_reason(),
        Some(OrphanReason::AmbiguousMatch),
        "a surviving same-shaped sibling must never inherit the annotation"
    );
}

#[test]
fn a_rename_is_never_inferred_from_a_signature_with_no_body_behind_it() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    // A stub: kind and line count are all its signature has. Every other stub
    // of the same length carries the same one, so it cannot say that the
    // survivor in the file used to be the annotated symbol.
    let bookmark =
        anchored_bookmark_with(&store, Some(4), Some(discriminating()), OUTLINE_SIGNATURE);
    let core = FakeCore::at_generation(5).with(candidate_with_signature(
        99,
        "/repo/src/lib.rs",
        "some_other_stub",
        OUTLINE_SIGNATURE,
    ));

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve bookmark");

    assert_eq!(resolution.status(), ResolutionStatus::Orphaned);
    assert_eq!(
        resolution.orphan_reason(),
        Some(OrphanReason::TargetDeleted),
        "a signature with no body behind it must not identify a rename"
    );
}

#[test]
fn a_move_still_rebinds_from_a_signature_with_no_body_behind_it() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    // The move probe already knows which symbol it is looking at, because the
    // qualified name identified it and was unique. The signature only has to
    // agree, so a stub still moves.
    let bookmark =
        anchored_bookmark_with(&store, Some(4), Some(discriminating()), OUTLINE_SIGNATURE);
    let core = FakeCore::at_generation(5).with(candidate_with_signature(
        99,
        "/repo/src/moved.rs",
        "alpha",
        OUTLINE_SIGNATURE,
    ));

    assert_eq!(
        resolve_bookmark(&bookmark, &core)
            .expect("resolve bookmark")
            .node_id(),
        Some(99)
    );
}

#[test]
fn a_unique_move_rebinds_and_an_ambiguous_move_orphans() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let moved = FakeCore::at_generation(5).with(candidate(99, "/repo/src/moved.rs", "alpha"));
    assert_eq!(
        resolve_bookmark(&bookmark, &moved)
            .expect("resolve bookmark")
            .node_id(),
        Some(99)
    );

    let ambiguous = FakeCore::at_generation(5)
        .with(candidate(99, "/repo/src/moved.rs", "alpha"))
        .with(candidate(100, "/repo/src/other.rs", "alpha"));
    assert_eq!(
        resolve_bookmark(&bookmark, &ambiguous)
            .expect("resolve bookmark")
            .orphan_reason(),
        Some(OrphanReason::AmbiguousMatch)
    );
}

#[test]
fn a_moved_name_whose_shape_disagrees_is_a_visible_signature_changed_orphan() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    // The name turns up in one other file, but the code there is not the code
    // the user annotated. Sharing a name is not evidence of a move.
    let core = FakeCore::at_generation(5).with(candidate_with_signature(
        99,
        "/repo/src/moved.rs",
        "alpha",
        OTHER_SIGNATURE,
    ));

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve bookmark");

    assert_eq!(resolution.status(), ResolutionStatus::Orphaned);
    assert_eq!(
        resolution.orphan_reason(),
        Some(OrphanReason::SignatureChanged)
    );
}

#[test]
fn a_move_is_never_inferred_from_a_name_that_already_named_two_symbols() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark_with_discrimination(
        &store,
        Some(4),
        Some(AnchorDiscrimination {
            anchor_tuple_unique: true,
            signature_unique_in_file: true,
            qualified_name_unique: false,
        }),
    );
    let core = FakeCore::at_generation(5).with(candidate(99, "/repo/src/other.rs", "alpha"));

    assert_eq!(
        resolve_bookmark(&bookmark, &core)
            .expect("resolve bookmark")
            .orphan_reason(),
        Some(OrphanReason::AmbiguousMatch),
        "a name that was never unique cannot prove where its symbol went"
    );
}

#[test]
fn a_deleted_target_orphans_and_reappearance_rebinds() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = anchored_bookmark(&store, Some(4));
    let core = FakeCore::at_generation(5);

    let orphaned = resolve_bookmark(&bookmark, &core).expect("resolve bookmark");
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

    let core = core.with(candidate(99, "/repo/src/lib.rs", "alpha"));
    let rebound = resolve_bookmark(&stored, &core).expect("resolve bookmark");
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
    let core = FakeCore::at_generation(5)
        .with(CoreAnchorCandidate {
            canonical_id: Some("route_endpoint:GET:/users".to_string()),
            ..candidate(11, "/repo/src/routes.rs", "users_handler")
        })
        .with(candidate(99, "/repo/src/lib.rs", "alpha"));

    assert_eq!(
        resolve_bookmark(&bookmark, &core)
            .expect("resolve bookmark")
            .node_id(),
        Some(11)
    );
}

#[test]
fn rust_ordinal_epoch_does_not_promote_historically_ambiguous_or_missing_evidence() {
    for canonical_id in [Some("/repo/src/lib.rs:alpha#2"), None] {
        for (label, discrimination) in [
            ("no discrimination", None),
            (
                "historical evidence without tuple discrimination",
                Some(serde_json::json!({
                    "signature_unique_in_file": false,
                    "qualified_name_unique": false,
                })),
            ),
            (
                "known ambiguous tuple",
                Some(serde_json::json!({
                    "signature_unique_in_file": false,
                    "qualified_name_unique": false,
                    "anchor_tuple_unique": false,
                })),
            ),
        ] {
            let dir = TempDir::new().expect("temp dir");
            let store = open_store(&dir, "unix:1:1");
            let bookmark = rust_ordinal_bookmark(&store, canonical_id, discrimination);
            let core = FakeCore::at_generation(5).with(CoreAnchorCandidate {
                kind: Some(13),
                canonical_id: Some("rust-ordinal-v2:/repo/src/lib.rs:alpha#0".to_string()),
                ..candidate(99, "/repo/src/lib.rs", "alpha")
            });

            let resolution = resolve_bookmark(&bookmark, &core).expect("resolve Rust epoch");
            assert_eq!(resolution.status(), ResolutionStatus::Orphaned, "{label}");
            assert_eq!(
                resolution.orphan_reason(),
                Some(OrphanReason::AmbiguousMatch),
                "{label}: a tuple shrinking to one must not invent historical uniqueness"
            );
            store
                .apply_resolution(&bookmark.uuid, &resolution)
                .expect("persist visible orphan");
            let stored = store
                .bookmark(&bookmark.uuid)
                .expect("reload orphan")
                .expect("bookmark remains visible");
            assert_eq!(stored.resolution_status, ResolutionStatus::Orphaned);
            assert_eq!(stored.orphan_reason, Some(OrphanReason::AmbiguousMatch));
            assert_eq!(stored.comment.as_deref(), Some("keep Rust bookmark"));
            assert_eq!(stored.canonical_id, bookmark.canonical_id);
            assert_eq!(stored.file_identity, bookmark.file_identity);
            assert_eq!(stored.qualified_name, bookmark.qualified_name);
            assert_eq!(stored.kind, bookmark.kind);
            assert_eq!(stored.normalized_signature, bookmark.normalized_signature);
            assert_eq!(stored.start_line, bookmark.start_line);
            assert_eq!(stored.last_known_evidence, bookmark.last_known_evidence);
            assert!(
                store
                    .bookmarks(None)
                    .expect("list visible bookmarks")
                    .iter()
                    .any(|item| item.uuid == stored.uuid)
            );
            let repeated = resolve_bookmark(&stored, &core).expect("resolve orphan again");
            assert_eq!(repeated.orphan_reason(), Some(OrphanReason::AmbiguousMatch));
            store
                .apply_resolution(&stored.uuid, &repeated)
                .expect("persist repeated orphan");
            let repeated = store.bookmark(&stored.uuid).unwrap().unwrap();
            assert_eq!(repeated.comment, stored.comment);
            assert_eq!(repeated.canonical_id, stored.canonical_id);
            assert_eq!(repeated.last_known_evidence, stored.last_known_evidence);
        }
    }
}

#[test]
fn rust_ordinal_epoch_migrates_a_previously_unique_file_tuple_after_body_edits() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = rust_ordinal_bookmark(
        &store,
        Some("/repo/src/lib.rs:alpha#0"),
        Some(serde_json::json!({
            "signature_unique_in_file": false,
            "qualified_name_unique": false,
            "anchor_tuple_unique": true,
        })),
    );
    let new_key = "rust-ordinal-v2:/repo/src/lib.rs:alpha#0";
    let core = FakeCore::at_generation(5)
        .with(CoreAnchorCandidate {
            kind: Some(13),
            canonical_id: Some(new_key.to_string()),
            start_line: Some(90),
            ..candidate_with_signature(99, "/repo/src/lib.rs", "alpha", OTHER_SIGNATURE)
        })
        .with(CoreAnchorCandidate {
            kind: Some(13),
            canonical_id: Some("rust-ordinal-v2:/repo/src/other.rs:alpha#0".to_string()),
            ..candidate(100, "/repo/src/other.rs", "alpha")
        });

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve unique Rust tuple");
    assert_eq!(resolution.node_id(), Some(99));
    store
        .apply_resolution(&bookmark.uuid, &resolution)
        .expect("persist epoch migration");
    let stored = store
        .bookmark(&bookmark.uuid)
        .expect("reload migrated bookmark")
        .expect("bookmark remains visible");
    assert_eq!(stored.canonical_id.as_deref(), Some(new_key));
    assert_eq!(
        stored.normalized_signature.as_deref(),
        Some(OTHER_SIGNATURE)
    );
    assert_eq!(stored.start_line, Some(90));
    assert_eq!(stored.comment.as_deref(), Some("keep Rust bookmark"));
}

#[test]
fn rust_ordinal_epoch_migrates_a_previously_unique_strong_shape() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let bookmark = rust_ordinal_bookmark(
        &store,
        Some("/repo/src/lib.rs:alpha#0"),
        Some(serde_json::json!({
            "signature_unique_in_file": true,
            "qualified_name_unique": false,
            "anchor_tuple_unique": false,
        })),
    );
    let new_key = "rust-ordinal-v2:/repo/src/lib.rs:alpha#0";
    let core = FakeCore::at_generation(5)
        .with(CoreAnchorCandidate {
            kind: Some(13),
            canonical_id: Some(new_key.to_string()),
            ..candidate(99, "/repo/src/lib.rs", "alpha")
        })
        .with(CoreAnchorCandidate {
            kind: Some(13),
            ..candidate_with_signature(100, "/repo/src/lib.rs", "sibling", OTHER_SIGNATURE)
        });

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve strong Rust shape");
    assert_eq!(resolution.node_id(), Some(99));
    let AnnotationResolution::Bound { evidence, .. } = resolution else {
        panic!("old phantom tuple ambiguity must not discard unique strong evidence");
    };
    assert_eq!(evidence.canonical_id.as_deref(), Some(new_key));
    assert_eq!(
        evidence.normalized_signature.as_deref(),
        Some(ALPHA_SIGNATURE)
    );
}

#[test]
fn rust_ordinal_epoch_rejects_unproven_shapes_and_current_tuple_ambiguity() {
    for (label, previous_signature, current_signature, unique_shape, ambiguous_tuple) in [
        (
            "weak outline",
            Some(OUTLINE_SIGNATURE),
            Some(OUTLINE_SIGNATURE),
            true,
            false,
        ),
        (
            "changed shape",
            Some(ALPHA_SIGNATURE),
            Some(OTHER_SIGNATURE),
            true,
            false,
        ),
        (
            "missing previous shape",
            None,
            Some(ALPHA_SIGNATURE),
            true,
            false,
        ),
        (
            "missing current shape",
            Some(ALPHA_SIGNATURE),
            None,
            true,
            false,
        ),
        (
            "historically nonunique shape",
            Some(ALPHA_SIGNATURE),
            Some(ALPHA_SIGNATURE),
            false,
            false,
        ),
        (
            "ambiguous current tuple",
            Some(ALPHA_SIGNATURE),
            Some(ALPHA_SIGNATURE),
            true,
            true,
        ),
    ] {
        let dir = TempDir::new().expect("temp dir");
        let store = open_store(&dir, "unix:1:1");
        let bookmark = rust_ordinal_bookmark(
            &store,
            Some("/repo/src/lib.rs:alpha#0"),
            Some(serde_json::json!({
                "anchor_tuple_unique": false,
                "signature_unique_in_file": unique_shape,
                "qualified_name_unique": false,
            })),
        );
        let mut evidence = bookmark.last_known_evidence.clone().unwrap();
        evidence.normalized_signature = previous_signature.map(str::to_string);
        store
            .apply_resolution(
                &bookmark.uuid,
                &AnnotationResolution::Bound {
                    node_id: 7,
                    evidence,
                },
            )
            .expect("persist historical shape");
        let bookmark = store.bookmark(&bookmark.uuid).unwrap().unwrap();
        let mut core = FakeCore::at_generation(5).with(CoreAnchorCandidate {
            kind: Some(13),
            canonical_id: Some("rust-ordinal-v2:/repo/src/lib.rs:alpha#0".to_string()),
            normalized_signature: current_signature.map(str::to_string),
            ..candidate(99, "/repo/src/lib.rs", "alpha")
        });
        if ambiguous_tuple {
            core = core.with(CoreAnchorCandidate {
                kind: Some(13),
                canonical_id: Some("rust-ordinal-v2:/repo/src/lib.rs:alpha#1".to_string()),
                ..candidate_with_signature(100, "/repo/src/lib.rs", "alpha", OTHER_SIGNATURE)
            });
        }
        let resolution = resolve_bookmark(&bookmark, &core).expect("resolve Rust shape boundary");
        assert_eq!(
            resolution.orphan_reason(),
            Some(OrphanReason::AmbiguousMatch),
            "{label}"
        );
    }
}

#[test]
fn rust_ordinal_epoch_preserves_canonical_lookup_within_the_epoch() {
    let dir = TempDir::new().expect("temp dir");
    let store = open_store(&dir, "unix:1:1");
    let key = "rust-ordinal-v2:/repo/src/lib.rs:alpha#0";
    let bookmark = rust_ordinal_bookmark(&store, Some(key), None);
    let core = FakeCore::at_generation(5)
        .with(CoreAnchorCandidate {
            kind: Some(13),
            canonical_id: Some(key.to_string()),
            start_line: Some(90),
            ..candidate_with_signature(99, "/repo/src/lib.rs", "alpha", OTHER_SIGNATURE)
        })
        .with(CoreAnchorCandidate {
            kind: Some(13),
            canonical_id: Some("rust-ordinal-v2:/repo/src/lib.rs:alpha#1".to_string()),
            ..candidate(100, "/repo/src/lib.rs", "alpha")
        });

    let resolution = resolve_bookmark(&bookmark, &core).expect("resolve same Rust epoch");
    assert_eq!(
        resolution.node_id(),
        Some(99),
        "same-epoch canonical identity must survive a body edit and line shift"
    );
}

fn rust_ordinal_bookmark(
    store: &AnnotationStore,
    canonical_id: Option<&str>,
    discrimination: Option<serde_json::Value>,
) -> AnnotationBookmark {
    let bookmark = anchored_bookmark_with_discrimination(store, Some(4), None);
    let mut evidence = serde_json::to_value(
        bookmark
            .last_known_evidence
            .as_ref()
            .expect("initial anchor evidence"),
    )
    .expect("serialize historical evidence");
    evidence["canonical_id"] = serde_json::json!(canonical_id);
    evidence["kind"] = serde_json::json!(13);
    evidence["discrimination"] = discrimination.unwrap_or(serde_json::Value::Null);
    store
        .apply_resolution(
            &bookmark.uuid,
            &AnnotationResolution::Bound {
                node_id: 7,
                evidence: serde_json::from_value(evidence).expect("decode historical evidence"),
            },
        )
        .expect("persist historical Rust anchor");
    store
        .update_bookmark(&bookmark.uuid, None, Some(Some("keep Rust bookmark")))
        .expect("retain user comment")
}

#[test]
fn script_ordinal_epoch_requires_historical_discrimination_and_retains_orphans() {
    use codestory_contracts::graph::SCRIPT_ORDINAL_CANONICAL_ID_PREFIX;
    for (
        label,
        tuple_unique,
        shape_unique,
        previous_shape,
        current_shape,
        extra_tuple,
        same_epoch,
        binds,
    ) in [
        (
            "previously unique tuple survives body edits",
            Some(true),
            false,
            ALPHA_SIGNATURE,
            OTHER_SIGNATURE,
            false,
            false,
            true,
        ),
        (
            "previously unique matching strong shape",
            Some(false),
            true,
            ALPHA_SIGNATURE,
            ALPHA_SIGNATURE,
            false,
            false,
            true,
        ),
        (
            "current unique tuple cannot prove historical uniqueness",
            Some(false),
            false,
            ALPHA_SIGNATURE,
            ALPHA_SIGNATURE,
            false,
            false,
            false,
        ),
        (
            "missing historical discrimination",
            None,
            false,
            ALPHA_SIGNATURE,
            ALPHA_SIGNATURE,
            false,
            false,
            false,
        ),
        (
            "historically unique shape changed",
            Some(false),
            true,
            ALPHA_SIGNATURE,
            OTHER_SIGNATURE,
            false,
            false,
            false,
        ),
        (
            "outline is not strong shape evidence",
            Some(false),
            true,
            OUTLINE_SIGNATURE,
            OUTLINE_SIGNATURE,
            false,
            false,
            false,
        ),
        (
            "current tuple remains ambiguous",
            Some(true),
            false,
            ALPHA_SIGNATURE,
            ALPHA_SIGNATURE,
            true,
            false,
            false,
        ),
        (
            "same epoch exact identity survives body edits",
            Some(false),
            false,
            ALPHA_SIGNATURE,
            OTHER_SIGNATURE,
            true,
            true,
            true,
        ),
    ] {
        let dir = TempDir::new().expect("temp dir");
        let store = open_store(&dir, "unix:1:1");
        let category = store.create_category("Script anchors").unwrap();
        let file = "/repo/src/owner.js";
        let new_key = format!("{SCRIPT_ORDINAL_CANONICAL_ID_PREFIX}{file}:alpha#0");
        let old_key = if same_epoch {
            new_key.clone()
        } else {
            format!("{file}:alpha#2")
        };
        let kind = codestory_contracts::graph::NodeKind::FUNCTION as i64;
        let discrimination = tuple_unique.map(|anchor_tuple_unique| AnchorDiscrimination {
            anchor_tuple_unique,
            signature_unique_in_file: shape_unique,
            qualified_name_unique: false,
        });
        let bookmark = store
            .create_bookmark(
                category.id,
                &BookmarkAnchorInput {
                    canonical_id: Some(old_key.clone()),
                    file_identity: Some(file.into()),
                    qualified_name: Some("alpha".into()),
                    kind: Some(kind),
                    normalized_signature: Some(previous_shape.into()),
                    start_line: Some(10),
                    comment: Some("keep script bookmark".into()),
                    evidence: Some(BookmarkAnchorEvidence {
                        generation: Some(4),
                        node_id: Some(7),
                        canonical_id: Some(old_key),
                        file_identity: Some(file.into()),
                        qualified_name: Some("alpha".into()),
                        kind: Some(kind),
                        normalized_signature: Some(previous_shape.into()),
                        start_line: Some(10),
                        discrimination,
                    }),
                },
            )
            .unwrap();
        let mut core = FakeCore::at_generation(5).with(CoreAnchorCandidate {
            canonical_id: Some(new_key.clone()),
            kind: Some(kind),
            ..candidate_with_signature(99, file, "alpha", current_shape)
        });
        if extra_tuple {
            core = core.with(CoreAnchorCandidate {
                canonical_id: Some(format!(
                    "{SCRIPT_ORDINAL_CANONICAL_ID_PREFIX}{file}:alpha#1"
                )),
                kind: Some(kind),
                ..candidate_with_signature(100, file, "alpha", OTHER_SIGNATURE)
            });
        }
        let resolution = resolve_bookmark(&bookmark, &core).unwrap();
        if binds {
            assert_eq!(resolution.node_id(), Some(99), "{label}");
        } else {
            assert_eq!(
                resolution.orphan_reason(),
                Some(OrphanReason::AmbiguousMatch),
                "{label}"
            );
        }
        store.apply_resolution(&bookmark.uuid, &resolution).unwrap();
        let stored = store
            .bookmark(&bookmark.uuid)
            .unwrap()
            .expect("migration must retain bookmark");
        assert_eq!(stored.comment, bookmark.comment, "{label}");
        assert_eq!(
            store.bookmarks(None).unwrap().len(),
            1,
            "{label}: no bookmark deletion"
        );
        if binds {
            assert_eq!(
                stored.canonical_id.as_deref(),
                Some(new_key.as_str()),
                "{label}"
            );
        } else {
            assert_eq!(
                stored.resolution_status,
                ResolutionStatus::Orphaned,
                "{label}"
            );
            assert_eq!(stored.canonical_id, bookmark.canonical_id, "{label}");
            assert_eq!(
                stored.last_known_evidence, bookmark.last_known_evidence,
                "{label}"
            );
            let repeated = resolve_bookmark(&stored, &core).unwrap();
            assert_eq!(
                repeated.orphan_reason(),
                Some(OrphanReason::AmbiguousMatch),
                "{label}"
            );
        }
    }
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
        resolve_bookmark(&bookmark, &FakeCore::default())
            .expect("resolve bookmark")
            .orphan_reason(),
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
