//! The single benchmark-only structural frontier experiment. No product route.
#![allow(dead_code)]
use anyhow::{Result, ensure};
use clap::Parser;
use std::path::PathBuf;
#[path = "codestory_proof_availability/build_provenance.rs"]
mod build_provenance;
#[path = "codestory_etr1/contract.rs"]
mod contract;
#[path = "codestory_etr1/control.rs"]
mod control;
#[path = "codestory_etr1/prepare.rs"]
mod prepare;
// Reuse native encoding, token accounting, exact source authentication and
// lexical reconstruction. The ETR command and algorithm remain unchanged.
#[path = "codestory_etr1/run.rs"]
mod run;
#[derive(Parser)]
struct Args {
    #[arg(long)]
    job: PathBuf,
}
fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("internal-embedding-server") {
        ensure!(
            build_provenance::SOURCE_DIRTY.trim() == "false",
            "dirty_str1_binary"
        );
        return codestory_cli::run_native_embedding_server();
    }
    run::str1::execute(&Args::parse().job)
}
