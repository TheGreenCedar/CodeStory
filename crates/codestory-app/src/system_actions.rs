use super::*;
use std::process::Command;

pub(super) fn expand_ide_template(
    template: &str,
    file_path: &Path,
    line: Option<u32>,
    col: Option<u32>,
) -> String {
    let file = file_path.to_string_lossy();
    let line = line.unwrap_or(1).to_string();
    let col = col.unwrap_or(1).to_string();
    template
        .replace("{file}", &file)
        .replace("{line}", &line)
        .replace("{col}", &col)
}

pub(super) fn run_shell_command(command: &str) -> io::Result<()> {
    if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", command]).spawn()?;
        return Ok(());
    }
    Command::new("sh").args(["-lc", command]).spawn()?;
    Ok(())
}

pub(super) fn open_with_os_default(path: &Path) -> io::Result<()> {
    if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()?;
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()?;
        return Ok(());
    }
    Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

pub(super) fn open_folder_in_os(path: &Path) -> io::Result<()> {
    if cfg!(target_os = "windows") {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()?;
        return Ok(());
    }
    let folder = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    };
    if cfg!(target_os = "macos") {
        Command::new("open").arg(folder).spawn()?;
        return Ok(());
    }
    Command::new("xdg-open").arg(folder).spawn()?;
    Ok(())
}

pub(super) fn launch_definition_in_ide(
    path: &Path,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<SystemActionResponse, ApiError> {
    if let Ok(template) = std::env::var("CODESTORY_IDE_COMMAND") {
        let trimmed = template.trim();
        if !trimmed.is_empty() {
            let expanded = expand_ide_template(trimmed, path, line, col);
            if run_shell_command(&expanded).is_ok() {
                return Ok(status_response(format!(
                    "Opened definition in IDE via CODESTORY_IDE_COMMAND: {}",
                    path.display()
                )));
            }
        }
    }

    let goto = format!(
        "{}:{}:{}",
        path.display(),
        line.unwrap_or(1),
        col.unwrap_or(1)
    );
    if Command::new("code")
        .args(["--goto", goto.as_str()])
        .spawn()
        .is_ok()
    {
        return Ok(status_response(format!(
            "Opened definition in IDE via `code --goto`: {}",
            path.display()
        )));
    }

    open_with_os_default(path).map_err(|e| {
        ApiError::internal(format!(
            "Failed to open definition using OS default handler for {}: {e}",
            path.display()
        ))
    })?;
    Ok(status_response(format!(
        "Opened definition with OS default handler: {}",
        path.display()
    )))
}
