//! Fixed CodeRankEmbed vectors backed by the process-wide, linked llama.cpp engine.

use crate::config::SidecarRuntimeConfig;
use crate::in_process_embedding::{
    ProcessEmbeddingIdentity, embed_prepared_in_process, embed_prepared_query_in_process,
};
#[cfg(not(feature = "test-support"))]
use crate::in_process_embedding::{
    process_embedding_identity, process_embedding_identity_if_initialized,
};
#[cfg(not(feature = "test-support"))]
use anyhow::anyhow;
use anyhow::{Result, bail};
use std::path::PathBuf;
#[cfg(not(feature = "test-support"))]
use std::time::Instant;

/// CodeRankEmbed vector width shared by stored and query vectors.
pub const RETRIEVAL_EMBEDDING_DIM: usize = 768;
pub const CODERANK_EMBED_Q8_GGUF: &str = codestory_llama_sys::MODEL_FILE_NAME;
pub const CODERANK_QUERY_PREFIX_DEFAULT: &str =
    "Represent this query for searching relevant code: ";

/// Manifest producer identity. Changing the model or linked ggml source makes
/// existing semantic generations stale and causes one transparent rebuild.
pub const PRODUCT_EMBEDDING_RUNTIME_ID: &str = codestory_llama_sys::PRODUCT_EMBEDDING_RUNTIME_ID;

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeProbe {
    pub reachable: bool,
    pub detail: String,
    pub elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingAcceleratorSmoke {
    pub elapsed_ms: u64,
    pub device: EmbeddingDeviceReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingDeviceReadiness {
    pub requested_policy: &'static str,
    pub observed_state: &'static str,
    pub observation_source: &'static str,
    pub detected_provider: Option<String>,
    pub detected_gpu: Option<String>,
    pub accelerator_requested: bool,
    pub accelerator_request_provider: Option<String>,
    pub accelerator_request_device: Option<String>,
    pub cpu_allowed: bool,
    pub full_retrieval_allowed: bool,
    pub degraded_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingEngineSnapshot {
    pub probe: EmbeddingRuntimeProbe,
    pub device: EmbeddingDeviceReadiness,
    pub identity: Option<ProcessEmbeddingIdentity>,
}

/// Cheap cloneable handle into the one engine owned by this process.
#[derive(Debug, Clone)]
pub struct InProcessEmbeddingClient {
    cache_root: PathBuf,
    allow_cpu: bool,
}

impl InProcessEmbeddingClient {
    pub fn new(runtime: &SidecarRuntimeConfig) -> Self {
        Self {
            cache_root: runtime.cache_root.clone(),
            allow_cpu: runtime.embedding.allow_cpu,
        }
    }

    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        if text.trim().is_empty() {
            bail!("cannot embed an empty query");
        }
        let prepared = format!("{CODERANK_QUERY_PREFIX_DEFAULT}{text}");
        embed_prepared_query_in_process(&self.cache_root, self.allow_cpu, prepared)
    }

    pub fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_prepared_texts(texts)
    }

    pub fn embed_prepared_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.iter().any(|text| text.trim().is_empty()) {
            bail!("cannot embed empty text");
        }
        embed_prepared_in_process(&self.cache_root, self.allow_cpu, texts)
    }
}

pub fn embedding_runtime_id() -> String {
    PRODUCT_EMBEDDING_RUNTIME_ID.into()
}

pub fn embedding_runtime_id_for_runtime(_runtime: &SidecarRuntimeConfig) -> String {
    embedding_runtime_id()
}

pub fn manifest_embedding_backend_is_product(backend: Option<&str>) -> bool {
    backend == Some(PRODUCT_EMBEDDING_RUNTIME_ID)
}

pub fn semantic_vector_dim() -> usize {
    RETRIEVAL_EMBEDDING_DIM
}

pub fn embedding_backend_label() -> &'static str {
    "inprocess"
}

pub fn embedding_backend_label_for_runtime(_runtime: &SidecarRuntimeConfig) -> &'static str {
    embedding_backend_label()
}

pub fn embed_query_for_runtime(runtime: &SidecarRuntimeConfig, text: &str) -> Result<Vec<f32>> {
    InProcessEmbeddingClient::new(runtime).embed_query(text)
}

pub fn embed_documents_for_runtime(
    runtime: &SidecarRuntimeConfig,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    InProcessEmbeddingClient::new(runtime).embed_documents(texts)
}

#[cfg(test)]
pub fn embed_query(text: &str) -> Result<Vec<f32>> {
    embed_query_for_runtime(&SidecarRuntimeConfig::local(), text)
}

/// Initializes the engine on an activating product path and validates the
/// exact model, build, device, policy, and timed startup smoke evidence.
pub fn ensure_product_embedding_backend() -> Result<()> {
    ensure_product_embedding_backend_for_runtime(&SidecarRuntimeConfig::local())
}

pub fn ensure_product_embedding_backend_for_runtime(runtime: &SidecarRuntimeConfig) -> Result<()> {
    #[cfg(feature = "test-support")]
    {
        let _ = runtime;
        Ok(())
    }
    #[cfg(not(feature = "test-support"))]
    {
        let identity =
            process_embedding_identity(&runtime.cache_root, runtime.embedding.allow_cpu)?;
        validate_identity(&identity, runtime.embedding.allow_cpu)
    }
}

/// Observes readiness without starting the engine or materializing the model.
pub fn probe_product_embedding_runtime() -> EmbeddingRuntimeProbe {
    probe_product_embedding_runtime_for_runtime(&SidecarRuntimeConfig::local())
}

pub fn probe_product_embedding_runtime_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> EmbeddingRuntimeProbe {
    embedding_engine_snapshot_for_runtime(runtime).probe
}

pub fn embedding_engine_snapshot_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> EmbeddingEngineSnapshot {
    #[cfg(feature = "test-support")]
    {
        let _ = runtime;
        let allow_cpu = runtime.embedding.allow_cpu;
        EmbeddingEngineSnapshot {
            probe: EmbeddingRuntimeProbe {
                reachable: true,
                detail: "retrieval embeddings ready".into(),
                elapsed_ms: Some(0),
            },
            device: EmbeddingDeviceReadiness {
                requested_policy: requested_policy(allow_cpu),
                observed_state: if allow_cpu {
                    "cpu_explicit"
                } else {
                    "accelerated"
                },
                observation_source: "test_support",
                detected_provider: Some(if allow_cpu { "CPU" } else { "test-accelerator" }.into()),
                detected_gpu: (!allow_cpu).then(|| "test-accelerator".into()),
                accelerator_requested: !allow_cpu,
                accelerator_request_provider: (!allow_cpu).then(|| "test-accelerator".into()),
                accelerator_request_device: (!allow_cpu).then(|| "test-accelerator".into()),
                cpu_allowed: allow_cpu,
                full_retrieval_allowed: true,
                degraded_reason: None,
            },
            identity: None,
        }
    }
    #[cfg(not(feature = "test-support"))]
    {
        let started = Instant::now();
        let result = process_embedding_identity_if_initialized(
            &runtime.cache_root,
            runtime.embedding.allow_cpu,
        )
        .and_then(|identity| {
            let identity =
                identity.ok_or_else(|| anyhow!("retrieval embeddings not initialized"))?;
            validate_identity(&identity, runtime.embedding.allow_cpu)?;
            Ok(identity)
        });
        let elapsed_ms = Some(elapsed_ms(started));
        match result {
            Ok(identity) => EmbeddingEngineSnapshot {
                probe: EmbeddingRuntimeProbe {
                    reachable: true,
                    detail: "retrieval embeddings ready".into(),
                    elapsed_ms,
                },
                device: readiness_from_identity(&identity, runtime.embedding.allow_cpu),
                identity: Some(identity),
            },
            Err(error) => EmbeddingEngineSnapshot {
                probe: EmbeddingRuntimeProbe {
                    reachable: false,
                    detail: format!("retrieval embeddings unavailable: {error}"),
                    elapsed_ms,
                },
                device: unavailable_readiness(runtime.embedding.allow_cpu, &error.to_string()),
                identity: None,
            },
        }
    }
}

pub fn embedding_device_readiness() -> EmbeddingDeviceReadiness {
    embedding_device_readiness_for_runtime(&SidecarRuntimeConfig::local())
}

pub fn embedding_device_readiness_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> EmbeddingDeviceReadiness {
    embedding_engine_snapshot_for_runtime(runtime).device
}

pub fn ensure_embedding_accelerator_smoke_for_runtime(
    runtime: &SidecarRuntimeConfig,
) -> Result<Option<EmbeddingAcceleratorSmoke>> {
    #[cfg(feature = "test-support")]
    {
        let device = embedding_device_readiness_for_runtime(runtime);
        Ok(
            (!runtime.embedding.allow_cpu).then_some(EmbeddingAcceleratorSmoke {
                elapsed_ms: 0,
                device,
            }),
        )
    }
    #[cfg(not(feature = "test-support"))]
    {
        let identity =
            process_embedding_identity(&runtime.cache_root, runtime.embedding.allow_cpu)?;
        validate_identity(&identity, runtime.embedding.allow_cpu)?;
        if runtime.embedding.allow_cpu {
            return Ok(None);
        }
        let device = readiness_from_identity(&identity, false);
        Ok(Some(EmbeddingAcceleratorSmoke {
            elapsed_ms: identity.smoke_ms,
            device,
        }))
    }
}

#[cfg(any(not(feature = "test-support"), test))]
fn validate_identity(identity: &ProcessEmbeddingIdentity, allow_cpu: bool) -> Result<()> {
    if !identity.embedded_model {
        bail!("embedding model is not embedded in this executable");
    }
    if identity.model_digest != codestory_llama_sys::MODEL_SHA256 {
        bail!(
            "embedding model digest mismatch: expected={} observed={}",
            codestory_llama_sys::MODEL_SHA256,
            identity.model_digest
        );
    }
    if identity.ggml_build_identity != codestory_llama_sys::GGML_BUILD_IDENTITY {
        bail!(
            "ggml build identity mismatch: expected={} observed={}",
            codestory_llama_sys::GGML_BUILD_IDENTITY,
            identity.ggml_build_identity
        );
    }
    if allow_cpu {
        if identity.policy != "cpu_explicit" {
            bail!(
                "explicit CPU policy selected but engine reported {}",
                identity.policy
            );
        }
        return Ok(());
    }
    if identity.policy != "accelerated"
        || !identity.accelerator_execution_verified
        || identity.execution_device_names.is_empty()
        || identity.offloaded_layer_count != identity.model_layer_count
    {
        bail!(
            "accelerated embedding execution is unverified: backend={} adapter={} offloaded_layers={}/{}",
            identity.backend,
            identity.adapter_name,
            identity.offloaded_layer_count,
            identity.model_layer_count
        );
    }
    Ok(())
}

#[cfg(not(feature = "test-support"))]
fn readiness_from_identity(
    identity: &ProcessEmbeddingIdentity,
    allow_cpu: bool,
) -> EmbeddingDeviceReadiness {
    let validation = validate_identity(identity, allow_cpu);
    let full_retrieval_allowed = validation.is_ok();
    EmbeddingDeviceReadiness {
        requested_policy: requested_policy(allow_cpu),
        observed_state: if allow_cpu {
            "cpu_explicit"
        } else if full_retrieval_allowed {
            "accelerated"
        } else {
            "unverified"
        },
        observation_source: "inprocess_engine",
        detected_provider: Some(identity.backend.clone()),
        detected_gpu: (!allow_cpu).then(|| identity.adapter_name.clone()),
        accelerator_requested: !allow_cpu,
        accelerator_request_provider: (!allow_cpu).then(|| identity.backend.clone()),
        accelerator_request_device: (!allow_cpu).then(|| identity.adapter_name.clone()),
        cpu_allowed: allow_cpu,
        full_retrieval_allowed,
        degraded_reason: validation.err().map(|error| error.to_string()),
    }
}

#[cfg(not(feature = "test-support"))]
fn unavailable_readiness(allow_cpu: bool, reason: &str) -> EmbeddingDeviceReadiness {
    EmbeddingDeviceReadiness {
        requested_policy: requested_policy(allow_cpu),
        observed_state: "unavailable",
        observation_source: "inprocess_engine",
        detected_provider: None,
        detected_gpu: None,
        accelerator_requested: !allow_cpu,
        accelerator_request_provider: None,
        accelerator_request_device: None,
        cpu_allowed: allow_cpu,
        full_retrieval_allowed: false,
        degraded_reason: Some(reason.to_string()),
    }
}

fn requested_policy(allow_cpu: bool) -> &'static str {
    if allow_cpu {
        "cpu_explicit"
    } else {
        "accelerator_required"
    }
}

#[cfg(not(feature = "test-support"))]
fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(policy: &'static str) -> ProcessEmbeddingIdentity {
        ProcessEmbeddingIdentity {
            instance_id: "test".into(),
            model_load_count: 1,
            model_digest: codestory_llama_sys::MODEL_SHA256,
            ggml_build_identity: codestory_llama_sys::GGML_BUILD_IDENTITY,
            backend: if policy == "accelerated" {
                "Metal"
            } else {
                "CPU"
            }
            .into(),
            adapter_name: if policy == "accelerated" {
                "Apple GPU"
            } else {
                "CPU"
            }
            .into(),
            adapter_description: "test".into(),
            policy,
            embedded_model: true,
            materialized_path: PathBuf::from("model.gguf"),
            materialized_reused: true,
            initialization_ms: 1,
            smoke_ms: 1,
            adapter_memory_total: 1,
            adapter_memory_used_by_load: 1,
            execution_device_names: if policy == "accelerated" {
                vec!["Apple GPU".into()]
            } else {
                Vec::new()
            },
            model_layer_count: 13,
            offloaded_layer_count: if policy == "accelerated" { 13 } else { 0 },
            accelerator_execution_verified: policy == "accelerated",
        }
    }

    #[test]
    fn accelerated_identity_requires_full_offload_proof() {
        let mut identity = identity("accelerated");
        assert!(validate_identity(&identity, false).is_ok());
        identity.offloaded_layer_count -= 1;
        assert!(validate_identity(&identity, false).is_err());
    }

    #[test]
    fn cpu_identity_is_accepted_only_under_explicit_policy() {
        let identity = identity("cpu_explicit");
        assert!(validate_identity(&identity, true).is_ok());
        assert!(validate_identity(&identity, false).is_err());
    }
}
