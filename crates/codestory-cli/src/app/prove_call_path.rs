use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};

use crate::args::{OutputFormat, ProjectArgs, VerifyIndexedDirectCallsCommand};
use crate::output::emit;
use crate::prove_call_path::{
    internal_projection_root, parse_request, project_public_verification_result, projection_root,
    read_bounded_spec, validate_request,
};
use crate::runtime::{RuntimeContext, map_api_error};

pub(super) fn run_verify_indexed_direct_calls(cmd: VerifyIndexedDirectCallsCommand) -> Result<()> {
    let bytes = read_bounded_spec(&cmd.spec)?;
    let value = serde_json::from_slice(&bytes).context("parse proof spec JSON")?;
    let request = parse_request(value).map_err(anyhow::Error::msg)?;
    let validation = validate_request(request).map_err(anyhow::Error::msg)?;
    let project = ProjectArgs {
        project: cmd.project,
        cache_dir: None,
    };
    let runtime = RuntimeContext::new_inspect_only(&project)?;
    runtime
        .activation
        .ensure_complete_core_for_observation(
            &runtime.project_root,
            &runtime.storage_path,
            Arc::new(AtomicBool::new(false)),
        )
        .map_err(map_api_error)?;
    let root = match validation {
        codestory_runtime::proof_qualification_support::ValidationOutcome::Validated {
            contract,
            hashes,
            rendering,
        } => {
            let operation = codestory_runtime::proof_qualification_support::
                run_observed_call_path_public_operation(
                    &runtime.runtime,
                    &contract,
                    &hashes,
                    &rendering,
                    Arc::new(AtomicBool::new(false)),
                )
                .map_err(map_api_error)?;
            projection_root(&operation).map_err(anyhow::Error::msg)?
        }
        codestory_runtime::proof_qualification_support::ValidationOutcome::Unknown {
            spec,
            hashes,
            rendering,
            gaps,
        } => {
            let operation = codestory_runtime::proof_qualification_support::
                run_translation_unknown_public_operation(
                    &runtime.runtime,
                    &spec,
                    &hashes,
                    &rendering,
                    &gaps,
                    Arc::new(AtomicBool::new(false)),
                )
                .map_err(map_api_error)?;
            internal_projection_root(&operation.value)
        }
    };
    let public = project_public_verification_result(root).map_err(anyhow::Error::msg)?;
    emit(OutputFormat::Json, &public, String::new(), None)
}
