#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn cli_command() -> Command {
    command(cli_binary_path())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub fn cli_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_codestory-cli-runtime"))
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn cli_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_codestory-cli"))
}

pub fn launcher_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_codestory-cli"))
}

pub fn command(binary: impl AsRef<OsStr>) -> Command {
    let state = test_state_root();
    std::fs::create_dir_all(&state).expect("create isolated CodeStory test state root");
    let mut command = Command::new(binary);
    command
        .env("CODESTORY_CACHE_ROOT", state.join("cache"))
        .env("CODESTORY_STDIO_CACHE_ROOT", state.join("stdio-cache"))
        .env("CODESTORY_INSTALL_ID", install_id())
        .env("CODESTORY_PLUGIN_DATA", state.join("plugin-data"));
    command
}

pub fn test_state_root() -> PathBuf {
    test_root().join("codestory-state")
}

pub fn os_user_root() -> PathBuf {
    test_root().join("os-user")
}

fn test_root() -> PathBuf {
    process_root().join(thread_name())
}

fn process_root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let base = option_env!("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        base.join("codestory-integration-tests").join(format!(
            "{}-{}-{nonce}",
            env!("CARGO_PKG_NAME"),
            std::process::id()
        ))
    })
}

fn thread_name() -> String {
    format!("{:?}", std::thread::current().id())
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn install_id() -> String {
    format!("integration-{}-{}", std::process::id(), thread_name())
}

/// Downgrade the published core's durable `user_version` in place, emulating a
/// core written by an older release. Returns the generation database path so
/// callers can prove the image is untouched afterward. The same fixture shape
/// backs the I2 #3c probe-stage case, so keep it a schema downgrade of a real
/// published generation rather than a hand-built database.
pub fn set_active_core_schema_version(cache_dir: &std::path::Path, version: u32) -> PathBuf {
    let pointer: serde_json::Value = serde_json::from_slice(
        &std::fs::read(cache_dir.join("core/publication.json")).expect("committed core pointer"),
    )
    .expect("core pointer json");
    let generation = pointer["active"]["generation_id"]
        .as_str()
        .expect("active generation id")
        .to_string();
    let database = cache_dir
        .join("core")
        .join("generations")
        .join(generation)
        .join("codestory.db");
    let metadata = std::fs::metadata(&database).expect("generation metadata");
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    {
        permissions.set_readonly(false);
    }
    std::fs::set_permissions(&database, permissions).expect("unseal generation image");
    {
        let connection = rusqlite::Connection::open(&database).expect("open generation image");
        connection
            .pragma_update(None, "user_version", version)
            .expect("downgrade durable schema version");
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .expect("checkpoint schema downgrade");
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = database.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }
    database
}
