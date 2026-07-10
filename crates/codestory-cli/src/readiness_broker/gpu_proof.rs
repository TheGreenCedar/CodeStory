use super::types::{BrokerGpuProofInput, BrokerGpuProofSnapshot, BrokerGpuRuntimeIdentity};

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
        runtime_identity: None,
    }
}

pub(crate) fn bind_verified_runtime_identity(
    proof: &mut BrokerGpuProofSnapshot,
    runtime_identity: Option<&BrokerGpuRuntimeIdentity>,
) {
    if proof.proof_status != "verified" {
        proof.runtime_identity = None;
        return;
    }
    if let Some(runtime_identity) = runtime_identity
        && runtime_identity_supports_proof(proof, runtime_identity)
    {
        proof.runtime_identity = Some(runtime_identity.clone());
        return;
    }
    invalidate_verified_proof(proof);
}

pub(crate) fn inherit_verified_smoke(
    observed: &mut BrokerGpuProofSnapshot,
    persisted: &BrokerGpuProofSnapshot,
    current_runtime_identity: Option<&BrokerGpuRuntimeIdentity>,
) {
    if observed.proof_status != "gpu_unverified"
        || observed.embed_smoke_ok.is_some()
        || persisted.proof_status != "verified"
        || !same_gpu_observation(observed, persisted)
    {
        return;
    }
    let (Some(persisted_identity), Some(current_identity)) = (
        persisted.runtime_identity.as_ref(),
        current_runtime_identity,
    ) else {
        return;
    };
    if persisted_identity != current_identity
        || !runtime_identity_supports_proof(persisted, current_identity)
    {
        return;
    }
    *observed = persisted.clone();
}

fn invalidate_verified_proof(proof: &mut BrokerGpuProofSnapshot) {
    proof.proof_status = "gpu_unverified".to_string();
    proof.meaningful_accelerator_work_proven = false;
    proof.embed_smoke_ok = None;
    proof.embed_smoke_ms = None;
    proof.degraded_reason = Some("gpu_unverified".to_string());
    proof.runtime_identity = None;
}

fn runtime_identity_supports_proof(
    proof: &BrokerGpuProofSnapshot,
    identity: &BrokerGpuRuntimeIdentity,
) -> bool {
    if identity.workspace_id.is_empty()
        || identity.namespace.is_empty()
        || identity.compose_project.is_empty()
        || identity.embed_url.is_empty()
        || identity.started_at_epoch_ms <= 0
    {
        return false;
    }
    match proof.observation_source.as_deref() {
        Some("native_log") => identity.embedding_launch.as_ref().is_some_and(|launch| {
            launch.launch_mode
                == codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned.as_str()
                && launch.pid.is_some()
        }),
        Some("sidecar_log") => true,
        _ => false,
    }
}

fn same_gpu_observation(
    observed: &BrokerGpuProofSnapshot,
    persisted: &BrokerGpuProofSnapshot,
) -> bool {
    observed.requested == persisted.requested
        && observed.requested_provider == persisted.requested_provider
        && observed.requested_device == persisted.requested_device
        && observed.policy == persisted.policy
        && observed.observed_state == persisted.observed_state
        && observed.observation_source == persisted.observation_source
        && observed.detected_provider == persisted.detected_provider
        && observed.detected_gpu == persisted.detected_gpu
        && observed.cpu_allowed == persisted.cpu_allowed
}
