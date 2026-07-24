use codestory_contracts::workspace::SourceIndexPolicy;
use codestory_retrieval::SidecarRuntimeConfig;

/// Immutable process-owned defaults injected into one runtime.
///
/// Adapters capture ambient configuration once, then pass this value through
/// every retained project context. Runtime and lower layers never reread the
/// process environment.
#[derive(Debug, Clone)]
pub struct RuntimeProcessConfig {
    pub sidecar: SidecarRuntimeConfig,
    pub source_index_policy: SourceIndexPolicy,
}

impl RuntimeProcessConfig {
    pub fn new(sidecar: SidecarRuntimeConfig, source_index_policy: SourceIndexPolicy) -> Self {
        Self {
            sidecar,
            source_index_policy,
        }
    }

    pub fn local() -> Self {
        Self::new(SidecarRuntimeConfig::local(), SourceIndexPolicy::default())
    }
}
