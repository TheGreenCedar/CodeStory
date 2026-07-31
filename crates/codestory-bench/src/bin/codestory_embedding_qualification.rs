use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "codestory-embedding-qualification")]
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
    codestory_bench::qualification::run(arguments.cli, arguments.request, arguments.output)
}
