use codestory_contracts::workspace::SourceIndexPolicy;
use codestory_retrieval::SidecarRuntimeConfig;

/// Immutable process-owned defaults injected into one runtime.
///
/// Adapters capture sidecar defaults and source-index policy once, then pass
/// this value through every retained project context. Other feature and
/// evaluation controls remain owned by their respective subsystems.
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
