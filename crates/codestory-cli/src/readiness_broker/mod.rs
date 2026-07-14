mod gpu_proof;
mod machine_lock;
mod native_lease;
mod operations;
mod paths;
mod reconcile;
mod scope;
mod snapshot;
mod types;

#[cfg(test)]
mod tests;

// Re-export the full former flat-module pub(crate) surface for callers.
#[allow(unused_imports)]
pub(crate) use gpu_proof::gpu_proof;
#[cfg(test)]
pub(crate) use machine_lock::try_acquire_machine_resource_lock;
#[allow(unused_imports)]
pub(crate) use machine_lock::{
    BrokerMachineResourceBusy, BrokerMachineResourceLock, BrokerMachineResourceLockAttempt,
    NATIVE_EMBEDDING_RESOURCE,
};
#[allow(unused_imports)]
pub(crate) use native_lease::{
    BrokerNativeEmbeddingResourceLease, NativeEmbeddingLeaseLifecycleParams,
    cleanup_native_embedding_resource_lease_after_transfer_error,
    native_embedding_owner_down_command, reusable_native_embedding_resource_pid_for_snapshot,
    run_with_native_embedding_lease_lifecycle, sidecar_down_with_native_embedding_handoff,
    transfer_native_embedding_resource_lease,
};
pub(crate) use paths::machine_resource_cache_fingerprint;
#[allow(unused_imports)]
pub(crate) use reconcile::{reconcile_before_enqueue, reconcile_before_enqueue_for_sidecar};
#[allow(unused_imports)]
pub(crate) use scope::{
    BROKER_SCHEMA_VERSION, agent_repair_scope, broker_operation_id, operation_scope,
};
#[allow(unused_imports)]
pub(crate) use snapshot::{
    observe_broker_snapshot, observe_broker_snapshot_for_sidecar, refresh_broker_snapshot,
    refresh_broker_snapshot_for_sidecar,
};
#[allow(unused_imports)]
pub(crate) use types::{
    BrokerGpuProofInput, BrokerGpuProofSnapshot, BrokerGpuRuntimeIdentity, BrokerOperationSnapshot,
    BrokerReconciliationSnapshot, BrokerResourceSnapshot, BrokerScope, BrokerSnapshotInput,
    ReadinessBrokerSnapshot,
};
