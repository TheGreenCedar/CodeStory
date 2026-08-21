//! Closed artifact contracts and inert command parsing for proof availability.
//!
//! This binary deliberately does not register a product route. Later benchmark
//! tasks own source materialization, execution, and report production.

use anyhow::{Context, Result, bail};

pub mod cli;
#[allow(dead_code)] // Later qualification tasks consume the full closed artifact surface.
pub mod contracts;
mod corpus;
mod util;

pub fn execute(cli: cli::Cli) -> Result<()> {
    match cli.command {
        cli::Command::Materialize(arguments) => {
            let corpus = corpus::load(&arguments.corpus)?;
            corpus
                .validate()
                .context("validate proof availability corpus")?;
            util::refuse_existing_output(&arguments.out)?;
            if arguments.verify_only {
                // Verification is deliberately parse-and-validate only until Task 10
                // owns detached checkout materialization. In particular, no indexer
                // or proof runner is reachable from this path.
                return Ok(());
            }
            bail!("proof_availability_materialize_not_implemented")
        }
        cli::Command::Run(arguments) => {
            util::refuse_existing_output(&arguments.out)?;
            bail!("proof_availability_run_not_implemented")
        }
        cli::Command::Verify(arguments) => {
            let thresholds = corpus::load_thresholds(&arguments.thresholds)?;
            thresholds
                .validate()
                .context("validate proof availability thresholds")?;
            // Verify is intentionally read-only. It neither creates an output
            // directory nor starts indexing or proof execution.
            Ok(())
        }
    }
}
