use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::{TempDir, tempdir};

struct Fixture {
    _root: TempDir,
    project: PathBuf,
    cache: PathBuf,
    database: PathBuf,
    generation: String,
    bookmark_id: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempdir().expect("fixture");
        let project = root.path().join("project");
        let cache = root.path().join("cache");
        fs::create_dir(&project).expect("project");
        fs::write(
            project.join("lib.rs"),
            "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { alpha() }\n",
        )
        .expect("source with edge");
        let mut fixture = Self {
            _root: root,
            project,
            cache,
            database: PathBuf::new(),
            generation: String::new(),
            bookmark_id: String::new(),
        };
        let seeded = fixture.success(&["index", "--refresh", "full", "--format", "json"]);
        assert!(
            seeded["summary"]["stats"]["node_count"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );
        let pointer: Value = serde_json::from_slice(
            &fs::read(fixture.cache.join("core/publication.json")).expect("complete pointer"),
        )
        .expect("pointer JSON");
        fixture.generation = pointer["active"]["generation_id"]
            .as_str()
            .expect("generation")
            .to_owned();
        fixture.database = fixture
            .cache
            .join("core/generations")
            .join(&fixture.generation)
            .join("codestory.db");
        let connection =
            Connection::open_with_flags(&fixture.database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("read seeded database");
        for table in ["node", "edge", "index_publication"] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("nonempty complete seed");
            assert!(count > 0, "{table} must be meaningful");
        }
        let alpha: i64 = connection
            .query_row(
                "SELECT id FROM node WHERE serialized_name='alpha' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("alpha");
        drop(connection);
        let bookmark = fixture.success(&[
            "bookmark",
            "add",
            "--id",
            &alpha.to_string(),
            "--comment",
            "keep this note",
            "--format",
            "json",
        ]);
        fixture.bookmark_id = bookmark["bookmark"]["bookmark"]["id"]
            .as_str()
            .expect("actual bookmark id")
            .to_owned();
        assert!(!snapshot_annotations(&fixture.cache).is_empty());
        fs::write(
            fixture.cache.join("unrelated.sqlite3"),
            b"unrelated cache sibling",
        )
        .expect("unrelated sibling");
        fs::create_dir(fixture.cache.join("unrelated-tree")).expect("unrelated tree");
        fs::write(fixture.cache.join("unrelated-tree/note"), b"keep tree").expect("unrelated file");
        fs::write(
            fixture.cache.join("core/unrelated-note"),
            b"keep core sibling",
        )
        .expect("unrelated core sibling");
        // Registered retained user state; this fixture does not claim that a
        // current producer creates this nonempty migration export.
        fs::write(
            fixture.cache.join("annotations.pre-migration.json"),
            b"{\"retained_note\":\"keep migration export\"}",
        )
        .expect("retained export fixture");
        fixture
    }

    fn set_forward_schema(&self) {
        // Fault a real complete current generation to model a future writer.
        // This is not an archived schema fixture or a newer-binary claim.
        let original = fs::metadata(&self.database)
            .expect("metadata")
            .permissions();
        let mut writable = original.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            writable.set_mode(writable.mode() | 0o200);
        }
        #[cfg(windows)]
        writable.set_readonly(false);
        let original_bytes = fs::read(&self.database).expect("sealed database bytes");
        let original_annotations = snapshot_annotations(&self.cache);
        let original_pointer = fs::read(self.cache.join("core/publication.json")).expect("pointer");
        let mut bytes = original_bytes.clone();
        assert_eq!(&bytes[..16], b"SQLite format 3\0");
        // SQLite's user_version is the big-endian word at header offset 60.
        // Change exactly this fault boundary, without enabling a WAL writer.
        bytes[60..64].copy_from_slice(&36_u32.to_be_bytes());
        fs::set_permissions(&self.database, writable).expect("fixture writable");
        fs::write(&self.database, bytes).expect("forward-schema header fault");
        fs::set_permissions(&self.database, original).expect("restore sealed permissions");
        let changed_bytes = fs::read(&self.database).expect("forward database bytes");
        assert_eq!(&changed_bytes[..60], &original_bytes[..60]);
        assert_eq!(&changed_bytes[64..], &original_bytes[64..]);
        assert_eq!(snapshot_annotations(&self.cache), original_annotations);
        assert_eq!(
            fs::read(self.cache.join("core/publication.json")).unwrap(),
            original_pointer
        );
        let reader = Connection::open_with_flags(&self.database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read forward-schema fixture");
        let version: u32 = reader
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("forward schema version");
        assert_eq!(version, 36);
        for table in ["node", "edge", "index_publication"] {
            let count: i64 = reader
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("retained complete graph/publication");
            assert!(count > 0, "{table} must survive fault injection");
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        test_support::cli_command()
            .args(args)
            .arg("--project")
            .arg(&self.project)
            .arg("--cache-dir")
            .arg(&self.cache)
            .env(
                "CODESTORY_CACHE_ROOT",
                self._root.path().join("global-cache"),
            )
            .env(
                "CODESTORY_STDIO_CACHE_ROOT",
                self._root.path().join("stdio-cache"),
            )
            .env(
                "CODESTORY_PLUGIN_DATA",
                self._root.path().join("plugin-data"),
            )
            .output()
            .expect("CLI process")
    }

    fn success(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        serde_json::from_slice(&output.stdout).expect("CLI JSON")
    }
    fn runtime_config(&self) -> codestory_runtime::RuntimeRetrievalConfig {
        codestory_runtime::RuntimeRetrievalConfig::for_project_auto_with_process_defaults(
            &self.project,
            &codestory_runtime::RetrievalProcessDefaults::new(
                self._root.path().join("stdio-cache"),
                codestory_runtime::RetrievalRuntimeDefaults::default(),
            ),
            &codestory_runtime::RetrievalRuntimeOverrides::default(),
        )
    }
}

#[test]
fn dry_run_identifies_newer_immutable_core_without_mutation() {
    let fixture = Fixture::new();
    fixture.set_forward_schema();
    let before = snapshot_tree(&fixture.cache);
    let output = fixture.success(&[
        "cache",
        "reset",
        "--derived-only",
        "--dry-run",
        "--format",
        "json",
    ]);
    assert_eq!(output["applied"], false);
    assert_eq!(
        snapshot_tree(&fixture.cache),
        before,
        "dry-run mutated cache"
    );
    let planned = output["quarantined"].as_array().expect("reset plan");
    assert!(
        planned.iter().any(|path| path == "core/publication.json"),
        "{output}"
    );
    assert!(
        planned.iter().any(|path| path == "core/generations"),
        "{output}"
    );
    assert!(!planned.iter().any(|path| path == "core/acquisition.lock"));
    assert!(!planned.iter().any(|path| path == "annotations.sqlite3"));
    assert!(!planned.iter().any(|path| path == "unrelated.sqlite3"));
}

#[test]
fn confirmed_reset_recovers_newer_immutable_core_and_preserves_annotations() {
    let fixture = Fixture::new();
    fixture.set_forward_schema();
    let refused = fixture.run(&["index", "--refresh", "full", "--format", "json"]);
    assert!(
        !refused.status.success(),
        "newer schema must refuse ordinary full refresh"
    );
    let refused_json: Value = serde_json::from_slice(&refused.stdout).expect("JSON error");
    assert!(
        refused_json["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Unsupported database schema version: 36")),
        "{refused:?}"
    );
    let annotations = snapshot_annotations(&fixture.cache);
    let core = fs::read(&fixture.database).expect("old core");
    let coordination = coordination_identities(&fixture.cache);
    let output = fixture.success(&[
        "cache",
        "reset",
        "--derived-only",
        "--confirm",
        "--format",
        "json",
    ]);
    assert_eq!(output["applied"], true);
    assert_eq!(
        snapshot_annotations(&fixture.cache),
        annotations,
        "annotation bytes changed"
    );
    assert_eq!(
        fs::read(fixture.cache.join("unrelated.sqlite3")).unwrap(),
        b"unrelated cache sibling"
    );
    assert_eq!(
        fs::read(fixture.cache.join("unrelated-tree/note")).unwrap(),
        b"keep tree"
    );
    assert_eq!(
        fs::read(fixture.cache.join("core/unrelated-note")).unwrap(),
        b"keep core sibling"
    );
    assert_eq!(
        coordination_identities(&fixture.cache),
        coordination,
        "reset split a coordination inode"
    );

    // Capture the actual suggested next interaction even on the red baseline.
    let rebuilt = fixture.run(&["index", "--refresh", "full", "--format", "json"]);
    assert!(
        rebuilt.status.success(),
        "reset left a newer immutable core blocking recovery: reset={output}; next={rebuilt:?}"
    );
    let quarantine = PathBuf::from(output["quarantine_dir"].as_str().expect("quarantine"));
    assert_eq!(
        fs::read(
            quarantine
                .join("core/generations")
                .join(&fixture.generation)
                .join("codestory.db")
        )
        .unwrap(),
        core
    );
    assert!(quarantine.join("core/publication.json").is_file());
    for coordination in [
        "codestory.index-writer.lock",
        "codestory.db.promotion.lock",
        "core/acquisition.lock",
    ] {
        assert!(
            fixture.cache.join(coordination).is_file(),
            "coordination disappeared: {coordination}"
        );
        assert!(!quarantine.join(coordination).exists());
    }
    let summary: Value = serde_json::from_slice(&rebuilt.stdout).expect("rebuilt output");
    assert!(
        summary["summary"]["stats"]["node_count"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    let bookmarks = fixture.success(&["bookmark", "list", "--format", "json"]);
    assert!(
        bookmarks.to_string().contains(&fixture.bookmark_id),
        "saved annotation lost: {bookmarks}"
    );
    assert!(bookmarks.to_string().contains("keep this note"));
}

fn coordination_identities(
    cache: &Path,
) -> BTreeMap<PathBuf, codestory_workspace::WorkspacePathIdentity> {
    [
        "codestory.index-writer.lock",
        "codestory.db.promotion.lock",
        "core/acquisition.lock",
    ]
    .into_iter()
    .map(|name| {
        let path = cache.join(name);
        let identity =
            codestory_workspace::workspace_path_identity(&path).expect("native lock identity");
        (path, identity)
    })
    .collect()
}

#[test]
fn reset_refuses_actual_pinned_core_reader_then_retries() {
    let fixture = Fixture::new();
    let runtime = codestory_runtime::Runtime::new_with_config(
        fixture.runtime_config().as_raw_config_for_test().clone(),
    );
    let summary = runtime
        .project_service()
        .open_core_read_only_with_storage_path(
            fixture.project.clone(),
            fixture.cache.join("codestory.db"),
        )
        .expect("attach complete core");
    assert!(summary.publication.is_some());
    let before = snapshot_tree(&fixture.cache);
    runtime
        .public_operation_service()
        .run_with_cancel(
            "graph",
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            || {
                let pinned = runtime
                    .public_operation_service()
                    .active_project_summary()?;
                assert!(pinned.stats.node_count > 0);
                let refused = fixture.run(&[
                    "cache",
                    "reset",
                    "--derived-only",
                    "--confirm",
                    "--format",
                    "json",
                ]);
                assert!(
                    !refused.status.success(),
                    "reset moved a pinned generation: {refused:?}"
                );
                assert!(
                    String::from_utf8_lossy(&refused.stdout).contains("Core reader is active"),
                    "{refused:?}"
                );
                assert_eq!(
                    snapshot_tree(&fixture.cache),
                    before,
                    "busy reader reset changed cache"
                );
                Ok(())
            },
        )
        .expect("actual pinned graph operation");
    fixture.success(&[
        "cache",
        "reset",
        "--derived-only",
        "--confirm",
        "--format",
        "json",
    ]);
    assert!(!fixture.cache.join("core/publication.json").exists());
}

#[test]
fn reset_refuses_retrieval_publication_fence_then_retries() {
    let fixture = Fixture::new();
    let config = fixture.runtime_config();
    let raw = config.as_raw_config_for_test();
    let fence = codestory_retrieval::GenerationRetentionLock::acquire_shared(
        &codestory_retrieval::global_generation_gc_state_file(raw),
        codestory_retrieval::GLOBAL_GENERATION_GC_LOCK_SCOPE,
    )
    .expect("publisher-shaped global fence");
    let before = snapshot_tree(&fixture.cache);
    let refused = fixture.run(&[
        "cache",
        "reset",
        "--derived-only",
        "--confirm",
        "--format",
        "json",
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stdout)
            .contains("Retrieval reader or publisher is active"),
        "{refused:?}"
    );
    assert_eq!(snapshot_tree(&fixture.cache), before);
    drop(fence);
    fixture.success(&[
        "cache",
        "reset",
        "--derived-only",
        "--confirm",
        "--format",
        "json",
    ]);
    assert!(!fixture.cache.join("core/publication.json").exists());
}

#[test]
fn reset_refuses_unprovisioned_generation_before_any_move() {
    let fixture = Fixture::new();
    let lease = fixture
        .database
        .parent()
        .unwrap()
        .join(".codestory-core-lease.lock");
    let marker = fs::read(&lease).expect("lease");
    fs::remove_file(&lease).expect("simulate old unprovisioned layout");
    let before = snapshot_tree(&fixture.cache);
    let refused = fixture.run(&[
        "cache",
        "reset",
        "--derived-only",
        "--confirm",
        "--format",
        "json",
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stdout).contains("unprovisioned core generation"),
        "{refused:?}"
    );
    assert_eq!(snapshot_tree(&fixture.cache), before);
    fs::write(&lease, marker).expect("restore fixture lease");
    fixture.success(&[
        "cache",
        "reset",
        "--derived-only",
        "--confirm",
        "--format",
        "json",
    ]);
    assert!(!fixture.cache.join("core/publication.json").exists());
}

fn snapshot_annotations(cache: &Path) -> BTreeMap<PathBuf, String> {
    snapshot_tree(cache)
        .into_iter()
        .filter(|(path, _)| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy().starts_with("annotations.sqlite3")
                    || name == "annotations.pre-migration.json"
            })
        })
        .collect()
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, String> {
    fn visit(root: &Path, directory: &Path, output: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(directory).expect("directory") {
            let entry = entry.expect("entry");
            if entry.file_type().expect("type").is_dir() {
                visit(root, &entry.path(), output);
            } else {
                output.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    format!(
                        "{:x}",
                        Sha256::digest(fs::read(entry.path()).expect("bytes"))
                    ),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

mod test_support;
