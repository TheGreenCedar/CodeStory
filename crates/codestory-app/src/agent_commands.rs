use codestory_api::{AgentBackend, AgentConnectionSettingsDto};
use std::path::PathBuf;

pub(crate) fn agent_backend_label(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Codex => "Codex",
        AgentBackend::ClaudeCode => "Claude Code",
    }
}

fn default_agent_command(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Codex => {
            if cfg!(target_os = "windows") {
                "codex.cmd"
            } else {
                "codex"
            }
        }
        AgentBackend::ClaudeCode => "claude",
    }
}

pub(crate) fn configured_agent_command(connection: &AgentConnectionSettingsDto) -> String {
    connection
        .command
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_agent_command(connection.backend).to_string())
}

pub(crate) fn resolve_agent_command(command: &str) -> String {
    if !cfg!(target_os = "windows") {
        return command.to_string();
    }

    if command.contains('\\') || command.contains('/') {
        return command.to_string();
    }

    let mut candidates = Vec::new();
    if let Ok(app_data) = std::env::var("APPDATA") {
        let npm_bin = PathBuf::from(app_data).join("npm");
        candidates.push(npm_bin.join(format!("{command}.cmd")));
        candidates.push(npm_bin.join(format!("{command}.exe")));
        candidates.push(npm_bin.join(command));
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let local_bin = PathBuf::from(user_profile).join(".local").join("bin");
        candidates.push(local_bin.join(format!("{command}.exe")));
        candidates.push(local_bin.join(format!("{command}.cmd")));
        candidates.push(local_bin.join(command));
    }
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let windows_apps = PathBuf::from(local_app_data)
            .join("Microsoft")
            .join("WindowsApps");
        candidates.push(windows_apps.join(format!("{command}.exe")));
        candidates.push(windows_apps.join(format!("{command}.cmd")));
        candidates.push(windows_apps.join(command));
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
        .unwrap_or_else(|| command.to_string())
}
