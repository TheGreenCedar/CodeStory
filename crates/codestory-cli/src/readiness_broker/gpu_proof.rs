use super::types::{BrokerGpuProofInput, BrokerGpuProofSnapshot};

pub(crate) fn gpu_proof(input: BrokerGpuProofInput) -> BrokerGpuProofSnapshot {
    let requested = input.embedding_accelerator_requested.unwrap_or(false);
    let cpu_allowed = input.embedding_cpu_allowed.unwrap_or(false);
    let observed_state = input.embedding_device_state.clone();
    let accelerated = observed_state.as_deref() == Some("accelerated");
    let runtime_observed = matches!(
        input.embedding_device_observation_source.as_deref(),
        Some("native_log" | "sidecar_log")
    );
    let smoke_ok = input.embed_smoke_ok == Some(true);
    let proof_status = if accelerated && runtime_observed && !cpu_allowed && smoke_ok {
        "verified"
    } else if (accelerated && !cpu_allowed) || requested {
        "gpu_unverified"
    } else if cpu_allowed {
        "cpu_allowed"
    } else {
        "not_requested"
    };
    let degraded_reason = if proof_status == "gpu_unverified" {
        Some("gpu_unverified".to_string())
    } else {
        input.degraded_reason.clone()
    };
    BrokerGpuProofSnapshot {
        requested,
        requested_provider: input.embedding_accelerator_request_provider,
        requested_device: input.embedding_accelerator_request_device,
        policy: input.embedding_device_policy,
        observed_state,
        observation_source: input.embedding_device_observation_source,
        detected_provider: input.embedding_detected_provider,
        detected_gpu: input.embedding_detected_gpu,
        cpu_allowed,
        proof_status: proof_status.to_string(),
        meaningful_accelerator_work_proven: proof_status == "verified",
        embed_smoke_ok: input.embed_smoke_ok,
        embed_smoke_ms: input.embed_smoke_ms,
        degraded_reason,
    }
}
