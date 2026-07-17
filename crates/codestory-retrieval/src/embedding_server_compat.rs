//! Compatibility surface for callers that still use the former process-engine
//! names. Native embedding ownership lives only in the per-user server.

use crate::config::SidecarRuntimeConfig;
#[cfg(not(feature = "test-support"))]
use crate::per_user_embedding::PerUserEmbeddingResidencyLease;
use crate::per_user_embedding::{EmbeddingEngineIdentity, PerUserEmbeddingClient};
use anyhow::{Result, anyhow};
use std::path::PathBuf;
#[cfg(not(feature = "test-support"))]
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ProductEmbeddingIdentity {
    pub instance_id: String,
    pub load_generation: u64,
    pub model_load_count: u64,
    pub residency: &'static str,
    pub worker_alive: bool,
    pub load_error: Option<String>,
    pub model_digest: &'static str,
    pub ggml_build_identity: &'static str,
    pub backend: String,
    pub adapter_name: String,
    pub adapter_description: String,
    pub policy: &'static str,
    pub embedded_model: bool,
    pub materialized_path: PathBuf,
    pub materialized_reused: bool,
    pub initialization_ms: u64,
    pub smoke_ms: u64,
    pub adapter_memory_total: usize,
    pub adapter_memory_used_by_load: usize,
    pub execution_device_names: Vec<String>,
    pub execution_backend_names: Vec<String>,
    pub execution_observation_source: &'static str,
    pub encode_count: u64,
    pub execution_node_count: u64,
    pub resident_accelerator_tensor_count: u64,
    pub resident_accelerator_tensor_bytes: u64,
    pub model_layer_count: u32,
    pub offloaded_layer_count: u32,
    pub accelerator_execution_verified: bool,
}

#[derive(Debug)]
#[cfg(not(feature = "test-support"))]
pub struct ProductEmbeddingServerLease {
    inner: Mutex<PerUserEmbeddingResidencyLease>,
    identity: ProductEmbeddingIdentity,
}

#[cfg(not(feature = "test-support"))]
impl ProductEmbeddingServerLease {
    pub fn identity(&self) -> &ProductEmbeddingIdentity {
        &self.identity
    }

    pub fn revalidate(&self) -> Result<ProductEmbeddingIdentity> {
        let identity = self
            .inner
            .lock()
            .map_err(|_| anyhow!("embedding publication lease was poisoned"))?
            .revalidate()?;
        let identity = compatibility_identity(identity)?;
        Ok(identity)
    }
}

pub fn product_embedding_identity(
    runtime: &SidecarRuntimeConfig,
) -> Result<ProductEmbeddingIdentity> {
    let identity = PerUserEmbeddingClient::for_runtime(runtime)?.ensure_resident()?;
    let identity = compatibility_identity(identity)?;
    Ok(identity)
}

/// Observes the per-user server without spawning it or loading the model.
#[cfg(not(feature = "test-support"))]
pub fn product_embedding_identity_if_initialized(
    runtime: &SidecarRuntimeConfig,
) -> Result<Option<ProductEmbeddingIdentity>> {
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let Some((_snapshot, identity)) = client.observe_with_identity()? else {
        return Ok(None);
    };
    identity.map(compatibility_identity).transpose()
}

#[cfg(not(feature = "test-support"))]
pub fn acquire_product_embedding_server_lease(
    runtime: &SidecarRuntimeConfig,
) -> Result<ProductEmbeddingServerLease> {
    let inner = PerUserEmbeddingClient::for_runtime(runtime)?.acquire_residency_lease()?;
    let identity = compatibility_identity(inner.identity().clone())?;
    Ok(ProductEmbeddingServerLease {
        inner: Mutex::new(inner),
        identity,
    })
}

pub fn embed_prepared_via_server(
    runtime: &SidecarRuntimeConfig,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>> {
    let raw = inputs
        .iter()
        .map(|input| {
            input
                .strip_prefix(crate::embedding_contract::CODERANK_DOCUMENT_PREFIX)
                .unwrap_or(input)
                .to_string()
        })
        .collect::<Vec<_>>();
    PerUserEmbeddingClient::for_runtime(runtime)?.embed_documents(&raw)
}

pub fn embed_prepared_query_via_server(
    runtime: &SidecarRuntimeConfig,
    input: String,
) -> Result<Vec<f32>> {
    let raw = input
        .strip_prefix(crate::embedding_contract::CODERANK_QUERY_PREFIX)
        .unwrap_or(&input);
    PerUserEmbeddingClient::for_runtime(runtime)?.embed_query(raw)
}

fn compatibility_identity(identity: EmbeddingEngineIdentity) -> Result<ProductEmbeddingIdentity> {
    let residency = stable_name(&identity.residency)?;
    let policy = stable_name(&identity.policy)?;
    let execution_observation_source = stable_name(&identity.execution_observation_source)?;
    let model_digest = stable_name(&identity.model_digest)?;
    let ggml_build_identity = stable_name(&identity.ggml_build_identity)?;
    Ok(ProductEmbeddingIdentity {
        instance_id: identity.server_instance_id,
        load_generation: identity.load_generation,
        model_load_count: identity.model_load_count,
        residency,
        worker_alive: identity.worker_alive,
        load_error: identity.load_error,
        model_digest,
        ggml_build_identity,
        backend: identity.backend,
        adapter_name: identity.adapter_name,
        adapter_description: identity.adapter_description,
        policy,
        embedded_model: identity.embedded_model,
        // Paths and adapter memory are intentionally not sent over the
        // agent-facing protocol. These legacy fields remain neutral.
        materialized_path: PathBuf::new(),
        materialized_reused: identity.materialized_reused,
        initialization_ms: identity.initialization_ms,
        smoke_ms: identity.smoke_ms,
        adapter_memory_total: identity.adapter_memory_total as usize,
        adapter_memory_used_by_load: identity.adapter_memory_used_by_load as usize,
        execution_device_names: identity.execution_device_names,
        execution_backend_names: identity.execution_backend_names,
        execution_observation_source,
        encode_count: identity.encode_count,
        execution_node_count: identity.execution_node_count,
        resident_accelerator_tensor_count: identity.resident_accelerator_tensor_count,
        resident_accelerator_tensor_bytes: identity.resident_accelerator_tensor_bytes,
        model_layer_count: identity.model_layer_count,
        offloaded_layer_count: identity.offloaded_layer_count,
        accelerator_execution_verified: identity.accelerator_execution_verified,
    })
}

fn stable_name(value: &str) -> Result<&'static str> {
    match value {
        "resident" => Ok("resident"),
        "sleeping" => Ok("sleeping"),
        "cpu_explicit" => Ok("cpu_explicit"),
        "accelerated" => Ok("accelerated"),
        "ggml_eval_callback" => Ok("ggml_eval_callback"),
        value if value == codestory_llama_sys::MODEL_SHA256 => {
            Ok(codestory_llama_sys::MODEL_SHA256)
        }
        value if value == codestory_llama_sys::GGML_BUILD_IDENTITY => {
            Ok(codestory_llama_sys::GGML_BUILD_IDENTITY)
        }
        _ => Err(anyhow!(
            "embedding engine returned an unknown identity value"
        )),
    }
}
