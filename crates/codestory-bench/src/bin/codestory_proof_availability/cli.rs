use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codestory-proof-availability")]
#[command(about = "Materialize and verify closed proof-availability artifacts")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate a corpus and, in a later benchmark task, materialize its sources.
    Materialize(MaterializeArgs),
    /// Run the qualification harness into a new output directory.
    Run(RunArgs),
    /// Read and validate existing qualification inputs without writing output.
    Verify(VerifyArgs),
}

#[derive(Debug, Args)]
pub struct MaterializeArgs {
    #[arg(long, value_name = "CORPUS_JSON")]
    pub corpus: PathBuf,
    /// Validate corpus identities and oracle ranges without indexing or execution.
    #[arg(long)]
    pub verify_only: bool,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long, value_name = "CORPUS_JSON")]
    pub corpus: PathBuf,
    #[arg(long, value_name = "THRESHOLDS_JSON")]
    pub thresholds: PathBuf,
    #[arg(long, value_name = "OUTPUT_DIR")]
    pub output: PathBuf,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    #[arg(long, value_name = "CORPUS_JSON")]
    pub corpus: PathBuf,
    #[arg(long, value_name = "THRESHOLDS_JSON")]
    pub thresholds: PathBuf,
}
