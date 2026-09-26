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
    Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
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
    let output = test_support::cli_command()
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
        .expect("run CLI");
    assert!(
        output.status.success(),
        "{args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

mod test_support;
