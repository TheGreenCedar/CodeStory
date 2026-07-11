#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn cli_command() -> Command {
    command(cli_binary_path())
}

pub fn cli_binary_path() -> PathBuf {
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
