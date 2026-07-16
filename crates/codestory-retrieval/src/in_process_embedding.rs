use anyhow::{Result, anyhow, bail};
#[cfg(not(feature = "test-support"))]
use codestory_llama_sys::EmbeddingResidencyLease;
use codestory_llama_sys::{
    EmbeddingEngine, EngineError, EngineLifecycleSnapshot, NativeDeviceClass,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

type ProcessEngineState = Option<Arc<ProcessEmbeddingEngine>>;

static PROCESS_ENGINE: OnceLock<Mutex<ProcessEngineState>> = OnceLock::new();
static PROCESS_EXIT_HOOK: OnceLock<Result<(), String>> = OnceLock::new();

const BACKEND_POLICY_MISMATCH_REASON: &str = "embedding_backend_policy_mismatch";

#[derive(Debug, Clone)]
pub struct ProcessEmbeddingIdentity {
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
struct ProcessEmbeddingEngine {
    engine: EmbeddingEngine,
    cache_root: PathBuf,
    allow_cpu: bool,
    instance_id: String,
}

#[derive(Debug)]
#[cfg(not(feature = "test-support"))]
pub struct ProcessEmbeddingResidencyLease {
    _process: Arc<ProcessEmbeddingEngine>,
    _lease: EmbeddingResidencyLease,
    identity: ProcessEmbeddingIdentity,
}

#[cfg(not(feature = "test-support"))]
impl ProcessEmbeddingResidencyLease {
    pub fn identity(&self) -> &ProcessEmbeddingIdentity {
        &self.identity
    }
}

pub fn process_embedding_identity(
    cache_root: &Path,
    allow_cpu: bool,
) -> Result<ProcessEmbeddingIdentity> {
    let process = process_engine(cache_root, allow_cpu)?;
    let snapshot = process.engine.ensure_resident()?;
    identity_from_snapshot(&process, &snapshot)
}

/// Observes the process engine without starting it. Status and doctor surfaces
/// use this so a read cannot materialize the model or initialize an adapter.
pub fn process_embedding_identity_if_initialized(
    cache_root: &Path,
    allow_cpu: bool,
) -> Result<Option<ProcessEmbeddingIdentity>> {
    let Some(slot) = PROCESS_ENGINE.get() else {
        return Ok(None);
    };
    let state = slot
        .lock()
        .map_err(|_| anyhow!("embedding engine state mutex was poisoned"))?;
    let Some(process) = state.as_ref() else {
        return Ok(None);
    };
    validate_process_selection(process, cache_root, allow_cpu)?;
    let snapshot = process.engine.snapshot()?;
    Ok(Some(identity_from_snapshot(process, &snapshot)?))
}

#[cfg(not(feature = "test-support"))]
pub fn acquire_process_embedding_residency(
    cache_root: &Path,
    allow_cpu: bool,
) -> Result<ProcessEmbeddingResidencyLease> {
    let process = process_engine(cache_root, allow_cpu)?;
    let lease = process.engine.acquire_residency_lease()?;
    let identity = identity_from_snapshot(&process, lease.snapshot())?;
    Ok(ProcessEmbeddingResidencyLease {
        _process: process,
        _lease: lease,
        identity,
    })
}

pub fn embed_prepared_in_process(
    cache_root: &Path,
    allow_cpu: bool,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>> {
    let vectors = process_engine(cache_root, allow_cpu)?
        .engine
        .embed_prepared(inputs)
        .map_err(native_engine_error)?;
    crate::embedding_contract::normalize_and_validate_vectors(vectors)
}

pub fn embed_prepared_query_in_process(
    cache_root: &Path,
    allow_cpu: bool,
    input: String,
) -> Result<Vec<f32>> {
    let vector = process_engine(cache_root, allow_cpu)?
        .engine
        .embed_query_prepared(input)
        .map_err(native_engine_error)?;
    crate::embedding_contract::normalize_and_validate_vectors(vec![vector])?
        .pop()
        .ok_or_else(|| anyhow!("embedding_vector_missing: native engine returned no query vector"))
}

/// Stops the process-wide engine while Rust thread state is still live.
/// Executable entry points own this guard; the late C atexit hook is only a
/// last-resort leak boundary for abrupt exits.
pub fn shutdown_process_embedding_engine() {
    let Some(slot) = PROCESS_ENGINE.get() else {
        return;
    };
    if let Ok(mut state) = slot.lock() {
        state.take();
    }
}

fn process_engine(cache_root: &Path, allow_cpu: bool) -> Result<Arc<ProcessEmbeddingEngine>> {
    let slot = PROCESS_ENGINE.get_or_init(|| Mutex::new(None));
    let mut state = slot
        .lock()
        .map_err(|_| anyhow!("embedding engine state mutex was poisoned"))?;
    if state.is_none() {
        let config = crate::embedding_contract::native_engine_config(allow_cpu)?;
        let engine =
            EmbeddingEngine::initialize(cache_root, config).map_err(native_engine_error)?;
        // Register only after llama.cpp has initialized its backend globals.
        // atexit callbacks run in reverse order, so this drops the live model
        // and context before ggml releases the selected Metal/Vulkan device.
        ensure_process_exit_hook()?;
        *state = Some(Arc::new(ProcessEmbeddingEngine {
            engine,
            cache_root: cache_root.to_path_buf(),
            allow_cpu,
            instance_id: format!(
                "inprocess:{}:{}",
                std::process::id(),
                &codestory_llama_sys::MODEL_SHA256[..16]
            ),
        }));
    }
    let process = state
        .as_ref()
        .expect("embedding engine state initialized above")
        .clone();
    drop(state);
    validate_process_selection(&process, cache_root, allow_cpu)?;
    Ok(process)
}

fn validate_process_selection(
    process: &ProcessEmbeddingEngine,
    cache_root: &Path,
    allow_cpu: bool,
) -> Result<()> {
    validate_process_policy(process.allow_cpu, allow_cpu)?;
    if process.cache_root != cache_root {
        bail!(
            "embedding_cache_root_mismatch: initialized={} requested={}",
            process.cache_root.display(),
            cache_root.display()
        );
    }
    Ok(())
}

fn validate_process_policy(initialized_allow_cpu: bool, requested_allow_cpu: bool) -> Result<()> {
    if initialized_allow_cpu != requested_allow_cpu {
        bail!(
            "{BACKEND_POLICY_MISMATCH_REASON}: initialized={} requested={}",
            policy_name(initialized_allow_cpu),
            policy_name(requested_allow_cpu)
        );
    }
    Ok(())
}

fn identity_from_snapshot(
    process: &ProcessEmbeddingEngine,
    snapshot: &EngineLifecycleSnapshot,
) -> Result<ProcessEmbeddingIdentity> {
    let identity = &snapshot.identity;
    let policy = match identity.selected_device_class {
        NativeDeviceClass::Cpu => "cpu_explicit",
        NativeDeviceClass::Accelerator => "accelerated",
        NativeDeviceClass::Unknown => {
            bail!(
                "embedding_backend_device_class_unknown: native engine selected an unknown device class"
            )
        }
    };
    Ok(ProcessEmbeddingIdentity {
        instance_id: process.instance_id.clone(),
        load_generation: snapshot.load_generation,
        model_load_count: snapshot.model_load_count,
        residency: snapshot.residency.as_str(),
        worker_alive: snapshot.worker_alive,
        load_error: snapshot.load_error.clone(),
        model_digest: identity.model_digest,
        ggml_build_identity: identity.ggml_build_identity,
        backend: identity.backend.clone(),
        adapter_name: identity.adapter_name.clone(),
        adapter_description: identity.adapter_description.clone(),
        policy,
        embedded_model: identity.embedded_model,
        materialized_path: identity.materialized_path.clone(),
        materialized_reused: identity.materialized_reused,
        initialization_ms: duration_ms(identity.initialization_duration),
        smoke_ms: duration_ms(identity.smoke_duration),
        adapter_memory_total: identity.adapter_memory_total,
        adapter_memory_used_by_load: identity
            .adapter_memory_free_before_load
            .saturating_sub(identity.adapter_memory_free_after_load),
        execution_device_names: identity.execution_device_names.clone(),
        execution_backend_names: identity.execution_backend_names.clone(),
        execution_observation_source: identity.execution_observation_source,
        encode_count: identity.encode_count,
        execution_node_count: identity.execution_node_count,
        resident_accelerator_tensor_count: identity.resident_accelerator_tensor_count,
        resident_accelerator_tensor_bytes: identity.resident_accelerator_tensor_bytes,
        model_layer_count: identity.model_layer_count,
        offloaded_layer_count: identity.offloaded_layer_count,
        accelerator_execution_verified: identity.accelerator_execution_verified,
    })
}

fn ensure_process_exit_hook() -> Result<()> {
    PROCESS_EXIT_HOOK
        .get_or_init(|| {
            let status = unsafe { atexit(drop_process_engine_at_exit) };
            if status == 0 {
                Ok(())
            } else {
                Err(format!(
                    "failed to register embedding engine exit hook: {status}"
                ))
            }
        })
        .clone()
        .map_err(anyhow::Error::msg)
}

extern "C" fn drop_process_engine_at_exit() {
    preserve_process_engine_at_exit();
}

fn preserve_process_engine_at_exit() {
    let Some(slot) = PROCESS_ENGINE.get() else {
        return;
    };
    if let Ok(mut state) = slot.lock()
        && let Some(process) = state.take()
    {
        // Rust thread-local state may already be gone when C invokes atexit.
        // Normal executable shutdown clears the engine before this point; an
        // abrupt exit retains the allocation for the OS instead of running
        // channel destructors after the Rust runtime has been torn down.
        std::mem::forget(process);
    }
}

unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
}

fn policy_name(allow_cpu: bool) -> &'static str {
    if allow_cpu {
        "cpu_explicit"
    } else {
        "accelerated"
    }
}

fn native_engine_error(error: EngineError) -> anyhow::Error {
    anyhow!("{}: {error}", error.reason_code())
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_policy_mismatch_has_a_stable_machine_reason() {
        for (initialized, requested) in [(false, true), (true, false)] {
            let error = validate_process_policy(initialized, requested)
                .expect_err("changing process policy must fail closed");
            assert!(
                error
                    .to_string()
                    .starts_with(BACKEND_POLICY_MISMATCH_REASON),
                "unexpected policy error: {error:#}"
            );
        }
        validate_process_policy(false, false).expect("stable accelerated policy");
        validate_process_policy(true, true).expect("stable explicit CPU policy");
    }
}
