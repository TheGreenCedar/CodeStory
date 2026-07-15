use anyhow::{Result, anyhow, bail};
use codestory_llama_sys::{EmbeddingEngine, ExecutionPolicy};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

type ProcessEngineState = Option<Arc<ProcessEmbeddingEngine>>;

static PROCESS_ENGINE: OnceLock<Mutex<ProcessEngineState>> = OnceLock::new();
static PROCESS_EXIT_HOOK: OnceLock<Result<(), String>> = OnceLock::new();
static MODEL_LOAD_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ProcessEmbeddingIdentity {
    pub instance_id: String,
    pub model_load_count: u64,
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

pub fn process_embedding_identity(
    cache_root: &Path,
    allow_cpu: bool,
) -> Result<ProcessEmbeddingIdentity> {
    let process = process_engine(cache_root, allow_cpu)?;
    Ok(identity_from_process(&process))
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
    Ok(Some(identity_from_process(process)))
}

pub fn embed_prepared_in_process(
    cache_root: &Path,
    allow_cpu: bool,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>> {
    process_engine(cache_root, allow_cpu)?
        .engine
        .embed_prepared(inputs)
        .map_err(Into::into)
}

pub fn embed_prepared_query_in_process(
    cache_root: &Path,
    allow_cpu: bool,
    input: String,
) -> Result<Vec<f32>> {
    process_engine(cache_root, allow_cpu)?
        .engine
        .embed_query_prepared(input)
        .map_err(Into::into)
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
        let engine = EmbeddingEngine::initialize(cache_root, allow_cpu)?;
        // Register only after llama.cpp has initialized its backend globals.
        // atexit callbacks run in reverse order, so this drops the live model
        // and context before ggml releases the selected Metal/Vulkan device.
        ensure_process_exit_hook()?;
        MODEL_LOAD_COUNT.fetch_add(1, Ordering::AcqRel);
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
    if process.allow_cpu != allow_cpu {
        bail!(
            "embedding engine policy changed after process initialization: initialized={} requested={}",
            policy_name(process.allow_cpu),
            policy_name(allow_cpu)
        );
    }
    if process.cache_root != cache_root {
        bail!(
            "embedding engine cache root changed after process initialization: initialized={} requested={}",
            process.cache_root.display(),
            cache_root.display()
        );
    }
    Ok(())
}

fn identity_from_process(process: &ProcessEmbeddingEngine) -> ProcessEmbeddingIdentity {
    let identity = process.engine.identity();
    ProcessEmbeddingIdentity {
        instance_id: process.instance_id.clone(),
        model_load_count: MODEL_LOAD_COUNT.load(Ordering::Acquire),
        model_digest: identity.model_digest,
        ggml_build_identity: identity.ggml_build_identity,
        backend: identity.backend.clone(),
        adapter_name: identity.adapter_name.clone(),
        adapter_description: identity.adapter_description.clone(),
        policy: identity.policy.as_str(),
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
        model_layer_count: identity.model_layer_count,
        offloaded_layer_count: identity.offloaded_layer_count,
        accelerator_execution_verified: identity.accelerator_execution_verified,
    }
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
        ExecutionPolicy::CpuExplicit.as_str()
    } else {
        ExecutionPolicy::Accelerated.as_str()
    }
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
