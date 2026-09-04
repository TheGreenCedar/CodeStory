//! Benchmark-only implementation of the frozen ETR-1 frontier experiment.

use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[path = "codestory_proof_availability/build_provenance.rs"]
mod build_provenance;
#[path = "codestory_etr1/contract.rs"]
mod contract;
#[path = "codestory_etr1/prepare.rs"]
mod prepare;
#[path = "codestory_etr1/run.rs"]
mod run;

#[derive(Parser)]
#[command(about = "Run the frozen benchmark-only ETR-1 frontier experiment")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate the frozen corpus, rebuild BM25 memberships, and emit exact
    /// fragment documents for the existing embedding diagnostic.
    Prepare {
        #[arg(long)]
        evidence_root: PathBuf,
        #[arg(long)]
        corpus_root: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Build paired unconditioned and source-conditioned frontiers from a
    /// previously authenticated preparation and exact fragment vectors.
    Run {
        #[arg(long)]
        prepared: PathBuf,
        #[arg(long)]
        prepared_sha256: String,
        #[arg(long)]
        fragment_vectors: PathBuf,
        #[arg(long)]
        fragment_vectors_sha256: String,
        #[arg(long)]
        state_root: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
    },
}

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("internal-embedding-server") {
        ensure!(
            build_provenance::SOURCE_DIRTY.trim() == "false",
            "dirty_etr1_binary"
        );
        return codestory_cli::run_native_embedding_server();
    }
    let args = Args::parse();
    match args.command {
        Command::Prepare {
            evidence_root,
            corpus_root,
            output_dir,
        } => prepare::execute(&evidence_root, &corpus_root, &output_dir),
        Command::Run {
            prepared,
            prepared_sha256,
            fragment_vectors,
            fragment_vectors_sha256,
            state_root,
            output_dir,
        } => run::execute(
            &prepared,
            &prepared_sha256,
            &fragment_vectors,
            &fragment_vectors_sha256,
            &state_root,
            &output_dir,
        ),
    }
}
