use crate::args::{SetupCommand, SetupHost};
use anyhow::{Context, Result, bail, ensure};
use codestory_workspace::atomic_file::write_file_atomic;
use codestory_workspace::discover_git_worktree_root;
use std::{
    env, fs,
    io::{ErrorKind, Write},
    ops::Range,
    path::{Path, PathBuf},
};

struct Markers {
    start: &'static [u8],
    existing: &'static [u8],
    created: &'static [u8],
    end: &'static [u8],
}

const DEFAULT_MARKERS: Markers = Markers {
    start: b"<!-- codestory-lookups:start",
    existing: b"<!-- codestory-lookups:start -->\n",
    created: b"<!-- codestory-lookups:start new-file -->\n",
    end: b"<!-- codestory-lookups:end -->\n",
};
const OPENCODE_MARKERS: Markers = Markers {
    start: b"<!-- codestory-opencode-lookups:start",
    existing: b"<!-- codestory-opencode-lookups:start -->\n",
    created: b"<!-- codestory-opencode-lookups:start new-file -->\n",
    end: b"<!-- codestory-opencode-lookups:end -->\n",
};
const CURSOR_HEADER: &[u8] = b"---\nalwaysApply: true\n---\n";
const NOTE: &str = include_str!("../../../../plugins/codestory/standalone/lookup-note.md");
#[cfg(unix)]
const LAUNCHER: &str = include_str!("../../../../plugins/codestory/standalone/cs.sh");
#[cfg(windows)]
const LAUNCHER: &str = include_str!("../../../../plugins/codestory/standalone/cs.cmd");

pub(super) fn run_setup(cmd: SetupCommand) -> Result<()> {
    let (directory, home_var, home_file, repo_file) = match cmd.host {
        SetupHost::AgentsFile => (".codex", Some("CODEX_HOME"), Some("AGENTS.md"), "AGENTS.md"),
        SetupHost::ClaudeFile => (
            ".claude",
            Some("CLAUDE_CONFIG_DIR"),
            Some("CLAUDE.md"),
            "CLAUDE.md",
        ),
        SetupHost::Cursor => (".cursor", None, None, ".cursor/rules/codestory-lookups.mdc"),
        SetupHost::OpenCode => (
            ".config/opencode",
            Some("OPENCODE_CONFIG_DIR"),
            Some("AGENTS.md"),
            "AGENTS.md",
        ),
        SetupHost::Antigravity => (".gemini", None, Some("GEMINI.md"), "GEMINI.md"),
    };
    let configured = home_var
        .and_then(|name| configured_home(name, ""))
        .or_else(|| match cmd.host {
            SetupHost::OpenCode => configured_home("XDG_CONFIG_HOME", "/opencode"),
            _ => None,
        });
    let default_home = env::home_dir().map(|home| home.join(directory));
    let host_home = configured
        .as_ref()
        .map(|(path, _, _)| path.clone())
        .or_else(|| default_home.clone())
        .context("could not resolve the agent's configuration directory")?;
    let host_home = std::path::absolute(host_home)?;
    let cursor = matches!(cmd.host, SetupHost::Cursor);
    let instructions = match home_file.filter(|_| !cmd.repo) {
        Some(file) => host_home.join(file),
        None => {
            let guidance = if cursor {
                "Run `codestory-cli setup cursor` from a Git repository to manage its Cursor rule"
            } else {
                "--repo requires a Git working tree"
            };
            discover_git_worktree_root(&env::current_dir()?)
                .context(guidance)?
                .join(repo_file)
        }
    };
    let markers = if matches!(cmd.host, SetupHost::OpenCode) {
        &OPENCODE_MARKERS
    } else {
        &DEFAULT_MARKERS
    };
    let header = if cursor { CURSOR_HEADER } else { &[] };
    let original = read_file(&instructions)?;
    let bytes = original
        .as_ref()
        .map_or(&[][..], |file| file.bytes.as_slice());
    let block = managed_range(bytes, markers)
        .with_context(|| format!("read {}", instructions.display()))?;

    if cmd.remove {
        if let Some((range, created_file)) = block {
            let updated = [&bytes[..range.start], &bytes[range.end..]].concat();
            if created_file && updated == header {
                ensure_unchanged(&instructions, original.as_ref())?;
                fs::remove_file(&instructions)?;
                println!("Removed {}", instructions.display());
            } else {
                replace(&instructions, original.as_ref(), &updated, false)?;
            }
        } else {
            println!("No lookup instructions in {}", instructions.display());
        }
        return Ok(());
    }

    if !header.is_empty() && original.is_some() {
        ensure!(
            block.is_some() && bytes.starts_with(header),
            "{} already exists without the CodeStory Cursor rule format",
            instructions.display()
        );
    }
    let configured_invocation = configured
        .as_ref()
        .filter(|_| default_home.as_deref() != Some(host_home.as_path()))
        .map(|(_, name, suffix)| (*name, *suffix));
    let (invocation, filename) = launcher_command(directory, configured_invocation);
    let launcher = host_home.join(filename);
    let note = NOTE.replace("@COMMAND@", &invocation);
    ensure!(note.len() < 800, "lookup instructions exceed 799 bytes");
    let cli = env::current_exe().context("resolve the CodeStory executable")?;
    let script = LAUNCHER.replace("@CODESTORY_CLI@", &quoted_executable(&cli)?);
    let launcher_original = read_file(&launcher)?;
    if let Some(existing) = &launcher_original {
        ensure!(
            existing
                .bytes
                .split(|byte| *byte == b'\n')
                .nth(1)
                .is_some_and(|line| {
                    line.trim_ascii_end() == b"# Managed by codestory-cli setup."
                        || line.trim_ascii_end() == b"rem Managed by codestory-cli setup."
                }),
            "{} already exists and is not a CodeStory launcher",
            launcher.display()
        );
    }
    replace(
        &launcher,
        launcher_original.as_ref(),
        script.as_bytes(),
        true,
    )?;

    let (range, created_file) = block.unwrap_or((bytes.len()..bytes.len(), original.is_none()));
    let marker = if created_file {
        markers.created
    } else {
        markers.existing
    };
    let updated = [
        if original.is_none() { header } else { &[] },
        &bytes[..range.start],
        b"\n\n",
        marker,
        note.as_bytes(),
        markers.end,
        &bytes[range.end..],
    ]
    .concat();
    replace(&instructions, original.as_ref(), &updated, false)
}

fn configured_home(
    name: &'static str,
    suffix: &'static str,
) -> Option<(PathBuf, &'static str, &'static str)> {
    let value = env::var_os(name).filter(|value| !value.is_empty())?;
    Some((
        PathBuf::from(value).join(suffix.trim_start_matches('/')),
        name,
        suffix,
    ))
}

#[cfg(unix)]
fn launcher_command(directory: &str, configured: Option<(&str, &str)>) -> (String, &'static str) {
    let command = match configured {
        None => format!("~/{directory}/cs"),
        Some((name, suffix)) => format!("\"${name}{suffix}/cs\""),
    };
    (command, "cs")
}

#[cfg(windows)]
fn launcher_command(directory: &str, configured: Option<(&str, &str)>) -> (String, &'static str) {
    let command = match configured {
        None => format!("& ~/{directory}/cs.cmd"),
        Some((name, suffix)) => format!("& \"$env:{name}{suffix}/cs.cmd\""),
    };
    (command, "cs.cmd")
}

fn quoted_executable(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .context("CodeStory executable path is not UTF-8")?;
    ensure!(
        !path.contains(['\r', '\n']),
        "CodeStory executable path contains a newline"
    );
    #[cfg(unix)]
    return Ok(format!("'{}'", path.replace('\'', "'\\''")));
    #[cfg(windows)]
    return Ok(format!("\"{}\"", path.replace('%', "%%")));
}

fn unique_marker(bytes: &[u8], marker: &[u8]) -> Result<Option<usize>> {
    let mut matches = bytes
        .windows(marker.len())
        .enumerate()
        .filter(|(_, slice)| *slice == marker);
    let first = matches.next().map(|(i, _)| i);
    ensure!(
        matches.next().is_none(),
        "duplicate CodeStory lookup marker"
    );
    Ok(first)
}

fn managed_range(bytes: &[u8], markers: &Markers) -> Result<Option<(Range<usize>, bool)>> {
    match (
        unique_marker(bytes, markers.start)?,
        unique_marker(bytes, markers.end)?,
    ) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) if start >= 2 && &bytes[start - 2..start] == b"\n\n" => {
            let created = bytes[start..].starts_with(markers.created);
            let header = if created {
                markers.created
            } else {
                markers.existing
            };
            ensure!(
                bytes[start..].starts_with(header)
                    && end >= start + header.len()
                    && bytes[end - 1] == b'\n',
                "malformed CodeStory lookup block"
            );
            Ok(Some((start - 2..end + markers.end.len(), created)))
        }
        _ => bail!("incomplete or malformed CodeStory lookup block"),
    }
}

struct FileContents {
    bytes: Vec<u8>,
    permissions: fs::Permissions,
}

fn read_file(path: &Path) -> Result<Option<FileContents>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.file_type().is_file(),
                "{} is not a regular file",
                path.display()
            );
            Ok(Some(FileContents {
                bytes: fs::read(path)?,
                permissions: meta.permissions(),
            }))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn ensure_unchanged(path: &Path, previous: Option<&FileContents>) -> Result<()> {
    let current = read_file(path)?;
    ensure!(
        current
            .as_ref()
            .map(|file| (&file.bytes, &file.permissions))
            == previous.map(|file| (&file.bytes, &file.permissions)),
        "{} changed during CodeStory setup; retry with the current file",
        path.display()
    );
    Ok(())
}

fn replace(
    path: &Path,
    previous: Option<&FileContents>,
    bytes: &[u8],
    executable: bool,
) -> Result<()> {
    if previous.is_some_and(|file| {
        file.bytes == bytes && (!executable || is_executable(&file.permissions))
    }) {
        println!("Unchanged {}", path.display());
        return Ok(());
    }
    write_file_atomic(
        path,
        "codestory-setup",
        |file| {
            file.write_all(bytes)?;
            let mut permissions = previous.map_or_else(
                || file.metadata().map(|m| m.permissions()),
                |old| Ok(old.permissions.clone()),
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                permissions.set_mode(if executable {
                    0o700
                } else {
                    previous.map_or(0o600, |old| old.permissions.mode())
                });
            }
            #[cfg(windows)]
            let _ = &mut permissions;
            file.set_permissions(permissions)?;
            Ok(())
        },
        |temporary| {
            ensure!(fs::read(temporary)? == bytes, "setup file did not verify");
            ensure_unchanged(path, previous)
        },
    )?;
    println!(
        "{} {}",
        if previous.is_some() {
            "Updated"
        } else {
            "Created"
        },
        path.display()
    );
    Ok(())
}

#[cfg(unix)]
fn is_executable(permissions: &fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;
    permissions.mode() & 0o100 != 0
}

#[cfg(windows)]
fn is_executable(_: &fs::Permissions) -> bool {
    true
}
