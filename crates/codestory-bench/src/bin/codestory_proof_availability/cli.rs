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
    #[arg(long, value_name = "WORKSPACE_DIR")]
    pub workspace: PathBuf,
    #[arg(long, value_name = "CACHE_DIR")]
    pub cache_root: PathBuf,
    #[arg(long, value_name = "ENVIRONMENT_JSON")]
    pub out: PathBuf,
    /// Immutable indexed-qualification identity bound to the source commit.
    #[arg(
        long,
        value_name = "YYYYMMDDTHHMMSSZ-COMMIT12",
        value_parser = parse_qualification_id,
        required_unless_present = "verify_only",
        conflicts_with = "verify_only"
    )]
    pub qualification_id: Option<String>,
    /// Validate corpus identities and oracle ranges without indexing or execution.
    #[arg(long)]
    pub verify_only: bool,
}

fn parse_qualification_id(value: &str) -> Result<String, String> {
    if valid_qualification_id(value) {
        Ok(value.to_owned())
    } else {
        Err(
            "expected YYYYMMDDTHHMMSSZ followed by '-' and 12 lowercase hexadecimal characters"
                .to_owned(),
        )
    }
}

pub(crate) fn valid_qualification_id(value: &str) -> bool {
    super::contracts::valid_qualification_id(value)
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long, value_name = "CORPUS_JSON")]
    pub corpus: PathBuf,
    #[arg(long, value_name = "THRESHOLDS_JSON")]
    pub thresholds: PathBuf,
    #[arg(long, value_name = "ENVIRONMENT_JSON")]
    pub environment: PathBuf,
    #[arg(long, value_name = "OUTPUT_DIR")]
    pub out: PathBuf,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    #[arg(long, value_name = "CORPUS_JSON")]
    pub corpus: PathBuf,
    #[arg(long, value_name = "THRESHOLDS_JSON")]
    pub thresholds: PathBuf,
    #[arg(long, value_name = "RESULTS_DIR")]
    pub results: PathBuf,
}
