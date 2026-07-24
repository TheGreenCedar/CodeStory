//! Server response construction, identity mapping, and error delivery.

use super::super::{
    EmbeddingCapacityPressureWire, EmbeddingClientBudgets, EmbeddingEngineIdentity,
    EmbeddingProtocolError, EmbeddingProtocolRequest, EmbeddingProtocolResponse,
    EmbeddingRequestClass, EmbeddingResult, EmbeddingServerStream,
    PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION, PER_USER_EMBEDDING_PROTOCOL_V1, duration_ms,
    write_frame,
};
use anyhow::{Context, Result, bail};
use codestory_llama_sys::{
    EmbeddingAdmissionSnapshot, EmbeddingCapacityPressure, EmbeddingCapacityReason,
    EmbeddingEngine, EngineError, EngineLifecycleSnapshot, NativeDeviceClass,
};
use std::time::Duration;
use uuid::Uuid;

pub(in crate::per_user_embedding) fn engine_identity(
    server_instance_id: &str,
    snapshot: &EngineLifecycleSnapshot,
) -> Result<EmbeddingEngineIdentity> {
    let identity = &snapshot.identity;
    let policy = match identity.selected_device_class {
        NativeDeviceClass::Cpu => "cpu_explicit",
        NativeDeviceClass::Accelerator => "accelerated",
        NativeDeviceClass::Unknown => bail!("embedding_backend_device_class_unknown"),
    };
    Ok(EmbeddingEngineIdentity {
        server_instance_id: server_instance_id.into(),
        load_generation: snapshot.load_generation,
        model_load_count: snapshot.model_load_count,
        residency: snapshot.residency.as_str().into(),
        worker_alive: snapshot.worker_alive,
        load_error: snapshot.load_error.clone(),
        model_digest: identity.model_digest.into(),
        ggml_build_identity: identity.ggml_build_identity.into(),
        backend: identity.backend.clone(),
        adapter_name: identity.adapter_name.clone(),
        adapter_description: identity.adapter_description.clone(),
        policy: policy.into(),
        embedded_model: identity.embedded_model,
        materialized_model_sha256: identity.model_digest.into(),
        materialized_reused: identity.materialized_reused,
        initialization_ms: duration_ms(identity.initialization_duration),
        smoke_ms: duration_ms(identity.smoke_duration),
        adapter_memory_total: identity.adapter_memory_total as u64,
        adapter_memory_used_by_load: identity
            .adapter_memory_free_before_load
            .saturating_sub(identity.adapter_memory_free_after_load)
            as u64,
        execution_device_names: identity.execution_device_names.clone(),
        execution_backend_names: identity.execution_backend_names.clone(),
        execution_observation_source: identity.execution_observation_source.into(),
        encode_count: identity.encode_count,
        execution_node_count: identity.execution_node_count,
        resident_accelerator_tensor_count: identity.resident_accelerator_tensor_count,
        resident_accelerator_tensor_bytes: identity.resident_accelerator_tensor_bytes,
        model_layer_count: identity.model_layer_count,
        offloaded_layer_count: identity.offloaded_layer_count,
        accelerator_execution_verified: identity.accelerator_execution_verified,
    })
}

pub(in crate::per_user_embedding) fn validate_protocol_request(
    request: &EmbeddingProtocolRequest,
) -> Result<()> {
    if request.protocol != PER_USER_EMBEDDING_PROTOCOL_V1
        || request.schema_version != PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION
        || request.request_id.trim().is_empty()
        || request.request_id.len() > 256
    {
        bail!("embedding_server_protocol_mismatch");
    }
    Ok(())
}

pub(in crate::per_user_embedding) fn capacity_wire(
    snapshot: &EmbeddingAdmissionSnapshot,
    reason: EmbeddingCapacityReason,
    request_class: EmbeddingRequestClass,
    retry_after_ms: u64,
    retry_condition: &str,
) -> EmbeddingCapacityPressureWire {
    let (capacity, depth) = match request_class {
        EmbeddingRequestClass::Query => (snapshot.query_capacity, snapshot.query_depth),
        EmbeddingRequestClass::Bulk => (snapshot.bulk_capacity, snapshot.bulk_depth),
    };
    EmbeddingCapacityPressureWire {
        reason: reason.as_str().into(),
        queue_class: request_class.as_str().into(),
        capacity: capacity as u64,
        depth: depth as u64,
        retry_after_ms,
        retry_condition: retry_condition.into(),
        owner_state: snapshot.owner_state.as_str().into(),
        active_scope_id: snapshot
            .active_request
            .as_ref()
            .map(|active| active.scope_id.clone()),
        active_request_id: snapshot
            .active_request
            .as_ref()
            .map(|active| active.request_id.clone()),
        active_request_class: snapshot
            .active_request
            .as_ref()
            .map(|active| active.request_class.as_str().into()),
    }
}

pub(in crate::per_user_embedding) fn success_response(
    request_id: &str,
    result: EmbeddingResult,
) -> EmbeddingProtocolResponse {
    EmbeddingProtocolResponse {
        protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
        schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
        request_id: request_id.into(),
        result: Some(result),
        error: None,
    }
}

pub(in crate::per_user_embedding) fn failure_response(
    request_id: &str,
    error: EmbeddingProtocolError,
) -> EmbeddingProtocolResponse {
    EmbeddingProtocolResponse {
        protocol: PER_USER_EMBEDDING_PROTOCOL_V1.into(),
        schema_version: PER_USER_EMBEDDING_PROTOCOL_SCHEMA_VERSION,
        request_id: request_id.into(),
        result: None,
        error: Some(error),
    }
}

pub(in crate::per_user_embedding) fn protocol_error(
    code: &str,
    message: &str,
) -> EmbeddingProtocolError {
    EmbeddingProtocolError {
        code: code.into(),
        message: message.into(),
        retry_class: "terminal".into(),
        retry_after_ms: 0,
        retry_condition: "the request or compatible executable changes".into(),
        capacity: None,
    }
}

pub(in crate::per_user_embedding) fn configure_server_operation_timeout(
    stream: &dyn EmbeddingServerStream,
    deadline_ms: u64,
) -> Result<()> {
    let wire_timeout = Duration::from_millis(deadline_ms);
    if wire_timeout.is_zero() {
        bail!("embedding_server_deadline_invalid");
    }
    // The wire deadline can shorten an exchange, but it cannot lengthen a
    // response write. In particular, Windows PIPE_NOWAIT writes must retry
    // zero progress while the kernel buffer is full. A peer-selected timeout
    // there would let a non-reading same-user client retain every bounded
    // connection handler. The smallest frozen request budget is already
    // qualified for responses larger than the Windows pipe buffer.
    let timeout = wire_timeout.min(EmbeddingClientBudgets::current().query_request);
    stream
        .set_read_timeout(Some(timeout))
        .context("bound embedding server request read")?;
    stream
        .set_write_timeout(Some(timeout))
        .context("bound embedding server response write")
}

pub(in crate::per_user_embedding) fn write_deadline_invalid(
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
) -> Result<()> {
    write_protocol_response(
        stream,
        failure_response(
            request_id,
            protocol_error(
                "embedding_server_deadline_invalid",
                "embedding request deadline must be finite and positive",
            ),
        ),
        &[],
    )
}

pub(in crate::per_user_embedding) fn write_deadline_exceeded(
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
    retry_after_ms: u64,
    request_class: EmbeddingRequestClass,
    engine: Option<&EmbeddingEngine>,
) -> Result<()> {
    let capacity = engine.map(|engine| {
        capacity_wire(
            &engine.admission_snapshot(),
            EmbeddingCapacityReason::DeadlineElapsed,
            request_class,
            retry_after_ms,
            "the active request completes or the server instance changes",
        )
    });
    write_protocol_response(
        stream,
        failure_response(
            request_id,
            EmbeddingProtocolError {
                code: "embedding_deadline_exceeded".into(),
                message: "embedding request exceeded its server-owned soft deadline".into(),
                retry_class: "after_delay".into(),
                retry_after_ms,
                retry_condition: "the active request completes or the server instance changes"
                    .into(),
                capacity,
            },
        ),
        &[],
    )
}

pub(in crate::per_user_embedding) fn write_protocol_response(
    stream: &mut dyn EmbeddingServerStream,
    response: EmbeddingProtocolResponse,
    payload: &[u8],
) -> Result<()> {
    write_frame(stream, &response, payload)
}

pub(in crate::per_user_embedding) fn write_engine_failure(
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
    error: EngineError,
) -> Result<()> {
    let protocol_error = match error.capacity_pressure() {
        Some(pressure) => EmbeddingProtocolError {
            code: "embedding_capacity".into(),
            message: error.to_string(),
            retry_class: "after_capacity_change".into(),
            retry_after_ms: pressure.retry_after_ms,
            retry_condition: pressure.retry_condition.clone(),
            capacity: Some(EmbeddingCapacityPressureWire::from(pressure)),
        },
        None => EmbeddingProtocolError {
            code: error.reason_code().into(),
            message: error.to_string(),
            retry_class: if matches!(error, EngineError::Cancelled) {
                "none"
            } else {
                "after_server_change"
            }
            .into(),
            retry_after_ms: 0,
            retry_condition: "the server instance or engine evidence changes".into(),
            capacity: None,
        },
    };
    write_protocol_response(stream, failure_response(request_id, protocol_error), &[])
}

pub(in crate::per_user_embedding) fn write_anyhow_failure(
    stream: &mut dyn EmbeddingServerStream,
    request_id: &str,
    error: anyhow::Error,
) -> Result<()> {
    if let Some(engine) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<EngineError>())
    {
        let code = engine.reason_code();
        return write_protocol_response(
            stream,
            failure_response(
                request_id,
                EmbeddingProtocolError {
                    code: code.into(),
                    message: error.to_string(),
                    retry_class: "after_server_change".into(),
                    retry_after_ms: 0,
                    retry_condition: "the server instance or engine evidence changes".into(),
                    capacity: engine
                        .capacity_pressure()
                        .map(EmbeddingCapacityPressureWire::from),
                },
            ),
            &[],
        );
    }
    write_protocol_response(
        stream,
        failure_response(
            request_id,
            EmbeddingProtocolError {
                code: "embedding_server_internal_error".into(),
                message: error.to_string(),
                retry_class: "terminal".into(),
                retry_after_ms: 0,
                retry_condition: "the request or server implementation changes".into(),
                capacity: None,
            },
        ),
        &[],
    )
}

impl From<&EmbeddingCapacityPressure> for EmbeddingCapacityPressureWire {
    fn from(pressure: &EmbeddingCapacityPressure) -> Self {
        Self {
            reason: pressure.reason.as_str().into(),
            queue_class: pressure.request_class.as_str().into(),
            capacity: pressure.capacity as u64,
            depth: pressure.depth as u64,
            retry_after_ms: pressure.retry_after_ms,
            retry_condition: pressure.retry_condition.clone(),
            owner_state: pressure.owner_state.as_str().into(),
            active_scope_id: pressure.active_scope_id.clone(),
            active_request_id: pressure.active_request_id.clone(),
            active_request_class: pressure
                .active_request_class
                .map(|class| class.as_str().into()),
        }
    }
}

pub(in crate::per_user_embedding) fn request_key(connection_id: &str, request_id: &str) -> String {
    format!("{connection_id}:{request_id}")
}

pub(in crate::per_user_embedding) fn valid_cancel_token(token: &str) -> bool {
    Uuid::parse_str(token).is_ok()
}

pub(in crate::per_user_embedding) fn engine_error(error: EngineError) -> anyhow::Error {
    anyhow::Error::new(error)
}
