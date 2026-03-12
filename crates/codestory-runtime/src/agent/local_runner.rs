use crate::{
    AgentBackend, AgentConnectionSettingsDto, ApiError, AppController, LocalAgentResponse,
    agent_backend_label, configured_agent_command, resolve_agent_command, truncate_for_diagnostic,
};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn is_windows_batch_command(command: &str) -> bool {
    if !cfg!(target_os = "windows") {
        return false;
    }
    Path::new(command)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| {
            let ext = value.to_ascii_lowercase();
            ext == "cmd" || ext == "bat"
        })
        .unwrap_or(false)
}

pub(crate) fn run_codex_agent(
    command: &str,
    cwd: &Path,
    prompt: &str,
) -> Result<LocalAgentResponse, ApiError> {
    let backend_label = agent_backend_label(AgentBackend::Codex);
    let temp_nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let output_path =
        std::env::temp_dir().join(format!("codestory-codex-response-{temp_nonce}.txt"));

    let mut command_builder = if is_windows_batch_command(command) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    } else {
        Command::new(command)
    };

    let mut child = command_builder
        .arg("exec")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--skip-git-repo-check")
        .arg("--cd")
        .arg(cwd)
        .arg("--output-last-message")
        .arg(&output_path)
        .arg("-")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            ApiError::internal(format!(
                "Failed to run {backend_label} command `{command}`: {e}"
            ))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(|e| {
            ApiError::internal(format!(
                "Failed to write prompt to {backend_label} command `{command}`: {e}"
            ))
        })?;
        if !prompt.ends_with('\n') {
            stdin.write_all(b"\n").map_err(|e| {
                ApiError::internal(format!(
                    "Failed to finalize prompt for {backend_label} command `{command}`: {e}"
                ))
            })?;
        }
    }

    let output = child.wait_with_output().map_err(|e| {
        ApiError::internal(format!(
            "Failed to wait for {backend_label} command `{command}`: {e}"
        ))
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let response_text = fs::read_to_string(&output_path)
        .ok()
        .or_else(|| {
            let fallback = stdout.trim().to_string();
            if fallback.is_empty() {
                None
            } else {
                Some(fallback)
            }
        })
        .unwrap_or_default();
    let _ = fs::remove_file(&output_path);

    build_agent_response(
        backend_label,
        command,
        output.status.success(),
        &stdout,
        &stderr,
        &response_text,
    )
}

pub(crate) fn run_claude_agent(
    command: &str,
    cwd: &Path,
    prompt: &str,
) -> Result<LocalAgentResponse, ApiError> {
    let backend_label = agent_backend_label(AgentBackend::ClaudeCode);
    let output = Command::new(command)
        .arg("-p")
        .arg("--output-format")
        .arg("text")
        .arg("--permission-mode")
        .arg("dontAsk")
        .arg(prompt)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            ApiError::internal(format!(
                "Failed to run {backend_label} command `{command}`: {e}"
            ))
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    build_agent_response(
        backend_label,
        command,
        output.status.success(),
        &stdout,
        &stderr,
        &stdout,
    )
}

pub(crate) fn run_local_agent(
    controller: &AppController,
    connection: &AgentConnectionSettingsDto,
    prompt: &str,
) -> Result<LocalAgentResponse, ApiError> {
    let command = configured_agent_command(connection);
    let command = resolve_agent_command(&command);
    let cwd = controller.require_project_root()?;
    match connection.backend {
        AgentBackend::Codex => run_codex_agent(&command, &cwd, prompt),
        AgentBackend::ClaudeCode => run_claude_agent(&command, &cwd, prompt),
    }
}

fn build_agent_response(
    backend_label: &'static str,
    command: &str,
    success: bool,
    stdout: &str,
    stderr: &str,
    response_text: &str,
) -> Result<LocalAgentResponse, ApiError> {
    if !success {
        let diagnostic = if stderr.trim().is_empty() {
            truncate_for_diagnostic(stdout, 280)
        } else {
            truncate_for_diagnostic(stderr, 280)
        };
        return Err(ApiError::invalid_argument(format!(
            "{backend_label} command failed: {}",
            if diagnostic.is_empty() {
                "no diagnostics available".to_string()
            } else {
                diagnostic
            }
        )));
    }

    let markdown = response_text.trim().to_string();
    if markdown.is_empty() {
        return Err(ApiError::invalid_argument(format!(
            "{backend_label} returned an empty response."
        )));
    }

    Ok(LocalAgentResponse {
        backend_label,
        command: command.to_string(),
        markdown,
    })
}
