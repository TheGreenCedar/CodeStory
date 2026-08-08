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
    doctor_sidecar_status_from_observation(
        runtime
            .activation
            .retrieval_status(&runtime.project_root, &runtime.storage_path),
    )
}

pub(in crate::app::diagnostics) fn doctor_sidecar_status_for_profile(
    runtime: &RuntimeContext,
    profile: codestory_runtime::RuntimeRetrievalProfile,
    run_id: Option<&str>,
) -> RetrievalStatusOutput {
    doctor_sidecar_status_from_observation(runtime.activation.retrieval_status_for_profile(
        &runtime.project_root,
        &runtime.storage_path,
        profile,
        run_id,
    ))
}

fn doctor_sidecar_status_from_observation(
    observation: Result<
        codestory_runtime::RetrievalStatusObservation,
        codestory_runtime::RetrievalStatusObservationError,
    >,
) -> RetrievalStatusOutput {
    match observation {
        Ok(observation) => {
            let ready_lease = observation.ready_lease().clone();
            let (selection, report) = observation.into_parts();
            doctor_sidecar_status_from_report_with_lease(
                report,
                Some(selection.profile().as_str()),
                selection.run_id(),
                ready_lease,
            )
        }
        Err(error) => {
            let ready_lease = error.ready_lease().clone();
            let profile = error.selection().profile().as_str();
            let run_id = error.selection().run_id().map(str::to_string);
            doctor_sidecar_status_error_with_lease(
                error,
                Some(profile),
                run_id.as_deref(),
                ready_lease,
            )
        }
    }
}

#[cfg(test)]
pub(in crate::app::diagnostics) fn doctor_sidecar_status_from_report(
    report: codestory_runtime::RetrievalStatusReport,
    profile: Option<&str>,
    run_id: Option<&str>,
) -> RetrievalStatusOutput {
    doctor_sidecar_status_from_report_with_lease(
        report,
        profile,
        run_id,
        codestory_runtime::ReadyLeaseEvidence::default(),
    )
}

fn doctor_sidecar_status_from_report_with_lease(
    report: codestory_runtime::RetrievalStatusReport,
    profile: Option<&str>,
    run_id: Option<&str>,
    ready_lease: codestory_runtime::ReadyLeaseEvidence,
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
        profile: profile.map(str::to_string),
        run_id: run_id.map(str::to_string),
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
        ready_lease,
    }
}

fn doctor_sidecar_status_error_with_lease(
    error: impl std::fmt::Display,
    profile: Option<&str>,
    run_id: Option<&str>,
    ready_lease: codestory_runtime::ReadyLeaseEvidence,
) -> RetrievalStatusOutput {
    RetrievalStatusOutput {
        profile: profile.map(str::to_string),
        run_id: run_id.map(str::to_string),
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
        ready_lease,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status_wire_test_support as wire;
    use serde_json::{Value, json};

    fn observed_status_cases() -> Value {
        json!({
            "healthy": doctor_sidecar_status_from_report_with_lease(
                wire::healthy_status_report(),
                Some("agent"),
                Some("golden-run"),
                wire::ready_lease_evidence(),
            ),
            "degraded": doctor_sidecar_status_from_report_with_lease(
                wire::degraded_status_report(),
                Some("agent"),
                Some("golden-run"),
                wire::stale_ready_lease_evidence(),
            ),
            "unavailable": doctor_sidecar_status_from_report(
                wire::unavailable_status_report(),
                Some("agent"),
                Some("golden-run"),
            ),
            "probe_error": doctor_sidecar_status_error_with_lease(
                wire::probe_error(),
                Some("agent"),
                Some("golden-run"),
                wire::unproven_ready_lease_evidence(),
            ),
        })
    }

    #[test]
    fn pre_change_status_wire_doctor_surface_is_non_vacuous() {
        let cases = observed_status_cases();
        let cases = cases.as_object().expect("doctor cases");
        let union = cases
            .values()
            .flat_map(|case| case.as_object().expect("doctor status object").keys())
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            union,
            wire::DOCTOR_STATUS_FIELDS.into_iter().collect(),
            "doctor status field set drifted"
        );
        wire::assert_non_null_coverage(
            &cases.values().cloned().collect::<Vec<_>>(),
            &wire::DOCTOR_STATUS_FIELDS,
            "doctor status",
        );
        wire::assert_json_golden(
            &Value::Object(cases.clone()),
            wire::DOCTOR_GOLDEN,
            "doctor status",
        );
    }
}
