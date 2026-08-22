//! Closed artifact contracts and inert command parsing for proof availability.
//!
//! This binary deliberately does not register a product route. It owns only the
//! fixture and frozen-corpus qualification lifecycle.

use anyhow::{Context, Result};

mod build_provenance;
pub mod cli;
#[allow(dead_code)] // Later qualification tasks consume the full closed artifact surface.
pub mod contracts;
mod corpus;
#[allow(dead_code)] // Task 11 consumes the completed qualification inventory.
mod inventory;
mod materialize;
mod report;
mod runner;
#[allow(dead_code)] // Tasks 11 and 13 consume the frozen decision evaluator.
mod thresholds;
#[allow(dead_code)] // Task 11 consumes the completed qualification denominators.
mod trails;
mod util;

pub fn execute(cli: cli::Cli) -> Result<()> {
    match cli.command {
        cli::Command::Materialize(arguments) => {
            let loaded = corpus::load_complete(&arguments.corpus)?;
            if arguments.verify_only {
                return materialize::verify_only(&arguments, &loaded);
            }
            util::refuse_existing_output(&arguments.out)?;
            materialize::materialize_indexed(&arguments, &loaded)
        }
        cli::Command::Run(arguments) => {
            let loaded = corpus::load_complete(&arguments.corpus)?;
            let thresholds = corpus::load_thresholds(&arguments.thresholds)?;
            loaded
                .corpus
                .validate_against_thresholds(&thresholds)
                .context("validate proof availability corpus against thresholds")?;
            util::refuse_existing_output(&arguments.out)?;
            let operational = materialize::load_operational_environment(&arguments.environment)?;
            report::require_result_directory_identity(
                &arguments.out,
                &operational.environment.qualification_id,
            )?;
            let output_parent = arguments.out.parent().ok_or_else(|| {
                anyhow::anyhow!("proof_availability_case_diagnostic_parent_invalid")
            })?;
            let reservation = report::reserve_case_diagnostic(
                output_parent,
                &operational.environment.qualification_id,
            )?;
            let input = runner::run_qualification(&loaded, &thresholds, &operational)?;
            let summary = match report::build_summary(input, &loaded.corpus, &thresholds) {
                Ok(summary) => summary,
                Err(error) => {
                    if let Some(failure) = error.downcast_ref::<contracts::CaseValidationFailure>()
                    {
                        let forbidden_values =
                            std::iter::once(operational.workspace_root.display().to_string())
                                .chain(std::iter::once(
                                    operational.cache_root.display().to_string(),
                                ))
                                .chain(operational.repositories.iter().flat_map(|repository| {
                                    [
                                        repository.checkout_root.display().to_string(),
                                        repository.project_root.display().to_string(),
                                        repository.database_path.display().to_string(),
                                    ]
                                }))
                                .collect::<Vec<_>>();
                        report::write_invalid_case_diagnostic(
                            &reservation,
                            &operational.environment.qualification_id,
                            &operational.environment.qualification_source_commit,
                            &operational.environment.qualification_source_tree,
                            failure,
                            &forbidden_values,
                        )?;
                    }
                    return Err(error);
                }
            };
            let leak_policy = report::PublicLeakPolicy::new(
                std::iter::once(operational.workspace_root.display().to_string())
                    .chain(std::iter::once(
                        operational.cache_root.display().to_string(),
                    ))
                    .chain(operational.repositories.iter().flat_map(|repository| {
                        [
                            repository.checkout_root.display().to_string(),
                            repository.project_root.display().to_string(),
                            repository.database_path.display().to_string(),
                        ]
                    })),
            );
            report::build_and_publish(
                &arguments.out,
                &summary,
                &loaded.corpus,
                &thresholds,
                &leak_policy,
            )?;
            Ok(())
        }
        cli::Command::Verify(arguments) => {
            let loaded = corpus::load_complete(&arguments.corpus)?;
            let thresholds = corpus::load_thresholds(&arguments.thresholds)?;
            loaded
                .corpus
                .validate_against_thresholds(&thresholds)
                .context("validate proof availability corpus against thresholds")?;
            // Verify is intentionally read-only. It neither creates an output
            // directory nor starts indexing or proof execution.
            report::verify_published(
                &arguments.results,
                &loaded.corpus,
                &thresholds,
                &loaded.path_files,
                &report::PublicLeakPolicy::new(std::iter::empty::<String>()),
            )?;
            Ok(())
        }
    }
}
