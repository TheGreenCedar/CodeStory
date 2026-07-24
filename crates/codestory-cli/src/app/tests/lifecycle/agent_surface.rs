use super::transport::EnvVarSnapshot;
use crate::app::open_agent_surface;
use crate::args;
use crate::args::ProjectArgs;
use crate::runtime;
use crate::runtime::RuntimeContext;
use codestory_contracts::api::IndexMode;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn agent_surface_refresh_fixture() -> (tempfile::TempDir, ProjectArgs, PathBuf, u32) {
    let temp = tempdir().expect("create temp dir");
    let project = temp.path().join("project");
    let cache = temp.path().join("cache");
    fs::create_dir_all(project.join("src")).expect("create source directory");
    fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"agent-surface-refresh-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
    fs::write(
        project.join("src/lib.rs"),
        "pub fn agent_surface_refresh_fixture() -> u32 { 1 }\n",
    )
    .expect("write source");
    let project_args = ProjectArgs {
        project,
        cache_dir: Some(cache),
    };
    let runtime = RuntimeContext::new_inspect_only(&project_args).expect("create runtime");
    runtime
        .ensure_open(args::RefreshMode::Full)
        .expect("publish current core generation");
    let storage_path = runtime.storage_path.clone();
    let schema_version = sqlite_schema_version(&storage_path);
    (temp, project_args, storage_path, schema_version)
}

fn sqlite_schema_version(path: &Path) -> u32 {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open database read-only");
    connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
        .expect("read schema version")
}

fn stamp_sqlite_schema_version(path: &Path, version: u32) {
    let connection = rusqlite::Connection::open(path).expect("open database");
    connection
        .pragma_update(None, "user_version", version)
        .expect("stamp schema version");
}

fn durable_database_and_wal(path: &Path) -> (Vec<u8>, Option<Vec<u8>>) {
    (
        fs::read(path).expect("read database"),
        fs::read(path.with_extension("db-wal")).ok(),
    )
}

#[test]
fn agent_surface_preflights_precurrent_schema_before_summary_open() {
    let _env_lock = crate::config::config_env_test_lock();
    let _env_snapshot = EnvVarSnapshot::clear(&[
        "CODESTORY_RETRIEVAL_PROFILE",
        "CODESTORY_RETRIEVAL_RUN_ID",
        "CI",
        "GITHUB_ACTIONS",
    ]);
    let (_temp, project_args, storage_path, current_schema) = agent_surface_refresh_fixture();
    assert!(current_schema > 1, "fixture needs a pre-current schema");
    let old_schema = current_schema - 1;
    stamp_sqlite_schema_version(&storage_path, old_schema);
    let durable_before = durable_database_and_wal(&storage_path);

    let error = match open_agent_surface(
        &project_args,
        None,
        None,
        args::RefreshMode::Incremental,
        "packet",
    ) {
        Ok(_) => panic!("explicit incremental must reject the old schema"),
        Err(error) => error,
    };
    let api = runtime::api_error_in_chain(&error).expect("typed compatibility error");
    assert_eq!(api.code, "full_refresh_required");
    assert_eq!(
        api.details
            .as_deref()
            .and_then(|details| details.cause_code.as_deref()),
        Some("core_schema_upgrade_required")
    );
    assert_eq!(durable_database_and_wal(&storage_path), durable_before);
    assert_eq!(sqlite_schema_version(&storage_path), old_schema);

    let opened = open_agent_surface(&project_args, None, None, args::RefreshMode::Auto, "packet")
        .expect("auto may select full recovery");
    assert!(
        opened.before.is_none(),
        "compatibility recovery has no safe pre-refresh summary"
    );
    assert_eq!(opened.opened.refresh_mode, Some(IndexMode::Full));
    assert_eq!(
        opened.opened.refresh_reason.as_deref(),
        Some("core_schema_upgrade_required")
    );
    assert_eq!(sqlite_schema_version(&storage_path), current_schema);
}

#[test]
fn agent_surface_preflight_preserves_pending_promotion_without_recovery() {
    let _env_lock = crate::config::config_env_test_lock();
    let _env_snapshot = EnvVarSnapshot::clear(&[
        "CODESTORY_RETRIEVAL_PROFILE",
        "CODESTORY_RETRIEVAL_RUN_ID",
        "CI",
        "GITHUB_ACTIONS",
    ]);
    let (_temp, project_args, storage_path, _current_schema) = agent_surface_refresh_fixture();
    let prepared_path = PathBuf::from(format!(
        "{}.promotion.prepared.json",
        storage_path.display()
    ));
    let prepared = b"pending promotion evidence";
    fs::write(&prepared_path, prepared).expect("write pending promotion marker");
    let durable_before = durable_database_and_wal(&storage_path);

    for refresh in [args::RefreshMode::Auto, args::RefreshMode::Incremental] {
        let error = match open_agent_surface(&project_args, None, None, refresh, "packet") {
            Ok(_) => panic!("pending promotion must fail closed for {refresh:?}"),
            Err(error) => error,
        };
        let api = runtime::api_error_in_chain(&error).expect("typed fail-closed error");
        assert_eq!(api.code, "internal");
        assert!(
            api.message.contains("promotion recovery is pending"),
            "{api:?}"
        );
    }

    assert_eq!(durable_database_and_wal(&storage_path), durable_before);
    assert_eq!(
        fs::read(&prepared_path).expect("pending promotion marker remains"),
        prepared
    );
}

pub(super) fn assert_order(markdown: &str, first: &str, second: &str) {
    let first_index = markdown
        .find(first)
        .unwrap_or_else(|| panic!("missing `{first}` in:\n{markdown}"));
    let second_index = markdown
        .find(second)
        .unwrap_or_else(|| panic!("missing `{second}` in:\n{markdown}"));
    assert!(
        first_index < second_index,
        "expected `{first}` before `{second}` in:\n{markdown}"
    );
}
