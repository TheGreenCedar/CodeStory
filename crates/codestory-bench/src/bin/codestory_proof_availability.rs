use anyhow::Result;
use clap::Parser;

#[path = "codestory_proof_availability/mod.rs"]
mod proof_availability;

fn main() -> Result<()> {
    let cli = proof_availability::cli::Cli::parse();
    proof_availability::execute(cli)
}
