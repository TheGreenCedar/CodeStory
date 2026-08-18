use crate::args::{InternalDirtyHookAction, InternalDirtyHookCommand};
use anyhow::{Context, Result};
use codestory_workspace::{RepositoryHookAction, RepositoryHookRequest, manage_repository_hooks};
use std::io::Write as _;

pub(crate) fn run_internal_dirty_hook(command: InternalDirtyHookCommand) -> Result<()> {
    let action = match command.action {
        InternalDirtyHookAction::Install => RepositoryHookAction::Install,
        InternalDirtyHookAction::Uninstall => RepositoryHookAction::Uninstall,
        InternalDirtyHookAction::Status => RepositoryHookAction::Status,
    };
    let report = manage_repository_hooks(&RepositoryHookRequest {
        action,
        project_root: command.project,
        plugin_data_dir: command.plugin_data,
        node_path: command.node,
        script_path: command.script,
    });
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, &report).context("serialize dirty-hook result")?;
    output.write_all(b"\n").context("write dirty-hook result")
}
