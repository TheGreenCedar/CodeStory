use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::{
    ActivationService, FinalizeIndexOutcome, QueryResult, RetainedRollbackObservation,
    RollbackActivationError, RollbackActivationOutcome, RuntimeRetrievalConfig, SidecarGcReport,
    SidecarInventoryReport,
};

impl ActivationService {
    /// Observe the retained rollback pointer without validating or mutating it.
    pub fn observe_retained_rollback_generation(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> anyhow::Result<Option<RetainedRollbackObservation>> {
        codestory_retrieval::observe_retained_rollback_generation(
            project_root,
            storage_path,
            self.controller.runtime_config.as_ref(),
        )
    }

    /// Validate and optionally activate the retained rollback generation.
    pub fn activate_retained_rollback_generation(
        &self,
        project_root: &Path,
        storage_path: &Path,
        apply: bool,
    ) -> Result<RollbackActivationOutcome, RollbackActivationError> {
        codestory_retrieval::activate_retained_rollback_generation(
            project_root,
            storage_path,
            self.controller.runtime_config.as_ref(),
            apply,
        )
    }

    /// Observe immutable retrieval generations and the current retention plan.
    pub fn retrieval_inventory(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> anyhow::Result<SidecarInventoryReport> {
        codestory_retrieval::sidecar_inventory_with_storage(project_root, storage_path)
    }

    /// Apply the retrieval owner's bounded generation-retention plan.
    pub fn apply_retrieval_gc(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> anyhow::Result<SidecarGcReport> {
        codestory_retrieval::sidecar_gc_apply_with_storage(project_root, storage_path)
    }

    /// Execute one query with a fresh caller-isolated retrieval cache.
    pub fn execute_retrieval_query(
        &self,
        project_root: &Path,
        storage_path: &Path,
        query: &str,
        budget_ms: Option<u64>,
    ) -> anyhow::Result<QueryResult> {
        codestory_retrieval::execute_retrieval_query_with_cache_for_runtime(
            codestory_retrieval::QueryRequest {
                project_root,
                storage_path,
                query,
                budget_ms,
                cancelled: None,
            },
            &mut codestory_retrieval::RetrievalCache::new(),
            self.controller.runtime_config.as_ref(),
        )
    }

    /// Finalize retrieval artifacts for the adapter-selected runtime profile.
    pub fn finalize_retrieval_index_with_cancel(
        &self,
        project_root: &Path,
        storage_path: &Path,
        config: &RuntimeRetrievalConfig,
        cancelled: &AtomicBool,
    ) -> anyhow::Result<FinalizeIndexOutcome> {
        codestory_retrieval::finalize_index_for_runtime_with_cancel(
            project_root,
            storage_path,
            config.as_inner(),
            cancelled,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Runtime, RuntimeProcessConfig};
    use codestory_contracts::workspace::SourceIndexPolicy;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::tempdir;

    fn error_chain(error: &anyhow::Error) -> Vec<String> {
        error.chain().map(ToString::to_string).collect()
    }

    #[test]
    fn facade_preserves_observation_query_and_finalize_error_contracts() {
        let project = tempdir().expect("operation project");
        let storage = tempdir().expect("operation storage");
        let storage_path = storage.path().join("codestory.db");
        let raw = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
            Some(project.path()),
            codestory_retrieval::SidecarProfile::Local,
        );
        let runtime = Runtime::new_with_process_config(RuntimeProcessConfig::new(
            raw.clone(),
            SourceIndexPolicy::default(),
        ));
        let facade = runtime.activation_service();

        let direct_observation = codestory_retrieval::observe_retained_rollback_generation(
            project.path(),
            &storage_path,
            &raw,
        )
        .expect("direct observation");
        let facade_observation = facade
            .observe_retained_rollback_generation(project.path(), &storage_path)
            .expect("facade observation");
        assert_eq!(facade_observation, direct_observation);

        let direct_query = codestory_retrieval::execute_retrieval_query_with_cache_for_runtime(
            codestory_retrieval::QueryRequest {
                project_root: project.path(),
                storage_path: &storage_path,
                query: "missing",
                budget_ms: Some(37),
                cancelled: None,
            },
            &mut codestory_retrieval::RetrievalCache::new(),
            &raw,
        )
        .expect_err("missing storage must fail direct query");
        let facade_query = facade
            .execute_retrieval_query(project.path(), &storage_path, "missing", Some(37))
            .expect_err("missing storage must fail facade query");
        assert_eq!(error_chain(&facade_query), error_chain(&direct_query));

        let selected: RuntimeRetrievalConfig = raw.clone().into();
        let cancelled = AtomicBool::new(true);
        let direct_finalize = codestory_retrieval::finalize_index_for_runtime_with_cancel(
            project.path(),
            &storage_path,
            &raw,
            &cancelled,
        )
        .expect_err("cancelled direct finalize must fail");
        let facade_finalize = facade
            .finalize_retrieval_index_with_cancel(
                project.path(),
                &storage_path,
                &selected,
                &cancelled,
            )
            .expect_err("cancelled facade finalize must fail");
        assert_eq!(error_chain(&facade_finalize), error_chain(&direct_finalize));
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn facade_preserves_rollback_refusal_and_inventory_results() {
        let cache = tempdir().expect("operation cache");
        codestory_retrieval::with_test_cache_root(cache.path(), || {
            let project = tempdir().expect("operation project");
            let storage = tempdir().expect("operation storage");
            let storage_path = storage.path().join("codestory.db");
            let raw = codestory_retrieval::SidecarRuntimeConfig::for_project_profile(
                Some(project.path()),
                codestory_retrieval::SidecarProfile::Local,
            );
            let runtime = Runtime::new_with_process_config(RuntimeProcessConfig::new(
                raw.clone(),
                SourceIndexPolicy::default(),
            ));
            let facade = runtime.activation_service();

            let direct_refusal = codestory_retrieval::activate_retained_rollback_generation(
                project.path(),
                &storage_path,
                &raw,
                false,
            )
            .expect_err("missing publication must refuse direct activation");
            let facade_refusal = facade
                .activate_retained_rollback_generation(project.path(), &storage_path, false)
                .expect_err("missing publication must refuse facade activation");
            assert_eq!(facade_refusal.code(), direct_refusal.code());
            assert_eq!(facade_refusal.to_string(), direct_refusal.to_string());

            let direct_inventory =
                codestory_retrieval::sidecar_inventory_with_storage(project.path(), &storage_path)
                    .expect("direct inventory");
            let facade_inventory = facade
                .retrieval_inventory(project.path(), &storage_path)
                .expect("facade inventory");
            assert_eq!(facade_inventory, direct_inventory);

            let direct_gc =
                codestory_retrieval::sidecar_gc_apply_with_storage(project.path(), &storage_path)
                    .expect("direct gc");
            let facade_gc = facade
                .apply_retrieval_gc(project.path(), &storage_path)
                .expect("facade gc");
            assert_eq!(facade_gc, direct_gc);
        });
    }
}
