//! Private, package-bound producer for per-user embedding qualification data.
//!
//! The driver validates the requested package and evidence directory. Scenario
//! owners collect raw product-path observations; the release harness evaluates
//! the retained evidence.

use anyhow::{Context, Result, bail};
use codestory_retrieval::SidecarRuntimeConfig;
use output::{QualificationRawOutput, write_atomic_json};
use request::REQUIRED_SCENARIOS;
use scenarios::artifact::{ScenarioContext, run_measurements, run_scenario};
use std::collections::BTreeMap;
use std::path::PathBuf;

mod output;
mod request;
mod scenarios;

const DIAGNOSTIC_SCENARIO_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIAGNOSTIC_SCENARIO";

pub(super) fn run(cli: PathBuf, request_path: PathBuf, output_path: PathBuf) -> Result<()> {
    let validated = request::load(cli, &request_path, &output_path)?;
    let request::ValidatedRequest {
        request,
        executable,
        output_directory,
        output_path,
        nonce_sha256,
        request_sha256,
    } = validated;
    let runtimes = request
        .projects
        .iter()
        .map(|project| SidecarRuntimeConfig::for_project_auto(project))
        .collect::<Vec<_>>();

    if diagnostic_worker_stall_enabled()? {
        let artifact = run_scenario(ScenarioContext {
            scenario: "worker_stall",
            runtimes: &runtimes,
            projects: &request.projects,
            primary_index: 0,
            contracts: &request.contracts,
            qualification_runtime: &request.runtime,
            output_directory: &output_directory,
            nonce_sha256: &nonce_sha256,
            executable: &executable,
        })
        .context("run diagnostic embedding qualification scenario worker_stall")?;
        return write_atomic_json(&output_path, &artifact)
            .context("write diagnostic worker_stall artifact");
    }

    let measurements_artifact_name = "measurements.raw.json";
    let measurements_artifact = run_measurements(ScenarioContext {
        scenario: "measurements",
        runtimes: &runtimes,
        projects: &request.projects,
        primary_index: 0,
        contracts: &request.contracts,
        qualification_runtime: &request.runtime,
        output_directory: &output_directory,
        nonce_sha256: &nonce_sha256,
        executable: &executable,
    })
    .context("run embedding qualification measurements")?;
    let measurements = measurements_artifact.summary(measurements_artifact_name.into());
    write_atomic_json(
        &output_directory.join(measurements_artifact_name),
        &measurements_artifact,
    )
    .context("write raw embedding qualification measurements")?;

    let mut scenario_summaries = BTreeMap::new();
    for (index, scenario) in REQUIRED_SCENARIOS.iter().enumerate() {
        let artifact = run_scenario(ScenarioContext {
            scenario,
            runtimes: &runtimes,
            projects: &request.projects,
            primary_index: index % runtimes.len(),
            contracts: &request.contracts,
            qualification_runtime: &request.runtime,
            output_directory: &output_directory,
            nonce_sha256: &nonce_sha256,
            executable: &executable,
        })
        .with_context(|| format!("run named embedding qualification scenario {scenario}"))?;
        let artifact_name = format!("{scenario}.raw.json");
        write_atomic_json(&output_directory.join(&artifact_name), &artifact)
            .with_context(|| format!("write raw qualification artifact {artifact_name}"))?;
        scenario_summaries.insert((*scenario).to_string(), artifact.summary(artifact_name));
    }

    write_atomic_json(
        &output_path,
        &QualificationRawOutput {
            schema_version: 2,
            tier: request.proof_tier,
            source: request.source,
            package: request.package,
            contracts: request.contracts,
            runtime: request.runtime,
            request_sha256,
            measurements,
            scenarios: scenario_summaries,
        },
    )
    .context("write raw qualification output")
}

fn diagnostic_worker_stall_enabled() -> Result<bool> {
    match std::env::var_os(DIAGNOSTIC_SCENARIO_ENV) {
        None => Ok(false),
        Some(value) if value == "worker_stall" => Ok(true),
        Some(_) => bail!("embedding_qualification_diagnostic_scenario_invalid"),
    }
}
