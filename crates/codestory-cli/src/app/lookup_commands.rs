//! `find`, `refs` and `read`: lookups answered from the live source tree.
//!
//! These handlers resolve the project and its storage path without opening
//! the project: no refresh, no activation, no whole-tree freshness check. The
//! runtime reads live text and consults the pinned index only for the files
//! it serves.

use crate::args::{FindCommand, ReadCommand, RefsCommand};
use crate::runtime::{RuntimeContext, map_api_error};
use anyhow::Result;
use codestory_contracts::api::ApiError;
use codestory_runtime::served_lookup;
use std::io::Write as _;

pub(super) fn run_find(cmd: FindCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    emit(served_lookup::find(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.terms,
        &cmd.file_globs,
        cmd.limit,
        cmd.bodies.budget(),
    ))
}

pub(super) fn run_refs(cmd: RefsCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    emit(served_lookup::refs(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.name,
        cmd.limit,
        cmd.bodies.budget(),
    ))
}

pub(super) fn run_read(cmd: ReadCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    emit(served_lookup::read(
        &runtime.project_root,
        &runtime.storage_path,
        &cmd.selectors,
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
