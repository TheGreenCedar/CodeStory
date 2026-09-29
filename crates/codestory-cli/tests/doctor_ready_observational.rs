use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

#[test]
fn doctor_does_not_create_an_absent_cache() {
    assert_absent_cache_is_observational("doctor");
}

#[test]
fn ready_does_not_create_an_absent_cache() {
    assert_absent_cache_is_observational("ready");
}

fn assert_absent_cache_is_observational(command: &str) {
    let fixture = tempdir().expect("fixture");
    let project = fixture.path().join("project");
    fs::create_dir(&project).expect("project");
    fs::write(project.join("lib.rs"), "pub fn alpha() {}\n").expect("source");
    let cache = fixture.path().join("absent-cache");
    assert!(!cache.exists());

    let output = run_cli(&project, &cache, &[command, "--format", "json"]);
    let json: Value = serde_json::from_str(&output).expect("diagnostic json");
    assert!(!cache.exists(), "{command} created the absent cache");
    assert!(!fixture.path().join("plugin-data").exists());
    assert!(!fixture.path().join("global-cache").exists());
    assert!(!fixture.path().join("stdio-cache").exists());
    assert_eq!(json["core_status"], "unavailable", "{command}: {output}");
    assert_unavailable_verdict(command, &json, "No core cache database");
}

#[test]
fn doctor_preserves_a_complete_schema31_cache() {
    assert_schema31_cache_is_observational("doctor");
}

#[test]
fn ready_preserves_a_complete_schema31_cache() {
    assert_schema31_cache_is_observational("ready");
}

#[cfg(unix)]
#[test]
fn doctor_and_ready_report_pending_retirement_without_changing_the_legacy_cache() {
    use std::os::unix::fs::MetadataExt as _;

    let fixture = tempdir().expect("fixture");
    let project = fixture.path().join("project");
    let cache = fixture.path().join("cache");
    prepare_complete_schema31(&project, &cache);
    let database = cache.join("codestory.db");
    let metadata = fs::metadata(&database).expect("legacy native identity");
    let receipt_path = cache.join("core/legacy-retirement.json");
    fs::create_dir_all(receipt_path.parent().expect("receipt directory"))
        .expect("create receipt directory");
    fs::write(
        &receipt_path,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "source_identity": {"Unix": {"device": metadata.dev(), "inode": metadata.ino()}},
            "candidate_generation_id": "generation-2",
            "sidecars": [],
            "committed": false,
            "retired": false
        }))
        .expect("serialize pending receipt"),
    )
    .expect("write pending receipt");
    let before = snapshot_tree(&cache);

    let doctor: Value =
        serde_json::from_str(&run_cli(&project, &cache, &["doctor", "--format", "json"]))
            .expect("doctor json");
    let ready: Value =
        serde_json::from_str(&run_cli(&project, &cache, &["ready", "--format", "json"]))
            .expect("ready json");
    for report in [
        &doctor["sidecar_retrieval"]["legacy_retirement"],
        &ready["legacy_retirement"],
    ] {
        assert_eq!(report["pending"], true, "{report}");
        assert!(report["legacy_bytes"].as_u64().unwrap_or(0) > 0, "{report}");
        assert!(
            report["errors"][0]
                .as_str()
                .is_some_and(|error| error.contains("awaits a committed")),
            "{report}"
        );
    }
    assert_eq!(
        snapshot_tree(&cache),
        before,
        "diagnostics changed the cache"
    );
    assert_eq!(schema_version(&database), 31);
}

#[cfg(unix)]
#[test]
fn retrieval_status_reports_postcommit_pending_retirement_without_cleanup() {
    use std::os::unix::fs::MetadataExt as _;

    let fixture = tempdir().expect("fixture");
    let project = fixture.path().join("project");
    let cache = fixture.path().join("cache");
    prepare_complete_schema31(&project, &cache);
    run_cli(
        &project,
        &cache,
        &["index", "--refresh", "full", "--format", "json"],
    );
    let pointer: Value = serde_json::from_slice(
        &fs::read(cache.join("core/publication.json")).expect("committed core pointer"),
    )
    .expect("core pointer json");
    let active = pointer["active"]["generation_id"]
        .as_str()
        .expect("active generation");
    let rollback = pointer["rollback"]["generation_id"]
        .as_str()
        .expect("rollback generation");
    let legacy = cache.join("codestory.db");
    fs::copy(
        cache
            .join("core/generations")
            .join(rollback)
            .join("codestory.db"),
        &legacy,
    )
    .expect("restore a deferred standalone image for observation");
    let metadata = fs::metadata(&legacy).expect("standalone native identity");
    let receipt_path = cache.join("core/legacy-retirement.json");
    fs::write(
        &receipt_path,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "source_identity": {"Unix": {"device": metadata.dev(), "inode": metadata.ino()}},
            "candidate_generation_id": active,
            "sidecars": [],
            "committed": true,
            "retired": false,
            "last_error": "in-use deletion deferred"
        }))
        .expect("serialize deferred receipt"),
    )
    .expect("write deferred receipt");
    let before = snapshot_tree(&cache);

    let status: Value = serde_json::from_str(&run_cli(
        &project,
        &cache,
        &["retrieval", "status", "--format", "json"],
    ))
    .expect("retrieval status json");
    assert_eq!(status["legacy_retirement"]["pending"], true, "{status}");
    assert!(
        status["legacy_retirement"]["legacy_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(
        status["legacy_retirement"]["errors"][0]
            .as_str()
            .is_some_and(|error| error.contains("in-use deletion deferred"))
    );
    assert_eq!(snapshot_tree(&cache), before, "status performed retirement");
}

fn assert_schema31_cache_is_observational(command: &str) {
    let fixture = tempdir().expect("fixture");
    let project = fixture.path().join("project");
    let cache = fixture.path().join("cache");
    prepare_complete_schema31(&project, &cache);
    let database = cache.join("codestory.db");
    let before = snapshot_tree(&cache);
    let plugin_before = snapshot_tree_if_exists(&fixture.path().join("plugin-data"));
    let process_cache_before = snapshot_tree_if_exists(&fixture.path().join("global-cache"));
    let stdio_cache_before = snapshot_tree_if_exists(&fixture.path().join("stdio-cache"));

    let output = run_cli(&project, &cache, &[command, "--format", "json"]);
    let json: Value = serde_json::from_str(&output).expect("diagnostic json");
    assert_eq!(
        schema_version(&database),
        31,
        "{command} migrated the legacy cache"
    );
    assert_eq!(
        snapshot_tree(&cache),
        before,
        "{command} changed cache files or sidecars"
    );
    assert_eq!(
        snapshot_tree_if_exists(&fixture.path().join("plugin-data")),
        plugin_before
    );
    assert_eq!(
        snapshot_tree_if_exists(&fixture.path().join("global-cache")),
        process_cache_before
    );
    assert_eq!(
        snapshot_tree_if_exists(&fixture.path().join("stdio-cache")),
        stdio_cache_before
    );
    let legacy = Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("inspect preserved legacy database");
    assert_eq!(row_count(&legacy, "bookmark_node"), 1);
    assert_eq!(row_count(&legacy, "index_publication"), 1);
    assert!(row_count(&legacy, "node") > 0);
    assert!(row_count(&legacy, "edge") > 0);
    assert_eq!(
        json["core_status"], "upgrade_required",
        "{command}: {output}"
    );
    assert_unavailable_verdict(command, &json, "schema 31");
}

fn assert_unavailable_verdict(command: &str, json: &Value, reason: &str) {
    let verdicts = if command == "doctor" {
        &json["readiness"]
    } else {
        &json["verdicts"]
    };
    let verdicts = verdicts.as_array().expect("readiness verdicts");
    assert_eq!(verdicts.len(), 2);
    for verdict in verdicts {
        assert_eq!(verdict["status"], "repair_index", "{command}: {json}");
        assert!(
            verdict["summary"]
                .as_str()
                .is_some_and(|text| text.contains(reason)),
            "{command} did not explain the unavailable core: {json}"
        );
        assert!(
            verdict["minimum_next"][0].as_str().is_some_and(|text| text
                .contains("index --project")
                && text.contains("--refresh full")),
            "{command} did not give the managed upgrade step: {json}"
        );
    }
}

/// Seed a real publication before constructing an incompatible generation.
fn prepare_current_generation(project: &Path, cache: &Path) -> PathBuf {
    fs::create_dir(project).expect("project");
    fs::write(
        project.join("lib.rs"),
        "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { alpha() }\n",
    )
    .expect("source");
    let seed: Value = serde_json::from_str(&run_cli(
        project,
        cache,
        &["index", "--refresh", "full", "--format", "json"],
    ))
    .expect("seed index json");
    assert!(seed["summary"]["stats"]["node_count"].as_u64().unwrap_or(0) > 0);
    active_generation_database(cache)
}

fn active_generation_database(cache: &Path) -> PathBuf {
    let pointer: Value = serde_json::from_slice(
        &fs::read(cache.join("core/publication.json")).expect("committed core pointer"),
    )
    .expect("core pointer json");
    cache
        .join("core/generations")
        .join(
            pointer["active"]["generation_id"]
                .as_str()
                .expect("generation id"),
        )
        .join("codestory.db")
}

#[test]
fn doctor_preserves_incompatible_generations_and_reports_recovery() {
    for newer in [false, true] {
        assert_incompatible_generation_is_observational("doctor", newer);
    }
}

#[test]
fn ready_preserves_incompatible_generations_and_reports_recovery() {
    for newer in [false, true] {
        assert_incompatible_generation_is_observational("ready", newer);
    }
}

fn assert_incompatible_generation_is_observational(command: &str, newer: bool) {
    let fixture = tempdir().expect("fixture");
    let project = fixture.path().join("project");
    let cache = fixture.path().join("cache");
    let current = schema_version(&prepare_current_generation(&project, &cache));
    let schema = if newer { current + 1 } else { current - 1 };
    let database = test_support::set_active_core_schema_version(&cache, schema);
    // A plain read-only SQLite open of a WAL database can materialize -shm/-wal
    // in a writable directory, so probe the durable version before the baseline
    // snapshot rather than letting the probe pollute the comparison.
    assert_eq!(schema_version(&database), schema);
    let before = snapshot_tree(&cache);

    let output = run_cli(&project, &cache, &[command, "--format", "json"]);
    let json: Value = serde_json::from_str(&output).expect("diagnostic json");
    assert_eq!(
        schema_version(&database),
        schema,
        "{command} migrated the incompatible generation"
    );
    assert_eq!(
        snapshot_tree(&cache),
        before,
        "{command} changed stale cache files or sidecars"
    );
    if !newer {
        assert_eq!(
            json["core_status"], "upgrade_required",
            "{command}: {output}"
        );
        assert_unavailable_verdict(command, &json, &format!("schema {schema}"));
    } else {
        assert_eq!(json["core_status"], "newer_schema", "{command}: {output}");
        let verdicts = if command == "doctor" {
            &json["readiness"]
        } else {
            &json["verdicts"]
        };
        for verdict in verdicts.as_array().unwrap() {
            assert_eq!(verdict["status"], "repair_index");
            let minimum = verdict["minimum_next"].as_array().unwrap();
            assert!(
                minimum[0].as_str().unwrap().contains("cache reset")
                    && minimum[0].as_str().unwrap().contains("--derived-only")
                    && minimum[0].as_str().unwrap().contains("--dry-run"),
                "{verdict}"
            );
            assert!(
                minimum[1].as_str().unwrap().contains("--confirm"),
                "{verdict}"
            );
            assert!(
                minimum[2].as_str().unwrap().contains("--refresh full"),
                "{verdict}"
            );
            let full = verdict["full_repair"].as_array().unwrap();
            assert_eq!(
                &full[..minimum.len()],
                minimum.as_slice(),
                "full recovery must retain the ordered reset and rebuild steps"
            );
            assert!(full.last().unwrap().as_str().unwrap().contains("doctor"));
        }
        if command == "doctor" {
            assert!(
                json["next_commands"][0]
                    .as_str()
                    .unwrap()
                    .contains("cache reset"),
                "{json}"
            );
        }
    }
}

#[test]
fn doctor_lists_other_cached_projects_with_stale_core_schema() {
    let fixture = tempdir().expect("fixture");
    let project_a = fixture.path().join("project-a");
    let cache_a = fixture.path().join("cache-a");
    fs::create_dir(&project_a).expect("project a");
    fs::write(project_a.join("a.rs"), "pub fn alpha() {}\n").expect("source a");
    run_cli(
        &project_a,
        &cache_a,
        &["index", "--refresh", "full", "--format", "json"],
    );

    // A second project indexed through the derived process cache root lands
    // beside other project caches exactly as an operator's ambient cache does.
    let project_b = fixture.path().join("project-b");
    fs::create_dir(&project_b).expect("project b");
    fs::write(project_b.join("b.rs"), "pub fn beta() {}\n").expect("source b");
    // The derived project cache lives under the process cache root, which the
    // CLI resolves from CODESTORY_STDIO_CACHE_ROOT when it is set.
    let process_root = fixture.path().join("stdio-cache");
    let indexed = test_support::cli_command()
        .args(["index", "--refresh", "full", "--format", "json"])
        .arg("--project")
        .arg(&project_b)
        .env("CODESTORY_CACHE_ROOT", fixture.path().join("global-cache"))
        .env("CODESTORY_STDIO_CACHE_ROOT", &process_root)
        .env("CODESTORY_PLUGIN_DATA", fixture.path().join("plugin-data"))
        .env("CODESTORY_TEST_EMBED_ALLOW_CPU", "1")
        .output()
        .expect("run sibling index");
    assert!(
        indexed.status.success(),
        "sibling index failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&indexed.stdout),
        String::from_utf8_lossy(&indexed.stderr)
    );
    // The process cache root also holds non-project directories (retention,
    // models); derive the sibling's cache the same way the CLI does.
    let cache_b = process_root.join(codestory_workspace::workspace_id_v3_for_root(&project_b));
    assert!(
        cache_b.join("core").is_dir(),
        "derived sibling cache missing: {}",
        cache_b.display()
    );
    let current = schema_version(&active_generation_database(&cache_b));
    for schema in [current - 1, current + 1] {
        let database_b = test_support::set_active_core_schema_version(&cache_b, schema);
        let process_before = snapshot_tree(&process_root);

        let output = run_cli(&project_a, &cache_a, &["doctor", "--format", "json"]);
        let json: Value = serde_json::from_str(&output).expect("doctor json");
        let stale = json["stale_cached_cores"]
            .as_array()
            .expect("stale_cached_cores array");
        let entry = stale
            .iter()
            .find(|entry| entry["found_schema"].as_u64() == Some(schema as u64))
            .unwrap_or_else(|| panic!("doctor must report the stale sibling cache: {output}"));
        let required = entry["required_schema"].as_u64().expect("required schema");
        assert!(
            required == current as u64,
            "required schema must be the current core schema: {entry}"
        );
        {
            let root = entry["project_root"]
                .as_str()
                .expect("attributed sibling project root");
            let action = entry["next_action"].as_str().expect("next action");
            if schema > current {
                assert!(
                    action.contains("cache reset")
                        && action.contains("--derived-only")
                        && action.contains("--dry-run"),
                    "newer sibling must name derived-cache recovery: {action}"
                );
            } else {
                assert!(
                    action.contains("index --project") && action.contains("--refresh full"),
                    "older sibling must name full refresh: {action}"
                );
            }
            assert!(
                action.contains(root),
                "next action must target the sibling's project root: {action}"
            );
        }
        assert_eq!(
            snapshot_tree(&process_root),
            process_before,
            "the cross-project scan must not create, migrate, or recover cache state"
        );
        assert_eq!(schema_version(&database_b), schema);
    }
}

fn prepare_complete_schema31(project: &Path, cache: &Path) {
    fs::create_dir(project).expect("project");
    fs::write(
        project.join("lib.rs"),
        "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { alpha() }\n",
    )
    .expect("source with call edge");
    let seed: Value = serde_json::from_str(&run_cli(
        project,
        cache,
        &["index", "--refresh", "full", "--format", "json"],
    ))
    .expect("seed index json");
    assert!(seed["summary"]["stats"]["node_count"].as_u64().unwrap_or(0) > 0);

    let pointer: Value = serde_json::from_slice(
        &fs::read(cache.join("core/publication.json")).expect("seed pointer"),
    )
    .expect("pointer json");
    let generation = pointer["active"]["generation_id"]
        .as_str()
        .expect("active generation");
    let seed_database = cache
        .join("core/generations")
        .join(generation)
        .join("codestory.db");
    let legacy_path = cache.parent().expect("fixture root").join("schema31.db");
    let legacy = Connection::open(&legacy_path).expect("legacy database");
    legacy
        .execute_batch(include_str!(
            "../../codestory-store/tests/fixtures/v17_5_schema31.sql"
        ))
        .expect("authentic v0.17.5 schema-31 DDL");
    legacy
        .execute(
            "ATTACH DATABASE ?1 AS seed",
            [seed_database.to_string_lossy().as_ref()],
        )
        .expect("attach completed seed publication");
    let tables = legacy
        .prepare(
            "SELECT name FROM main.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        )
        .expect("table list")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("table rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("table names");
    for table in tables {
        let columns = legacy
            .prepare(&format!("PRAGMA main.table_info(\"{table}\")"))
            .expect("columns")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("column rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("column names")
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", ");
        legacy
            .execute(
                &format!("INSERT OR REPLACE INTO main.\"{table}\" ({columns}) SELECT {columns} FROM seed.\"{table}\""),
                [],
            )
            .unwrap_or_else(|error| panic!("copy {table}: {error}"));
    }
    legacy
        .execute_batch("DETACH DATABASE seed")
        .expect("detach seed");
    let alpha: i64 = legacy
        .query_row(
            "SELECT id FROM node WHERE serialized_name='alpha' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("indexed alpha node");
    legacy
        .execute(
            "INSERT INTO bookmark_category(id, name) VALUES(1, 'Legacy')",
            [],
        )
        .expect("legacy category");
    legacy
        .execute(
            "INSERT INTO bookmark_node(id, category_id, node_id, comment) VALUES(1, 1, ?1, 'legacy note')",
            [alpha],
        )
        .expect("legacy annotation");
    assert_eq!(schema_version(&legacy_path), 31);
    for table in ["node", "edge", "index_publication", "bookmark_node"] {
        assert!(
            row_count(&legacy, table) > 0,
            "legacy fixture needs {table}"
        );
    }
    drop(legacy);
    fs::remove_dir_all(cache.join("core")).expect("remove seed generations");
    let database = cache.join("codestory.db");
    if database.exists() {
        fs::remove_file(&database).expect("remove seed standalone database");
    }
    fs::rename(legacy_path, database).expect("install complete legacy core");
    assert!(!cache.join("core/publication.json").exists());
}

fn row_count(database: &Connection, table: &str) -> i64 {
    database
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|error| panic!("count {table}: {error}"))
}

fn schema_version(database: &Path) -> u32 {
    // A plain read-only open of a WAL database can still materialize -shm/-wal
    // in a writable directory; immutable keeps the probe strictly observational.
    let uri = format!("file:{}?mode=ro&immutable=1", database.display());
    Connection::open_with_flags(
        &uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .expect("read database")
    .query_row("PRAGMA user_version", [], |row| row.get(0))
    .expect("schema version")
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, String> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(path).expect("cache directory") {
            let path = entry.expect("cache entry").path();
            if path.is_dir() {
                files.insert(
                    path.strip_prefix(root)
                        .expect("relative cache directory")
                        .to_path_buf(),
                    "directory".to_string(),
                );
                visit(root, &path, files);
            } else {
                let bytes = fs::read(&path).expect("cache bytes");
                files.insert(
                    path.strip_prefix(root)
                        .expect("relative cache path")
                        .to_path_buf(),
                    format!("{}:{:x}", bytes.len(), Sha256::digest(bytes)),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn snapshot_tree_if_exists(root: &Path) -> Option<BTreeMap<PathBuf, String>> {
    root.exists().then(|| snapshot_tree(root))
}

fn run_cli(project: &Path, cache: &Path, args: &[&str]) -> String {
    let output = run_cli_output(project, cache, args);
    assert!(
        output.status.success(),
        "{args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

fn run_cli_output(project: &Path, cache: &Path, args: &[&str]) -> std::process::Output {
    test_support::cli_command()
        .args(args)
        .arg("--project")
        .arg(project)
        .arg("--cache-dir")
        .arg(cache)
        .env(
            "CODESTORY_CACHE_ROOT",
            cache.parent().expect("fixture root").join("global-cache"),
        )
        .env(
            "CODESTORY_STDIO_CACHE_ROOT",
            cache.parent().expect("fixture root").join("stdio-cache"),
        )
        .env(
            "CODESTORY_PLUGIN_DATA",
            cache.parent().expect("fixture root").join("plugin-data"),
        )
        .env("CODESTORY_TEST_EMBED_ALLOW_CPU", "1")
        .output()
        .expect("run CLI")
}

/// Issue #2531-4: the default markdown failure output must carry the same
/// `context.causes` chain and `next_action` guidance the JSON envelope reports.
/// The vehicle is a real `lock_wait_timeout`: a held local-refresh state guard
/// makes `ready --wait-fresh` exhaust its bounded wait.
#[test]
fn markdown_failure_matches_json_on_a_lock_wait_timeout() {
    let fixture = tempdir().expect("fixture");
    let project = fixture.path().join("project");
    let cache = fixture.path().join("cache");
    fs::create_dir(&project).expect("project");
    fs::write(project.join("lib.rs"), "pub fn alpha() {}\n").expect("source");
    run_cli(
        &project,
        &cache,
        &["index", "--refresh", "full", "--format", "json"],
    );
    fs::write(
        project.join("lib.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .expect("source edit");

    let guard_path =
        cache.join(codestory_contracts::owned_artifacts::LOCAL_REFRESH_STATE_GUARD_FILE);
    let holder = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&guard_path)
        .expect("open refresh state guard");
    codestory_contracts::bounded_locks::acquire_with_deadline(
        &holder,
        codestory_contracts::bounded_locks::FileLockKind::Exclusive,
        codestory_contracts::bounded_locks::LockDeadline::immediate(),
        None,
    )
    .expect("hold refresh state guard");

    let json_run = run_cli_output(
        &project,
        &cache,
        &["ready", "--wait-fresh", "--format", "json"],
    );
    assert!(
        !json_run.status.success(),
        "ready --wait-fresh must fail behind a held guard"
    );
    let envelope: Value = serde_json::from_slice(&json_run.stdout).expect("json failure envelope");
    let message = envelope["error"]["message"]
        .as_str()
        .expect("failure message");
    assert!(
        message.contains("local refresh state guard"),
        "the refusal must name the contended guard: {envelope}"
    );
    let causes = envelope["context"]["causes"]
        .as_array()
        .expect("json failure causes");
    assert!(
        causes.iter().any(|cause| {
            cause
                .as_str()
                .is_some_and(|cause| cause.contains("lock_wait_timeout"))
        }),
        "json failure must carry the real lock_wait_timeout cause: {envelope}"
    );

    let markdown_run = run_cli_output(&project, &cache, &["ready", "--wait-fresh"]);
    assert!(!markdown_run.status.success());
    let stderr = String::from_utf8_lossy(&markdown_run.stderr);
    assert!(
        stderr.contains(&format!("Error: {message}")),
        "markdown failure must lead with the same message: {stderr}"
    );
    // The two runs are separate processes: elapsed-time fields like the wait
    // budget legitimately differ by a millisecond. Mask digits before comparing
    // so parity is judged on the cause text, not the timing sample.
    let mask_digits = |text: &str| -> String {
        text.chars()
            .map(|ch| if ch.is_ascii_digit() { '#' } else { ch })
            .collect()
    };
    let stderr_masked = mask_digits(&stderr);
    for cause in causes {
        let cause = cause.as_str().expect("cause text");
        assert!(
            stderr_masked.contains(&mask_digits(cause)),
            "markdown failure dropped a JSON cause ({cause}): {stderr}"
        );
    }
    if let Some(details) = envelope["error"]["details"].as_object()
        && let Some(next) = details.get("minimum_next").and_then(Value::as_array)
    {
        for action in next {
            let action = action.as_str().expect("next action text");
            assert!(
                stderr.contains(action),
                "markdown failure dropped a JSON next_action ({action}): {stderr}"
            );
        }
    }
}

/// Issue #2531-4: `doctor --support-bundle` writes one local file holding the
/// doctor report plus the process's diagnostics records. The records were
/// redacted at capture time, so the bundle must not leak a redacted field in
/// clear — proven here by a sentinel that only ever appears inside a failed
/// command's error chain.
#[test]
fn doctor_support_bundle_writes_the_redacted_diagnostics_records() {
    let fixture = tempdir().expect("fixture");
    let project = fixture.path().join("project");
    let cache = fixture.path().join("cache");
    fs::create_dir(&project).expect("project");
    fs::write(project.join("lib.rs"), "pub fn alpha() {}\n").expect("source");
    run_cli(
        &project,
        &cache,
        &["index", "--refresh", "full", "--format", "json"],
    );

    // A real failure records a command_failure with the whole error redacted;
    // the sentinel only exists inside that error chain.
    let sentinel = format!("missing-project-{}", std::process::id());
    let missing = fixture.path().join(&sentinel);
    let failed = run_cli_output(&missing, &cache, &["ready"]);
    assert!(
        !failed.status.success(),
        "ready on a missing project must fail"
    );
    let failure_stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        failure_stderr.contains(&sentinel),
        "the live error names the missing project, proving the sentinel is a real disclosure risk: {failure_stderr}"
    );

    // Seed a panic-shaped record the way the panic hook writes one; the bundle
    // assembles whatever records the sink already holds. The process
    // diagnostics sink lives under the ambient cache root (CODESTORY_CACHE_ROOT
    // in this fixture), captured once at process start.
    let diagnostics_dir = fixture.path().join("global-cache").join("diagnostics");
    fs::create_dir_all(&diagnostics_dir).expect("diagnostics dir");
    use std::io::Write as _;
    let mut log = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(diagnostics_dir.join("codestory.jsonl"))
        .expect("open diagnostics log");
    writeln!(
        log,
        "{}",
        concat!(
            "{\"event\":\"panic\",\"level\":\"ERROR\",\"payload\":\"[redacted]\",",
            "\"payload_kind\":\"str\",\"payload_bytes\":43,",
            "\"location\":\"crates/codestory-indexer/src/lib.rs:3555:14\",",
            "\"schema_version\":1,\"timestamp_unix_ms\":1,\"pid\":1,",
            "\"correlation_id\":\"test\"}"
        )
    )
    .expect("seed panic record");

    let bundle = fixture.path().join("support-bundle.json");
    let bundle_arg = bundle.to_str().expect("bundle path").to_string();
    run_cli(
        &project,
        &cache,
        &[
            "doctor",
            "--format",
            "json",
            "--support-bundle",
            &bundle_arg,
        ],
    );

    let raw = fs::read_to_string(&bundle).expect("bundle file");
    let document: Value = serde_json::from_str(&raw).expect("bundle json");
    let canonical_project = project.canonicalize().expect("canonical project root");
    assert_eq!(
        document["report"]["project"].as_str(),
        Some(canonical_project.to_str().expect("project path")),
        "bundle must carry the doctor report: {document}"
    );
    let records = document["diagnostics"]
        .as_array()
        .expect("bundle diagnostics records");
    assert!(
        records
            .iter()
            .any(|record| record["event"] == "command_failure"),
        "bundle must carry the recorded command failure: {records:?}"
    );
    assert!(
        records.iter().any(|record| {
            record["event"] == "panic"
                && record["location"] == "crates/codestory-indexer/src/lib.rs:3555:14"
                && record["payload_bytes"] == 43
        }),
        "bundle must carry the panic site file:line:column and payload size: {records:?}"
    );
    assert!(
        raw.contains("[redacted]"),
        "redaction markers survive: {raw}"
    );
    assert!(
        !raw.contains(&sentinel),
        "a redacted error field must never reach the bundle in clear: {raw}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&bundle)
                .expect("bundle metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "support bundles are private by default"
        );
    }
}

mod test_support;
