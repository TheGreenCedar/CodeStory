//! `find`, `refs`, `read`, `edit`, and `slice`: lookups answered from the live source tree.
//!
//! These handlers resolve the project and its storage path without opening
//! the project: no refresh, no activation, no whole-tree freshness check. The
//! runtime reads live text and consults the pinned index only for the files
//! it serves.

use crate::args::{EditCommand, FindCommand, ReadCommand, RefsCommand, SliceCommand};
use crate::runtime::{RuntimeContext, map_api_error};
use anyhow::Result;
use codestory_contracts::api::ApiError;
use codestory_runtime::served_lookup;
use std::io::{Read as _, Write as _};

pub(super) fn run_find(cmd: FindCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    emit(served_lookup::find_with_budget(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.terms,
        &cmd.file_globs,
        cmd.limit,
        cmd.bodies.budget(),
        cmd.bodies.max_bytes,
    ))
}

pub(super) fn run_refs(cmd: RefsCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    emit(served_lookup::refs_with_budget_and_globs(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.name,
        &cmd.file_globs,
        cmd.limit,
        cmd.bodies.budget(),
        cmd.bodies.max_bytes,
    ))
}

pub(super) fn run_read(cmd: ReadCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let max_bytes = (cmd.max_bytes > 0).then_some(cmd.max_bytes);
    emit(served_lookup::read_with_budget(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.selectors,
        max_bytes,
    ))
}

pub(super) fn run_edit(cmd: EditCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let replacement = match cmd.replacement {
        Some(text) if !cmd.stdin => text,
        _ => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    emit(served_lookup::edit(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.selector,
        &replacement,
    ))
}

pub(super) fn run_slice(cmd: SliceCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    emit(served_lookup::slice(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.question,
        cmd.max_bytes,
    ))
}

/// Print a reply. A reader that stops early (`| head`) is not a failure, and
/// must not turn a read-only lookup into a recorded panic.
fn emit(reply: Result<String, ApiError>) -> Result<()> {
    let reply = reply.map_err(map_api_error)?;
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(reply.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => Err(error.into()),
        _ => Ok(()),
    }
}
