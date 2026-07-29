use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codestory-embedding-constant-calibration")]
struct Arguments {
    #[arg(long, value_name = "CODESTORY_CLI")]
    cli: PathBuf,
    #[arg(long, value_name = "PRIVATE_JSON")]
    request: PathBuf,
    #[arg(long, value_name = "PRIVATE_JSON")]
    output: PathBuf,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    codestory_bench::qualification::run_constant_calibration(
        arguments.cli,
        arguments.request,
        arguments.output,
    )
}
