use super::*;
use std::ffi::OsString;
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

fn os_default_open_invocation(path: &Path) -> (&'static str, Vec<OsString>) {
    if cfg!(target_os = "windows") {
        return ("explorer", vec![path.as_os_str().to_os_string()]);
    }
    if cfg!(target_os = "macos") {
        return ("open", vec![path.as_os_str().to_os_string()]);
    }
    ("xdg-open", vec![path.as_os_str().to_os_string()])
}

pub(super) fn open_with_os_default(path: &Path) -> io::Result<()> {
    let (program, args) = os_default_open_invocation(path);
    Command::new(program).args(args).spawn()?;
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
            // CODESTORY_IDE_COMMAND is an explicit user shell template.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_default_open_passes_metacharacter_path_as_argument() {
        let path = Path::new("repo & $(touch nope); file.rs");
        let (program, args) = os_default_open_invocation(path);

        assert_ne!(program, "cmd");
        assert_ne!(program, "sh");
        assert_eq!(args, vec![path.as_os_str().to_os_string()]);
    }

    #[test]
    fn ide_template_substitution_is_for_explicit_shell_templates() {
        let path = Path::new("repo & $(touch nope); file.rs");

        let expanded = expand_ide_template("code --goto {file}:{line}:{col}", path, Some(7), None);

        assert_eq!(expanded, "code --goto repo & $(touch nope); file.rs:7:1");
    }
}
