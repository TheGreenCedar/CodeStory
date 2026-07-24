use crate::args::RetrievalStatusOutput;
use crate::readiness;
use crate::runtime::RuntimeContext;
use codestory_contracts::api::IndexFreshnessDto;

pub(crate) fn build_summary_readiness(
    project: &str,
    stats: &codestory_contracts::api::StorageStatsDto,
    freshness: Option<&IndexFreshnessDto>,
    sidecar: &RetrievalStatusOutput,
) -> Vec<codestory_contracts::api::ReadinessVerdictDto> {
    readiness::build_readiness_verdicts(readiness::ReadinessInputs {
        project,
        stats,
        freshness,
        sidecar: Some(readiness_sidecar_input(sidecar)),
    })
}

pub(in crate::app::diagnostics) fn readiness_sidecar_input(
    sidecar: &RetrievalStatusOutput,
) -> readiness::ReadinessSidecarInput<'_> {
    readiness::ReadinessSidecarInput {
        profile: sidecar.profile.as_deref(),
        run_id: sidecar.run_id.as_deref(),
        retrieval_mode: sidecar.retrieval_mode.as_str(),
        degraded_reason: sidecar.degraded_reason.as_deref(),
        embedding_device_policy: Some(sidecar.embedding_device_policy.as_str()),
        embedding_device_state: Some(sidecar.embedding_device_state.as_str()),
        embedding_device_observation_source: Some(
            sidecar.embedding_device_observation_source.as_str(),
        ),
        embedding_detected_provider: sidecar.embedding_detected_provider.as_deref(),
        embedding_detected_gpu: sidecar.embedding_detected_gpu.as_deref(),
        embedding_accelerator_requested: sidecar.embedding_accelerator_requested,
        embedding_accelerator_request_provider: sidecar
            .embedding_accelerator_request_provider
            .as_deref(),
        embedding_accelerator_request_device: sidecar
            .embedding_accelerator_request_device
            .as_deref(),
        embedding_cpu_allowed: sidecar.embedding_cpu_allowed,
        manifest_generation: sidecar.manifest_generation.as_deref(),
        manifest_input_hash: sidecar.manifest_input_hash.as_deref(),
    }
}

pub(crate) fn doctor_sidecar_status(runtime: &RuntimeContext) -> RetrievalStatusOutput {
    doctor_sidecar_status_for_runtime(runtime, runtime.sidecar.clone())
}

pub(in crate::app::diagnostics) fn doctor_sidecar_status_for_runtime(
    runtime: &RuntimeContext,
    sidecar: codestory_retrieval::SidecarRuntimeConfig,
) -> RetrievalStatusOutput {
    match codestory_retrieval::strict_sidecar_status_for_runtime(
        &runtime.project_root,
        Some(&runtime.storage_path),
        sidecar.clone(),
    ) {
        Ok(report) => doctor_sidecar_status_from_report(report, Some(&sidecar)),
        Err(error) => doctor_sidecar_status_error(error, Some(&sidecar)),
    }
}

pub(in crate::app::diagnostics) fn doctor_sidecar_status_from_report(
    report: codestory_retrieval::RetrievalStatusReport,
    runtime: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> RetrievalStatusOutput {
    let manifest_generation = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.sidecar_generation.clone());
    let manifest_input_hash = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.sidecar_input_hash.clone());
    let precise_semantic_import_status = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_status.clone());
    let precise_semantic_import_reason = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_reason.clone());
    let precise_semantic_import_revision = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_revision.clone());
    let precise_semantic_import_producer = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_producer.clone());
    RetrievalStatusOutput {
        profile: runtime.map(|runtime| runtime.profile.as_str().to_string()),
        run_id: runtime.and_then(|runtime| runtime.run_id.clone()),
        retrieval_mode: report.retrieval_mode,
        degraded_reason: report.degraded_reason,
        embedding_device_policy: report.embedding_device_policy,
        embedding_device_state: report.embedding_device_state,
        embedding_device_observation_source: report.embedding_device_observation_source,
        embedding_detected_provider: report.embedding_detected_provider,
        embedding_detected_gpu: report.embedding_detected_gpu,
        embedding_accelerator_requested: report.embedding_accelerator_requested,
        embedding_accelerator_request_provider: report.embedding_accelerator_request_provider,
        embedding_accelerator_request_device: report.embedding_accelerator_request_device,
        embedding_cpu_allowed: report.embedding_cpu_allowed,
        manifest_generation,
        manifest_input_hash,
        precise_semantic_import_status,
        precise_semantic_import_reason,
        precise_semantic_import_revision,
        precise_semantic_import_producer,
    }
}

pub(in crate::app::diagnostics) fn doctor_sidecar_status_error(
    error: anyhow::Error,
    runtime: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> RetrievalStatusOutput {
    RetrievalStatusOutput {
        profile: runtime.map(|runtime| runtime.profile.as_str().to_string()),
        run_id: runtime.and_then(|runtime| runtime.run_id.clone()),
        retrieval_mode: "unavailable".to_string(),
        degraded_reason: Some(format!("retrieval_status_error: {error}")),
        embedding_device_policy: "accelerator_required".to_string(),
        embedding_device_state: "unknown".to_string(),
        embedding_device_observation_source: "retrieval_unobserved".to_string(),
        embedding_detected_provider: None,
        embedding_detected_gpu: None,
        embedding_accelerator_requested: false,
        embedding_accelerator_request_provider: None,
        embedding_accelerator_request_device: None,
        embedding_cpu_allowed: false,
        manifest_generation: None,
        manifest_input_hash: None,
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    }
}
