//! Write `docs/users/configuration-reference.md` from the configuration registry.
//!
//! `cargo run -p codestory-contracts --bin generate_config_docs`
//!
//! The drift test in `tests/config_reference_drift.rs` fails when the checked-in
//! page and this rendering disagree, so the page cannot describe settings the
//! registry does not declare.

use std::path::PathBuf;
use std::process::ExitCode;

use codestory_contracts::config_registry::{
    CONFIGURATION_REFERENCE_PATH, render_configuration_reference,
};

fn main() -> ExitCode {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .map(PathBuf::from);
    let Some(repo_root) = repo_root else {
        eprintln!("could not resolve the workspace root from the contracts manifest directory");
        return ExitCode::FAILURE;
    };
    let target = repo_root.join(CONFIGURATION_REFERENCE_PATH);
    match std::fs::write(&target, render_configuration_reference()) {
        Ok(()) => {
            println!("wrote {}", target.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to write {}: {error}", target.display());
            ExitCode::FAILURE
        }
    }
}
