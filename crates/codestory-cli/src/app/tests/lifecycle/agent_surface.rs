use super::transport::EnvVarSnapshot;
use crate::app::{open_agent_surface, open_search_surface};
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
    let generation_path = codestory_runtime::resolve_core_database_path(&storage_path)
        .expect("resolve active immutable generation");
    let schema_version = sqlite_schema_version(&generation_path);
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

fn stamp_active_generation_schema_version(storage_path: &Path, version: u32) {
    let generation_path = codestory_runtime::resolve_core_database_path(storage_path)
        .expect("resolve active immutable generation");
    // Drop any leftover sidecars first so the write open does not inherit a
    // read-only WAL/SHM pair from an observational reader.
    for suffix in ["-wal", "-journal", "-shm"] {
        let mut sidecar = generation_path.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = fs::remove_file(PathBuf::from(sidecar));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&generation_path, fs::Permissions::from_mode(0o644))
            .expect("make generation owner-writable");
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::metadata(&generation_path).expect("generation metadata");
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&generation_path, permissions).expect("make generation writable");
    }
    {
        let connection = rusqlite::Connection::open(&generation_path).expect("open generation");
        connection
            .pragma_update(None, "user_version", version)
            .expect("stamp schema version");
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .expect("checkpoint schema stamp");
    }
    for suffix in ["-wal", "-journal", "-shm"] {
        let mut sidecar = generation_path.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = fs::remove_file(PathBuf::from(sidecar));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&generation_path, fs::Permissions::from_mode(0o444))
            .expect("reseal generation as immutable");
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::metadata(&generation_path).expect("generation metadata after stamp");
        let mut permissions = metadata.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&generation_path, permissions).expect("reseal generation");
    }
}

fn durable_database_and_wal(storage_path: &Path) -> (Vec<u8>, Option<Vec<u8>>) {
    let generation_path = codestory_runtime::resolve_core_database_path(storage_path)
        .expect("resolve active immutable generation");
    (
        fs::read(&generation_path).expect("read database"),
        fs::read(PathBuf::from(format!("{}-wal", generation_path.display()))).ok(),
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
    stamp_active_generation_schema_version(&storage_path, old_schema);
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
    let generation_path = codestory_runtime::resolve_core_database_path(&storage_path)
        .expect("resolve active immutable generation");
    assert_eq!(sqlite_schema_version(&generation_path), old_schema);

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
    let recovered_path = codestory_runtime::resolve_core_database_path(&storage_path)
        .expect("resolve recovered generation");
    assert_eq!(sqlite_schema_version(&recovered_path), current_schema);
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

#[test]
fn agent_surface_embedding_preflight_preserves_cli_error_text() {
    let _env_lock = crate::config::config_env_test_lock();
    let _env_snapshot = EnvVarSnapshot::clear(&[
        "CODESTORY_RETRIEVAL_PROFILE",
        "CODESTORY_RETRIEVAL_RUN_ID",
        "CI",
        "GITHUB_ACTIONS",
    ]);
    let (_temp, project_args, _storage_path, _current_schema) = agent_surface_refresh_fixture();
    let runtime = RuntimeContext::new_inspect_only(&project_args).expect("create runtime");
    let embedding_cache_root = runtime.sidecar.as_raw_config_for_test().cache_root.clone();
    fs::create_dir_all(&embedding_cache_root).expect("create embedding cache root");
    let marker = embedding_cache_root.join(codestory_retrieval::TEST_EMBEDDING_UNAVAILABLE_MARKER);
    fs::write(&marker, b"unavailable").expect("write embedding unavailable marker");

    let error =
        match open_agent_surface(&project_args, None, None, args::RefreshMode::None, "packet") {
            Ok(_) => panic!("unavailable embedding backend must block packet activation"),
            Err(error) => error,
        };
    assert_eq!(
        format!("{error:#}"),
        format!(
            "initialize retrieval for packet: embedding backend unavailable by test marker in {}",
            embedding_cache_root.display()
        )
    );
    fs::remove_file(marker).expect("remove embedding unavailable marker");
}

#[test]
fn exact_search_opens_the_complete_core_without_embedding_preflight() {
    let _env_lock = crate::config::config_env_test_lock();
    let _env_snapshot = EnvVarSnapshot::clear(&[
        "CODESTORY_RETRIEVAL_PROFILE",
        "CODESTORY_RETRIEVAL_RUN_ID",
        "CI",
        "GITHUB_ACTIONS",
    ]);
    let (_temp, project_args, _storage_path, _current_schema) = agent_surface_refresh_fixture();
    let runtime = RuntimeContext::new_inspect_only(&project_args).expect("create runtime");
    let embedding_cache_root = runtime.sidecar.as_raw_config_for_test().cache_root.clone();
    fs::create_dir_all(&embedding_cache_root).expect("create embedding cache root");
    let marker = embedding_cache_root.join(codestory_retrieval::TEST_EMBEDDING_UNAVAILABLE_MARKER);
    fs::write(&marker, b"unavailable").expect("write embedding unavailable marker");

    open_search_surface(
        &project_args,
        None,
        None,
        args::RefreshMode::None,
        codestory_contracts::api::SearchRepoTextMode::Off,
    )
    .expect("exact search must not initialize embeddings");

    let error = match open_search_surface(
        &project_args,
        None,
        None,
        args::RefreshMode::None,
        codestory_contracts::api::SearchRepoTextMode::Auto,
    ) {
        Ok(_) => panic!("ordinary search still requires embeddings"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("embedding backend unavailable by test marker"),
        "{error:#}"
    );
    fs::remove_file(marker).expect("remove embedding unavailable marker");
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
