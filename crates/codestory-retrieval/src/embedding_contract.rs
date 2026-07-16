//! Product embedding policy layered over the native llama.cpp execution boundary.

use anyhow::{Result, bail};
use codestory_llama_sys::{
    EmbeddingEngineConfig, NativeBackendRequest, NativeDeviceClass, NativeEmbeddingPooling,
    NativeEmbeddingRequest, compiled_engine_capabilities,
};

pub const RETRIEVAL_EMBEDDING_DIM: usize = 768;
pub const CODERANK_QUERY_PREFIX: &str = "Represent this query for searching relevant code: ";
pub const CODERANK_DOCUMENT_PREFIX: &str = "";
#[cfg(test)]
pub const EMBEDDING_POOLING: &str = "cls";
#[cfg(test)]
pub const EMBEDDING_NORMALIZATION: &str = "l2";
#[cfg(test)]
pub const EMBEDDING_ELEMENT_TYPE: &str = "f32_le";
#[cfg(test)]
pub const EMBEDDING_VECTOR_SCHEMA_VERSION: u32 = 2;

const CONTEXT_TOKENS: u32 = 4096;
const MAX_INPUT_TOKENS: usize = 512;
const BATCH_TOKENS: u32 = 1024;
const MAX_BATCH_SEQUENCES: u32 = 6;

pub(crate) fn native_engine_config(allow_cpu: bool) -> Result<EmbeddingEngineConfig> {
    let capabilities = compiled_engine_capabilities();
    let (backend, device_class) = if allow_cpu {
        ("cpu", NativeDeviceClass::Cpu)
    } else {
        let backend = match capabilities.target_os {
            "macos" => "metal",
            "windows" | "linux" => "vulkan",
            unsupported => {
                bail!(
                    "embedding_backend_policy_unsupported_target: no accelerated backend policy for {unsupported}"
                )
            }
        };
        (backend, NativeDeviceClass::Accelerator)
    };
    if !capabilities
        .backends
        .iter()
        .any(|compiled| *compiled == backend)
    {
        bail!(
            "embedding_backend_policy_uncompiled: requested={backend} compiled={}",
            capabilities.backends.join(",")
        );
    }

    Ok(EmbeddingEngineConfig {
        backend: NativeBackendRequest {
            backend: backend.to_string(),
            device_class,
            reject_software_adapters: true,
        },
        embedding: NativeEmbeddingRequest {
            model_id: codestory_llama_sys::MODEL_FILE_NAME.to_string(),
            model_sha256: codestory_llama_sys::MODEL_SHA256.to_string(),
            dimension: RETRIEVAL_EMBEDDING_DIM,
            pooling: NativeEmbeddingPooling::Cls,
            context_tokens: CONTEXT_TOKENS,
            max_input_tokens: MAX_INPUT_TOKENS,
            batch_tokens: BATCH_TOKENS,
            max_batch_sequences: MAX_BATCH_SEQUENCES,
            smoke_input: format!("{CODERANK_QUERY_PREFIX}codestory embedding smoke"),
        },
    })
}

pub(crate) fn normalize_and_validate_vectors(mut vectors: Vec<Vec<f32>>) -> Result<Vec<Vec<f32>>> {
    for vector in &mut vectors {
        normalize_and_validate_vector(vector)?;
    }
    Ok(vectors)
}

fn normalize_and_validate_vector(vector: &mut [f32]) -> Result<()> {
    if vector.len() != RETRIEVAL_EMBEDDING_DIM {
        bail!(
            "embedding_vector_dimension_mismatch: expected={} observed={}",
            RETRIEVAL_EMBEDDING_DIM,
            vector.len()
        );
    }
    if vector.iter().any(|value| !value.is_finite()) {
        bail!("embedding_vector_non_finite: native engine returned a non-finite value");
    }
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if norm <= f64::EPSILON {
        bail!("embedding_vector_zero_norm: native engine returned an unusable vector");
    }
    let scale = (1.0 / norm) as f32;
    for value in vector {
        *value *= scale;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_owns_the_product_semantics_and_matches_the_compiled_model() {
        let compiled = codestory_llama_sys::PRODUCT_EMBEDDING_VECTOR_SEMANTICS;
        assert_eq!(RETRIEVAL_EMBEDDING_DIM, compiled.dimension());
        assert_eq!(EMBEDDING_POOLING, compiled.pooling_id());
        assert_eq!(EMBEDDING_NORMALIZATION, compiled.normalization_id());
        assert_eq!(
            CODERANK_QUERY_PREFIX,
            codestory_llama_sys::EMBEDDING_QUERY_PREFIX
        );
        assert_eq!(
            CODERANK_DOCUMENT_PREFIX,
            codestory_llama_sys::EMBEDDING_DOCUMENT_PREFIX
        );
        assert_eq!(
            EMBEDDING_ELEMENT_TYPE,
            codestory_llama_sys::EMBEDDING_ELEMENT_TYPE
        );
        assert_eq!(
            EMBEDDING_VECTOR_SCHEMA_VERSION,
            codestory_llama_sys::EMBEDDING_VECTOR_SCHEMA_VERSION
        );
    }

    #[test]
    fn backend_policy_is_explicit_and_never_uses_implicit_cpu_fallback() {
        let cpu = native_engine_config(true).expect("explicit CPU config");
        assert_eq!(cpu.backend.backend, "cpu");
        assert_eq!(cpu.backend.device_class, NativeDeviceClass::Cpu);

        let accelerated = native_engine_config(false).expect("accelerated config");
        assert_eq!(
            accelerated.backend.device_class,
            NativeDeviceClass::Accelerator
        );
        assert_ne!(accelerated.backend.backend, "cpu");
    }

    #[test]
    fn normalization_is_fail_closed_in_retrieval() {
        let mut vector = vec![0.0; RETRIEVAL_EMBEDDING_DIM];
        vector[0] = 3.0;
        vector[1] = 4.0;
        normalize_and_validate_vector(&mut vector).expect("normalize product vector");
        assert!((vector[0] - 0.6).abs() < f32::EPSILON);
        assert!((vector[1] - 0.8).abs() < f32::EPSILON);

        let mut zero = vec![0.0; RETRIEVAL_EMBEDDING_DIM];
        assert!(normalize_and_validate_vector(&mut zero).is_err());
        let mut wrong = vec![1.0; RETRIEVAL_EMBEDDING_DIM - 1];
        assert!(normalize_and_validate_vector(&mut wrong).is_err());
        let mut non_finite = vec![1.0; RETRIEVAL_EMBEDDING_DIM];
        non_finite[0] = f32::NAN;
        assert!(normalize_and_validate_vector(&mut non_finite).is_err());
    }
}
