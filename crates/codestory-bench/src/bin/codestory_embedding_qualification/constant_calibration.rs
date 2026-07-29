//! Private, package-bound producer for the runtime constants' measurement data.
//!
//! This lane deliberately shares only the qualification driver's low-level
//! worker protocol. It has its own request and output schemas and never enters
//! the lifecycle-scenario or external-evidence paths.

use anyhow::{Context, Result, bail};
use codestory_retrieval::SidecarRuntimeConfig;
use output::{ConstantCalibrationRawOutput, ConstantCalibrationRunSummary};
use request::REQUIRED_RUNS;
use scenarios::artifact::{ScenarioContext, run_constant_calibration};
use std::path::PathBuf;

use super::{output as qualification_output, scenarios};

mod output;
mod request;

pub fn run(cli: PathBuf, request_path: PathBuf, output_path: PathBuf) -> Result<()> {
    let validated = request::load(cli, &request_path, &output_path)?;
    let request::ValidatedRequest {
        request,
        executable,
        output_directory,
        output_path,
        nonce_sha256,
        request_sha256,
    } = validated;
    let runtime = SidecarRuntimeConfig::for_project_auto(&request.project);
    if runtime.embedding.allow_cpu {
        bail!("embedding_constant_calibration_cpu_fallback_enabled");
    }
    let runtimes = [runtime];
    let projects = [request.project.clone()];
    let artifacts = run_constant_calibration(
        ScenarioContext {
            scenario: "constant_calibration",
            runtimes: &runtimes,
            projects: &projects,
            primary_index: 0,
            contracts: &request.contracts,
            qualification_runtime: &request.runtime,
            output_directory: &output_directory,
            nonce_sha256: &nonce_sha256,
            worker_nonce: Some(&request.calibration_nonce),
            executable: &executable,
        },
        request.required_runs,
        &request.package.model_sha256,
    )
    .context("run embedding constant calibration")?;
    if artifacts.len() != REQUIRED_RUNS as usize {
        bail!("embedding_constant_calibration_run_count_invalid");
    }

    let mut calibration_runs = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let artifact_name = format!("constant-calibration-run-{}.raw.json", artifact.run_index());
        qualification_output::write_atomic_json(&output_directory.join(&artifact_name), &artifact)
            .with_context(|| {
                format!("write raw embedding constant calibration artifact {artifact_name}")
            })?;
        calibration_runs.push(ConstantCalibrationRunSummary::from_artifact(
            &artifact,
            artifact_name,
        ));
    }

    qualification_output::write_atomic_json(
        &output_path,
        &ConstantCalibrationRawOutput {
            schema_version: 1,
            source: request.source,
            package: request.package,
            contracts: request.contracts,
            runtime: request.runtime,
            request_sha256,
            calibration_runs,
        },
    )
    .context("write raw embedding constant calibration output")
}
